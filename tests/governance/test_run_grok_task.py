"""Focused tests for scripts/run_grok_task.py.

Every test drives a fake ``grok`` executable generated into a temporary
directory: no network access, no real CLI session, no credentials, and no
writes outside the test's temp root. The tests assert the orchestration
contract only: safe argv/defaults, task isolation, bounded capture, timeout
process-group termination, atomic JSON, and dry-run.
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
import time
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
SCRIPTS = ROOT / "scripts"
sys.path.insert(0, str(SCRIPTS))

import run_grok_task as orchestrator  # noqa: E402

SCRIPT_PATH = SCRIPTS / "run_grok_task.py"
SECRET_TASK = "UNIQUE_SECRET_TASK_PAYLOAD do the work\n"

FAKE_GROK_SOURCE = '''#!/usr/bin/env python3
"""Fake grok launcher for orchestrator tests; never touches the network."""

from __future__ import annotations

import json
import os
import sys
import time


def main() -> int:
    config_path = os.environ.get("FAKE_GROK_CONFIG")
    config = json.loads(open(config_path, "r", encoding="utf-8").read()) if config_path else {}

    if config.get("record"):
        with open(config["record"], "w", encoding="utf-8") as handle:
            json.dump(
                {
                    "argv": sys.argv,
                    "cwd": os.getcwd(),
                    "env_keys": sorted(os.environ),
                },
                handle,
            )

    grandchild_marker = config.get("grandchild_marker")
    grandchild_pidfile = config.get("grandchild_pidfile")
    if grandchild_marker:
        pid = os.fork()
        if pid == 0:
            if grandchild_pidfile:
                with open(grandchild_pidfile, "w", encoding="utf-8") as handle:
                    handle.write(str(os.getpid()))
            time.sleep(float(config.get("grandchild_sleep", 8)))
            with open(grandchild_marker, "w", encoding="utf-8") as handle:
                handle.write("survived")
            os._exit(0)
        if grandchild_pidfile:
            deadline = time.time() + 5.0
            while time.time() < deadline and not os.path.exists(grandchild_pidfile):
                time.sleep(0.01)

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
        if config.get("write_file"):
            with open(config["write_file"], "w", encoding="utf-8") as handle:
                handle.write(config.get("write_content", "changed\\n"))
        sys.stdout.write(config.get("stdout", ""))
        sys.stdout.flush()
        sys.stderr.write(config.get("stderr", ""))
        sys.stderr.flush()

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
    """Shared fixture: temp root, fake grok, config/result helpers."""

    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.fake_grok = self._write_fake_grok()
        self.record = self.root / "record.json"

    def _write_fake_grok(self) -> Path:
        path = self.root / "fake-grok"
        source = FAKE_GROK_SOURCE.replace(
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

    def _base_env(self, config_path: Path, extra: dict[str, str] | None = None) -> dict[str, str]:
        env = {
            "PATH": os.environ.get("PATH", ""),
            "HOME": str(self.root),
            "FAKE_GROK_CONFIG": str(config_path),
        }
        env.update(extra or {})
        return env

    def _run(
        self,
        task_text: str = SECRET_TASK,
        *,
        config: dict[str, object] | None = None,
        env_extra: dict[str, str] | None = None,
        task_file: Path | None = None,
        task_bytes: bytes | None = None,
        grok: Path | str | None = None,
        result_file: Path | None = None,
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
            workspace=overrides.pop("workspace", self.root),
            grok=str(grok if grok is not None else self.fake_grok),
            timeout_seconds=overrides.pop("timeout_seconds", 15.0),
            kill_grace_seconds=overrides.pop("kill_grace_seconds", 1.0),
            max_task_bytes=overrides.pop(
                "max_task_bytes", orchestrator.DEFAULT_MAX_TASK_BYTES
            ),
            max_output_bytes=overrides.pop("max_output_bytes", 1 << 20),
            max_turns=overrides.pop("max_turns", orchestrator.DEFAULT_MAX_TURNS),
            reasoning_effort=overrides.pop(
                "reasoning_effort", orchestrator.DEFAULT_REASONING_EFFORT
            ),
            sandbox=overrides.pop("sandbox", orchestrator.DEFAULT_SANDBOX),
            require_change=overrides.pop("require_change", False),
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


class SafeInvocationTests(OrchestratorTestCase):
    def test_exact_safe_argv_and_defaults(self) -> None:
        code, stdout, stderr, request = self._run(
            config={"record": str(self.record), "stdout": "done\n"},
        )
        self.assertEqual(code, 0, stderr)
        recorded = self._recorded(self.record)
        expected_argv = [
            str(self.fake_grok),
            "--model",
            "grok-4.7",
            "--reasoning-effort",
            "xhigh",
            "--sandbox",
            "workspace",
            "--permission-mode",
            "bypassPermissions",
            "--disable-web-search",
            "--no-subagents",
            "--no-plan",
            "--system-prompt-override",
            orchestrator.GROK_SYSTEM_PROMPT,
            "--json-schema",
            orchestrator.RECEIPT_SCHEMA,
            "--output-format",
            "json",
            "--max-turns",
            str(orchestrator.DEFAULT_MAX_TURNS),
            "--cwd",
            str(self.root),
            "--prompt-file",
            str(request.task_file),
        ]
        self.assertEqual(recorded["argv"], expected_argv)
        self.assertEqual(recorded["cwd"], str(self.root))
        self.assertNotIn("--resume", recorded["argv"])
        self.assertNotIn("--continue", recorded["argv"])
        self.assertNotIn("-c", recorded["argv"])
        self.assertNotIn("-r", recorded["argv"])
        self.assertNotIn("-p", recorded["argv"])
        self.assertNotIn("--single", recorded["argv"])
        self.assertEqual(stdout.strip(), str(request.result_file))
        self.assertEqual(stderr, "")

        result = self._read_result(request)
        self.assertEqual(result["grok"]["argv"], orchestrator.display_argv(expected_argv))
        self.assertEqual(result["grok"]["argv"][-1], "[TASK_FILE]")
        self.assertEqual(result["settings"]["model"], "grok-4.7")
        self.assertEqual(result["settings"]["reasoning_effort"], "xhigh")
        self.assertEqual(result["settings"]["sandbox"], "workspace")
        self.assertEqual(result["settings"]["permission_mode"], "bypassPermissions")
        self.assertFalse(result["settings"]["web_search"])
        self.assertFalse(result["settings"]["subagents"])
        self.assertFalse(result["settings"]["plan_mode"])
        self.assertTrue(result["settings"]["completion_receipt"])
        self.assertFalse(result["settings"]["require_change"])
        self.assertEqual(
            result["settings"]["rules_profile"], orchestrator.RULES_PROFILE
        )
        self.assertEqual(result["settings"]["output_format"], "json")
        self.assertEqual(result["settings"]["max_turns"], orchestrator.DEFAULT_MAX_TURNS)
        self.assertTrue(result["settings"]["fresh_session"])
        self.assertNotIn("env", result)

    def test_task_text_never_appears_in_argv_or_result(self) -> None:
        code, _, _, request = self._run(
            config={"record": str(self.record), "stdout": "ok\n"},
        )
        self.assertEqual(code, 0)
        recorded = json.dumps(self._recorded(self.record))
        result_raw = request.result_file.read_text(encoding="utf-8")
        self.assertNotIn("UNIQUE_SECRET_TASK_PAYLOAD", recorded)
        self.assertNotIn("UNIQUE_SECRET_TASK_PAYLOAD", result_raw)
        result = json.loads(result_raw)
        payload = SECRET_TASK.encode("utf-8")
        self.assertEqual(result["task"]["bytes"], len(payload))
        self.assertEqual(result["task"]["sha256"], hashlib.sha256(payload).hexdigest())
        self.assertNotIn("text", result["task"])
        self.assertNotIn("content", result["task"])
        prompt_index = result["grok"]["argv"].index("--prompt-file")
        self.assertEqual(result["grok"]["argv"][prompt_index + 1], "[TASK_FILE]")


class ResultContractTests(OrchestratorTestCase):
    def test_success_captures_response_and_diagnostics_without_forwarding(self) -> None:
        code, stdout, stderr, request = self._run(
            config={
                "stdout": "final answer\n",
                "stderr": "grok: reasoning: step 1\n",
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
        self.assertEqual(result["diagnostics"], "grok: reasoning: step 1\n")
        self.assertIsNone(result["error"])
        self.assertTrue(result["stream_drain_complete"])
        self.assertIsNotNone(result["started_at"])
        self.assertIsNotNone(result["finished_at"])
        self.assertGreaterEqual(result["duration_seconds"], 0)
        self.assertNotIn("final answer", stdout)
        self.assertNotIn("reasoning", stdout)

    def test_nonzero_exit_is_child_failed(self) -> None:
        code, _, _, request = self._run(
            config={"exit_code": 1, "stderr": "grok: aborted: tests failed\n"}
        )
        self.assertEqual(code, 1)
        result = self._read_result(request)
        self.assertFalse(result["ok"])
        self.assertEqual(result["outcome"], "child_failed")
        self.assertEqual(result["exit_code"], 1)
        self.assertIn("aborted", result["error"])
        self.assertFalse(result["timed_out"])

    def test_max_turns_failure_is_reported_explicitly(self) -> None:
        code, _, _, request = self._run(
            config={"exit_code": 1, "stderr": "Error: max turns reached\n"}
        )
        self.assertEqual(code, 1)
        result = self._read_result(request)
        self.assertFalse(result["ok"])
        self.assertEqual(result["outcome"], "max_turns_reached")
        self.assertEqual(result["exit_code"], 1)
        self.assertIn("max turns reached", result["error"])
        self.assertFalse(result["timed_out"])

    def test_result_is_written_atomically_mode_0600_without_leftovers(self) -> None:
        code, _, _, request = self._run(config={"stdout": "ok\n"})
        self.assertEqual(code, 0)
        parent = request.result_file.parent
        leftovers = list(parent.glob("*.tmp")) + list(parent.glob(".*.tmp"))
        self.assertEqual(leftovers, [])
        self.assertTrue(request.result_file.is_file())
        self.assertEqual(stat.S_IMODE(request.result_file.stat().st_mode), 0o600)

    def test_output_over_cap_is_truncated_and_the_stream_is_still_drained(self) -> None:
        started = time.monotonic()
        code, _, _, request = self._run(
            config={"stream_bytes": 256 * 1024, "stream_target": "stdout"},
            max_output_bytes=4096,
            timeout_seconds=15.0,
        )
        elapsed = time.monotonic() - started
        self.assertEqual(code, 0)
        result = self._read_result(request)
        self.assertTrue(result["ok"])
        self.assertEqual(result["outcome"], "completed")
        self.assertEqual(len(result["response"]), 4096)
        self.assertTrue(result["output_overflow"]["stdout_truncated"])
        self.assertGreaterEqual(result["output_overflow"]["stdout_bytes"], 256 * 1024)
        self.assertFalse(result["output_overflow"]["stderr_truncated"])
        self.assertTrue(result["stream_drain_complete"])
        self.assertLess(elapsed, 10.0)


class ImplementationProofTests(OrchestratorTestCase):
    def setUp(self) -> None:
        super().setUp()
        subprocess.run(["git", "init", "-q"], cwd=self.root, check=True)

    @staticmethod
    def _receipt(outcome: str = "COMPLETED") -> str:
        receipt = {
            "outcome": outcome,
            "changed_files": ["implemented.txt"] if outcome == "COMPLETED" else [],
            "checks": ["focused check passed"] if outcome == "COMPLETED" else [],
            "remaining": [],
        }
        return json.dumps({"text": json.dumps(receipt)})

    def test_require_change_rejects_success_with_no_workspace_edit(self) -> None:
        code, _, _, request = self._run(
            config={"stdout": self._receipt()}, require_change=True
        )
        self.assertEqual(code, 1)
        result = self._read_result(request)
        self.assertFalse(result["ok"])
        self.assertEqual(result["outcome"], "unverified_completion")
        self.assertFalse(result["workspace_change"]["observed"])
        self.assertIn("made no workspace change", result["error"])

    def test_require_change_accepts_real_edit_with_completed_receipt(self) -> None:
        target = self.root / "implemented.txt"
        code, _, _, request = self._run(
            config={
                "stdout": self._receipt(),
                "write_file": str(target),
                "write_content": "actual implementation\n",
            },
            require_change=True,
        )
        self.assertEqual(code, 0)
        result = self._read_result(request)
        self.assertTrue(result["ok"])
        self.assertEqual(result["outcome"], "completed")
        self.assertTrue(result["workspace_change"]["observed"])
        self.assertEqual(result["receipt"]["outcome"], "COMPLETED")
        self.assertEqual(target.read_text(encoding="utf-8"), "actual implementation\n")

    def test_require_change_reads_grok_structured_output(self) -> None:
        receipt = {
            "outcome": "COMPLETED",
            "changed_files": ["implemented.txt"],
            "checks": ["focused check passed"],
            "remaining": [],
        }
        code, _, _, request = self._run(
            config={
                "stdout": json.dumps(
                    {"text": "non-canonical display text", "structuredOutput": receipt}
                ),
                "write_file": str(self.root / "implemented.txt"),
            },
            require_change=True,
        )
        self.assertEqual(code, 0)
        result = self._read_result(request)
        self.assertEqual(result["receipt"], receipt)

    def test_require_change_rejects_edit_without_valid_receipt(self) -> None:
        code, _, _, request = self._run(
            config={
                "stdout": '{"text":"I changed it"}',
                "write_file": str(self.root / "implemented.txt"),
            },
            require_change=True,
        )
        self.assertEqual(code, 1)
        result = self._read_result(request)
        self.assertEqual(result["outcome"], "unverified_completion")
        self.assertTrue(result["workspace_change"]["observed"])
        self.assertIsNone(result["receipt"])
        self.assertIn("valid COMPLETED receipt", result["error"])

    def test_require_change_rejects_receipt_claiming_a_different_file(self) -> None:
        receipt = {
            "outcome": "COMPLETED",
            "changed_files": ["invented.py"],
            "checks": ["focused check passed"],
            "remaining": [],
        }
        code, _, _, request = self._run(
            config={
                "stdout": json.dumps({"structuredOutput": receipt}),
                "write_file": str(self.root / "implemented.txt"),
            },
            require_change=True,
        )
        self.assertEqual(code, 1)
        result = self._read_result(request)
        self.assertEqual(result["outcome"], "unverified_completion")
        self.assertEqual(result["workspace_change"]["actual_files"], ["implemented.txt"])
        self.assertIn("did not actually change", result["error"])

    def test_require_change_rejects_completed_receipt_without_a_check(self) -> None:
        receipt = {
            "outcome": "COMPLETED",
            "changed_files": ["implemented.txt"],
            "checks": [],
            "remaining": [],
        }
        code, _, _, request = self._run(
            config={
                "stdout": json.dumps({"structuredOutput": receipt}),
                "write_file": str(self.root / "implemented.txt"),
            },
            require_change=True,
        )
        self.assertEqual(code, 1)
        result = self._read_result(request)
        self.assertEqual(result["outcome"], "unverified_completion")
        self.assertIn("no focused check", result["error"])


class TimeoutTests(OrchestratorTestCase):
    def test_timeout_terminates_the_whole_process_group(self) -> None:
        marker = self.root / "grandchild-survived"
        pidfile = self.root / "grandchild.pid"
        finished = self.root / "finished"
        code, _, _, request = self._run(
            config={
                "stdout": "partial\n",
                "stderr": "grok: still working\n",
                "sleep_seconds": 8.0,
                "grandchild_marker": str(marker),
                "grandchild_pidfile": str(pidfile),
                "grandchild_sleep": 1.5,
                "finished_marker": str(finished),
            },
            timeout_seconds=0.6,
            kill_grace_seconds=0.5,
        )
        self.assertEqual(code, 4)
        result = self._read_result(request)
        self.assertFalse(result["ok"])
        self.assertEqual(result["outcome"], "timed_out")
        self.assertTrue(result["timed_out"])
        self.assertEqual(result["response"], "partial\n")
        self.assertFalse(finished.exists(), "killed worker must not finish")
        self.assertLess(result["duration_seconds"], 4.0)
        self.assertTrue(pidfile.exists(), "grandchild should have started")
        # Wait long enough that an unkilled grandchild would have written the
        # marker; absence proves the whole process group was terminated.
        deadline = time.monotonic() + 2.0
        while time.monotonic() < deadline and not marker.exists():
            time.sleep(0.05)
        self.assertFalse(marker.exists(), "process-group grandchild must not survive")
        grandchild_pid = int(pidfile.read_text(encoding="utf-8"))
        still_running = True
        try:
            os.kill(grandchild_pid, 0)
            status_path = Path(f"/proc/{grandchild_pid}/status")
            if not status_path.exists():
                still_running = False
            else:
                state = status_path.read_text(encoding="utf-8")
                still_running = "State:\tZ" not in state
        except OSError:
            still_running = False
        self.assertFalse(still_running, "grandchild pid must not still be running")


class TaskValidationTests(OrchestratorTestCase):
    def test_size_nul_and_utf8_are_rejected_without_starting_grok(self) -> None:
        cases: dict[str, tuple[Path, bytes | None]] = {
            "missing": (self.root / "missing.md", None),
            "blank": (self.root / "blank.md", b"   \n\t\n"),
            "nul": (self.root / "nul.md", b"hello\x00world"),
            "not-utf8": (self.root / "bad.md", b"\xff\xfetask"),
            "oversize": (self.root / "big.md", b"x" * 64),
            "directory": (self.root / "task-dir", None),
        }
        (self.root / "task-dir").mkdir()
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
                dumped = json.dumps(result)
                self.assertNotIn("UNIQUE_SECRET_TASK_PAYLOAD", dumped)
                self.assertFalse(self.record.exists(), "worker must not start")


class MissingExecutableTests(OrchestratorTestCase):
    def test_missing_grok_executable_fails_truthfully(self) -> None:
        missing = self.root / "no-such-grok"
        code, _, _, request = self._run(grok=missing)
        self.assertEqual(code, 3)
        result = self._read_result(request)
        self.assertFalse(result["ok"])
        self.assertEqual(result["outcome"], "grok_not_found")
        self.assertIsNone(result["grok"]["path"])
        self.assertEqual(result["grok"]["requested"], str(missing))
        self.assertIsNone(result["grok"]["argv"])
        self.assertIn("does not exist", result["error"])


class DryRunTests(OrchestratorTestCase):
    def test_dry_run_prints_redacted_plan_without_running_or_writing(self) -> None:
        code, stdout, _, request = self._run(
            dry_run=True, config={"record": str(self.record)}
        )
        self.assertEqual(code, 0)
        plan = json.loads(stdout)
        self.assertTrue(plan["dry_run"])
        self.assertEqual(plan["settings"]["model"], "grok-4.7")
        self.assertEqual(plan["settings"]["reasoning_effort"], "xhigh")
        self.assertEqual(plan["settings"]["sandbox"], "workspace")
        self.assertEqual(plan["settings"]["permission_mode"], "bypassPermissions")
        self.assertFalse(plan["settings"]["web_search"])
        self.assertFalse(plan["settings"]["subagents"])
        argv = plan["grok"]["argv"]
        self.assertIn("--prompt-file", argv)
        self.assertEqual(argv[argv.index("--prompt-file") + 1], "[TASK_FILE]")
        self.assertNotIn("--resume", argv)
        self.assertNotIn(SECRET_TASK.strip(), json.dumps(plan))
        self.assertNotIn("UNIQUE_SECRET_TASK_PAYLOAD", json.dumps(plan))
        self.assertFalse(request.result_file.exists())
        self.assertFalse(self.record.exists())


class CommandLineTests(OrchestratorTestCase):
    def test_main_rejects_non_directory_workspace_without_writing_a_result(self) -> None:
        result_file = self.root / "unused.json"
        stderr = io.StringIO()
        with mock.patch.object(sys, "stderr", stderr):
            code = orchestrator.main(
                [
                    "--task-file",
                    str(self.root / "task.md"),
                    "--workspace",
                    str(self.root / "nope"),
                    "--result-file",
                    str(result_file),
                    "--grok",
                    str(self.fake_grok),
                ]
            )
        self.assertEqual(code, 2)
        self.assertIn("--workspace is not a directory", stderr.getvalue())
        self.assertFalse(result_file.exists())

    def test_main_rejects_non_positive_limits_and_disallowed_settings(self) -> None:
        task = self.root / "task.md"
        task.write_text("do the work\n", encoding="utf-8")
        cases = [
            ["--timeout-seconds", "0"],
            ["--max-task-bytes", "-1"],
            ["--max-output-bytes", "0"],
            ["--max-turns", "0"],
            ["--reasoning-effort", "ultra"],
            ["--sandbox", "danger-full-access"],
        ]
        for extra in cases:
            with self.subTest(flags=extra):
                unused = self.root / f"unused-{'-'.join(extra)}.json"
                stderr = io.StringIO()
                with mock.patch.object(sys, "stderr", stderr):
                    code = orchestrator.main(
                        [
                            "--task-file",
                            str(task),
                            "--result-file",
                            str(unused),
                            "--grok",
                            str(self.fake_grok),
                            "--workspace",
                            str(self.root),
                            *extra,
                        ]
                    )
                self.assertEqual(code, 2)
                self.assertFalse(unused.exists())

    def test_main_help_exits_zero(self) -> None:
        stdout = io.StringIO()
        with mock.patch.object(sys, "stdout", stdout):
            code = orchestrator.main(["--help"])
        self.assertEqual(code, 0)
        self.assertIn("--task-file", stdout.getvalue())
        self.assertIn("--prompt-file", stdout.getvalue())


if __name__ == "__main__":
    unittest.main()
