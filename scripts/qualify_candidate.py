#!/usr/bin/env python3
"""Qualify one exact Narrivy candidate package in a disposable container.

The entrypoint accepts an explicit absolute candidate ``.deb`` path and its
expected SHA-256, rejects mismatched inputs before any container work, and then
exercises one fresh test-owned Ubuntu 24.04 Docker container: full apt
installation with declared dependencies, fixed installed daemon/runtime/model
paths, empty-cache ``awaiting_model`` startup, explicit offline provisioning
through ``scripts/qualify_installed_model.py``, ready model/worker status and
the independently queried enabled configured pipeline,
bounded SIGTERM shutdown with socket removal, restart, package removal/purge,
and exact model preservation. It writes one sanitized, non-overwriting result
JSON carrying the package hash.

The result is deliberately unqualified: it is not schema-v2 requirement
evidence and it does not qualify native hardware, a desktop session, or release
provenance. Only the container created by this run is removed; nothing is
installed, started, or removed on the host.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import re
import selectors
import stat
import subprocess
import sys
import time
import uuid
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Any, Callable, Mapping, Protocol, Sequence

REPOSITORY_ROOT = Path(__file__).resolve().parents[1]
QUALIFY_MODEL_SCRIPT = REPOSITORY_ROOT / "scripts" / "qualify_installed_model.py"
PROBE_STATUS_SCRIPT = REPOSITORY_ROOT / "scripts" / "probe_installed_status.py"

DEFAULT_IMAGE = "ubuntu:24.04"
IMAGE_RE = re.compile(r"ubuntu:24\.04(?:@sha256:[0-9a-f]{64})?")
SHA256_RE = re.compile(r"[0-9a-f]{64}")
PID_RE = re.compile(r"[1-9][0-9]{0,6}")
MAX_CANDIDATE_BYTES = 512 * 1024 * 1024

PACKAGE_NAME = "narrivy"
CONTAINER_ROOT = "/qualification"
CANDIDATE_IN_CONTAINER = f"{CONTAINER_ROOT}/candidate.deb"
PYTHON = "python3"

# Registry-pinned bundled model. Must match
# crates/narrivy-model-manager/seed/registry.json and the packaged model.
MODEL_ID = "whisper-tiny.en-Q8_0"
MODEL_FAMILY = "whisper"
MODEL_SHA256 = "e8c9b73c06344307d8b346e07fbe93dd88d894627854bcff31523f1ce44394fa"
MODEL_SIZE = 45_904_544
INSTALLED_MODEL = f"/usr/share/narrivy/models/{MODEL_ID}.gguf"
# Installed cache layout is the registry layout, never a flat file name:
# models/<family>/<id>/<sha256-prefix>/model.gguf
MODEL_CACHE_RELATIVE = (
    f"narrivy/models/{MODEL_FAMILY}/{MODEL_ID}/{MODEL_SHA256[:12]}/model.gguf"
)

RUNTIME_DIR = f"{CONTAINER_ROOT}/runtime"
STATE_DIR = f"{CONTAINER_ROOT}/state"
CACHE_DIR = f"{CONTAINER_ROOT}/cache"
DATA_DIR = f"{CONTAINER_ROOT}/data"
HOME_DIR = f"{CONTAINER_ROOT}/home"
SOCKET_PATH = f"{RUNTIME_DIR}/narrivyd.sock"
PID_FILE = f"{CONTAINER_ROOT}/narrivyd.pid"
EXIT_FILE = f"{CONTAINER_ROOT}/narrivyd.exit"
LOG_FILE = f"{CONTAINER_ROOT}/narrivyd.log"
# Present only when systemd is actually running as this container's init. The
# lifecycle stops the daemon as a plain process; it never claims a systemd
# user-service stop qualification.
SYSTEMD_CONTROL_DIR = "/run/systemd/system"
CACHE_MODEL_PATH = f"{CACHE_DIR}/{MODEL_CACHE_RELATIVE}"

INSTALLED_PATHS = (
    "/usr/bin/narrivyd",
    "/usr/bin/narrivy-cli",
    "/usr/bin/narrivy-desktop",
    "/usr/lib/narrivy/narrivy-transcribe-worker",
    "/usr/share/narrivy/installed-runtime-manifest.json",
    INSTALLED_MODEL,
    "/usr/lib/systemd/user/narrivy-daemon.service",
    "/usr/share/applications/narrivy.desktop",
)
EXECUTABLE_PATHS = (
    "/usr/bin/narrivyd",
    "/usr/bin/narrivy-cli",
    "/usr/bin/narrivy-desktop",
    "/usr/lib/narrivy/narrivy-transcribe-worker",
)

# Fixed in-container environment mirroring the packaged user unit. The
# systemd `%S`/`%C` specifiers are resolved here to test-owned directories.
DAEMON_ENV = {
    "HOME": HOME_DIR,
    "XDG_RUNTIME_DIR": RUNTIME_DIR,
    "XDG_STATE_HOME": STATE_DIR,
    "XDG_CACHE_HOME": CACHE_DIR,
    "XDG_DATA_HOME": DATA_DIR,
    "NARRIVY_SOCKET": SOCKET_PATH,
    "NARRIVY_RECOVERY_MODE": "linux-mic-inbox",
    "NARRIVY_DEFAULT_PIPELINE": "clean",
    "NARRIVY_ASR_ENGINE": "transcribe-cpp",
    "NARRIVY_AUDIO_SOURCE": "pipewire",
    "NARRIVY_DELIVERY_BACKEND": "inbox",
    "NARRIVY_DELIVERY_FAIL": "0",
    "NARRIVY_TRANSCRIBE_RUNTIME": "installed",
    "NARRIVY_TRANSCRIBE_MODEL_ID": MODEL_ID,
}
APT_ENV = {"DEBIAN_FRONTEND": "noninteractive"}

DIRECTORY_SCRIPT = (
    "set -eu\n"
    f"mkdir -p {CONTAINER_ROOT}\n"
    f"mkdir -p {RUNTIME_DIR} {STATE_DIR} {CACHE_DIR} {DATA_DIR} {HOME_DIR}\n"
    f"chmod 700 {CONTAINER_ROOT} {RUNTIME_DIR} {STATE_DIR} {CACHE_DIR} {DATA_DIR} {HOME_DIR}\n"
)
DAEMON_START_SCRIPT = (
    "set -u\n"
    f"rm -f {PID_FILE} {EXIT_FILE}\n"
    f"/usr/bin/narrivyd >{LOG_FILE} 2>&1 &\n"
    "pid=$!\n"
    f"printf '%s\\n' \"$pid\" >{PID_FILE}\n"
    'wait "$pid"\n'
    f"printf '%s\\n' \"$?\" >{EXIT_FILE}\n"
)
EMPTY_CACHE_SCRIPT = (
    "set -eu\n"
    f"if [ -e {CACHE_DIR}/narrivy/models ]; then\n"
    f"  [ -z \"$(ls -A {CACHE_DIR}/narrivy/models)\" ]"
    " || { echo cache_not_empty; exit 1; }\n"
    "fi\n"
)
MANIFEST_WORKER_READ = [
    PYTHON,
    "-c",
    "import json;print(json.load(open("
    "'/usr/share/narrivy/installed-runtime-manifest.json'))['worker']['sha256'])",
]


def _path_script(paths: Sequence[str], *, present: bool) -> str:
    lines = ["set -eu"]
    for path in paths:
        if present:
            lines.append(f'test -f {path} || {{ echo "missing={path}"; exit 1; }}')
        else:
            lines.append(f'test ! -e {path} || {{ echo "present={path}"; exit 1; }}')
    return "\n".join(lines) + "\n"


def _installed_paths_script() -> str:
    script = _path_script(INSTALLED_PATHS, present=True)
    executable = "\n".join(
        f'test -x {path} || {{ echo "not_executable={path}"; exit 1; }}'
        for path in EXECUTABLE_PATHS
    )
    return script + executable + "\n"


INSTALLED_PATHS_SCRIPT = _installed_paths_script()
REMOVED_PATHS_SCRIPT = _path_script(INSTALLED_PATHS, present=False)

_DETAIL_RE = re.compile(r"[^A-Za-z0-9_ .:,=/#+-]")
POLL_SECONDS = 0.25


def _sanitize_detail(value: object, limit: int = 200) -> str:
    text = value if isinstance(value, str) else str(value)
    return _DETAIL_RE.sub("?", text)[:limit]


def _scalar(value: object) -> object:
    if value is None or isinstance(value, (bool, int, float)):
        return value
    if isinstance(value, str):
        return _sanitize_detail(value)
    return None


class InputError(RuntimeError):
    """Rejected input; no container or other execution occurred."""


class DockerError(RuntimeError):
    """Docker interaction failed or was refused."""


class StepFailure(RuntimeError):
    """One named qualification step did not reach its required outcome."""

    def __init__(
        self,
        step: str,
        detail: str,
        diagnosis: Mapping[str, object] | None = None,
    ) -> None:
        super().__init__(f"{step}: {detail}")
        self.step = step
        self.detail = _sanitize_detail(detail)
        self.diagnosis = dict(diagnosis or {})


@dataclass(frozen=True)
class CommandResult:
    returncode: int
    stdout: bytes
    stderr: bytes


@dataclass(frozen=True)
class Limits:
    """Bounded budgets in seconds for the whole run and each step family."""

    total: float = 2400.0
    image: float = 60.0
    pull: float = 600.0
    create: float = 120.0
    copy: float = 300.0
    start: float = 60.0
    command: float = 120.0
    apt_update: float = 600.0
    install: float = 1500.0
    tooling: float = 300.0
    startup: float = 90.0
    provision: float = 300.0
    shutdown: float = 30.0
    remove: float = 120.0


DEFAULT_LIMITS = Limits()


class Runner(Protocol):
    """Minimal Docker surface used by the qualification flow."""

    def ensure_image(
        self, image: str, *, inspect_timeout: float, pull_timeout: float
    ) -> None: ...

    def create(
        self,
        image: str,
        name: str,
        labels: Sequence[tuple[str, str]],
        *,
        lifetime_seconds: int,
        timeout: float,
    ) -> str: ...

    def copy(
        self, container: str, source: Path, destination: str, *, timeout: float
    ) -> None: ...

    def start(self, container: str, *, timeout: float) -> None: ...

    def exec(
        self,
        container: str,
        argv: Sequence[str],
        *,
        timeout: float,
        env: Mapping[str, str] | None = None,
        detached: bool = False,
    ) -> CommandResult: ...

    def remove(
        self,
        container: str,
        expected_candidate_sha256: str,
        *,
        timeout: float,
    ) -> None: ...


class DockerRunner:
    """Runs docker argv vectors without a shell and with bounded capture.

    Output is streamed into fixed-size buffers and discarded beyond
    ``max_capture_bytes`` per stream, so a runaway command cannot consume
    unbounded memory or temporary disk.
    """

    def __init__(self, docker: str = "docker", max_capture_bytes: int = 65_536) -> None:
        self._docker = docker
        self._max_capture = max_capture_bytes

    @staticmethod
    def _terminate(process: subprocess.Popen[bytes]) -> None:
        try:
            process.kill()
        except OSError:
            pass
        try:
            process.wait(timeout=5)
        except (subprocess.TimeoutExpired, OSError):
            pass

    def _run(self, arguments: Sequence[str], timeout: float) -> CommandResult:
        if not math.isfinite(timeout) or timeout <= 0:
            raise DockerError("docker command time bound was invalid")
        deadline = time.monotonic() + timeout
        process = subprocess.Popen(
            [self._docker, *arguments],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            close_fds=True,
        )
        captured = [bytearray(), bytearray()]
        streams = ((process.stdout, 0), (process.stderr, 1))
        selector = selectors.DefaultSelector()
        try:
            for stream, index in streams:
                if stream is None:
                    raise DockerError("docker process streams were unavailable")
                os.set_blocking(stream.fileno(), False)
                selector.register(stream, selectors.EVENT_READ, index)
            while selector.get_map():
                wait_for = deadline - time.monotonic()
                if wait_for <= 0:
                    raise DockerError("docker command exceeded its time bound")
                for key, _events in selector.select(wait_for):
                    stream = key.fileobj
                    index = key.data
                    try:
                        chunk = os.read(stream.fileno(), 65_536)
                    except BlockingIOError:
                        continue
                    if not chunk:
                        selector.unregister(stream)
                        continue
                    room = self._max_capture - len(captured[index])
                    if room > 0:
                        captured[index] += chunk[:room]
            returncode = process.wait(timeout=max(0.0, deadline - time.monotonic()))
        except DockerError:
            self._terminate(process)
            raise
        except (subprocess.TimeoutExpired, OSError):
            self._terminate(process)
            raise DockerError("docker command exceeded its time bound") from None
        finally:
            selector.close()
            for stream, _index in streams:
                if stream is not None:
                    stream.close()
        return CommandResult(returncode, bytes(captured[0]), bytes(captured[1]))

    def ensure_image(
        self, image: str, *, inspect_timeout: float, pull_timeout: float
    ) -> None:
        if self._run(["image", "inspect", image], inspect_timeout).returncode == 0:
            return
        if self._run(["pull", image], pull_timeout).returncode != 0:
            raise DockerError("container image is unavailable")
        if self._run(["image", "inspect", image], inspect_timeout).returncode != 0:
            raise DockerError("container image is unavailable")

    def create(
        self,
        image: str,
        name: str,
        labels: Sequence[tuple[str, str]],
        *,
        lifetime_seconds: int,
        timeout: float,
    ) -> str:
        arguments = ["create", "--rm", "--pull=never", "--name", name]
        for key, value in labels:
            arguments += ["--label", f"{key}={value}"]
        arguments += [image, "timeout", str(lifetime_seconds), "sleep", "infinity"]
        outcome = self._run(arguments, timeout)
        container = outcome.stdout.decode("ascii", "strict").strip()
        if outcome.returncode != 0 or not SHA256_RE.fullmatch(container):
            raise DockerError("container creation failed")
        return container

    def copy(
        self, container: str, source: Path, destination: str, *, timeout: float
    ) -> None:
        outcome = self._run(["cp", str(source), f"{container}:{destination}"], timeout)
        if outcome.returncode != 0:
            raise DockerError("container copy failed")

    def start(self, container: str, *, timeout: float) -> None:
        if self._run(["start", container], timeout).returncode != 0:
            raise DockerError("container start failed")

    def exec(
        self,
        container: str,
        argv: Sequence[str],
        *,
        timeout: float,
        env: Mapping[str, str] | None = None,
        detached: bool = False,
    ) -> CommandResult:
        arguments = ["exec"]
        if detached:
            arguments.append("-d")
        for key, value in (env or {}).items():
            arguments += ["-e", f"{key}={value}"]
        arguments += [container, *argv]
        return self._run(arguments, timeout)

    def remove(
        self,
        container: str,
        expected_candidate_sha256: str,
        *,
        timeout: float,
    ) -> None:
        inspected = self._run(
            [
                "inspect",
                "--format",
                '{{.Id}}|{{index .Config.Labels "narrivy.qualification"}}|'
                '{{index .Config.Labels "narrivy.candidate.sha256"}}',
                container,
            ],
            timeout,
        )
        if inspected.returncode != 0:
            return
        fields = inspected.stdout.decode("ascii", "strict").strip().split("|")
        if (
            len(fields) != 3
            or fields[0] != container
            or fields[1] != "1"
            or fields[2] != expected_candidate_sha256
        ):
            raise DockerError("refusing to remove a container not created by this run")
        if self._run(["rm", "-f", container], timeout).returncode != 0:
            raise DockerError("container removal failed")


def _hash_file(path: Path) -> tuple[str, int]:
    digest = hashlib.sha256()
    size = 0
    with open(path, "rb", buffering=0) as handle:
        while True:
            chunk = handle.read(1024 * 1024)
            if not chunk:
                break
            size += len(chunk)
            if size > MAX_CANDIDATE_BYTES:
                raise InputError("candidate size is invalid")
            digest.update(chunk)
    return digest.hexdigest(), size


def validate_inputs(
    candidate: Path, expected_sha256: str, result_path: Path
) -> tuple[str, int]:
    """Reject mismatched or unsafe inputs before any container work."""

    if not SHA256_RE.fullmatch(expected_sha256):
        raise InputError("expected SHA-256 must be 64 lowercase hex characters")
    if not candidate.is_absolute():
        raise InputError("candidate path must be absolute")
    if not result_path.is_absolute():
        raise InputError("result path must be absolute")
    try:
        metadata = candidate.lstat()
    except OSError as error:
        raise InputError("candidate is unavailable") from error
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        raise InputError("candidate must be a regular non-symlink file")
    if metadata.st_size <= 0 or metadata.st_size > MAX_CANDIDATE_BYTES:
        raise InputError("candidate size is invalid")
    digest, size = _hash_file(candidate)
    if digest != expected_sha256:
        raise InputError("candidate SHA-256 does not match the expected value")
    if os.path.lexists(result_path):
        raise InputError("result path already exists")
    if not result_path.parent.is_dir():
        raise InputError("result parent directory does not exist")
    for script in (QUALIFY_MODEL_SCRIPT, PROBE_STATUS_SCRIPT):
        if not script.is_file():
            raise InputError("qualification support script is unavailable")
    return digest, size


def _validate_limits(limits: Limits) -> None:
    """Reject NaN, infinite, or non-positive time budgets before any work."""

    for name in Limits.__dataclass_fields__:
        value = getattr(limits, name)
        if (
            isinstance(value, bool)
            or not isinstance(value, (int, float))
            or not math.isfinite(value)
            or value <= 0
        ):
            raise InputError("qualification time budgets must be positive finite seconds")


def _parse_object(output: bytes, step: str) -> dict[str, Any]:
    try:
        value = json.loads(output.decode("utf-8", "strict").strip())
    except (UnicodeDecodeError, json.JSONDecodeError):
        raise StepFailure(step, "typed JSON outcome was unavailable") from None
    if not isinstance(value, dict):
        raise StepFailure(step, "typed JSON outcome was not an object")
    return value


def _digest_of(output: bytes, step: str) -> str:
    token = output.decode("ascii", "strict").strip().split(" ")[0]
    if not SHA256_RE.fullmatch(token):
        raise StepFailure(step, "digest output was not a SHA-256")
    return token


def _exit_detail(outcome: CommandResult) -> str:
    message = outcome.stdout.decode("utf-8", "replace").strip()
    return _sanitize_detail(message) if message else f"exit_status={outcome.returncode}"


DIAGNOSIS_PREFIX = "qualification_diagnosis="
DIAGNOSIS_FACT_LIMIT = 16
DIAGNOSIS_VALUE_CHARS = 64
DIAGNOSIS_TOKEN_RE = re.compile(r"[A-Za-z0-9_.:+-]{1,64}")
DIAGNOSIS_FACT_KEYS = frozenset({
    "installed_model", "worker", "worker_ready", "worker_artifact",
    "model_artifact", "provisioning_state", "provisioning_publication",
    "provisioning_cleanup", "provisioning_failure",
    "pipeline", "pipeline_enabled",
})


def _helper_diagnosis(outcome: CommandResult) -> dict[str, Any]:
    """Read the helper's bounded sanitized typed failure summary.

    The helper emits one ``qualification_diagnosis=`` JSON line whose values are
    closed-vocabulary lifecycle tokens. Nothing else from its output is read,
    so a generic ``exit_status=1`` can never hide the typed cause.
    """

    stream = outcome.stderr + b"\n" + outcome.stdout
    for raw in reversed(stream.splitlines()):
        line = raw.decode("utf-8", "replace").strip()
        if not line.startswith(DIAGNOSIS_PREFIX):
            continue
        try:
            value = json.loads(line[len(DIAGNOSIS_PREFIX) :])
        except json.JSONDecodeError:
            return {}
        if not isinstance(value, dict) or value.get("schema_version") != 1:
            return {}
        reason = value.get("reason")
        facts = value.get("facts")
        if (
            not isinstance(reason, str)
            or DIAGNOSIS_TOKEN_RE.fullmatch(reason) is None
            or not isinstance(facts, dict)
        ):
            return {}
        summary = {
            "helper_reason": _sanitize_detail(reason, DIAGNOSIS_VALUE_CHARS),
            "helper_facts": {},
        }
        for index, key in enumerate(sorted(DIAGNOSIS_FACT_KEYS.intersection(facts))):
            if index >= DIAGNOSIS_FACT_LIMIT or not isinstance(key, str):
                break
            item = facts[key]
            if key in ("worker_ready", "pipeline_enabled"):
                if isinstance(item, bool):
                    summary["helper_facts"][key] = item
            elif isinstance(item, str) and DIAGNOSIS_TOKEN_RE.fullmatch(item):
                summary["helper_facts"][key] = item
        return summary
    return {}


def _helper_failure(step: str, outcome: CommandResult) -> StepFailure:
    diagnosis = _helper_diagnosis(outcome)
    parts = [f"helper_exit={outcome.returncode}"]
    reason = diagnosis.get("helper_reason")
    if isinstance(reason, str) and reason:
        parts.append(f"reason={reason}")
    facts = diagnosis.get("helper_facts")
    if isinstance(facts, dict):
        for key in (
            "installed_model",
            "worker",
            "provisioning_state",
            "provisioning_failure",
        ):
            value = facts.get(key)
            if isinstance(value, (str, bool, int)):
                parts.append(f"{key}={value}")
    return StepFailure(step, " ".join(parts), diagnosis)


def _base_result(
    file_name: str, digest: str, size: int, image: str
) -> dict[str, Any]:
    return {
        "schema_version": 1,
        "classification": "candidate-lifecycle-unqualified",
        "result": "failed",
        "failed_step": None,
        "failure_detail": None,
        "diagnosis": None,
        "candidate": {"file": file_name, "sha256": digest, "size_bytes": size},
        "container": {"image": image, "created": False, "os": None, "removed": False},
        "steps": [],
        "startup": None,
        "provisioning": None,
        "model_cache": None,
        "shutdown": None,
        "restart": None,
        "restart_stop": None,
        "removal": None,
        "unqualified": True,
        "requirement_evidence": False,
        "native_hardware_qualified": False,
    }


def _write_result(path: Path, payload: dict[str, Any]) -> None:
    data = (
        json.dumps(payload, sort_keys=True, separators=(",", ":"), ensure_ascii=True)
        + "\n"
    ).encode("ascii")
    descriptor = os.open(
        path,
        os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC | os.O_NOFOLLOW,
        0o644,
    )
    try:
        view = memoryview(data)
        while view:
            written = os.write(descriptor, view)
            if written <= 0:
                raise OSError("result write failed")
            view = view[written:]
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def qualify(
    candidate: Path,
    expected_sha256: str,
    result_path: Path,
    runner: Runner,
    *,
    image: str = DEFAULT_IMAGE,
    limits: Limits = DEFAULT_LIMITS,
    clock: Callable[[], float] = time.monotonic,
    sleep: Callable[[float], None] = time.sleep,
) -> dict[str, Any]:
    """Run the bounded lifecycle qualification and return its result object."""

    digest, size = validate_inputs(candidate, expected_sha256, result_path)
    _validate_limits(limits)
    deadline = clock() + limits.total
    result = _base_result(candidate.name, digest, size, image)
    steps: list[dict[str, str]] = []
    state: dict[str, str] = {"current": "validate"}
    container: str | None = None

    def remaining(
        step: str, budget: float, local_deadline: float | None = None
    ) -> float:
        """Smallest of the step budget, any local deadline, and the overall one."""

        if not math.isfinite(budget) or budget <= 0:
            raise StepFailure(step, "operation time budget was invalid")
        if local_deadline is not None:
            local_left = local_deadline - clock()
            if local_left <= 0:
                raise StepFailure(step, "operation exceeded its local time bound")
            budget = min(budget, local_left)
        overall_left = deadline - clock()
        if overall_left <= 0:
            raise StepFailure(step, "overall qualification deadline exceeded")
        return min(budget, overall_left)

    def begin(step: str) -> None:
        state["current"] = step

    def run_exec(
        step: str,
        argv: Sequence[str],
        budget: float,
        env: Mapping[str, str] | None = None,
        detached: bool = False,
        local_deadline: float | None = None,
    ) -> CommandResult:
        begin(step)
        if container is None:
            raise StepFailure(step, "container is unavailable")
        try:
            return runner.exec(
                container,
                argv,
                timeout=remaining(step, budget, local_deadline),
                env=env,
                detached=detached,
            )
        except DockerError as error:
            raise StepFailure(step, _sanitize_detail(error)) from None

    def run_ok(
        step: str,
        argv: Sequence[str],
        budget: float,
        env: Mapping[str, str] | None = None,
        detached: bool = False,
        local_deadline: float | None = None,
    ) -> CommandResult:
        outcome = run_exec(
            step, argv, budget, env=env, detached=detached, local_deadline=local_deadline
        )
        if outcome.returncode != 0:
            raise StepFailure(step, f"exit_status={outcome.returncode}")
        return outcome

    def passed(step: str) -> None:
        steps.append({"step": step, "status": "passed"})

    def pause(local_deadline: float) -> None:
        sleep(min(POLL_SECONDS, max(0.0, local_deadline - clock())))

    def cached_model_digest(step: str) -> str:
        outcome = run_ok(step, ["sha256sum", CACHE_MODEL_PATH], limits.command)
        return _digest_of(outcome.stdout, step)

    def final_status_facts() -> dict[str, Any]:
        """Capture one sanitized typed status snapshot before container removal.

        This runs only after a failure and only reads the probe's narrow,
        content-free summary. Raw daemon status never enters the result file.
        """

        if container is None:
            return {"probe": "container_unavailable"}
        try:
            outcome = runner.exec(
                container,
                [
                    PYTHON,
                    f"{CONTAINER_ROOT}/{PROBE_STATUS_SCRIPT.name}",
                    "--socket",
                    SOCKET_PATH,
                ],
                timeout=min(limits.command, 15.0),
            )
        except (DockerError, OSError):
            return {"probe": "unavailable"}
        if outcome.returncode != 0:
            return {"probe": "failed", "exit_status": outcome.returncode}
        try:
            summary = _parse_object(outcome.stdout, "diagnosis_probe")
        except StepFailure:
            return {"probe": "invalid"}
        snapshot: dict[str, Any] = {"probe": "ok"}
        for key in (
            "installed_model",
            "worker",
            "worker_ready",
            "worker_artifact",
            "model_artifact",
            "pipeline",
            "pipeline_enabled",
        ):
            value = summary.get(key)
            if value is None or isinstance(value, bool):
                snapshot[key] = value
            elif isinstance(value, str) and DIAGNOSIS_TOKEN_RE.fullmatch(value):
                snapshot[key] = value
        return snapshot

    def record_diagnosis(diagnosis: Mapping[str, object] | None) -> None:
        result["diagnosis"] = {
            "failure": dict(diagnosis or {}),
            "final_status": final_status_facts(),
        }

    def wait_for_daemon(step: str, expected_model: str, expect_worker: bool) -> dict[str, Any]:
        local_deadline = clock() + remaining(step, limits.startup)
        while True:
            if clock() >= local_deadline:
                raise StepFailure(step, "daemon did not reach its required state within the bound")
            socket_probe = run_exec(
                step,
                ["test", "-S", SOCKET_PATH],
                limits.command,
                local_deadline=local_deadline,
            )
            if socket_probe.returncode == 0:
                status_probe = run_exec(
                    step,
                    [
                        PYTHON,
                        f"{CONTAINER_ROOT}/{PROBE_STATUS_SCRIPT.name}",
                        "--socket",
                        SOCKET_PATH,
                    ],
                    limits.command,
                    local_deadline=local_deadline,
                )
                if status_probe.returncode == 0:
                    summary = _parse_object(status_probe.stdout, step)
                    if summary.get("installed_model") == expected_model and (
                        not expect_worker or summary.get("worker_ready") is True
                    ) and summary.get("pipeline") == "clean" and (
                        summary.get("pipeline_enabled") is expect_worker
                    ):
                        return summary
            else:
                exited = run_exec(
                    step,
                    ["test", "-f", EXIT_FILE],
                    limits.command,
                    local_deadline=local_deadline,
                )
                if exited.returncode == 0:
                    code = run_exec(
                        step,
                        ["cat", EXIT_FILE],
                        limits.command,
                        local_deadline=local_deadline,
                    )
                    raise StepFailure(
                        step,
                        f"daemon_exited={code.stdout.decode('ascii', 'replace').strip()}",
                    )
            pause(local_deadline)

    def stop_daemon(step: str, budget: float) -> dict[str, Any]:
        """SIGTERM the running daemon and confirm a clean bounded exit."""

        started = clock()
        pid = run_ok(step, ["cat", PID_FILE], limits.command).stdout.decode(
            "ascii", "strict"
        ).strip()
        if not PID_RE.fullmatch(pid):
            raise StepFailure(step, "daemon pid was unavailable")
        local_deadline = clock() + remaining(step, budget)
        terminate = run_exec(
            step,
            ["/bin/sh", "-c", 'kill -TERM "$1"', "kill", pid],
            limits.command,
            local_deadline=local_deadline,
        )
        if terminate.returncode != 0:
            raise StepFailure(step, f"exit_status={terminate.returncode}")
        while True:
            if clock() >= local_deadline:
                raise StepFailure(step, "daemon did not exit within the bound")
            exited = run_exec(
                step,
                ["test", "-f", EXIT_FILE],
                limits.command,
                local_deadline=local_deadline,
            )
            if exited.returncode == 0:
                break
            pause(local_deadline)
        exit_code = run_exec(
            step, ["cat", EXIT_FILE], limits.command, local_deadline=local_deadline
        ).stdout.decode("ascii", "replace").strip()
        if exit_code != "0":
            raise StepFailure(step, f"daemon_exit_status={exit_code}")
        socket_gone = run_exec(
            step,
            ["test", "!", "-e", SOCKET_PATH],
            limits.command,
            local_deadline=local_deadline,
        )
        if socket_gone.returncode != 0:
            raise StepFailure(step, "socket was not removed")
        return {
            "exit_code": 0,
            "socket_removed": True,
            "bounded": True,
            "seconds": round(clock() - started, 3),
        }

    try:
        begin("image")
        try:
            runner.ensure_image(
                image,
                inspect_timeout=remaining("image", limits.image),
                pull_timeout=remaining("image", limits.pull),
            )
        except DockerError as error:
            raise StepFailure("image", _sanitize_detail(error)) from None
        passed("image")

        begin("container_create")
        try:
            container = runner.create(
                image,
                f"narrivy-qualify-{uuid.uuid4().hex[:12]}",
                (
                    ("narrivy.qualification", "1"),
                    ("narrivy.candidate.sha256", digest),
                    ("narrivy.run", uuid.uuid4().hex),
                ),
                lifetime_seconds=int(limits.total) + 600,
                timeout=remaining("container_create", limits.create),
            )
        except DockerError as error:
            raise StepFailure("container_create", _sanitize_detail(error)) from None
        result["container"]["created"] = True
        passed("container_create")

        begin("container_start")
        try:
            runner.start(container, timeout=remaining("container_start", limits.start))
        except DockerError as error:
            raise StepFailure("container_start", _sanitize_detail(error)) from None
        run_ok("container_start", ["sh", "-c", DIRECTORY_SCRIPT], limits.command)
        try:
            runner.copy(
                container,
                candidate,
                CANDIDATE_IN_CONTAINER,
                timeout=remaining("container_start", limits.copy),
            )
            for script in (QUALIFY_MODEL_SCRIPT, PROBE_STATUS_SCRIPT):
                runner.copy(
                    container,
                    script,
                    f"{CONTAINER_ROOT}/{script.name}",
                    timeout=remaining("container_start", limits.copy),
                )
        except DockerError as error:
            raise StepFailure("container_start", _sanitize_detail(error)) from None
        transferred = _digest_of(
            run_ok(
                "container_start",
                ["sha256sum", CANDIDATE_IN_CONTAINER],
                limits.command,
            ).stdout,
            "container_start",
        )
        if transferred != digest:
            raise StepFailure("container_start", "transferred candidate digest mismatch")
        passed("container_start")

        os_release = run_ok("container_os", ["cat", "/etc/os-release"], limits.command)
        os_text = os_release.stdout.decode("utf-8", "replace")
        if "ID=ubuntu" not in os_text or 'VERSION_ID="24.04"' not in os_text:
            raise StepFailure("container_os", "container is not ubuntu-24.04")
        result["container"]["os"] = "ubuntu-24.04"
        passed("container_os")

        run_ok("apt_update", ["apt-get", "update", "-qq"], limits.apt_update, APT_ENV)
        passed("apt_update")
        run_ok(
            "apt_install_package",
            ["apt-get", "install", "-y", "-qq", CANDIDATE_IN_CONTAINER],
            limits.install,
            APT_ENV,
        )
        passed("apt_install_package")
        run_ok(
            "apt_install_python3",
            ["apt-get", "install", "-y", "-qq", "--no-install-recommends", PYTHON],
            limits.tooling,
            APT_ENV,
        )
        passed("apt_install_python3")

        paths = run_exec(
            "installed_paths", ["sh", "-c", INSTALLED_PATHS_SCRIPT], limits.command
        )
        if paths.returncode != 0:
            raise StepFailure("installed_paths", _exit_detail(paths))
        passed("installed_paths")

        worker_digest = _digest_of(
            run_ok(
                "installed_runtime_binding",
                ["sha256sum", "/usr/lib/narrivy/narrivy-transcribe-worker"],
                limits.command,
            ).stdout,
            "installed_runtime_binding",
        )
        manifest_digest = _digest_of(
            run_ok(
                "installed_runtime_binding",
                MANIFEST_WORKER_READ,
                limits.command,
            ).stdout,
            "installed_runtime_binding",
        )
        if worker_digest != manifest_digest:
            raise StepFailure(
                "installed_runtime_binding", "manifest worker digest does not match"
            )
        passed("installed_runtime_binding")
        installed_model_digest = _digest_of(
            run_ok(
                "installed_model",
                ["sha256sum", INSTALLED_MODEL],
                limits.command,
            ).stdout,
            "installed_model",
        )
        if installed_model_digest != MODEL_SHA256:
            raise StepFailure("installed_model", "packaged model digest mismatch")
        installed_model_size = run_ok(
            "installed_model", ["stat", "-c", "%s", INSTALLED_MODEL], limits.command
        ).stdout.decode("ascii", "strict").strip()
        if installed_model_size != str(MODEL_SIZE):
            raise StepFailure("installed_model", "packaged model size mismatch")
        passed("installed_model")

        begin("daemon_start")
        run_ok(
            "daemon_start",
            ["/bin/sh", "-c", DAEMON_START_SCRIPT],
            limits.command,
            DAEMON_ENV,
            detached=True,
        )
        startup = wait_for_daemon("daemon_start", "awaiting_model", False)
        empty_cache = run_exec(
            "daemon_start", ["sh", "-c", EMPTY_CACHE_SCRIPT], limits.command
        )
        if empty_cache.returncode != 0:
            raise StepFailure("daemon_start", _exit_detail(empty_cache))
        result["startup"] = startup
        passed("daemon_start")

        provision_outcome = run_exec(
            "offline_provision",
            [
                PYTHON,
                f"{CONTAINER_ROOT}/{QUALIFY_MODEL_SCRIPT.name}",
                "--socket",
                SOCKET_PATH,
                "--model-source",
                INSTALLED_MODEL,
                "--timeout",
                "240",
            ],
            limits.provision,
            DAEMON_ENV,
        )
        if provision_outcome.returncode != 0:
            raise _helper_failure("offline_provision", provision_outcome)
        provisioning = _parse_object(provision_outcome.stdout, "offline_provision")
        if (
            provisioning.get("state") != "ready"
            or provisioning.get("installed_model") != "ready"
            or provisioning.get("worker_ready") is not True
            or provisioning.get("pipeline") != "clean"
            or provisioning.get("pipeline_enabled") is not True
        ):
            raise StepFailure(
                "offline_provision",
                "typed provisioning outcome mismatch",
                {"helper_facts": {"worker": _sanitize_detail(provisioning.get("worker"))}}
                if provisioning.get("worker") is not None
                else None,
            )
        result["provisioning"] = {
            key: _scalar(provisioning.get(key))
            for key in (
                "state",
                "installed_model",
                "worker",
                "worker_ready",
                "publication",
                "cleanup",
                "pipeline",
                "pipeline_enabled",
            )
        }
        passed("offline_provision")

        cache_digest = cached_model_digest("model_cache")
        if cache_digest != MODEL_SHA256:
            raise StepFailure("model_cache", "cached model digest mismatch")
        cache_size = run_ok(
            "model_cache", ["stat", "-c", "%s", CACHE_MODEL_PATH], limits.command
        ).stdout.decode("ascii", "strict").strip()
        if cache_size != str(MODEL_SIZE):
            raise StepFailure("model_cache", "cached model size mismatch")
        run_ok(
            "model_cache",
            ["test", "-f", f"{CACHE_MODEL_PATH.rsplit('/', 1)[0]}/meta.json"],
            limits.command,
        )
        result["model_cache"] = {
            "path": MODEL_CACHE_RELATIVE,
            "sha256": cache_digest,
            "size_bytes": int(cache_size),
            "preserved_after_remove": False,
            "preserved_after_purge": False,
        }
        passed("model_cache")

        result["shutdown"] = stop_daemon("sigterm_shutdown", limits.shutdown)
        passed("sigterm_shutdown")

        run_ok("restart_ready", ["rm", "-f", PID_FILE, EXIT_FILE], limits.command)
        run_ok(
            "restart_ready",
            ["/bin/sh", "-c", DAEMON_START_SCRIPT],
            limits.command,
            DAEMON_ENV,
            detached=True,
        )
        result["restart"] = wait_for_daemon("restart_ready", "ready", True)
        passed("restart_ready")

        result["restart_stop"] = stop_daemon("restart_stop", limits.shutdown)
        passed("restart_stop")

        begin("remove")
        systemd_init = run_exec(
            "remove", ["test", "!", "-d", SYSTEMD_CONTROL_DIR], limits.command
        )
        if systemd_init.returncode != 0:
            raise StepFailure(
                "remove",
                "container init is systemd; user-service stop is not container-qualified",
            )
        run_ok(
            "remove",
            ["apt-get", "remove", "-y", "-qq", PACKAGE_NAME],
            limits.install,
            APT_ENV,
        )
        removed = run_exec("remove", ["sh", "-c", REMOVED_PATHS_SCRIPT], limits.command)
        if removed.returncode != 0:
            raise StepFailure("remove", _exit_detail(removed))
        result["removal"] = {
            "daemon_stopped_before_removal": True,
            "user_service_stop_qualified": False,
            "systemd_init": False,
        }
        passed("remove")
        if cached_model_digest("remove_preserves_model") != MODEL_SHA256:
            raise StepFailure("remove_preserves_model", "cached model changed after removal")
        result["model_cache"]["preserved_after_remove"] = True
        passed("remove_preserves_model")

        run_ok(
            "purge",
            ["apt-get", "purge", "-y", "-qq", PACKAGE_NAME],
            limits.install,
            APT_ENV,
        )
        registered = run_exec("purge", ["dpkg-query", "-W", PACKAGE_NAME], limits.command)
        if registered.returncode == 0:
            raise StepFailure("purge", "package is still registered")
        purged = run_exec("purge", ["sh", "-c", REMOVED_PATHS_SCRIPT], limits.command)
        if purged.returncode != 0:
            raise StepFailure("purge", _exit_detail(purged))
        passed("purge")
        if cached_model_digest("purge_preserves_model") != MODEL_SHA256:
            raise StepFailure("purge_preserves_model", "cached model changed after purge")
        result["model_cache"]["preserved_after_purge"] = True
        passed("purge_preserves_model")

        result["result"] = "passed"
    except StepFailure as failure:
        result["result"] = "failed"
        result["failed_step"] = failure.step
        result["failure_detail"] = failure.detail
        if not any(entry["step"] == failure.step for entry in steps):
            steps.append({"step": failure.step, "status": "failed"})
        record_diagnosis(failure.diagnosis)
    except (DockerError, OSError, UnicodeDecodeError, ValueError) as error:
        step = state["current"]
        result["result"] = "failed"
        result["failed_step"] = step
        result["failure_detail"] = _sanitize_detail(error)
        if not any(entry["step"] == step for entry in steps):
            steps.append({"step": step, "status": "failed"})
        record_diagnosis(None)
    finally:
        if container is not None:
            begin("container_remove")
            try:
                runner.remove(
                    container,
                    digest,
                    timeout=limits.remove,
                )
            except (DockerError, OSError) as error:
                result["container"]["removed"] = False
                if result["result"] == "passed":
                    result["result"] = "failed"
                    result["failed_step"] = "container_remove"
                    result["failure_detail"] = _sanitize_detail(error)
                    steps.append({"step": "container_remove", "status": "failed"})
            else:
                result["container"]["removed"] = True
                if result["result"] == "passed":
                    steps.append({"step": "container_remove", "status": "passed"})
        result["steps"] = steps
        _write_result(result_path, result)
    return result


def _arguments(argv: list[str] | None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--candidate", required=True, type=Path)
    parser.add_argument("--expected-sha256", required=True)
    parser.add_argument("--result", required=True, type=Path)
    parser.add_argument("--image", default=DEFAULT_IMAGE)
    parser.add_argument("--timeout", type=float, default=DEFAULT_LIMITS.total)
    parser.add_argument("--docker", default="docker")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    values = _arguments(argv)
    try:
        if not IMAGE_RE.fullmatch(values.image):
            raise InputError("container image must be ubuntu:24.04")
        if not math.isfinite(values.timeout) or values.timeout <= 0:
            raise InputError("timeout must be positive and finite")
        result = qualify(
            values.candidate,
            values.expected_sha256,
            values.result,
            DockerRunner(docker=values.docker),
            image=values.image,
            limits=replace(DEFAULT_LIMITS, total=values.timeout),
        )
    except InputError as error:
        print(f"qualify_candidate: {error}", file=sys.stderr)
        return 2
    except (DockerError, OSError, ValueError) as error:
        print(f"qualify_candidate: {_sanitize_detail(error)}", file=sys.stderr)
        return 1
    if result["result"] == "passed":
        print(f"qualify_candidate: passed -> {values.result}")
        return 0
    print(
        f"qualify_candidate: failed step={result['failed_step']} -> {values.result}",
        file=sys.stderr,
    )
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
