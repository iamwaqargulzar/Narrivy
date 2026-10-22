"""Fail-closed tests for measured procedure assertions.

A label must not be able to validate itself. These tests prove that a record
whose measurement is missing, unresolvable, unsatisfied, or contradicted by its
own declared result cannot qualify through any of the release validators, and
that a well-measured record still can.
"""
from __future__ import annotations

import contextlib
import csv
import hashlib
import importlib.util
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

import evidence_policy
from evidence_policy import (
    MEASURED_EVIDENCE_SCHEMA_VERSION,
    assessment_errors_for_record,
    measurement_failure_reason,
    resolve_measurement_metric,
)

EVIDENCE_SCHEMA_PATH = ROOT / "spec/schemas/evidence.schema.json"
REQUIREMENT_ID = "TEST-003"
VERIFICATION_ID = "VER-TEST-003"
VERIFICATION_LAYER = "unit/contract"
PROCEDURE_ID = "unit-fixture-procedure-v1"
ASSERTION_ID = "deterministic-contract"
MEASUREMENT = {"metric": "gates.contract.exit_code", "comparator": "eq", "expected": 0}
METRICS = {"gates": {"contract": {"exit_code": 0}}}


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def measured_record(**overrides: object) -> dict[str, object]:
    """A minimal controlled unit record whose single claim is measured."""
    data: dict[str, object] = {
        "schema_version": MEASURED_EVIDENCE_SCHEMA_VERSION,
        "verification_id": VERIFICATION_ID,
        "requirement_ids": [REQUIREMENT_ID],
        "phase": "P1",
        "scenario_id": PROCEDURE_ID,
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
        "metrics": json.loads(json.dumps(METRICS)),
        "artifact_hashes": {},
        "procedure": {
            "id": PROCEDURE_ID,
            "profile_id": "unit-contract-v1",
            "assertions": [
                {"id": ASSERTION_ID, "result": "pass", "measurement": dict(MEASUREMENT)}
            ],
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


def mutate(record: dict[str, object], mutation: object) -> dict[str, object]:
    """Apply an in-place mutation to a deep copy and return the mutated copy."""
    data = json.loads(json.dumps(record))
    mutation(data)
    return data


def set_assertion(record: dict[str, object], **fields: object) -> None:
    procedure = record["procedure"]
    assert isinstance(procedure, dict)
    assertions = procedure["assertions"]
    assert isinstance(assertions, list)
    assertions[0].clear()
    assertions[0].update(fields)


@contextlib.contextmanager
def isolated_repository(record: dict[str, object] | None, *, pinned: object = "pinned"):
    """Disposable governance-only repository holding one evidence fixture."""
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary) / "narrivy"
        (root / "spec/schemas").mkdir(parents=True)
        (root / "agent/evidence/P1").mkdir(parents=True)
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

        evidence_path = root / "agent/evidence/P1/VER-fixture-v3.json"
        data = json.loads(json.dumps(record)) if record is not None else None
        if data is not None:
            evidence_path.write_text(json.dumps(data, indent=2) + "\n")

        contract_path = root / "spec/evidence-verification-profiles.json"
        contract = json.loads(contract_path.read_text())
        if data is not None:
            entry = {"id": ASSERTION_ID}
            if pinned != "bare":
                entry["measurement"] = dict(MEASUREMENT if pinned == "pinned" else pinned)
            elif pinned == "bare":
                entry = ASSERTION_ID
            contract["procedures"] = [
                {
                    "id": PROCEDURE_ID,
                    "verification_id": VERIFICATION_ID,
                    "profile_id": "unit-contract-v1",
                    "command_argv": ["python3", "-m", "unittest"],
                    "permitted_substitutions": [],
                    "required_assertions": [entry],
                }
            ]
        contract_path.write_text(json.dumps(contract, indent=2) + "\n")

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
                    "qualified_evidence": (
                        [
                            {
                                "path": "agent/evidence/P1/VER-fixture-v3.json",
                                "sha256": sha256(evidence_path),
                                "disposition": "qualified",
                            }
                        ]
                        if data is not None
                        else []
                    ),
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


def run_marker(root: Path, evidence_path: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [
            sys.executable,
            str(ROOT / "scripts/mark_requirement.py"),
            REQUIREMENT_ID,
            "passed",
            "--evidence",
            evidence_path.relative_to(root).as_posix(),
        ],
        cwd=ROOT,
        env=dict(os.environ, NARRIVY_ROOT=str(root), PYTHONDONTWRITEBYTECODE="1"),
        capture_output=True,
        text=True,
    )


def run_verify(root: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(ROOT / "scripts/verify_evidence.py"), "--production"],
        cwd=ROOT,
        env=dict(os.environ, NARRIVY_ROOT=str(root), PYTHONDONTWRITEBYTECODE="1"),
        capture_output=True,
        text=True,
    )


def load_phase_gate():
    spec = importlib.util.spec_from_file_location(
        "measured_phase_gate", ROOT / "scripts/phase_gate.py"
    )
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def release_validation_api() -> tuple[dict[str, object], object]:
    source = (ROOT / "scripts/release_gate.py").read_text()
    preamble, separator, _ = source.partition("\nap = argparse.ArgumentParser()")
    if not separator:
        raise AssertionError("release_gate.py no longer exposes validation before its CLI")
    namespace: dict[str, object] = {
        "__name__": "measured_release_validation_test",
        "__file__": str(ROOT / "scripts/release_gate.py"),
    }
    exec(compile(preamble, str(ROOT / "scripts/release_gate.py"), "exec"), namespace)
    validator = namespace.get("validate_evidence")
    if not callable(validator):
        raise AssertionError("release_gate.py did not define validate_evidence")
    return namespace, validator


def all_validators(root: Path, evidence_path: Path) -> dict[str, list[str]]:
    """Collect the same record's errors from every independent validator."""
    marker = run_marker(root, evidence_path)
    verifier = run_verify(root)
    phase_gate = load_phase_gate()
    original_root = phase_gate.ROOT
    try:
        phase_gate.ROOT = root
        phase_errors = phase_gate.verify_evidence(
            evidence_path,
            phase="P1",
            commit="unknown",
            verification_id=VERIFICATION_ID,
            verification_layer=VERIFICATION_LAYER,
            requirement_id=REQUIREMENT_ID,
        )
    finally:
        phase_gate.ROOT = original_root
    namespace, release_validator = release_validation_api()
    namespace["ROOT"] = root
    release_errors = release_validator(
        evidence_path,
        rid=REQUIREMENT_ID,
        vid=VERIFICATION_ID,
        verification_layer=VERIFICATION_LAYER,
        commit="unknown",
    )
    return {
        "mark_requirement": [line.strip(" -") for line in (marker.stdout + marker.stderr).splitlines() if line.strip() and not line.startswith("[")],
        "marker_returncode": [str(marker.returncode)],
        "verify_evidence": [line.strip(" -") for line in (verifier.stdout + verifier.stderr).splitlines() if line.startswith(" -")],
        "verify_returncode": [str(verifier.returncode)],
        "phase_gate": list(phase_errors),
        "release_gate": list(release_errors),
    }


class MeasurementSchemaTests(unittest.TestCase):
    def setUp(self):
        self.schema = json.loads(EVIDENCE_SCHEMA_PATH.read_text())

    def test_a_measured_record_satisfies_the_released_schema(self):
        jsonschema.validate(measured_record(), self.schema)

    def test_schema_v3_requires_a_measurement_on_every_assertion(self):
        record = measured_record()
        set_assertion(record, id=ASSERTION_ID, result="pass")
        with self.assertRaises(jsonschema.ValidationError):
            jsonschema.validate(record, self.schema)

    def test_schema_v3_requires_metrics_to_bind_against(self):
        record = measured_record()
        record.pop("metrics")
        with self.assertRaises(jsonschema.ValidationError):
            jsonschema.validate(record, self.schema)

    def test_schema_v2_records_stay_readable_but_cannot_borrow_a_measurement(self):
        legacy = measured_record(schema_version=2)
        set_assertion(legacy, id=ASSERTION_ID, result="pass")
        jsonschema.validate(legacy, self.schema)
        with self.assertRaises(jsonschema.ValidationError):
            set_assertion(legacy, id=ASSERTION_ID, result="pass", measurement=dict(MEASUREMENT))
            jsonschema.validate(legacy, self.schema)

    def test_a_measurement_with_an_unknown_comparator_is_rejected_by_the_schema(self):
        record = measured_record()
        set_assertion(
            record,
            id=ASSERTION_ID,
            result="pass",
            measurement={"metric": "gates.contract.exit_code", "comparator": "probably"},
        )
        with self.assertRaises(jsonschema.ValidationError):
            jsonschema.validate(record, self.schema)

    def test_a_measurement_with_an_unexpected_field_is_rejected_by_the_schema(self):
        record = measured_record()
        set_assertion(
            record,
            id=ASSERTION_ID,
            result="pass",
            measurement={**MEASUREMENT, "note": "trust me"},
        )
        with self.assertRaises(jsonschema.ValidationError):
            jsonschema.validate(record, self.schema)

    def test_the_released_schemas_are_themselves_valid(self):
        for rel in (
            "spec/schemas/evidence.schema.json",
            "spec/schemas/evidence-verification-profiles.schema.json",
        ):
            with self.subTest(schema=rel):
                jsonschema.Draft202012Validator.check_schema(
                    json.loads((ROOT / rel).read_text())
                )


class MeasurementResolutionTests(unittest.TestCase):
    def test_a_missing_metric_key_never_resolves(self):
        with self.assertRaises(evidence_policy.MeasurementError):
            resolve_measurement_metric(METRICS, "gates.contract.missing")
        with self.assertRaises(evidence_policy.MeasurementError):
            resolve_measurement_metric(METRICS, "gates.absent.exit_code")
        self.assertEqual(resolve_measurement_metric(METRICS, "gates.contract.exit_code"), 0)

    def test_a_malformed_metric_path_never_resolves(self):
        for path in ("", ".leading", "trailing.", "double..dot", "spaces here", 7, None):
            with self.subTest(path=path):
                with self.assertRaises(evidence_policy.MeasurementError):
                    resolve_measurement_metric(METRICS, path)

    def test_a_non_mapping_metrics_object_never_resolves(self):
        for metrics in (None, [], "gates", 3):
            with self.subTest(metrics=metrics):
                with self.assertRaises(evidence_policy.MeasurementError):
                    resolve_measurement_metric(metrics, "gates.contract.exit_code")

    def test_satisfied_measurements_return_no_reason(self):
        cases = (
            ({"a": 5}, {"metric": "a", "comparator": "eq", "expected": 5}, None),
            ({"a": 5}, {"metric": "a", "comparator": "eq", "expected": 5.4, "tolerance": 0.5}, None),
            ({"a": 5}, {"metric": "a", "comparator": "ne", "expected": 6}, None),
            ({"a": 5}, {"metric": "a", "comparator": "lt", "expected": 6}, None),
            ({"a": 5}, {"metric": "a", "comparator": "le", "expected": 5}, None),
            ({"a": 5}, {"metric": "a", "comparator": "gt", "expected": 4}, None),
            ({"a": 5}, {"metric": "a", "comparator": "ge", "expected": 5}, None),
            ({"a": "cpu"}, {"metric": "a", "comparator": "eq", "expected": "cpu"}, None),
            ({"a": True}, {"metric": "a", "comparator": "is_true"}, None),
            ({"a": False}, {"metric": "a", "comparator": "is_false"}, None),
            ({"a": [1, 2, 3]}, {"metric": "a", "comparator": "len_eq", "expected": 3}, None),
            ({"a": [1, 2, 3]}, {"metric": "a", "comparator": "len_ge", "expected": 2}, None),
            ({"a": [1, 2, 3]}, {"metric": "a", "comparator": "len_le", "expected": 3}, None),
            ({"a": [1, 2, 3]}, {"metric": "a", "comparator": "len_gt", "expected": 2}, None),
            ({"a": [1, 2, 3]}, {"metric": "a", "comparator": "len_ne", "expected": 2}, None),
            ({"a": [1, 2, 3]}, {"metric": "a", "comparator": "len_lt", "expected": 4}, None),
            ({"a": "x"}, {"metric": "a", "comparator": "in", "expected": ["x", "y"]}, None),
            ({"a": "x"}, {"metric": "a", "comparator": "not_in", "expected": ["y"]}, None),
            ({"a": ["p", "q"]}, {"metric": "a", "comparator": "contains", "expected": ["q"]}, None),
            ({"a": {"k": 1}}, {"metric": "a", "comparator": "contains", "expected": ["k"]}, None),
        )
        for metrics, measurement, expected_reason in cases:
            with self.subTest(measurement=measurement):
                self.assertEqual(measurement_failure_reason(metrics, measurement), expected_reason)

    def test_unsatisfied_measurements_all_report_a_reason(self):
        cases = (
            ({"a": 5}, {"metric": "a", "comparator": "eq", "expected": 0}),
            ({"a": 5}, {"metric": "a", "comparator": "ne", "expected": 5}),
            ({"a": 5}, {"metric": "a", "comparator": "ge", "expected": 6}),
            ({"a": 5}, {"metric": "a", "comparator": "le", "expected": 4}),
            ({"a": 5}, {"metric": "a", "comparator": "is_true"}),
            ({"a": 5}, {"metric": "a", "comparator": "is_false"}),
            ({"a": [1, 2]}, {"metric": "a", "comparator": "len_ge", "expected": 3}),
            ({"a": "x"}, {"metric": "a", "comparator": "in", "expected": ["y"]}),
            ({"a": ["p"]}, {"metric": "a", "comparator": "contains", "expected": ["q"]}),
        )
        for metrics, measurement in cases:
            with self.subTest(measurement=measurement):
                self.assertIsNotNone(measurement_failure_reason(metrics, measurement))

    def test_booleans_are_not_numbers_and_types_are_not_interchangeable(self):
        # True == 1 in Python; a governance measurement must not inherit that.
        self.assertIsNotNone(
            measurement_failure_reason({"a": True}, {"metric": "a", "comparator": "eq", "expected": 1})
        )
        self.assertIsNotNone(
            measurement_failure_reason({"a": 1}, {"metric": "a", "comparator": "is_true"})
        )
        self.assertIsNotNone(
            measurement_failure_reason({"a": 0}, {"metric": "a", "comparator": "is_false"})
        )
        self.assertIsNotNone(
            measurement_failure_reason({"a": "5"}, {"metric": "a", "comparator": "eq", "expected": 5})
        )
        self.assertIsNotNone(
            measurement_failure_reason({"a": None}, {"metric": "a", "comparator": "eq", "expected": False})
        )

    def test_unusable_comparators_and_malformed_bindings_are_hard_errors(self):
        cases = (
            ({"a": 1}, {"metric": "a", "comparator": "maybe"}),
            ({"a": 1}, {"metric": "a", "comparator": "eq"}),
            ({"a": 1}, {"metric": "a", "comparator": "ge", "expected": "1"}),
            ({"a": 1}, {"metric": "a", "comparator": "len_eq", "expected": 1}),
            ({"a": 1}, {"metric": "a", "comparator": "contains", "expected": [1]}),
            ({"a": 1}, {"metric": "a", "comparator": "in", "expected": []}),
            ({"a": 1}, {"metric": "a", "comparator": "is_true", "expected": True}),
            ({"a": 1}, {"metric": "a", "comparator": "eq", "expected": 1, "tolerance": "x"}),
            ({"a": "s"}, {"metric": "a", "comparator": "eq", "expected": "s", "tolerance": 1}),
            ({"a": 1}, {"metric": "a", "comparator": "eq", "expected": 1, "extra": True}),
            ({"a": 1}, "not-a-mapping"),
            ({"a": 1}, None),
        )
        for metrics, measurement in cases:
            with self.subTest(measurement=measurement):
                self.assertIsNotNone(measurement_failure_reason(metrics, measurement))


class AssessmentConsistencyTests(unittest.TestCase):
    def test_assessment_agrees_with_the_declared_result_in_both_directions(self):
        satisfied = measured_record()
        errors = assessment_errors_for_record(satisfied, assertion_id=ASSERTION_ID)
        self.assertEqual(errors, [])

        overstated = mutate(
            satisfied,
            lambda data: data["metrics"]["gates"]["contract"].update({"exit_code": 1}),
        )
        errors = assessment_errors_for_record(overstated, assertion_id=ASSERTION_ID)
        self.assertEqual(len(errors), 1, errors)
        self.assertIn("contradicts its measurement", errors[0])
        self.assertIn("gates.contract.exit_code", errors[0])

        unmeasured = mutate(
            satisfied,
            lambda data: data["procedure"]["assertions"][0].pop("measurement"),
        )
        errors = assessment_errors_for_record(unmeasured, assertion_id=ASSERTION_ID)
        self.assertEqual(len(errors), 1, errors)
        self.assertIn("declares no measurement binding", errors[0])


class FailClosedValidatorTests(unittest.TestCase):
    def assert_fails_everywhere(self, record: dict[str, object], *needle_groups: object) -> None:
        """Each group is one expectation; a group may offer alternative wordings.

        Validators fail at different layers - the released schema rejects a
        malformed path before the policy ever sees it - so a group lists the
        equivalent messages each layer is allowed to produce.
        """
        real_traceability = (ROOT / "spec/requirements-traceability.csv").read_bytes()
        with isolated_repository(record) as (root, evidence_path):
            before = (root / "spec/requirements-traceability.csv").read_bytes()
            results = all_validators(root, evidence_path)
            for validator, errors in results.items():
                if validator.endswith("_returncode"):
                    continue
                with self.subTest(validator=validator):
                    joined = "\n".join(errors)
                    for group in needle_groups:
                        options = (group,) if isinstance(group, str) else tuple(group)
                        self.assertTrue(
                            any(option in joined for option in options),
                            f"{validator}: none of {options} in {joined}",
                        )
            self.assertNotEqual("0", results["marker_returncode"][0])
            self.assertNotEqual("0", results["verify_returncode"][0])
            self.assertEqual(before, (root / "spec/requirements-traceability.csv").read_bytes())
        self.assertEqual(real_traceability, (ROOT / "spec/requirements-traceability.csv").read_bytes())

    def test_a_label_cannot_claim_pass_while_its_metric_says_otherwise(self):
        record = mutate(
            measured_record(),
            lambda data: data["metrics"]["gates"]["contract"].update({"exit_code": 1}),
        )
        self.assert_fails_everywhere(
            record, "assertion result contradicts its measurement"
        )

    def test_a_missing_metric_key_fails_everywhere(self):
        record = mutate(
            measured_record(),
            lambda data: data["procedure"]["assertions"][0]["measurement"].update(
                {"metric": "gates.contract.does_not_exist"}
            ),
        )
        self.assert_fails_everywhere(record, "measurement metric does not resolve")

    def test_a_malformed_metric_path_fails_everywhere(self):
        record = mutate(
            measured_record(),
            lambda data: data["procedure"]["assertions"][0]["measurement"].update(
                {"metric": "gates..contract"}
            ),
        )
        self.assert_fails_everywhere(record, "gates..contract")

    def test_a_comparator_that_cannot_be_evaluated_fails_everywhere(self):
        record = mutate(
            measured_record(),
            lambda data: data["procedure"]["assertions"][0]["measurement"].update(
                {"comparator": "eq", "expected": "zero"}
            ),
        )
        self.assert_fails_everywhere(
            record, ("is not satisfied", "not satisfied by observed")
        )

    def test_a_record_without_measurements_fails_everywhere(self):
        record = mutate(
            measured_record(),
            lambda data: data["procedure"]["assertions"][0].pop("measurement"),
        )
        self.assert_fails_everywhere(
            record, ("declares no measurement binding", "measurement")
        )

    def test_a_record_with_no_metrics_at_all_fails_everywhere(self):
        record = mutate(measured_record(), lambda data: data.update({"metrics": None}))
        self.assert_fails_everywhere(
            record, ("is not of type 'object'", "no metrics object")
        )

    def test_a_recorder_cannot_substitute_a_weaker_measurement_than_reviewed(self):
        record = mutate(
            measured_record(),
            lambda data: data["procedure"]["assertions"][0]["measurement"].update(
                {"metric": "gates.contract.exit_code", "comparator": "le", "expected": 1}
            ),
        )
        self.assert_fails_everywhere(
            record, "assertion measurement does not match the reviewed binding"
        )

    def test_a_legacy_unpinned_contract_entry_cannot_be_qualified(self):
        record = measured_record()
        with isolated_repository(record, pinned="bare") as (root, evidence_path):
            results = all_validators(root, evidence_path)
            for validator, errors in results.items():
                if validator.endswith("_returncode"):
                    continue
                with self.subTest(validator=validator):
                    self.assertIn(
                        "reviewed procedure assertion has no reviewed measurement binding",
                        "\n".join(errors),
                    )
            self.assertNotEqual("0", results["marker_returncode"][0])
            self.assertNotEqual("0", results["verify_returncode"][0])

    def test_a_record_claiming_a_result_other_than_pass_is_rejected(self):
        record = measured_record(result="fail", **{"pass": False}, exit_code=1)
        self.assert_fails_everywhere(record, "not pass")

    def test_an_assertion_omitted_from_the_record_fails_everywhere(self):
        record = mutate(
            measured_record(),
            lambda data: data["procedure"]["assertions"][0].update({"id": "unrelated-claim"}),
        )
        self.assert_fails_everywhere(record, "did not pass")


class MeasuredRecordQualifiesTests(unittest.TestCase):
    def test_a_fully_measured_record_still_qualifies(self):
        real_traceability = (ROOT / "spec/requirements-traceability.csv").read_bytes()
        with isolated_repository(measured_record()) as (root, evidence_path):
            marker = run_marker(root, evidence_path)
            self.assertEqual(0, marker.returncode, marker.stdout + marker.stderr)
            with (root / "spec/requirements-traceability.csv").open(newline="") as handle:
                row = next(
                    row
                    for row in csv.DictReader(handle)
                    if row["requirement_id"] == REQUIREMENT_ID
                )
            self.assertEqual("passed", row["status"])
            self.assertEqual("unit", row["evidence_classification"])
            verifier = run_verify(root)
            self.assertEqual(0, verifier.returncode, verifier.stdout + verifier.stderr)
        self.assertEqual(real_traceability, (ROOT / "spec/requirements-traceability.csv").read_bytes())

    def test_a_metric_that_changes_after_recording_invalidates_the_evidence(self):
        with isolated_repository(measured_record()) as (root, evidence_path):
            self.assertEqual(0, run_verify(root).returncode)
            record = json.loads(evidence_path.read_text())
            record["metrics"]["gates"]["contract"]["exit_code"] = 7
            evidence_path.write_text(json.dumps(record, indent=2) + "\n")
            refresh_index(root, evidence_path)
            verifier = run_verify(root)
            self.assertNotEqual(0, verifier.returncode, verifier.stdout + verifier.stderr)
            self.assertIn("contradicts its measurement", verifier.stdout + verifier.stderr)


class ContractPinningTests(unittest.TestCase):
    """The reviewed contract, not the recorder, decides what backs each claim."""

    MIGRATED_RECORDERS = (
        "record_controlled_integration_evidence",
        "record_unit_contract_evidence",
        "record_native_accessibility_evidence",
        "record_native_hardening_evidence",
    )

    def load_recorder(self, name: str):
        spec = importlib.util.spec_from_file_location(
            f"pinned_{name}", ROOT / "scripts" / f"{name}.py"
        )
        assert spec and spec.loader
        module = importlib.util.module_from_spec(spec)
        sys.path.insert(0, str(ROOT / "scripts"))
        try:
            spec.loader.exec_module(module)
        finally:
            sys.path.remove(str(ROOT / "scripts"))
        return module

    def contract(self) -> dict[str, object]:
        return json.loads((ROOT / "spec/evidence-verification-profiles.json").read_text())

    def test_migrated_recorders_emit_exactly_the_reviewed_measurements(self):
        contract = self.contract()
        by_id = {procedure["id"]: procedure for procedure in contract["procedures"]}
        checked = 0
        for name in self.MIGRATED_RECORDERS:
            module = self.load_recorder(name)
            for record in module.RECORDS.values():
                procedure_id = record["procedure"]
                with self.subTest(procedure=procedure_id):
                    reviewed = by_id[procedure_id]["required_assertions"]
                    self.assertTrue(reviewed)
                    for entry in reviewed:
                        self.assertIsInstance(
                            entry, dict, f"{procedure_id} still has an unmeasured claim"
                        )
                        self.assertIn("measurement", entry)
                    self.assertEqual(
                        [entry["id"] for entry in reviewed],
                        [assertion["id"] for assertion in record["assertions"]],
                    )
                    for entry, assertion in zip(reviewed, record["assertions"], strict=True):
                        self.assertEqual(entry["measurement"], assertion["measurement"])
                checked += 1
        self.assertGreaterEqual(checked, 48)

    def test_no_migrated_procedure_can_qualify_without_measurements(self):
        contract = self.contract()
        procedure_ids: set[str] = set()
        for name in self.MIGRATED_RECORDERS:
            module = self.load_recorder(name)
            procedure_ids.update(record["procedure"] for record in module.RECORDS.values())
        procedure_ids.update(
            {
                "packaged-cpu-final-only-capability-v1",
                "packaged-cpu-residency-states-v1",
                "packaged-cpu-streaming-capability-v1",
                "packaged-cpu-device-qualification-v1",
                "exact-package-supply-chain-v1",
            }
        )
        by_id = {procedure["id"]: procedure for procedure in contract["procedures"]}
        for procedure_id in sorted(procedure_ids):
            with self.subTest(procedure=procedure_id):
                for entry in by_id[procedure_id]["required_assertions"]:
                    self.assertIsInstance(entry, dict)
                    self.assertIn("measurement", entry)

    def test_the_contract_loads_without_errors(self):
        contract, errors = evidence_policy.load_verification_contract(ROOT)
        self.assertIsNotNone(contract)
        self.assertEqual(errors, [])

    def test_a_contract_that_pins_an_unusable_measurement_is_refused(self):
        contract = json.loads((ROOT / "spec/evidence-verification-profiles.json").read_text())
        contract["procedures"] = [
            {
                "id": "broken-v1",
                "verification_id": VERIFICATION_ID,
                "profile_id": "unit-contract-v1",
                "command_argv": ["python3", "-m", "unittest"],
                "permitted_substitutions": [],
                "required_assertions": [
                    {
                        "id": ASSERTION_ID,
                        "measurement": {
                            "metric": "a.b",
                            "comparator": "ge",
                            "expected": "not a number",
                        },
                    }
                ],
            }
        ]
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "spec/schemas").mkdir(parents=True)
            shutil.copy2(
                ROOT / "spec/evidence-verification-profiles.json",
                root / "spec/evidence-verification-profiles.json",
            )
            (root / "spec/evidence-verification-profiles.json").write_text(
                json.dumps(contract, indent=2) + "\n"
            )
            for rel in (
                "spec/schemas/evidence-verification-profiles.schema.json",
            ):
                shutil.copy2(ROOT / rel, root / rel)
            loaded, errors = evidence_policy.load_verification_contract(root)
            self.assertIsNone(loaded)
            self.assertTrue(any("unusable measurement" in error for error in errors), errors)


if __name__ == "__main__":
    unittest.main()
