#!/usr/bin/env python3
"""Qualify exact .deb upgrade/rollback behavior and record VER-OPS-003."""
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

import qualify_candidate_known_phrase as package_identity
import qualify_candidate_upgrade as upgrade
import recorder_safety

ROOT = Path(__file__).resolve().parents[1]
CURRENT = "artifacts/releases/narrivy-linux-amd64.deb"
PREVIOUS = "artifacts/releases/narrivy-previous-amd64.deb"
EVIDENCE = "agent/evidence/P10/VER-OPS-003.json"
INDEX = "agent/evidence/INDEX.json"
HELPER = "scripts/qualify_candidate_upgrade.py"
HELPER_SHA256 = "16cd9c039ca4cead0aee37bf73399610e546950faa6c8118482edab573cdfe2c"
# The reviewed previous release the upgrade starts from, pinned by hash. Set it
# to the sha256 of the previous published .deb before recording; until then the
# recorder fails closed rather than upgrading from an arbitrary file.
PREVIOUS_SHA256: str | None = None
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
    "confirm_purge", "container_remove", *upgrade.DAEMON_CHECK_STEPS,
}


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def fail(message: str) -> int:
    print(f"record_package_upgrade_evidence: {message}", file=sys.stderr)
    return 1


def main() -> int:
    if sys.argv != EXPECTED_ARGV[1:]:
        return fail("argv does not match the reviewed procedure")
    if sha256(ROOT / HELPER) != HELPER_SHA256:
        return fail("reviewed upgrade helper hash mismatch")
    evidence_path = ROOT / EVIDENCE
    index_path = ROOT / INDEX
    index = json.loads(index_path.read_text())
    if not recorder_safety.replaceable(evidence_path, index, ROOT):
        return fail("evidence output is current, unindexed v2, or does not match its INDEX hash")
    if PREVIOUS_SHA256 is None or not upgrade.SHA256_RE.fullmatch(PREVIOUS_SHA256):
        return fail("the previous package is not pinned (set PREVIOUS_SHA256 to the reviewed previous release)")
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
        if previous_hash != PREVIOUS_SHA256:
            raise RuntimeError("previous package does not match its pinned sha256")
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
    daemon_checks = result.get("daemon_checks", {})
    if not all(daemon_checks.get(step) is True for step in upgrade.DAEMON_CHECK_STEPS):
        return fail("the installed daemon did not answer status after every install")
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
            "previous_sha256": previous_hash, "previous_sha256_pinned": previous_hash == PREVIOUS_SHA256,
            "current_version": result["current"]["version"],
            "daemon_status_checks_passed": sum(1 for step in upgrade.DAEMON_CHECK_STEPS if daemon_checks.get(step) is True),
            "passed_steps": len(passed_steps), "sentinel_hashes": sentinels["hashes"],
            **{key: sentinels[key] for key in preservation}},
        "artifact_hashes": {CURRENT: current_hash, PREVIOUS: previous_hash, HELPER: HELPER_SHA256,
            "scripts/record_package_upgrade_evidence.py": sha256(Path(__file__))},
        "artifact_under_test": {"kind": "package", "path": CURRENT, "sha256": current_hash},
        "procedure": {"id": PROCEDURE_ID, "profile_id": "package-artifact-v1",
            "assertions": [{"id": item, "result": "pass"} for item in ASSERTIONS]},
        "notes": "A fresh Ubuntu 24.04 container installed the hash-pinned previous package, upgraded to the exact current package, explicitly downgraded for rollback, re-upgraded, removed and purged program files, and preserved byte-identical non-root XDG config/data/state/cache sentinels throughout. Package scripts did not traverse user homes. After the first install, the upgrade, the rollback and the re-upgrade, the installed daemon started, answered narrivy-cli status and exited cleanly.",
        "evidence_type": "package-artifact",
        "execution": {"lane_available": True, "child_failures_ignored": False,
            "environment_class": "release-package", "substitutions": []},
    }
    jsonschema.validate(instance=evidence, schema=json.loads((ROOT / "spec/schemas/evidence.schema.json").read_text()))
    payload = (json.dumps(evidence, indent=2, sort_keys=True) + "\n").encode()
    index_schema = json.loads((ROOT / "spec/schemas/evidence-index.schema.json").read_text())
    try:
        recorder_safety.commit_records(
            ROOT, index_path, index, [(evidence_path, payload)],
            validate_index=lambda value: jsonschema.validate(instance=value, schema=index_schema),
        )
    except (OSError, recorder_safety.RecorderSafetyError, jsonschema.ValidationError) as error:
        return fail(str(error))
    print(json.dumps({"artifact_sha256": current_hash, "previous_sha256": previous_hash, "evidence": EVIDENCE, "result": "pass"}, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
