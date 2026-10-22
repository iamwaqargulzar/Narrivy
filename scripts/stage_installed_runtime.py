#!/usr/bin/env python3
"""Stage Narrivy's fixed installed resident runtime into a package root.

This creates package input only. It does not build a package, establish dpkg
membership, generate evidence, or assert release provenance.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import stat
import struct
import sys
from pathlib import Path

MAX_WORKER_BYTES = 256 * 1024 * 1024
WORKER_RELATIVE = Path("usr/lib/narrivy/narrivy-transcribe-worker")
MANIFEST_RELATIVE = Path("usr/share/narrivy/installed-runtime-manifest.json")
REPOSITORY_ROOT = Path(__file__).resolve().parents[1]
PROTECTED_PATHS = {
    REPOSITORY_ROOT / "artifacts/releases/narrivy_0.2.3_amd64.deb",
    REPOSITORY_ROOT / "packaging/deb/BUILD_LOG.md",
}
ELF64_HEADER_BYTES = 64
ELF_MACHINE_X86_64 = 62


class StageError(RuntimeError):
    """Content-free package staging failure."""


class StagedRuntimeManifest(dict[str, object]):
    """Manifest payload carrying a non-serialized root identity handoff."""

    def __init__(self, payload: dict[str, object], root_identity: tuple[int, int]) -> None:
        super().__init__(payload)
        self.root_identity = root_identity


def _secure_directory(root: Path, relative: Path) -> Path:
    current = root
    for component in relative.parts:
        current = current / component
        try:
            current.mkdir(mode=0o755)
        except FileExistsError:
            metadata = current.lstat()
            if not stat.S_ISDIR(metadata.st_mode) or stat.S_ISLNK(metadata.st_mode):
                raise StageError("package staging directory is unsafe") from None
            if metadata.st_uid != os.geteuid() or metadata.st_mode & 0o022:
                raise StageError("package staging directory ownership or mode is unsafe")
        os.chmod(current, 0o755, follow_symlinks=False)
    return current


def _open_worker(path: Path) -> tuple[int, os.stat_result]:
    if not path.is_absolute():
        raise StageError("worker input must be an absolute path")
    try:
        if path.parent.resolve(strict=True) != path.parent:
            raise StageError("worker input ancestry may not contain symlinks")
    except OSError as error:
        raise StageError("worker input ancestry is unavailable") from error
    try:
        path_metadata = path.lstat()
    except OSError as error:
        raise StageError("worker input is unavailable") from error
    if not stat.S_ISREG(path_metadata.st_mode) or stat.S_ISLNK(path_metadata.st_mode):
        raise StageError("worker input must be a regular non-symlink file")
    if path_metadata.st_uid != os.geteuid() or stat.S_IMODE(path_metadata.st_mode) != 0o755:
        raise StageError("worker input ownership or mode is unsafe")
    if path_metadata.st_size <= 0 or path_metadata.st_size > MAX_WORKER_BYTES:
        raise StageError("worker input size is invalid")
    if path_metadata.st_mode & 0o111 == 0:
        raise StageError("worker input is not executable")
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW)
    except OSError as error:
        raise StageError("worker input could not be opened safely") from error
    opened = os.fstat(descriptor)
    identity = lambda value: (
        value.st_dev,
        value.st_ino,
        value.st_mode,
        value.st_uid,
        value.st_gid,
        value.st_size,
        value.st_mtime_ns,
        value.st_ctime_ns,
    )
    if identity(opened) != identity(path_metadata):
        os.close(descriptor)
        raise StageError("worker input identity changed")
    return descriptor, opened


def _validate_amd64_elf(descriptor: int, header: bytes, size: int) -> None:
    if len(header) < ELF64_HEADER_BYTES or header[:4] != b"\x7fELF":
        raise StageError("worker input is not a complete native ELF executable")
    if header[4:7] != b"\x02\x01\x01":
        raise StageError("worker ELF class, byte order, or version is unsupported")
    elf_type, machine, version = struct.unpack_from("<HHI", header, 16)
    program_offset = struct.unpack_from("<Q", header, 32)[0]
    header_size, program_entry_size, program_count = struct.unpack_from("<HHH", header, 52)
    if elf_type not in (2, 3) or machine != ELF_MACHINE_X86_64 or version != 1:
        raise StageError("worker ELF identity is incompatible with the amd64 package")
    if header_size != ELF64_HEADER_BYTES or program_entry_size != 56 or program_count == 0:
        raise StageError("worker ELF program table is invalid")
    program_end = program_offset + program_entry_size * program_count
    if program_offset < ELF64_HEADER_BYTES or program_end > size:
        raise StageError("worker ELF program table is out of bounds")
    executable_load = False
    for index in range(program_count):
        offset = program_offset + index * program_entry_size
        entry = os.pread(descriptor, program_entry_size, offset)
        if len(entry) != program_entry_size:
            raise StageError("worker ELF program table is truncated")
        segment_type, flags, file_offset, _, _, file_size, memory_size, alignment = (
            struct.unpack("<IIQQQQQQ", entry)
        )
        if file_size > memory_size or file_offset + file_size > size:
            raise StageError("worker ELF segment is out of bounds")
        if alignment not in (0, 1) and alignment & (alignment - 1):
            raise StageError("worker ELF segment alignment is invalid")
        if segment_type == 1 and flags & 1 and file_size > 0:
            executable_load = True
    if not executable_load:
        raise StageError("worker ELF has no executable load segment")


def _file_identity(metadata: os.stat_result) -> tuple[int, int]:
    return metadata.st_dev, metadata.st_ino


def _cleanup_created_tree(
    parent_descriptor: int,
    root_descriptor: int,
    root_name: str,
    expected_root: tuple[int, int],
) -> None:
    """Remove only the fixed entries beneath the still-open created root."""

    for relative in (MANIFEST_RELATIVE, WORKER_RELATIVE):
        try:
            os.unlink(relative, dir_fd=root_descriptor)
        except FileNotFoundError:
            pass
    for relative in (
        MANIFEST_RELATIVE.parent,
        Path("usr/share"),
        WORKER_RELATIVE.parent,
        Path("usr/lib"),
        Path("usr"),
    ):
        try:
            os.rmdir(relative, dir_fd=root_descriptor)
        except FileNotFoundError:
            pass
        except OSError:
            # A non-empty fixed directory is not recursively traversed. This
            # prevents cleanup from deleting content not created by this run.
            pass
    try:
        current = os.stat(root_name, dir_fd=parent_descriptor, follow_symlinks=False)
    except FileNotFoundError:
        return
    if _file_identity(current) != expected_root or not stat.S_ISDIR(current.st_mode):
        raise StageError("package stage cleanup identity changed")
    os.rmdir(root_name, dir_fd=parent_descriptor)


def stage_installed_runtime(stage_root: Path, worker: Path) -> dict[str, object]:
    """Copy one exact worker and emit the matching fixed-path manifest."""

    if not stage_root.is_absolute():
        raise StageError("package stage root must be absolute")
    normalized = Path(os.path.abspath(stage_root))
    if normalized == Path("/") or normalized == REPOSITORY_ROOT or normalized in PROTECTED_PATHS:
        raise StageError("package stage root is a forbidden target")
    if normalized.exists() or normalized.is_symlink():
        raise StageError("package stage root must not already exist")
    try:
        parent = normalized.parent.resolve(strict=True)
        parent_metadata = parent.lstat()
    except OSError as error:
        raise StageError("package stage parent is unavailable") from error
    if not stat.S_ISDIR(parent_metadata.st_mode) or stat.S_ISLNK(parent_metadata.st_mode):
        raise StageError("package stage parent is unsafe")
    if parent != normalized.parent:
        raise StageError("package stage parent may not contain symlinks")
    parent_descriptor = -1
    root_path_descriptor = -1
    root_descriptor = -1
    root_created = False
    root_identity: tuple[int, int] | None = None
    try:
        parent_descriptor = os.open(
            parent,
            os.O_RDONLY | os.O_CLOEXEC | os.O_DIRECTORY | os.O_NOFOLLOW,
        )
        if _file_identity(os.fstat(parent_descriptor)) != _file_identity(parent_metadata):
            raise StageError("package stage parent identity changed")
        os.mkdir(normalized.name, mode=0o700, dir_fd=parent_descriptor)
        root_created = True
        root_path_descriptor = os.open(
            normalized.name,
            os.O_PATH | os.O_CLOEXEC | os.O_DIRECTORY | os.O_NOFOLLOW,
            dir_fd=parent_descriptor,
        )
        root_identity = _file_identity(os.fstat(root_path_descriptor))
        # O_PATH acquires a stable handle even when a restrictive process
        # umask created the directory without search permission. chmod through
        # procfs applies to that exact handle rather than resolving the caller's
        # original path again.
        os.chmod(f"/proc/self/fd/{root_path_descriptor}", 0o700)
        root_descriptor = os.open(
            ".",
            os.O_RDONLY | os.O_CLOEXEC | os.O_DIRECTORY | os.O_NOFOLLOW,
            dir_fd=root_path_descriptor,
        )
        if _file_identity(os.fstat(root_descriptor)) != root_identity:
            raise StageError("package stage root identity changed")
        descriptor_root = Path(f"/proc/self/fd/{root_descriptor}")

        _secure_directory(descriptor_root, WORKER_RELATIVE.parent)
        _secure_directory(descriptor_root, MANIFEST_RELATIVE.parent)
        staged_worker = descriptor_root / WORKER_RELATIVE
        staged_manifest = descriptor_root / MANIFEST_RELATIVE

        source, source_metadata = _open_worker(worker)
        digest = hashlib.sha256()
        copied = 0
        output = -1
        try:
            output = os.open(
                staged_worker,
                os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC | os.O_NOFOLLOW,
                0o755,
            )
            prefix = bytearray()
            while True:
                chunk = os.read(source, 64 * 1024)
                if not chunk:
                    break
                if len(prefix) < ELF64_HEADER_BYTES:
                    take = min(ELF64_HEADER_BYTES - len(prefix), len(chunk))
                    prefix.extend(chunk[:take])
                copied += len(chunk)
                if copied > source_metadata.st_size or copied > MAX_WORKER_BYTES:
                    raise StageError("worker input size changed")
                digest.update(chunk)
                view = memoryview(chunk)
                while view:
                    written = os.write(output, view)
                    if written <= 0:
                        raise StageError("worker staging write failed")
                    view = view[written:]
            if copied != source_metadata.st_size:
                raise StageError("worker input size changed")
            _validate_amd64_elf(source, prefix, copied)
            final_source = os.fstat(source)
            if (
                final_source.st_dev,
                final_source.st_ino,
                final_source.st_size,
                final_source.st_mtime_ns,
                final_source.st_ctime_ns,
            ) != (
                source_metadata.st_dev,
                source_metadata.st_ino,
                source_metadata.st_size,
                source_metadata.st_mtime_ns,
                source_metadata.st_ctime_ns,
            ):
                raise StageError("worker input changed during staging")
            os.fchmod(output, 0o755)
            os.fsync(output)
        finally:
            if output >= 0:
                os.close(output)
            os.close(source)

        manifest: dict[str, object] = {
            "schema_version": 1,
            "runtime_id": "transcribe.cpp",
            "worker": {
                "sha256": digest.hexdigest(),
                "size_bytes": copied,
            },
        }
        payload = (json.dumps(manifest, sort_keys=True, separators=(",", ":")) + "\n").encode()
        manifest_fd = -1
        try:
            manifest_fd = os.open(
                staged_manifest,
                os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC | os.O_NOFOLLOW,
                0o644,
            )
            view = memoryview(payload)
            while view:
                written = os.write(manifest_fd, view)
                if written <= 0:
                    raise StageError("runtime manifest staging write failed")
                view = view[written:]
            os.fchmod(manifest_fd, 0o644)
            os.fsync(manifest_fd)
        finally:
            if manifest_fd >= 0:
                os.close(manifest_fd)

        for relative in (
            WORKER_RELATIVE.parent,
            Path("usr/lib"),
            MANIFEST_RELATIVE.parent,
            Path("usr/share"),
            Path("usr"),
        ):
            descriptor = os.open(
                relative,
                os.O_RDONLY
                | os.O_CLOEXEC
                | os.O_DIRECTORY
                | os.O_NOFOLLOW,
                dir_fd=root_descriptor,
            )
            try:
                os.fsync(descriptor)
            finally:
                os.close(descriptor)
        os.fsync(root_descriptor)
        published = os.stat(
            normalized.name,
            dir_fd=parent_descriptor,
            follow_symlinks=False,
        )
        if (
            _file_identity(published) != root_identity
            or not stat.S_ISDIR(published.st_mode)
        ):
            raise StageError("package stage root identity changed before publication")
        os.fsync(parent_descriptor)
        return StagedRuntimeManifest(manifest, root_identity)
    except Exception as original:
        if (
            root_created
            and parent_descriptor >= 0
            and root_descriptor >= 0
            and root_identity is not None
        ):
            try:
                _cleanup_created_tree(
                    parent_descriptor,
                    root_descriptor,
                    normalized.name,
                    root_identity,
                )
            except Exception as cleanup_error:
                raise StageError("package stage cleanup failed closed") from cleanup_error
        elif root_created and parent_descriptor >= 0 and root_identity is not None:
            try:
                current = os.stat(
                    normalized.name,
                    dir_fd=parent_descriptor,
                    follow_symlinks=False,
                )
                if (
                    _file_identity(current) != root_identity
                    or not stat.S_ISDIR(current.st_mode)
                ):
                    raise StageError("package stage cleanup identity changed")
                os.rmdir(normalized.name, dir_fd=parent_descriptor)
            except FileNotFoundError:
                pass
            except OSError as cleanup_error:
                raise StageError("package stage cleanup failed closed") from cleanup_error
        elif root_created:
            raise StageError("package stage cleanup identity was not established") from original
        raise original
    finally:
        if root_descriptor >= 0:
            os.close(root_descriptor)
        if root_path_descriptor >= 0:
            os.close(root_path_descriptor)
        if parent_descriptor >= 0:
            os.close(parent_descriptor)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--stage-root", required=True, type=Path)
    parser.add_argument("--worker", required=True, type=Path)
    arguments = parser.parse_args(argv)
    try:
        manifest = stage_installed_runtime(arguments.stage_root, arguments.worker)
    except (OSError, StageError) as error:
        print(f"stage_installed_runtime: {error}", file=sys.stderr)
        return 1
    print(
        "staged installed runtime: "
        f"worker_size={manifest['worker']['size_bytes']} "
        f"worker_sha256={manifest['worker']['sha256']}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
