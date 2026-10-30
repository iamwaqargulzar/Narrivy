"""Negative tests for finding P-5: the gates must not trust self-declared fields.

Each test forges one field that an evidence or release file declares about
itself and checks that the gate computes or cross-checks it and rejects the
forgery. The gates stay fail-closed; nothing here relaxes a check.
"""
from __future__ import annotations

import copy
import hashlib
import json
import sys
import tempfile
import time
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

import evidence_policy  # noqa: E402
import phase_gate  # noqa: E402
import release_gate  # noqa: E402

COMMIT = "a" * 40
OTHER_COMMIT = "b" * 40

BASE_EVIDENCE = {
    "schema_version": 3,
    "verification_id": "VER-TEST-010",
    "requirement_ids": ["TEST-010"],
    "phase": "P11",
    "scenario_id": "none",
    "git_commit": COMMIT,
    "git_dirty": False,
    "build_id": "test-build",
    "environment": {
        "id": "cosmic",
        "native_or_vm": "native",
        "os": "Pop!_OS",
        "os_version": "24.04",
        "kernel": "6.18.0",
        "desktop": "COSMIC",
        "session": "wayland",
        "qualification_lane": "cosmic-wayland",
        "lane_available": True,
    },
    "execution": {
        "lane_available": True,
        "child_failures_ignored": False,
        "substitutions": [],
        "environment_class": "native",
    },
    "evidence_type": "native-platform",
    "command": "true",
    "started_at": "2026-10-30T10:00:00+00:00",
    "ended_at": "2026-10-30T10:05:00+00:00",
    "exit_code": 0,
    "result": "pass",
    "pass": True,
    "artifact_hashes": {},
}


def forged(**changes) -> dict:
    data = copy.deepcopy(BASE_EVIDENCE)
    for key, value in changes.items():
        data[key] = value
    return data


def assess(data: dict, **kwargs) -> list[str]:
    return evidence_policy.assess_evidence(data, current_commit=COMMIT, **kwargs)


class EvidenceFieldTests(unittest.TestCase):
    """Facts every evidence record declares, checked by evidence_policy."""

    def test_baseline_record_is_accepted(self) -> None:
        self.assertEqual(assess(forged(), expected_lane="cosmic-wayland"), [])

    def test_forged_commit_is_rejected(self) -> None:
        errors = assess(forged(git_commit=OTHER_COMMIT))
        self.assertIn("evidence commit does not match the candidate commit", errors)

    def test_forged_pass_with_failing_exit_code_is_rejected(self) -> None:
        errors = assess(forged(exit_code=1))
        self.assertTrue(any("exit_code is not 0" in e for e in errors), errors)

    def test_forged_result_without_pass_flag_is_rejected(self) -> None:
        errors = assess(forged(**{"pass": "true"}))
        self.assertTrue(any("pass flag is not exactly true" in e for e in errors), errors)

    def test_forged_clean_flag_is_rejected(self) -> None:
        self.assertIn("evidence was captured from a dirty worktree", assess(forged(git_dirty=True)))

    def test_forged_environment_lane_is_rejected(self) -> None:
        env = dict(BASE_EVIDENCE["environment"], qualification_lane="gnome-wayland")
        errors = assess(forged(environment=env), expected_lane="cosmic-wayland")
        self.assertTrue(any("!= expected 'cosmic-wayland'" in e for e in errors), errors)

    def test_unavailable_lane_is_rejected(self) -> None:
        env = dict(BASE_EVIDENCE["environment"], lane_available=False)
        self.assertIn("evidence records an unavailable qualification lane", assess(forged(environment=env)))

    def test_reversed_timestamps_are_rejected(self) -> None:
        errors = assess(forged(started_at="2026-10-30T11:00:00+00:00"))
        self.assertIn("evidence ended_at precedes started_at", errors)

    def test_malformed_timestamp_is_rejected(self) -> None:
        errors = assess(forged(ended_at="yesterday"))
        self.assertTrue(any("timestamp is invalid" in e for e in errors), errors)

    def test_timestamp_without_zone_is_rejected(self) -> None:
        errors = assess(forged(ended_at="2026-10-30T10:05:00"))
        self.assertTrue(any("no time zone" in e for e in errors), errors)

    def test_nan_and_infinity_are_rejected(self) -> None:
        for token in ("NaN", "Infinity", "-Infinity"):
            with self.subTest(token=token), self.assertRaises(ValueError):
                evidence_policy.loads_strict('{"metrics": {"p95_ms": %s}}' % token)
        self.assertEqual(evidence_policy.loads_strict('{"x": 1.5}'), {"x": 1.5})

    def test_forged_artifact_hash_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "bin").write_bytes(b"real bytes")
            data = forged(artifact_hashes={"bin": hashlib.sha256(b"other bytes").hexdigest()})
            self.assertEqual(
                evidence_policy.verify_artifact_hashes(data, root=root), ["artifact hash mismatch: bin"]
            )
            data["artifact_hashes"] = {"bin": hashlib.sha256(b"real bytes").hexdigest()}
            self.assertEqual(evidence_policy.verify_artifact_hashes(data, root=root), [])


class ReleaseGateTests(unittest.TestCase):
    """Facts release files declare, recomputed by release_gate."""

    def test_nan_evidence_file_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "ev.json"
            path.write_text(json.dumps(forged()).replace('"exit_code": 0', '"exit_code": NaN'))
            errors = release_gate.validate_evidence(path, commit=COMMIT)
            self.assertEqual(len(errors), 1)
            self.assertIn("non-finite JSON number NaN", errors[0])

    def test_dirty_tree_is_measured_not_declared(self) -> None:
        porcelain = " M scripts/release_gate.py\n?? notes.txt\n"
        errors = release_gate.worktree_errors(porcelain)
        self.assertEqual(len(errors), 2)

    def test_release_outputs_may_differ(self) -> None:
        porcelain = (
            " M spec/requirements-traceability.csv\n"
            "?? agent/evidence/P11/VER-TEST-010.json\n"
            " M BUILD-PACK-MANIFEST.json\n"
            "?? artifacts/release-evidence/linux-1.0/summary.md\n"
        )
        self.assertEqual(release_gate.worktree_errors(porcelain), [])

    def test_unreadable_git_status_fails_closed(self) -> None:
        self.assertTrue(release_gate.worktree_errors(None))

    def _rows(self) -> list[dict]:
        return [
            {"requirement_id": rid, "release_gate": "required"}
            for rid in ("TEST-010", "PORT-006", "ACC-001", "TEST-007", "PRIV-002", "SEC-005")
        ] + [{"requirement_id": "ACC-099", "release_gate": "roadmap"}]

    def _flags(self, value: bool) -> dict:
        return {key: value for key in release_gate.QUALIFICATION_FLAG_REQUIREMENTS}

    def test_forged_qualification_flags_are_rejected(self) -> None:
        accepted = {"TEST-010", "PORT-006", "TEST-007", "PRIV-002", "SEC-005"}  # ACC-001 not accepted
        errors = release_gate.qualification_flag_errors(self._flags(True), self._rows(), accepted)
        self.assertTrue(any("accessibility_pass: requirements not accepted by the gate: ACC-001" in e for e in errors))
        self.assertTrue(any("accessibility_pass=True disagrees" in e for e in errors))
        self.assertEqual(len(errors), 2)

    def test_flags_follow_accepted_evidence(self) -> None:
        accepted = {row["requirement_id"] for row in self._rows()}
        self.assertEqual(release_gate.qualification_flag_errors(self._flags(True), self._rows(), accepted), [])
        errors = release_gate.qualification_flag_errors(self._flags(False), self._rows(), accepted)
        self.assertEqual(len(errors), 3)

    def _lane(self) -> dict:
        env = BASE_EVIDENCE["environment"]
        return {field: env[field] for field in release_gate.LANE_ENVIRONMENT_FIELDS} | {"git_commit": COMMIT}

    def test_lane_matches_its_evidence(self) -> None:
        self.assertEqual(release_gate.lane_environment_errors("cosmic-wayland", self._lane(), forged(), COMMIT), [])

    def test_forged_native_lane_is_rejected(self) -> None:
        evidence = forged(environment=dict(BASE_EVIDENCE["environment"], native_or_vm="vm"))
        errors = release_gate.lane_environment_errors("cosmic-wayland", self._lane(), evidence, COMMIT)
        self.assertIn("cosmic-wayland evidence was not recorded natively", errors)

    def test_forged_lane_environment_is_rejected(self) -> None:
        lane = self._lane() | {"kernel": "6.99.0"}
        errors = release_gate.lane_environment_errors("cosmic-wayland", lane, forged(), COMMIT)
        self.assertEqual(errors, ["cosmic-wayland qualification kernel '6.99.0' != evidence '6.18.0'"])

    def test_forged_lane_commit_is_rejected(self) -> None:
        lane = self._lane() | {"git_commit": OTHER_COMMIT}
        errors = release_gate.lane_environment_errors("cosmic-wayland", lane, forged(), COMMIT)
        self.assertIn("cosmic-wayland qualification/evidence commit is not the candidate commit", errors)

    def test_forged_performance_metric_is_rejected(self) -> None:
        evidence = forged(metrics={"hot_decode": {"p50_ms": 120.0, "p95_ms": 480.0}})
        perf = {"metrics": {"hot_decode": {"p50_ms": 120.0, "p95_ms": 200.0}}}
        errors = release_gate.performance_metric_errors(perf, evidence)
        self.assertEqual(errors, ["performance metric hot_decode.p95_ms 200.0 != evidence 480.0"])
        perf["metrics"]["cold"] = {"p50_ms": 1, "p95_ms": 2}
        self.assertIn(
            "performance metric cold is not in the performance evidence",
            release_gate.performance_metric_errors(perf, evidence),
        )

    def test_summary_must_name_the_commit(self) -> None:
        text = "Narrivy 1.0 qualified on Pop!_OS COSMIC Wayland. " * 10
        self.assertIn(
            "release summary does not name the candidate commit (full SHA)",
            release_gate.summary_errors(text, COMMIT),
        )
        self.assertEqual(release_gate.summary_errors(text + COMMIT, COMMIT), [])

    def test_p11_json_report_is_mandatory(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            errors = release_gate.p11_report_errors(Path(tmp) / "P11.json", COMMIT)
        self.assertEqual(errors, ["missing P11 JSON phase report agent/phase-reports/P11.json"])


BLOCKERS = """# External Blockers

## B-A-001 — Open release blocker [OPEN]
- Phase/Requirement: All requirements; P11 TEST-010 and release gate.

## B-A-002 — Done [RESOLVED 2026-08-21]
- Phase/Requirement: P11 legal metadata

## B-A-003 — Waived [PENDING — owner plan; not blocking now]
- Phase/Requirement: P3 FUN-001

## B-A-004 — Header says narrowed [NARROWED 2026-08-22]
- Phase/Requirement: P10 release
- Status: **OPEN** — review still required.

## B-A-005 — Status says resolved [OPEN]
- Phase/Requirement: P4 capture
- Status: **RESOLVED** — fixed.

## B-A-006 — Locally resolved only [LOCALLY RESOLVED; RELEASE EVIDENCE PENDING]
- Phase/Requirement: SEC-005, P8 daemon
"""


class PhaseGateTests(unittest.TestCase):
    def _blocked(self, phase: str) -> list[str]:
        return [e.split(": ", 1)[1].split(" — ")[0] for e in phase_gate.blocker_errors(phase, BLOCKERS)]

    def test_open_blocker_without_blocks_wording_is_found(self) -> None:
        self.assertEqual(self._blocked("P11"), ["## B-A-001"])

    def test_phase_tokens_do_not_overlap(self) -> None:
        self.assertEqual(self._blocked("P1"), [])

    def test_resolved_and_waived_blockers_do_not_block(self) -> None:
        self.assertEqual(self._blocked("P3"), [])
        self.assertEqual(self._blocked("P4"), [])

    def test_status_line_overrides_header(self) -> None:
        self.assertEqual(self._blocked("P10"), ["## B-A-004"])

    def test_unknown_status_fails_closed(self) -> None:
        self.assertEqual(self._blocked("P8"), ["## B-A-006"])

    def test_markdown_report_is_not_a_fallback(self) -> None:
        self.assertEqual(
            phase_gate.phase_report_errors("PX-none", COMMIT),
            ["missing JSON phase report agent/phase-reports/PX-none.json"],
        )

    def test_raw_native_field_does_not_count_as_native(self) -> None:
        # Declares native, result pass; would have counted under the old raw check.
        data = forged(phase="P11", git_commit=OTHER_COMMIT, schema_version=1)
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "ev.json"
            path.write_text(json.dumps(data))
            self.assertFalse(phase_gate.verified_native(path, phase="P11", commit=COMMIT))

    def test_nan_evidence_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "ev.json"
            path.write_text('{"exit_code": Infinity}')
            data, errors = phase_gate.read_evidence(path)
        self.assertIsNone(data)
        self.assertIn("non-finite JSON number Infinity", errors[0])

    def test_commands_time_out_and_their_group_is_killed(self) -> None:
        started = time.monotonic()
        rc = phase_gate.run_command("sh -c 'sleep 30 & sleep 30'", timeout=1)
        self.assertEqual(rc, 124)
        self.assertLess(time.monotonic() - started, 10)


if __name__ == "__main__":
    unittest.main()
