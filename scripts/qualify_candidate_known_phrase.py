#!/usr/bin/env python3
"""Run one exact-candidate COSMIC physical-microphone known-phrase procedure.

The entrypoint accepts explicit absolute paths for an existing candidate
``.deb``, a PCM WAV fixture, and a new result JSON, plus expected SHA-256
values and ``--confirm-audible-playback``. Missing confirmation is rejected
before any process or audio interaction. The fixture is played once through
the current default speakers and captured through the current default
physical source. The packaged daemon, CLI, resident worker and GGUF model
are extracted into a private tree and never installed.

The JSON result is content-safe and explicitly unqualified. It is not
requirement evidence, release qualification, target-insertion proof,
accessibility proof, GNOME proof, or a performance result.
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import os
import platform
import re
import selectors
import signal
import socket
import stat
import subprocess
import sys
import tarfile
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Mapping, Protocol, Sequence

import recorder_safety

REPOSITORY_ROOT = Path(__file__).resolve().parents[1]
GIT = Path("/usr/bin/git")
PACTL = Path("/usr/bin/pactl")
PAPLAY = Path("/usr/bin/paplay")
PW_RECORD = Path("/usr/bin/pw-record")
REQUIRED_HELPERS = (GIT, PACTL, PAPLAY, PW_RECORD)

PACKAGE_NAME = "narrivy"
BUILD_INFO_MEMBER = "usr/share/narrivy/build-info.json"
CLEAN_CLASSIFICATION = "clean-unqualified-release-candidate"
RESULT_CLASSIFICATION = (
    "native-cosmic-physical-microphone-known-phrase-exact-candidate-unqualified"
)
EXPECTED_FRAGMENT = "ask not what your country"
CAPTURE_SECONDS = 20
MODEL_ID = "whisper-tiny.en-Q8_0"

DAEMON_MEMBER = "usr/bin/narrivyd"
CLI_MEMBER = "usr/bin/narrivy-cli"
WORKER_MEMBER = "usr/lib/narrivy/narrivy-transcribe-worker"
MODEL_MEMBER = f"usr/share/narrivy/models/{MODEL_ID}.gguf"
EXTRACT_MEMBERS = {
    DAEMON_MEMBER: "narrivyd",
    CLI_MEMBER: "narrivy-cli",
    WORKER_MEMBER: "resident-worker",
    MODEL_MEMBER: "bundled-model",
}
EMBEDDED_KEYS = (
    "narrivyd",
    "narrivy-cli",
    "narrivy-desktop",
    "resident-worker",
    "bundled-model",
)

AR_MAGIC = b"!<arch>\n"
AR_HEADER_SIZE = 60
MAX_CANDIDATE_BYTES = 512 * 1024 * 1024
MAX_FIXTURE_BYTES = 2 * 1024 * 1024
MAX_BUILD_INFO_BYTES = 64 * 1024
MAX_BINARY_BYTES = 256 * 1024 * 1024
MAX_MODEL_BYTES = 64 * 1024 * 1024
MAX_AR_MEMBERS = 8
MAX_TAR_MEMBERS = 4096
MAX_TAR_DECLARED_BYTES = MAX_CANDIDATE_BYTES
MAX_GIT_TEXT_BYTES = 4096
MAX_HELPER_OUTPUT_BYTES = 4096
MAX_LOG_BYTES = 1_048_576
MAX_TRANSCRIPT_BYTES = 65_536
MAX_SOURCE_NAME_BYTES = 256
MAX_SOURCE_DATE_EPOCH = 253_402_300_799
WAV_RATE = 16_000
WAV_CHANNELS = 1
WAV_BITS = 16
MIN_WAV_SECONDS = 1.0
MAX_WAV_SECONDS = 20.0
GIT_TIMEOUT_SECONDS = 5.0
POLL_SECONDS = 0.05
SHA256_RE = re.compile(r"[0-9a-f]{64}")
COMMIT_RE = re.compile(r"[0-9a-f]{40}")
_DETAIL_RE = re.compile(r"[^A-Za-z0-9_ .:,=/#+-]")
_UNSAFE_SOURCE_TOKENS = frozenset(
    {
        "dummy",
        "loopback",
        "monitor",
        "narrivy",
        "null",
        "qualification",
        "remap",
        "virtual",
    }
)
GIT_ENV = {
    "PATH": "/usr/bin:/bin",
    "LC_ALL": "C",
    "TZ": "UTC",
    "GIT_CONFIG_NOSYSTEM": "1",
    "GIT_CONFIG_GLOBAL": "/dev/null",
    "GIT_OPTIONAL_LOCKS": "0",
    "GIT_TERMINAL_PROMPT": "0",
}


class InputError(RuntimeError):
    """Rejected input; no extraction, daemon, or audio interaction occurred."""


class StepFailure(RuntimeError):
    """A named procedure step did not reach its required outcome."""

    def __init__(self, step: str, detail: str) -> None:
        super().__init__(f"{step}: {detail}")
        self.step = step
        self.detail = _sanitize_detail(detail)


@dataclass(frozen=True)
class CommandResult:
    returncode: int
    stdout: bytes
    stderr: bytes


@dataclass(frozen=True)
class Limits:
    total: float = 180.0
    git: float = GIT_TIMEOUT_SECONDS
    helper: float = 5.0
    socket: float = 15.0
    start: float = 10.0
    playback: float = 30.0
    stop: float = 10.0
    idle: float = 45.0
    shutdown: float = 10.0


DEFAULT_LIMITS = Limits()


@dataclass(frozen=True)
class Inputs:
    candidate: Path
    fixture: Path
    result: Path
    expected_candidate_sha256: str
    expected_fixture_sha256: str
    confirm_audible_playback: bool
    source_root: Path = REPOSITORY_ROOT


@dataclass(frozen=True)
class EmbeddedIdentity:
    package: str
    version: str
    source_head: str
    source_date_epoch: int
    embedded: dict[str, dict[str, object]]


class Spawned(Protocol):
    def poll(self) -> int | None: ...

    def kill_group(self) -> bool: ...

    def output(self) -> bytes: ...


class Operations(Protocol):
    def environ(self) -> Mapping[str, str]: ...

    def containerized(self) -> bool: ...

    def native_lane(self) -> Mapping[str, object]: ...

    def helper_ready(self, path: Path) -> bool: ...

    def monotonic(self) -> float: ...

    def sleep(self, seconds: float) -> None: ...

    def run(
        self,
        argv: Sequence[str],
        *,
        env: Mapping[str, str],
        timeout: float,
        cwd: Path | None = None,
    ) -> CommandResult: ...

    def spawn(
        self,
        argv: Sequence[str],
        *,
        env: Mapping[str, str],
        log_path: Path,
        cwd: Path,
    ) -> Spawned: ...


def _sanitize_detail(value: object, limit: int = 200) -> str:
    text = value if isinstance(value, str) else str(value)
    return _DETAIL_RE.sub("?", text)[:limit]


def _canonical_json(value: object) -> bytes:
    return (
        json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True)
        + "\n"
    ).encode("ascii")


def _digest_of(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def _require_sha256(value: str, label: str) -> str:
    if not SHA256_RE.fullmatch(value):
        raise InputError(f"{label} SHA-256 must be 64 lowercase hex characters")
    return value


def _validate_limits(limits: Limits) -> None:
    for name in Limits.__dataclass_fields__:
        value = getattr(limits, name)
        if (
            isinstance(value, bool)
            or not isinstance(value, (int, float))
            or value != value
            or value <= 0
        ):
            raise InputError("procedure time budgets must be positive finite seconds")


def _kill_process_group(
    process: subprocess.Popen[bytes], *, graceful_root_seconds: float = 0.0
) -> bool:
    if process.poll() is not None:
        return True
    if graceful_root_seconds > 0:
        try:
            process.terminate()
        except OSError:
            pass
        try:
            process.wait(timeout=graceful_root_seconds)
            return True
        except (subprocess.TimeoutExpired, OSError):
            pass
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except OSError:
        pass
    try:
        process.wait(timeout=2)
    except (subprocess.TimeoutExpired, OSError):
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except OSError:
            pass
        try:
            process.wait(timeout=2)
        except (subprocess.TimeoutExpired, OSError):
            return False
    return process.poll() is not None


def _bounded_communicate(
    process: subprocess.Popen[bytes],
    timeout: float,
    maximum: int,
) -> CommandResult:
    if timeout <= 0:
        _kill_process_group(process)
        raise StepFailure("process", "operation time budget was invalid")
    deadline = time.monotonic() + timeout
    captured = [bytearray(), bytearray()]
    streams = ((process.stdout, 0), (process.stderr, 1))
    selector = selectors.DefaultSelector()
    try:
        for stream, index in streams:
            if stream is None:
                raise StepFailure("process", "process streams were unavailable")
            os.set_blocking(stream.fileno(), False)
            selector.register(stream, selectors.EVENT_READ, index)
        while selector.get_map():
            wait_for = deadline - time.monotonic()
            if wait_for <= 0:
                raise TimeoutError
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
                room = maximum - len(captured[index])
                if room > 0:
                    captured[index] += chunk[:room]
        returncode = process.wait(timeout=max(0.0, deadline - time.monotonic()))
    except TimeoutError:
        _kill_process_group(process)
        raise StepFailure("process", "command exceeded its time bound") from None
    except (subprocess.TimeoutExpired, OSError):
        _kill_process_group(process)
        raise StepFailure("process", "command exceeded its time bound") from None
    finally:
        selector.close()
        for stream, _index in streams:
            if stream is not None:
                stream.close()
    return CommandResult(returncode, bytes(captured[0]), bytes(captured[1]))


class HostProcess:
    def __init__(self, process: subprocess.Popen[bytes], log_path: Path) -> None:
        self._process = process
        self._log_path = log_path

    def poll(self) -> int | None:
        return self._process.poll()

    def kill_group(self) -> bool:
        # narrivyd owns its worker processes and has a bounded SIGTERM shutdown
        # path. Signal the daemon alone first so it can reap those children and
        # report a truthful exit status. Kill the full group only on timeout.
        return _kill_process_group(
            self._process, graceful_root_seconds=DEFAULT_LIMITS.shutdown
        )

    def output(self) -> bytes:
        # A missing or unreadable log is a failure, never an empty (clean) log.
        try:
            observed = self._log_path.lstat()
        except OSError:
            raise StepFailure("log_hygiene", "daemon log is missing") from None
        if not stat.S_ISREG(observed.st_mode):
            raise StepFailure("log_hygiene", "daemon log is not a regular file")
        if observed.st_size > MAX_LOG_BYTES:
            raise StepFailure("log_hygiene", "daemon log exceeded the scan bound")
        try:
            return self._log_path.read_bytes()
        except OSError:
            raise StepFailure("log_hygiene", "daemon log is unreadable") from None


class HostOperations:
    def environ(self) -> Mapping[str, str]:
        return os.environ

    def containerized(self) -> bool:
        return Path("/.dockerenv").exists() or Path("/run/.containerenv").exists()

    def native_lane(self) -> Mapping[str, object]:
        return recorder_safety.native_lane(environ=self.environ())

    def helper_ready(self, path: Path) -> bool:
        return os.path.isfile(path) and os.access(path, os.X_OK)

    def monotonic(self) -> float:
        return time.monotonic()

    def sleep(self, seconds: float) -> None:
        time.sleep(seconds)

    def run(
        self,
        argv: Sequence[str],
        *,
        env: Mapping[str, str],
        timeout: float,
        cwd: Path | None = None,
    ) -> CommandResult:
        if not argv:
            raise StepFailure("process", "command argv was empty")
        try:
            process = subprocess.Popen(
                list(argv),
                stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                cwd=None if cwd is None else os.fspath(cwd),
                env=dict(env),
                start_new_session=True,
                close_fds=True,
            )
        except OSError as error:
            raise StepFailure("process", _sanitize_detail(error)) from error
        return _bounded_communicate(process, timeout, MAX_LOG_BYTES)

    def spawn(
        self,
        argv: Sequence[str],
        *,
        env: Mapping[str, str],
        log_path: Path,
        cwd: Path,
    ) -> Spawned:
        descriptor = os.open(
            log_path,
            os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC | os.O_NOFOLLOW,
            0o600,
        )
        try:
            process = subprocess.Popen(
                list(argv),
                stdin=subprocess.DEVNULL,
                stdout=descriptor,
                stderr=descriptor,
                cwd=os.fspath(cwd),
                env=dict(env),
                start_new_session=True,
                close_fds=True,
            )
        except OSError as error:
            os.close(descriptor)
            raise StepFailure("startup", _sanitize_detail(error)) from error
        os.close(descriptor)
        return HostProcess(process, log_path)


def _read_regular_file(path: Path, maximum: int, label: str) -> bytes:
    try:
        observed = path.lstat()
    except OSError as error:
        raise InputError(f"{label} is unavailable") from error
    if stat.S_ISLNK(observed.st_mode) or not stat.S_ISREG(observed.st_mode):
        raise InputError(f"{label} must be a regular non-symlink file")
    if observed.st_size <= 0 or observed.st_size > maximum:
        raise InputError(f"{label} size is invalid")
    descriptor = -1
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW)
        opened = os.fstat(descriptor)
        if (opened.st_dev, opened.st_ino, opened.st_mode, opened.st_size) != (
            observed.st_dev,
            observed.st_ino,
            observed.st_mode,
            observed.st_size,
        ):
            raise InputError(f"{label} identity changed")
        chunks: list[bytes] = []
        total = 0
        while True:
            chunk = os.read(descriptor, min(64 * 1024, maximum + 1 - total))
            if not chunk:
                break
            chunks.append(chunk)
            total += len(chunk)
            if total > maximum:
                raise InputError(f"{label} size is invalid")
        if total != opened.st_size:
            raise InputError(f"{label} identity changed")
        return b"".join(chunks)
    except OSError as error:
        raise InputError(f"{label} is unavailable") from error
    finally:
        if descriptor >= 0:
            os.close(descriptor)


def _require_absolute_path(path: Path, label: str) -> Path:
    if not path.is_absolute() or path.name in ("", ".", ".."):
        raise InputError(f"{label} path must be absolute")
    return path


def _require_new_result(path: Path) -> Path:
    _require_absolute_path(path, "result")
    if os.path.lexists(path):
        raise InputError("result path already exists")
    parent = path.parent
    if not parent.is_dir():
        raise InputError("result parent directory does not exist")
    return path


def _run_git(
    source_root: Path,
    arguments: tuple[str, ...],
    operations: Operations,
    timeout: float,
) -> bytes:
    try:
        outcome = operations.run(
            [os.fspath(GIT), "-C", os.fspath(source_root), *arguments],
            env=GIT_ENV,
            timeout=timeout,
        )
    except StepFailure as error:
        raise InputError("source git command failed") from error
    if outcome.returncode != 0 or len(outcome.stdout) > MAX_GIT_TEXT_BYTES:
        raise InputError("source git command failed")
    return outcome.stdout


def _git_text(
    source_root: Path,
    arguments: tuple[str, ...],
    operations: Operations,
    timeout: float,
) -> str:
    try:
        text = _run_git(source_root, arguments, operations, timeout).decode("utf-8")
    except UnicodeDecodeError as error:
        raise InputError("source git command failed") from error
    return text.rstrip("\n")


def _require_clean_source(
    source_root: Path, operations: Operations, timeout: float
) -> tuple[Path, str]:
    _require_absolute_path(source_root, "source root")
    try:
        resolved = source_root.resolve(strict=True)
    except OSError as error:
        raise InputError("source root must be an existing absolute directory") from error
    if not resolved.is_dir():
        raise InputError("source root must be an existing absolute directory")
    toplevel = _git_text(
        source_root, ("rev-parse", "--show-toplevel"), operations, timeout
    )
    try:
        resolved_toplevel = Path(toplevel).resolve(strict=True)
    except OSError as error:
        raise InputError("source root is not the git toplevel") from error
    if resolved_toplevel != resolved:
        raise InputError("source root is not the git toplevel")
    head = _git_text(
        source_root, ("rev-parse", "--verify", "HEAD"), operations, timeout
    )
    if not COMMIT_RE.fullmatch(head):
        raise InputError("source HEAD is invalid")
    status = _run_git(
        source_root,
        ("status", "--porcelain=v1", "--untracked-files=all"),
        operations,
        timeout,
    )
    if status != b"":
        raise InputError("source tree is not clean")
    return resolved, head


def _parse_debian_archive(payload: bytes) -> dict[str, bytes]:
    if not payload.startswith(AR_MAGIC):
        raise InputError("candidate is not a Debian archive")
    offset = len(AR_MAGIC)
    members: dict[str, bytes] = {}
    while offset < len(payload):
        if len(members) >= MAX_AR_MEMBERS:
            raise InputError("Debian archive has too many members")
        if offset + AR_HEADER_SIZE > len(payload):
            raise InputError("candidate is not a Debian archive")
        header = payload[offset : offset + AR_HEADER_SIZE]
        if header[58:60] != b"`\n":
            raise InputError("candidate is not a Debian archive")
        try:
            name = header[0:16].decode("ascii").strip()
            size = int(header[48:58].decode("ascii").strip(), 10)
        except (UnicodeDecodeError, ValueError) as error:
            raise InputError("candidate is not a Debian archive") from error
        if name.endswith("/"):
            name = name[:-1]
        if (
            not name
            or name in (".", "..")
            or "/" in name
            or size < 0
            or offset + AR_HEADER_SIZE + size > len(payload)
        ):
            raise InputError("candidate is not a Debian archive")
        offset += AR_HEADER_SIZE
        data = payload[offset : offset + size]
        offset += size
        if size % 2 == 1 and offset < len(payload):
            offset += 1
        if name in members:
            raise InputError("Debian archive contains a duplicate member")
        members[name] = data
    if members.get("debian-binary") not in (b"2.0\n", b"2.0"):
        raise InputError("candidate is not a Debian archive")
    return members


def _archive_payload(members: Mapping[str, bytes], stem: str) -> tuple[str, bytes]:
    matches = [name for name in members if name == stem or name.startswith(stem + ".")]
    if len(matches) != 1:
        raise InputError("Debian archive membership is invalid")
    name = matches[0]
    return name, members[name]


def _open_tar(payload: bytes, archive_name: str) -> tarfile.TarFile:
    if archive_name.endswith(".tar.gz"):
        mode = "r:gz"
    elif archive_name.endswith(".tar.xz"):
        mode = "r:xz"
    elif archive_name.endswith(".tar"):
        mode = "r:"
    else:
        raise InputError("Debian archive compression is unsupported")
    try:
        return tarfile.open(fileobj=io.BytesIO(payload), mode=mode)
    except (tarfile.TarError, OSError, EOFError) as error:
        raise InputError("Debian archive is invalid") from error


def _normalize_tar_name(name: str) -> str:
    normalized = name.removeprefix("./") or "."
    path = PurePosixPath(normalized)
    if path.is_absolute() or ".." in path.parts:
        raise InputError("candidate archive path is unsafe")
    return normalized


def _required_tar_regular_files(
    payload: bytes, archive_name: str, wanted: Mapping[str, int]
) -> dict[str, bytes]:
    found: dict[str, bytes] = {}
    member_count = 0
    declared_bytes = 0
    archive = _open_tar(payload, archive_name)
    try:
        for member in archive:
            member_count += 1
            declared_bytes += max(member.size, 0)
            if (
                member_count > MAX_TAR_MEMBERS
                or member.size < 0
                or declared_bytes > MAX_TAR_DECLARED_BYTES
            ):
                raise InputError("Debian archive exceeds bounded tar limits")
            normalized = _normalize_tar_name(member.name)
            if normalized not in wanted:
                continue
            if normalized in found:
                raise InputError(f"{normalized} member is duplicated")
            if not member.isfile() or member.issym() or member.islnk():
                raise InputError(f"{normalized} member is not a regular file")
            maximum = wanted[normalized]
            if member.size < 1 or member.size > maximum:
                raise InputError(f"{normalized} member size is invalid")
            extracted = archive.extractfile(member)
            if extracted is None:
                raise InputError(f"{normalized} member is not a regular file")
            data = extracted.read(maximum + 1)
            if len(data) != member.size:
                raise InputError(f"{normalized} member is invalid")
            found[normalized] = data
    except (tarfile.TarError, OSError, EOFError) as error:
        raise InputError("Debian archive is invalid") from error
    finally:
        archive.close()
    missing = [name for name in wanted if name not in found]
    if missing:
        raise InputError("candidate package member is missing")
    return found


def _digest_size(value: object) -> dict[str, object]:
    if not isinstance(value, dict):
        raise InputError("build identity is not a clean unqualified candidate")
    digest = value.get("sha256")
    size = value.get("size_bytes")
    if not isinstance(digest, str) or not SHA256_RE.fullmatch(digest):
        raise InputError("build identity is not a clean unqualified candidate")
    if isinstance(size, bool) or not isinstance(size, int) or size <= 0:
        raise InputError("build identity is not a clean unqualified candidate")
    return {"sha256": digest, "size_bytes": size}


def _parse_build_info(payload: bytes) -> EmbeddedIdentity:
    if len(payload) > MAX_BUILD_INFO_BYTES:
        raise InputError("build-info member is oversized")
    try:
        document = json.loads(payload.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise InputError("build identity is not a clean unqualified candidate") from error
    if not isinstance(document, dict):
        raise InputError("build identity is not a clean unqualified candidate")
    inputs = document.get("inputs")
    if (
        document.get("schema_version") != 2
        or document.get("classification") != CLEAN_CLASSIFICATION
        or document.get("git_dirty") is not False
        or document.get("package") != PACKAGE_NAME
        or not isinstance(document.get("version"), str)
        or not document["version"]
        or not isinstance(document.get("source_head"), str)
        or not COMMIT_RE.fullmatch(document["source_head"])
        or isinstance(document.get("source_date_epoch"), bool)
        or not isinstance(document.get("source_date_epoch"), int)
        or document["source_date_epoch"] <= 0
        or document["source_date_epoch"] > MAX_SOURCE_DATE_EPOCH
        or not isinstance(inputs, dict)
    ):
        raise InputError("build identity is not a clean unqualified candidate")
    embedded = {key: _digest_size(inputs.get(key)) for key in EMBEDDED_KEYS}
    return EmbeddedIdentity(
        package=PACKAGE_NAME,
        version=document["version"],
        source_head=document["source_head"],
        source_date_epoch=document["source_date_epoch"],
        embedded=embedded,
    )


def _parse_wav_duration(payload: bytes) -> float:
    if len(payload) < 44 or payload[0:4] != b"RIFF" or payload[8:12] != b"WAVE":
        raise InputError("fixture is not a RIFF/WAVE PCM16 mono 16 kHz file")
    rate = None
    channels = None
    bits = None
    audio_format = None
    data_bytes = None
    position = 12
    while position + 8 <= len(payload):
        chunk_id = payload[position : position + 4]
        size = int.from_bytes(payload[position + 4 : position + 8], "little")
        body = position + 8
        end = body + size
        if end > len(payload):
            raise InputError("fixture WAV is truncated")
        if chunk_id == b"fmt ":
            if size < 16:
                raise InputError("fixture WAV fmt chunk is invalid")
            audio_format = int.from_bytes(payload[body : body + 2], "little")
            channels = int.from_bytes(payload[body + 2 : body + 4], "little")
            rate = int.from_bytes(payload[body + 4 : body + 8], "little")
            bits = int.from_bytes(payload[body + 14 : body + 16], "little")
        elif chunk_id == b"data":
            if data_bytes is not None:
                raise InputError("fixture WAV contains duplicate data")
            data_bytes = size
            if body + size > len(payload):
                raise InputError("fixture WAV is truncated")
        position = end + (size & 1)
    if (
        audio_format != 1
        or channels != WAV_CHANNELS
        or rate != WAV_RATE
        or bits != WAV_BITS
        or data_bytes is None
        or data_bytes < 1
        or data_bytes % 2 != 0
    ):
        raise InputError("fixture is not a RIFF/WAVE PCM16 mono 16 kHz file")
    duration = data_bytes / 2 / WAV_RATE
    if duration < MIN_WAV_SECONDS or duration > MAX_WAV_SECONDS:
        raise InputError("fixture duration is outside the permitted bound")
    return duration


def _desktop_lane(environ: Mapping[str, str]) -> str:
    raw = environ.get("XDG_CURRENT_DESKTOP", "")
    return raw.split(":")[-1].strip().upper() or "UNKNOWN"


def _require_native_cosmic(operations: Operations) -> tuple[str, str]:
    """The shared native-lane proof; environment strings alone never suffice."""
    try:
        operations.native_lane()
    except recorder_safety.RecorderSafetyError as error:
        raise InputError(f"session is not the reviewed native lane: {error}") from None
    environ = operations.environ()
    if operations.containerized() or _desktop_lane(environ) != "COSMIC":
        raise InputError("session is not native COSMIC Wayland")
    return _desktop_lane(environ), "wayland"


def _require_physical_source(operations: Operations, timeout: float) -> None:
    try:
        outcome = operations.run(
            [os.fspath(PACTL), "get-default-source"],
            env={
                "PATH": "/usr/bin:/bin",
                "LC_ALL": "C",
                **_pipewire_env(operations.environ()),
            },
            timeout=timeout,
        )
    except StepFailure as error:
        raise InputError("default Pulse/PipeWire source is unavailable") from error
    if outcome.returncode != 0:
        raise InputError("default Pulse/PipeWire source is unavailable")
    if len(outcome.stdout) > MAX_SOURCE_NAME_BYTES:
        raise InputError("default Pulse/PipeWire source is invalid")
    try:
        name = outcome.stdout.decode("utf-8").splitlines()[0].strip()
    except (UnicodeDecodeError, IndexError) as error:
        raise InputError("default Pulse/PipeWire source is invalid") from error
    if not name or any(ord(character) < 32 for character in name):
        raise InputError("default Pulse/PipeWire source is invalid")
    lowered = name.casefold()
    tokens = set(re.split(r"[^a-z0-9]+", lowered))
    if ".monitor" in lowered or tokens & _UNSAFE_SOURCE_TOKENS:
        raise InputError("default source is not a physical microphone")


def _pipewire_env(environ: Mapping[str, str]) -> dict[str, str]:
    runtime = str(environ.get("XDG_RUNTIME_DIR", "")).strip()
    if not runtime:
        raise InputError("host PipeWire runtime is unavailable")
    env = {
        "XDG_RUNTIME_DIR": runtime,
        "PIPEWIRE_RUNTIME_DIR": str(environ.get("PIPEWIRE_RUNTIME_DIR", "")).strip()
        or runtime,
    }
    remote = str(environ.get("PIPEWIRE_REMOTE", "")).strip()
    if remote:
        env["PIPEWIRE_REMOTE"] = remote
    pulse_server = str(environ.get("PULSE_SERVER", "")).strip()
    if pulse_server:
        env["PULSE_SERVER"] = pulse_server
    pulse_runtime = str(environ.get("PULSE_RUNTIME_PATH", "")).strip()
    if pulse_runtime:
        env["PULSE_RUNTIME_PATH"] = pulse_runtime
    return env


def _member_maximum(key: str) -> int:
    if key == "bundled-model":
        return MAX_MODEL_BYTES
    if key == BUILD_INFO_MEMBER:
        return MAX_BUILD_INFO_BYTES
    return MAX_BINARY_BYTES


def _prepare_package(
    payload: bytes, source_head: str
) -> tuple[EmbeddedIdentity, dict[str, bytes]]:
    members = _parse_debian_archive(payload)
    _data_name, data_payload = _archive_payload(members, "data.tar")
    wanted = {BUILD_INFO_MEMBER: MAX_BUILD_INFO_BYTES}
    for member, key in EXTRACT_MEMBERS.items():
        wanted[member] = _member_maximum(key)
    extracted = _required_tar_regular_files(data_payload, _data_name, wanted)
    identity = _parse_build_info(extracted[BUILD_INFO_MEMBER])
    if identity.source_head != source_head:
        raise InputError("candidate source_head does not match the clean HEAD")
    for member, key in EXTRACT_MEMBERS.items():
        expected = identity.embedded[key]
        data = extracted[member]
        if len(data) != expected["size_bytes"] or _digest_of(data) != expected["sha256"]:
            raise InputError("packaged member identity does not match build-info")
    return identity, extracted


def _write_extracted(root: Path, relative: str, payload: bytes, mode: int) -> Path:
    path = root.joinpath(*relative.split("/"))
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor = os.open(
        path,
        os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC | os.O_NOFOLLOW,
        mode,
    )
    try:
        view = memoryview(payload)
        while view:
            written = os.write(descriptor, view)
            if written <= 0:
                raise StepFailure("extract", "extracted member write failed")
            view = view[written:]
        os.fsync(descriptor)
    finally:
        os.close(descriptor)
    return path


def _remove_tree(path: Path | None) -> bool:
    if path is None or not os.path.lexists(path):
        return True
    try:
        if path.is_dir() and not path.is_symlink():
            for child in path.iterdir():
                if not _remove_tree(child):
                    return False
            os.rmdir(path)
        else:
            os.unlink(path)
        return not os.path.lexists(path)
    except OSError:
        return False


def _write_result(path: Path, payload: dict[str, object]) -> None:
    data = _canonical_json(payload)
    directory = path.parent
    fd, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=directory)
    try:
        os.fchmod(fd, 0o600)
        view = memoryview(data)
        while view:
            written = os.write(fd, view)
            if written <= 0:
                raise OSError("result write failed")
            view = view[written:]
        os.fsync(fd)
    except OSError:
        os.close(fd)
        try:
            os.unlink(temporary)
        except OSError:
            pass
        raise
    os.close(fd)
    try:
        os.link(temporary, path)
    except OSError:
        try:
            os.unlink(temporary)
        except OSError:
            pass
        raise
    try:
        os.unlink(temporary)
    except OSError:
        pass


def _base_result(
    *,
    candidate_name: str,
    candidate_sha256: str,
    candidate_size: int,
    fixture_name: str,
    fixture_sha256: str,
    fixture_size: int,
    fixture_duration: float | None,
    source_head: str | None,
    desktop: str | None,
    session: str | None,
) -> dict[str, object]:
    return {
        "schema_version": 1,
        "classification": RESULT_CLASSIFICATION,
        "result": "failed",
        "failed_step": None,
        "failure_detail": None,
        "candidate": {
            "file": candidate_name,
            "sha256": candidate_sha256,
            "size_bytes": candidate_size,
        },
        "fixture": {
            "file": fixture_name,
            "sha256": fixture_sha256,
            "size_bytes": fixture_size,
            "duration_seconds": fixture_duration,
        },
        "source": {
            "head": source_head,
            "git_dirty": False,
            "repository": "narrivy",
        },
        "environment": {
            "desktop": desktop,
            "session": session,
            "native_or_vm": "native",
            "default_source_kind": "physical",
            "microphone_class": "physical-default-source",
            "os": platform.system(),
            "os_version": platform.release(),
            "kernel": platform.release(),
        },
        "steps": [],
        "observations": None,
        "cleanup": {
            "extraction_removed": False,
            "runtime_removed": False,
            "state_removed": False,
            "holder_removed": False,
        },
        "unqualified": True,
        "requirement_evidence": False,
        "release_qualified": False,
        "target_insertion": False,
    }


def _summary(result: Mapping[str, object]) -> dict[str, object]:
    payload: dict[str, object] = {
        "classification": RESULT_CLASSIFICATION,
        "release_qualified": False,
        "requirement_evidence": False,
        "result": result.get("result"),
        "target_insertion": False,
        "unqualified": True,
    }
    if result.get("result") != "passed":
        payload["failed_step"] = result.get("failed_step")
    return payload


def _daemon_env(
    *,
    holder: Path,
    runtime_dir: Path,
    state_dir: Path,
    socket_path: Path,
    worker: Path,
    model: Path,
    identity: EmbeddedIdentity,
    pipewire: Mapping[str, str],
) -> dict[str, str]:
    home = holder / "home"
    cache = holder / "cache"
    data = holder / "data"
    env = {
        "PATH": "/usr/bin:/bin",
        "HOME": str(home),
        "LC_ALL": "C",
        "TZ": "UTC",
        "XDG_STATE_HOME": str(state_dir),
        "XDG_CACHE_HOME": str(cache),
        "XDG_DATA_HOME": str(data),
        "XDG_CONFIG_HOME": str(holder / "config"),
        "NARRIVY_RUNTIME_DIR": str(runtime_dir),
        "NARRIVY_SOCKET": str(socket_path),
        "NARRIVY_RECOVERY_MODE": "linux-mic-inbox",
        "NARRIVY_DEFAULT_PIPELINE": "clean",
        "NARRIVY_ASR_ENGINE": "transcribe-cpp",
        "NARRIVY_AUDIO_SOURCE": "pipewire",
        "NARRIVY_DELIVERY_BACKEND": "inbox",
        "NARRIVY_DELIVERY_FAIL": "0",
        "NARRIVY_TRANSCRIBE_RUNTIME": "resident",
        "NARRIVY_TRANSCRIBE_WORKER": str(worker),
        "NARRIVY_TRANSCRIBE_WORKER_SHA256": str(
            identity.embedded["resident-worker"]["sha256"]
        ),
        "NARRIVY_TRANSCRIBE_WORKER_SIZE": str(
            identity.embedded["resident-worker"]["size_bytes"]
        ),
        "NARRIVY_TRANSCRIBE_MODEL": str(model),
        "NARRIVY_TRANSCRIBE_MODEL_SHA256": str(
            identity.embedded["bundled-model"]["sha256"]
        ),
        "NARRIVY_TRANSCRIBE_MODEL_SIZE": str(
            identity.embedded["bundled-model"]["size_bytes"]
        ),
        "NARRIVY_CAPTURE_SECONDS": str(CAPTURE_SECONDS),
    }
    env.update(pipewire)
    return env


def _wait_for_socket(
    socket_path: Path,
    daemon: Spawned,
    operations: Operations,
    deadline: float,
) -> None:
    while True:
        if daemon.poll() is not None:
            raise StepFailure("startup", "daemon exited before the socket was ready")
        if socket_path.exists():
            probe = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            probe.settimeout(0.2)
            try:
                probe.connect(os.fspath(socket_path))
                return
            except OSError:
                pass
            finally:
                try:
                    probe.close()
                except OSError:
                    pass
        if operations.monotonic() >= deadline:
            raise StepFailure("startup", "daemon never became reachable")
        operations.sleep(min(POLL_SECONDS, max(0.0, deadline - operations.monotonic())))


def _scan_logs(
    blobs: Sequence[bytes], transcripts: Sequence[str] = (), canaries: Sequence[str] = ()
) -> tuple[int, int]:
    """Count fixture-fragment, transcript n-gram and canary hits in the logs.

    ``transcripts`` are the actual committed transcripts, held in memory only.
    """
    combined = bytearray()
    for blob in blobs:
        room = MAX_LOG_BYTES - len(combined)
        if room <= 0:
            raise StepFailure("log_hygiene", "collected logs exceeded the scan bound")
        combined += blob[:room]
        if len(blob) > room:
            raise StepFailure("log_hygiene", "collected logs exceeded the scan bound")
    try:
        text = bytes(combined).decode("utf-8", "replace")
    except Exception as error:
        raise StepFailure("log_hygiene", "log scan failed") from error
    hits = text.casefold().count(EXPECTED_FRAGMENT)
    if transcripts or canaries:
        try:
            scan = recorder_safety.scan_log_privacy(
                [bytes(combined)], transcripts=transcripts, canaries=canaries
            )
        except recorder_safety.RecorderSafetyError as error:
            raise StepFailure("log_hygiene", str(error)) from None
        hits += int(scan["transcript_ngram_hits"]) + int(scan["canary_hits"])
    return len(text), hits


def _list_inbox(inbox_dir: Path) -> list[Path]:
    if not inbox_dir.is_dir():
        return []
    found: list[Path] = []
    for entry in sorted(inbox_dir.iterdir()):
        try:
            observed = entry.lstat()
        except OSError as error:
            raise StepFailure("inbox", "inbox could not be read") from error
        if entry.name.startswith(".inbox-"):
            raise StepFailure("inbox", "inbox contained an unfinished artifact")
        if entry.name.startswith("."):
            continue
        if stat.S_ISLNK(observed.st_mode):
            raise StepFailure("inbox", "inbox contained a symlink")
        if not stat.S_ISREG(observed.st_mode):
            raise StepFailure("inbox", "inbox contained a non-file artifact")
        if entry.suffix != ".txt":
            raise StepFailure("inbox", "inbox contained a non-transcript artifact")
        found.append(entry)
    return found


def _read_transcript(path: Path) -> tuple[str, int]:
    digest, size, _text = _read_transcript_text(path)
    return digest, size


def _read_transcript_text(path: Path) -> tuple[str, int, str]:
    """Like ``_read_transcript`` but also returns the text, for in-memory log scans."""
    try:
        observed = path.lstat()
    except OSError as error:
        raise StepFailure("fragment", "inbox artifact is unavailable") from error
    if stat.S_ISLNK(observed.st_mode) or not stat.S_ISREG(observed.st_mode):
        raise StepFailure("fragment", "inbox artifact is not a regular file")
    if observed.st_size <= 0 or observed.st_size > MAX_TRANSCRIPT_BYTES:
        raise StepFailure("fragment", "inbox artifact size is invalid")
    payload = path.read_bytes()
    if len(payload) != observed.st_size:
        raise StepFailure("fragment", "inbox artifact identity changed")
    try:
        text = payload.decode("utf-8")
    except UnicodeDecodeError as error:
        raise StepFailure("fragment", "inbox artifact was not UTF-8") from error
    if EXPECTED_FRAGMENT not in text.casefold():
        raise StepFailure("fragment", "expected fragment was not present")
    digest = _digest_of(payload)
    size = len(payload)
    del payload
    return digest, size, text


def qualify(
    inputs: Inputs,
    operations: Operations | None = None,
    limits: Limits = DEFAULT_LIMITS,
) -> dict[str, object]:
    """Run the bounded physical known-phrase procedure and write its result."""

    if not inputs.confirm_audible_playback:
        raise InputError("audible playback was not confirmed")
    _validate_limits(limits)
    ops = operations if operations is not None else HostOperations()
    result_path = _require_new_result(inputs.result)
    candidate_path = _require_absolute_path(inputs.candidate, "candidate")
    if candidate_path.suffix != ".deb":
        raise InputError("candidate path must end in .deb")
    fixture_path = _require_absolute_path(inputs.fixture, "fixture")
    expected_candidate = _require_sha256(
        inputs.expected_candidate_sha256, "candidate"
    )
    expected_fixture = _require_sha256(inputs.expected_fixture_sha256, "fixture")
    for helper in REQUIRED_HELPERS:
        if not ops.helper_ready(helper):
            raise InputError("required system helper is missing")

    source_root, source_head = _require_clean_source(
        inputs.source_root, ops, limits.git
    )
    candidate_payload = _read_regular_file(
        candidate_path, MAX_CANDIDATE_BYTES, "candidate"
    )
    candidate_digest = _digest_of(candidate_payload)
    candidate_size = len(candidate_payload)
    if candidate_digest != expected_candidate:
        raise InputError("candidate SHA-256 does not match the expected value")
    fixture_payload = _read_regular_file(fixture_path, MAX_FIXTURE_BYTES, "fixture")
    fixture_digest = _digest_of(fixture_payload)
    fixture_size = len(fixture_payload)
    if fixture_digest != expected_fixture:
        raise InputError("fixture SHA-256 does not match the expected value")
    fixture_duration = _parse_wav_duration(fixture_payload)
    identity, members = _prepare_package(candidate_payload, source_head)
    del candidate_payload
    del fixture_payload
    desktop, session = _require_native_cosmic(ops)
    pipewire = _pipewire_env(ops.environ())
    _require_physical_source(ops, limits.helper)

    result = _base_result(
        candidate_name=candidate_path.name,
        candidate_sha256=candidate_digest,
        candidate_size=candidate_size,
        fixture_name=fixture_path.name,
        fixture_sha256=fixture_digest,
        fixture_size=fixture_size,
        fixture_duration=round(fixture_duration, 3),
        source_head=source_head,
        desktop=desktop,
        session=session,
    )

    steps: list[dict[str, str]] = [{"step": "validate", "status": "passed"}]
    result["steps"] = steps
    holder: Path | None = None
    daemon: Spawned | None = None
    log_blobs: list[bytes] = []
    state = {"current": "extract"}

    def remaining(step: str, budget: float, local_deadline: float | None = None) -> float:
        if budget <= 0:
            raise StepFailure(step, "operation time budget was invalid")
        overall_left = overall_deadline - ops.monotonic()
        if overall_left <= 0:
            raise StepFailure(step, "overall procedure deadline exceeded")
        if local_deadline is not None:
            local_left = local_deadline - ops.monotonic()
            if local_left <= 0:
                raise StepFailure(step, "operation exceeded its local time bound")
            budget = min(budget, local_left)
        return min(budget, overall_left)

    def passed(step: str) -> None:
        steps.append({"step": step, "status": "passed"})
        state["current"] = step

    def run_cli(step: str, arguments: Sequence[str], budget: float) -> CommandResult:
        try:
            outcome = ops.run(
                [str(cli_bin), "--socket", str(socket_path), *arguments],
                env=daemon_env,
                timeout=remaining(step, budget),
                cwd=holder,
            )
        except StepFailure as error:
            raise StepFailure(step, error.detail) from None
        log_blobs.append(outcome.stdout)
        log_blobs.append(outcome.stderr)
        if outcome.returncode != 0:
            raise StepFailure(step, f"exit_status={outcome.returncode}")
        return outcome

    overall_deadline = ops.monotonic() + limits.total
    extract_dir: Path | None = None
    runtime_dir: Path | None = None
    state_dir: Path | None = None
    socket_path: Path | None = None
    cli_bin: Path | None = None
    daemon_env: dict[str, str] = {}

    try:
        holder = Path(tempfile.mkdtemp(prefix="narrivy-known-phrase-"))
        os.chmod(holder, 0o700)
        extract_dir = holder / "extract"
        runtime_dir = holder / "runtime"
        state_dir = holder / "state"
        for directory in (
            extract_dir,
            runtime_dir,
            state_dir,
            holder / "home",
            holder / "cache",
            holder / "data",
            holder / "config",
        ):
            directory.mkdir(mode=0o700)
            directory.chmod(0o700)
        written: dict[str, Path] = {}
        for member, key in EXTRACT_MEMBERS.items():
            mode = 0o755 if key != "bundled-model" else 0o644
            path = _write_extracted(extract_dir, member, members[member], mode)
            if _digest_of(path.read_bytes()) != identity.embedded[key]["sha256"]:
                raise StepFailure("extract", "extracted member identity changed")
            written[member] = path
        del members
        daemon_bin = written[DAEMON_MEMBER]
        cli_bin = written[CLI_MEMBER]
        worker_bin = written[WORKER_MEMBER]
        model_path = written[MODEL_MEMBER]
        socket_path = runtime_dir / "narrivyd.sock"
        daemon_env = _daemon_env(
            holder=holder,
            runtime_dir=runtime_dir,
            state_dir=state_dir,
            socket_path=socket_path,
            worker=worker_bin,
            model=model_path,
            identity=identity,
            pipewire=pipewire,
        )
        if "NARRIVY_ALLOW_VIRTUAL_MIC" in daemon_env:
            raise StepFailure("extract", "virtual-microphone authorization was set")
        passed("extract")

        state["current"] = "startup"
        log_path = runtime_dir / "daemon.log"
        daemon = ops.spawn(
            [str(daemon_bin)],
            env=daemon_env,
            log_path=log_path,
            cwd=holder,
        )
        _wait_for_socket(
            socket_path,
            daemon,
            ops,
            ops.monotonic() + remaining("startup", limits.socket),
        )
        if daemon.poll() is not None:
            raise StepFailure("startup", "daemon exited after binding")
        passed("startup")

        state["current"] = "start"
        start = run_cli("start", ("start", "--pipeline", "clean"), limits.start)
        if b"rejected" in start.stderr.lower() or b"rejected" in start.stdout.lower():
            raise StepFailure("start", "session start was rejected")
        passed("start")

        state["current"] = "playback"
        paplay_env = {
            "PATH": "/usr/bin:/bin",
            "LC_ALL": "C",
            **pipewire,
        }
        try:
            playback = ops.run(
                [os.fspath(PAPLAY), "--", os.fspath(fixture_path)],
                env=paplay_env,
                timeout=remaining("playback", limits.playback),
                cwd=holder,
            )
        except StepFailure as error:
            raise StepFailure("playback", error.detail) from None
        log_blobs.append(playback.stdout)
        log_blobs.append(playback.stderr)
        if playback.returncode != 0:
            raise StepFailure("playback", f"exit_status={playback.returncode}")
        passed("playback")

        state["current"] = "stop"
        run_cli("stop", ("stop",), limits.stop)
        passed("stop")

        state["current"] = "idle"
        idle_deadline = ops.monotonic() + remaining("idle", limits.idle)
        while True:
            status = run_cli("idle", ("status",), limits.helper)
            text = status.stdout.decode("utf-8", "replace")
            if "no active session" in text:
                break
            if ops.monotonic() >= idle_deadline:
                raise StepFailure("idle", "session did not become idle")
            if daemon.poll() is not None:
                raise StepFailure("idle", "daemon exited before idle")
            ops.sleep(min(POLL_SECONDS, max(0.0, idle_deadline - ops.monotonic())))
        passed("idle")

        state["current"] = "inbox"
        inbox_dir = runtime_dir / "inbox"
        artifacts = _list_inbox(inbox_dir)
        if len(artifacts) != 1:
            raise StepFailure("inbox", f"expected one inbox artifact, observed {len(artifacts)}")
        passed("inbox")

        state["current"] = "fragment"
        transcript_sha256, transcript_bytes, transcript_text = _read_transcript_text(artifacts[0])
        passed("fragment")

        state["current"] = "log_hygiene"
        if daemon is None or not daemon.kill_group():
            raise StepFailure("shutdown", "daemon process group did not terminate")
        if daemon.poll() != 0:
            raise StepFailure("shutdown", "daemon exited unsuccessfully")
        log_blobs.append(daemon.output())
        scanned, hits = _scan_logs(log_blobs, transcripts=(transcript_text,))
        del transcript_text
        if hits != 0:
            raise StepFailure("log_hygiene", "ordinary logs contained the fixture fragment")
        if daemon.poll() not in (None, 0):
            raise StepFailure("log_hygiene", "a child failure was ignored")
        result["observations"] = {
            "inbox_files": 1,
            "transcript_sha256": transcript_sha256,
            "transcript_utf8_bytes": transcript_bytes,
            "fragment_matched": True,
            "log_fragment_hits": 0,
            "log_bytes_scanned": scanned,
            "child_failures_ignored": False,
            "capture_seconds": CAPTURE_SECONDS,
        }
        passed("log_hygiene")
        result["result"] = "passed"
    except StepFailure as error:
        result["result"] = "failed"
        result["failed_step"] = error.step
        result["failure_detail"] = error.detail
        if not any(entry["step"] == error.step for entry in steps):
            steps.append({"step": error.step, "status": "failed"})
        if daemon is not None:
            try:
                log_blobs.append(daemon.output())
            except StepFailure:
                pass
    except OSError as error:
        step = state["current"]
        result["result"] = "failed"
        result["failed_step"] = step
        result["failure_detail"] = _sanitize_detail(error)
        if not any(entry["step"] == step for entry in steps):
            steps.append({"step": step, "status": "failed"})
    finally:
        termination_ok = daemon is None or daemon.poll() is not None or daemon.kill_group()
        log_blobs.clear()
        cleanup = {
            "extraction_removed": _remove_tree(extract_dir) if extract_dir else True,
            "runtime_removed": _remove_tree(runtime_dir) if runtime_dir else True,
            "state_removed": _remove_tree(state_dir) if state_dir else True,
            "holder_removed": _remove_tree(holder) if holder else True,
        }
        result["cleanup"] = cleanup
        result["steps"] = steps
        if not termination_ok and result["result"] == "passed":
            result["result"] = "failed"
            result["failed_step"] = "shutdown"
            result["failure_detail"] = "daemon process group termination was uncertain"
        if not all(cleanup.values()) and result["result"] == "passed":
            result["result"] = "failed"
            result["failed_step"] = "cleanup"
            result["failure_detail"] = "transcript-bearing paths were not removed"
        _write_result(result_path, result)
    return result


def _arguments(argv: list[str] | None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--candidate", required=True, type=Path)
    parser.add_argument("--expected-candidate-sha256", required=True)
    parser.add_argument("--fixture", required=True, type=Path)
    parser.add_argument("--expected-fixture-sha256", required=True)
    parser.add_argument("--result", required=True, type=Path)
    parser.add_argument(
        "--confirm-audible-playback",
        action="store_true",
        help="required; plays the fixture once through the current default speakers",
    )
    return parser.parse_args(argv)


def main(
    argv: list[str] | None = None,
    *,
    operations: Operations | None = None,
) -> int:
    values = _arguments(argv)
    inputs = Inputs(
        candidate=values.candidate,
        fixture=values.fixture,
        result=values.result,
        expected_candidate_sha256=values.expected_candidate_sha256,
        expected_fixture_sha256=values.expected_fixture_sha256,
        confirm_audible_playback=values.confirm_audible_playback,
        source_root=REPOSITORY_ROOT,
    )
    try:
        result = qualify(inputs, operations=operations)
    except InputError as error:
        print(f"qualify_candidate_known_phrase: {error}", file=sys.stderr)
        return 2
    except OSError as error:
        print(
            f"qualify_candidate_known_phrase: {_sanitize_detail(error)}",
            file=sys.stderr,
        )
        return 1
    print(json.dumps(_summary(result), sort_keys=True, separators=(",", ":")), flush=True)
    return 0 if result["result"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
