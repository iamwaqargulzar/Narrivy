#!/usr/bin/env python3
"""Run the reviewed Debian lifecycle and register VER-OPS-002 evidence."""
from __future__ import annotations

import hashlib
import json
import platform
import shlex
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path

import jsonschema

import qualify_candidate as lifecycle
import qualify_candidate_known_phrase as package_identity
import recorder_safety

ROOT = Path(__file__).resolve().parents[1]
ARTIFACT = "artifacts/releases/narrivy-linux-amd64.deb"
EVIDENCE = "agent/evidence/P10/VER-OPS-002.json"
INDEX = "agent/evidence/INDEX.json"
LIFECYCLE_RUNNER = "scripts/qualify_candidate.py"
LIFECYCLE_SHA256 = "215b9edb1e2a97e77a8d2519c8103f429d8e78e11c07d1c2e47675d37383affb"
IDENTITY_RUNNER = "scripts/qualify_candidate_known_phrase.py"
IDENTITY_SHA256 = "881e3f12bd04a5f3ee01b86dac3ff7daa82e0191a2a5922cbb8e4e35752ea30b"
MODEL_HELPER = "scripts/qualify_installed_model.py"
MODEL_HELPER_SHA256 = "9de831caa27e875c68c3f5a43811c9b714b4356741d3a7f77dbec0f874b32fdf"
STATUS_HELPER = "scripts/probe_installed_status.py"
STATUS_HELPER_SHA256 = "d4e3c31fecde6924e3e2cc8642bf2120ef387add6f781487dc41adc39d1ce493"
PROCEDURE_ID = "ubuntu-deb-install-runtime-remove-v1"
PROFILE_ID = "package-artifact-v1"
EXPECTED_ARGV = [
    "python3",
    "scripts/record_package_lifecycle_evidence.py",
    ARTIFACT,
    EVIDENCE,
]
ASSERTIONS = [
    "exact-clean-package-bound",
    "apt-install-passed",
    "installed-paths-passed",
    "offline-model-provisioned",
    "configured-pipeline-ready",
    "bounded-daemon-restart-passed",
    "package-remove-purge-passed",
    "model-preservation-policy-passed",
    "owned-container-removed",
]


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def fail(message: str) -> int:
    print(f"record_package_lifecycle_evidence: {message}", file=sys.stderr)
    return 1


def main() -> int:
    if sys.argv != EXPECTED_ARGV[1:]:
        return fail("argv does not match the reviewed procedure")
    evidence_path = ROOT / EVIDENCE
    artifact = ROOT / ARTIFACT
    pinned = {
        LIFECYCLE_RUNNER: LIFECYCLE_SHA256,
        IDENTITY_RUNNER: IDENTITY_SHA256,
        MODEL_HELPER: MODEL_HELPER_SHA256,
        STATUS_HELPER: STATUS_HELPER_SHA256,
    }
    index_path = ROOT / INDEX
    index = json.loads(index_path.read_text())
    if not recorder_safety.replaceable(evidence_path, index, ROOT):
        return fail("evidence output is current, unindexed v2, or does not match its INDEX hash")
    for relative, expected in pinned.items():
        if sha256(ROOT / relative) != expected:
            return fail(f"reviewed helper hash mismatch: {relative}")
    try:
        source_root, source_head = package_identity._require_clean_source(
            ROOT, package_identity.HostOperations(), 5.0
        )
        if source_root != ROOT:
            return fail("source root mismatch")
        package_payload = package_identity._read_regular_file(
            artifact, package_identity.MAX_CANDIDATE_BYTES, "candidate"
        )
        identity, _members = package_identity._prepare_package(package_payload, source_head)
    except (package_identity.InputError, OSError) as error:
        return fail(str(error))
    artifact_hash = hashlib.sha256(package_payload).hexdigest()
    del package_payload
    started = datetime.now(timezone.utc)
    with tempfile.TemporaryDirectory(prefix="narrivy-package-evidence-") as temporary:
        result_path = Path(temporary) / "lifecycle.json"
        result = lifecycle.qualify(
            artifact.resolve(),
            artifact_hash,
            result_path.resolve(),
            lifecycle.DockerRunner(),
        )
    if result.get("result") != "passed":
        return fail(f"package lifecycle failed at {result.get('failed_step')}")
    if result.get("container", {}).get("removed") is not True:
        return fail("owned container removal was not confirmed")
    steps = {item.get("step") for item in result.get("steps", []) if item.get("status") == "passed"}
    required_steps = {
        "apt_install_package",
        "installed_paths",
        "installed_runtime_binding",
        "offline_provision",
        "model_cache",
        "sigterm_shutdown",
        "restart_ready",
        "restart_stop",
        "remove",
        "remove_preserves_model",
        "purge",
        "purge_preserves_model",
        "container_remove",
    }
    if not required_steps.issubset(steps):
        return fail("package lifecycle omitted a required step")
    ended = datetime.now(timezone.utc)
    evidence = {
        "schema_version": 2,
        "verification_id": "VER-OPS-002",
        "requirement_ids": ["OPS-002"],
        "phase": "P10",
        "scenario_id": "ubuntu-24-04-deb-install-runtime-remove-v1",
        "git_commit": source_head,
        "git_dirty": False,
        "build_id": identity.version,
        "package_sha256": artifact_hash,
        "environment": {
            "id": "ubuntu-24.04-disposable-package-lifecycle",
            "native_or_vm": "controlled",
            "os": "Ubuntu",
            "os_version": "24.04",
            "kernel": platform.release(),
            "desktop": "none",
            "session": "container",
            "cpu": platform.machine(),
            "gpu": None,
            "npu": None,
            "device_details": "fresh test-owned Docker container",
            "microphone": None,
            "input_backend": None,
            "display_scale": None,
            "qualification_lane": "ubuntu-24.04-disposable-package-lifecycle",
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
            "passed_steps": len(steps),
            "initial_shutdown_seconds": result["shutdown"]["seconds"],
            "restart_shutdown_seconds": result["restart_stop"]["seconds"],
            "model_sha256": result["model_cache"]["sha256"],
            "model_preserved_after_remove": result["model_cache"]["preserved_after_remove"],
            "model_preserved_after_purge": result["model_cache"]["preserved_after_purge"],
            "configured_pipeline": result["restart"]["pipeline"],
            "configured_pipeline_enabled": result["restart"]["pipeline_enabled"],
        },
        "artifact_hashes": {
            ARTIFACT: artifact_hash,
            **pinned,
            "scripts/record_package_lifecycle_evidence.py": sha256(Path(__file__)),
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
            "Fresh Ubuntu 24.04 apt install, fixed installed paths, offline model "
            "provisioning, ready configured pipeline, bounded daemon restart, remove "
            "and purge were exercised. The model-preservation policy passed. This "
            "controlled lane does not claim a native desktop or systemd user-service run."
        ),
        "evidence_type": "package-artifact",
        "execution": {
            "lane_available": True,
            "child_failures_ignored": False,
            "environment_class": "release-package",
            "substitutions": [],
        },
    }
    jsonschema.validate(
        instance=evidence,
        schema=json.loads((ROOT / "spec/schemas/evidence.schema.json").read_text()),
    )
    payload = (json.dumps(evidence, indent=2, sort_keys=True) + "\n").encode()
    index_schema = json.loads((ROOT / "spec/schemas/evidence-index.schema.json").read_text())
    try:
        recorder_safety.commit_records(
            ROOT, index_path, index, [(evidence_path, payload)],
            validate_index=lambda value: jsonschema.validate(instance=value, schema=index_schema),
        )
    except (OSError, recorder_safety.RecorderSafetyError, jsonschema.ValidationError) as error:
        return fail(str(error))
    print(
        json.dumps(
            {
                "artifact_sha256": artifact_hash,
                "evidence": EVIDENCE,
                "evidence_sha256": hashlib.sha256(payload).hexdigest(),
                "result": "pass",
            },
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
