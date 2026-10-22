"""Focused tests for scripts/run_deepseek_task.py.

Every test drives a fake ``dsh`` executable generated into a temporary
directory: no network access, no real harness profile, no credentials, and no
writes outside the test's temp root. The tests assert the orchestration
contract only: task validation, argv shape, permission pinning, bounded
capture, timeout/overflow termination, atomic JSON, redaction, and locking.
"""

from __future__ import annotations

import hashlib
import io
import json
import os
import stat
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
SCRIPTS = ROOT / "scripts"
sys.path.insert(0, str(SCRIPTS))

import run_deepseek_task as orchestrator  # noqa: E402

SCRIPT_PATH = SCRIPTS / "run_deepseek_task.py"
OVERLAY = ROOT / "agent" / "deepseek-v4.1-flash.patch.yml"

FAKE_DSH_SOURCE = '''#!/usr/bin/env python3
"""Fake dsh launcher for orchestrator tests; never touches the network."""

from __future__ import annotations

import json
import os
import sys
import time


def main() -> int:
    config_path = os.environ.get("FAKE_DSH_CONFIG")
    config = json.loads(open(config_path, "r", encoding="utf-8").read()) if config_path else {}

    if config.get("record"):
        with open(config["record"], "w", encoding="utf-8") as handle:
            json.dump(
                {
                    "argv": sys.argv,
                    "cwd": os.getcwd(),
                    "permission_mode": os.environ.get("DSH_PERMISSION_MODE"),
                    "api_key_present": "OPENCODE_GO_API_KEY" in os.environ,
                },
                handle,
            )

    stream_bytes = int(config.get("stream_bytes", 0))
    if stream_bytes:
        target = config.get("stream_target", "stdout")
        stream = sys.stdout.buffer if target == "stdout" else sys.stderr.buffer
        chunk = b"y" * 4096
        written = 0
        while written < stream_bytes:
            stream.write(chunk)
            stream.flush()
            written += len(chunk)
    else:
        sys.stdout.write(config.get("stdout", ""))
        sys.stdout.flush()
        sys.stderr.write(config.get("stderr", ""))
        sys.stderr.flush()

    if config.get("started_marker"):
        with open(config["started_marker"], "w", encoding="utf-8") as handle:
            handle.write("started")

    delay = float(config.get("sleep_seconds", 0))
    if delay:
        time.sleep(delay)

    if config.get("finished_marker"):
        with open(config["finished_marker"], "w", encoding="utf-8") as handle:
            handle.write("finished")

    return int(config.get("exit_code", 0))


if __name__ == "__main__":
    raise SystemExit(main())
'''


class OrchestratorTestCase(unittest.TestCase):
    """Shared fixture: temp root, fake dsh, config/result helpers."""

    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.fake_dsh = self._write_fake_dsh()
        self.record = self.root / "record.json"

    def _write_fake_dsh(self) -> Path:
        path = self.root / "fake-dsh"
        source = FAKE_DSH_SOURCE.replace(
            "#!/usr/bin/env python3", f"#!{sys.executable}", 1
        )
        path.write_text(source, encoding="utf-8")
        path.chmod(path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
        return path

    def _write_config(self, name: str, **values: object) -> Path:
        path = self.root / name
        path.write_text(json.dumps(values), encoding="utf-8")
        return path

    def _config(self, **values: object) -> Path:
        return self._write_config("fake-config.json", **values)

    def _task(self, text: str = "do the work\n") -> Path:
        path = self.root / "task.md"
        path.write_text(text, encoding="utf-8")
        return path

    def _base_env(self, config_path: Path, extra: dict[str, str] | None = None) -> dict[str, str]:
        env = {
            "PATH": os.environ.get("PATH", ""),
            "HOME": str(self.root),
            "FAKE_DSH_CONFIG": str(config_path),
        }
        env.update(extra or {})
        return env

    def _run(
        self,
        task_text: str = "do the work\n",
        *,
        config: dict[str, object] | None = None,
        env_extra: dict[str, str] | None = None,
        task_file: Path | None = None,
        task_bytes: bytes | None = None,
        overlay: Path | None = None,
        dsh: Path | str | None = None,
        result_file: Path | None = None,
        lock_file: Path | None = None,
        expected_model: str | None = None,
        expected_provider: str | None = None,
        dry_run: bool = False,
        **overrides: object,
    ) -> tuple[int, str, str, orchestrator.RunRequest]:
        if task_file is None:
            task_file = self.root / "task.md"
            if task_bytes is not None:
                task_file.write_bytes(task_bytes)
            else:
                task_file.write_text(task_text, encoding="utf-8")
        config_path = self._config(**(config or {}))
        request = orchestrator.RunRequest(
            task_file=task_file,
            result_file=result_file or (self.root / "out" / "result.json"),
            workspace=overrides.pop("workspace", ROOT),
            overlay=overlay or OVERLAY,
            dsh=str(dsh if dsh is not None else self.fake_dsh),
            lock_file=lock_file or (self.root / "locks" / "orchestrator.lock"),
            timeout_seconds=overrides.pop("timeout_seconds", 15.0),
            kill_grace_seconds=overrides.pop("kill_grace_seconds", 1.0),
            max_task_bytes=overrides.pop("max_task_bytes", orchestrator.DEFAULT_MAX_TASK_BYTES),
            max_output_bytes=overrides.pop("max_output_bytes", 1 << 20),
            on_output_overflow=overrides.pop(
                "on_output_overflow", orchestrator.OVERFLOW_TRUNCATE
            ),
            lock_wait_seconds=overrides.pop("lock_wait_seconds", 0.0),
            expected_model=(
                orchestrator.DEFAULT_EXPECTED_MODEL
                if expected_model is None
                else expected_model
            ),
            expected_provider=(
                orchestrator.DEFAULT_EXPECTED_PROVIDER
                if expected_provider is None
                else expected_provider
            ),
            dry_run=dry_run,
        )
        self.assertEqual(overrides, {}, f"unused overrides: {sorted(overrides)}")
        out, err = io.StringIO(), io.StringIO()
        code = orchestrator.run_request(
            request, env=self._base_env(config_path, env_extra), out=out, err=err
        )
        return code, out.getvalue(), err.getvalue(), request

    def _read_result(self, request: orchestrator.RunRequest) -> dict[str, object]:
        return json.loads(request.result_file.read_text(encoding="utf-8"))

    def _recorded(self, path: Path) -> dict[str, object]:
        return json.loads(path.read_text(encoding="utf-8"))


class TaskValidationTests(OrchestratorTestCase):
    def test_invalid_task_files_are_rejected_without_starting_the_worker(self) -> None:
        cases: dict[str, tuple[Path, bytes | None]] = {
            "missing": (self.root / "missing.md", None),
            "blank": (self.root / "blank.md", b"   \n\t\n"),
            "nul": (self.root / "nul.md", b"hello\x00world"),
            "not-utf8": (self.root / "bad.md", b"\xff\xfetask"),
            "oversize": (self.root / "big.md", b"x" * 64),
        }
        for name, (task_file, payload) in cases.items():
            with self.subTest(case=name):
                if payload is not None:
                    task_file.write_bytes(payload)
                result_file = self.root / f"out-{name}.json"
                code, _, _, request = self._run(
                    config={"record": str(self.record), "stdout": "should not run\n"},
                    task_file=task_file,
                    result_file=result_file,
                    max_task_bytes=(
                        8 if name == "oversize" else orchestrator.DEFAULT_MAX_TASK_BYTES
                    ),
                )
                self.assertEqual(code, 2)
                result = self._read_result(request)
                self.assertFalse(result["ok"])
                self.assertEqual(result["outcome"], "invalid_task_file")
                self.assertIsNotNone(result["error"])
                self.assertFalse(self.record.exists(), "worker must not start")

    def test_task_metadata_is_recorded(self) -> None:
        code, _, _, request = self._run(config={"stdout": "ok\n"})
        self.assertEqual(code, 0)
        result = self._read_result(request)
        payload = "do the work\n".encode("utf-8")
        self.assertEqual(result["task"]["bytes"], len(payload))
        self.assertEqual(result["task"]["sha256"], hashlib.sha256(payload).hexdigest())
        self.assertEqual(result["task"]["file"], str(self.root / "task.md"))


class WorkerInvocationTests(OrchestratorTestCase):
    def test_worker_receives_pinned_profile_overlay_and_workspace(self) -> None:
        code, stdout, stderr, request = self._run(
            task_text="---\nnot an option\n",
            config={"record": str(self.record), "stdout": "done\n"},
        )
        self.assertEqual(code, 0, stderr)
        recorded = self._recorded(self.record)
        self.assertEqual(
            recorded["argv"],
            [
                str(self.fake_dsh),
                "--profile",
                "headless",
                "--patch",
                str(OVERLAY),
                "--",
                "---\nnot an option\n",
            ],
        )
        self.assertEqual(recorded["cwd"], str(ROOT))
        self.assertEqual(recorded["permission_mode"], "workspace-write")
        # Only the result path may reach the orchestrator's stdout.
        self.assertEqual(stdout.strip(), str(request.result_file))

    def test_ambient_permission_mode_cannot_widen_the_fence(self) -> None:
        code, _, _, request = self._run(
            config={"record": str(self.record), "stdout": "ok\n"},
            env_extra={"DSH_PERMISSION_MODE": "danger-full-access"},
        )
        self.assertEqual(code, 0)
        self.assertEqual(self._recorded(self.record)["permission_mode"], "workspace-write")
        result = self._read_result(request)
        self.assertEqual(result["dsh"]["permission_mode"], "workspace-write")
        self.assertEqual(result["dsh"]["profile"], "headless")

    def test_overlay_is_validated_before_any_worker_starts(self) -> None:
        cases = {
            "missing": self.root / "missing-overlay.yml",
            "wrong-model": self._write_config(
                "wrong-overlay.yml", provider="opencode-go", model="other-model"
            ),
        }
        for name, overlay in cases.items():
            with self.subTest(case=name):
                code, _, _, request = self._run(
                    config={"record": str(self.record), "stdout": "should not run\n"},
                    overlay=overlay,
                    result_file=self.root / f"overlay-{name}.json",
                )
                self.assertEqual(code, 2)
                result = self._read_result(request)
                self.assertEqual(result["outcome"], "invalid_overlay")
                self.assertIsNone(result["overlay"]["sha256"])
                self.assertFalse(self.record.exists())

    def test_missing_dsh_executable_fails_before_spawn(self) -> None:
        code, _, _, request = self._run(dsh=self.root / "no-such-dsh")
        self.assertEqual(code, 3)
        result = self._read_result(request)
        self.assertEqual(result["outcome"], "dsh_not_found")
        self.assertIsNone(result["dsh"]["path"])
        self.assertEqual(result["dsh"]["requested"], str(self.root / "no-such-dsh"))


class ResultContractTests(OrchestratorTestCase):
    def test_success_captures_response_and_diagnostics_separately(self) -> None:
        code, stdout, stderr, request = self._run(
            config={
                "stdout": "final answer\n",
                "stderr": "dsh: reasoning: step 1\n",
                "exit_code": 0,
            }
        )
        self.assertEqual(code, 0)
        self.assertEqual(stderr, "")
        self.assertEqual(stdout.strip(), str(request.result_file))
        result = self._read_result(request)
        self.assertTrue(result["ok"])
        self.assertEqual(result["outcome"], "completed")
        self.assertEqual(result["exit_code"], 0)
        self.assertEqual(result["response"], "final answer\n")
        self.assertEqual(result["diagnostics"], "dsh: reasoning: step 1\n")
        self.assertIsNone(result["error"])
        self.assertTrue(result["stream_drain_complete"])
        self.assertEqual(
            result["overlay"]["sha256"], hashlib.sha256(OVERLAY.read_bytes()).hexdigest()
        )
        self.assertEqual(
            stat.S_IMODE(request.result_file.stat().st_mode), 0o600
        )
        # The task text is never echoed into the machine-readable record.
        self.assertNotIn("do the work", json.dumps(result["dsh"]["argv"]))

    def test_child_failure_is_propagated_truthfully(self) -> None:
        code, _, _, request = self._run(
            config={"exit_code": 1, "stderr": "dsh: aborted: tests failed\n"}
        )
        self.assertEqual(code, 1)
        result = self._read_result(request)
        self.assertFalse(result["ok"])
        self.assertEqual(result["outcome"], "child_failed")
        self.assertEqual(result["exit_code"], 1)
        self.assertIn("aborted", result["error"])

    def test_result_is_written_atomically_without_leftovers(self) -> None:
        code, _, _, request = self._run(config={"stdout": "ok\n"})
        self.assertEqual(code, 0)
        parent = request.result_file.parent
        leftovers = list(parent.glob("*.tmp")) + list(parent.glob(".*.tmp"))
        self.assertEqual(leftovers, [])
        self.assertTrue(request.result_file.is_file())

    def test_result_write_failure_is_reported(self) -> None:
        blocker = self.root / "blocked"
        blocker.write_text("not a directory", encoding="utf-8")
        result_file = blocker / "result.json"
        code, _, stderr, _ = self._run(
            config={"stdout": "ok\n"}, result_file=result_file
        )
        self.assertEqual(code, 7)
        self.assertIn("was not written", stderr)
        self.assertFalse(result_file.exists())

    def test_timeout_terminates_the_worker(self) -> None:
        marker = self.root / "finished"
        code, _, _, request = self._run(
            config={
                "stdout": "partial\n",
                "stderr": "dsh: reasoning: slow\n",
                "sleep_seconds": 5.0,
                "finished_marker": str(marker),
            },
            timeout_seconds=0.5,
            kill_grace_seconds=0.5,
        )
        self.assertEqual(code, 4)
        result = self._read_result(request)
        self.assertFalse(result["ok"])
        self.assertEqual(result["outcome"], "timed_out")
        self.assertTrue(result["timed_out"])
        self.assertEqual(result["response"], "partial\n")
        self.assertFalse(marker.exists(), "killed worker must not finish")
        self.assertLess(result["duration_seconds"], 4.0)

    def test_output_over_cap_is_truncated_and_flagged(self) -> None:
        code, _, _, request = self._run(
            config={"stream_bytes": 256 * 1024, "stream_target": "stdout"},
            max_output_bytes=4096,
        )
        self.assertEqual(code, 0)
        result = self._read_result(request)
        self.assertTrue(result["ok"])
        self.assertEqual(len(result["response"]), 4096)
        self.assertTrue(result["output_overflow"]["stdout_truncated"])
        self.assertGreaterEqual(result["output_overflow"]["stdout_bytes"], 256 * 1024)
        self.assertFalse(result["output_overflow"]["stderr_truncated"])

    def test_stderr_over_cap_is_truncated_and_flagged(self) -> None:
        code, _, _, request = self._run(
            config={"stream_bytes": 256 * 1024, "stream_target": "stderr"},
            max_output_bytes=2048,
        )
        self.assertEqual(code, 0)
        result = self._read_result(request)
        self.assertEqual(len(result["diagnostics"]), 2048)
        self.assertTrue(result["output_overflow"]["stderr_truncated"])
        self.assertFalse(result["output_overflow"]["stdout_truncated"])

    def test_output_overflow_aborts_when_requested(self) -> None:
        marker = self.root / "finished"
        code, _, _, request = self._run(
            config={
                "stream_bytes": 64 * 1024 * 1024,
                "stream_target": "stdout",
                "finished_marker": str(marker),
            },
            max_output_bytes=4096,
            on_output_overflow=orchestrator.OVERFLOW_ABORT,
            timeout_seconds=20.0,
            kill_grace_seconds=1.0,
        )
        self.assertEqual(code, 5)
        result = self._read_result(request)
        self.assertFalse(result["ok"])
        self.assertEqual(result["outcome"], "output_overflow")
        self.assertTrue(result["output_overflow"]["stdout_truncated"])
        self.assertEqual(len(result["response"]), 4096)
        self.assertFalse(marker.exists(), "aborted worker must not finish")

    def test_credentials_are_never_written_to_the_result(self) -> None:
        secret = "sk-live-0123456789abcdef"
        code, _, _, request = self._run(
            config={
                "record": str(self.record),
                "stdout": f"using {secret}\n",
                "stderr": f"env has {secret}\n",
            },
            env_extra={"OPENCODE_GO_API_KEY": secret},
        )
        self.assertEqual(code, 0)
        raw = request.result_file.read_text(encoding="utf-8")
        self.assertNotIn(secret, raw)
        self.assertIn("[REDACTED:OPENCODE_GO_API_KEY]", raw)
        result = json.loads(raw)
        self.assertEqual(result["response"], "using [REDACTED:OPENCODE_GO_API_KEY]\n")
        self.assertEqual(result["diagnostics"], "env has [REDACTED:OPENCODE_GO_API_KEY]\n")
        self.assertNotIn("env", result)
        # The worker still inherits the credential it needs; only the
        # orchestrator's outputs never carry it.
        self.assertTrue(self._recorded(self.record)["api_key_present"])

    def test_credentials_are_redacted_from_failure_diagnostics(self) -> None:
        secret = "sk-live-0123456789abcdef"
        code, _, _, request = self._run(
            config={"exit_code": 1, "stderr": f"dsh: auth failed for {secret}\n"},
            env_extra={"OPENCODE_GO_API_KEY": secret},
        )
        self.assertEqual(code, 1)
        result = self._read_result(request)
        self.assertNotIn(secret, result["error"])
        self.assertIn("[REDACTED:OPENCODE_GO_API_KEY]", result["error"])


class DryRunTests(OrchestratorTestCase):
    def test_dry_run_prints_the_plan_without_running_or_writing(self) -> None:
        code, stdout, _, request = self._run(
            dry_run=True, config={"record": str(self.record)}
        )
        self.assertEqual(code, 0)
        plan = json.loads(stdout)
        self.assertTrue(plan["dry_run"])
        self.assertEqual(
            plan["dsh"]["argv"][1:5],
            ["--profile", "headless", "--patch", str(OVERLAY)],
        )
        self.assertEqual(plan["dsh"]["argv"][5], "--")
        self.assertIn("<task:", plan["dsh"]["argv"][6])
        self.assertEqual(plan["permission_mode"], "workspace-write")
        self.assertFalse(request.result_file.exists())
        self.assertFalse(self.record.exists())


class ConcurrencyTests(OrchestratorTestCase):
    def test_second_run_fails_closed_while_the_lock_is_held(self) -> None:
        lock_path = self.root / "locks" / "orchestrator.lock"
        held = orchestrator._RunLock(lock_path)
        self.assertTrue(held.acquire())
        self.addCleanup(held.release)
        code, stdout, _, request = self._run(
            config={"record": str(self.record), "stdout": "should not run\n"},
            lock_file=lock_path,
        )
        self.assertEqual(code, 6)
        result = self._read_result(request)
        self.assertFalse(result["ok"])
        self.assertEqual(result["outcome"], "lock_held")
        self.assertIn("already holds", result["error"])
        self.assertFalse(self.record.exists(), "locked-out run must not start")
        self.assertEqual(stdout.strip(), str(request.result_file))

    def test_run_lock_waits_when_asked(self) -> None:
        lock_path = self.root / "locks" / "orchestrator.lock"
        held = orchestrator._RunLock(lock_path)
        self.assertTrue(held.acquire())
        timer = threading.Timer(0.4, held.release)
        timer.start()
        self.addCleanup(timer.cancel)
        self.addCleanup(held.release)
        code, _, stderr, _ = self._run(
            config={"stdout": "ok\n"}, lock_file=lock_path, lock_wait_seconds=5.0
        )
        self.assertEqual(code, 0, stderr)

    def test_concurrent_processes_do_not_collide(self) -> None:
        lock_path = self.root / "locks" / "orchestrator.lock"
        started = self.root / "started"
        task = self._task("long task\n")
        first_config = self._write_config(
            "first.json",
            record=str(self.record),
            started_marker=str(started),
            sleep_seconds=3.0,
            stdout="first\n",
        )
        second_config = self._write_config(
            "second.json",
            record=str(self.root / "second-record.json"),
            stdout="second\n",
        )
        first_result = self.root / "result-first.json"
        second_result = self.root / "result-second.json"
        base = [sys.executable, str(SCRIPT_PATH)]
        common = [
            "--task-file",
            str(task),
            "--dsh",
            str(self.fake_dsh),
            "--lock-file",
            str(lock_path),
            "--workspace",
            str(ROOT),
            "--timeout-seconds",
            "30",
        ]
        first = subprocess.Popen(
            base + common + ["--result-file", str(first_result)],
            env={**os.environ, "FAKE_DSH_CONFIG": str(first_config)},
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        self.addCleanup(first.kill)
        deadline = time.monotonic() + 10.0
        while time.monotonic() < deadline and not started.exists():
            time.sleep(0.05)
        self.assertTrue(started.exists(), "first worker never started")

        second = subprocess.run(
            base + common + ["--result-file", str(second_result)],
            env={**os.environ, "FAKE_DSH_CONFIG": str(second_config)},
            capture_output=True,
            text=True,
            timeout=30,
        )
        self.assertEqual(second.returncode, 6, second.stderr)
        second_payload = json.loads(second_result.read_text(encoding="utf-8"))
        self.assertEqual(second_payload["outcome"], "lock_held")
        self.assertFalse((self.root / "second-record.json").exists())

        _, first_stderr = first.communicate(timeout=30)
        self.assertEqual(first.returncode, 0, first_stderr)
        first_payload = json.loads(first_result.read_text(encoding="utf-8"))
        self.assertEqual(first_payload["outcome"], "completed")


class InterruptionTests(OrchestratorTestCase):
    def test_sigterm_reaps_the_worker_group_and_records_the_run(self) -> None:
        started = self.root / "started"
        marker = self.root / "finished"
        result_file = self.root / "sigterm" / "result.json"
        config = self._write_config(
            "sigterm.json",
            started_marker=str(started),
            finished_marker=str(marker),
            sleep_seconds=30.0,
            stdout="partial\n",
        )
        process = subprocess.Popen(
            [
                sys.executable,
                str(SCRIPT_PATH),
                "--task-file",
                str(self._task()),
                "--result-file",
                str(result_file),
                "--dsh",
                str(self.fake_dsh),
                "--lock-file",
                str(self.root / "sigterm" / "run.lock"),
                "--workspace",
                str(ROOT),
                "--timeout-seconds",
                "60",
            ],
            env={**os.environ, "FAKE_DSH_CONFIG": str(config)},
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        self.addCleanup(process.kill)
        deadline = time.monotonic() + 10.0
        while time.monotonic() < deadline and not started.exists():
            time.sleep(0.05)
        self.assertTrue(started.exists(), "worker never started")

        process.terminate()
        _, stderr = process.communicate(timeout=20)
        self.assertEqual(process.returncode, 130, stderr)
        payload = json.loads(result_file.read_text(encoding="utf-8"))
        self.assertFalse(payload["ok"])
        self.assertEqual(payload["outcome"], "interrupted")
        self.assertEqual(payload["response"], "partial\n")
        self.assertFalse(marker.exists(), "interrupted worker must not finish")


class CommandLineTests(OrchestratorTestCase):
    def test_main_reports_usage_errors(self) -> None:
        stderr = io.StringIO()
        with mock.patch.object(sys, "stderr", stderr):
            missing_required = orchestrator.main([])
        self.assertEqual(missing_required, 2)
        self.assertIn("usage:", stderr.getvalue())

        stderr = io.StringIO()
        with mock.patch.object(sys, "stderr", stderr):
            bad_timeout = orchestrator.main(
                [
                    "--task-file",
                    str(self._task()),
                    "--timeout-seconds",
                    "0",
                    "--result-file",
                    str(self.root / "unused.json"),
                ]
            )
        self.assertEqual(bad_timeout, 2)
        self.assertFalse((self.root / "unused.json").exists())

    def test_main_rejects_a_missing_workspace_without_writing_a_result(self) -> None:
        result_file = self.root / "unused.json"
        stderr = io.StringIO()
        with mock.patch.object(sys, "stderr", stderr):
            code = orchestrator.main(
                [
                    "--task-file",
                    str(self._task()),
                    "--workspace",
                    str(self.root / "nope"),
                    "--result-file",
                    str(result_file),
                ]
            )
        self.assertEqual(code, 2)
        self.assertIn("--workspace is not a directory", stderr.getvalue())
        self.assertFalse(result_file.exists())

    def test_main_help_exits_zero(self) -> None:
        stdout = io.StringIO()
        with mock.patch.object(sys, "stdout", stdout):
            code = orchestrator.main(["--help"])
        self.assertEqual(code, 0)
        self.assertIn("--task-file", stdout.getvalue())

    def test_main_runs_end_to_end_with_explicit_paths(self) -> None:
        task = self._task("hello\n")
        config = self._config(stdout="answer\n")
        result_file = self.root / "cli" / "result.json"
        out, err = io.StringIO(), io.StringIO()
        env = self._base_env(config)
        with mock.patch.dict(os.environ, env, clear=True), mock.patch.object(
            sys, "stdout", out
        ), mock.patch.object(sys, "stderr", err):
            code = orchestrator.main(
                [
                    "--task-file",
                    str(task),
                    "--result-file",
                    str(result_file),
                    "--dsh",
                    str(self.fake_dsh),
                    "--lock-file",
                    str(self.root / "cli" / "run.lock"),
                    "--workspace",
                    str(ROOT),
                ]
            )
        self.assertEqual(code, 0, err.getvalue())
        self.assertEqual(out.getvalue().strip(), str(result_file))
        payload = json.loads(result_file.read_text(encoding="utf-8"))
        self.assertEqual(payload["outcome"], "completed")
        self.assertEqual(payload["response"], "answer\n")


class RedactionTests(unittest.TestCase):
    def test_short_and_unrelated_values_are_not_redacted(self) -> None:
        env = {"PATH": "/usr/bin", "SHORT_TOKEN": "abc", "MY_NOTE": "not-a-secret-value"}
        text = "abc /usr/bin/not-a-secret-value"
        self.assertEqual(orchestrator.redact_secrets(text, env), text)

    def test_longer_secrets_replace_their_own_substrings_first(self) -> None:
        env = {"A_TOKEN": "abcdefgh", "B_TOKEN": "xabcdefghy"}
        self.assertEqual(
            orchestrator.redact_secrets("value=xabcdefghy", env),
            "value=[REDACTED:B_TOKEN]",
        )

    def test_empty_input_and_empty_env_are_unchanged(self) -> None:
        self.assertEqual(orchestrator.redact_secrets("", {"A_TOKEN": "abcdefgh"}), "")
        self.assertEqual(orchestrator.redact_secrets("text", {}), "text")


if __name__ == "__main__":
    unittest.main()
