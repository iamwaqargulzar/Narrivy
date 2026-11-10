from __future__ import annotations

import hashlib
import json
import os
import stat
import struct
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

import stage_installed_runtime as staging  # noqa: E402
from stage_installed_runtime import (  # noqa: E402
    MANIFEST_RELATIVE,
    WORKER_RELATIVE,
    StageError,
    stage_installed_runtime,
)


class InstalledRuntimeStagingTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.stage = self.root / "stage"
        self.worker = self.root / "narrivy-transcribe-worker"
        header = bytearray(64)
        header[:7] = b"\x7fELF\x02\x01\x01"
        struct.pack_into("<HHI", header, 16, 3, 62, 1)
        struct.pack_into("<Q", header, 32, 64)
        struct.pack_into("<HHH", header, 52, 64, 56, 1)
        program = struct.pack("<IIQQQQQQ", 1, 5, 0, 0, 0, 120, 120, 8)
        self.payload = bytes(header) + program
        self.worker.write_bytes(self.payload)
        self.worker.chmod(0o755)

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def test_stages_exact_worker_and_canonical_manifest(self) -> None:
        manifest = stage_installed_runtime(self.stage, self.worker.resolve())
        self.assertEqual(
            manifest.root_identity,
            (self.stage.lstat().st_dev, self.stage.lstat().st_ino),
        )
        staged_worker = self.stage / WORKER_RELATIVE
        staged_manifest = self.stage / MANIFEST_RELATIVE
        self.assertEqual(staged_worker.read_bytes(), self.payload)
        self.assertEqual(
            stat.S_IMODE(staged_worker.stat().st_mode),
            0o755,
        )
        self.assertEqual(stat.S_IMODE(staged_manifest.stat().st_mode), 0o644)
        self.assertEqual(json.loads(staged_manifest.read_text()), manifest)
        import jsonschema

        schema = json.loads(
            (ROOT / "spec/schemas/installed-runtime-manifest.schema.json").read_text()
        )
        jsonschema.validate(manifest, schema)
        self.assertEqual(
            manifest,
            {
                "schema_version": 1,
                "runtime_id": "transcribe.cpp",
                "worker": {
                    "sha256": hashlib.sha256(self.payload).hexdigest(),
                    "size_bytes": len(self.payload),
                },
            },
        )
        leaves = sorted(
            path.relative_to(self.stage).as_posix()
            for path in self.stage.rglob("*")
            if path.is_file()
        )
        self.assertEqual(
            leaves,
            [WORKER_RELATIVE.as_posix(), MANIFEST_RELATIVE.as_posix()],
        )

    def test_rejects_symlink_non_elf_non_executable_and_relative_worker(self) -> None:
        link = self.root / "worker-link"
        link.symlink_to(self.worker)
        with self.assertRaises(StageError):
            stage_installed_runtime(self.stage, link)

        self.worker.write_bytes(b"not an elf")
        with self.assertRaises(StageError):
            stage_installed_runtime(self.stage, self.worker.resolve())

    def test_rejects_directory_fifo_zero_and_truncated_elf(self) -> None:
        directory = self.root / "directory-worker"
        directory.mkdir(mode=0o755)
        with self.assertRaises(StageError):
            stage_installed_runtime(self.stage, directory.resolve())

        fifo = self.root / "fifo-worker"
        os.mkfifo(fifo, mode=0o755)
        with self.assertRaises(StageError):
            stage_installed_runtime(self.stage, fifo.resolve())

        self.worker.write_bytes(b"")
        self.worker.chmod(0o755)
        with self.assertRaises(StageError):
            stage_installed_runtime(self.stage, self.worker.resolve())

        self.worker.write_bytes(b"\x7fELF\x02\x01\x01")
        self.worker.chmod(0o755)
        with self.assertRaises(StageError):
            stage_installed_runtime(self.stage, self.worker.resolve())

        self.worker.write_bytes(self.payload)
        self.worker.chmod(0o600)
        with self.assertRaises(StageError):
            stage_installed_runtime(self.stage, self.worker.resolve())

        with self.assertRaises(StageError):
            stage_installed_runtime(self.stage, Path("relative-worker"))

        self.worker.write_bytes(self.payload)
        self.worker.chmod(0o4755)
        with self.assertRaises(StageError):
            stage_installed_runtime(self.stage, self.worker.resolve())

    def test_existing_and_symlink_stage_roots_fail_without_overwrite(self) -> None:
        self.stage.mkdir(mode=0o700)
        marker = self.stage / "preserve"
        marker.write_bytes(b"preserve")
        with self.assertRaises(StageError):
            stage_installed_runtime(self.stage, self.worker.resolve())
        self.assertEqual(marker.read_bytes(), b"preserve")

        unsafe = self.root / "unsafe-stage"
        unsafe.symlink_to(self.stage, target_is_directory=True)
        with self.assertRaises(StageError):
            stage_installed_runtime(unsafe, self.worker.resolve())

    def test_failure_removes_only_the_newly_created_stage_root(self) -> None:
        self.worker.write_bytes(b"not an elf")
        with self.assertRaises(StageError):
            stage_installed_runtime(self.stage, self.worker.resolve())
        self.assertFalse(self.stage.exists())

    def test_separate_roots_are_byte_identical(self) -> None:
        first = self.root / "first"
        second = self.root / "second"
        stage_installed_runtime(first, self.worker.resolve())
        stage_installed_runtime(second, self.worker.resolve())
        self.assertEqual(
            (first / WORKER_RELATIVE).read_bytes(),
            (second / WORKER_RELATIVE).read_bytes(),
        )
        self.assertEqual(
            (first / MANIFEST_RELATIVE).read_bytes(),
            (second / MANIFEST_RELATIVE).read_bytes(),
        )

    def test_size_mutation_and_fsync_failures_leave_no_root(self) -> None:
        from unittest import mock

        with mock.patch.object(staging, "MAX_WORKER_BYTES", len(self.payload) - 1):
            with self.assertRaises(StageError):
                stage_installed_runtime(self.stage, self.worker.resolve())
        self.assertFalse(self.stage.exists())

        with mock.patch.object(staging.os, "fsync", side_effect=OSError("controlled")):
            with self.assertRaises(OSError):
                stage_installed_runtime(self.stage, self.worker.resolve())
        self.assertFalse(self.stage.exists())

    def test_source_metadata_mutation_is_detected(self) -> None:
        from unittest import mock

        real_read = os.read
        mutated = False

        def mutating_read(descriptor: int, count: int) -> bytes:
            nonlocal mutated
            chunk = real_read(descriptor, count)
            if chunk and not mutated:
                mutated = True
                os.utime(self.worker, None)
            return chunk

        with mock.patch.object(staging.os, "read", side_effect=mutating_read):
            with self.assertRaises(StageError):
                stage_installed_runtime(self.stage, self.worker.resolve())
        self.assertFalse(self.stage.exists())

    def test_restrictive_umask_cannot_weaken_or_break_staging_modes(self) -> None:
        previous = os.umask(0o777)
        try:
            stage_installed_runtime(self.stage, self.worker.resolve())
        finally:
            os.umask(previous)

        self.assertEqual(stat.S_IMODE(self.stage.stat().st_mode), 0o700)
        self.assertEqual(
            stat.S_IMODE((self.stage / WORKER_RELATIVE).stat().st_mode),
            0o755,
        )
        self.assertEqual(
            stat.S_IMODE((self.stage / MANIFEST_RELATIVE).stat().st_mode),
            0o644,
        )

    def test_collision_created_during_mkdir_is_preserved(self) -> None:
        from unittest import mock

        real_mkdir = staging.os.mkdir
        collision = self.stage / "preserve"

        def collide(path: object, mode: int = 0o777, *, dir_fd: int | None = None) -> None:
            if path == self.stage.name and dir_fd is not None:
                real_mkdir(path, mode=0o700, dir_fd=dir_fd)
                collision.write_bytes(b"not-created-by-stager")
                raise FileExistsError(path)
            real_mkdir(path, mode=mode, dir_fd=dir_fd)

        with mock.patch.object(staging.os, "mkdir", side_effect=collide):
            with self.assertRaises(FileExistsError):
                stage_installed_runtime(self.stage, self.worker.resolve())

        self.assertEqual(collision.read_bytes(), b"not-created-by-stager")

    def test_parent_swap_failure_never_deletes_the_redirected_tree(self) -> None:
        from unittest import mock

        original_parent = self.root / "original-parent"
        original_parent.mkdir()
        stage = original_parent / "stage"
        moved_parent = self.root / "moved-parent"
        victim_parent = self.root / "victim-parent"
        victim_stage = victim_parent / "stage"
        victim_stage.mkdir(parents=True)
        victim_marker = victim_stage / "preserve"
        victim_marker.write_bytes(b"victim")
        real_secure_directory = staging._secure_directory
        swapped = False

        def swap_parent(root: Path, relative: Path) -> Path:
            nonlocal swapped
            if not swapped:
                swapped = True
                original_parent.rename(moved_parent)
                original_parent.symlink_to(victim_parent, target_is_directory=True)
                raise StageError("controlled parent swap")
            return real_secure_directory(root, relative)

        with mock.patch.object(
            staging,
            "_secure_directory",
            side_effect=swap_parent,
        ):
            with self.assertRaises(StageError):
                stage_installed_runtime(stage, self.worker.resolve())

        self.assertEqual(victim_marker.read_bytes(), b"victim")
        self.assertFalse((moved_parent / "stage").exists())

    def test_root_name_replacement_cannot_report_false_success(self) -> None:
        from unittest import mock

        moved_stage = self.root / "moved-stage"
        replacement_marker = self.stage / "preserve"
        real_secure_directory = staging._secure_directory
        swapped = False

        def replace_root(root: Path, relative: Path) -> Path:
            nonlocal swapped
            result = real_secure_directory(root, relative)
            if not swapped:
                swapped = True
                self.stage.rename(moved_stage)
                self.stage.mkdir(mode=0o700)
                replacement_marker.write_bytes(b"replacement")
            return result

        with mock.patch.object(
            staging,
            "_secure_directory",
            side_effect=replace_root,
        ):
            with self.assertRaises(StageError):
                stage_installed_runtime(self.stage, self.worker.resolve())

        self.assertEqual(replacement_marker.read_bytes(), b"replacement")
        self.assertFalse((self.stage / MANIFEST_RELATIVE).exists())
        self.assertFalse((moved_stage / MANIFEST_RELATIVE).exists())
        self.assertFalse((moved_stage / WORKER_RELATIVE).exists())

    def test_every_fixed_directory_is_fsynced_leaf_to_parent(self) -> None:
        from unittest import mock

        real_fsync = os.fsync
        synced_directories: list[Path] = []

        def observe_fsync(descriptor: int) -> None:
            if stat.S_ISDIR(os.fstat(descriptor).st_mode):
                synced_directories.append(Path(os.readlink(f"/proc/self/fd/{descriptor}")))
            real_fsync(descriptor)

        with mock.patch.object(staging.os, "fsync", side_effect=observe_fsync):
            stage_installed_runtime(self.stage, self.worker.resolve())

        self.assertEqual(
            synced_directories,
            [
                self.stage / "usr/lib/narrivy",
                self.stage / "usr/lib",
                self.stage / "usr/share/narrivy",
                self.stage / "usr/share",
                self.stage / "usr",
                self.stage,
                self.stage.parent,
            ],
        )


if __name__ == "__main__":
    unittest.main()
