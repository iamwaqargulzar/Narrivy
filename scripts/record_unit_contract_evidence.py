#!/usr/bin/env python3
"""Run the controlled contract suites once and register schema-v2 evidence."""
from __future__ import annotations

import csv
import hashlib
import json
import os
import platform
import shlex
import subprocess
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path

import jsonschema


ROOT = Path(__file__).resolve().parents[1]
INDEX = "agent/evidence/INDEX.json"
EXPECTED_ARGV = ["python3", "scripts/record_unit_contract_evidence.py"]

SUITES = {
    "core": ["cargo", "test", "-p", "narrivy-core", "--locked"],
    "pipeline": ["cargo", "test", "-p", "narrivy-pipeline", "--locked"],
    "ipc": ["cargo", "test", "-p", "narrivy-ipc", "--locked"],
    "policy": ["cargo", "test", "-p", "narrivy-policy", "--locked"],
    "daemon-e2e": [
        "cargo",
        "test",
        "-p",
        "narrivyd",
        "--test",
        "e2e_mock_dictation",
        "--locked",
    ],
    "desktop-boundary": [
        "python3",
        "-m",
        "unittest",
        "tests.governance.test_desktop_tauri",
    ],
    "dependency-boundary": ["python3", "scripts/verify_dependency_boundaries.py"],
    "traceability": ["python3", "scripts/verify_traceability.py"],
}

# Every semantic claim is bound to named tests that must appear as passing in
# this run's own output, so the claim is measured rather than asserted. A named
# test that does not appear aborts the whole run.
PROOF_TESTS: dict[str, tuple[str, ...]] = {
    "transcript_variants": ("variants_track_raw_corrected_delivered_and_classify",),
    "declarative_modes": ("full_legal_dictation_chain_validates",),
    "session_state_machine": (
        "test_state_machine_all_100_two_step_sequences_corruption_free",
        "test_state_machine_exhaustive_two_step_illegal_rejected",
    ),
    "worker_shutdown_contract": (
        "admitted_worker_is_cancelled_and_joined_by_shutdown",
        "shutdown_all_joins_bounded",
    ),
    "daemon_client_contract": (
        "ipc_handshake_accepts_matching_versions",
        "ipc_envelope_version_skew_detected_on_decode",
    ),
    "capability_driven_policy": ("data_class_default_deny_until_explicitly_listed",),
    "migration_recovery": ("migration_interrupted_by_failure_recovers_on_next_open",),
    "versioned_pipeline_store": (
        "update_requires_exact_version_bump",
        "version_compare_zero_pads_components",
    ),
    "resource_identity_separation": ("resource_identity_has_no_url_field_on_the_wire",),
}


def measurement(metric: str, comparator: str, expected: object = None) -> dict[str, object]:
    binding: dict[str, object] = {"metric": metric, "comparator": comparator}
    if comparator not in {"is_true", "is_false"}:
        binding["expected"] = expected
    return binding


def suite_measurement(suite: str) -> dict[str, object]:
    return measurement(f"suites.{suite}.exit_code", "eq", 0)


def proof_measurement(key: str) -> dict[str, object]:
    return measurement(f"proofs.{key}.executed", "eq", len(PROOF_TESTS[key]))


def measured(assertion: str, binding: dict[str, object]) -> dict[str, object]:
    return {"id": assertion, "result": "pass", "measurement": binding}


def proof_metrics(outputs: list[str]) -> dict[str, object]:
    """Resolve, for every declared claim, the named tests that actually passed."""
    proofs: dict[str, object] = {}
    for key, names in PROOF_TESTS.items():
        observed = [name for name in names if any(name in output for output in outputs)]
        if len(observed) != len(names):
            missing = sorted(set(names) - set(observed))
            raise RuntimeError(
                f"no measured evidence for proof {key!r}: these named tests did not pass in "
                f"this run: {missing}"
            )
        proofs[key] = {"expected": len(names), "executed": len(observed), "tests": observed}
    return proofs


RECORDS = {
    "FUN-005": {
        "phase": "P1",
        "verification": "VER-FUN-005",
        "procedure": "unit-contract-fun-005-v1",
        "assertions": [
            measured("core-contract-suite", suite_measurement("core")),
            measured("transcript-variants-preserved", proof_measurement("transcript_variants")),
        ],
        "artifacts": ["crates/narrivy-core/src/transcript.rs"],
    },
    "FUN-020": {
        "phase": "P1",
        "verification": "VER-FUN-020",
        "procedure": "unit-contract-fun-020-v1",
        "assertions": [
            measured("pipeline-contract-suite", suite_measurement("pipeline")),
            measured("declarative-modes-proven", proof_measurement("declarative_modes")),
        ],
        "artifacts": ["crates/narrivy-pipeline/src/lib.rs", "crates/narrivy-pipeline/tests/contracts.rs"],
    },
    "REL-001": {
        "phase": "P1",
        "verification": "VER-REL-001",
        "procedure": "unit-contract-rel-001-v1",
        "assertions": [
            measured("core-contract-suite", suite_measurement("core")),
            measured("session-state-machine-proven", proof_measurement("session_state_machine")),
        ],
        "artifacts": ["crates/narrivy-core/src/session.rs"],
    },
    "REL-007": {
        "phase": "P1",
        "verification": "VER-REL-007",
        "procedure": "unit-contract-rel-007-v1",
        "assertions": [
            measured("core-contract-suite", suite_measurement("core")),
            measured("worker-shutdown-contract-proven", proof_measurement("worker_shutdown_contract")),
        ],
        "artifacts": ["crates/narrivy-core/src/worker.rs"],
    },
    "PORT-003": {
        "phase": "P1",
        "verification": "VER-PORT-003",
        "procedure": "unit-contract-port-003-v1",
        "assertions": [
            measured("desktop-boundary-suite", suite_measurement("desktop-boundary")),
            measured("daemon-client-contract-proven", proof_measurement("daemon_client_contract")),
        ],
        "artifacts": [
            "apps/narrivy-desktop/CONTRACT.md",
            "apps/narrivy-desktop/src/lib/bridge.ts",
            "apps/narrivy-desktop/src-tauri/src/lib.rs",
            "apps/narrivy-desktop/src-tauri/src/tauri_commands.rs",
            "tests/governance/test_desktop_tauri.py",
        ],
    },
    "PORT-004": {
        "phase": "P1",
        "verification": "VER-PORT-004",
        "procedure": "unit-contract-port-004-v1",
        "assertions": [
            measured("ipc-contract-suite", suite_measurement("ipc")),
            # The executable evidence that a transport never leaks into the
            # domain is the dependency-boundary gate, not the framing suite.
            measured("transport-hidden-from-domain", suite_measurement("dependency-boundary")),
        ],
        "artifacts": ["crates/narrivy-ipc/src/framing.rs", "crates/narrivy-ipc/src/lib.rs"],
    },
    "PORT-005": {
        "phase": "P1",
        "verification": "VER-PORT-005",
        "procedure": "unit-contract-port-005-v1",
        "assertions": [
            measured("policy-contract-suite", suite_measurement("policy")),
            measured("capability-driven-policy-proven", proof_measurement("capability_driven_policy")),
        ],
        "artifacts": ["crates/narrivy-policy/src/lib.rs"],
    },
    "TEST-003": {
        "phase": "P1",
        "verification": "VER-TEST-003",
        "procedure": "unit-contract-test-003-v1",
        "assertions": [
            measured("core-contract-suite", suite_measurement("core")),
            measured("pipeline-contract-suite", suite_measurement("pipeline")),
            measured("ipc-contract-suite", suite_measurement("ipc")),
            measured("policy-contract-suite", suite_measurement("policy")),
            measured("daemon-contract-suite", suite_measurement("daemon-e2e")),
            measured("traceability-verified", suite_measurement("traceability")),
        ],
        "artifacts": [
            "crates/narrivy-core/src/session.rs",
            "crates/narrivy-pipeline/tests/contracts.rs",
            "crates/narrivy-ipc/src/protocol.rs",
            "crates/narrivy-policy/src/lib.rs",
            "services/narrivyd/tests/e2e_mock_dictation.rs",
        ],
    },
    "OPS-006": {
        "phase": "P1",
        "verification": "VER-OPS-006",
        "procedure": "unit-contract-ops-006-v1",
        "assertions": [
            measured("core-contract-suite", suite_measurement("core")),
            measured("migration-recovery-proven", proof_measurement("migration_recovery")),
        ],
        "artifacts": ["crates/narrivy-core/src/persistence.rs"],
    },
    "FUN-030": {
        "phase": "P1",
        "verification": "VER-FUN-030",
        "procedure": "unit-contract-fun-030-v1",
        "assertions": [
            measured("pipeline-contract-suite", suite_measurement("pipeline")),
            measured("versioned-pipeline-store-proven", proof_measurement("versioned_pipeline_store")),
        ],
        "artifacts": ["crates/narrivy-pipeline/src/lib.rs", "crates/narrivy-pipeline/tests/contracts.rs"],
    },
    "PORT-008": {
        "phase": "P1",
        "verification": "VER-PORT-008",
        "procedure": "unit-contract-port-008-v1",
        "assertions": [
            measured("pipeline-contract-suite", suite_measurement("pipeline")),
            measured("dependency-boundary-suite", suite_measurement("dependency-boundary")),
            measured("resource-identity-separation-proven", proof_measurement("resource_identity_separation")),
        ],
        "artifacts": [
            "crates/narrivy-pipeline/src/lib.rs",
            "crates/narrivy-pipeline/tests/contracts.rs",
            "scripts/verify_dependency_boundaries.py",
        ],
    },
}


DEFAULT_NOTE = (
    "Controlled source-contract qualification. Every suite assertion is measured by that suite's "
    "own exit code and every semantic claim by the named tests that passed in this same run (see "
    "metrics.proofs). All shared suites completed in one clean-worktree run; this record makes no "
    "native platform claim."
)
NOTES = {
    "PORT-004": (
        "transport-hidden-from-domain is measured by the dependency-boundary gate rather than by "
        "the ipc framing suite: architectural invisibility of the transport has no unit test, and "
        "the gate is the only executable evidence for it."
    ),
}


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def fail(message: str) -> int:
    print(f"record_unit_contract_evidence: {message}", file=sys.stderr)
    return 1


def git(*args: str) -> str:
    result = subprocess.run(
        ["git", *args], cwd=ROOT, capture_output=True, text=True, check=True
    )
    return result.stdout.strip()


def atomic_json(path: Path, value: object) -> None:
    payload = (json.dumps(value, indent=2, sort_keys=True) + "\n").encode()
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(payload)
            stream.flush()
            os.fsync(stream.fileno())
        os.chmod(temporary, 0o644)
        os.replace(temporary, path)
    finally:
        try:
            Path(temporary).unlink()
        except FileNotFoundError:
            pass


def main() -> int:
    if sys.argv != EXPECTED_ARGV[1:]:
        return fail("argv does not match the reviewed procedure")
    if git("status", "--porcelain"):
        return fail("worktree must be clean before controlled evidence execution")
    missing = sorted(
        artifact
        for record in RECORDS.values()
        for artifact in record["artifacts"]
        if not (ROOT / artifact).is_file()
    )
    if missing:
        return fail(f"reviewed artifact paths are missing: {missing}")

    started = datetime.now(timezone.utc)
    suite_metrics: dict[str, object] = {}
    outputs: list[str] = []
    try:
        for name, command in SUITES.items():
            result = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
            suite_metrics[name] = {
                "command": shlex.join(command),
                "exit_code": result.returncode,
                "stdout_lines": len(result.stdout.splitlines()),
                "stderr_lines": len(result.stderr.splitlines()),
            }
            combined = result.stdout + result.stderr
            outputs.append(combined)
            if result.returncode != 0:
                return fail(f"suite {name} failed\n" + "\n".join(combined.splitlines()[-20:]))
        proofs = proof_metrics(outputs)
    except RuntimeError as error:
        return fail(str(error))
    if git("status", "--porcelain"):
        return fail("controlled suites changed the worktree")

    ended = datetime.now(timezone.utc)
    head = git("rev-parse", "HEAD")
    script_rel = "scripts/record_unit_contract_evidence.py"
    script_hash = sha256(ROOT / script_rel)
    evidence_schema = json.loads((ROOT / "spec/schemas/evidence.schema.json").read_text())
    index_schema = json.loads((ROOT / "spec/schemas/evidence-index.schema.json").read_text())
    index_path = ROOT / INDEX
    index = json.loads(index_path.read_text())
    target_paths = {
        f"agent/evidence/{record['phase']}/{record['verification']}.json"
        for record in RECORDS.values()
    }
    index["qualified_evidence"] = [
        item for item in index["qualified_evidence"] if item.get("path") not in target_paths
    ]

    payloads: list[tuple[Path, dict[str, object], bytes]] = []
    for requirement_id, record in RECORDS.items():
        evidence_rel = f"agent/evidence/{record['phase']}/{record['verification']}.json"
        hashes = {script_rel: script_hash}
        for artifact in record["artifacts"]:
            hashes[artifact] = sha256(ROOT / artifact)
        evidence = {
            "schema_version": 3,
            "verification_id": record["verification"],
            "requirement_ids": [requirement_id],
            "phase": record["phase"],
            "scenario_id": record["procedure"],
            "git_commit": head,
            "git_dirty": False,
            "build_id": f"source-{head[:12]}",
            "environment": {
                "id": "controlled-unit-contract",
                "native_or_vm": "controlled",
                "os": platform.system(),
                "os_version": platform.release(),
                "kernel": platform.release(),
                "desktop": "not-applicable",
                "session": "controlled",
                "cpu": platform.machine(),
                "gpu": None,
                "npu": None,
                "device_details": None,
                "microphone": None,
                "input_backend": None,
                "display_scale": None,
                "qualification_lane": "controlled-unit-contract",
                "lane_available": True,
            },
            "command": shlex.join(EXPECTED_ARGV),
            "scenario_version": "1",
            "scenario_hash": script_hash,
            "started_at": started.isoformat(),
            "ended_at": ended.isoformat(),
            "exit_code": 0,
            "result": "pass",
            "pass": True,
            "metrics": {"suites": suite_metrics, "proofs": proofs},
            "artifact_hashes": hashes,
            "procedure": {
                "id": record["procedure"],
                "profile_id": "unit-contract-v1",
                "assertions": record["assertions"],
            },
            "notes": NOTES.get(requirement_id, DEFAULT_NOTE),
            "evidence_type": "unit",
            "execution": {
                "lane_available": True,
                "child_failures_ignored": False,
                "environment_class": "controlled",
                "substitutions": [],
            },
        }
        jsonschema.validate(instance=evidence, schema=evidence_schema)
        payload = (json.dumps(evidence, indent=2, sort_keys=True) + "\n").encode()
        payloads.append((ROOT / evidence_rel, evidence, payload))
        index["qualified_evidence"].append(
            {
                "path": evidence_rel,
                "sha256": hashlib.sha256(payload).hexdigest(),
                "disposition": "qualified",
            }
        )

    index["qualified_evidence"].sort(key=lambda item: item["path"])
    jsonschema.validate(instance=index, schema=index_schema)
    for path, evidence, _payload in payloads:
        atomic_json(path, evidence)
    atomic_json(index_path, index)
    print(json.dumps({"result": "pass", "records": len(payloads)}, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
