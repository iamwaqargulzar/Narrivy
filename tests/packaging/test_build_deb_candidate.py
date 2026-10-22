from __future__ import annotations

import hashlib
import json
import os
import struct
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

import build_deb_candidate as candidate  # noqa: E402


class DisposableDebCandidateTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        header = bytearray(64)
        header[:7] = b"\x7fELF\x02\x01\x01"
        struct.pack_into("<HHI", header, 16, 3, 62, 1)
        struct.pack_into("<Q", header, 32, 64)
        struct.pack_into("<HHH", header, 52, 64, 56, 1)
        program = struct.pack("<IIQQQQQQ", 1, 5, 0, 0, 0, 120, 120, 8)
        self.elf = bytes(header) + program
        binaries = {}
        for name in ("narrivyd", "narrivy-cli", "narrivy-desktop", "worker"):
            path = self.root / name
            path.write_bytes(self.elf)
            path.chmod(0o755)
            binaries[name] = path.resolve()
        self.inputs = candidate.CandidateInputs(
            narrivyd=binaries["narrivyd"],
            cli=binaries["narrivy-cli"],
            desktop=binaries["narrivy-desktop"],
            worker=binaries["worker"],
            service=(
                ROOT / "packaging/recovery/narrivy-daemon-candidate.service"
            ).resolve(),
            desktop_entry=(ROOT / "packaging/deb/narrivy.desktop").resolve(),
            icon_32=(ROOT / "packaging/linux/icons/32x32/narrivy.png").resolve(),
            icon_128=(ROOT / "packaging/linux/icons/128x128/narrivy.png").resolve(),
            icon_256=(ROOT / "packaging/linux/icons/256x256/narrivy.png").resolve(),
            icon_512=(ROOT / "packaging/linux/icons/512x512/narrivy.png").resolve(),
            license_file=(ROOT / "LICENSE").resolve(),
            notices=(ROOT / "THIRD_PARTY_NOTICES.md").resolve(),
            source_head="f" * 40,
            source_date_epoch=1_700_000_000,
        )
        self.probe = mock.patch.object(candidate, "_probe_installed_daemon")
        self.probe_mock = self.probe.start()

    def tearDown(self) -> None:
        self.probe.stop()
        self.temporary.cleanup()

    def test_candidate_is_exact_install_blocked_and_disposable(self) -> None:
        before = {
            path.name
            for path in Path(tempfile.gettempdir()).glob("narrivy-deb-candidate-*")
        }
        summary = candidate.compose_disposable_candidate(self.inputs)
        after = {
            path.name
            for path in Path(tempfile.gettempdir()).glob("narrivy-deb-candidate-*")
        }
        self.assertEqual(before, after)
        self.assertEqual(
            summary["classification"],
            "non-release-disposable-candidate",
        )
        self.assertEqual(summary["package"], candidate.PACKAGE_NAME)
        self.assertEqual(summary["version"], candidate.PACKAGE_VERSION)
        self.assertEqual(summary["data_file_count"], 14)
        self.assertIs(summary["install_blocked"], True)
        self.assertGreater(summary["archive_size_bytes"], 0)
        self.assertRegex(summary["archive_sha256"], r"^[0-9a-f]{64}$")
        self.probe_mock.assert_called_once()
        self.assertIsInstance(self.probe_mock.call_args.args[0], int)

    def test_two_compositions_are_byte_deterministic(self) -> None:
        first = candidate.compose_disposable_candidate(self.inputs)
        second = candidate.compose_disposable_candidate(self.inputs)
        self.assertEqual(first, second)

    def test_restrictive_umask_is_deterministic(self) -> None:
        ordinary = candidate.compose_disposable_candidate(self.inputs)
        previous = os.umask(0o077)
        try:
            restricted = candidate.compose_disposable_candidate(self.inputs)
        finally:
            os.umask(previous)
        self.assertEqual(ordinary, restricted)

    def test_rejects_debug_mode_non_elf_symlink_and_bad_source_identity(self) -> None:
        self.inputs.narrivyd.chmod(0o775)
        with self.assertRaises(RuntimeError):
            candidate.compose_disposable_candidate(self.inputs)
        self.inputs.narrivyd.chmod(0o755)

        self.inputs.cli.write_bytes(b"not an ELF")
        self.inputs.cli.chmod(0o755)
        with self.assertRaises(RuntimeError):
            candidate.compose_disposable_candidate(self.inputs)
        self.inputs.cli.write_bytes(self.elf)
        self.inputs.cli.chmod(0o755)

        link = self.root / "worker-link"
        link.symlink_to(self.inputs.worker)
        linked = candidate.CandidateInputs(
            **{**self.inputs.__dict__, "worker": link}
        )
        with self.assertRaises(RuntimeError):
            candidate.compose_disposable_candidate(linked)

        invalid_head = candidate.CandidateInputs(
            **{**self.inputs.__dict__, "source_head": "unknown"}
        )
        with self.assertRaises(candidate.CandidateError):
            candidate.compose_disposable_candidate(invalid_head)

    def test_service_rejects_mock_raw_optional_or_unbounded_composition(self) -> None:
        valid = self.inputs.service.read_bytes()
        candidate._validate_service(valid)
        self.assertEqual(
            valid,
            (ROOT / "packaging/deb/narrivy-daemon.service").read_bytes(),
        )
        self.assertEqual(
            valid,
            (ROOT / "packaging/linux/deb/narrivy-daemon.service").read_bytes(),
        )
        for replacement in (
            valid.replace(b"=transcribe-cpp", b"=mock", 1),
            valid + b"\nEnvironmentFile=-/tmp/optional\n",
            valid + b"\nExecStartPost=/tmp/unreviewed\n",
            valid + b"\nEnvironment=NARRIVY_TRANSCRIBE_RUNTIME=sidecar\n",
            valid + b"\nRestart=no\n",
            valid + b"\nExecStart=\nExecStart=/usr/bin/true\n",
            valid + b"\nCapabilityBoundingSet=~\n",
            valid.replace(b"[Service]\n", b"").replace(
                b"[Install]\n", b"[Service]\n[Install]\n"
            ),
            valid.replace(b"RestartPreventExitStatus=1 2\n", b""),
            valid.replace(b"ExitType=cgroup\n", b""),
            valid.replace(b"KillMode=control-group\n", b""),
            valid.replace(b"TimeoutStopSec=3s\n", b""),
            valid.replace(b"TimeoutStopFailureMode=kill\n", b""),
            valid.replace(b"SendSIGKILL=yes\n", b""),
            valid.replace(b"UnsetEnvironment=NARRIVY_TRANSCRIBE_MODEL\n", b""),
        ):
            with self.assertRaises(candidate.CandidateError):
                candidate._validate_service(replacement)

    def test_icon_and_legal_inputs_fail_closed(self) -> None:
        candidate._validate_png(self.inputs.icon_32.read_bytes(), 32)
        with self.assertRaises(candidate.CandidateError):
            candidate._validate_png(self.inputs.icon_32.read_bytes(), 128)
        with self.assertRaises(candidate.CandidateError):
            candidate._validate_legal_text(
                b"MIT License stub",
                self.inputs.notices.read_bytes(),
            )
        with self.assertRaises(candidate.CandidateError):
            candidate._validate_desktop_entry(
                self.inputs.desktop_entry.read_bytes() + b"Exec=/tmp/unreviewed\n"
            )
        group_writable = self.root / "group-writable"
        group_writable.write_bytes(b"bounded input")
        group_writable.chmod(0o664)
        with self.assertRaises(candidate.CandidateError):
            candidate._read_asset(group_writable.resolve())

    def test_installed_daemon_probe_accepts_only_exact_missing_runtime_failure(self) -> None:
        self.probe.stop()
        accepted = self.root / "accepted-probe"
        accepted.write_text(
            "#!/bin/sh\n"
            "echo 'narrivyd: installed resident runtime failed verification' >&2\n"
            "exit 1\n",
            encoding="utf-8",
        )
        accepted.chmod(0o755)
        accepted_descriptor = os.open(accepted, os.O_RDONLY | os.O_CLOEXEC)
        try:
            accepted.rename(self.root / "accepted-probe-retained")
            accepted.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
            accepted.chmod(0o755)
            candidate._probe_installed_daemon(accepted_descriptor)
        finally:
            os.close(accepted_descriptor)

        wrong_exit = self.root / "wrong-exit"
        wrong_exit.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
        wrong_exit.chmod(0o755)
        wrong_descriptor = os.open(wrong_exit, os.O_RDONLY | os.O_CLOEXEC)
        try:
            with self.assertRaises(candidate.CandidateError):
                candidate._probe_installed_daemon(wrong_descriptor)
        finally:
            os.close(wrong_descriptor)

        escaped = self.root / "escaped-probe"
        escaped.write_text(
            "#!/bin/sh\n"
            "setsid /bin/sleep 30 &\n"
            "sleep 0.1\n"
            "echo 'narrivyd: installed resident runtime failed verification' >&2\n"
            "exit 1\n",
            encoding="utf-8",
        )
        escaped.chmod(0o755)
        escaped_descriptor = os.open(escaped, os.O_RDONLY | os.O_CLOEXEC)
        try:
            with self.assertRaises(candidate.CandidateError):
                candidate._probe_installed_daemon(escaped_descriptor)
        finally:
            os.close(escaped_descriptor)
        self.probe = mock.patch.object(candidate, "_probe_installed_daemon")
        self.probe_mock = self.probe.start()

    def test_tar_reader_rejects_duplicate_members(self) -> None:
        archive_path = self.root / "duplicate.tar"
        with candidate.tarfile.open(archive_path, "w") as archive:
            first = candidate.tarfile.TarInfo("./duplicate")
            first.type = candidate.tarfile.DIRTYPE
            archive.addfile(first)
            second = candidate.tarfile.TarInfo("./duplicate")
            second.type = candidate.tarfile.DIRTYPE
            archive.addfile(second)
        descriptor = os.open(archive_path, os.O_RDONLY | os.O_CLOEXEC)
        try:
            with self.assertRaises(candidate.CandidateError):
                candidate._tar_members(
                    ["/bin/cat", f"/proc/self/fd/{descriptor}"],
                    descriptor,
                    {"duplicate"},
                    {},
                    set(),
                )
        finally:
            os.close(descriptor)

    def test_archive_snapshot_is_stable_after_path_replacement(self) -> None:
        archive = self.root / "candidate.deb"
        archive.write_bytes(b"first archive bytes")
        descriptor, digest, size = candidate._snapshot_archive(archive)
        try:
            archive.unlink()
            archive.write_bytes(b"replacement")
            self.assertEqual(size, len(b"first archive bytes"))
            self.assertEqual(digest, hashlib.sha256(b"first archive bytes").hexdigest())
            self.assertEqual(os.pread(descriptor, size, 0), b"first archive bytes")
        finally:
            os.close(descriptor)
        oversized = self.root / "oversized.deb"
        with oversized.open("wb") as output:
            output.truncate(candidate.MAX_ARCHIVE_BYTES + 1)
        with self.assertRaises(candidate.CandidateError):
            candidate._snapshot_archive(oversized)

    def test_inspection_rejects_unexpected_directory_and_noncanonical_preinst(self) -> None:
        epoch = self.inputs.source_date_epoch
        canonical_control = (
            f"Package: {candidate.PACKAGE_NAME}\n"
            f"Version: {candidate.PACKAGE_VERSION}\n"
            "Section: utils\nPriority: optional\nArchitecture: amd64\n"
            "Maintainer: Narrivy Recovery <release@narrivy.invalid>\n"
            "Depends: pipewire, xdg-utils\n"
            "Description: Narrivy disposable recovery package candidate\n"
            " This archive is deliberately install-blocked and is not a release artifact.\n"
        ).encode()
        canonical_preinst = (
            "#!/bin/sh\n"
            "echo 'Narrivy recovery candidate: installation is prohibited' >&2\n"
            "exit 1\n"
        ).encode()

        def build(root: Path, preinst: bytes, *, extra_directory: bool) -> int:
            (root / "DEBIAN").mkdir(parents=True)
            candidate._write_file(root / "DEBIAN/control", canonical_control, 0o644)
            candidate._write_file(root / "DEBIAN/preinst", preinst, 0o755)
            if extra_directory:
                (root / "unexpected").mkdir()
            candidate._normalize_tree(root, epoch)
            archive = root.parent / f"{root.name}.deb"
            result = subprocess.run(
                [
                    str(candidate.DPKG_DEB),
                    "-Zxz",
                    "-z9",
                    "--root-owner-group",
                    "--build",
                    str(root),
                    str(archive),
                ],
                check=False,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                env={
                    "PATH": "/usr/bin:/bin",
                    "LC_ALL": "C",
                    "TZ": "UTC",
                    "SOURCE_DATE_EPOCH": str(epoch),
                },
            )
            self.assertEqual(result.returncode, 0)
            descriptor, _, _ = candidate._snapshot_archive(archive)
            return descriptor

        unexpected = build(self.root / "unexpected-root", canonical_preinst, extra_directory=True)
        try:
            with self.assertRaises(candidate.CandidateError):
                candidate._inspect_candidate(
                    unexpected, {}, canonical_control, canonical_preinst, epoch
                )
        finally:
            os.close(unexpected)

        noncanonical = build(
            self.root / "noncanonical-root",
            b"#!/bin/sh\nexit 0\nexit 1\n",
            extra_directory=False,
        )
        try:
            with self.assertRaises(candidate.CandidateError):
                candidate._inspect_candidate(
                    noncanonical, {}, canonical_control, canonical_preinst, epoch
                )
        finally:
            os.close(noncanonical)

    def test_package_tree_mutation_cannot_redefine_expected_binary(self) -> None:
        real_normalize = candidate._normalize_anchored_tree

        def mutate_before_build(
            root_descriptor: int,
            expected_files: dict[str, candidate.ExpectedFile],
            epoch: int,
        ) -> None:
            target = Path(f"/proc/self/fd/{root_descriptor}/usr/bin/narrivy-cli")
            target.write_bytes(self.elf + b"mutated")
            target.chmod(0o755)
            real_normalize(root_descriptor, expected_files, epoch)

        with mock.patch.object(
            candidate, "_normalize_anchored_tree", side_effect=mutate_before_build
        ):
            with self.assertRaises(candidate.CandidateError):
                candidate.compose_disposable_candidate(self.inputs)

        real_copy = candidate._copy_release_elf

        def same_size_daemon_swap(
            source: Path,
            root_descriptor: int,
            relative: candidate.PurePosixPath,
        ) -> dict[str, object]:
            identity = real_copy(source, root_descriptor, relative)
            if str(relative) == "usr/bin/narrivyd":
                target = Path(f"/proc/self/fd/{root_descriptor}/{relative}")
                target.write_bytes(b"X" * int(identity["size_bytes"]))
                target.chmod(0o755)
            return identity

        with mock.patch.object(candidate, "_copy_release_elf", side_effect=same_size_daemon_swap):
            with self.assertRaises(candidate.CandidateError):
                candidate.compose_disposable_candidate(self.inputs)

        outside = self.root / "outside"
        outside.mkdir()
        real_write = candidate._write_file_at
        injected = False

        def redirect_ancestor(
            root_descriptor: int,
            relative: candidate.PurePosixPath,
            payload: bytes,
            mode: int,
        ) -> None:
            nonlocal injected
            if not injected and str(relative) == "usr/share/applications/narrivy.desktop":
                injected = True
                parent = Path(f"/proc/self/fd/{root_descriptor}/usr/share/applications")
                parent.symlink_to(outside, target_is_directory=True)
            real_write(root_descriptor, relative, payload, mode)

        with mock.patch.object(candidate, "_write_file_at", side_effect=redirect_ancestor):
            with self.assertRaises(OSError):
                candidate.compose_disposable_candidate(self.inputs)
        self.assertEqual(list(outside.iterdir()), [])

    def test_dpkg_failure_does_not_leave_candidate_or_call_legacy_builder(self) -> None:
        protected = {
            path: hashlib.sha256(path.read_bytes()).hexdigest()
            for path in (
                ROOT / "artifacts/releases/narrivy_0.2.3_amd64.deb",
                ROOT / "packaging/deb/BUILD_LOG.md",
                ROOT / "scripts/build_deb.sh",
            )
        }
        real_run = subprocess.run

        def fail_build(argv: list[str], **kwargs: object) -> subprocess.CompletedProcess[bytes]:
            if "--build" in argv:
                return subprocess.CompletedProcess(argv, 3, b"", b"controlled")
            return real_run(argv, **kwargs)

        with mock.patch.object(candidate.subprocess, "run", side_effect=fail_build):
            with self.assertRaises(candidate.CandidateError):
                candidate.compose_disposable_candidate(self.inputs)
        for path, digest in protected.items():
            self.assertEqual(hashlib.sha256(path.read_bytes()).hexdigest(), digest)

    def test_archive_output_symlink_and_hardlink_collisions_cannot_overwrite(self) -> None:
        for collision in ("symlink", "hardlink"):
            protected = self.root / f"protected-{collision}"
            protected.write_bytes(f"protected-{collision}".encode())
            before = protected.read_bytes()
            real_open = os.open
            injected = False

            def collide(path: object, flags: int, *args: object, **kwargs: object) -> int:
                nonlocal injected
                candidate_path = Path(path)
                if not injected and candidate_path.name == "candidate.deb":
                    injected = True
                    if collision == "symlink":
                        candidate_path.symlink_to(protected)
                    else:
                        os.link(protected, candidate_path)
                return real_open(path, flags, *args, **kwargs)

            with mock.patch.object(candidate.os, "open", side_effect=collide):
                with self.assertRaises(FileExistsError):
                    candidate.compose_disposable_candidate(self.inputs)
            self.assertEqual(protected.read_bytes(), before)

    def test_cli_prints_only_canonical_content_free_summary(self) -> None:
        argv = []
        mapping = {
            "narrivyd": self.inputs.narrivyd,
            "cli": self.inputs.cli,
            "desktop": self.inputs.desktop,
            "worker": self.inputs.worker,
            "service": self.inputs.service,
            "desktop-entry": self.inputs.desktop_entry,
            "icon-32": self.inputs.icon_32,
            "icon-128": self.inputs.icon_128,
            "icon-256": self.inputs.icon_256,
            "icon-512": self.inputs.icon_512,
            "license-file": self.inputs.license_file,
            "notices": self.inputs.notices,
        }
        for name, path in mapping.items():
            argv.extend((f"--{name}", str(path)))
        argv.extend(
            (
                "--source-head",
                self.inputs.source_head,
                "--source-date-epoch",
                str(self.inputs.source_date_epoch),
            )
        )
        parsed = candidate._arguments(argv)
        summary = candidate.compose_disposable_candidate(parsed)
        encoded = json.dumps(summary, sort_keys=True, separators=(",", ":"))
        self.assertNotIn(str(self.root), encoded)
        self.assertNotIn("transcript", encoded.lower())


if __name__ == "__main__":
    unittest.main()
