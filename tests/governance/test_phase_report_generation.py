"""Governance tests for the generated phase reports.

``scripts/generate_phase_report.py`` replaced hand-typed progress figures, and
these tests are the teeth behind that: the committed report must equal what the
traceability CSV, the evidence index and the on-disk evidence tree actually say,
and the generator must be structurally incapable of reporting ``passed`` over a
red gate or a blocked requirement set. They also fail closed, so a missing or
malformed input aborts generation instead of producing a plausible-looking
report.
"""
from __future__ import annotations

import csv
import hashlib
import io
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import jsonschema

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

import generate_phase_report as generator

REPORTS_DIR = ROOT / "agent/phase-reports"
GENERATOR = ROOT / "scripts/generate_phase_report.py"
PHASES = ("P0", "P0F", "P1", "P2", "P3", "P4", "P5", "P6", "P7", "P8", "P9", "P10", "P11")

CSV_HEADER = (
    "requirement_id",
    "severity",
    "scope",
    "primary_phase",
    "secondary_phase",
    "component",
    "acceptance_summary",
    "verification_id",
    "verification_layer",
    "release_gate",
    "status",
    "evidence",
    "evidence_classification",
    "evidence_disposition",
)

# The report schema forbids any status claim the schema itself does not allow.
ALLOWED_STATUS = {"passed", "failed", "not_started", "in_progress"}


def committed_reports() -> list[Path]:
    return sorted(
        path for path in REPORTS_DIR.glob("P*.json") if path.stem in PHASES
    )


def run_generator(root: Path, *args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(GENERATOR), "--root", str(root), *args],
        cwd=root,
        capture_output=True,
        text=True,
        timeout=120,
        check=False,
    )


def git(root: Path, *argv: str) -> subprocess.CompletedProcess[str]:
    env = {
        **os.environ,
        "GIT_AUTHOR_NAME": "phase report fixture",
        "GIT_AUTHOR_EMAIL": "fixture@narrivy.invalid",
        "GIT_COMMITTER_NAME": "phase report fixture",
        "GIT_COMMITTER_EMAIL": "fixture@narrivy.invalid",
        "GIT_AUTHOR_DATE": "2026-01-01T00:00:00+00:00",
        "GIT_COMMITTER_DATE": "2026-01-01T00:00:00+00:00",
    }
    return subprocess.run(
        ["git", "-C", str(root), *argv], capture_output=True, text=True, env=env, check=False
    )


class CommittedReportMatchesAuthoritativeDataTests(unittest.TestCase):
    """A hand-edited total in a committed report must fail here."""

    @classmethod
    def setUpClass(cls) -> None:
        cls.rows = generator.load_traceability(ROOT)
        cls.index = generator.traceability_index(cls.rows)
        cls.evidence = generator.load_evidence_inventory(ROOT)

    def test_at_least_one_phase_report_is_committed(self) -> None:
        self.assertTrue(committed_reports(), "no committed phase report JSON was found")

    def test_every_committed_report_equals_the_traceability_csv(self) -> None:
        reports = committed_reports()
        self.assertTrue(reports)
        for path in reports:
            with self.subTest(report=path.name):
                report = json.loads(path.read_text())
                phase = report["phase"]
                derived = generator.derive_deterministic_metrics(
                    self.rows, phase, self.evidence
                )
                for key in generator.DETERMINISTIC_METRIC_KEYS:
                    if key not in derived:
                        continue
                    self.assertEqual(
                        report["metrics"].get(key),
                        derived[key],
                        f"{path.name}: metrics.{key} disagrees with the authoritative data",
                    )
                self.assertEqual(
                    report["requirements"],
                    generator.derive_requirements(self.rows, phase),
                    f"{path.name}: requirement list disagrees with the CSV",
                )

    def test_every_committed_report_validates_against_the_schema(self) -> None:
        schema = json.loads((ROOT / "spec/schemas/phase-report.schema.json").read_text())
        for path in committed_reports():
            with self.subTest(report=path.name):
                jsonschema.validate(instance=json.loads(path.read_text()), schema=schema)

    def test_reports_name_only_requirements_that_exist_in_the_csv(self) -> None:
        for path in committed_reports():
            with self.subTest(report=path.name):
                report = json.loads(path.read_text())
                for item in report["requirements"]:
                    rid = item["requirement_id"]
                    self.assertIn(
                        rid, self.index, f"{path.name}: {rid} is not a traceability requirement"
                    )
                    row = self.index[rid]
                    self.assertEqual(item["verification_id"], row["verification_id"])
                    self.assertEqual(
                        item["status"],
                        generator.STATUS_TO_REPORT[row["status"]],
                        f"{path.name}: {rid} status does not mirror the CSV",
                    )

    def test_reports_cannot_claim_passing_over_a_red_gate_or_open_requirements(self) -> None:
        for path in committed_reports():
            with self.subTest(report=path.name):
                report = json.loads(path.read_text())
                self.assertIn(report["status"], ALLOWED_STATUS)
                if report["status"] != "passed":
                    continue
                self.assertEqual(report["gate_exit_code"], 0)
                self.assertEqual(
                    report["metrics"]["release_gated_requirements_blocked"],
                    0,
                    f"{path.name}: passed while release-gated requirements remain open",
                )

    def test_reports_only_attest_to_immutable_release_artifacts(self) -> None:
        for path in committed_reports():
            report = json.loads(path.read_text())
            for artifact in report["artifacts"]:
                with self.subTest(report=path.name, artifact=artifact["path"]):
                    self.assertTrue(
                        artifact["path"].startswith("artifacts/"),
                        f"{path.name}: a mutable working-tree file was hashed into a snapshot",
                    )
                    target = ROOT / artifact["path"]
                    self.assertTrue(target.is_file(), f"{path.name}: {artifact['path']} is gone")
                    self.assertEqual(
                        hashlib.sha256(target.read_bytes()).hexdigest(), artifact["sha256"]
                    )

    def test_generated_totals_match_the_headline_the_report_narrates(self) -> None:
        checked = 0
        for path in sorted(REPORTS_DIR.glob("P*.md")):
            found = generator.extract_headline_block(path.read_text())
            if found is None:
                continue
            checked += 1
            with self.subTest(report=path.name):
                report = json.loads((REPORTS_DIR / f"{path.stem}.json").read_text())
                metrics = generator.derive_deterministic_metrics(
                    self.rows, report["phase"], self.evidence
                )
                self.assertEqual(
                    found,
                    generator.headline_block(metrics),
                    f"{path.name}: the authoritative headline block is stale",
                )
        self.assertGreater(checked, 0, "no narrative report carries a generated headline block")

    def test_p11_narrative_states_the_authoritative_counts(self) -> None:
        text = (REPORTS_DIR / "P11.md").read_text()
        self.assertIsNotNone(
            generator.extract_headline_block(text),
            "P11.md must carry the generated authoritative headline block",
        )
        expected = generator.derive_deterministic_metrics(self.rows, "P11", self.evidence)
        self.assertIn(
            f"| Release-gated not passed | {expected['release_gated_requirements_blocked']} |",
            text,
        )


class GeneratorFailsClosedTests(unittest.TestCase):
    """An unreadable or unusable input must abort, not emit a plausible report."""

    def test_missing_traceability_aborts_without_writing(self) -> None:
        with self.subTest(phase="P11"):
            with tempfile.TemporaryDirectory() as tmp:
                result = run_generator(
                    Path(tmp),
                    "--phase",
                    "P11",
                    "--gate-command",
                    f"{sys.executable} -c \"raise SystemExit(0)\"",
                )
                self.assertEqual(result.returncode, generator.EXIT_FAILURE)
                self.assertIn("cannot read", result.stderr)
                self.assertEqual(list(Path(tmp).rglob("P11.json")), [])

    def test_unknown_phase_aborts(self) -> None:
        with fixture([row("FUN-001", "P11", "passed")]) as root:
            result = run_generator(root, "--phase", "P99", "--headline")
            self.assertEqual(result.returncode, generator.EXIT_FAILURE)
            self.assertIn("unknown phase P99", result.stderr)

    def test_unmappable_traceability_status_aborts(self) -> None:
        with fixture([row("FUN-001", "P11", "mostly-passed")]) as root:
            result = run_generator(
                root, "--phase", "P11", "--gate-command", gate_command(0)
            )
            self.assertEqual(result.returncode, generator.EXIT_FAILURE)
            self.assertIn("has no phase-report mapping", result.stderr)
            self.assertEqual(list((root / "agent/phase-reports").rglob("P11.json")), [])

    def test_duplicate_requirement_id_aborts(self) -> None:
        with fixture([row("FUN-001", "P11", "passed")]) as root:
            write_csv(root, [row("FUN-001", "P11", "passed"), row("FUN-001", "P11", "blocked")])
            result = run_generator(
                root, "--phase", "P11", "--gate-command", gate_command(0)
            )
            self.assertEqual(result.returncode, generator.EXIT_FAILURE)
            self.assertIn("duplicate requirement_id", result.stderr)

    def test_malformed_requirement_id_aborts(self) -> None:
        with fixture([row("FUN-001", "P11", "passed")]) as root:
            write_csv(root, [row("fun-1", "P11", "passed")])
            result = run_generator(
                root, "--phase", "P11", "--gate-command", gate_command(0)
            )
            self.assertEqual(result.returncode, generator.EXIT_FAILURE)
            self.assertIn("requirement_id is malformed", result.stderr)

    def test_evidence_index_hash_mismatch_is_reported_not_hidden(self) -> None:
        with fixture([row("FUN-001", "P11", "passed")]) as root:
            index_path = root / "agent/evidence/INDEX.json"
            index = json.loads(index_path.read_text())
            index["qualified_evidence"][0]["sha256"] = "0" * 64
            index_path.write_text(json.dumps(index, indent=2))
            evidence = generator.load_evidence_inventory(root)
            self.assertEqual(evidence["hash_mismatches"], ["agent/evidence/P11/VER-FUN-001.json"])

    def test_a_requirement_id_absent_from_the_csv_is_refused(self) -> None:
        index = {"FUN-001": {"verification_id": "VER-FUN-001", "status": "passed"}}
        report = {
            "requirements": [
                {
                    "requirement_id": "FUN-999",
                    "verification_id": "VER-FUN-999",
                    "status": "passed",
                    "evidence": "agent/evidence/P11/VER-FUN-999.json",
                }
            ]
        }
        with self.assertRaises(generator.GeneratorError) as caught:
            generator.validate_requirement_ids(report, index)
        self.assertIn("FUN-999", str(caught.exception))


class GeneratorCannotWashTheGateGreenTests(unittest.TestCase):
    """A green exit code alone must never produce ``status: passed``."""

    def test_green_gate_over_blocked_requirements_still_reports_not_passed(self) -> None:
        with fixture(
            [row("FUN-001", "P11", "passed"), row("FUN-002", "P11", "blocked")]
        ) as root:
            report = generate(root, "P11", gate_exit=0)
            self.assertEqual(report["gate_exit_code"], 0)
            self.assertNotEqual(report["status"], "passed")
            self.assertEqual(report["metrics"]["release_gated_requirements_blocked"], 1)

    def test_red_gate_over_fully_passed_requirements_is_not_a_pass(self) -> None:
        with fixture([row("FUN-001", "P11", "passed")]) as root:
            report = generate(root, "P11", gate_exit=1)
            self.assertNotEqual(report["status"], "passed")
            self.assertEqual(report["metrics"]["release_gated_requirements_blocked"], 0)

    def test_green_gate_over_fully_passed_requirements_does_pass(self) -> None:
        with fixture([row("FUN-001", "P11", "passed")]) as root:
            report = generate(root, "P11", gate_exit=0)
            self.assertEqual(report["status"], "passed")
            self.assertEqual(report["gate_exit_code"], 0)

    def test_validate_claims_rejects_a_hand_forged_passing_report(self) -> None:
        internally_consistent = {
            "status": "passed",
            "gate_exit_code": 0,
            "requirements": [],
            "metrics": {
                "release_gated_requirements_total": 94,
                "release_gated_requirements_passed": 94,
                "release_gated_requirements_blocked": 0,
                "phase_primary_release_gated_requirements": 1,
                "phase_primary_release_gated_passed": 1,
                "phase_primary_release_gated_blocked": 0,
            },
        }
        generator.validate_claims(internally_consistent)

        red_gate = {**internally_consistent, "gate_exit_code": 1}
        with self.assertRaises(generator.GeneratorError):
            generator.validate_claims(red_gate)

        open_requirements = {
            **internally_consistent,
            "metrics": {
                **internally_consistent["metrics"],
                "release_gated_requirements_passed": 39,
                "release_gated_requirements_blocked": 55,
            },
        }
        with self.assertRaises(generator.GeneratorError):
            generator.validate_claims(open_requirements)

        mismatched_tally = {
            **internally_consistent,
            "metrics": {
                **internally_consistent["metrics"],
                "release_gated_requirements_passed": 90,
                "release_gated_requirements_blocked": 0,
            },
        }
        with self.assertRaises(generator.GeneratorError):
            generator.validate_claims(mismatched_tally)


class TamperedCommittedReportIsDetectedTests(unittest.TestCase):
    """The CI hook must fail on a report whose numbers were typed by hand."""

    def test_check_reports_drift_for_a_tampered_count(self) -> None:
        with fixture([row("FUN-001", "P11", "passed"), row("FUN-002", "P11", "blocked")]) as root:
            generate(root, "P11", gate_exit=0)
            self.assertEqual(run_generator(root, "--phase", "P11", "--check").returncode, 0)

            report_path = root / "agent/phase-reports/P11.json"
            tampered = json.loads(report_path.read_text())
            tampered["metrics"]["release_gated_requirements_passed"] = 2
            tampered["metrics"]["release_gated_requirements_blocked"] = 55
            report_path.write_text(json.dumps(tampered, indent=2))

            result = run_generator(root, "--phase", "P11", "--check")
            self.assertEqual(result.returncode, generator.EXIT_DRIFT)
            self.assertIn("metrics.release_gated_requirements_passed", result.stderr)

    def test_check_reports_drift_for_a_forged_requirement_list(self) -> None:
        with fixture([row("FUN-001", "P11", "passed"), row("FUN-002", "P11", "blocked")]) as root:
            report_path = root / "agent/phase-reports/P11.json"
            generate(root, "P11", gate_exit=0)
            tampered = json.loads(report_path.read_text())
            tampered["requirements"] = [
                {
                    "requirement_id": "FUN-002",
                    "verification_id": "VER-FUN-002",
                    "status": "passed",
                    "evidence": "agent/evidence/P11/VER-FUN-002.json",
                }
            ]
            report_path.write_text(json.dumps(tampered, indent=2))
            result = run_generator(root, "--phase", "P11", "--check")
            self.assertEqual(result.returncode, generator.EXIT_DRIFT)
            self.assertIn("requirements", result.stderr)


# --------------------------------------------------------------------------- #
# fixture
# --------------------------------------------------------------------------- #
def row(requirement_id: str, primary_phase: str, status: str) -> dict[str, str]:
    return {
        "requirement_id": requirement_id,
        "severity": "must",
        "scope": "linux-1.0",
        "primary_phase": primary_phase,
        "secondary_phase": "",
        "component": "fixture",
        "acceptance_summary": f"fixture acceptance for {requirement_id}",
        "verification_id": f"VER-{requirement_id}",
        "verification_layer": "linux-integration",
        "release_gate": "required",
        "status": status,
        "evidence": f"agent/evidence/{primary_phase}/VER-{requirement_id}.json",
        "evidence_classification": "native-platform",
        "evidence_disposition": "historical-quarantined",
    }


def write(root: Path, rel: str, text: str) -> None:
    path = root / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def write_csv(root: Path, rows: list[dict[str, str]]) -> None:
    buffer = io.StringIO()
    writer = csv.DictWriter(buffer, fieldnames=CSV_HEADER, lineterminator="\n")
    writer.writeheader()
    for entry in rows:
        writer.writerow(entry)
    write(root, "spec/requirements-traceability.csv", buffer.getvalue())


def gate_command(exit_code: int) -> str:
    return f"{sys.executable} -c \"raise SystemExit({exit_code})\""


def build_fixture(root: Path, rows: list[dict[str, str]]) -> None:
    write_csv(root, rows)
    write(
        root,
        "agent/PHASES.json",
        json.dumps(
            {
                "schema_version": 1,
                "phases": [
                    {
                        "id": "P11",
                        "name": "fixture",
                        "spec": "docs/fixture.md",
                        "prompt": "docs/prompt.md",
                        "acceptance": "spec/acceptance/P11.json",
                        "gate": gate_command(0),
                        "working_app_required": True,
                    }
                ],
            },
            indent=2,
        ),
    )
    write(
        root,
        "spec/acceptance/P11.json",
        json.dumps(
            {
                "schema_version": 2,
                "phase": "P11",
                "implemented": True,
                "primary_requirements": [entry["requirement_id"] for entry in rows],
                "required_artifacts": ["artifacts/present.bin", "artifacts/absent.bin"],
                "commands": [],
                "smoke_commands": [],
            },
            indent=2,
        ),
    )
    write(
        root,
        "spec/schemas/phase-report.schema.json",
        (ROOT / "spec/schemas/phase-report.schema.json").read_text(),
    )
    write(root, "BUILD-PACK-VERSION", (ROOT / "BUILD-PACK-VERSION").read_text())
    write(root, "artifacts/present.bin", "present\n")

    registered = []
    for entry in rows:
        rel = entry["evidence"]
        record = {
            "schema_version": 2,
            "verification_id": entry["verification_id"],
            "requirement_ids": [entry["requirement_id"]],
            "phase": entry["primary_phase"],
            "started_at": "2026-01-01T00:00:00+00:00",
            "ended_at": "2026-01-01T00:00:01+00:00",
        }
        write(root, rel, json.dumps(record, indent=2))
        registered.append(
            {
                "disposition": "qualified",
                "path": rel,
                "sha256": generator.hashlib.sha256((root / rel).read_bytes()).hexdigest(),
            }
        )
    write(
        root,
        "agent/evidence/INDEX.json",
        json.dumps(
            {
                "legacy_default": {
                    "disposition": "historical-quarantined",
                    "eligible_for_release": False,
                    "reason": "fixture",
                },
                "policy": "release-evidence-v2-required",
                "qualified_evidence": registered,
                "schema_version": 1,
            },
            indent=2,
        ),
    )
    git(root, "init", "-q", str(root))
    git(root, "add", "-A")
    git(root, "commit", "-q", "-m", "phase report fixture")


def generate(root: Path, phase: str, *, gate_exit: int) -> dict:
    result = run_generator(
        root, "--phase", phase, "--gate-command", gate_command(gate_exit)
    )
    if result.returncode != 0:
        raise AssertionError(f"generator failed: {result.returncode}\n{result.stderr}")
    return json.loads((root / f"agent/phase-reports/{phase}.json").read_text())


class fixture:
    """Build a throwaway repository that satisfies every generator input."""

    def __init__(self, rows: list[dict[str, str]]) -> None:
        self.rows = rows
        self._tmp: tempfile.TemporaryDirectory[str] | None = None

    def __enter__(self) -> Path:
        self._tmp = tempfile.TemporaryDirectory()
        root = Path(self._tmp.name)
        build_fixture(root, self.rows)
        return root

    def __exit__(self, *_exc: object) -> None:
        if self._tmp is not None:
            self._tmp.cleanup()


if __name__ == "__main__":
    unittest.main()
