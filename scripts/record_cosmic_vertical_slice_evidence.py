#!/usr/bin/env python3
"""Run the reviewed COSMIC vertical slice and register schema-v2 evidence."""
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

import jsonschema

import qualify_candidate_known_phrase as qualifier

ROOT = Path(__file__).resolve().parents[1]
ARTIFACT = "artifacts/releases/narrivy-linux-amd64.deb"
FIXTURE = "tests/fixtures/jfk16.wav"
EVIDENCE = "agent/evidence/P6/VER-TEST-009.json"
INDEX = "agent/evidence/INDEX.json"
QUALIFIER = "scripts/qualify_candidate_known_phrase.py"
QUALIFIER_SHA256 = "6c3d9ad93dea7681bad7bb6726c69608444140dc80eda16299b8ec1709c408be"
FIXTURE_SHA256 = "59dfb9a4acb36fe2a2affc14bacbee2920ff435cb13cc314a08c13f66ba7860e"
PROCEDURE_ID = "cosmic-physical-mic-local-asr-inbox-v1"
PROFILE_ID = "native-behavior-v1"
EXPECTED_ARGV = [
    "python3",
    "scripts/record_cosmic_vertical_slice_evidence.py",
    ARTIFACT,
    FIXTURE,
    EVIDENCE,
    "--confirm-audible-playback",
]
ASSERTIONS = [
    "exact-package-source-bound",
    "physical-microphone-capture",
    "local-asr-known-phrase",
    "clean-pipeline-completed",
    "inbox-fallback-delivered",
    "transcript-absent-from-ordinary-logs",
    "daemon-clean-shutdown",
    "transient-state-removed",
]


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def fail(message: str) -> int:
    print(f"record_cosmic_vertical_slice_evidence: {message}", file=sys.stderr)
    return 1


def os_release() -> tuple[str, str]:
    values: dict[str, str] = {}
    try:
        for line in Path("/etc/os-release").read_text().splitlines():
            if "=" not in line:
                continue
            key, value = line.split("=", 1)
            values[key] = value.strip().strip('"')
    except OSError:
        pass
    return values.get("NAME", "Linux"), values.get("PRETTY_NAME", platform.release())


def atomic_json(path: Path, value: object, mode: int) -> None:
    payload = (json.dumps(value, indent=2, sort_keys=True) + "\n").encode("utf-8")
    temporary = path.with_name(f".{path.name}.tmp-{os.getpid()}")
    descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, mode)
    try:
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(payload)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
    finally:
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass


def main() -> int:
    if sys.argv != EXPECTED_ARGV[1:]:
        return fail("argv does not match the reviewed procedure")
    artifact = ROOT / ARTIFACT
    fixture = ROOT / FIXTURE
    evidence_path = ROOT / EVIDENCE
    index_path = ROOT / INDEX
    qualifier_path = ROOT / QUALIFIER
    if evidence_path.exists():
        return fail("evidence output already exists")
    if sha256(qualifier_path) != QUALIFIER_SHA256:
        return fail("primary qualifier hash does not match the reviewed adapter")
    if sha256(fixture) != FIXTURE_SHA256:
        return fail("fixture hash mismatch")
    artifact_hash = sha256(artifact)
    started = datetime.now(timezone.utc)
    with tempfile.TemporaryDirectory(prefix="narrivy-evidence-") as temporary:
        result_path = Path(temporary) / "known-phrase-result.json"
        result = qualifier.qualify(
            qualifier.Inputs(
                candidate=artifact.resolve(),
                fixture=fixture.resolve(),
                result=result_path.resolve(),
                expected_candidate_sha256=artifact_hash,
                expected_fixture_sha256=FIXTURE_SHA256,
                confirm_audible_playback=True,
                source_root=ROOT,
            )
        )
    if result.get("result") != "passed":
        return fail(f"native qualifier failed at {result.get('failed_step')}")
    observations = result.get("observations")
    cleanup = result.get("cleanup")
    if not isinstance(observations, dict) or not isinstance(cleanup, dict):
        return fail("native qualifier returned an incomplete result")
    if not all(cleanup.values()):
        return fail("native qualifier did not remove transient state")
    ended = datetime.now(timezone.utc)
    os_name, os_version = os_release()
    source = result["source"]
    environment = result["environment"]
    evidence = {
        "schema_version": 2,
        "verification_id": "VER-TEST-009",
        "requirement_ids": ["TEST-009"],
        "phase": "P6",
        "scenario_id": "cosmic-physical-mic-local-asr-clean-inbox-v1",
        "git_commit": source["head"],
        "git_dirty": source["git_dirty"],
        "build_id": artifact.name,
        "package_sha256": artifact_hash,
        "environment": {
            "id": "cosmic-wayland-native-physical-microphone",
            "native_or_vm": environment["native_or_vm"],
            "os": os_name,
            "os_version": os_version,
            "kernel": environment["kernel"],
            "desktop": environment["desktop"],
            "session": environment["session"],
            "cpu": platform.machine(),
            "gpu": None,
            "npu": None,
            "device_details": "physical default audio source",
            "microphone": environment["microphone_class"],
            "input_backend": "PipeWire/PulseAudio compatibility",
            "display_scale": None,
            "qualification_lane": "cosmic-wayland-physical-microphone-local-asr-inbox",
            "lane_available": True,
        },
        "command": shlex.join(EXPECTED_ARGV),
        "scenario_version": "1",
        "scenario_hash": sha256(Path(__file__)),
        "started_at": started.isoformat(),
        "ended_at": ended.isoformat(),
        "exit_code": 0,
        "result": "pass",
        "pass": True,
        "metrics": {
            "capture_seconds": observations["capture_seconds"],
            "inbox_files": observations["inbox_files"],
            "fragment_matched": observations["fragment_matched"],
            "log_bytes_scanned": observations["log_bytes_scanned"],
            "log_fragment_hits": observations["log_fragment_hits"],
            "transcript_utf8_bytes": observations["transcript_utf8_bytes"],
            "transcript_sha256": observations["transcript_sha256"],
        },
        "artifact_hashes": {
            ARTIFACT: artifact_hash,
            FIXTURE: FIXTURE_SHA256,
            QUALIFIER: QUALIFIER_SHA256,
            "scripts/record_cosmic_vertical_slice_evidence.py": sha256(Path(__file__)),
        },
        "artifact_under_test": {
            "kind": "package",
            "path": ARTIFACT,
            "sha256": artifact_hash,
        },
        "procedure": {
            "id": PROCEDURE_ID,
            "profile_id": PROFILE_ID,
            "assertions": [{"id": item, "result": "pass"} for item in ASSERTIONS],
        },
        "notes": (
            "Exact packaged vertical slice: physical microphone, local ASR, Clean "
            "processing and durable Inbox fallback. This does not claim external "
            "target insertion or complete Linux 1.0 release qualification."
        ),
        "evidence_type": "native-platform",
        "execution": {
            "lane_available": True,
            "child_failures_ignored": observations["child_failures_ignored"],
            "environment_class": "native-platform",
            "substitutions": [],
        },
    }
    evidence_schema = json.loads((ROOT / "spec/schemas/evidence.schema.json").read_text())
    jsonschema.validate(instance=evidence, schema=evidence_schema)
    index = json.loads(index_path.read_text())
    if any(item.get("path") == EVIDENCE for item in index["qualified_evidence"]):
        return fail("evidence index already contains the output path")
    evidence_payload = (json.dumps(evidence, indent=2, sort_keys=True) + "\n").encode()
    index["qualified_evidence"].append(
        {
            "path": EVIDENCE,
            "sha256": hashlib.sha256(evidence_payload).hexdigest(),
            "disposition": "qualified",
        }
    )
    index_schema = json.loads((ROOT / "spec/schemas/evidence-index.schema.json").read_text())
    jsonschema.validate(instance=index, schema=index_schema)
    atomic_json(evidence_path, evidence, 0o644)
    atomic_json(index_path, index, 0o644)
    print(
        json.dumps(
            {
                "artifact_sha256": artifact_hash,
                "evidence": EVIDENCE,
                "evidence_sha256": hashlib.sha256(evidence_payload).hexdigest(),
                "result": "pass",
            },
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
