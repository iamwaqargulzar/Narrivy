"""Focused, audio-free tests for the native known-phrase runner."""
from __future__ import annotations

import hashlib
import importlib.util
import io
import json
import os
import stat
import sys
import tarfile
import tempfile
import unittest
import wave
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
SPEC = importlib.util.spec_from_file_location(
    "known_phrase", ROOT / "scripts/qualify_candidate_known_phrase.py"
)
assert SPEC and SPEC.loader
kp = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = kp
SPEC.loader.exec_module(kp)

HEAD = "a" * 40


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def ar_member(name: str, data: bytes) -> bytes:
    header = (
        f"{name + '/':<16}{0:<12}{0:<6}{0:<6}{'100644':<8}{len(data):<10}`\n"
    ).encode("ascii")
    return header + data + (b"\n" if len(data) % 2 else b"")


def candidate(head: str = HEAD) -> bytes:
    payloads = {
        kp.DAEMON_MEMBER: b"daemon",
        kp.CLI_MEMBER: b"cli",
        kp.WORKER_MEMBER: b"worker",
        kp.MODEL_MEMBER: b"model",
    }
    keyed = {
        "narrivyd": payloads[kp.DAEMON_MEMBER],
        "narrivy-cli": payloads[kp.CLI_MEMBER],
        "narrivy-desktop": b"desktop",
        "resident-worker": payloads[kp.WORKER_MEMBER],
        "bundled-model": payloads[kp.MODEL_MEMBER],
    }
    info = {
        "schema_version": 2,
        "classification": kp.CLEAN_CLASSIFICATION,
        "git_dirty": False,
        "package": "narrivy",
        "version": "0.1.0~test",
        "source_head": head,
        "source_date_epoch": 1,
        "inputs": {
            key: {"sha256": digest(value), "size_bytes": len(value)}
            for key, value in keyed.items()
        },
    }
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w:xz") as archive:
        for name, data in {
            kp.BUILD_INFO_MEMBER: json.dumps(info).encode(),
            **payloads,
        }.items():
            item = tarfile.TarInfo(name)
            item.size = len(data)
            item.mode = 0o755 if name != kp.MODEL_MEMBER else 0o644
            archive.addfile(item, io.BytesIO(data))
    control = io.BytesIO()
    with tarfile.open(fileobj=control, mode="w:xz"):
        pass
    return (
        kp.AR_MAGIC
        + ar_member("debian-binary", b"2.0\n")
        + ar_member("control.tar.xz", control.getvalue())
        + ar_member("data.tar.xz", buffer.getvalue())
    )


def wav() -> bytes:
    out = io.BytesIO()
    with wave.open(out, "wb") as audio:
        audio.setnchannels(1)
        audio.setsampwidth(2)
        audio.setframerate(16_000)
        audio.writeframes(b"\0\0" * 16_000)
    return out.getvalue()


class FakeSpawn:
    def __init__(self, terminate: bool = True, code: int = 0, logs: bytes = b"clean"):
        self.code = None
        self.terminate = terminate
        self.final_code = code
        self.logs = logs

    def poll(self):
        return self.code

    def kill_group(self):
        if self.terminate:
            self.code = self.final_code
        return self.terminate

    def output(self):
        return self.logs


class FakeOps:
    def __init__(self, root: Path, *, spawn: FakeSpawn | None = None):
        self.root = root
        self.spawned = spawn or FakeSpawn()
        self.calls: list[list[str]] = []
        self.now = 0.0

    def environ(self):
        return {
            "XDG_CURRENT_DESKTOP": "COSMIC",
            "XDG_SESSION_TYPE": "wayland",
            "WAYLAND_DISPLAY": "wayland-1",
            "XDG_RUNTIME_DIR": "/run/user/1000",
        }

    def containerized(self):
        return False

    def native_lane(self):
        return {"desktop": "COSMIC", "session": "wayland"}

    def helper_ready(self, _path):
        return True

    def monotonic(self):
        return self.now

    def sleep(self, seconds):
        self.now += seconds

    def spawn(self, _argv, *, env, log_path, cwd):
        self.calls.append(["spawn"])
        return self.spawned

    def run(self, argv, *, env, timeout, cwd=None):
        args = [str(value) for value in argv]
        self.calls.append(args)
        if args[0] == str(kp.GIT):
            if "--show-toplevel" in args:
                return kp.CommandResult(0, (str(self.root) + "\n").encode(), b"")
            if "--verify" in args:
                return kp.CommandResult(0, (HEAD + "\n").encode(), b"")
            return kp.CommandResult(0, b"", b"")
        if args[0] == str(kp.PACTL):
            return kp.CommandResult(0, b"alsa_input.usb-test.analog-stereo\n", b"")
        if args[0] == str(kp.PAPLAY):
            return kp.CommandResult(0, b"", b"")
        if "status" in args:
            socket_path = Path(args[args.index("--socket") + 1])
            inbox = socket_path.parent / "inbox"
            inbox.mkdir(exist_ok=True)
            (inbox / "tx.txt").write_text("ask not what your country")
            return kp.CommandResult(0, b"narrivy-cli status: no active session\n", b"")
        return kp.CommandResult(0, b"accepted\n", b"")


class RunnerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name).resolve()
        self.package = self.root / "candidate.deb"
        self.fixture = self.root / "fixture.wav"
        self.result = self.root / "result.json"
        self.package.write_bytes(candidate())
        self.fixture.write_bytes(wav())

    def tearDown(self):
        self.temp.cleanup()

    def inputs(self, **changes):
        values = dict(
            candidate=self.package,
            fixture=self.fixture,
            result=self.result,
            expected_candidate_sha256=digest(self.package.read_bytes()),
            expected_fixture_sha256=digest(self.fixture.read_bytes()),
            confirm_audible_playback=True,
            source_root=self.root,
        )
        values.update(changes)
        return kp.Inputs(**values)

    def run_success(self, ops: FakeOps | None = None):
        operations = ops or FakeOps(self.root)
        with mock.patch.object(kp, "_wait_for_socket"):
            result = kp.qualify(self.inputs(), operations=operations)
        return result, operations

    def test_full_fake_success_is_private_atomic_and_content_free(self):
        result, ops = self.run_success()
        self.assertEqual(result["result"], "passed")
        self.assertTrue(result["observations"]["fragment_matched"])
        self.assertNotIn(str(self.root), self.result.read_text())
        self.assertEqual(stat.S_IMODE(self.result.stat().st_mode), 0o600)
        self.assertEqual(ops.spawned.poll(), 0)

    def test_confirmation_fails_before_operations(self):
        ops = FakeOps(self.root)
        with self.assertRaisesRegex(kp.InputError, "confirmed"):
            kp.qualify(self.inputs(confirm_audible_playback=False), operations=ops)
        self.assertEqual(ops.calls, [])

    def test_requires_deb_suffix_and_new_output(self):
        wrong = self.root / "candidate.bin"
        wrong.write_bytes(self.package.read_bytes())
        with self.assertRaisesRegex(kp.InputError, "end in .deb"):
            kp.qualify(self.inputs(candidate=wrong), operations=FakeOps(self.root))
        self.result.write_text("existing")
        with self.assertRaisesRegex(kp.InputError, "already exists"):
            kp.qualify(self.inputs(), operations=FakeOps(self.root))

    def test_hash_and_source_commit_mismatch_fail_closed(self):
        with self.assertRaisesRegex(kp.InputError, "SHA-256"):
            kp.qualify(self.inputs(expected_candidate_sha256="0" * 64), operations=FakeOps(self.root))
        self.package.write_bytes(candidate("b" * 40))
        with self.assertRaisesRegex(kp.InputError, "source_head"):
            kp.qualify(self.inputs(expected_candidate_sha256=digest(self.package.read_bytes())), operations=FakeOps(self.root))

    def test_wav_contract_rejects_malformed_input(self):
        for payload in (b"not-wav", wav()[:20]):
            with self.subTest(size=len(payload)):
                with self.assertRaises(kp.InputError):
                    kp._parse_wav_duration(payload)

    def test_physical_source_rejects_monitor_and_virtual_names(self):
        for name in (b"sink.monitor\n", b"narrivy_virtual_source\n"):
            ops = FakeOps(self.root)
            original = ops.run
            ops.run = lambda argv, **kw: kp.CommandResult(0, name, b"") if str(argv[0]) == str(kp.PACTL) else original(argv, **kw)
            with self.subTest(name=name), self.assertRaisesRegex(kp.InputError, "physical"):
                kp._require_physical_source(ops, 1)

    def test_fragment_and_log_leak_checks(self):
        transcript = self.root / "text.txt"
        transcript.write_text("different words")
        with self.assertRaisesRegex(kp.StepFailure, "expected fragment"):
            kp._read_transcript(transcript)
        scanned, hits = kp._scan_logs([b"ASK NOT WHAT YOUR COUNTRY"])
        self.assertGreater(scanned, 0)
        self.assertEqual(hits, 1)

    def test_shutdown_failure_prevents_success(self):
        result, _ = self.run_success(FakeOps(self.root, spawn=FakeSpawn(terminate=False)))
        self.assertEqual(result["result"], "failed")
        self.assertEqual(result["failed_step"], "shutdown")

    def test_host_shutdown_signals_daemon_before_process_group(self):
        process = mock.Mock()
        process.poll.side_effect = [None, 0]
        process.wait.return_value = 0
        self.assertTrue(kp._kill_process_group(process, graceful_root_seconds=1.0))
        process.terminate.assert_called_once_with()
        process.wait.assert_called_once_with(timeout=1.0)

    def test_nonzero_daemon_exit_prevents_success(self):
        result, _ = self.run_success(FakeOps(self.root, spawn=FakeSpawn(code=1)))
        self.assertEqual(result["failed_step"], "shutdown")

    def test_late_daemon_log_leak_is_detected(self):
        result, _ = self.run_success(FakeOps(self.root, spawn=FakeSpawn(logs=kp.EXPECTED_FRAGMENT.encode())))
        self.assertEqual(result["failed_step"], "log_hygiene")

    def test_cleanup_uncertainty_prevents_pass(self):
        with mock.patch.object(kp, "_wait_for_socket"), mock.patch.object(kp, "_remove_tree", return_value=False):
            result = kp.qualify(self.inputs(), operations=FakeOps(self.root))
        self.assertEqual(result["result"], "failed")
        self.assertEqual(result["failed_step"], "cleanup")


if __name__ == "__main__":
    unittest.main()
