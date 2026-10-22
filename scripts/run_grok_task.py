#!/usr/bin/env python3
"""Repository-local, non-interactive orchestration entrypoint for the Grok 4.7 worker.

Grok implements the task file. Codex orchestrates, reviews, tests, and accepts.
This script is transport and bookkeeping only: it never edits product code,
never commits, and never records an acceptance decision.

It invokes the Grok CLI with ``--prompt-file`` (task text stays out of process
arguments), pinned safe defaults, a timeout that kills the child process group,
and one atomic mode-0600 JSON result. It never resumes a previous session.

See ``docs/maintainers/agent/grok-orchestrator.md`` for the contract.
"""

from __future__ import annotations

import argparse
import datetime as _dt
import hashlib
import json
import math
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
import uuid
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable, Mapping, Sequence, TextIO

SCRIPT_NAME = "scripts/run_grok_task.py"
SCHEMA_VERSION = 1
MODEL_ID = "grok-4.7"
PERMISSION_MODE = "bypassPermissions"
OUTPUT_FORMAT = "json"
DEFAULT_REASONING_EFFORT = "xhigh"
DEFAULT_SANDBOX = "workspace"
DEFAULT_GROK = "grok"
DEFAULT_TIMEOUT_SECONDS = 3600.0
DEFAULT_KILL_GRACE_SECONDS = 10.0
DEFAULT_MAX_TASK_BYTES = 256 * 1024
DEFAULT_MAX_OUTPUT_BYTES = 8 * 1024 * 1024
DEFAULT_MAX_TURNS = 24
RULES_PROFILE = "narrivy-grounded-v2"
GROK_SYSTEM_PROMPT = """\
You are Grok 4.7 acting as a bounded implementation worker. The user task file is
authoritative. Work only inside the supplied workspace and finish the smallest correct
change that satisfies it.

Repository tools are the only source of repository facts. Locate and read a file, symbol,
API, command, or test in this run before describing or using it. If required material is
absent, return BLOCKED with the exact missing item. Use only tools actually available in
the session. Text printed in your answer is not an implementation: implementation work
must be written to repository files with tools.

Inspect existing contracts and affected call sites before editing. Run the task's focused
checks after editing and inspect the resulting diff. A check passed only when its command
completed successfully in this run. Do not invent files, APIs, edits, commands, results, or
verification. A blocked or partial result is useful and must be reported truthfully.

Run each long command once. Wait once for its bounded result or report it still running;
do not spend turns polling. Stop as soon as the bounded acceptance conditions are met.
Never claim that the wider project or release is complete.

Your final response must follow the supplied JSON schema. Use COMPLETED only after real
workspace edits and successful requested checks. Use NO_CHANGE_REQUIRED only when the task
was already satisfied. Otherwise use BLOCKED and state the concrete reason.
"""
RECEIPT_SCHEMA = json.dumps(
    {
        "type": "object",
        "additionalProperties": False,
        "required": ["outcome", "changed_files", "checks", "remaining"],
        "properties": {
            "outcome": {
                "type": "string",
                "enum": ["COMPLETED", "BLOCKED", "NO_CHANGE_REQUIRED"],
            },
            "changed_files": {"type": "array", "items": {"type": "string"}},
            "checks": {"type": "array", "items": {"type": "string"}},
            "remaining": {"type": "array", "items": {"type": "string"}},
        },
    },
    separators=(",", ":"),
)
RUN_DIRECTORY_REL = "artifacts/grok-runs"
REASONING_EFFORTS = ("low", "medium", "high", "xhigh")
SANDBOXES = ("workspace", "read-only")
TASK_FILE_PLACEHOLDER = "[TASK_FILE]"

OUTCOME_COMPLETED = "completed"
OUTCOME_CHILD_FAILED = "child_failed"
OUTCOME_MAX_TURNS_REACHED = "max_turns_reached"
OUTCOME_TIMED_OUT = "timed_out"
OUTCOME_SPAWN_FAILED = "spawn_failed"
OUTCOME_GROK_NOT_FOUND = "grok_not_found"
OUTCOME_INVALID_TASK = "invalid_task_file"
OUTCOME_UNVERIFIED_COMPLETION = "unverified_completion"

EXIT_OK = 0
EXIT_CHILD_FAILED = 1
EXIT_USAGE = 2
EXIT_SPAWN_FAILED = 3
EXIT_TIMED_OUT = 4
EXIT_RESULT_WRITE_FAILED = 7

REPOSITORY_ROOT = Path(__file__).resolve().parents[1]
_READ_CHUNK_BYTES = 65536
_MIN_STREAM_JOIN_SECONDS = 5.0
_MAX_ERROR_LINE_CHARS = 300


@dataclass(frozen=True)
class RunRequest:
    """One fully resolved orchestration request."""

    task_file: Path
    result_file: Path
    workspace: Path
    grok: str
    timeout_seconds: float = DEFAULT_TIMEOUT_SECONDS
    kill_grace_seconds: float = DEFAULT_KILL_GRACE_SECONDS
    max_task_bytes: int = DEFAULT_MAX_TASK_BYTES
    max_output_bytes: int = DEFAULT_MAX_OUTPUT_BYTES
    max_turns: int = DEFAULT_MAX_TURNS
    reasoning_effort: str = DEFAULT_REASONING_EFFORT
    sandbox: str = DEFAULT_SANDBOX
    require_change: bool = False
    dry_run: bool = False


class _OrchestrationInputError(Exception):
    """A local precondition failed; ``meta`` carries whatever was already known."""

    def __init__(self, message: str, meta: Mapping[str, Any] | None = None) -> None:
        super().__init__(message)
        self.meta: dict[str, Any] = dict(meta or {})


def _utc_now() -> str:
    return (
        _dt.datetime.now(_dt.timezone.utc)
        .isoformat(timespec="milliseconds")
        .replace("+00:00", "Z")
    )


def _first_non_empty_line(text: str, limit: int = _MAX_ERROR_LINE_CHARS) -> str:
    for line in text.splitlines():
        stripped = line.strip()
        if stripped:
            return stripped[:limit]
    return ""


class _StreamCapture(threading.Thread):
    """Drain one child pipe completely while retaining at most ``limit`` bytes."""

    def __init__(self, stream: Any, limit: int) -> None:
        super().__init__(name="grok-stream-capture", daemon=True)
        self._stream = stream
        self._limit = limit
        self._chunks: list[bytes] = []
        self._finished = threading.Event()
        self.total_bytes = 0
        self.truncated = False
        self.error: str | None = None

    @property
    def finished(self) -> bool:
        return self._finished.is_set()

    def run(self) -> None:  # noqa: D102 - threading entry point
        retained = 0
        try:
            while True:
                chunk = self._stream.read(_READ_CHUNK_BYTES)
                if not chunk:
                    break
                self.total_bytes += len(chunk)
                if retained < self._limit:
                    take = min(self._limit - retained, len(chunk))
                    self._chunks.append(bytes(chunk[:take]))
                    retained += take
                    if take < len(chunk):
                        self.truncated = True
                else:
                    self.truncated = True
        except Exception as exc:  # pragma: no cover - defensive
            self.error = f"{type(exc).__name__}: {exc}"
        finally:
            self._finished.set()

    def text(self) -> str:
        return b"".join(self._chunks).decode("utf-8", "replace")


def _signal_process_group(process: subprocess.Popen, sig: int) -> None:
    try:
        os.killpg(os.getpgid(process.pid), sig)
    except OSError:
        try:
            process.send_signal(sig)
        except OSError:
            pass


def _wait_briefly(process: subprocess.Popen, seconds: float) -> bool:
    try:
        process.wait(timeout=max(0.0, seconds))
        return True
    except subprocess.TimeoutExpired:
        return False


def _terminate_process_group(process: subprocess.Popen, grace_seconds: float) -> None:
    """SIGTERM the child's process group, then SIGKILL after ``grace_seconds``."""

    if process.poll() is not None:
        return
    _signal_process_group(process, signal.SIGTERM)
    if _wait_briefly(process, grace_seconds):
        return
    _signal_process_group(process, signal.SIGKILL)
    _wait_briefly(process, max(1.0, grace_seconds))


def _load_task(request: RunRequest) -> dict[str, Any]:
    path = request.task_file
    meta: dict[str, Any] = {"file": str(path)}
    try:
        stat_result = path.stat()
    except OSError as exc:
        raise _OrchestrationInputError(
            f"task file is not readable: {path}: {exc}", meta
        ) from exc
    if not path.is_file():
        raise _OrchestrationInputError(f"task file is not a regular file: {path}", meta)
    if stat_result.st_size > request.max_task_bytes:
        meta["bytes"] = stat_result.st_size
        raise _OrchestrationInputError(
            f"task file is {stat_result.st_size} bytes, above "
            f"--max-task-bytes={request.max_task_bytes}",
            meta,
        )
    try:
        raw = path.read_bytes()
    except OSError as exc:
        raise _OrchestrationInputError(
            f"task file could not be read: {path}: {exc}", meta
        ) from exc
    meta["bytes"] = len(raw)
    meta["sha256"] = hashlib.sha256(raw).hexdigest()
    if len(raw) > request.max_task_bytes:
        raise _OrchestrationInputError(
            f"task file grew to {len(raw)} bytes, above "
            f"--max-task-bytes={request.max_task_bytes}",
            meta,
        )
    if b"\x00" in raw:
        raise _OrchestrationInputError(
            "task file contains NUL bytes and cannot be used as a prompt file",
            meta,
        )
    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError as exc:
        raise _OrchestrationInputError(
            f"task file is not valid UTF-8: {exc}", meta
        ) from exc
    if not text.strip():
        raise _OrchestrationInputError(
            "task file is empty or contains only whitespace", meta
        )
    return meta


def _resolve_grok(name: str) -> Path:
    if not name:
        raise _OrchestrationInputError("--grok must not be empty")
    if os.sep in name or (os.altsep is not None and os.altsep in name):
        candidate = Path(name)
    else:
        found = shutil.which(name)
        if found is None:
            raise _OrchestrationInputError(
                f"grok executable {name!r} was not found on PATH"
            )
        candidate = Path(found)
    if not candidate.exists():
        raise _OrchestrationInputError(f"grok executable does not exist: {candidate}")
    if not candidate.is_file():
        raise _OrchestrationInputError(
            f"grok executable is not a regular file: {candidate}"
        )
    if not os.access(candidate, os.X_OK):
        raise _OrchestrationInputError(f"grok executable is not runnable: {candidate}")
    # Deliberately no symlink resolution: a launcher shim must run as itself.
    return Path(os.path.abspath(candidate))


def _fsync_directory(directory: Path) -> None:
    try:
        fd = os.open(str(directory), os.O_RDONLY)
    except OSError:
        return
    try:
        os.fsync(fd)
    except OSError:
        pass
    finally:
        os.close(fd)


def _write_result_atomically(path: Path, payload: Mapping[str, Any]) -> str | None:
    """Write ``payload`` as JSON atomically; return an error string on failure."""

    temp_path: Path | None = None
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        fd, temp_name = tempfile.mkstemp(
            prefix=f".{path.name}.", suffix=".tmp", dir=str(path.parent)
        )
        temp_path = Path(temp_name)
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            json.dump(
                payload,
                handle,
                indent=2,
                sort_keys=True,
                ensure_ascii=False,
                default=str,
            )
            handle.write("\n")
            handle.flush()
            os.fsync(handle.fileno())
        os.chmod(temp_path, 0o600)
        os.replace(temp_path, path)
        temp_path = None
    except OSError as exc:
        return f"{type(exc).__name__}: {exc}"
    finally:
        if temp_path is not None:
            try:
                temp_path.unlink()
            except OSError:
                pass
    _fsync_directory(path.parent)
    return None


def build_argv(grok_path: Path, request: RunRequest) -> list[str]:
    """Exact Grok CLI argv. Never includes --resume/--continue or task text."""

    return [
        str(grok_path),
        "--model",
        MODEL_ID,
        "--reasoning-effort",
        request.reasoning_effort,
        "--sandbox",
        request.sandbox,
        "--permission-mode",
        PERMISSION_MODE,
        "--disable-web-search",
        "--no-subagents",
        "--no-plan",
        "--system-prompt-override",
        GROK_SYSTEM_PROMPT,
        "--json-schema",
        RECEIPT_SCHEMA,
        "--output-format",
        OUTPUT_FORMAT,
        "--max-turns",
        str(request.max_turns),
        "--cwd",
        str(request.workspace),
        "--prompt-file",
        str(request.task_file),
    ]


def display_argv(argv: Sequence[str]) -> list[str]:
    """Exact argv shape with the prompt path replaced by ``[TASK_FILE]``."""

    displayed: list[str] = []
    skip_value = False
    for index, arg in enumerate(argv):
        if skip_value:
            displayed.append(TASK_FILE_PLACEHOLDER)
            skip_value = False
            continue
        displayed.append(arg)
        if arg == "--prompt-file" and index + 1 < len(argv):
            skip_value = True
    return displayed


def _settings(request: RunRequest) -> dict[str, Any]:
    return {
        "model": MODEL_ID,
        "reasoning_effort": request.reasoning_effort,
        "sandbox": request.sandbox,
        "permission_mode": PERMISSION_MODE,
        "web_search": False,
        "subagents": False,
        "plan_mode": False,
        "rules_profile": RULES_PROFILE,
        "completion_receipt": True,
        "require_change": request.require_change,
        "output_format": OUTPUT_FORMAT,
        "max_turns": request.max_turns,
        "fresh_session": True,
    }


def _base_result(request: RunRequest, run_id: str, started_at: str) -> dict[str, Any]:
    return {
        "schema_version": SCHEMA_VERSION,
        "run_id": run_id,
        "started_at": started_at,
        "finished_at": None,
        "duration_seconds": None,
        "ok": False,
        "outcome": None,
        "exit_code": None,
        "signal": None,
        "timed_out": False,
        "response": "",
        "receipt": None,
        "diagnostics": "",
        "stream_drain_complete": None,
        "output_overflow": {
            "stdout_truncated": False,
            "stderr_truncated": False,
            "stdout_bytes": 0,
            "stderr_bytes": 0,
        },
        "task": {"file": str(request.task_file), "bytes": None, "sha256": None},
        "grok": {
            "requested": request.grok,
            "path": None,
            "argv": None,
        },
        "settings": _settings(request),
        "workspace": str(request.workspace),
        "workspace_change": {
            "required": request.require_change,
            "observed": None,
            "before_sha256": None,
            "after_sha256": None,
            "actual_files": [],
        },
        "limits": {
            "timeout_seconds": request.timeout_seconds,
            "kill_grace_seconds": request.kill_grace_seconds,
            "max_task_bytes": request.max_task_bytes,
            "max_output_bytes": request.max_output_bytes,
            "max_turns": request.max_turns,
        },
        "orchestrator": {"script": SCRIPT_NAME, "pid": os.getpid()},
        "error": None,
    }


def _git_workspace_snapshot(workspace: Path) -> dict[str, Any]:
    """Hash Git state and per-path contents without modifying the worktree."""

    state_commands = (
        ("git", "status", "--porcelain=v1", "-z", "--untracked-files=all"),
        ("git", "diff", "--binary", "--no-ext-diff"),
        ("git", "diff", "--cached", "--binary", "--no-ext-diff"),
    )
    path_commands = (
        ("git", "diff", "--name-only", "-z", "--no-ext-diff"),
        ("git", "diff", "--cached", "--name-only", "-z", "--no-ext-diff"),
        ("git", "ls-files", "--others", "--exclude-standard", "-z"),
    )
    outputs: list[bytes] = []
    for command in (*state_commands, *path_commands):
        completed = subprocess.run(
            command,
            cwd=workspace,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
            timeout=30,
        )
        if completed.returncode != 0:
            detail = _first_non_empty_line(completed.stderr.decode("utf-8", "replace"))
            raise _OrchestrationInputError(
                f"--require-change needs a Git workspace: {detail or 'git command failed'}"
            )
        outputs.append(completed.stdout)

    digest = hashlib.sha256()
    for output in outputs:
        digest.update(len(output).to_bytes(8, "big"))
        digest.update(output)
    raw_paths = {
        raw_path
        for output in outputs[len(state_commands) :]
        for raw_path in output.split(b"\0")
        if raw_path
    }
    files: dict[str, str] = {}
    for raw_path in sorted(raw_paths):
        path = os.fsdecode(raw_path)
        candidate = workspace / path
        try:
            content = candidate.read_bytes() if candidate.exists() else b"<deleted>"
        except OSError as exc:
            raise _OrchestrationInputError(
                f"could not fingerprint untracked file {candidate}: {exc}"
            ) from exc
        mode = candidate.lstat().st_mode if candidate.exists() else 0
        file_digest = hashlib.sha256(mode.to_bytes(8, "big") + content).hexdigest()
        files[path] = file_digest
        digest.update(raw_path)
        digest.update(file_digest.encode("ascii"))
    return {"sha256": digest.hexdigest(), "files": files}


def _parse_receipt(response: str) -> dict[str, Any] | None:
    """Extract the schema-constrained final receipt from Grok's JSON envelope."""

    try:
        envelope = json.loads(response)
    except (TypeError, json.JSONDecodeError):
        return None
    payload: Any = None
    if isinstance(envelope, dict):
        payload = envelope.get("structuredOutput")
        if payload is None:
            payload = envelope.get("text")
    if isinstance(payload, str):
        try:
            payload = json.loads(payload)
        except json.JSONDecodeError:
            return None
    if not isinstance(payload, dict):
        return None
    if payload.get("outcome") not in {"COMPLETED", "BLOCKED", "NO_CHANGE_REQUIRED"}:
        return None
    for key in ("changed_files", "checks", "remaining"):
        value = payload.get(key)
        if not isinstance(value, list) or not all(isinstance(item, str) for item in value):
            return None
    return payload


def run_request(
    request: RunRequest,
    *,
    env: Mapping[str, str] | None = None,
    out: TextIO | None = None,
    err: TextIO | None = None,
    clock: Callable[[], float] = time.monotonic,
) -> int:
    """Validate, run one Grok headless task, and write the JSON result."""

    parent_env = dict(os.environ if env is None else env)
    stdout = sys.stdout if out is None else out
    stderr = sys.stderr if err is None else err

    started_at = _utc_now()
    started = clock()
    run_id = uuid.uuid4().hex
    result = _base_result(request, run_id, started_at)

    def emit(
        outcome: str,
        exit_code: int,
        *,
        child_exit: int | None = None,
        signal_number: int | None = None,
        error: str | None = None,
        timed_out: bool = False,
        overflow: Mapping[str, Any] | None = None,
    ) -> int:
        result["outcome"] = outcome
        result["ok"] = outcome == OUTCOME_COMPLETED
        result["exit_code"] = child_exit
        result["signal"] = signal_number
        result["timed_out"] = timed_out
        if overflow is not None:
            result["output_overflow"] = dict(overflow)
        result["error"] = error
        result["finished_at"] = _utc_now()
        result["duration_seconds"] = round(max(0.0, clock() - started), 3)
        write_failure = _write_result_atomically(request.result_file, result)
        if write_failure is not None:
            print(
                f"{SCRIPT_NAME}: result file {request.result_file} was not written: "
                f"{write_failure}",
                file=stderr,
                flush=True,
            )
            return EXIT_RESULT_WRITE_FAILED
        print(str(request.result_file), file=stdout, flush=True)
        return exit_code

    try:
        task_meta = _load_task(request)
    except _OrchestrationInputError as exc:
        result["task"].update(exc.meta)
        return emit(OUTCOME_INVALID_TASK, EXIT_USAGE, error=str(exc))
    result["task"].update(task_meta)

    try:
        grok_path = _resolve_grok(request.grok)
    except _OrchestrationInputError as exc:
        return emit(OUTCOME_GROK_NOT_FOUND, EXIT_SPAWN_FAILED, error=str(exc))
    result["grok"]["path"] = str(grok_path)
    argv = build_argv(grok_path, request)
    result["grok"]["argv"] = display_argv(argv)

    if request.dry_run:
        plan = {
            "dry_run": True,
            "grok": {
                "path": str(grok_path),
                "argv": result["grok"]["argv"],
            },
            "settings": result["settings"],
            "task": result["task"],
            "workspace": result["workspace"],
            "result_file": str(request.result_file),
            "limits": result["limits"],
        }
        print(json.dumps(plan, indent=2, sort_keys=True), file=stdout, flush=True)
        return EXIT_OK

    before_snapshot: dict[str, Any] | None = None
    if request.require_change:
        try:
            before_snapshot = _git_workspace_snapshot(request.workspace)
        except _OrchestrationInputError as exc:
            return emit(OUTCOME_INVALID_TASK, EXIT_USAGE, error=str(exc))
        result["workspace_change"]["before_sha256"] = before_snapshot["sha256"]

    try:
        process = subprocess.Popen(
            argv,
            cwd=str(request.workspace),
            env=parent_env,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            start_new_session=True,
            close_fds=True,
        )
    except OSError as exc:
        return emit(
            OUTCOME_SPAWN_FAILED,
            EXIT_SPAWN_FAILED,
            error=f"could not start {grok_path} in {request.workspace}: {exc}",
        )

    captures = {
        "stdout": _StreamCapture(process.stdout, request.max_output_bytes),
        "stderr": _StreamCapture(process.stderr, request.max_output_bytes),
    }
    for capture in captures.values():
        capture.start()

    timed_out = False
    try:
        returncode = process.wait(timeout=request.timeout_seconds)
    except subprocess.TimeoutExpired:
        timed_out = True
        _terminate_process_group(process, request.kill_grace_seconds)
        returncode = process.returncode

    join_timeout = max(_MIN_STREAM_JOIN_SECONDS, request.kill_grace_seconds + 2.0)
    for capture in captures.values():
        capture.join(timeout=join_timeout)
    drained = all(capture.finished for capture in captures.values())
    if drained:
        for stream in (process.stdout, process.stderr):
            if stream is not None:
                try:
                    stream.close()
                except OSError:
                    pass

    stdout_capture = captures["stdout"]
    stderr_capture = captures["stderr"]
    result["response"] = stdout_capture.text()
    result["diagnostics"] = stderr_capture.text()
    result["stream_drain_complete"] = drained
    overflow = {
        "stdout_truncated": stdout_capture.truncated,
        "stderr_truncated": stderr_capture.truncated,
        "stdout_bytes": stdout_capture.total_bytes,
        "stderr_bytes": stderr_capture.total_bytes,
    }

    child_exit = returncode if returncode is not None and returncode >= 0 else None
    signal_number = -returncode if returncode is not None and returncode < 0 else None

    if request.require_change:
        try:
            after_snapshot = _git_workspace_snapshot(request.workspace)
        except _OrchestrationInputError as exc:
            return emit(
                OUTCOME_UNVERIFIED_COMPLETION,
                EXIT_CHILD_FAILED,
                child_exit=child_exit,
                signal_number=signal_number,
                error=str(exc),
                overflow=overflow,
            )
        result["workspace_change"]["after_sha256"] = after_snapshot["sha256"]
        result["workspace_change"]["observed"] = (
            after_snapshot["sha256"] != before_snapshot["sha256"]
        )
        before_files = before_snapshot["files"]
        after_files = after_snapshot["files"]
        result["workspace_change"]["actual_files"] = sorted(
            path
            for path in set(before_files) | set(after_files)
            if before_files.get(path) != after_files.get(path)
        )

    result["receipt"] = _parse_receipt(result["response"])

    if timed_out:
        return emit(
            OUTCOME_TIMED_OUT,
            EXIT_TIMED_OUT,
            child_exit=child_exit,
            signal_number=signal_number,
            error=(
                f"grok did not finish within --timeout-seconds={request.timeout_seconds}"
            ),
            timed_out=True,
            overflow=overflow,
        )
    if child_exit == 0 and request.require_change:
        receipt = result["receipt"]
        if not result["workspace_change"]["observed"]:
            return emit(
                OUTCOME_UNVERIFIED_COMPLETION,
                EXIT_CHILD_FAILED,
                child_exit=0,
                error="implementation run exited successfully but made no workspace change",
                overflow=overflow,
            )
        if receipt is None or receipt["outcome"] != "COMPLETED":
            return emit(
                OUTCOME_UNVERIFIED_COMPLETION,
                EXIT_CHILD_FAILED,
                child_exit=0,
                error="implementation run lacks a valid COMPLETED receipt",
                overflow=overflow,
            )
        if not receipt["checks"]:
            return emit(
                OUTCOME_UNVERIFIED_COMPLETION,
                EXIT_CHILD_FAILED,
                child_exit=0,
                error="COMPLETED receipt contains no focused check result",
                overflow=overflow,
            )
        claimed_files = receipt["changed_files"]
        actual_files = set(result["workspace_change"]["actual_files"])
        if not claimed_files or any(
            Path(path).is_absolute()
            or ".." in Path(path).parts
            or Path(path).as_posix().removeprefix("./") not in actual_files
            for path in claimed_files
        ):
            return emit(
                OUTCOME_UNVERIFIED_COMPLETION,
                EXIT_CHILD_FAILED,
                child_exit=0,
                error="COMPLETED receipt names files that did not actually change",
                overflow=overflow,
            )
    if child_exit == 0:
        return emit(OUTCOME_COMPLETED, EXIT_OK, child_exit=0, overflow=overflow)

    summary = _first_non_empty_line(result["diagnostics"])
    detail = (
        f"grok exited with {child_exit}"
        if child_exit is not None
        else "grok was terminated by a signal"
    )
    if summary:
        detail = f"{detail}: {summary}"
    max_turns_reached = "max turns reached" in result["diagnostics"].lower()
    return emit(
        OUTCOME_MAX_TURNS_REACHED if max_turns_reached else OUTCOME_CHILD_FAILED,
        child_exit if child_exit is not None else EXIT_CHILD_FAILED,
        child_exit=child_exit,
        signal_number=signal_number,
        error=detail,
        overflow=overflow,
    )


_EPILOG = """\
examples:
  python3 scripts/run_grok_task.py --task-file agent/tasks/T-001.md \\
      --result-file artifacts/grok-runs/T-001.json
  python3 scripts/run_grok_task.py --task-file task.md --timeout-seconds 900
  python3 scripts/run_grok_task.py --task-file task.md --dry-run

Grok implements the task; Codex orchestrates, reviews, tests, and accepts it.
"""


def _positive_int(value: str) -> int:
    try:
        number = int(value)
    except ValueError as exc:
        raise argparse.ArgumentTypeError(f"expected an integer, got {value!r}") from exc
    if number <= 0:
        raise argparse.ArgumentTypeError("expected a positive integer")
    return number


def _positive_float(value: str) -> float:
    try:
        number = float(value)
    except ValueError as exc:
        raise argparse.ArgumentTypeError(f"expected a number, got {value!r}") from exc
    if not math.isfinite(number) or number <= 0:
        raise argparse.ArgumentTypeError("expected a positive finite number")
    return number


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="run_grok_task.py",
        description=(
            "Run one bounded Grok 4.7 headless task from a UTF-8 task file and "
            "write a machine-readable JSON result."
        ),
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=_EPILOG,
    )
    parser.add_argument(
        "--task-file",
        required=True,
        type=Path,
        metavar="PATH",
        help="UTF-8 task text handed to the worker via --prompt-file (required)",
    )
    parser.add_argument(
        "--result-file",
        type=Path,
        metavar="PATH",
        help=f"JSON result path (default: <workspace>/{RUN_DIRECTORY_REL}/grok-<utc>-<id>.json)",
    )
    parser.add_argument(
        "--workspace",
        type=Path,
        metavar="PATH",
        help="worker working directory and sandbox workspace root (default: repository root)",
    )
    parser.add_argument(
        "--grok",
        default=DEFAULT_GROK,
        metavar="PATH|NAME",
        help="Grok CLI to invoke (default: grok from PATH)",
    )
    parser.add_argument(
        "--timeout-seconds",
        type=_positive_float,
        default=DEFAULT_TIMEOUT_SECONDS,
        metavar="SECONDS",
        help=f"whole-run timeout (default: {DEFAULT_TIMEOUT_SECONDS:g})",
    )
    parser.add_argument(
        "--max-task-bytes",
        type=_positive_int,
        default=DEFAULT_MAX_TASK_BYTES,
        metavar="BYTES",
        help=f"reject larger task files (default: {DEFAULT_MAX_TASK_BYTES})",
    )
    parser.add_argument(
        "--max-output-bytes",
        type=_positive_int,
        default=DEFAULT_MAX_OUTPUT_BYTES,
        metavar="BYTES",
        help=f"retain at most this many bytes per stream (default: {DEFAULT_MAX_OUTPUT_BYTES})",
    )
    parser.add_argument(
        "--max-turns",
        type=_positive_int,
        default=DEFAULT_MAX_TURNS,
        metavar="N",
        help=f"Grok --max-turns bound (default: {DEFAULT_MAX_TURNS})",
    )
    parser.add_argument(
        "--reasoning-effort",
        choices=REASONING_EFFORTS,
        default=DEFAULT_REASONING_EFFORT,
        help=f"Grok reasoning effort (default: {DEFAULT_REASONING_EFFORT})",
    )
    parser.add_argument(
        "--sandbox",
        choices=SANDBOXES,
        default=DEFAULT_SANDBOX,
        help=f"Grok sandbox profile (default: {DEFAULT_SANDBOX})",
    )
    parser.add_argument(
        "--require-change",
        action="store_true",
        help=(
            "for implementation tasks, require a real Git workspace change and a valid "
            "COMPLETED receipt"
        ),
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="validate and print the planned invocation without running or writing a result",
    )
    return parser


def _request_from_args(args: argparse.Namespace) -> RunRequest:
    workspace = Path(os.path.abspath(Path(args.workspace or REPOSITORY_ROOT).expanduser()))
    if not workspace.is_dir():
        raise _OrchestrationInputError(f"--workspace is not a directory: {workspace}")
    task_file = Path(os.path.abspath(Path(args.task_file).expanduser()))
    if args.result_file is None:
        stamp = _dt.datetime.now(_dt.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
        result_file = workspace / RUN_DIRECTORY_REL / f"grok-{stamp}-{uuid.uuid4().hex[:8]}.json"
    else:
        result_file = Path(os.path.abspath(Path(args.result_file).expanduser()))
    return RunRequest(
        task_file=task_file,
        result_file=result_file,
        workspace=workspace,
        grok=args.grok,
        timeout_seconds=args.timeout_seconds,
        max_task_bytes=args.max_task_bytes,
        max_output_bytes=args.max_output_bytes,
        max_turns=args.max_turns,
        reasoning_effort=args.reasoning_effort,
        sandbox=args.sandbox,
        require_change=args.require_change,
        dry_run=args.dry_run,
    )


def main(argv: Sequence[str] | None = None) -> int:
    parser = build_parser()
    try:
        args = parser.parse_args(list(argv) if argv is not None else None)
    except SystemExit as exc:  # argparse help/usage both surface as SystemExit
        return exc.code if isinstance(exc.code, int) else EXIT_USAGE
    try:
        request = _request_from_args(args)
    except _OrchestrationInputError as exc:
        parser.print_usage(sys.stderr)
        print(f"{SCRIPT_NAME}: {exc}", file=sys.stderr)
        return EXIT_USAGE
    return run_request(request)


if __name__ == "__main__":  # pragma: no cover - process entry point
    raise SystemExit(main())
