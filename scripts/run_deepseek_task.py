#!/usr/bin/env python3
"""Repository-local, non-interactive orchestration entrypoint for the DeepSeek worker.

Division of responsibility
--------------------------
DeepSeek (``deepseek-v4.1-flash`` under the ``headless`` dsh profile) implements
whatever the task file asks for. Codex orchestrates, reviews, tests, and accepts
the result. This script is the orchestration half only: it never edits product
code, never commits, and never records an acceptance decision.

What it does
------------
1. reads one UTF-8 task file (bounded, hashed, NUL-free);
2. invokes ``dsh --profile headless --patch <pinned overlay> -- <task>`` from the
   workspace root (the repository root by default) with
   ``DSH_PERMISSION_MODE=workspace-write`` and stdin closed, so the run is
   unattended and the file fence is pinned;
3. captures the final response (stdout) and the diagnostic reasoning/error text
   (stderr) separately, each bounded by ``--max-output-bytes``;
4. writes one machine-readable JSON result atomically (mode 0600) and prints its
   absolute path as the only line on stdout;
5. propagates failure truthfully and serializes runs with a workspace lock.

Usage
-----
Run a task and capture the result::

    python3 scripts/run_deepseek_task.py \\
        --task-file agent/tasks/T-001.md \\
        --result-file artifacts/deepseek-runs/T-001.json

Common variations::

    python3 scripts/run_deepseek_task.py --task-file task.md --timeout-seconds 900
    python3 scripts/run_deepseek_task.py --task-file task.md --result-file out.json
    python3 scripts/run_deepseek_task.py --task-file task.md --dry-run

Exit codes
----------
``0`` completed, ``1`` the worker failed (its own exit code), ``2`` usage or
invalid local input, ``3`` dsh missing or unstartable, ``4`` timeout, ``5``
output budget exceeded with ``--on-output-overflow abort``, ``6`` run lock
unavailable, ``7`` the result JSON could not be written. The JSON ``outcome``
field is authoritative whenever a worker exit code lands in ``2..7``.

Usage errors raised while building the request (bad flag values, a missing
workspace) print to stderr and write no JSON; every other outcome, including a
missing/unreadable task file or overlay, still writes a JSON result.

See ``docs/maintainers/agent/deepseek-orchestrator.md`` for the complete contract.
"""

from __future__ import annotations

import argparse
import datetime as _dt
import hashlib
import json
import math
import os
import re
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

try:  # POSIX advisory locking is the concurrency primitive; see _RunLock.
    import fcntl as _fcntl
except ImportError:  # pragma: no cover - non-POSIX host
    _fcntl = None  # type: ignore[assignment]


# --------------------------------------------------------------------------- #
# Fixed contract
# --------------------------------------------------------------------------- #

SCRIPT_NAME = "scripts/run_deepseek_task.py"
SCHEMA_VERSION = 1
PROFILE_NAME = "headless"
PERMISSION_MODE = "workspace-write"
DEFAULT_OVERLAY_REPO_REL = "agent/deepseek-v4.1-flash.patch.yml"
DEFAULT_EXPECTED_MODEL = "deepseek-v4.1-flash"
DEFAULT_EXPECTED_PROVIDER = "opencode-go"
DEFAULT_TIMEOUT_SECONDS = 3600.0
DEFAULT_KILL_GRACE_SECONDS = 10.0
DEFAULT_MAX_TASK_BYTES = 256 * 1024
DEFAULT_MAX_OUTPUT_BYTES = 8 * 1024 * 1024
DEFAULT_LOCK_WAIT_SECONDS = 0.0
RUN_DIRECTORY_REL = "artifacts/deepseek-runs"
LOCK_FILE_REL = f"{RUN_DIRECTORY_REL}/orchestrator.lock"

OVERFLOW_TRUNCATE = "truncate"
OVERFLOW_ABORT = "abort"

OUTCOME_COMPLETED = "completed"
OUTCOME_CHILD_FAILED = "child_failed"
OUTCOME_TIMED_OUT = "timed_out"
OUTCOME_OUTPUT_OVERFLOW = "output_overflow"
OUTCOME_SPAWN_FAILED = "spawn_failed"
OUTCOME_DSH_NOT_FOUND = "dsh_not_found"
OUTCOME_INVALID_TASK = "invalid_task_file"
OUTCOME_INVALID_OVERLAY = "invalid_overlay"
OUTCOME_LOCK_HELD = "lock_held"
OUTCOME_LOCK_ERROR = "lock_error"
OUTCOME_INTERRUPTED = "interrupted"

EXIT_OK = 0
EXIT_DSH_FAILED = 1
EXIT_USAGE = 2
EXIT_SPAWN_FAILED = 3
EXIT_TIMED_OUT = 4
EXIT_OUTPUT_OVERFLOW = 5
EXIT_LOCK_UNAVAILABLE = 6
EXIT_RESULT_WRITE_FAILED = 7
EXIT_INTERRUPTED = 130

EXIT_CODE_MEANINGS = {
    EXIT_OK: "completed",
    EXIT_DSH_FAILED: "worker failed",
    EXIT_USAGE: "usage or invalid local input",
    EXIT_SPAWN_FAILED: "dsh missing or unstartable",
    EXIT_TIMED_OUT: "timeout",
    EXIT_OUTPUT_OVERFLOW: "output budget exceeded",
    EXIT_LOCK_UNAVAILABLE: "run lock unavailable",
    EXIT_RESULT_WRITE_FAILED: "result JSON could not be written",
    EXIT_INTERRUPTED: "run interrupted by SIGINT/SIGTERM",
}

REPOSITORY_ROOT = Path(__file__).resolve().parents[1]

_SENSITIVE_ENV_NAME = re.compile(
    r"(?i)(api[_-]?key|access[_-]?key|secret|token|password|passwd|credential)"
)
_MIN_SECRET_CHARS = 8
_READ_CHUNK_BYTES = 65536
_LOCK_POLL_SECONDS = 0.25
_MIN_STREAM_JOIN_SECONDS = 5.0
_MAX_ERROR_LINE_CHARS = 300


# --------------------------------------------------------------------------- #
# Request model
# --------------------------------------------------------------------------- #


@dataclass(frozen=True)
class RunRequest:
    """One fully resolved orchestration request (no user input left to parse)."""

    task_file: Path
    result_file: Path
    workspace: Path
    overlay: Path
    dsh: str
    lock_file: Path
    timeout_seconds: float = DEFAULT_TIMEOUT_SECONDS
    kill_grace_seconds: float = DEFAULT_KILL_GRACE_SECONDS
    max_task_bytes: int = DEFAULT_MAX_TASK_BYTES
    max_output_bytes: int = DEFAULT_MAX_OUTPUT_BYTES
    on_output_overflow: str = OVERFLOW_TRUNCATE
    lock_wait_seconds: float = DEFAULT_LOCK_WAIT_SECONDS
    expected_model: str = DEFAULT_EXPECTED_MODEL
    expected_provider: str = DEFAULT_EXPECTED_PROVIDER
    dry_run: bool = False


class _OrchestrationInputError(Exception):
    """A local precondition failed; ``meta`` carries whatever was already known."""

    def __init__(self, message: str, meta: Mapping[str, Any] | None = None) -> None:
        super().__init__(message)
        self.meta: dict[str, Any] = dict(meta or {})


# --------------------------------------------------------------------------- #
# Small helpers
# --------------------------------------------------------------------------- #


def _utc_now() -> str:
    return (
        _dt.datetime.now(_dt.timezone.utc)
        .isoformat(timespec="milliseconds")
        .replace("+00:00", "Z")
    )


def _display_path(path: Path, workspace: Path) -> str:
    try:
        return path.relative_to(workspace).as_posix()
    except ValueError:
        return str(path)


def _secret_values(env: Mapping[str, str]) -> dict[str, str]:
    secrets: dict[str, str] = {}
    for name, value in env.items():
        if not isinstance(value, str) or len(value) < _MIN_SECRET_CHARS:
            continue
        if _SENSITIVE_ENV_NAME.search(name):
            secrets[name] = value
    return secrets


def redact_secrets(text: str, env: Mapping[str, str]) -> str:
    """Replace any inherited credential value with a named placeholder.

    Longer values are replaced first so a secret that contains another secret
    cannot leave a partial value behind. Values shorter than eight characters
    are ignored: they are far more likely to be ordinary words than credentials,
    and redacting them would corrupt unrelated output.
    """

    if not text:
        return text
    for name, value in sorted(_secret_values(env).items(), key=lambda item: -len(item[1])):
        text = text.replace(value, f"[REDACTED:{name}]")
    return text


def _first_non_empty_line(text: str, limit: int = _MAX_ERROR_LINE_CHARS) -> str:
    for line in text.splitlines():
        stripped = line.strip()
        if stripped:
            return stripped[:limit]
    return ""


# --------------------------------------------------------------------------- #
# Bounded stream capture
# --------------------------------------------------------------------------- #


class _StreamCapture(threading.Thread):
    """Drain one child pipe completely while retaining at most ``limit`` bytes."""

    def __init__(
        self,
        stream: Any,
        limit: int,
        on_overflow: Callable[[], None] | None = None,
    ) -> None:
        super().__init__(name="dsh-stream-capture", daemon=True)
        self._stream = stream
        self._limit = limit
        self._on_overflow = on_overflow
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
                        self._note_overflow()
                else:
                    self._note_overflow()
        except Exception as exc:  # pragma: no cover - defensive
            self.error = f"{type(exc).__name__}: {exc}"
        finally:
            self._finished.set()

    def _note_overflow(self) -> None:
        first = not self.truncated
        self.truncated = True
        if first and self._on_overflow is not None:
            try:
                self._on_overflow()
            except Exception:  # pragma: no cover - defensive
                pass

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


def _install_sigterm_handler() -> Callable[[], None] | None:
    """Turn SIGTERM into KeyboardInterrupt so the worker group is reaped first.

    Only the main thread may install a handler; anywhere else this is a no-op and
    the caller keeps the platform's default SIGTERM behaviour.
    """

    if threading.current_thread() is not threading.main_thread():
        return None
    try:
        previous = signal.getsignal(signal.SIGTERM)
    except (ValueError, OSError):  # pragma: no cover - defensive
        return None

    def handle(signum: int, frame: Any) -> None:
        raise KeyboardInterrupt

    try:
        signal.signal(signal.SIGTERM, handle)
    except (ValueError, OSError):  # pragma: no cover - defensive
        return None

    def restore() -> None:
        try:
            signal.signal(signal.SIGTERM, previous)
        except (ValueError, OSError):  # pragma: no cover - defensive
            pass

    return restore


# --------------------------------------------------------------------------- #
# Workspace-wide run lock
# --------------------------------------------------------------------------- #


class _RunLock:
    """POSIX advisory lock: one DeepSeek run may mutate the workspace at a time."""

    def __init__(self, path: Path) -> None:
        self.path = path
        self._fd: int | None = None

    def acquire(
        self,
        wait_seconds: float = 0.0,
        *,
        clock: Callable[[], float] = time.monotonic,
        sleep: Callable[[float], None] = time.sleep,
    ) -> bool:
        if _fcntl is None:
            raise OSError("POSIX advisory locking (fcntl) is unavailable on this platform")
        self.path.parent.mkdir(parents=True, exist_ok=True)
        fd = os.open(str(self.path), os.O_CREAT | os.O_RDWR, 0o600)
        try:
            deadline = clock() + max(0.0, wait_seconds)
            while True:
                try:
                    _fcntl.flock(fd, _fcntl.LOCK_EX | _fcntl.LOCK_NB)
                except OSError:
                    if clock() >= deadline:
                        return False
                    sleep(_LOCK_POLL_SECONDS)
                    continue
                self._fd = fd
                os.ftruncate(fd, 0)
                os.write(fd, f"pid={os.getpid()} acquired={_utc_now()}\n".encode("utf-8"))
                return True
        finally:
            if self._fd is None:
                os.close(fd)

    def release(self) -> None:
        if self._fd is None:
            return
        try:
            _fcntl.flock(self._fd, _fcntl.LOCK_UN)
        except OSError:
            pass
        try:
            os.close(self._fd)
        except OSError:
            pass
        self._fd = None


# --------------------------------------------------------------------------- #
# Validation
# --------------------------------------------------------------------------- #


def _load_task(request: RunRequest) -> tuple[str, dict[str, Any]]:
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
            "task file contains NUL bytes and cannot be passed as an argument", meta
        )
    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError as exc:
        raise _OrchestrationInputError(f"task file is not valid UTF-8: {exc}", meta) from exc
    if not text.strip():
        raise _OrchestrationInputError(
            "task file is empty or contains only whitespace", meta
        )
    return text, meta


def _load_overlay(request: RunRequest) -> dict[str, Any]:
    path = request.overlay
    if not path.is_file():
        raise _OrchestrationInputError(f"pinned overlay is missing: {path}")
    try:
        raw = path.read_bytes()
    except OSError as exc:
        raise _OrchestrationInputError(f"pinned overlay is not readable: {path}: {exc}") from exc
    if not raw.strip():
        raise _OrchestrationInputError(f"pinned overlay is empty: {path}")
    text = raw.decode("utf-8", "replace")
    for label, token in (
        ("model", request.expected_model),
        ("provider", request.expected_provider),
    ):
        if token and token not in text:
            raise _OrchestrationInputError(
                f"pinned overlay {path} does not mention the expected {label} "
                f"{token!r}; refusing to run a task with an unexpected worker configuration"
            )
    return {
        "path": _display_path(path, request.workspace),
        "sha256": hashlib.sha256(raw).hexdigest(),
    }


def _resolve_dsh(name: str) -> Path:
    if not name:
        raise _OrchestrationInputError("--dsh must not be empty")
    if os.sep in name or (os.altsep is not None and os.altsep in name):
        candidate = Path(name)
    else:
        found = shutil.which(name)
        if found is None:
            raise _OrchestrationInputError(f"dsh executable {name!r} was not found on PATH")
        candidate = Path(found)
    if not candidate.exists():
        raise _OrchestrationInputError(f"dsh executable does not exist: {candidate}")
    if not candidate.is_file():
        raise _OrchestrationInputError(f"dsh executable is not a regular file: {candidate}")
    if not os.access(candidate, os.X_OK):
        raise _OrchestrationInputError(f"dsh executable is not runnable: {candidate}")
    # Deliberately no symlink resolution: the launcher may be a symlink to a Node
    # entry point that must run through its shim, not be executed directly.
    return Path(os.path.abspath(candidate))


# --------------------------------------------------------------------------- #
# Atomic JSON result
# --------------------------------------------------------------------------- #


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
            json.dump(payload, handle, indent=2, sort_keys=True, ensure_ascii=False, default=str)
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


# --------------------------------------------------------------------------- #
# Result assembly
# --------------------------------------------------------------------------- #


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
        "diagnostics": "",
        "stream_drain_complete": None,
        "output_overflow": {
            "policy": request.on_output_overflow,
            "stdout_truncated": False,
            "stderr_truncated": False,
            "stdout_bytes": 0,
            "stderr_bytes": 0,
        },
        "task": {"file": str(request.task_file), "bytes": None, "sha256": None},
        "overlay": {
            "path": _display_path(request.overlay, request.workspace),
            "sha256": None,
        },
        "dsh": {
            "requested": request.dsh,
            "path": None,
            "profile": PROFILE_NAME,
            "permission_mode": PERMISSION_MODE,
            "argv": None,
        },
        "workspace": str(request.workspace),
        "limits": {
            "timeout_seconds": request.timeout_seconds,
            "kill_grace_seconds": request.kill_grace_seconds,
            "max_task_bytes": request.max_task_bytes,
            "max_output_bytes": request.max_output_bytes,
            "on_output_overflow": request.on_output_overflow,
        },
        "orchestrator": {"script": SCRIPT_NAME, "pid": os.getpid()},
        "error": None,
    }


def _display_argv(dsh_path: Path, request: RunRequest, task_meta: Mapping[str, Any]) -> list[str]:
    """The exact argv shape, with the task text elided so the result stays safe."""

    task_bytes = task_meta.get("bytes")
    digest = str(task_meta.get("sha256") or "")[:12]
    return [
        str(dsh_path),
        "--profile",
        PROFILE_NAME,
        "--patch",
        str(request.overlay),
        "--",
        f"<task:{task_bytes} bytes sha256:{digest}>",
    ]


# --------------------------------------------------------------------------- #
# Orchestration
# --------------------------------------------------------------------------- #


def run_request(
    request: RunRequest,
    *,
    env: Mapping[str, str] | None = None,
    out: TextIO | None = None,
    err: TextIO | None = None,
    clock: Callable[[], float] = time.monotonic,
) -> int:
    """Validate, run one dsh headless task, and write the JSON result.

    Returns the process exit code from the contract documented in the module
    docstring. ``env`` defaults to the current environment; it is passed to the
    worker with ``DSH_PERMISSION_MODE`` pinned and is never written to the result.
    """

    parent_env = dict(os.environ if env is None else env)
    stdout = sys.stdout if out is None else out
    stderr = sys.stderr if err is None else err

    started_at = _utc_now()
    started = clock()
    run_id = uuid.uuid4().hex
    result = _base_result(request, run_id, started_at)
    lock: _RunLock | None = None

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
        result["error"] = redact_secrets(error, parent_env) if error else None
        result["finished_at"] = _utc_now()
        result["duration_seconds"] = round(max(0.0, clock() - started), 3)
        write_failure = _write_result_atomically(request.result_file, result)
        if write_failure is not None:
            print(
                f"{SCRIPT_NAME}: result file {request.result_file} was not written: {write_failure}",
                file=stderr,
                flush=True,
            )
            return EXIT_RESULT_WRITE_FAILED
        print(str(request.result_file), file=stdout, flush=True)
        return exit_code

    try:
        task_text, task_meta = _load_task(request)
    except _OrchestrationInputError as exc:
        result["task"].update(exc.meta)
        return emit(OUTCOME_INVALID_TASK, EXIT_USAGE, error=str(exc))
    result["task"].update(task_meta)

    try:
        result["overlay"].update(_load_overlay(request))
    except _OrchestrationInputError as exc:
        return emit(OUTCOME_INVALID_OVERLAY, EXIT_USAGE, error=str(exc))

    try:
        dsh_path = _resolve_dsh(request.dsh)
    except _OrchestrationInputError as exc:
        return emit(OUTCOME_DSH_NOT_FOUND, EXIT_SPAWN_FAILED, error=str(exc))
    result["dsh"]["path"] = str(dsh_path)
    # The `--` separator keeps a task whose text looks like an option (for example
    # a file that starts with `--help` or `---`) a positional for dsh's launcher.
    argv = [
        str(dsh_path),
        "--profile",
        PROFILE_NAME,
        "--patch",
        str(request.overlay),
        "--",
        task_text,
    ]
    result["dsh"]["argv"] = _display_argv(dsh_path, request, task_meta)

    if request.dry_run:
        plan = {
            "dry_run": True,
            "dsh": {"path": str(dsh_path), "profile": PROFILE_NAME, "argv": result["dsh"]["argv"]},
            "overlay": result["overlay"],
            "task": result["task"],
            "workspace": result["workspace"],
            "permission_mode": PERMISSION_MODE,
            "result_file": str(request.result_file),
            "lock_file": str(request.lock_file),
            "limits": result["limits"],
        }
        print(json.dumps(plan, indent=2, sort_keys=True), file=stdout, flush=True)
        return EXIT_OK

    lock = _RunLock(request.lock_file)
    try:
        acquired = lock.acquire(wait_seconds=request.lock_wait_seconds, clock=clock)
    except OSError as exc:
        lock = None
        return emit(
            OUTCOME_LOCK_ERROR,
            EXIT_LOCK_UNAVAILABLE,
            error=f"could not use run lock {request.lock_file}: {exc}",
        )
    if not acquired:
        lock = None
        return emit(
            OUTCOME_LOCK_HELD,
            EXIT_LOCK_UNAVAILABLE,
            error=(
                f"another DeepSeek run already holds {request.lock_file}; "
                "retry after it finishes or pass --lock-wait-seconds"
            ),
        )

    restore_sigterm: Callable[[], None] | None = None
    try:
        restore_sigterm = _install_sigterm_handler()
        child_env = dict(parent_env)
        child_env["DSH_PERMISSION_MODE"] = PERMISSION_MODE
        result["dsh"]["permission_mode"] = PERMISSION_MODE

        try:
            process = subprocess.Popen(
                argv,
                cwd=str(request.workspace),
                env=child_env,
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
                error=f"could not start {dsh_path} in {request.workspace}: {exc}",
            )

        abort_on_overflow = request.on_output_overflow == OVERFLOW_ABORT
        overflow_triggered = threading.Event()

        def on_overflow() -> None:
            overflow_triggered.set()
            if abort_on_overflow:
                _terminate_process_group(process, request.kill_grace_seconds)

        captures = {
            "stdout": _StreamCapture(
                process.stdout, request.max_output_bytes, on_overflow if abort_on_overflow else None
            ),
            "stderr": _StreamCapture(
                process.stderr, request.max_output_bytes, on_overflow if abort_on_overflow else None
            ),
        }
        for capture in captures.values():
            capture.start()

        timed_out = False
        interrupted = False
        try:
            returncode = process.wait(timeout=request.timeout_seconds)
        except subprocess.TimeoutExpired:
            timed_out = True
            _terminate_process_group(process, request.kill_grace_seconds)
            returncode = process.returncode
        except KeyboardInterrupt:
            interrupted = True
            _terminate_process_group(process, request.kill_grace_seconds)
            returncode = process.returncode

        join_timeout = max(_MIN_STREAM_JOIN_SECONDS, request.kill_grace_seconds + 2.0)
        for capture in captures.values():
            capture.join(timeout=join_timeout)
        drained = all(capture.finished for capture in captures.values())
        if drained:
            # Every reader has exited, so closing the pipes cannot race a read.
            for stream in (process.stdout, process.stderr):
                if stream is not None:
                    try:
                        stream.close()
                    except OSError:
                        pass

        stdout_capture = captures["stdout"]
        stderr_capture = captures["stderr"]
        result["response"] = redact_secrets(stdout_capture.text(), parent_env)
        result["diagnostics"] = redact_secrets(stderr_capture.text(), parent_env)
        result["stream_drain_complete"] = drained
        overflow = {
            "policy": request.on_output_overflow,
            "stdout_truncated": stdout_capture.truncated,
            "stderr_truncated": stderr_capture.truncated,
            "stdout_bytes": stdout_capture.total_bytes,
            "stderr_bytes": stderr_capture.total_bytes,
        }

        child_exit = returncode if returncode is not None and returncode >= 0 else None
        signal_number = -returncode if returncode is not None and returncode < 0 else None

        if overflow_triggered.is_set():
            return emit(
                OUTCOME_OUTPUT_OVERFLOW,
                EXIT_OUTPUT_OVERFLOW,
                child_exit=child_exit,
                signal_number=signal_number,
                error=(
                    f"output exceeded --max-output-bytes={request.max_output_bytes}; "
                    "run aborted by --on-output-overflow=abort"
                ),
                overflow=overflow,
            )
        if timed_out:
            return emit(
                OUTCOME_TIMED_OUT,
                EXIT_TIMED_OUT,
                child_exit=child_exit,
                signal_number=signal_number,
                error=(
                    f"dsh did not finish within --timeout-seconds={request.timeout_seconds}"
                ),
                timed_out=True,
                overflow=overflow,
            )
        if interrupted:
            return emit(
                OUTCOME_INTERRUPTED,
                EXIT_INTERRUPTED,
                child_exit=child_exit,
                signal_number=signal_number,
                error=(
                    "orchestrator received SIGINT/SIGTERM; the worker process group "
                    "was terminated before releasing the run lock"
                ),
                overflow=overflow,
            )
        if child_exit == 0:
            return emit(OUTCOME_COMPLETED, EXIT_OK, child_exit=0, overflow=overflow)

        summary = _first_non_empty_line(result["diagnostics"])
        detail = f"dsh exited with {child_exit}" if child_exit is not None else "dsh was terminated by a signal"
        if summary:
            detail = f"{detail}: {summary}"
        return emit(
            OUTCOME_CHILD_FAILED,
            child_exit if child_exit is not None else EXIT_DSH_FAILED,
            child_exit=child_exit,
            signal_number=signal_number,
            error=detail,
            overflow=overflow,
        )
    finally:
        if restore_sigterm is not None:
            restore_sigterm()
        if lock is not None:
            lock.release()


# --------------------------------------------------------------------------- #
# Command line
# --------------------------------------------------------------------------- #

_EPILOG = """\
examples:
  python3 scripts/run_deepseek_task.py --task-file agent/tasks/T-001.md \\
      --result-file artifacts/deepseek-runs/T-001.json
  python3 scripts/run_deepseek_task.py --task-file task.md --timeout-seconds 900
  python3 scripts/run_deepseek_task.py --task-file task.md --dry-run

DeepSeek implements the task; Codex orchestrates, reviews, tests, and accepts it.
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


def _non_negative_float(value: str) -> float:
    try:
        number = float(value)
    except ValueError as exc:
        raise argparse.ArgumentTypeError(f"expected a number, got {value!r}") from exc
    if not math.isfinite(number) or number < 0:
        raise argparse.ArgumentTypeError("expected a non-negative finite number")
    return number


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="run_deepseek_task.py",
        description=(
            "Run one bounded DeepSeek (deepseek-v4.1-flash) headless task from a UTF-8 "
            "task file and write a machine-readable JSON result."
        ),
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=_EPILOG,
    )
    parser.add_argument(
        "--task-file",
        required=True,
        type=Path,
        metavar="PATH",
        help="UTF-8 task text handed to the worker (required)",
    )
    parser.add_argument(
        "--result-file",
        type=Path,
        metavar="PATH",
        help=f"JSON result path (default: <workspace>/{RUN_DIRECTORY_REL}/deepseek-<utc>-<id>.json)",
    )
    parser.add_argument(
        "--workspace",
        type=Path,
        metavar="PATH",
        help="worker working directory and sandbox workspace root (default: repository root)",
    )
    parser.add_argument(
        "--overlay",
        type=Path,
        metavar="PATH",
        help=f"pinned patch overlay applied after the profile (default: {DEFAULT_OVERLAY_REPO_REL})",
    )
    parser.add_argument(
        "--expect-model",
        default=DEFAULT_EXPECTED_MODEL,
        metavar="ID",
        help=f"model token the overlay must pin (default: {DEFAULT_EXPECTED_MODEL})",
    )
    parser.add_argument(
        "--expect-provider",
        default=DEFAULT_EXPECTED_PROVIDER,
        metavar="ID",
        help=f"provider token the overlay must pin (default: {DEFAULT_EXPECTED_PROVIDER})",
    )
    parser.add_argument(
        "--dsh",
        default="dsh",
        metavar="PATH|NAME",
        help="dsh launcher to invoke (default: dsh from PATH)",
    )
    parser.add_argument(
        "--timeout-seconds",
        type=_positive_float,
        default=DEFAULT_TIMEOUT_SECONDS,
        metavar="SECONDS",
        help=f"whole-run timeout (default: {DEFAULT_TIMEOUT_SECONDS:g})",
    )
    parser.add_argument(
        "--kill-grace-seconds",
        type=_non_negative_float,
        default=DEFAULT_KILL_GRACE_SECONDS,
        metavar="SECONDS",
        help=f"SIGTERM grace before SIGKILL on timeout/overflow (default: {DEFAULT_KILL_GRACE_SECONDS:g})",
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
        "--on-output-overflow",
        choices=(OVERFLOW_TRUNCATE, OVERFLOW_ABORT),
        default=OVERFLOW_TRUNCATE,
        help=(
            "when a stream exceeds --max-output-bytes: keep draining and flag "
            "truncation (truncate, default), or terminate the run and fail (abort)"
        ),
    )
    parser.add_argument(
        "--lock-file",
        type=Path,
        metavar="PATH",
        help=f"run lock path (default: <workspace>/{LOCK_FILE_REL})",
    )
    parser.add_argument(
        "--lock-wait-seconds",
        type=_non_negative_float,
        default=DEFAULT_LOCK_WAIT_SECONDS,
        metavar="SECONDS",
        help="wait this long for the run lock before failing (default: 0, fail immediately)",
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
    overlay_source = args.overlay if args.overlay is not None else REPOSITORY_ROOT / DEFAULT_OVERLAY_REPO_REL
    overlay = Path(os.path.abspath(Path(overlay_source).expanduser()))
    task_file = Path(os.path.abspath(Path(args.task_file).expanduser()))
    if args.result_file is None:
        stamp = _dt.datetime.now(_dt.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
        result_file = workspace / RUN_DIRECTORY_REL / f"deepseek-{stamp}-{uuid.uuid4().hex[:8]}.json"
    else:
        result_file = Path(os.path.abspath(Path(args.result_file).expanduser()))
    if args.lock_file is None:
        lock_file = workspace / LOCK_FILE_REL
    else:
        lock_file = Path(os.path.abspath(Path(args.lock_file).expanduser()))
    return RunRequest(
        task_file=task_file,
        result_file=result_file,
        workspace=workspace,
        overlay=overlay,
        dsh=args.dsh,
        lock_file=lock_file,
        timeout_seconds=args.timeout_seconds,
        kill_grace_seconds=args.kill_grace_seconds,
        max_task_bytes=args.max_task_bytes,
        max_output_bytes=args.max_output_bytes,
        on_output_overflow=args.on_output_overflow,
        lock_wait_seconds=args.lock_wait_seconds,
        expected_model=args.expect_model,
        expected_provider=args.expect_provider,
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
