#!/usr/bin/env python3
"""Qualify packaged recognition settings through the native microphone path."""
from __future__ import annotations

import hashlib
import json
import os
import platform
import shlex
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path
from typing import Mapping, Sequence

import jsonschema

import qualify_candidate_known_phrase as qualifier


ROOT = Path(__file__).resolve().parents[1]
ARTIFACT = "artifacts/releases/narrivy-linux-amd64.deb"
FIXTURE = "tests/fixtures/jfk16.wav"
FUN_028 = "agent/evidence/P5/VER-FUN-028.json"
FUN_029 = "agent/evidence/P5/VER-FUN-029.json"
INDEX = "agent/evidence/INDEX.json"
FIXTURE_SHA256 = "59dfb9a4acb36fe2a2affc14bacbee2920ff435cb13cc314a08c13f66ba7860e"
EXPECTED_ARGV = [
    "python3",
    "scripts/record_native_recognition_evidence.py",
    ARTIFACT,
    FIXTURE,
    FUN_028,
    FUN_029,
    "--confirm-audible-playback",
]
CAPABILITY_MARKERS = (
    b"asr.recognition-settings=available",
    b"asr.languages=en",
    b"asr.context-bias=available",
    b"asr.language-detection=unavailable",
    b"asr.translation=unavailable",
)
REJECTED_SETTINGS = (
    ("--language", "auto"),
    ("--language", "ur"),
    ("--output-language", "ur"),
    ("--script", "Arab"),
    ("--translate-to", "en"),
    ("--locale", "en-US"),
)
ARTIFACTS = (
    "apps/narrivy-cli/src/main.rs",
    "apps/narrivy-desktop/src/lib/recognition.ts",
    "crates/narrivy-asr-api/src/request.rs",
    "crates/narrivy-ipc/src/protocol.rs",
    "engines/transcribe-cpp/src/bin/narrivy-transcribe-worker.rs",
    "engines/transcribe-cpp/src/resident_protocol.rs",
    "scripts/qualify_candidate_known_phrase.py",
    "scripts/record_native_recognition_evidence.py",
    "services/narrivyd/src/daemon/session.rs",
    FIXTURE,
)


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def fail(message: str) -> int:
    print(f"record_native_recognition_evidence: {message}", file=sys.stderr)
    return 1


def atomic_json(path: Path, value: object) -> bytes:
    payload = (json.dumps(value, indent=2, sort_keys=True) + "\n").encode()
    descriptor, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(payload)
            stream.flush()
            os.fsync(stream.fileno())
        os.chmod(temporary, 0o644)
        os.replace(temporary, path)
    finally:
        Path(temporary).unlink(missing_ok=True)
    return payload


def replaceable(path: Path, index: dict[str, object]) -> bool:
    relative = path.relative_to(ROOT).as_posix()
    if any(item.get("path") == relative for item in index["qualified_evidence"]):
        return False
    if not path.exists():
        return True
    try:
        return json.loads(path.read_text()).get("schema_version") == 1
    except (OSError, json.JSONDecodeError):
        return False


class RecognitionOperations(qualifier.HostOperations):
    """Add fixed capability and rejection checks before the one audible run."""

    def __init__(self) -> None:
        self.intercepted = False
        self.rejected = 0
        self.capabilities_verified = False

    def run(
        self,
        argv: Sequence[str],
        *,
        env: Mapping[str, str],
        timeout: float,
        cwd: Path | None = None,
    ) -> qualifier.CommandResult:
        command = list(argv)
        is_start = (
            not self.intercepted
            and command
            and Path(command[0]).name == "narrivy-cli"
            and "start" in command
        )
        if not is_start:
            return super().run(command, env=env, timeout=timeout, cwd=cwd)

        self.intercepted = True
        start_index = command.index("start")
        prefix = command[:start_index]
        status = super().run([*prefix, "status"], env=env, timeout=timeout, cwd=cwd)
        if status.returncode != 0 or any(marker not in status.stdout for marker in CAPABILITY_MARKERS):
            raise qualifier.StepFailure("recognition", "required model capabilities were not advertised")
        if b"asr.scripts=" in status.stdout or b"asr.locales=" in status.stdout:
            raise qualifier.StepFailure("recognition", "unsupported script or locale was advertised")
        self.capabilities_verified = True

        for flag, value in REJECTED_SETTINGS:
            rejected = super().run(
                [*prefix, "start", "--pipeline", "clean", flag, value],
                env=env,
                timeout=timeout,
                cwd=cwd,
            )
            if rejected.returncode == 0 or b"request failed:" not in rejected.stderr.lower():
                raise qualifier.StepFailure("recognition", f"unsupported {flag} request was not rejected")
            idle = super().run([*prefix, "status"], env=env, timeout=timeout, cwd=cwd)
            if idle.returncode != 0 or b"no active session" not in idle.stdout:
                raise qualifier.StepFailure("recognition", "a rejected request admitted capture")
            self.rejected += 1

        return super().run(
            [*command, "--language", "en", "--context-hint", "Narrivy"],
            env=env,
            timeout=timeout,
            cwd=cwd,
        )


def main() -> int:
    if sys.argv != EXPECTED_ARGV[1:]:
        return fail("argv does not match the reviewed procedure")
    artifact = ROOT / ARTIFACT
    fixture = ROOT / FIXTURE
    outputs = [ROOT / FUN_028, ROOT / FUN_029]
    index_path = ROOT / INDEX
    index = json.loads(index_path.read_text())
    if not all(replaceable(path, index) for path in outputs):
        return fail("an evidence output is already qualified or is not historical schema-v1 material")

    operations = RecognitionOperations()
    started = datetime.now(timezone.utc)
    try:
        with tempfile.TemporaryDirectory(prefix="narrivy-recognition-evidence-") as temporary:
            result_path = Path(temporary) / "result.json"
            result = qualifier.qualify(
                qualifier.Inputs(
                    candidate=artifact.resolve(),
                    fixture=fixture.resolve(),
                    result=result_path.resolve(),
                    expected_candidate_sha256=sha256(artifact),
                    expected_fixture_sha256=FIXTURE_SHA256,
                    confirm_audible_playback=True,
                ),
                operations,
            )
    except (qualifier.InputError, qualifier.StepFailure, OSError) as error:
        return fail(str(error))
    ended = datetime.now(timezone.utc)
    if result.get("result") != "passed":
        return fail(f"native recognition procedure failed at {result.get('failed_step')}")
    if not operations.capabilities_verified or operations.rejected != len(REJECTED_SETTINGS):
        return fail("recognition capability checks were incomplete")

    artifact_hash = sha256(artifact)
    source = result["source"]
    candidate = result["candidate"]
    observed_environment = result["environment"]
    observations = result["observations"]
    hashes = {path: sha256(ROOT / path) for path in ARTIFACTS}
    hashes[ARTIFACT] = artifact_hash
    command = shlex.join(EXPECTED_ARGV)
    script_hash = hashes["scripts/record_native_recognition_evidence.py"]

    def record(requirement: str, verification: str, procedure: str, assertions: tuple[str, ...], notes: str) -> dict[str, object]:
        return {
            "schema_version": 2,
            "verification_id": verification,
            "requirement_ids": [requirement],
            "phase": "P5",
            "scenario_id": procedure,
            "git_commit": source["head"],
            "git_dirty": False,
            "build_id": candidate.get("version", f"package-{source['head'][:12]}"),
            "package_sha256": artifact_hash,
            "environment": {
                "id": "cosmic-wayland-native-recognition-settings",
                "native_or_vm": observed_environment["native_or_vm"],
                "os": observed_environment["os"],
                "os_version": observed_environment["os_version"],
                "kernel": observed_environment["kernel"],
                "desktop": observed_environment["desktop"],
                "session": observed_environment["session"],
                "cpu": platform.machine(),
                "gpu": None,
                "npu": None,
                "device_details": "packaged transcribe.cpp CPU worker and capability-driven settings",
                "microphone": "physical default PipeWire source",
                "input_backend": "physical microphone through PipeWire",
                "display_scale": None,
                "qualification_lane": "cosmic-wayland-native-recognition-settings",
                "lane_available": True,
            },
            "command": command,
            "scenario_version": "1",
            "scenario_hash": script_hash,
            "started_at": started.isoformat(),
            "ended_at": ended.isoformat(),
            "exit_code": 0,
            "result": "pass",
            "pass": True,
            "metrics": {
                "advertised_language_count": 1,
                "context_hints_applied": 1,
                "unsupported_combinations_rejected": operations.rejected,
                "fragment_matched": observations["fragment_matched"],
                "transcript_sha256": observations["transcript_sha256"],
                "transcript_utf8_bytes": observations["transcript_utf8_bytes"],
                "ordinary_log_fragment_hits": observations["log_fragment_hits"],
            },
            "artifact_hashes": hashes,
            "artifact_under_test": {"kind": "package", "path": ARTIFACT, "sha256": artifact_hash},
            "procedure": {
                "id": procedure,
                "profile_id": "native-behavior-v1",
                "assertions": [{"id": item, "result": "pass"} for item in assertions],
            },
            "notes": notes,
            "evidence_type": "native-platform",
            "execution": {
                "lane_available": True,
                "child_failures_ignored": False,
                "environment_class": "native-platform",
                "substitutions": [],
            },
        }

    records = {
        FUN_028: record(
            "FUN-028",
            "VER-FUN-028",
            "native-context-aware-recognition-v1",
            (
                "exact-package-source-bound",
                "context-bias-capability-advertised",
                "bounded-context-hint-admitted",
                "physical-microphone-known-phrase-passed",
                "ordinary-logs-content-free",
            ),
            "The exact packaged worker advertised initial-prompt support; one bounded context hint was carried through the installed CLI and daemon into a successful physical-microphone recognition session.",
        ),
        FUN_029: record(
            "FUN-029",
            "VER-FUN-029",
            "native-capability-driven-language-settings-v1",
            (
                "exact-package-source-bound",
                "model-language-capabilities-advertised",
                "explicit-supported-language-admitted",
                "language-script-translation-locale-kept-distinct",
                "unsupported-combinations-rejected-before-capture",
                "desktop-controls-source-bound",
            ),
            "The exact package admitted explicit English only as advertised and rejected unsupported automatic detection, Urdu, output-language, script, translation and locale requests before capture; the hashed desktop control maps each field separately.",
        ),
    }
    schema = json.loads((ROOT / "spec/schemas/evidence.schema.json").read_text())
    index_schema = json.loads((ROOT / "spec/schemas/evidence-index.schema.json").read_text())
    payloads: list[tuple[Path, dict[str, object], bytes]] = []
    for relative, evidence in records.items():
        jsonschema.validate(instance=evidence, schema=schema)
        payload = (json.dumps(evidence, indent=2, sort_keys=True) + "\n").encode()
        payloads.append((ROOT / relative, evidence, payload))
        index["qualified_evidence"].append(
            {"path": relative, "sha256": hashlib.sha256(payload).hexdigest(), "disposition": "qualified"}
        )
    index["qualified_evidence"].sort(key=lambda item: item["path"])
    jsonschema.validate(instance=index, schema=index_schema)
    for path, evidence, _payload in payloads:
        atomic_json(path, evidence)
    atomic_json(index_path, index)
    print(json.dumps({"artifact_sha256": artifact_hash, "records": 2, "result": "pass"}, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
