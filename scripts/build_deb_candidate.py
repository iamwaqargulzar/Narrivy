#!/usr/bin/env python3
"""Build and inspect one disposable, deliberately non-installable Debian candidate.

The archive exists only inside a private temporary directory and is removed
before this command returns. This is composition testing, not package evidence
or release-artifact construction.
"""

from __future__ import annotations

import argparse
import ctypes
import fcntl
import hashlib
import json
import os
import re
import signal
import stat
import struct
import subprocess
import sys
import tarfile
import tempfile
import resource
from dataclasses import dataclass
from pathlib import Path, PurePosixPath

import stage_installed_runtime as runtime_staging

PACKAGE_NAME = "narrivy-recovery-candidate"
PACKAGE_VERSION = "0.0.0~recovery4qc4"
DPKG_DEB = Path("/usr/bin/dpkg-deb")
MAX_ASSET_BYTES = 16 * 1024 * 1024
MAX_BINARY_BYTES = 256 * 1024 * 1024
MAX_ARCHIVE_BYTES = 512 * 1024 * 1024
CANONICAL_SERVICE = b"""[Unit]
Description=Narrivy dictation daemon (per-user)
After=graphical-session.target pipewire.service
Wants=pipewire.service
StartLimitIntervalSec=60s
StartLimitBurst=3

[Service]
Type=simple
ExitType=cgroup
Environment=NARRIVY_RECOVERY_MODE=linux-mic-inbox
Environment=NARRIVY_DEFAULT_PIPELINE=clean
Environment=NARRIVY_ASR_ENGINE=transcribe-cpp
Environment=NARRIVY_AUDIO_SOURCE=pipewire
Environment=NARRIVY_DELIVERY_BACKEND=inbox
Environment=NARRIVY_DELIVERY_FAIL=0
Environment=NARRIVY_TRANSCRIBE_RUNTIME=installed
Environment=NARRIVY_TRANSCRIBE_MODEL_ID=whisper-tiny.en-Q8_0
UnsetEnvironment=NARRIVY_TRANSCRIBE_MODEL
UnsetEnvironment=NARRIVY_TRANSCRIBE_MODEL_SHA256
UnsetEnvironment=NARRIVY_TRANSCRIBE_MODEL_SIZE
UnsetEnvironment=NARRIVY_TRANSCRIBE_WORKER
UnsetEnvironment=NARRIVY_TRANSCRIBE_WORKER_SHA256
UnsetEnvironment=NARRIVY_TRANSCRIBE_WORKER_SIZE
UnsetEnvironment=TRANSCRIBE_CPP_BIN
UnsetEnvironment=NARRIVY_ALLOW_VIRTUAL_MIC
UnsetEnvironment=NARRIVY_CLIPBOARD_AUTHORIZED
ExecStart=/usr/bin/narrivyd
Restart=on-failure
RestartPreventExitStatus=1 2
RestartSec=2s
KillMode=control-group
TimeoutStopSec=3s
TimeoutStopFailureMode=kill
SendSIGKILL=yes
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=read-only
UMask=0077
CapabilityBoundingSet=
RestrictAddressFamilies=AF_UNIX
RuntimeDirectory=narrivy
RuntimeDirectoryMode=0700
StateDirectory=narrivy
StateDirectoryMode=0700
CacheDirectory=narrivy
CacheDirectoryMode=0700
Environment=XDG_STATE_HOME=%S
Environment=XDG_CACHE_HOME=%C

[Install]
WantedBy=default.target
"""
CANONICAL_DESKTOP_ENTRY = b"""[Desktop Entry]
Name=Narrivy
Comment=Local-first voice dictation
Exec=narrivy-desktop
Icon=narrivy
Terminal=false
Type=Application
Categories=Utility;AudioVideo;
StartupWMClass=narrivy-desktop
Keywords=dictation;voice;stt;whisper;asr;
X-GNOME-UsesNotifications=true
"""


class CandidateError(RuntimeError):
    """Content-free disposable package-composition failure."""


@dataclass(frozen=True)
class CandidateInputs:
    narrivyd: Path
    cli: Path
    desktop: Path
    worker: Path
    service: Path
    desktop_entry: Path
    icon_32: Path
    icon_128: Path
    icon_256: Path
    icon_512: Path
    license_file: Path
    notices: Path
    source_head: str
    source_date_epoch: int


@dataclass(frozen=True)
class ExpectedFile:
    mode: int
    size_bytes: int
    sha256: str


@dataclass(frozen=True)
class TarEntry:
    kind: str
    mode: int
    uid: int
    gid: int
    mtime: int
    size_bytes: int
    sha256: str
    payload: bytes


def _metadata_identity(value: os.stat_result) -> tuple[int, ...]:
    return (
        value.st_dev,
        value.st_ino,
        value.st_mode,
        value.st_uid,
        value.st_gid,
        value.st_size,
        value.st_mtime_ns,
        value.st_ctime_ns,
    )


def _read_asset(path: Path, *, maximum: int = MAX_ASSET_BYTES) -> bytes:
    if not path.is_absolute():
        raise CandidateError("candidate input must be absolute")
    try:
        if path.parent.resolve(strict=True) != path.parent:
            raise CandidateError("candidate input ancestry may not contain symlinks")
        observed = path.lstat()
    except OSError as error:
        raise CandidateError("candidate input is unavailable") from error
    if not stat.S_ISREG(observed.st_mode) or stat.S_ISLNK(observed.st_mode):
        raise CandidateError("candidate input must be a regular non-symlink file")
    if observed.st_uid != os.geteuid() or observed.st_mode & 0o6022:
        raise CandidateError("candidate input ownership or mode is unsafe")
    if observed.st_size <= 0 or observed.st_size > maximum:
        raise CandidateError("candidate input size is invalid")
    descriptor = -1
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW)
        opened = os.fstat(descriptor)
        if _metadata_identity(opened) != _metadata_identity(observed):
            raise CandidateError("candidate input identity changed")
        chunks: list[bytes] = []
        total = 0
        while True:
            chunk = os.read(descriptor, min(64 * 1024, maximum + 1 - total))
            if not chunk:
                break
            chunks.append(chunk)
            total += len(chunk)
            if total > maximum:
                raise CandidateError("candidate input size is invalid")
        final = os.fstat(descriptor)
        if _metadata_identity(final) != _metadata_identity(opened) or total != opened.st_size:
            raise CandidateError("candidate input changed while reading")
        return b"".join(chunks)
    finally:
        if descriptor >= 0:
            os.close(descriptor)


def _write_file(path: Path, payload: bytes, mode: int) -> None:
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o755)
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
                raise CandidateError("candidate staging write failed")
            view = view[written:]
        os.fchmod(descriptor, mode)
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def _open_directory_chain(root_descriptor: int, relative: PurePosixPath) -> int:
    current = os.dup(root_descriptor)
    try:
        for component in relative.parts:
            if component in ("", ".", "..") or "/" in component:
                raise CandidateError("candidate staging directory is invalid")
            created = False
            try:
                os.mkdir(component, mode=0o755, dir_fd=current)
                created = True
            except FileExistsError:
                pass
            following = os.open(
                component,
                os.O_RDONLY | os.O_CLOEXEC | os.O_DIRECTORY | os.O_NOFOLLOW,
                dir_fd=current,
            )
            metadata = os.fstat(following)
            if created:
                os.fchmod(following, 0o755)
                metadata = os.fstat(following)
            if metadata.st_uid != os.geteuid() or stat.S_IMODE(metadata.st_mode) != 0o755:
                os.close(following)
                raise CandidateError("candidate staging directory is unsafe")
            os.close(current)
            current = following
        return current
    except Exception:
        os.close(current)
        raise


def _write_file_at(
    root_descriptor: int,
    relative: PurePosixPath,
    payload: bytes,
    mode: int,
) -> None:
    parent = _open_directory_chain(root_descriptor, relative.parent)
    descriptor = -1
    try:
        descriptor = os.open(
            relative.name,
            os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC | os.O_NOFOLLOW,
            mode,
            dir_fd=parent,
        )
        view = memoryview(payload)
        while view:
            written = os.write(descriptor, view)
            if written <= 0:
                raise CandidateError("candidate staging write failed")
            view = view[written:]
        os.fchmod(descriptor, mode)
        os.fsync(descriptor)
    finally:
        if descriptor >= 0:
            os.close(descriptor)
        os.close(parent)


def _copy_release_elf(
    source_path: Path,
    root_descriptor: int,
    relative: PurePosixPath,
) -> dict[str, object]:
    source, source_metadata = runtime_staging._open_worker(source_path)
    if source_metadata.st_size > MAX_BINARY_BYTES:
        os.close(source)
        raise CandidateError("release binary is oversized")
    digest = hashlib.sha256()
    prefix = bytearray()
    copied = 0
    output = -1
    parent = -1
    try:
        parent = _open_directory_chain(root_descriptor, relative.parent)
        output = os.open(
            relative.name,
            os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC | os.O_NOFOLLOW,
            0o755,
            dir_fd=parent,
        )
        while True:
            chunk = os.read(source, 64 * 1024)
            if not chunk:
                break
            if len(prefix) < runtime_staging.ELF64_HEADER_BYTES:
                take = min(runtime_staging.ELF64_HEADER_BYTES - len(prefix), len(chunk))
                prefix.extend(chunk[:take])
            copied += len(chunk)
            if copied > source_metadata.st_size or copied > MAX_BINARY_BYTES:
                raise CandidateError("release binary size changed")
            digest.update(chunk)
            view = memoryview(chunk)
            while view:
                written = os.write(output, view)
                if written <= 0:
                    raise CandidateError("release binary staging write failed")
                view = view[written:]
        if copied != source_metadata.st_size:
            raise CandidateError("release binary size changed")
        runtime_staging._validate_amd64_elf(source, prefix, copied)
        final = os.fstat(source)
        if _metadata_identity(final) != _metadata_identity(source_metadata):
            raise CandidateError("release binary changed during staging")
        os.fchmod(output, 0o755)
        os.fsync(output)
        return {"sha256": digest.hexdigest(), "size_bytes": copied}
    finally:
        if output >= 0:
            os.close(output)
        if parent >= 0:
            os.close(parent)
        os.close(source)


def _validate_service(payload: bytes) -> None:
    if payload != CANONICAL_SERVICE:
        raise CandidateError("candidate service is not the exact fail-closed composition")


def _validate_desktop_entry(payload: bytes) -> None:
    if payload != CANONICAL_DESKTOP_ENTRY:
        raise CandidateError("desktop entry is not the exact packaged client entry")


def _validate_png(payload: bytes, dimension: int) -> None:
    if (
        len(payload) < 33
        or payload[:8] != b"\x89PNG\r\n\x1a\n"
        or payload[12:16] != b"IHDR"
    ):
        raise CandidateError("candidate icon is not a bounded PNG")
    width, height = struct.unpack(">II", payload[16:24])
    if width != dimension or height != dimension:
        raise CandidateError("candidate icon dimensions are wrong")


def _validate_legal_text(license_payload: bytes, notices: bytes) -> None:
    for payload in (license_payload, notices):
        try:
            text = payload.decode("utf-8")
        except UnicodeDecodeError as error:
            raise CandidateError("candidate legal input must be UTF-8") from error
        if len(text.strip()) < 64 or re.search(r"\b(stub|placeholder)\b", text, re.I):
            raise CandidateError("candidate legal input is incomplete")


def _validate_tool(path: Path) -> None:
    metadata = path.stat()
    if (
        path != DPKG_DEB
        or not stat.S_ISREG(metadata.st_mode)
        or metadata.st_uid != 0
        or stat.S_IMODE(metadata.st_mode) != 0o755
    ):
        raise CandidateError("dpkg-deb is not the fixed root-owned tool")


def _canonical_json(value: object) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()


def _expected_file(mode: int, payload: bytes) -> ExpectedFile:
    return ExpectedFile(mode, len(payload), hashlib.sha256(payload).hexdigest())


def _normalize_tree(root: Path, epoch: int) -> None:
    for path in sorted(root.rglob("*"), key=lambda item: len(item.parts), reverse=True):
        if path.is_symlink():
            raise CandidateError("candidate tree may not contain symlinks")
        os.utime(path, (epoch, epoch), follow_symlinks=False)
        if path.is_dir():
            path.chmod(0o755)
    os.utime(root, (epoch, epoch), follow_symlinks=False)
    root.chmod(0o700)


def _normalize_anchored_tree(
    root_descriptor: int,
    expected_files: dict[str, ExpectedFile],
    epoch: int,
) -> None:
    directories: set[PurePosixPath] = set()
    for name in expected_files:
        directories.update(
            parent for parent in PurePosixPath(name).parents if str(parent) != "."
        )
    directories.add(PurePosixPath("DEBIAN"))
    for directory in sorted(directories, key=lambda value: len(value.parts), reverse=True):
        descriptor = _open_directory_chain(root_descriptor, directory)
        try:
            os.fchmod(descriptor, 0o755)
            os.utime(descriptor, (epoch, epoch))
        finally:
            os.close(descriptor)
    for name, expected in expected_files.items():
        relative = PurePosixPath(name)
        parent = _open_directory_chain(root_descriptor, relative.parent)
        descriptor = -1
        try:
            descriptor = os.open(
                relative.name,
                os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW,
                dir_fd=parent,
            )
            metadata = os.fstat(descriptor)
            if not stat.S_ISREG(metadata.st_mode):
                raise CandidateError("candidate staging leaf is unsafe")
            os.fchmod(descriptor, expected.mode)
            os.utime(descriptor, (epoch, epoch))
        finally:
            if descriptor >= 0:
                os.close(descriptor)
            os.close(parent)
    os.fchmod(root_descriptor, 0o700)
    os.utime(root_descriptor, (epoch, epoch))


def _subreaper_state() -> int:
    value = ctypes.c_int()
    libc = ctypes.CDLL(None, use_errno=True)
    if libc.prctl(37, ctypes.byref(value), 0, 0, 0) != 0:
        raise OSError(ctypes.get_errno(), "PR_GET_CHILD_SUBREAPER failed")
    return value.value


def _set_subreaper(enabled: int) -> None:
    libc = ctypes.CDLL(None, use_errno=True)
    if libc.prctl(36, enabled, 0, 0, 0) != 0:
        raise OSError(ctypes.get_errno(), "PR_SET_CHILD_SUBREAPER failed")


def _direct_children() -> set[int]:
    path = Path(f"/proc/self/task/{os.getpid()}/children")
    try:
        content = path.read_text(encoding="ascii").strip()
    except FileNotFoundError:
        return set()
    return {int(value) for value in content.split()} if content else set()


def _drain_escaped_children(baseline: set[int]) -> bool:
    escaped = False
    for _ in range(32):
        children = _direct_children() - baseline
        if not children:
            return escaped
        escaped = True
        for child in children:
            try:
                os.kill(child, signal.SIGKILL)
            except ProcessLookupError:
                pass
        for child in children:
            try:
                os.waitpid(child, 0)
            except (ChildProcessError, ProcessLookupError):
                pass
    raise CandidateError("installed daemon negative probe descendant cleanup was unbounded")


def _probe_installed_daemon(narrivyd_descriptor: int) -> None:
    """Require the strict installed-runtime lane to fail once on an empty cache."""

    with tempfile.TemporaryDirectory(prefix="narrivy-daemon-negative-probe-") as temporary:
        private = Path(temporary)
        private.chmod(0o700)
        directories = {
            name: private / name for name in ("home", "runtime", "state", "cache", "config")
        }
        for path in directories.values():
            path.mkdir(mode=0o700)
        environment = {
            "HOME": str(directories["home"]),
            "LANG": "C.UTF-8",
            "LC_ALL": "C.UTF-8",
            "PATH": "/usr/bin:/bin",
            "TZ": "UTC",
            "XDG_CACHE_HOME": str(directories["cache"]),
            "XDG_CONFIG_HOME": str(directories["config"]),
            "XDG_RUNTIME_DIR": str(directories["runtime"]),
            "XDG_STATE_HOME": str(directories["state"]),
            "NARRIVY_RECOVERY_MODE": "linux-mic-inbox",
            "NARRIVY_DEFAULT_PIPELINE": "clean",
            "NARRIVY_ASR_ENGINE": "transcribe-cpp",
            "NARRIVY_AUDIO_SOURCE": "pipewire",
            "NARRIVY_DELIVERY_BACKEND": "inbox",
            "NARRIVY_DELIVERY_FAIL": "0",
            "NARRIVY_TRANSCRIBE_RUNTIME": "installed",
            "NARRIVY_TRANSCRIBE_MODEL_ID": "whisper-tiny.en-Q8_0",
        }
        stdout_path = private / "stdout"
        stderr_path = private / "stderr"
        stdout_file = os.fdopen(
            os.open(stdout_path, os.O_RDWR | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC, 0o600),
            "w+b",
        )
        stderr_file = os.fdopen(
            os.open(stderr_path, os.O_RDWR | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC, 0o600),
            "w+b",
        )

        def limit_output() -> None:
            # Bounds any single file the process writes, so a daemon that sprays
            # diagnostics cannot fill the disk. It is deliberately generous
            # enough not to bound legitimate state writes: composing the
            # optional History store writes a SQLite page, and at 4 KiB the
            # limit killed the process with SIGXFSZ before it could reach the
            # failure under test. The real bound on output is the exact
            # stdout/stderr comparison below, which is not loosened.
            resource.setrlimit(resource.RLIMIT_FSIZE, (64 * 1024, 64 * 1024))

        original_subreaper = _subreaper_state()
        if not original_subreaper:
            _set_subreaper(1)
        baseline_children = _direct_children()
        process: subprocess.Popen[bytes] | None = None
        escaped_descendant = False
        try:
            process = subprocess.Popen(
                [f"/proc/self/fd/{narrivyd_descriptor}"],
                stdin=subprocess.DEVNULL,
                stdout=stdout_file,
                stderr=stderr_file,
                env=environment,
                start_new_session=True,
                pass_fds=(narrivyd_descriptor,),
                preexec_fn=limit_output,
            )
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired as error:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()
                raise CandidateError("installed daemon negative probe timed out") from error

            group_survived = False
            try:
                os.killpg(process.pid, 0)
                group_survived = True
            except ProcessLookupError:
                pass
            if group_survived:
                os.killpg(process.pid, signal.SIGKILL)
                raise CandidateError("installed daemon negative probe left a child process")

            stdout_file.flush()
            stderr_file.flush()
            if os.fstat(stdout_file.fileno()).st_size > 4096 or os.fstat(
                stderr_file.fileno()
            ).st_size > 4096:
                raise CandidateError("installed daemon negative probe output exceeded its bound")
            stdout_file.seek(0)
            stderr_file.seek(0)
            stdout = stdout_file.read()
            stderr = stderr_file.read()
            expected = b"narrivyd: installed resident runtime failed verification\n"
            if process.returncode != 1 or stdout != b"" or stderr != expected:
                raise CandidateError("installed daemon negative probe was not fail-closed")

            for path in private.rglob("*"):
                if stat.S_ISSOCK(path.lstat().st_mode):
                    raise CandidateError("installed daemon negative probe left an IPC socket")
        finally:
            if process is not None and process.poll() is None:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()
            try:
                escaped_descendant = _drain_escaped_children(baseline_children)
            finally:
                if not original_subreaper:
                    _set_subreaper(0)
                stdout_file.close()
                stderr_file.close()
        if escaped_descendant:
            raise CandidateError("installed daemon negative probe left an escaped descendant")


def _snapshot_executable(
    root_descriptor: int,
    relative: PurePosixPath,
    expected: dict[str, object],
) -> int:
    parent = _open_directory_chain(root_descriptor, relative.parent)
    observed = os.stat(relative.name, dir_fd=parent, follow_symlinks=False)
    if not stat.S_ISREG(observed.st_mode) or stat.S_IMODE(observed.st_mode) != 0o755:
        os.close(parent)
        raise CandidateError("staged daemon identity is wrong")
    source = os.open(
        relative.name,
        os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW,
        dir_fd=parent,
    )
    os.close(parent)
    snapshot = -1
    try:
        opened = os.fstat(source)
        if _metadata_identity(opened) != _metadata_identity(observed):
            raise CandidateError("staged daemon identity changed")
        snapshot = os.memfd_create(
            "narrivy-daemon-candidate-snapshot",
            os.MFD_CLOEXEC | os.MFD_ALLOW_SEALING,
        )
        digest = hashlib.sha256()
        size = 0
        while True:
            chunk = os.read(source, 64 * 1024)
            if not chunk:
                break
            size += len(chunk)
            if size > MAX_BINARY_BYTES:
                raise CandidateError("staged daemon snapshot is oversized")
            digest.update(chunk)
            view = memoryview(chunk)
            while view:
                written = os.write(snapshot, view)
                if written <= 0:
                    raise CandidateError("staged daemon snapshot write failed")
                view = view[written:]
        final = os.fstat(source)
        if (
            _metadata_identity(final) != _metadata_identity(opened)
            or size != expected["size_bytes"]
            or digest.hexdigest() != expected["sha256"]
        ):
            raise CandidateError("staged daemon digest identity is wrong")
        os.fchmod(snapshot, 0o500)
        fcntl.fcntl(
            snapshot,
            fcntl.F_ADD_SEALS,
            fcntl.F_SEAL_WRITE | fcntl.F_SEAL_GROW | fcntl.F_SEAL_SHRINK | fcntl.F_SEAL_SEAL,
        )
        os.lseek(snapshot, 0, os.SEEK_SET)
        return snapshot
    except Exception:
        if snapshot >= 0:
            os.close(snapshot)
        raise
    finally:
        os.close(source)


def _tar_members(
    command: list[str],
    archive_descriptor: int,
    expected_directories: set[str],
    expected_file_sizes: dict[str, int],
    capture_payloads: set[str],
) -> dict[str, TarEntry]:
    if not capture_payloads.issubset(expected_file_sizes):
        raise CandidateError("candidate archive capture policy is invalid")
    expanded_limit = sum(expected_file_sizes.values())
    with tempfile.TemporaryFile() as errors:
        process = subprocess.Popen(
            command,
            stdout=subprocess.PIPE,
            stderr=errors,
            env={"PATH": "/usr/bin:/bin", "LC_ALL": "C", "TZ": "UTC"},
            pass_fds=(archive_descriptor,),
        )
        assert process.stdout is not None
        entries: dict[str, TarEntry] = {}
        parsing_error: BaseException | None = None
        expanded = 0
        try:
            with tarfile.open(fileobj=process.stdout, mode="r|") as archive:
                for member in archive:
                    normalized = member.name.removeprefix("./") or "."
                    path = PurePosixPath(normalized)
                    if path.is_absolute() or ".." in path.parts:
                        raise CandidateError("candidate archive path is unsafe")
                    if normalized in entries:
                        raise CandidateError("candidate archive contains a duplicate member")
                    if member.isdir():
                        if normalized not in expected_directories or member.size != 0:
                            raise CandidateError("candidate archive directory is unexpected")
                        kind = "directory"
                        payload = b""
                        digest = hashlib.sha256(payload).hexdigest()
                        size = 0
                    elif member.isfile():
                        if (
                            normalized not in expected_file_sizes
                            or member.size != expected_file_sizes[normalized]
                            or member.size < 0
                        ):
                            raise CandidateError("candidate archive file size is unexpected")
                        kind = "file"
                        extracted = archive.extractfile(member)
                        if extracted is None:
                            raise CandidateError("candidate archive member is unreadable")
                        hasher = hashlib.sha256()
                        chunks: list[bytes] = []
                        size = 0
                        while size < member.size:
                            chunk = extracted.read(min(64 * 1024, member.size - size))
                            if not chunk:
                                break
                            size += len(chunk)
                            expanded += len(chunk)
                            if expanded > expanded_limit:
                                raise CandidateError("candidate archive expansion exceeded its bound")
                            hasher.update(chunk)
                            if normalized in capture_payloads:
                                chunks.append(chunk)
                        if size != member.size or extracted.read(1):
                            raise CandidateError("candidate archive member length is wrong")
                        digest = hasher.hexdigest()
                        payload = b"".join(chunks)
                    else:
                        raise CandidateError("candidate archive contains an unsafe member")
                    entries[normalized] = TarEntry(
                        kind,
                        stat.S_IMODE(member.mode),
                        member.uid,
                        member.gid,
                        int(member.mtime),
                        size,
                        digest,
                        payload,
                    )
        except BaseException as error:
            parsing_error = error
        finally:
            process.stdout.close()
        return_code = process.wait()
        errors.seek(0)
        stderr = errors.read(256)
    if parsing_error is not None:
        raise parsing_error
    if return_code != 0:
        raise CandidateError("dpkg-deb archive inspection failed") from RuntimeError(
            stderr.decode("utf-8", errors="replace")
        )
    return entries


def _inspect_candidate(
    archive_descriptor: int,
    expected_files: dict[str, ExpectedFile],
    control_payload: bytes,
    preinst_payload: bytes,
    epoch: int,
) -> None:
    descriptor_path = f"/proc/self/fd/{archive_descriptor}"
    expected_directories = {"."}
    for name in expected_files:
        expected_directories.update(
            str(parent) for parent in PurePosixPath(name).parents if str(parent) != "."
        )
    data = _tar_members(
        [str(DPKG_DEB), "--fsys-tarfile", descriptor_path],
        archive_descriptor,
        expected_directories,
        {name: expected.size_bytes for name, expected in expected_files.items()},
        set(),
    )
    if set(data) != set(expected_files) | expected_directories:
        raise CandidateError("candidate archive payload membership is not exact")
    for name in expected_directories:
        entry = data[name]
        expected_mode = 0o700 if name == "." else 0o755
        if (
            entry.kind,
            entry.mode,
            entry.uid,
            entry.gid,
            entry.mtime,
            entry.size_bytes,
            entry.sha256,
            entry.payload,
        ) != (
            "directory",
            expected_mode,
            0,
            0,
            epoch,
            0,
            hashlib.sha256(b"").hexdigest(),
            b"",
        ):
            raise CandidateError("candidate archive directory identity is wrong")
    for name, expected in expected_files.items():
        entry = data[name]
        if (
            entry.kind != "file"
            or entry.mode != expected.mode
            or entry.uid != 0
            or entry.gid != 0
            or entry.mtime != epoch
            or entry.size_bytes != expected.size_bytes
            or entry.sha256 != expected.sha256
            or entry.payload != b""
        ):
            raise CandidateError("candidate archive payload identity is wrong")

    control = _tar_members(
        [str(DPKG_DEB), "--ctrl-tarfile", descriptor_path],
        archive_descriptor,
        {"."},
        {"control": len(control_payload), "preinst": len(preinst_payload)},
        {"control", "preinst"},
    )
    if set(control) != {".", "control", "preinst"}:
        raise CandidateError("candidate control membership is not exact")
    expected_control = {
        ".": ("directory", 0o755, b""),
        "control": ("file", 0o644, control_payload),
        "preinst": ("file", 0o755, preinst_payload),
    }
    for name, (kind, mode, payload) in expected_control.items():
        entry = control[name]
        if (
            entry.kind,
            entry.mode,
            entry.uid,
            entry.gid,
            entry.mtime,
            entry.size_bytes,
            entry.sha256,
            entry.payload,
        ) != (
            kind,
            mode,
            0,
            0,
            epoch,
            len(payload),
            hashlib.sha256(payload).hexdigest(),
            payload,
        ):
            raise CandidateError("candidate control identity is wrong")


def _snapshot_archive(path: Path) -> tuple[int, str, int]:
    observed = path.lstat()
    if not stat.S_ISREG(observed.st_mode) or stat.S_ISLNK(observed.st_mode):
        raise CandidateError("candidate archive output is unsafe")
    if observed.st_size <= 0 or observed.st_size > MAX_ARCHIVE_BYTES:
        raise CandidateError("candidate archive output size is invalid")
    source = os.open(path, os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW)
    try:
        if _metadata_identity(os.fstat(source)) != _metadata_identity(observed):
            raise CandidateError("candidate archive output identity changed")
        return _snapshot_archive_descriptor(source)
    finally:
        os.close(source)


def _snapshot_archive_descriptor(source: int) -> tuple[int, str, int]:
    opened = os.fstat(source)
    if (
        not stat.S_ISREG(opened.st_mode)
        or opened.st_uid != os.geteuid()
        or opened.st_size <= 0
        or opened.st_size > MAX_ARCHIVE_BYTES
    ):
        raise CandidateError("candidate archive descriptor is unsafe")
    snapshot = -1
    try:
        snapshot = os.memfd_create(
            "narrivy-deb-candidate-snapshot",
            os.MFD_CLOEXEC | os.MFD_ALLOW_SEALING,
        )
        digest = hashlib.sha256()
        size = 0
        offset = 0
        while True:
            chunk = os.pread(source, 64 * 1024, offset)
            if not chunk:
                break
            digest.update(chunk)
            size += len(chunk)
            offset += len(chunk)
            if size > MAX_ARCHIVE_BYTES:
                raise CandidateError("candidate archive output size is invalid")
            view = memoryview(chunk)
            while view:
                written = os.write(snapshot, view)
                if written <= 0:
                    raise CandidateError("candidate archive snapshot write failed")
                view = view[written:]
        final = os.fstat(source)
        if _metadata_identity(final) != _metadata_identity(opened) or size != opened.st_size:
            raise CandidateError("candidate archive output changed during snapshot")
        fcntl.fcntl(
            snapshot,
            fcntl.F_ADD_SEALS,
            fcntl.F_SEAL_WRITE | fcntl.F_SEAL_GROW | fcntl.F_SEAL_SHRINK | fcntl.F_SEAL_SEAL,
        )
        os.lseek(snapshot, 0, os.SEEK_SET)
        return snapshot, digest.hexdigest(), size
    except Exception:
        if snapshot >= 0:
            os.close(snapshot)
        raise


def compose_disposable_candidate(inputs: CandidateInputs) -> dict[str, object]:
    """Build, inspect, hash, and remove one non-installable candidate archive."""

    if not re.fullmatch(r"[0-9a-f]{40}", inputs.source_head):
        raise CandidateError("source HEAD must be lowercase 40-character hex")
    if inputs.source_date_epoch <= 0:
        raise CandidateError("SOURCE_DATE_EPOCH must be positive")
    _validate_tool(DPKG_DEB)

    service = _read_asset(inputs.service)
    desktop_entry = _read_asset(inputs.desktop_entry)
    _validate_service(service)
    _validate_desktop_entry(desktop_entry)
    icon_inputs = {
        32: _read_asset(inputs.icon_32),
        128: _read_asset(inputs.icon_128),
        256: _read_asset(inputs.icon_256),
        512: _read_asset(inputs.icon_512),
    }
    for dimension, payload in icon_inputs.items():
        _validate_png(payload, dimension)
    icons = {
        f"usr/share/icons/hicolor/{dimension}x{dimension}/apps/narrivy.png": payload
        for dimension, payload in icon_inputs.items()
    }
    license_payload = _read_asset(inputs.license_file)
    notices = _read_asset(inputs.notices)
    _validate_legal_text(license_payload, notices)

    with tempfile.TemporaryDirectory(prefix="narrivy-deb-candidate-") as temporary:
        private = Path(temporary)
        private.chmod(0o700)
        package_root = private / "package-root"
        runtime_manifest = runtime_staging.stage_installed_runtime(
            package_root,
            inputs.worker,
        )
        staged_root_identity = getattr(runtime_manifest, "root_identity", None)
        if staged_root_identity is None:
            raise CandidateError("runtime stager did not bind its root identity")
        root_observed = package_root.lstat()
        if (
            not stat.S_ISDIR(root_observed.st_mode)
            or stat.S_ISLNK(root_observed.st_mode)
            or root_observed.st_uid != os.geteuid()
            or stat.S_IMODE(root_observed.st_mode) != 0o700
        ):
            raise CandidateError("candidate package root is unsafe")
        if (root_observed.st_dev, root_observed.st_ino) != staged_root_identity:
            raise CandidateError("candidate package root changed after runtime staging")
        root_descriptor = os.open(
            package_root,
            os.O_RDONLY | os.O_CLOEXEC | os.O_DIRECTORY | os.O_NOFOLLOW,
        )
        try:
            if _metadata_identity(os.fstat(root_descriptor)) != _metadata_identity(root_observed):
                raise CandidateError("candidate package root identity changed")
            binary_identities = {
                "narrivyd": _copy_release_elf(
                    inputs.narrivyd, root_descriptor, PurePosixPath("usr/bin/narrivyd")
                ),
                "narrivy-cli": _copy_release_elf(
                    inputs.cli, root_descriptor, PurePosixPath("usr/bin/narrivy-cli")
                ),
                "narrivy-desktop": _copy_release_elf(
                    inputs.desktop,
                    root_descriptor,
                    PurePosixPath("usr/bin/narrivy-desktop"),
                ),
            }
            staged_daemon = _snapshot_executable(
                root_descriptor,
                PurePosixPath("usr/bin/narrivyd"),
                binary_identities["narrivyd"],
            )
            try:
                _probe_installed_daemon(staged_daemon)
            finally:
                os.close(staged_daemon)

            build_info = _canonical_json(
                {
                    "classification": "non-release-disposable-candidate",
                    "git_dirty": True,
                    "package": PACKAGE_NAME,
                    "schema_version": 1,
                    "source_date_epoch": inputs.source_date_epoch,
                    "source_head": inputs.source_head,
                    "version": PACKAGE_VERSION,
                    "inputs": {
                        **binary_identities,
                        "resident-worker": runtime_manifest["worker"],
                    },
                }
            )
            runtime_manifest_payload = _canonical_json(runtime_manifest)
            expected_files: dict[str, ExpectedFile] = {
                "usr/bin/narrivyd": ExpectedFile(0o755, **binary_identities["narrivyd"]),
                "usr/bin/narrivy-cli": ExpectedFile(
                    0o755, **binary_identities["narrivy-cli"]
                ),
                "usr/bin/narrivy-desktop": ExpectedFile(
                    0o755, **binary_identities["narrivy-desktop"]
                ),
                runtime_staging.WORKER_RELATIVE.as_posix(): ExpectedFile(
                    0o755, **runtime_manifest["worker"]
                ),
                runtime_staging.MANIFEST_RELATIVE.as_posix(): _expected_file(
                    0o644, runtime_manifest_payload
                ),
                "usr/lib/systemd/user/narrivy-daemon.service": _expected_file(0o644, service),
                "usr/share/applications/narrivy.desktop": _expected_file(
                    0o644, desktop_entry
                ),
                "usr/share/narrivy/candidate-build-info.json": _expected_file(
                    0o644, build_info
                ),
                "usr/share/doc/narrivy/copyright": _expected_file(0o644, license_payload),
                "usr/share/doc/narrivy/THIRD_PARTY_NOTICES.md": _expected_file(
                    0o644, notices
                ),
                **{name: _expected_file(0o644, payload) for name, payload in icons.items()},
            }
            write_payloads = {
                "usr/lib/systemd/user/narrivy-daemon.service": (0o644, service),
                "usr/share/applications/narrivy.desktop": (0o644, desktop_entry),
                "usr/share/narrivy/candidate-build-info.json": (0o644, build_info),
                "usr/share/doc/narrivy/copyright": (0o644, license_payload),
                "usr/share/doc/narrivy/THIRD_PARTY_NOTICES.md": (0o644, notices),
                **{name: (0o644, payload) for name, payload in icons.items()},
            }
            for name, (mode, payload) in write_payloads.items():
                _write_file_at(root_descriptor, PurePosixPath(name), payload, mode)

            control = (
                f"Package: {PACKAGE_NAME}\n"
                f"Version: {PACKAGE_VERSION}\n"
                "Section: utils\nPriority: optional\nArchitecture: amd64\n"
                "Maintainer: Narrivy Recovery <release@narrivy.invalid>\n"
                "Depends: pipewire, xdg-utils\n"
                "Description: Narrivy disposable recovery package candidate\n"
                " This archive is deliberately install-blocked and is not a release artifact.\n"
            ).encode()
            preinst = (
                "#!/bin/sh\n"
                "echo 'Narrivy recovery candidate: installation is prohibited' >&2\n"
                "exit 1\n"
            ).encode()
            _write_file_at(
                root_descriptor, PurePosixPath("DEBIAN/control"), control, 0o644
            )
            _write_file_at(
                root_descriptor, PurePosixPath("DEBIAN/preinst"), preinst, 0o755
            )
            normalization_files = {
                **expected_files,
                "DEBIAN/control": _expected_file(0o644, control),
                "DEBIAN/preinst": _expected_file(0o755, preinst),
            }
            _normalize_anchored_tree(
                root_descriptor, normalization_files, inputs.source_date_epoch
            )

            current_root = package_root.lstat()
            if _metadata_identity(current_root) != _metadata_identity(os.fstat(root_descriptor)):
                raise CandidateError("candidate package root moved before construction")

            archive = private / "candidate.deb"
            archive_output = os.open(
                archive,
                os.O_RDWR
                | os.O_CREAT
                | os.O_EXCL
                | os.O_CLOEXEC
                | os.O_NOFOLLOW,
                0o600,
            )
            try:
                archive_metadata = os.fstat(archive_output)
                if (
                    not stat.S_ISREG(archive_metadata.st_mode)
                    or archive_metadata.st_uid != os.geteuid()
                    or archive_metadata.st_nlink != 1
                    or stat.S_IMODE(archive_metadata.st_mode) != 0o600
                ):
                    raise CandidateError("candidate archive output descriptor is unsafe")
                result = subprocess.run(
                    [
                        str(DPKG_DEB),
                        "-Zxz",
                        "-z9",
                        "--root-owner-group",
                        "--build",
                        str(package_root),
                        f"/proc/self/fd/{archive_output}",
                    ],
                    stdin=subprocess.DEVNULL,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    env={
                        "PATH": "/usr/bin:/bin",
                        "LC_ALL": "C",
                        "TZ": "UTC",
                        "SOURCE_DATE_EPOCH": str(inputs.source_date_epoch),
                    },
                    pass_fds=(archive_output,),
                    timeout=60,
                    check=False,
                )
                if result.returncode != 0:
                    raise CandidateError(
                        "dpkg-deb candidate construction failed"
                    ) from RuntimeError(result.stderr.decode("utf-8", errors="replace")[:256])
                if _metadata_identity(package_root.lstat()) != _metadata_identity(
                    os.fstat(root_descriptor)
                ):
                    raise CandidateError("candidate package root moved during construction")
                archive_snapshot, archive_sha256, archive_size = (
                    _snapshot_archive_descriptor(archive_output)
                )
                try:
                    _inspect_candidate(
                        archive_snapshot,
                        expected_files,
                        control,
                        preinst,
                        inputs.source_date_epoch,
                    )
                    return {
                        "archive_sha256": archive_sha256,
                        "archive_size_bytes": archive_size,
                        "classification": "non-release-disposable-candidate",
                        "data_file_count": len(expected_files),
                        "install_blocked": True,
                        "package": PACKAGE_NAME,
                        "version": PACKAGE_VERSION,
                    }
                finally:
                    os.close(archive_snapshot)
            finally:
                os.close(archive_output)
        finally:
            os.close(root_descriptor)


def _arguments(argv: list[str] | None) -> CandidateInputs:
    parser = argparse.ArgumentParser()
    for name in (
        "narrivyd",
        "cli",
        "desktop",
        "worker",
        "service",
        "desktop-entry",
        "icon-32",
        "icon-128",
        "icon-256",
        "icon-512",
        "license-file",
        "notices",
    ):
        parser.add_argument(f"--{name}", required=True, type=Path)
    parser.add_argument("--source-head", required=True)
    parser.add_argument("--source-date-epoch", required=True, type=int)
    values = parser.parse_args(argv)
    return CandidateInputs(
        narrivyd=values.narrivyd,
        cli=values.cli,
        desktop=values.desktop,
        worker=values.worker,
        service=values.service,
        desktop_entry=values.desktop_entry,
        icon_32=values.icon_32,
        icon_128=values.icon_128,
        icon_256=values.icon_256,
        icon_512=values.icon_512,
        license_file=values.license_file,
        notices=values.notices,
        source_head=values.source_head,
        source_date_epoch=values.source_date_epoch,
    )


def main(argv: list[str] | None = None) -> int:
    try:
        summary = compose_disposable_candidate(_arguments(argv))
    except (CandidateError, OSError, subprocess.SubprocessError) as error:
        print(f"build_deb_candidate: {error}", file=sys.stderr)
        return 1
    print(json.dumps(summary, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
