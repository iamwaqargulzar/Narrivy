"""One test group per P-4 recorder defect (WP4 T4.5).

Each class names the defect it pins. The tests run without the native lane,
a package or a microphone: they exercise the recorder logic that decided
the defect.
"""
from __future__ import annotations

import hashlib
import json
import os
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

import evidence_policy  # noqa: E402
import phase_gate  # noqa: E402
import qualify_candidate_known_phrase as known_phrase  # noqa: E402
import record_controlled_integration_evidence as controlled  # noqa: E402
import record_cosmic_vertical_slice_evidence as vertical_slice  # noqa: E402
import record_cpu_performance_evidence as cpu  # noqa: E402
import record_native_accessibility_evidence as accessibility  # noqa: E402
import record_native_hardening_evidence as hardening  # noqa: E402
import record_native_ui_dictation_evidence as ui  # noqa: E402
import record_package_upgrade_evidence as upgrade_recorder  # noqa: E402
import record_unit_contract_evidence as unit_contract  # noqa: E402
import recorder_safety  # noqa: E402


def sha(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


class HardeningAssertionsAreNotWrappedTests(unittest.TestCase):
    """Defect 1: hardening wrapped each assertion dict as an ID."""

    def test_procedure_emits_measured_assertions_directly(self) -> None:
        for requirement, record in hardening.RECORDS.items():
            with self.subTest(requirement=requirement):
                procedure = hardening.procedure_for(record)
                for assertion in procedure["assertions"]:
                    self.assertIsInstance(assertion["id"], str)
                    self.assertIn("measurement", assertion)
                    self.assertIsNone(evidence_policy.measurement_shape_error(assertion["measurement"]))

    def test_main_uses_the_unwrapped_procedure(self) -> None:
        source = Path(hardening.__file__).read_text()
        self.assertIn('"procedure": procedure_for(record)', source)
        self.assertNotIn('{"id": assertion, "result": "pass"} for assertion in record["assertions"]', source)


class GuardedReplacementTests(unittest.TestCase):
    """Defect 2: recorders refused to replace their own indexed v2 outputs."""

    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.path = self.root / "agent/evidence/P1/VER-X.json"
        self.path.parent.mkdir(parents=True)
        self.index_path = self.root / "agent/evidence/INDEX.json"
        self.index: dict[str, object] = {"qualified_evidence": []}

    def write(self, version: int, *, indexed: bool, good_hash: bool = True) -> bytes:
        payload = (json.dumps({"schema_version": version}) + "\n").encode()
        self.path.write_bytes(payload)
        if indexed:
            self.index["qualified_evidence"] = [
                {"path": "agent/evidence/P1/VER-X.json", "sha256": sha(payload) if good_hash else "0" * 64,
                 "disposition": "qualified"}
            ]
        return payload

    def test_policy(self) -> None:
        self.assertTrue(recorder_safety.replaceable(self.path, self.index, self.root))
        self.write(1, indexed=False)
        self.assertTrue(recorder_safety.replaceable(self.path, self.index, self.root))
        self.write(2, indexed=False)
        self.assertFalse(recorder_safety.replaceable(self.path, self.index, self.root))
        self.write(2, indexed=True)
        self.assertTrue(recorder_safety.replaceable(self.path, self.index, self.root))
        self.write(1, indexed=True)
        self.assertTrue(recorder_safety.replaceable(self.path, self.index, self.root))
        self.write(2, indexed=True, good_hash=False)
        self.assertFalse(recorder_safety.replaceable(self.path, self.index, self.root))
        self.write(3, indexed=True)
        self.assertFalse(recorder_safety.replaceable(self.path, self.index, self.root))

    def test_commit_replaces_file_and_index_entry_together(self) -> None:
        self.write(2, indexed=True)
        new = b'{"schema_version": 3}\n'
        updated = recorder_safety.commit_records(self.root, self.index_path, self.index, [(self.path, new)])
        self.assertEqual(self.path.read_bytes(), new)
        on_disk = json.loads(self.index_path.read_text())
        self.assertEqual(on_disk, updated)
        self.assertEqual(on_disk["qualified_evidence"], [
            {"path": "agent/evidence/P1/VER-X.json", "sha256": sha(new), "disposition": "qualified"}])

    def test_failed_commit_restores_every_file(self) -> None:
        old = self.write(2, indexed=True)
        other = self.root / "agent/evidence/P1/VER-Y.json"
        self.index_path.write_text(json.dumps(self.index))
        index_before = self.index_path.read_bytes()
        calls = []

        def flaky(path: Path, payload: bytes) -> None:
            calls.append(path)
            if path == self.index_path:
                raise OSError("disk full")
            recorder_safety._atomic_write(path, payload)

        with self.assertRaises(OSError):
            recorder_safety.commit_records(
                self.root, self.index_path, self.index,
                [(self.path, b'{"schema_version": 3}\n'), (other, b'{"schema_version": 3}\n')],
                write=flaky,
            )
        self.assertEqual(self.path.read_bytes(), old)
        self.assertFalse(other.exists())
        self.assertEqual(self.index_path.read_bytes(), index_before)

    def test_commit_refuses_current_record(self) -> None:
        self.write(3, indexed=True)
        with self.assertRaises(recorder_safety.RecorderSafetyError):
            recorder_safety.commit_records(self.root, self.index_path, self.index, [(self.path, b"{}\n")])


class NativeLaneTests(unittest.TestCase):
    """Defect 3: native_or_vm was hard-coded or taken from spoofable env strings."""

    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.os_release = self.root / "os-release"
        self.os_release.write_text('NAME="Pop!_OS"\nID=pop\nVERSION_ID="24.04"\nPRETTY_NAME="Pop!_OS 24.04 LTS"\n')
        self.runtime = self.root / "run"
        self.runtime.mkdir()
        self.server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.addCleanup(self.server.close)
        self.server.bind(str(self.runtime / "wayland-1"))
        self.filesystem = self.root / "fs"
        self.filesystem.mkdir()
        self.env = {
            "XDG_CURRENT_DESKTOP": "COSMIC",
            "XDG_SESSION_TYPE": "wayland",
            "WAYLAND_DISPLAY": "wayland-1",
            "XDG_RUNTIME_DIR": str(self.runtime),
        }

    def lane(self, *, env=None, virt="none", container="none"):
        def detect(argv):
            return container if "--container" in argv else virt

        return recorder_safety.native_lane(
            environ=env or self.env, os_release_path=self.os_release,
            detect_virt=detect, filesystem_root=self.filesystem,
        )

    def test_bare_metal_pop_cosmic_wayland_passes(self) -> None:
        lane = self.lane()
        self.assertEqual(lane["os_id"], "pop")
        self.assertEqual(lane["virtualization"], "none")

    def test_spoofed_or_wrong_environments_fail(self) -> None:
        cases = {
            "vm": dict(virt="kvm"),
            "container": dict(container="docker"),
            "no wayland socket": dict(env={**self.env, "WAYLAND_DISPLAY": "wayland-9"}),
            "not cosmic": dict(env={**self.env, "XDG_CURRENT_DESKTOP": "GNOME"}),
            "x11": dict(env={**self.env, "XDG_SESSION_TYPE": "x11"}),
        }
        for name, kwargs in cases.items():
            with self.subTest(name), self.assertRaises(recorder_safety.RecorderSafetyError):
                self.lane(**kwargs)

    def test_wrong_os_and_container_marker_fail(self) -> None:
        (self.filesystem / ".dockerenv").write_text("")
        with self.assertRaisesRegex(recorder_safety.RecorderSafetyError, "container marker"):
            self.lane()
        (self.filesystem / ".dockerenv").unlink()
        self.os_release.write_text('ID=ubuntu\nVERSION_ID="24.04"\n')
        with self.assertRaisesRegex(recorder_safety.RecorderSafetyError, "Pop!_OS 24.04"):
            self.lane()

    def test_known_phrase_qualifier_uses_the_shared_lane(self) -> None:
        class Ops:
            def environ(self):
                return {"XDG_CURRENT_DESKTOP": "COSMIC", "XDG_SESSION_TYPE": "wayland", "WAYLAND_DISPLAY": "w"}

            def containerized(self):
                return False

            def native_lane(self):
                raise recorder_safety.RecorderSafetyError("native lane requires bare metal")

        with self.assertRaisesRegex(known_phrase.InputError, "bare metal"):
            known_phrase._require_native_cosmic(Ops())

    def test_recorders_no_longer_trust_environment_strings(self) -> None:
        for module in (cpu, accessibility, hardening):
            with self.subTest(module=module.__name__):
                source = Path(module.__file__).read_text()
                self.assertIn("recorder_safety.native_lane()", source)
                self.assertNotIn('os.environ.get("XDG_CURRENT_DESKTOP"', source)
        audio = (ROOT / "scripts/record_native_audio_policy_evidence.py").read_text()
        self.assertIn("recorder_safety.native_lane()", audio)
        self.assertNotIn('os.environ.get("XDG_CURRENT_DESKTOP"', audio)


class LogPrivacyTests(unittest.TestCase):
    """Defect 4: one fixed phrase was scanned and a missing log counted as clean."""

    TRANSCRIPT = "Please send the quarterly invoice to Marta before Friday."

    def test_transcript_ngram_and_canary_hits_are_counted(self) -> None:
        leak = b"debug: text=send the quarterly invoice\n"
        scan = recorder_safety.scan_log_privacy([leak], transcripts=[self.TRANSCRIPT], canaries=["CANARY-1"])
        self.assertFalse(scan["clean"])
        self.assertGreaterEqual(scan["transcript_ngram_hits"], 1)
        scan = recorder_safety.scan_log_privacy([b"x CANARY-1 y"], transcripts=[self.TRANSCRIPT], canaries=["CANARY-1"])
        self.assertEqual(scan["canary_hits"], 1)
        clean = recorder_safety.scan_log_privacy([b"session committed len=57"], transcripts=[self.TRANSCRIPT])
        self.assertTrue(clean["clean"])
        self.assertNotIn(self.TRANSCRIPT, json.dumps(clean))

    def test_missing_or_unavailable_log_is_a_failure(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            missing = Path(temporary) / "daemon.log"
            with self.assertRaises(recorder_safety.RecorderSafetyError):
                recorder_safety.scan_log_privacy([missing], canaries=["C"])
        with self.assertRaises(recorder_safety.RecorderSafetyError):
            recorder_safety.scan_log_privacy([None], canaries=["C"])
        with self.assertRaises(recorder_safety.RecorderSafetyError):
            recorder_safety.scan_log_privacy([], canaries=["C"])

    def test_known_phrase_daemon_output_fails_when_log_is_missing(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            process = mock.Mock()
            host = known_phrase.HostProcess(process, Path(temporary) / "absent.log")
            with self.assertRaises(known_phrase.StepFailure):
                host.output()

    def test_known_phrase_scan_uses_the_actual_transcript(self) -> None:
        _scanned, hits = known_phrase._scan_logs([b"text=the quarterly invoice to"], transcripts=[self.TRANSCRIPT])
        self.assertGreaterEqual(hits, 1)
        _scanned, hits = known_phrase._scan_logs([b"text=the quarterly invoice to"])
        self.assertEqual(hits, 0)


class CpuFailureInjectionTests(unittest.TestCase):
    """Defect 5: failed_decodes was 0 by construction."""

    class FakeWorker:
        def __init__(self, *, survives_kill: bool = False) -> None:
            self.process = mock.Mock()
            self.process.poll.return_value = None
            self.killed = False
            self.survives_kill = survives_kill

        def decode(self, samples, count, request_id, deadline):
            if self.killed and not self.survives_kill:
                raise cpu.BenchmarkError("worker response was truncated")
            return 12.5, "a" * 64, 40

        def close(self, deadline):
            return 0

    def run_injection(self, *, survives_kill: bool):
        workers = []

        def spawn():
            worker = self.FakeWorker(survives_kill=survives_kill)
            workers.append(worker)
            return worker

        def stop(process):
            for worker in workers:
                if worker.process is process:
                    worker.killed = True

        with mock.patch.object(cpu, "stop_process", side_effect=stop):
            return cpu.inject_failure_and_fallback(spawn, b"", 0, 1e12), workers

    def test_injected_failure_is_observed_and_recovered(self) -> None:
        (measured, digest, size), workers = self.run_injection(survives_kill=False)
        self.assertEqual(measured["injected"], 1)
        self.assertEqual(measured["detected"], 1)
        self.assertTrue(measured["fallback_recovered"])
        self.assertEqual(len(workers), 2)
        self.assertEqual((digest, size), ("a" * 64, 40))

    def test_unobserved_failure_aborts(self) -> None:
        with self.assertRaisesRegex(cpu.BenchmarkError, "not observed"):
            self.run_injection(survives_kill=True)

    def test_binding_is_the_recovery_not_a_constant_rate(self) -> None:
        contract = json.loads((ROOT / "spec/evidence-verification-profiles.json").read_text())
        procedure = next(p for p in contract["procedures"] if p["id"] == "packaged-cpu-device-qualification-v1")
        entry = next(e for e in procedure["required_assertions"] if e["id"] == "failure-and-fallback-recorded")
        self.assertEqual(entry["measurement"], {"metric": "failure_injection.fallback_recovered", "comparator": "is_true"})
        source = Path(cpu.__file__).read_text()
        self.assertNotIn("failed_decodes = 0", source)


class StructuredTestResultTests(unittest.TestCase):
    """Defect 6: a test "ran" when its name appeared, so skipped tests counted."""

    CARGO = (
        "running 3 tests\n"
        "test delivery::tests::fake_backend_routing_order_follows_ladder ... ok\n"
        "test delivery::tests::uinput_opt_in_sits_between_wtype_and_clipboard ... ignored\n"
        "test delivery::tests::all_rungs_exhausted_yields_failed ... FAILED\n"
    )

    def test_cargo_ignored_and_failed_are_not_passes(self) -> None:
        results = recorder_safety.cargo_test_results(self.CARGO)
        recorder_safety.require_tests_passed(results, ["fake_backend_routing_order_follows_ladder"])
        for name in ("uinput_opt_in_sits_between_wtype_and_clipboard", "all_rungs_exhausted_yields_failed", "absent"):
            with self.subTest(name), self.assertRaises(recorder_safety.RecorderSafetyError):
                recorder_safety.require_tests_passed(results, [name])

    def test_unittest_skipped_is_not_a_pass(self) -> None:
        output = (
            "test_a (tests.governance.test_x.T.test_a) ... ok\n"
            "test_b (tests.governance.test_x.T.test_b) ... skipped 'no cargo'\n"
            "test_c (tests.governance.test_x.T.test_c)\nDocstring line ... ok\n"
        )
        results = recorder_safety.unittest_results(output)
        recorder_safety.require_tests_passed(results, ["test_a", "test_c"])
        with self.assertRaises(recorder_safety.RecorderSafetyError):
            recorder_safety.require_tests_passed(results, ["test_b"])

    def vitest(self, *statuses: str) -> dict[str, object]:
        return {"testResults": [{"name": "/x/src/__tests__/A11yHardening.test.tsx", "assertionResults": [
            {"fullName": f"suite t{i}", "title": f"t{i}", "status": status} for i, status in enumerate(statuses)
        ]}]}

    def test_vitest_skipped_file_is_not_a_pass(self) -> None:
        results = recorder_safety.vitest_results(self.vitest("passed", "skipped"))
        recorder_safety.require_tests_passed(results, ["t0"])
        with self.assertRaises(recorder_safety.RecorderSafetyError):
            recorder_safety.require_tests_passed(results, ["A11yHardening.test.tsx"])
        with self.assertRaises(recorder_safety.RecorderSafetyError):
            recorder_safety.require_tests_passed(results, ["t1"])

    def test_accessibility_contract_files_require_every_test_passed(self) -> None:
        report = {"testResults": [
            {"name": f"/x/{name}", "assertionResults": [{"status": "passed"}]}
            for name in accessibility.FRONTEND_CONTRACT_FILES
        ]}
        measured = accessibility.frontend_contract_results(report)
        self.assertTrue(all(measured["contract_files"].values()))
        report["testResults"][1]["assertionResults"].append({"status": "skipped"})
        with self.assertRaisesRegex(RuntimeError, "did not pass every test"):
            accessibility.frontend_contract_results(report)

    def test_controlled_and_unit_proofs_reject_a_name_only_appearance(self) -> None:
        names = {name for names in controlled.PROOF_TESTS.values() for name in names}
        outputs = {"core": "\n".join(f"warning: mentions {name}" for name in names)}
        with self.assertRaisesRegex(RuntimeError, "no measured evidence"):
            controlled.proof_metrics(controlled.structured_results(outputs, {"testResults": []}))
        unit_names = {name for names in unit_contract.PROOF_TESTS.values() for name in names}
        passing = "\n".join(f"test m::{name} ... ok" for name in unit_names)
        proofs = unit_contract.proof_metrics(unit_contract.structured_results({"core": passing}))
        self.assertTrue(all(p["executed"] == p["expected"] for p in proofs.values()))
        ignored = passing.replace(
            "full_legal_dictation_chain_validates ... ok", "full_legal_dictation_chain_validates ... ignored")
        with self.assertRaisesRegex(RuntimeError, "full_legal_dictation_chain_validates"):
            unit_contract.proof_metrics(unit_contract.structured_results({"core": ignored}))


class UiDictationTimingTests(unittest.TestCase):
    """Defect 7: PERF-002 compared the wrong metric; negative latencies passed."""

    STAGES = {
        "capture_start": 1_000, "capture_ready": 21_000, "first_audio_accepted": 30_000,
        "capture_ended": 900_000, "raw_final_accepted": 1_200_000, "terminal_journal_committed": 1_250_000,
    }

    def test_monotonic_stages_give_non_negative_latencies(self) -> None:
        ready, latencies = ui.stage_latencies(dict(self.STAGES), 40.0)
        self.assertEqual(ready, 20.0)
        self.assertTrue(all(value >= 0 for value in latencies.values()))

    def test_non_monotonic_or_negative_stages_are_rejected(self) -> None:
        reordered = dict(self.STAGES, raw_final_accepted=800_000)
        with self.assertRaisesRegex(RuntimeError, "not monotonic"):
            ui.stage_latencies(reordered, 40.0)
        with self.assertRaisesRegex(RuntimeError, "negative"):
            ui.stage_latencies(dict(self.STAGES, capture_start=-5), 40.0)
        with self.assertRaisesRegex(RuntimeError, "negative or not finite"):
            ui.stage_latencies(dict(self.STAGES), -1.0)
        missing = dict(self.STAGES)
        del missing["capture_ready"]
        with self.assertRaisesRegex(RuntimeError, "omitted"):
            ui.stage_latencies(missing, 40.0)

    def test_perf_002_uses_hotkey_acknowledgement(self) -> None:
        base = {"warm_capture_ready_ms": 10.0, "visible_listening_feedback_ms": 60.0}
        slow_ack = ui.skipped_requirements_for({**base, "installed_toggle_ack_ms": 45.0})
        self.assertIn("PERF-002", slow_ack)
        self.assertNotIn("PERF-004", slow_ack)
        # Visible feedback above 30 ms must not fail PERF-002 when the ack is fast.
        fast_ack = ui.skipped_requirements_for({**base, "installed_toggle_ack_ms": 12.0})
        self.assertNotIn("PERF-002", fast_ack)


class UpgradePinTests(unittest.TestCase):
    """Defect 8 (recorder half): the previous package must be pinned by hash."""

    def test_unpinned_previous_package_fails_closed(self) -> None:
        argv = ["scripts/record_package_upgrade_evidence.py", *upgrade_recorder.EXPECTED_ARGV[2:]]
        with mock.patch.object(sys, "argv", argv), \
             mock.patch.object(upgrade_recorder.recorder_safety, "replaceable", return_value=True), \
             mock.patch.object(upgrade_recorder, "PREVIOUS_SHA256", None), \
             mock.patch("sys.stderr") as stderr:
            self.assertEqual(upgrade_recorder.main(), 1)
        written = "".join(call.args[0] for call in stderr.write.call_args_list)
        self.assertIn("not pinned", written)

    def test_daemon_steps_are_required(self) -> None:
        import qualify_candidate_upgrade as upgrade

        self.assertTrue(set(upgrade.DAEMON_CHECK_STEPS) <= upgrade_recorder.REQUIRED_STEPS)


class SupportReportTests(unittest.TestCase):
    """Defect 9: REL-008 never ran the packaged support report."""

    def test_support_report_is_measured(self) -> None:
        ok = known_phrase.CommandResult(0, b'{"report": "content-free"}\n', b"")
        self.assertEqual(hardening.support_report_metrics(ok),
                         {"exit_code": 0, "bytes": len(ok.stdout), "canaries_absent": True})
        leaked = known_phrase.CommandResult(0, hardening.CANARIES[1].encode() + b"\n", b"")
        self.assertFalse(hardening.support_report_metrics(leaked)["canaries_absent"])
        with self.assertRaisesRegex(RuntimeError, "support-report"):
            hardening.support_report_metrics(known_phrase.CommandResult(2, b"", b"refused"))

    def test_rel_008_binds_the_packaged_report(self) -> None:
        bindings = {a["id"]: a["measurement"] for a in hardening.RECORDS["REL-008"]["assertions"]}
        self.assertEqual(bindings["packaged-support-report-generated"]["metric"], "support_report.exit_code")
        self.assertEqual(bindings["support-report-default-redaction"]["metric"], "canaries.support_report_absent")


class PeerCredentialRejectionTests(unittest.TestCase):
    """Defect 10: the SO_PEERCRED rejection of another UID was never tested."""

    def serve(self, frame: bytes) -> tuple[str, threading.Thread]:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        path = os.path.join(directory.name, "s.sock")
        server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.addCleanup(server.close)
        server.bind(path)
        server.listen(1)

        def answer() -> None:
            connection, _ = server.accept()
            with connection:
                connection.sendall(struct.pack("<I", len(frame)) + frame)

        thread = threading.Thread(target=answer, daemon=True)
        thread.start()
        return path, thread

    def run_probe(self, frame: bytes) -> dict[str, object]:
        path, thread = self.serve(frame)
        result = subprocess.run([sys.executable, "-I", "-c", hardening.CROSS_UID_PROBE, path],
                                capture_output=True, text=True, timeout=10)
        thread.join(5)
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)

    def test_probe_recognises_the_daemon_rejection_frame(self) -> None:
        rejected = json.dumps({"kind": "rejected", "detail": "cross-user connection denied: uid 65534"}).encode()
        self.assertTrue(self.run_probe(rejected)["rejected"])
        self.assertFalse(self.run_probe(b'{"kind": "hello"}')["rejected"])

    def test_cross_uid_probe_requires_rejection_and_restores_modes(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            holder = Path(temporary) / "holder"
            runtime = holder / "runtime"
            runtime.mkdir(parents=True)
            os.chmod(holder, 0o700)
            os.chmod(runtime, 0o700)
            sock_path = runtime / "narrivyd.sock"
            server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            self.addCleanup(server.close)
            server.bind(str(sock_path))
            os.chmod(sock_path, 0o600)
            seen = []

            class Ops:
                def __init__(self, stdout: bytes, code: int = 0) -> None:
                    self.stdout, self.code = stdout, code

                def run(self, argv, *, env, timeout, cwd=None):
                    seen.append((list(argv), oct(os.stat(runtime).st_mode & 0o777)))
                    return known_phrase.CommandResult(self.code, self.stdout, b"")

            with mock.patch("pwd.getpwnam", return_value=mock.Mock(pw_uid=os.geteuid() + 1)):
                measured = hardening.probe_cross_uid(holder, runtime, sock_path, Ops(b'{"rejected": true}'), {})
                self.assertTrue(measured["cross_uid_rejected"])
                self.assertEqual(seen[0][0][:5], ["sudo", "-n", "-u", "nobody", "python3"])
                self.assertEqual(seen[0][1], "0o711")
                self.assertEqual(os.stat(runtime).st_mode & 0o777, 0o700)
                self.assertEqual(os.stat(sock_path).st_mode & 0o777, 0o600)
                with self.assertRaisesRegex(RuntimeError, "did not reject"):
                    hardening.probe_cross_uid(holder, runtime, sock_path, Ops(b'{"rejected": false}'), {})
                with self.assertRaisesRegex(RuntimeError, "sudo"):
                    hardening.probe_cross_uid(holder, runtime, sock_path, Ops(b"", code=1), {})
            self.assertEqual(os.stat(holder).st_mode & 0o777, 0o700)

    def test_sec_005_requires_the_rejection(self) -> None:
        bindings = {a["id"]: a["measurement"] for a in hardening.RECORDS["SEC-005"]["assertions"]}
        self.assertEqual(bindings["cross-uid-peer-rejected"],
                         {"metric": "endpoint.cross_uid_rejected", "comparator": "is_true"})


class VerticalSliceScenarioTests(unittest.TestCase):
    """Defect 11: the vertical slice used the wrong scenario ID and hash."""

    def test_scenario_is_p6_e2e_beta_and_hash_is_the_spec_file(self) -> None:
        self.assertEqual(vertical_slice.SCENARIO_ID, "p6-e2e-beta")
        scenario = json.loads((ROOT / vertical_slice.SCENARIO).read_text())
        self.assertEqual(scenario["verification_id"], "VER-TEST-009")
        expected_hash, error = phase_gate.scenario_hash(vertical_slice.SCENARIO_ID)
        self.assertIsNone(error)
        self.assertEqual(expected_hash, sha((ROOT / vertical_slice.SCENARIO).read_bytes()))
        source = Path(vertical_slice.__file__).read_text()
        self.assertIn('"scenario_hash": sha256(scenario_path)', source)
        self.assertNotIn("cosmic-physical-mic-local-asr-clean-inbox-v1", source)


# Defect 12 (OPS-004 NOASSERTION and unverified checksums) is covered in
# tests/packaging/test_record_supply_chain_evidence.py; defect 8's daemon
# start/status checks in tests/packaging/test_qualify_candidate_upgrade.py.


class RunBoundedTests(unittest.TestCase):
    """T4.5 item 12: a timed-out helper's whole process group is stopped."""

    def test_timeout_kills_the_group(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            marker = Path(temporary) / "child.pid"
            script = f"sleep 30 & echo $! > {marker}; wait"
            cancelled = []
            with self.assertRaises(subprocess.TimeoutExpired):
                recorder_safety.run_bounded(["/bin/sh", "-c", script], timeout=0.5,
                                            on_timeout=lambda: cancelled.append(True))
            self.assertEqual(cancelled, [True])
            child = int(marker.read_text())
            try:
                os.kill(child, 0)
                alive = Path(f"/proc/{child}").exists() and "Z" not in Path(f"/proc/{child}/stat").read_text().split()[2]
            except ProcessLookupError:
                alive = False
            self.assertFalse(alive)


if __name__ == "__main__":
    unittest.main()
