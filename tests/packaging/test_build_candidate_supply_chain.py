from __future__ import annotations

import hashlib
import io
import json
import os
import stat
import subprocess
import sys
import tarfile
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

import build_candidate_supply_chain as supply  # noqa: E402


SAMPLE_VERSION = "0.1.0~rc5"
SAMPLE_EPOCH = 1_700_000_000
REGISTRY_CHECKSUM = "ab" * 32
EMBEDDED = {
    "narrivyd": {"sha256": "aa" * 32, "size_bytes": 11},
    "narrivy-cli": {"sha256": "bb" * 32, "size_bytes": 22},
    "narrivy-desktop": {"sha256": "cc" * 32, "size_bytes": 33},
    "resident-worker": {"sha256": "dd" * 32, "size_bytes": 44},
    "bundled-model": {"sha256": "ee" * 32, "size_bytes": 55},
}
LOCKFILE = """version = 4

[[package]]
name = "example"
version = "1.2.3"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "%s"

[[package]]
name = "from-git"
version = "0.4.0"
source = "git+https://example.local/from-git?rev=deadbeef#deadbeef"

[[package]]
name = "local-crate"
version = "0.0.0"
""" % REGISTRY_CHECKSUM
CHANGED_LOCKFILE = LOCKFILE + """
[[package]]
name = "should-not-appear"
version = "9.9.9"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "%s"
""" % ("ff" * 32)


def _ar_archive(members: list[tuple[str, bytes]]) -> bytes:
    archive = bytearray(b"!<arch>\n")
    for name, payload in members:
        encoded_name = (name + "/").encode("ascii")
        if len(encoded_name) > 16:
            encoded_name = name.encode("ascii")[:16]
        header = (
            encoded_name.ljust(16)
            + b"0".ljust(12)
            + b"0".ljust(6)
            + b"0".ljust(6)
            + b"100644".ljust(8)
            + str(len(payload)).encode("ascii").ljust(10)
            + b"`\n"
        )
        if len(header) != 60:
            raise AssertionError("ar header size is wrong")
        archive.extend(header)
        archive.extend(payload)
        if len(payload) % 2 == 1:
            archive.append(0x0A)
    return bytes(archive)


def _gzip_tar(entries: list[tuple[str, bytes | None, str]]) -> bytes:
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w:gz") as archive:
        for name, payload, kind in entries:
            info = tarfile.TarInfo(name=name)
            if kind == "file":
                assert payload is not None
                info.type = tarfile.REGTYPE
                info.mode = 0o644
                info.size = len(payload)
                archive.addfile(info, io.BytesIO(payload))
            elif kind == "symlink":
                info.type = tarfile.SYMTYPE
                info.linkname = payload.decode("ascii") if payload else "target"
                archive.addfile(info)
            elif kind == "directory":
                info.type = tarfile.DIRTYPE
                info.mode = 0o755
                archive.addfile(info)
            else:
                raise AssertionError(kind)
    return buffer.getvalue()


def _control(version: str = SAMPLE_VERSION, package: str = "narrivy") -> bytes:
    return f"Package: {package}\nVersion: {version}\nArchitecture: amd64\n".encode()


def _validate_schema_v3(document: object) -> None:
    schema = json.loads(
        (ROOT / "spec/schemas/release-artifacts.schema.json").read_text(encoding="utf-8")
    )
    import jsonschema

    jsonschema.validate(instance=document, schema=schema)


class CandidateSupplyChainTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def _git(self, repo: Path, *arguments: str) -> subprocess.CompletedProcess[str]:
        env = {
            **supply.GIT_ENV,
            "GIT_AUTHOR_NAME": "Narrivy Tests",
            "GIT_AUTHOR_EMAIL": "tests@narrivy.local",
            "GIT_COMMITTER_NAME": "Narrivy Tests",
            "GIT_COMMITTER_EMAIL": "tests@narrivy.local",
        }
        return subprocess.run(
            [supply.GIT, "-C", os.fspath(repo), *arguments],
            check=True,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            env=env,
            timeout=supply.GIT_TIMEOUT_SECONDS,
        )

    def _repo(self, name: str = "repo", lockfile: str = LOCKFILE) -> tuple[Path, str]:
        repo = self.root / name
        repo.mkdir()
        (repo / "Cargo.lock").write_text(lockfile, encoding="utf-8")
        (repo / "id.txt").write_text(f"{name}\n", encoding="utf-8")
        self._git(repo, "init", "--initial-branch=main")
        self._git(repo, "add", "Cargo.lock", "id.txt")
        self._git(repo, "commit", "--no-gpg-sign", "-m", f"lock-{name}")
        head = self._git(repo, "rev-parse", "--verify", "HEAD").stdout.strip()
        return repo, head

    def _build_info(self, source_head: str, **overrides: object) -> bytes:
        document: dict[str, object] = {
            "classification": supply.CLEAN_CLASSIFICATION,
            "git_dirty": False,
            "inputs": EMBEDDED,
            "package": "narrivy",
            "schema_version": 2,
            "source_date_epoch": SAMPLE_EPOCH,
            "source_head": source_head,
            "version": SAMPLE_VERSION,
        }
        document.update(overrides)
        return supply._canonical_json(document)

    def _debian(
        self,
        source_head: str,
        *,
        data_entries: list[tuple[str, bytes | None, str]] | None = None,
        extra_data: list[tuple[str, bytes | None, str]] | None = None,
        control: bytes | None = None,
        build_info: bytes | None = None,
        duplicate_build_info: bool = False,
    ) -> bytes:
        if data_entries is None:
            payload = build_info if build_info is not None else self._build_info(source_head)
            data_entries = [
                (supply.BUILD_INFO_MEMBER, payload, "file"),
                ("usr/share/narrivy/readme.txt", b"ok\n", "file"),
            ]
            if extra_data:
                data_entries.extend(extra_data)
        data = _gzip_tar(data_entries)
        if duplicate_build_info:
            buffer = io.BytesIO()
            with tarfile.open(fileobj=buffer, mode="w:gz") as archive:
                for name, payload, kind in data_entries:
                    info = tarfile.TarInfo(name=name)
                    info.type = tarfile.REGTYPE
                    info.mode = 0o644
                    info.size = len(payload or b"")
                    archive.addfile(info, io.BytesIO(payload or b""))
                    if name == supply.BUILD_INFO_MEMBER:
                        archive.addfile(info, io.BytesIO(payload or b""))
            data = buffer.getvalue()
        return _ar_archive(
            [
                ("debian-binary", b"2.0\n"),
                ("control.tar.gz", _gzip_tar([("control", control or _control(), "file")])),
                ("data.tar.gz", data),
            ]
        )

    def _write_candidate(self, repo: Path, payload: bytes, name: str = "narrivy.deb") -> Path:
        path = repo / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(payload)
        return path

    def _inputs(
        self,
        repo: Path,
        candidate: Path,
        output: Path,
        expected: str | None = None,
        **overrides: object,
    ) -> supply.Inputs:
        if expected is None:
            if candidate.is_symlink():
                expected = hashlib.sha256(candidate.read_bytes()).hexdigest()
            elif candidate.is_file():
                expected = hashlib.sha256(candidate.read_bytes()).hexdigest()
            else:
                expected = "ab" * 32
        values = {
            "candidate": candidate,
            "source_root": repo,
            "output": output,
            "expected_sha256": expected,
            "builder_os": "Ubuntu 24.04",
            "builder_image": "narrivy-test-builder",
            "builder_image_digest": "unverified-test-image",
            "toolchain": "rustc 1.97.1 + cargo 1.97.1",
        }
        values.update(overrides)
        return supply.Inputs(**values)  # type: ignore[arg-type]

    def _write_executable(self, name: str, body: str) -> Path:
        path = self.root / name
        path.write_text(body, encoding="utf-8")
        path.chmod(path.stat().st_mode | stat.S_IXUSR)
        return path

    def _assert_no_output(self, output: Path, parent: Path | None = None) -> None:
        self.assertFalse(os.path.lexists(output))
        if parent is not None:
            leftovers = list(parent.glob(".narrivy-supply-chain-*"))
            self.assertEqual(leftovers, [])

    def test_success_is_deterministic_schema3_and_hashed(self) -> None:
        repo, head = self._repo()
        (repo / "Cargo.lock").write_text(CHANGED_LOCKFILE, encoding="utf-8")
        candidate = self._write_candidate(repo, self._debian(head), "packages/narrivy.deb")
        output = repo / "bundle"
        inputs = self._inputs(repo, candidate, output)
        first = supply.build(inputs)
        first_files = {
            name: (output / name).read_bytes()
            for name in (
                supply.SPDX_NAME,
                supply.PROVENANCE_NAME,
                supply.RELEASE_ARTIFACTS_NAME,
                supply.RESULT_NAME,
            )
        }
        shutil_move = output.rename(repo / "first-bundle")
        second = supply.build(self._inputs(repo, candidate, output))
        self.assertEqual(first["classification"], "candidate-supply-chain-unqualified")
        self.assertIs(first["native_hardware_qualified"], False)
        self.assertIs(first["release_qualified"], False)
        self.assertIs(first["requirement_evidence"], False)
        self.assertEqual(first["git_commit"], head)
        self.assertEqual(first["version"], SAMPLE_VERSION)
        self.assertEqual(first["package"], "narrivy")
        self.assertEqual(
            first["candidate_sha256"],
            hashlib.sha256(candidate.read_bytes()).hexdigest(),
        )
        self.assertEqual(
            first["cargo_lock_hash"],
            hashlib.sha256(LOCKFILE.encode()).hexdigest(),
        )
        self.assertNotEqual(first["cargo_lock_hash"], hashlib.sha256(CHANGED_LOCKFILE.encode()).hexdigest())
        for name in (
            supply.SPDX_NAME,
            supply.PROVENANCE_NAME,
            supply.RELEASE_ARTIFACTS_NAME,
            supply.RESULT_NAME,
        ):
            self.assertEqual((output / name).read_bytes(), first_files[name])
            self.assertEqual((shutil_move / name).read_bytes(), first_files[name])
        self.assertEqual(first, second)

        spdx = json.loads((output / supply.SPDX_NAME).read_text(encoding="utf-8"))
        self.assertEqual(spdx["spdxVersion"], "SPDX-2.3")
        names = [package["name"] for package in spdx["packages"]]
        self.assertEqual(names[0], "narrivy")
        self.assertIn("example", names)
        self.assertIn("from-git", names)
        self.assertIn("local-crate", names)
        self.assertNotIn("should-not-appear", names)
        self.assertNotIn("rustc", names)
        self.assertNotIn("cargo-deny", names)
        packages = {package["name"]: package for package in spdx["packages"]}
        self.assertEqual(
            packages["example"]["downloadLocation"],
            "registry+https://github.com/rust-lang/crates.io-index",
        )
        self.assertEqual(
            packages["example"]["checksums"],
            [{"algorithm": "SHA256", "checksumValue": REGISTRY_CHECKSUM}],
        )
        self.assertEqual(
            packages["from-git"]["downloadLocation"],
            "git+https://example.local/from-git?rev=deadbeef#deadbeef",
        )
        self.assertNotIn("checksums", packages["from-git"])
        self.assertEqual(packages["local-crate"]["downloadLocation"], "NOASSERTION")
        for package in spdx["packages"]:
            self.assertEqual(package["licenseConcluded"], "NOASSERTION")
            self.assertEqual(package["licenseDeclared"], "NOASSERTION")
            self.assertIs(package["filesAnalyzed"], False)

        provenance = json.loads((output / supply.PROVENANCE_NAME).read_text(encoding="utf-8"))
        provenance_text = (output / supply.PROVENANCE_NAME).read_text(encoding="utf-8").lower()
        self.assertEqual(provenance["_type"], "https://in-toto.io/Statement/v1")
        self.assertEqual(
            provenance["predicate"]["classification"],
            "unqualified-candidate-provenance",
        )
        self.assertEqual(
            provenance["subject"],
            [
                {
                    "digest": {"sha256": first["candidate_sha256"]},
                    "name": "packages/narrivy.deb",
                }
            ],
        )
        self.assertEqual(provenance["predicate"]["source_commit"], head)
        self.assertEqual(provenance["predicate"]["source_date_epoch"], SAMPLE_EPOCH)
        self.assertEqual(provenance["predicate"]["version"], SAMPLE_VERSION)
        self.assertEqual(provenance["predicate"]["cargo_lock_hash"], first["cargo_lock_hash"])
        self.assertEqual(provenance["predicate"]["embedded"], EMBEDDED)
        self.assertEqual(
            provenance["predicate"]["builder"],
            {
                "image": "narrivy-test-builder",
                "image_digest": "unverified-test-image",
                "os": "Ubuntu 24.04",
                "toolchain": "rustc 1.97.1 + cargo 1.97.1",
            },
        )
        self.assertNotIn("reproducible", provenance_text)
        self.assertNotIn("slsa", provenance_text)
        self.assertNotIn("signed", provenance_text)
        self.assertNotIn("qualified", provenance_text.replace("unqualified", ""))

        manifest = json.loads(
            (output / supply.RELEASE_ARTIFACTS_NAME).read_text(encoding="utf-8")
        )
        _validate_schema_v3(manifest)
        self.assertEqual(manifest["schema_version"], 3)
        self.assertEqual(manifest["git_commit"], head)
        self.assertIs(manifest["git_dirty"], False)
        self.assertEqual(manifest["signing"]["signed"], False)
        self.assertEqual(manifest["signing"]["status"], "not-supported")
        files = {
            "packages/narrivy.deb": candidate.read_bytes(),
            "bundle/spdx.json": (output / supply.SPDX_NAME).read_bytes(),
            "bundle/provenance.json": (output / supply.PROVENANCE_NAME).read_bytes(),
        }
        by_path = {artifact["path"]: artifact for artifact in manifest["artifacts"]}
        for path, payload in files.items():
            artifact = by_path[path]
            self.assertEqual(artifact["sha256"], hashlib.sha256(payload).hexdigest())
            self.assertEqual(artifact["size_bytes"], len(payload))
        self.assertEqual(by_path["packages/narrivy.deb"]["kind"], "deb")
        self.assertEqual(by_path["bundle/spdx.json"]["kind"], "sbom")
        self.assertEqual(by_path["bundle/provenance.json"]["kind"], "provenance")
        self.assertEqual(manifest["sbom_path"], "bundle/spdx.json")
        self.assertEqual(manifest["provenance_path"], "bundle/provenance.json")
        self.assertEqual(
            first["files"][supply.SPDX_NAME]["sha256"],
            hashlib.sha256(files["bundle/spdx.json"]).hexdigest(),
        )
        self.assertEqual(
            first["files"][supply.PROVENANCE_NAME]["sha256"],
            hashlib.sha256(files["bundle/provenance.json"]).hexdigest(),
        )
        self.assertEqual(
            first["files"][supply.RELEASE_ARTIFACTS_NAME]["sha256"],
            hashlib.sha256((output / supply.RELEASE_ARTIFACTS_NAME).read_bytes()).hexdigest(),
        )

        argv = [
            "--candidate",
            str(candidate),
            "--source-root",
            str(repo),
            "--output",
            str(repo / "cli-bundle"),
            "--expected-sha256",
            first["candidate_sha256"],
            "--builder-os",
            "Ubuntu 24.04",
            "--builder-image",
            "narrivy-test-builder",
            "--builder-image-digest",
            "unverified-test-image",
            "--toolchain",
            "rustc 1.97.1 + cargo 1.97.1",
        ]
        stdout = io.StringIO()
        with redirect_stdout(stdout):
            status = supply.main(argv)
        self.assertEqual(status, 0)
        printed = stdout.getvalue()
        result_text = (repo / "cli-bundle" / supply.RESULT_NAME).read_text(encoding="utf-8")
        self.assertEqual(printed, result_text)
        self.assertNotIn("HOME", printed)
        self.assertNotIn("USER", printed)
        self.assertNotIn(LOCKFILE, printed)

    def test_digest_mismatch_fails_closed(self) -> None:
        repo, head = self._repo()
        candidate = self._write_candidate(repo, self._debian(head))
        output = repo / "bundle"
        with self.assertRaises(supply.SupplyChainError) as raised:
            supply.build(
                self._inputs(repo, candidate, output, expected="ab" * 32)
            )
        self.assertIn("SHA-256", str(raised.exception))
        self._assert_no_output(output, repo)

    def test_symlink_candidate_fails_closed(self) -> None:
        repo, head = self._repo()
        real = self._write_candidate(repo, self._debian(head), "real.deb")
        link = repo / "link.deb"
        link.symlink_to(real)
        output = repo / "bundle"
        with self.assertRaises(supply.SupplyChainError) as raised:
            supply.build(self._inputs(repo, link, output))
        self.assertIn("symlink", str(raised.exception))
        self._assert_no_output(output, repo)

    def test_wrong_repository_fails_closed(self) -> None:
        repo, head = self._repo("one")
        other, _other_head = self._repo("two")
        payload = self._debian(head)
        candidate = self._write_candidate(other, payload)
        output = other / "bundle"
        with self.assertRaises(supply.SupplyChainError) as raised:
            supply.build(self._inputs(other, candidate, output))
        self.assertIn("source commit", str(raised.exception))
        self._assert_no_output(output, other)
        nested = repo / "nested"
        nested.mkdir()
        nested_candidate = self._write_candidate(repo, payload)
        with self.assertRaises(supply.SupplyChainError) as raised:
            supply.build(self._inputs(nested, nested_candidate, repo / "bundle"))
        self.assertIn("toplevel", str(raised.exception))

    def test_missing_build_info_fails_closed(self) -> None:
        repo, head = self._repo()
        payload = self._debian(
            head,
            data_entries=[("usr/share/narrivy/readme.txt", b"ok\n", "file")],
        )
        candidate = self._write_candidate(repo, payload)
        output = repo / "bundle"
        with self.assertRaises(supply.SupplyChainError) as raised:
            supply.build(self._inputs(repo, candidate, output))
        self.assertIn("missing", str(raised.exception))
        self._assert_no_output(output, repo)

    def test_duplicate_nonregular_oversized_unsafe_build_info_fail_closed(self) -> None:
        repo, head = self._repo()
        output = repo / "bundle"
        cases = [
            (
                self._debian(head, duplicate_build_info=True),
                "duplicated",
            ),
            (
                self._debian(
                    head,
                    data_entries=[
                        (supply.BUILD_INFO_MEMBER, b"target", "symlink"),
                    ],
                ),
                "regular",
            ),
            (
                self._debian(
                    head,
                    data_entries=[
                        (supply.BUILD_INFO_MEMBER, None, "directory"),
                    ],
                ),
                "regular",
            ),
            (
                self._debian(
                    head,
                    build_info=b"x" * (supply.MAX_BUILD_INFO_BYTES + 1),
                ),
                "oversized",
            ),
            (
                self._debian(
                    head,
                    data_entries=[
                        (
                            "/usr/share/narrivy/build-info.json",
                            self._build_info(head),
                            "file",
                        )
                    ],
                ),
                "unsafe",
            ),
            (
                self._debian(
                    head,
                    data_entries=[
                        (
                            "usr/share/narrivy/tmp/../build-info.json",
                            self._build_info(head),
                            "file",
                        )
                    ],
                ),
                "unsafe",
            ),
        ]
        for index, (payload, fragment) in enumerate(cases):
            with self.subTest(fragment=fragment, index=index):
                candidate = self._write_candidate(repo, payload, f"case-{index}.deb")
                with self.assertRaises(supply.SupplyChainError) as raised:
                    supply.build(self._inputs(repo, candidate, output))
                self.assertIn(fragment, str(raised.exception))
                self._assert_no_output(output, repo)

    def test_dirty_and_default_build_identity_fail_closed(self) -> None:
        repo, head = self._repo()
        output = repo / "bundle"
        dirty = self._debian(
            head,
            build_info=self._build_info(
                head,
                schema_version=1,
                classification="installable-unqualified-candidate",
                git_dirty=True,
            ),
        )
        default = self._debian(
            head,
            build_info=self._build_info(
                head,
                schema_version=1,
                classification="installable-unqualified-candidate",
            ),
        )
        for payload, name in ((dirty, "dirty.deb"), (default, "default.deb")):
            candidate = self._write_candidate(repo, payload, name)
            with self.assertRaises(supply.SupplyChainError) as raised:
                supply.build(self._inputs(repo, candidate, output))
            self.assertIn("clean unqualified candidate", str(raised.exception))
            self._assert_no_output(output, repo)

    def test_archive_traversal_bounds_and_epoch_fail_closed(self) -> None:
        repo, head = self._repo()
        output = repo / "bundle"
        too_many = [
            (f"usr/share/narrivy/filler-{index}", b"x", "file")
            for index in range(supply.MAX_TAR_MEMBERS)
        ]
        candidate = self._write_candidate(
            repo,
            self._debian(head, extra_data=too_many),
            "too-many.deb",
        )
        with self.assertRaisesRegex(supply.SupplyChainError, "bounded tar limits"):
            supply.build(self._inputs(repo, candidate, output))
        self._assert_no_output(output, repo)

        invalid_epoch = self._build_info(
            head, source_date_epoch=supply.MAX_SOURCE_DATE_EPOCH + 1
        )
        candidate = self._write_candidate(
            repo,
            self._debian(head, build_info=invalid_epoch),
            "invalid-epoch.deb",
        )
        with self.assertRaisesRegex(supply.SupplyChainError, "clean unqualified"):
            supply.build(self._inputs(repo, candidate, output))
        self._assert_no_output(output, repo)

    def test_nonexistent_source_commit_fails_closed(self) -> None:
        repo, head = self._repo()
        missing = "ab" * 20
        self.assertNotEqual(missing, head)
        candidate = self._write_candidate(repo, self._debian(missing))
        output = repo / "bundle"
        with self.assertRaises(supply.SupplyChainError) as raised:
            supply.build(self._inputs(repo, candidate, output))
        self.assertIn("source commit", str(raised.exception))
        self._assert_no_output(output, repo)

    def test_malformed_lockfile_fails_closed(self) -> None:
        repo, _head = self._repo("bad-lock", lockfile="this is not toml {")
        head = self._git(repo, "rev-parse", "--verify", "HEAD").stdout.strip()
        candidate = self._write_candidate(repo, self._debian(head))
        output = repo / "bundle"
        with self.assertRaises(supply.SupplyChainError) as raised:
            supply.build(self._inputs(repo, candidate, output))
        self.assertIn("Cargo.lock is malformed", str(raised.exception))
        self._assert_no_output(output, repo)
        incomplete = self.root / "incomplete"
        incomplete.mkdir()
        (incomplete / "Cargo.lock").write_text(
            '[[package]]\nname = "example"\n', encoding="utf-8"
        )
        self._git(incomplete, "init", "--initial-branch=main")
        self._git(incomplete, "add", "Cargo.lock")
        self._git(incomplete, "commit", "--no-gpg-sign", "-m", "bad")
        incomplete_head = self._git(incomplete, "rev-parse", "--verify", "HEAD").stdout.strip()
        incomplete_candidate = self._write_candidate(
            incomplete, self._debian(incomplete_head)
        )
        incomplete_output = incomplete / "bundle"
        with self.assertRaises(supply.SupplyChainError) as raised:
            supply.build(self._inputs(incomplete, incomplete_candidate, incomplete_output))
        self.assertIn("Cargo.lock is malformed", str(raised.exception))
        self._assert_no_output(incomplete_output, incomplete)

    def test_existing_output_fails_closed(self) -> None:
        repo, head = self._repo()
        candidate = self._write_candidate(repo, self._debian(head))
        output = repo / "bundle"
        output.mkdir()
        marker = output / "keep.txt"
        marker.write_text("keep\n", encoding="utf-8")
        with self.assertRaises(supply.SupplyChainError) as raised:
            supply.build(self._inputs(repo, candidate, output))
        self.assertIn("already exists", str(raised.exception))
        self.assertEqual(marker.read_text(encoding="utf-8"), "keep\n")
        self.assertEqual(list(output.iterdir()), [marker])

    def test_path_escape_fails_closed(self) -> None:
        repo, head = self._repo()
        payload = self._debian(head)
        outside = self.root / "outside.deb"
        outside.write_bytes(payload)
        output = repo / "bundle"
        with self.assertRaises(supply.SupplyChainError) as raised:
            supply.build(self._inputs(repo, outside, output))
        self.assertIn("outside the repository", str(raised.exception))
        self._assert_no_output(output, repo)
        candidate = self._write_candidate(repo, payload)
        outside_output = self.root / "escaped-bundle"
        with self.assertRaises(supply.SupplyChainError) as raised:
            supply.build(self._inputs(repo, candidate, outside_output))
        self.assertIn("outside the repository", str(raised.exception))
        self.assertFalse(os.path.lexists(outside_output))
        with self.assertRaises(supply.SupplyChainError) as raised:
            supply.build(
                self._inputs(repo, Path("relative.deb"), output)
            )
        self.assertIn("absolute", str(raised.exception))

    def test_git_timeout_and_failure_fail_closed(self) -> None:
        repo, head = self._repo()
        candidate = self._write_candidate(repo, self._debian(head))
        output = repo / "bundle"
        sleeper = self._write_executable("sleep-git", "#!/bin/sh\nexec sleep 8\n")
        with self.assertRaises(supply.SupplyChainError) as timed_out:
            supply.build(
                self._inputs(repo, candidate, output, git=str(sleeper), timeout=0.05)
            )
        self.assertIn("timed out", str(timed_out.exception))
        self._assert_no_output(output, repo)
        failing = self._write_executable("fail-git", "#!/bin/sh\nexit 1\n")
        with self.assertRaises(supply.SupplyChainError) as failed:
            supply.build(self._inputs(repo, candidate, output, git=str(failing)))
        self.assertIn("failed", str(failed.exception))
        self._assert_no_output(output, repo)
        record = self.root / "git-env.json"
        recorder = self._write_executable(
            "record-git",
            f"#!{sys.executable}\n"
            "import json, os, sys\n"
            f"json.dump({{'argv': sys.argv, 'env': dict(os.environ)}}, "
            f"open({str(record)!r}, 'w'), sort_keys=True)\n",
        )
        output_text = supply._run_git(
            repo,
            ("rev-parse", "--show-toplevel"),
            git=str(recorder),
            timeout=supply.GIT_TIMEOUT_SECONDS,
            maximum=supply.MAX_GIT_TEXT_BYTES,
        )
        self.assertEqual(output_text, b"")
        recorded = json.loads(record.read_text(encoding="utf-8"))
        self.assertEqual(
            recorded["argv"][1:],
            ["-C", os.fspath(repo), "rev-parse", "--show-toplevel"],
        )
        self.assertEqual(recorded["env"], supply.GIT_ENV)
        self.assertNotIn("HOME", recorded["env"])
        self.assertNotIn("GIT_DIR", recorded["env"])

    def test_no_partial_output_on_failure(self) -> None:
        repo, head = self._repo()
        candidate = self._write_candidate(repo, self._debian(head))
        output = repo / "bundle"
        with mock.patch("os.rename", side_effect=OSError("rename failed")):
            with self.assertRaises(supply.SupplyChainError):
                supply.build(self._inputs(repo, candidate, output))
        self._assert_no_output(output, repo)
        with mock.patch.object(
            supply,
            "_canonical_json",
            side_effect=supply.SupplyChainError("encode failed"),
        ):
            with self.assertRaises(supply.SupplyChainError):
                supply.build(self._inputs(repo, candidate, output))
        self._assert_no_output(output, repo)

    def test_cli_requires_explicit_nonempty_builder_identity(self) -> None:
        argv = [
            "--candidate",
            "/abs/candidate.deb",
            "--source-root",
            "/abs/source",
            "--output",
            "/abs/output",
            "--expected-sha256",
            "ab" * 32,
            "--builder-os",
            "Ubuntu 24.04",
            "--builder-image",
            "image",
            "--builder-image-digest",
            "digest",
            "--toolchain",
            "toolchain",
        ]
        parsed = supply._arguments(argv)
        self.assertEqual(parsed.builder_os, "Ubuntu 24.04")
        self.assertNotIn("reproducible", supply.Inputs.__dataclass_fields__)
        self.assertNotIn("signed", supply.Inputs.__dataclass_fields__)
        self.assertNotIn("slsa", supply.Inputs.__dataclass_fields__)
        with self.assertRaises(SystemExit), redirect_stderr(io.StringIO()):
            supply._arguments(argv[:-2])
        with self.assertRaises(SystemExit), redirect_stderr(io.StringIO()):
            supply._arguments([*argv[:-1], ""])
        with self.assertRaises(SystemExit), redirect_stderr(io.StringIO()):
            supply._arguments(
                [
                    *argv,
                    "--reproducible",
                ]
            )


if __name__ == "__main__":
    unittest.main()
