from __future__ import annotations

import contextlib
import csv
import hashlib
import importlib.util
import io
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import jsonschema

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from evidence_policy import MEASURED_EVIDENCE_SCHEMA_VERSION, assess_evidence


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


# Every positive fixture below is a schema-v3 record: each of its claims is
# backed by a measurement resolved in the record's own metrics, and the same
# measurement is pinned by the reviewed procedure contract. The v2 shape, in
# which an assertion is a bare "pass" label, can no longer qualify.
FIXTURE_METRICS = {"fixture": {"gates": {"contract_exit_code": 0}}}
FIXTURE_MEASUREMENT = {"metric": "fixture.gates.contract_exit_code", "comparator": "eq", "expected": 0}


def measured(assertion_id: str) -> dict[str, object]:
    return {"id": assertion_id, "result": "pass", "measurement": dict(FIXTURE_MEASUREMENT)}


def native_record(**overrides: object) -> dict[str, object]:
    artifact_path = "artifacts/releases/narrivy-test.deb"
    artifact_hash = "b" * 64
    data: dict[str, object] = {
        "schema_version": MEASURED_EVIDENCE_SCHEMA_VERSION,
        "verification_id": "VER-TEST-010",
        "requirement_ids": ["TEST-010"],
        "phase": "P11",
        "scenario_id": "native-fixture",
        "git_commit": "a" * 40,
        "git_dirty": False,
        "build_id": "governance-fixture",
        "environment": {
            "id": "isolated-native-fixture",
            "native_or_vm": "native",
            "os": "test-linux",
            "os_version": "1",
            "kernel": "test",
            "desktop": "test",
            "session": "wayland",
            "qualification_lane": "isolated-native",
            "lane_available": True,
        },
        "command": f"python3 fixtures/behavior_runner.py {artifact_path} --scenario native-fixture",
        "started_at": "2026-08-26T11:00:00+00:00",
        "ended_at": "2026-08-26T11:00:01+00:00",
        "exit_code": 0,
        "result": "pass",
        "pass": True,
        "metrics": json.loads(json.dumps(FIXTURE_METRICS)),
        "artifact_hashes": {artifact_path: artifact_hash},
        "artifact_under_test": {"kind": "package", "path": artifact_path, "sha256": artifact_hash},
        "procedure": {
            "id": "native-fixture-procedure-v1",
            "profile_id": "native-behavior-v1",
            "assertions": [measured("behavior-observed")],
        },
        "evidence_type": "native-platform",
        "execution": {
            "lane_available": True,
            "child_failures_ignored": False,
            "environment_class": "native-platform",
            "substitutions": [],
        },
    }
    data.update(overrides)
    return data


def package_record(**overrides: object) -> dict[str, object]:
    """A disposable package-artifact fixture for the P10 profile."""
    data = native_record()
    data.update(
        {
            "verification_id": "VER-OPS-002",
            "requirement_ids": ["OPS-002"],
            "phase": "P10",
            "scenario_id": "package-fixture",
            "evidence_type": "package-artifact",
            "procedure": {
                "id": "package-fixture-procedure-v1",
                "profile_id": "package-artifact-v1",
                "assertions": [measured("package-behavior-observed")],
            },
            "execution": {
                "lane_available": True,
                "child_failures_ignored": False,
                "environment_class": "release-package",
                "substitutions": [],
            },
        }
    )
    data.update(overrides)
    return data


def unit_record(**overrides: object) -> dict[str, object]:
    data: dict[str, object] = {
        "schema_version": MEASURED_EVIDENCE_SCHEMA_VERSION,
        "verification_id": "VER-TEST-003",
        "requirement_ids": ["TEST-003"],
        "phase": "P1",
        "scenario_id": "deterministic-unit-fixture",
        "git_commit": "a" * 40,
        "git_dirty": False,
        "build_id": "governance-fixture",
        "environment": {
            "id": "isolated-controlled-fixture",
            "native_or_vm": "controlled",
            "os": "test-linux",
            "os_version": "1",
            "kernel": "controlled",
            "desktop": "none",
            "session": "none",
            "qualification_lane": "deterministic-controlled",
            "lane_available": True,
        },
        "command": "python3 -m unittest",
        "started_at": "2026-08-26T11:00:00+00:00",
        "ended_at": "2026-08-26T11:00:01+00:00",
        "exit_code": 0,
        "result": "pass",
        "pass": True,
        "metrics": json.loads(json.dumps(FIXTURE_METRICS)),
        "artifact_hashes": {},
        "procedure": {
            "id": "unit-fixture-procedure-v1",
            "profile_id": "unit-contract-v1",
            "assertions": [measured("deterministic-contract")],
        },
        "evidence_type": "unit",
        "execution": {
            "lane_available": True,
            "child_failures_ignored": False,
            "environment_class": "controlled",
            "substitutions": [],
        },
    }
    data.update(overrides)
    return data


def approved_procedure(data: dict[str, object], root: Path) -> dict[str, object]:
    procedure = data["procedure"]
    assert isinstance(procedure, dict)
    if procedure["id"] == "unit-fixture-procedure-v1":
        return {
            "id": "unit-fixture-procedure-v1",
            "verification_id": "VER-TEST-003",
            "profile_id": "unit-contract-v1",
            "command_argv": ["python3", "-m", "unittest"],
            "permitted_substitutions": [],
            "required_assertions": [
                {"id": "deterministic-contract", "measurement": dict(FIXTURE_MEASUREMENT)}
            ],
        }
    package = procedure["id"] == "package-fixture-procedure-v1"
    runner = root / "fixtures/behavior_runner.py"
    runner.parent.mkdir(parents=True, exist_ok=True)
    runner.write_text("# isolated governance fixture runner; never product evidence\n")
    return {
        "id": "package-fixture-procedure-v1" if package else "native-fixture-procedure-v1",
        "verification_id": "VER-OPS-002" if package else "VER-TEST-010",
        "profile_id": "package-artifact-v1" if package else "native-behavior-v1",
        "command_argv": [
            "python3",
            "fixtures/behavior_runner.py",
            "artifacts/releases/narrivy-test.deb",
            "--scenario",
            "native-fixture",
        ],
        "runner": {
            "path": "fixtures/behavior_runner.py",
            "sha256": sha256(runner),
            "argv_index": 1,
        },
        "permitted_substitutions": [],
        "required_assertions": [
            {
                "id": "package-behavior-observed" if package else "behavior-observed",
                "measurement": dict(FIXTURE_MEASUREMENT),
            }
        ],
    }


@contextlib.contextmanager
def isolated_repository(record: dict[str, object] | str, *, registration: str = "valid"):
    """Create a disposable governance-only repository and evidence fixture."""
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary) / "narrivy"
        (root / "spec/schemas").mkdir(parents=True)
        (root / "agent/evidence/P11").mkdir(parents=True)
        (root / "artifacts/releases").mkdir(parents=True)
        for rel in (
            "spec/requirements-traceability.csv",
            "spec/evidence-verification-profiles.json",
            "spec/schemas/evidence.schema.json",
            "spec/schemas/evidence-index.schema.json",
            "spec/schemas/evidence-verification-profiles.schema.json",
        ):
            destination = root / rel
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(ROOT / rel, destination)

        evidence_path = root / "agent/evidence/P11/VER-fixture-v2.json"
        if isinstance(record, str):
            evidence_path.write_text(record)
            data: dict[str, object] | None = None
        else:
            data = json.loads(json.dumps(record))
            if data.get("verification_id") in {"VER-TEST-010", "VER-OPS-002"}:
                package = root / "artifacts/releases/narrivy-test.deb"
                package.write_bytes(b"disposable package fixture; never production evidence\n")
                package_hash = sha256(package)
                data["artifact_hashes"] = {"artifacts/releases/narrivy-test.deb": package_hash}
                data["artifact_under_test"] = {
                    "kind": "package",
                    "path": "artifacts/releases/narrivy-test.deb",
                    "sha256": package_hash,
                }
            evidence_path.write_text(json.dumps(data, indent=2) + "\n")

        contract_path = root / "spec/evidence-verification-profiles.json"
        contract = json.loads(contract_path.read_text())
        if data is not None:
            contract["procedures"] = [approved_procedure(data, root)]
        contract_path.write_text(json.dumps(contract, indent=2) + "\n")

        entries: list[dict[str, str]] = []
        if registration == "valid":
            entries.append(
                {
                    "path": "agent/evidence/P11/VER-fixture-v2.json",
                    "sha256": sha256(evidence_path),
                    "disposition": "qualified",
                }
            )
        elif registration == "wrong-hash":
            entries.append(
                {
                    "path": "agent/evidence/P11/VER-fixture-v2.json",
                    "sha256": "0" * 64,
                    "disposition": "qualified",
                }
            )
        elif registration != "unregistered":
            raise ValueError(f"unknown registration state: {registration}")
        (root / "agent/evidence/INDEX.json").write_text(
            json.dumps(
                {
                    "schema_version": 1,
                    "policy": "release-evidence-v2-required",
                    "legacy_default": {
                        "disposition": "historical-quarantined",
                        "eligible_for_release": False,
                        "reason": "isolated governance test",
                    },
                    "qualified_evidence": entries,
                },
                indent=2,
            )
            + "\n"
        )
        yield root, evidence_path


def refresh_index(root: Path, evidence_path: Path) -> None:
    path = root / "agent/evidence/INDEX.json"
    data = json.loads(path.read_text())
    data["qualified_evidence"] = [
        {
            "path": evidence_path.relative_to(root).as_posix(),
            "sha256": sha256(evidence_path),
            "disposition": "qualified",
        }
    ]
    path.write_text(json.dumps(data, indent=2) + "\n")


def run_marker(root: Path, evidence_path: Path, requirement_id: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [
            sys.executable,
            str(ROOT / "scripts/mark_requirement.py"),
            requirement_id,
            "passed",
            "--evidence",
            evidence_path.relative_to(root).as_posix(),
        ],
        cwd=ROOT,
        env=dict(os.environ, NARRIVY_ROOT=str(root), PYTHONDONTWRITEBYTECODE="1"),
        capture_output=True,
        text=True,
    )


def release_validation_api() -> tuple[dict[str, object], object]:
    source = (ROOT / "scripts/release_gate.py").read_text()
    preamble, separator, _ = source.partition("\ndef main() -> None:")
    if not separator:
        raise AssertionError("release_gate.py no longer exposes validation before its CLI entry point")
    namespace: dict[str, object] = {
        "__name__": "recovery_phase_1_release_validation_test",
        "__file__": str(ROOT / "scripts/release_gate.py"),
    }
    exec(compile(preamble, str(ROOT / "scripts/release_gate.py"), "exec"), namespace)
    validator = namespace.get("validate_evidence")
    if not callable(validator):
        raise AssertionError("release_gate.py did not define validate_evidence")
    return namespace, validator


def load_phase_gate():
    spec = importlib.util.spec_from_file_location("recovery_phase_1_phase_gate", ROOT / "scripts/phase_gate.py")
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class EvidencePolicyUnitTests(unittest.TestCase):
    def test_schema_requires_semantics_and_reviewed_procedure_not_artifact_for_all_profiles(self):
        schema = json.loads((ROOT / "spec/schemas/evidence.schema.json").read_text())
        jsonschema.validate(unit_record(), schema)
        jsonschema.validate(native_record(), schema)
        for missing in ("pass", "procedure", "execution"):
            with self.subTest(missing=missing):
                invalid = unit_record()
                invalid.pop(missing)
                with self.assertRaises(jsonschema.ValidationError):
                    jsonschema.validate(invalid, schema)
        self.assertNotIn("artifact_under_test", unit_record())

    def test_a_claim_without_a_measurement_cannot_mark_a_requirement_passed(self):
        """A label must be measured; an unmeasured label is not a claim."""
        real_traceability = (ROOT / "spec/requirements-traceability.csv").read_bytes()
        cases: dict[str, dict[str, object]] = {
            # The schema-v2 shape: an assertion is a bare "pass" label.
            "unmeasured": unit_record(
                procedure={
                    "id": "unit-fixture-procedure-v1",
                    "profile_id": "unit-contract-v1",
                    "assertions": [{"id": "deterministic-contract", "result": "pass"}],
                }
            ),
            # The measurement is declared but no metric backs it.
            "missing-metric": unit_record(
                procedure={
                    "id": "unit-fixture-procedure-v1",
                    "profile_id": "unit-contract-v1",
                    "assertions": [
                        {
                            "id": "deterministic-contract",
                            "result": "pass",
                            "measurement": {
                                "metric": "fixture.gates.absent",
                                "comparator": "eq",
                                "expected": 0,
                            },
                        }
                    ],
                }
            ),
            # The metric says otherwise while the label still claims pass.
            "contradicted": unit_record(
                metrics={"fixture": {"gates": {"contract_exit_code": 1}}},
            ),
        }
        for name, record in cases.items():
            with self.subTest(case=name), isolated_repository(record) as (temporary_root, evidence_path):
                temporary_traceability = temporary_root / "spec/requirements-traceability.csv"
                before = temporary_traceability.read_bytes()
                marker = run_marker(temporary_root, evidence_path, "TEST-003")
                self.assertNotEqual(0, marker.returncode, marker.stdout + marker.stderr)
                self.assertIn("measurement", marker.stdout + marker.stderr)
                verifier = subprocess.run(
                    [sys.executable, str(ROOT / "scripts/verify_evidence.py"), "--production"],
                    cwd=ROOT,
                    env=dict(
                        os.environ, NARRIVY_ROOT=str(temporary_root), PYTHONDONTWRITEBYTECODE="1"
                    ),
                    capture_output=True,
                    text=True,
                )
                self.assertNotEqual(0, verifier.returncode, verifier.stdout + verifier.stderr)
                self.assertEqual(before, temporary_traceability.read_bytes())
        self.assertEqual(real_traceability, (ROOT / "spec/requirements-traceability.csv").read_bytes())

    def test_semantic_failures_cannot_mark_a_requirement_passed(self):
        real_traceability = (ROOT / "spec/requirements-traceability.csv").read_bytes()
        cases: dict[str, dict[str, object] | str] = {
            "failed": native_record(result="fail", **{"pass": False}, exit_code=1),
            "skipped": native_record(result="skip", **{"pass": False}, exit_code=0),
            "missing-result": {key: value for key, value in native_record().items() if key != "result"},
            "malformed-json": "{not JSON\n",
            "nonzero-exit": native_record(exit_code=1),
        }
        for name, record in cases.items():
            with self.subTest(case=name), isolated_repository(record) as (temporary_root, evidence_path):
                temporary_traceability = temporary_root / "spec/requirements-traceability.csv"
                before = temporary_traceability.read_bytes()
                result = run_marker(temporary_root, evidence_path, "TEST-010")
                self.assertNotEqual(0, result.returncode, result.stdout + result.stderr)
                self.assertEqual(before, temporary_traceability.read_bytes())
        self.assertEqual(real_traceability, (ROOT / "spec/requirements-traceability.csv").read_bytes())

    def test_unit_contract_is_truthfully_qualifiable_without_a_release_artifact(self):
        real_traceability = (ROOT / "spec/requirements-traceability.csv").read_bytes()
        with isolated_repository(unit_record()) as (temporary_root, evidence_path):
            result = run_marker(temporary_root, evidence_path, "TEST-003")
            self.assertEqual(0, result.returncode, result.stdout + result.stderr)
            with (temporary_root / "spec/requirements-traceability.csv").open(newline="") as handle:
                row = next(row for row in csv.DictReader(handle) if row["requirement_id"] == "TEST-003")
            self.assertEqual("passed", row["status"])
            self.assertEqual("unit", row["evidence_classification"])
            verify = subprocess.run(
                [sys.executable, str(ROOT / "scripts/verify_evidence.py"), "--production"],
                cwd=ROOT,
                env=dict(os.environ, NARRIVY_ROOT=str(temporary_root), PYTHONDONTWRITEBYTECODE="1"),
                capture_output=True,
                text=True,
            )
            self.assertEqual(0, verify.returncode, verify.stdout + verify.stderr)
        self.assertEqual(real_traceability, (ROOT / "spec/requirements-traceability.csv").read_bytes())

    def test_isolated_native_fixture_requires_the_native_profile_and_reviewed_runner(self):
        real_traceability = (ROOT / "spec/requirements-traceability.csv").read_bytes()
        with isolated_repository(native_record()) as (temporary_root, evidence_path):
            result = run_marker(temporary_root, evidence_path, "TEST-010")
            self.assertEqual(0, result.returncode, result.stdout + result.stderr)
            with (temporary_root / "spec/requirements-traceability.csv").open(newline="") as handle:
                row = next(row for row in csv.DictReader(handle) if row["requirement_id"] == "TEST-010")
            self.assertEqual("passed", row["status"])
            self.assertEqual("native-platform", row["evidence_classification"])
        self.assertEqual(real_traceability, (ROOT / "spec/requirements-traceability.csv").read_bytes())

    def test_native_profile_rejects_unit_mock_and_missing_artifact(self):
        for evidence_type in ("unit", "mock", "simulated"):
            with self.subTest(evidence_type=evidence_type), isolated_repository(
                native_record(evidence_type=evidence_type)
            ) as (temporary_root, evidence_path):
                record = json.loads(evidence_path.read_text())
                record.pop("artifact_under_test")
                record["artifact_hashes"] = {}
                evidence_path.write_text(json.dumps(record, indent=2) + "\n")
                refresh_index(temporary_root, evidence_path)
                result = run_marker(temporary_root, evidence_path, "TEST-010")
                output = result.stdout + result.stderr
                self.assertNotEqual(0, result.returncode, output)
                self.assertIn("is not permitted by profile", output)
                self.assertIn("lacks an artifact_under_test", output)

    def test_index_registration_and_hash_are_enforced_by_all_validators(self):
        for registration, expected in (
            ("unregistered", "not registered in the authoritative evidence index"),
            ("wrong-hash", "index hash does not match"),
        ):
            with self.subTest(registration=registration), isolated_repository(
                native_record(), registration=registration
            ) as (temporary_root, evidence_path):
                marker = run_marker(temporary_root, evidence_path, "TEST-010")
                self.assertNotEqual(0, marker.returncode, marker.stdout + marker.stderr)
                self.assertIn(expected, marker.stdout + marker.stderr)
                verify = subprocess.run(
                    [sys.executable, str(ROOT / "scripts/verify_evidence.py"), "--production"],
                    cwd=ROOT,
                    env=dict(os.environ, NARRIVY_ROOT=str(temporary_root), PYTHONDONTWRITEBYTECODE="1"),
                    capture_output=True,
                    text=True,
                )
                self.assertNotEqual(0, verify.returncode, verify.stdout + verify.stderr)
                self.assertIn(expected, verify.stdout + verify.stderr)
                phase_gate = load_phase_gate()
                original_root = phase_gate.ROOT
                try:
                    phase_gate.ROOT = temporary_root
                    errors = phase_gate.verify_evidence(
                        evidence_path,
                        phase="P11",
                        commit="unknown",
                        verification_id="VER-TEST-010",
                        verification_layer="manual+release-e2e",
                        requirement_id="TEST-010",
                    )
                finally:
                    phase_gate.ROOT = original_root
                self.assertTrue(any(expected in error for error in errors), errors)
                namespace, release_validator = release_validation_api()
                namespace["ROOT"] = temporary_root
                release_errors = release_validator(
                    evidence_path,
                    rid="TEST-010",
                    vid="VER-TEST-010",
                    verification_layer="manual+release-e2e",
                    commit="unknown",
                )
                self.assertTrue(any(expected in error for error in release_errors), release_errors)

    def test_native_and_package_substitutions_fail_closed_in_every_validator(self):
        cases = (
            (
                "native",
                native_record,
                "TEST-010",
                "VER-TEST-010",
                "manual+release-e2e",
                "P11",
            ),
            (
                "package",
                package_record,
                "OPS-002",
                "VER-OPS-002",
                "packaging/release",
                "P10",
            ),
        )
        substitutions = ("mock", "fixture", "proxy", "stub", "synthetic-audio")
        real_traceability = (ROOT / "spec/requirements-traceability.csv").read_bytes()
        for profile_name, make_record, requirement_id, verification_id, layer, phase in cases:
            for substitution in substitutions:
                with self.subTest(profile=profile_name, substitution=substitution), isolated_repository(
                    make_record()
                ) as (temporary_root, evidence_path):
                    record = json.loads(evidence_path.read_text())
                    record["execution"]["substitutions"] = [substitution]
                    evidence_path.write_text(json.dumps(record, indent=2) + "\n")
                    refresh_index(temporary_root, evidence_path)
                    temporary_traceability = temporary_root / "spec/requirements-traceability.csv"
                    before = temporary_traceability.read_bytes()
                    expected = f"behavioral verification profiles forbid substitutions: {substitution}"

                    marker = run_marker(temporary_root, evidence_path, requirement_id)
                    self.assertNotEqual(0, marker.returncode, marker.stdout + marker.stderr)
                    self.assertIn(expected, marker.stdout + marker.stderr)
                    self.assertEqual(before, temporary_traceability.read_bytes())

                    verifier = subprocess.run(
                        [sys.executable, str(ROOT / "scripts/verify_evidence.py"), "--production"],
                        cwd=ROOT,
                        env=dict(os.environ, NARRIVY_ROOT=str(temporary_root), PYTHONDONTWRITEBYTECODE="1"),
                        capture_output=True,
                        text=True,
                    )
                    self.assertNotEqual(0, verifier.returncode, verifier.stdout + verifier.stderr)
                    self.assertIn(expected, verifier.stdout + verifier.stderr)

                    phase_gate = load_phase_gate()
                    original_root = phase_gate.ROOT
                    try:
                        phase_gate.ROOT = temporary_root
                        phase_errors = phase_gate.verify_evidence(
                            evidence_path,
                            phase=phase,
                            commit="unknown",
                            verification_id=verification_id,
                            verification_layer=layer,
                            requirement_id=requirement_id,
                        )
                    finally:
                        phase_gate.ROOT = original_root
                    self.assertTrue(any(expected in error for error in phase_errors), phase_errors)

                    namespace, release_validator = release_validation_api()
                    namespace["ROOT"] = temporary_root
                    release_errors = release_validator(
                        evidence_path,
                        rid=requirement_id,
                        vid=verification_id,
                        verification_layer=layer,
                        commit="unknown",
                    )
                    self.assertTrue(any(expected in error for error in release_errors), release_errors)
                    self.assertEqual(before, temporary_traceability.read_bytes())
        self.assertEqual(real_traceability, (ROOT / "spec/requirements-traceability.csv").read_bytes())

    def test_controlled_substitutions_require_explicit_profile_and_procedure_approval(self):
        real_traceability = (ROOT / "spec/requirements-traceability.csv").read_bytes()
        with isolated_repository(unit_record()) as (temporary_root, evidence_path):
            record = json.loads(evidence_path.read_text())
            record["execution"]["substitutions"] = ["fixture"]
            evidence_path.write_text(json.dumps(record, indent=2) + "\n")
            refresh_index(temporary_root, evidence_path)
            temporary_traceability = temporary_root / "spec/requirements-traceability.csv"
            before = temporary_traceability.read_bytes()
            marker = run_marker(temporary_root, evidence_path, "TEST-003")
            self.assertNotEqual(0, marker.returncode, marker.stdout + marker.stderr)
            self.assertIn("not permitted by verification profile: fixture", marker.stdout + marker.stderr)
            self.assertEqual(before, temporary_traceability.read_bytes())

        with isolated_repository(unit_record()) as (temporary_root, evidence_path):
            record = json.loads(evidence_path.read_text())
            record["execution"]["substitutions"] = ["fixture"]
            evidence_path.write_text(json.dumps(record, indent=2) + "\n")
            contract_path = temporary_root / "spec/evidence-verification-profiles.json"
            contract = json.loads(contract_path.read_text())
            profile = next(item for item in contract["profiles"] if item["id"] == "unit-contract-v1")
            profile["permitted_substitutions"] = ["fixture"]
            contract["procedures"][0]["permitted_substitutions"] = ["fixture"]
            contract_path.write_text(json.dumps(contract, indent=2) + "\n")
            refresh_index(temporary_root, evidence_path)
            marker = run_marker(temporary_root, evidence_path, "TEST-003")
            self.assertEqual(0, marker.returncode, marker.stdout + marker.stderr)
            verifier = subprocess.run(
                [sys.executable, str(ROOT / "scripts/verify_evidence.py"), "--production"],
                cwd=ROOT,
                env=dict(os.environ, NARRIVY_ROOT=str(temporary_root), PYTHONDONTWRITEBYTECODE="1"),
                capture_output=True,
                text=True,
            )
            self.assertEqual(0, verifier.returncode, verifier.stdout + verifier.stderr)
        self.assertEqual(real_traceability, (ROOT / "spec/requirements-traceability.csv").read_bytes())

    def test_behavioral_profile_rejects_existence_and_unrelated_commands(self):
        invalid_commands = {
            "test": "test artifacts/releases/narrivy-test.deb",
            "true": "true artifacts/releases/narrivy-test.deb",
            "comment": "true # artifacts/releases/narrivy-test.deb",
            "prose": "echo 'artifacts/releases/narrivy-test.deb'",
            "unrelated": "python3 fixtures/unrelated.py artifacts/releases/narrivy-test.deb",
        }
        for name, command in invalid_commands.items():
            with self.subTest(command=name), isolated_repository(native_record()) as (temporary_root, evidence_path):
                data = json.loads(evidence_path.read_text())
                data["command"] = command
                evidence_path.write_text(json.dumps(data, indent=2) + "\n")
                if name in {"test", "true", "prose"}:
                    contract_path = temporary_root / "spec/evidence-verification-profiles.json"
                    contract = json.loads(contract_path.read_text())
                    procedure = contract["procedures"][0]
                    procedure["command_argv"] = {
                        "test": ["test", "artifacts/releases/narrivy-test.deb"],
                        "true": ["true", "artifacts/releases/narrivy-test.deb"],
                        "prose": ["echo", "artifacts/releases/narrivy-test.deb"],
                    }[name]
                    procedure.pop("runner")
                    contract_path.write_text(json.dumps(contract, indent=2) + "\n")
                refresh_index(temporary_root, evidence_path)
                before = (temporary_root / "spec/requirements-traceability.csv").read_bytes()
                result = run_marker(temporary_root, evidence_path, "TEST-010")
                self.assertNotEqual(0, result.returncode, result.stdout + result.stderr)
                self.assertTrue(
                    "reviewed verification procedure" in result.stdout + result.stderr
                    or "behavioral verification procedure" in result.stdout + result.stderr
                    or "placeholder command" in result.stdout + result.stderr
                    or "artifact_under_test" in result.stdout + result.stderr,
                    result.stdout + result.stderr,
                )
                self.assertEqual(before, (temporary_root / "spec/requirements-traceability.csv").read_bytes())

    def test_historical_p0_markdown_cannot_pass_phase_or_release_validation(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / "narrivy"
            (root / "agent/evidence/P0").mkdir(parents=True)
            (root / "spec/schemas").mkdir(parents=True)
            for rel in (
                "spec/requirements-traceability.csv",
                "spec/schemas/evidence.schema.json",
                "spec/evidence-verification-profiles.json",
                "spec/schemas/evidence-verification-profiles.schema.json",
                "spec/schemas/evidence-index.schema.json",
            ):
                destination = root / rel
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(ROOT / rel, destination)
            markdown = root / "agent/evidence/P0/TEST-001-traceability.md"
            markdown.write_text("historical Markdown retained only for provenance\n" * 4)
            with (root / "spec/requirements-traceability.csv").open(newline="") as handle:
                rows = list(csv.DictReader(handle))
                fields = list(rows[0])
            for row in rows:
                if row["requirement_id"] == "TEST-001":
                    row["status"] = "passed"
                    row["evidence"] = "agent/evidence/P0/TEST-001-traceability.md"
                    break
            with (root / "spec/requirements-traceability.csv").open("w", newline="") as handle:
                writer = csv.DictWriter(handle, fieldnames=fields)
                writer.writeheader()
                writer.writerows(rows)
            phase_gate = load_phase_gate()
            original_root = phase_gate.ROOT
            try:
                phase_gate.ROOT = root
                phase_errors = phase_gate.verify_evidence(
                    markdown,
                    phase="P0",
                    commit="unknown",
                    verification_id="VER-TEST-001",
                    verification_layer="ci/architecture",
                    requirement_id="TEST-001",
                )
            finally:
                phase_gate.ROOT = original_root
            self.assertTrue(any("Markdown evidence is quarantined" in error for error in phase_errors), phase_errors)

            release = subprocess.run(
                [sys.executable, str(ROOT / "scripts/release_gate.py"), "--target", "linux-1.0"],
                cwd=ROOT,
                env=dict(os.environ, NARRIVY_ROOT=str(root), PYTHONDONTWRITEBYTECODE="1"),
                capture_output=True,
                text=True,
            )
            self.assertNotEqual(0, release.returncode, release.stdout + release.stderr)
            self.assertIn("TEST-001-traceability.md invalid evidence", release.stdout + release.stderr)


class NonMutatingValidatorUnitTests(unittest.TestCase):
    def test_all_legacy_entry_points_and_imports_cannot_write(self):
        paths = [
            ROOT / "BUILD-PACK-MANIFEST.json",
            ROOT / "spec/requirements-traceability.csv",
            ROOT / "agent/STATE.json",
            *sorted(path for path in (ROOT / "agent/evidence").rglob("*") if path.is_file()),
            *sorted(path for path in (ROOT / "agent/phase-reports").rglob("*") if path.is_file()),
        ]
        before = {path: path.read_bytes() for path in paths}
        manifest = subprocess.run(
            [sys.executable, "scripts/verify_manifest.py"], cwd=ROOT, capture_output=True, text=True
        )
        # The checkpoint manifest is regenerated with the reviewed
        # scripts/build_manifest.py, so the validator must now succeed while
        # remaining read-only; the byte comparison below still proves the
        # legacy entry points never mutate the protected paths.
        self.assertEqual(0, manifest.returncode, manifest.stdout + manifest.stderr)
        self.assertIn("Manifest OK", manifest.stdout)
        for module_path in (
            ROOT / "tools/narrivy-qualify/p2_runner.py",
            ROOT / "tools/narrivy-qualify/p3_runner.py",
            ROOT / "tools/narrivy-qualify/p5_compute_runner.py",
            ROOT / "tools/narrivy-qualify/p5_model_runner.py",
            ROOT / "tools/narrivy-qualify/p7_runner.py",
            ROOT / "tools/narrivy-qualify/p8_dict_runner.py",
            ROOT / "tools/narrivy-qualify/p8_history_runner.py",
            ROOT / "tools/narrivy-qualify/p6_runner.py",
            ROOT / "scripts/refresh_p0_evidence.py",
            ROOT / "scripts/refresh_p1_evidence.py",
            ROOT / "scripts/refresh_p2_evidence.py",
            ROOT / "scripts/refresh_p3_evidence.py",
            ROOT / "scripts/refresh_p4_evidence.py",
            ROOT / "scripts/refresh_p5_model_evidence.py",
            ROOT / "scripts/refresh_p6_evidence.py",
            ROOT / "scripts/refresh_p7_evidence.py",
            ROOT / "scripts/refresh_p8_evidence.py",
            ROOT / "tools/narrivy-qualify/p9_runner.py",
            ROOT / "tools/narrivy-qualify/p10_runner.py",
            ROOT / "tools/narrivy-qualify/p11_runner.py",
            ROOT / "scripts/refresh_p9_evidence.py",
            ROOT / "scripts/refresh_p10_evidence.py",
            ROOT / "scripts/refresh_p11_evidence.py",
        ):
            with self.subTest(entry_point=module_path.name):
                spec = importlib.util.spec_from_file_location(f"quarantined_{module_path.stem}", module_path)
                self.assertIsNotNone(spec)
                self.assertIsNotNone(spec.loader)
                module = importlib.util.module_from_spec(spec)
                spec.loader.exec_module(module)
                self.assertEqual(
                    {"main"},
                    {name for name in vars(module) if not name.startswith("_") and name != "annotations"},
                )
                output = io.StringIO()
                with contextlib.redirect_stdout(output):
                    self.assertEqual(2, module.main())
                self.assertIn("No files were changed", output.getvalue())
                cli = subprocess.run([sys.executable, str(module_path)], cwd=ROOT, capture_output=True, text=True)
                self.assertEqual(2, cli.returncode)
                self.assertIn("No files were changed", cli.stdout)
        run_gate = subprocess.run(
            [sys.executable, "scripts/run_gate.py", "--phase", "P11"],
            cwd=ROOT,
            capture_output=True,
            text=True,
        )
        self.assertEqual(2, run_gate.returncode)
        self.assertIn("No files were changed", run_gate.stdout)
        self.assertEqual(before, {path: path.read_bytes() for path in paths})

    def test_traceability_accepts_the_profiled_quarantined_baseline(self):
        result = subprocess.run(
            [sys.executable, "scripts/verify_traceability.py"], cwd=ROOT, capture_output=True, text=True
        )
        self.assertEqual(0, result.returncode, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
