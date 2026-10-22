#!/usr/bin/env python3
"""Qualify exact .deb upgrade/rollback behavior and record VER-OPS-003."""
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

import qualify_candidate_known_phrase as package_identity
import qualify_candidate_upgrade as upgrade

ROOT = Path(__file__).resolve().parents[1]
CURRENT = "artifacts/releases/narrivy-linux-amd64.deb"
PREVIOUS = "artifacts/releases/narrivy-previous-amd64.deb"
EVIDENCE = "agent/evidence/P10/VER-OPS-003.json"
INDEX = "agent/evidence/INDEX.json"
HELPER = "scripts/qualify_candidate_upgrade.py"
HELPER_SHA256 = "39a2d085e7fe7d46bee21f9793a831a9890072833fcde915b29006fa4c0dbb0f"
PROCEDURE_ID = "ubuntu-deb-upgrade-rollback-v1"
EXPECTED_ARGV = ["python3", "scripts/record_package_upgrade_evidence.py", CURRENT, PREVIOUS]
ASSERTIONS = [
    "exact-clean-package-bound",
    "previous-version-installed",
    "current-version-upgrade-passed",
    "user-xdg-data-preserved-after-upgrade",
    "explicit-package-rollback-passed",
    "user-xdg-data-preserved-after-rollback",
    "current-version-reupgrade-passed",
    "remove-and-purge-removed-program-files",
    "remove-and-purge-preserved-user-xdg-data",
    "owned-container-removed",
]
REQUIRED_STEPS = {
    "apt_install_previous", "create_sentinels", "apt_upgrade_current", "confirm_upgrade",
    "apt_rollback_previous", "confirm_rollback", "apt_reupgrade_current",
    "confirm_reupgrade", "apt_remove", "confirm_remove", "apt_purge",
    "confirm_purge", "container_remove",
}


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def fail(message: str) -> int:
    print(f"record_package_upgrade_evidence: {message}", file=sys.stderr)
    return 1


def atomic_json(path: Path, value: object) -> bytes:
    payload = (json.dumps(value, indent=2, sort_keys=True) + "\n").encode()
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(payload); stream.flush(); os.fsync(stream.fileno())
        os.chmod(temporary, 0o644); os.replace(temporary, path)
    finally:
        Path(temporary).unlink(missing_ok=True)
    return payload


def main() -> int:
    if sys.argv != EXPECTED_ARGV[1:]:
        return fail("argv does not match the reviewed procedure")
    if sha256(ROOT / HELPER) != HELPER_SHA256:
        return fail("reviewed upgrade helper hash mismatch")
    evidence_path = ROOT / EVIDENCE
    index_path = ROOT / INDEX
    index = json.loads(index_path.read_text())
    if any(item.get("path") == EVIDENCE for item in index["qualified_evidence"]):
        return fail("evidence index already contains the output path")
    if evidence_path.exists():
        try:
            if json.loads(evidence_path.read_text()).get("schema_version") != 1:
                return fail("evidence output is not replaceable historical material")
        except (OSError, json.JSONDecodeError):
            return fail("evidence output is unsafe")
    try:
        source_root, head = package_identity._require_clean_source(
            ROOT, package_identity.HostOperations(), 5.0
        )
        if source_root != ROOT:
            raise RuntimeError("source root mismatch")
        current_payload = package_identity._read_regular_file(
            ROOT / CURRENT, package_identity.MAX_CANDIDATE_BYTES, "current candidate"
        )
        identity, _members = package_identity._prepare_package(current_payload, head)
        current_hash = hashlib.sha256(current_payload).hexdigest()
        del current_payload
        previous_payload = package_identity._read_regular_file(
            ROOT / PREVIOUS, package_identity.MAX_CANDIDATE_BYTES, "previous candidate"
        )
        previous_hash = hashlib.sha256(previous_payload).hexdigest()
        del previous_payload
        started = datetime.now(timezone.utc)
        with tempfile.TemporaryDirectory(prefix="narrivy-upgrade-evidence-") as temporary:
            result = upgrade.qualify(
                (ROOT / PREVIOUS).resolve(), (ROOT / CURRENT).resolve(),
                (Path(temporary) / "upgrade.json").resolve(), upgrade.DockerRunner(),
            )
        ended = datetime.now(timezone.utc)
    except (OSError, RuntimeError, upgrade.InputError, json.JSONDecodeError) as error:
        return fail(str(error))
    if result.get("result") != "passed" or result.get("container", {}).get("removed") is not True:
        return fail(f"upgrade lifecycle failed at {result.get('failed_step')}")
    passed_steps = {x.get("step") for x in result.get("steps", []) if x.get("status") == "passed"}
    if not REQUIRED_STEPS.issubset(passed_steps):
        return fail("upgrade lifecycle omitted a required step")
    sentinels = result.get("sentinels", {})
    preservation = [
        "preserved_after_upgrade", "preserved_after_rollback", "preserved_after_reupgrade",
        "preserved_after_remove", "preserved_after_purge",
    ]
    if not all(sentinels.get(key) is True for key in preservation):
        return fail("upgrade lifecycle did not preserve every user-data sentinel")
    evidence = {
        "schema_version": 2, "verification_id": "VER-OPS-003", "requirement_ids": ["OPS-003"],
        "phase": "P10", "scenario_id": PROCEDURE_ID, "git_commit": head, "git_dirty": False,
        "build_id": identity.version, "package_sha256": current_hash,
        "environment": {"id": "ubuntu-24.04-disposable-package-upgrade", "native_or_vm": "controlled",
            "os": "Ubuntu", "os_version": "24.04", "kernel": platform.release(), "desktop": "none",
            "session": "container", "cpu": platform.machine(), "gpu": None, "npu": None,
            "device_details": "fresh test-owned Docker container", "microphone": None,
            "input_backend": None, "display_scale": None,
            "qualification_lane": "ubuntu-24.04-disposable-package-upgrade", "lane_available": True},
        "command": shlex.join(EXPECTED_ARGV), "scenario_version": "1", "scenario_hash": sha256(Path(__file__)),
        "started_at": started.isoformat(), "ended_at": ended.isoformat(), "exit_code": 0,
        "result": "pass", "pass": True,
        "metrics": {"previous_version": result["previous"]["version"],
            "previous_sha256": previous_hash, "current_version": result["current"]["version"],
            "passed_steps": len(passed_steps), "sentinel_hashes": sentinels["hashes"],
            **{key: sentinels[key] for key in preservation}},
        "artifact_hashes": {CURRENT: current_hash, PREVIOUS: previous_hash, HELPER: HELPER_SHA256,
            "scripts/record_package_upgrade_evidence.py": sha256(Path(__file__))},
        "artifact_under_test": {"kind": "package", "path": CURRENT, "sha256": current_hash},
        "procedure": {"id": PROCEDURE_ID, "profile_id": "package-artifact-v1",
            "assertions": [{"id": item, "result": "pass"} for item in ASSERTIONS]},
        "notes": "A fresh Ubuntu 24.04 container installed the previous package, upgraded to the exact current package, explicitly downgraded for rollback, re-upgraded, removed and purged program files, and preserved byte-identical non-root XDG config/data/state/cache sentinels throughout. Package scripts did not traverse user homes.",
        "evidence_type": "package-artifact",
        "execution": {"lane_available": True, "child_failures_ignored": False,
            "environment_class": "release-package", "substitutions": []},
    }
    jsonschema.validate(instance=evidence, schema=json.loads((ROOT / "spec/schemas/evidence.schema.json").read_text()))
    payload = (json.dumps(evidence, indent=2, sort_keys=True) + "\n").encode()
    index["qualified_evidence"].append({"path": EVIDENCE, "sha256": hashlib.sha256(payload).hexdigest(), "disposition": "qualified"})
    index["qualified_evidence"].sort(key=lambda item: item["path"])
    jsonschema.validate(instance=index, schema=json.loads((ROOT / "spec/schemas/evidence-index.schema.json").read_text()))
    atomic_json(evidence_path, evidence); atomic_json(index_path, index)
    print(json.dumps({"artifact_sha256": current_hash, "previous_sha256": previous_hash, "evidence": EVIDENCE, "result": "pass"}, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
