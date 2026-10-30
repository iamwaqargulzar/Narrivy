from __future__ import annotations

import io
import json
import os
import stat
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stderr
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

import build_deb_installable as installable  # noqa: E402

CLI_INPUT_FLAGS = (
    "narrivyd",
    "cli",
    "desktop",
    "worker",
    "model",
    "service",
    "desktop-entry",
    "postinst",
    "prerm",
    "postrm",
    "icon-32",
    "icon-128",
    "icon-256",
    "icon-512",
    "license-file",
    "notices",
    "output",
)
SAMPLE_BINARIES = {
    "narrivyd": {"sha256": "aa" * 32, "size_bytes": 11},
    "narrivy-cli": {"sha256": "bb" * 32, "size_bytes": 22},
    "narrivy-desktop": {"sha256": "cc" * 32, "size_bytes": 33},
}
SAMPLE_WORKER = {"sha256": "dd" * 32, "size_bytes": 44}
SAMPLE_HEAD = "ab" * 20
SAMPLE_EPOCH = 1_700_000_000
SAMPLE_VERSION = "1.0.0~rc1"


def _cli_argv(*extra: str) -> list[str]:
    argv: list[str] = []
    for name in CLI_INPUT_FLAGS:
        argv.extend((f"--{name}", "/abs/narrivy-input"))
    argv.extend(
        (
            "--version",
            SAMPLE_VERSION,
            "--source-head",
            SAMPLE_HEAD,
            "--source-date-epoch",
            str(SAMPLE_EPOCH),
        )
    )
    argv.extend(extra)
    return argv


class InstallableDebBuilderTests(unittest.TestCase):
    def test_control_names_the_maintained_runtime_and_exact_candidate_version(self) -> None:
        control = installable._control("1.0.0~rc1").decode()
        self.assertIn("Version: 1.0.0~rc1\n", control)
        self.assertIn("libwebkit2gtk-4.1-0 (>= 2.44)", control)
        self.assertIn("libgtk-layer-shell0 (>= 0.8)", control)
        self.assertNotIn("libwebkitgtk-6.0", control)
        self.assertIn("libopenblas0", control)
        # The worker links the Vulkan loader (ADR-0076); drivers are optional.
        self.assertIn("libvulkan1", control)
        # Downloads use the system download tool (ADR-0077).
        self.assertIn(" curl,", control)
        self.assertRegex(control, r"Recommends: .*mesa-vulkan-drivers")
        # Typing into the focused app needs the root-owned system wtype (F-C4).
        recommends = next(
            line for line in control.splitlines() if line.startswith("Recommends: ")
        )
        self.assertIn("wtype", recommends.removeprefix("Recommends: ").split(", "))
        # gdbus: the CLI's notification when the daemon cannot start (F-H4).
        self.assertIn("libglib2.0-bin", recommends.removeprefix("Recommends: ").split(", "))
        for template in ("packaging/deb/control", "packaging/linux/deb/control"):
            self.assertRegex(
                (ROOT / template).read_text(), r"(?m)^Recommends: (.*, )?wtype(,|$)"
            )
        self.assertIn("systemd", control)
        self.assertNotIn("tauri", control.lower())

    def test_package_identity_is_d3(self) -> None:
        # F-M6: the owner's maintainer and homepage, no placeholder host, no
        # invalid `Build-Using` field, and the same identity in the templates.
        control = installable._control("1.0.0~rc1").decode()
        for text in (
            control,
            (ROOT / "packaging/deb/control").read_text(),
            (ROOT / "packaging/linux/deb/control").read_text(),
        ):
            self.assertIn("Maintainer: Waqar Gulzar <iamwaqargulzar@gmail.com>\n", text)
            self.assertIn("Homepage: https://github.com/iamwaqargulzar/narrivy\n", text)
            self.assertNotIn("narrivy.local", text)
            self.assertNotIn("Build-Using", text)
        # Versions follow D3 and sort above the 0.2.3 build already installed.
        for good in ("1.0.0~rc1", "1.0.0~rc12", "1.0.0", "1.2.3"):
            self.assertTrue(installable.VERSION_RE.fullmatch(good), good)
        for bad in ("0.1.0~rc-test", "1.0.0~beta1", "1.0", "01.0.0", "1.0.0~rc0", "1.0.0+git"):
            self.assertIsNone(installable.VERSION_RE.fullmatch(bad), bad)

    def test_control_templates_match_the_builder(self) -> None:
        # F-M6: the checked-in templates are what the builder writes, at the
        # desktop app's current release version.
        tauri = json.loads(
            (ROOT / "apps/narrivy-desktop/src-tauri/tauri.conf.json").read_text()
        )
        release = tauri["version"].replace("-rc.", "~rc")
        expected = installable._control(release).decode()
        for template in ("packaging/deb/control", "packaging/linux/deb/control"):
            self.assertEqual((ROOT / template).read_text(), expected, template)

    def test_maintainer_scripts_stop_and_reload_running_user_daemons(self) -> None:
        # F-H15: remove/upgrade stop the daemon in every running user manager;
        # configure reloads and restarts a running one; nothing is enabled
        # for all users (D5); purge names every user data directory.
        for directory in ("packaging/deb", "packaging/linux/deb"):
            prerm = (ROOT / directory / "prerm").read_text()
            postinst = (ROOT / directory / "postinst").read_text()
            postrm = (ROOT / directory / "postrm").read_text()
            installable._validate_script(prerm.encode())
            installable._validate_script(postinst.encode())
            installable._validate_script(postrm.encode())
            self.assertIn("remove|deconfigure|upgrade|failed-upgrade)", prerm)
            self.assertIn(
                'timeout 10s systemctl --user -M "$uid@" stop narrivy-daemon.service',
                prerm,
            )
            self.assertIn(
                'timeout 10s systemctl --user -M "$uid@" try-restart narrivy-daemon.service',
                postinst,
            )
            self.assertIn('systemctl --user -M "$uid@" daemon-reload', postinst)
            self.assertIn('systemctl --user -M "$uid@" daemon-reload', postrm)
            for script in (prerm, postinst, postrm):
                self.assertNotIn("--global", script)
                self.assertNotRegex(script, r"systemctl[^\n]* enable ")
                self.assertNotIn("narrivy-migrate", script)
                for line in script.splitlines():
                    if "systemctl --user" in line:
                        self.assertIn("timeout 10s", line)
                        self.assertTrue(line.rstrip().endswith("|| true"), line)
            self.assertIn("~/.local/state/narrivy", postrm)

    def test_maintainer_script_validation_is_bounded_and_fail_closed(self) -> None:
        installable._validate_script(b"#!/bin/sh\nset -e\nexit 0\n")
        with self.assertRaises(installable.InstallableError):
            installable._validate_script(b"#!/bin/sh\nif then\n")
        with self.assertRaises(installable.InstallableError):
            installable._validate_script(b"#!/bin/sh\nprintf 'bad\x00script'\n")

    def test_publication_never_overwrites_or_deletes_an_existing_output(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "source"
            destination = root / "candidate.deb"
            source.write_bytes(b"new archive")
            destination.write_bytes(b"existing archive")
            descriptor = os.open(source, os.O_RDONLY | os.O_CLOEXEC)
            try:
                with self.assertRaises(FileExistsError):
                    installable._publish(descriptor, destination.resolve())
            finally:
                os.close(descriptor)
            self.assertEqual(destination.read_bytes(), b"existing archive")

    def test_bundled_model_identity_is_release_pinned(self) -> None:
        self.assertEqual(installable.MODEL_ID, "whisper-tiny.en-Q8_0")
        self.assertEqual(installable.MODEL_SIZE, 45_904_544)
        self.assertRegex(installable.MODEL_SHA256, r"^[0-9a-f]{64}$")

    def test_default_cli_and_identity_are_unchanged(self) -> None:
        parsed = installable._arguments(_cli_argv())
        self.assertIsNone(parsed.source_root)
        self.assertEqual(parsed.version, SAMPLE_VERSION)
        self.assertEqual(parsed.source_head, SAMPLE_HEAD)
        self.assertEqual(parsed.source_date_epoch, SAMPLE_EPOCH)
        identity = installable._package_identity(None)
        self.assertEqual(identity.classification, "installable-unqualified-candidate")
        self.assertIs(identity.git_dirty, True)
        self.assertEqual(identity.schema_version, 1)
        self.assertEqual(
            identity.build_info_relative,
            "usr/share/narrivy/candidate-build-info.json",
        )
        self.assertNotIn("git_dirty", installable.Inputs.__dataclass_fields__)
        self.assertNotIn("clean", installable.Inputs.__dataclass_fields__)
        self.assertNotIn("clean_source", installable.Inputs.__dataclass_fields__)


class CleanSourceInstallableTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def _git(self, repo: Path, *arguments: str) -> subprocess.CompletedProcess[str]:
        env = {
            **installable.GIT_ENV,
            "GIT_AUTHOR_NAME": "Narrivy Tests",
            "GIT_AUTHOR_EMAIL": "tests@narrivy.local",
            "GIT_COMMITTER_NAME": "Narrivy Tests",
            "GIT_COMMITTER_EMAIL": "tests@narrivy.local",
        }
        return subprocess.run(
            [installable.GIT, "-C", os.fspath(repo), *arguments],
            check=True,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            env=env,
            timeout=installable.GIT_TIMEOUT_SECONDS,
        )

    def _clean_repo(self, name: str = "repo") -> tuple[Path, str]:
        repo = self.root / name
        repo.mkdir()
        (repo / "tracked.txt").write_text("narrivy\n", encoding="utf-8")
        self._git(repo, "init", "--initial-branch=main")
        self._git(repo, "add", "tracked.txt")
        self._git(repo, "commit", "--no-gpg-sign", "-m", "init")
        head = self._git(repo, "rev-parse", "--verify", "HEAD").stdout.strip()
        return repo, head

    def _write_executable(self, name: str, body: str) -> Path:
        path = self.root / name
        path.write_text(body, encoding="utf-8")
        path.chmod(path.stat().st_mode | stat.S_IXUSR)
        return path

    def _inputs(
        self, *, source_root: Path | None, source_head: str
    ) -> installable.Inputs:
        dummy = self.root / "missing-input"
        return installable.Inputs(
            narrivyd=dummy,
            cli=dummy,
            desktop=dummy,
            worker=dummy,
            model=dummy,
            service=dummy,
            desktop_entry=dummy,
            postinst=dummy,
            prerm=dummy,
            postrm=dummy,
            icon_32=dummy,
            icon_128=dummy,
            icon_256=dummy,
            icon_512=dummy,
            license_file=dummy,
            notices=dummy,
            output=dummy,
            version=SAMPLE_VERSION,
            source_head=source_head,
            source_date_epoch=SAMPLE_EPOCH,
            source_root=source_root,
        )

    def _expected_build_info(
        self, identity: installable.PackageIdentity
    ) -> dict[str, object]:
        return {
            "classification": identity.classification,
            "git_dirty": identity.git_dirty,
            "inputs": {
                **SAMPLE_BINARIES,
                "resident-worker": SAMPLE_WORKER,
                "bundled-model": {
                    "sha256": installable.MODEL_SHA256,
                    "size_bytes": installable.MODEL_SIZE,
                },
            },
            "package": installable.PACKAGE_NAME,
            "schema_version": identity.schema_version,
            "source_date_epoch": SAMPLE_EPOCH,
            "source_head": SAMPLE_HEAD,
            "version": SAMPLE_VERSION,
        }

    def test_clean_source_succeeds_for_an_empty_tree(self) -> None:
        repo, head = self._clean_repo()
        installable._require_clean_source(repo, head)

    def test_dirty_tracked_state_fails_closed(self) -> None:
        repo, head = self._clean_repo()
        (repo / "tracked.txt").write_text("changed\n", encoding="utf-8")
        with self.assertRaises(installable.InstallableError) as raised:
            installable._require_clean_source(repo, head)
        self.assertIn("not clean", str(raised.exception))

    def test_untracked_state_fails_closed(self) -> None:
        repo, head = self._clean_repo()
        (repo / "extra.txt").write_text("untracked\n", encoding="utf-8")
        with self.assertRaises(installable.InstallableError) as raised:
            installable._require_clean_source(repo, head)
        self.assertIn("not clean", str(raised.exception))

    def test_head_mismatch_fails_closed(self) -> None:
        repo, head = self._clean_repo()
        other = "0" * 40
        self.assertNotEqual(head, other)
        with self.assertRaises(installable.InstallableError) as raised:
            installable._require_clean_source(repo, other)
        self.assertIn("HEAD", str(raised.exception))

    def test_non_repository_and_wrong_toplevel_fail_closed(self) -> None:
        missing = self.root / "not-a-repo"
        missing.mkdir()
        with self.assertRaises(installable.InstallableError):
            installable._require_clean_source(missing, SAMPLE_HEAD)
        relative = Path("relative-source")
        with self.assertRaises(installable.InstallableError) as raised:
            installable._require_clean_source(relative, SAMPLE_HEAD)
        self.assertIn("absolute", str(raised.exception))
        repo, head = self._clean_repo()
        nested = repo / "nested"
        nested.mkdir()
        with self.assertRaises(installable.InstallableError) as raised:
            installable._require_clean_source(nested, head)
        self.assertIn("toplevel", str(raised.exception))

    def test_git_timeout_and_failure_fail_closed(self) -> None:
        repo, head = self._clean_repo()
        sleeper = self._write_executable("sleep-git", "#!/bin/sh\nexec sleep 8\n")
        with self.assertRaises(installable.InstallableError) as timed_out:
            installable._require_clean_source(
                repo, head, git=str(sleeper), timeout=0.05
            )
        self.assertIn("timed out", str(timed_out.exception))
        failing = self._write_executable("fail-git", "#!/bin/sh\nexit 1\n")
        with self.assertRaises(installable.InstallableError) as failed:
            installable._require_clean_source(repo, head, git=str(failing))
        self.assertIn("failed", str(failed.exception))

    def test_only_a_resident_daemon_is_packaged(self) -> None:
        # F-M14: the daemon's own `--build-features` report decides.
        resident = self._write_executable(
            "resident-narrivyd",
            "#!/bin/sh\n[ \"$1\" = --build-features ] && echo asr-transcribe-cpp asr-transcribe-resident\n",
        )
        installable._require_resident_daemon(resident)
        for name, body in (
            ("plain-narrivyd", "#!/bin/sh\necho\n"),
            ("sidecar-narrivyd", "#!/bin/sh\necho asr-transcribe-cpp\n"),
            ("failing-narrivyd", "#!/bin/sh\necho asr-transcribe-resident; exit 3\n"),
            ("hanging-narrivyd", "#!/bin/sh\nsleep 5\n"),
        ):
            daemon = self._write_executable(name, body)
            with self.assertRaises(installable.InstallableError, msg=name):
                installable._require_resident_daemon(daemon, timeout=0.5)
        with self.assertRaises(installable.InstallableError):
            installable._require_resident_daemon(self.root / "missing-narrivyd")

    def test_git_environment_is_scrubbed_and_argv_only(self) -> None:
        record = self.root / "git-env.json"
        recorder = self._write_executable(
            "record-git",
            f"#!{sys.executable}\n"
            "import json, os, sys\n"
            f"json.dump({{'argv': sys.argv, 'env': dict(os.environ)}}, "
            f"open({str(record)!r}, 'w'), sort_keys=True)\n",
        )
        output = installable._run_git(
            self.root, ("rev-parse", "--show-toplevel"), git=str(recorder)
        )
        self.assertEqual(output, "")
        recorded = json.loads(record.read_text(encoding="utf-8"))
        self.assertEqual(
            recorded["argv"][1:],
            ["-C", os.fspath(self.root), "rev-parse", "--show-toplevel"],
        )
        self.assertEqual(recorded["env"], installable.GIT_ENV)
        self.assertNotIn("HOME", recorded["env"])
        self.assertNotIn("GIT_DIR", recorded["env"])

    def test_direct_library_clean_mode_validates_source_root(self) -> None:
        repo, head = self._clean_repo()
        (repo / "tracked.txt").write_text("dirty-library\n", encoding="utf-8")
        with self.assertRaises(installable.InstallableError) as raised:
            installable.build(self._inputs(source_root=repo, source_head=head))
        self.assertIn("not clean", str(raised.exception))
        clean_repo, clean_head = self._clean_repo("clean-repo")
        with self.assertRaises((installable.safe.CandidateError, OSError)):
            installable.build(
                self._inputs(source_root=clean_repo, source_head=clean_head)
            )

    def test_clean_and_default_build_info_payloads_and_paths(self) -> None:
        default_identity = installable._package_identity(None)
        clean_identity = installable._package_identity(self.root)
        default_payload = installable._build_info_payload(
            default_identity,
            version=SAMPLE_VERSION,
            source_head=SAMPLE_HEAD,
            source_date_epoch=SAMPLE_EPOCH,
            binaries=SAMPLE_BINARIES,
            worker=SAMPLE_WORKER,
        )
        clean_payload = installable._build_info_payload(
            clean_identity,
            version=SAMPLE_VERSION,
            source_head=SAMPLE_HEAD,
            source_date_epoch=SAMPLE_EPOCH,
            binaries=SAMPLE_BINARIES,
            worker=SAMPLE_WORKER,
        )
        self.assertEqual(
            default_identity.build_info_relative,
            "usr/share/narrivy/candidate-build-info.json",
        )
        self.assertEqual(
            clean_identity.build_info_relative,
            "usr/share/narrivy/build-info.json",
        )
        self.assertNotEqual(
            default_identity.build_info_relative,
            clean_identity.build_info_relative,
        )
        self.assertEqual(
            default_payload,
            installable.safe._canonical_json(self._expected_build_info(default_identity)),
        )
        self.assertEqual(
            clean_payload,
            installable.safe._canonical_json(self._expected_build_info(clean_identity)),
        )
        default_document = json.loads(default_payload)
        clean_document = json.loads(clean_payload)
        self.assertEqual(default_document["schema_version"], 1)
        self.assertEqual(
            default_document["classification"],
            "installable-unqualified-candidate",
        )
        self.assertIs(default_document["git_dirty"], True)
        self.assertEqual(clean_document["schema_version"], 2)
        self.assertEqual(
            clean_document["classification"],
            "clean-unqualified-release-candidate",
        )
        self.assertIs(clean_document["git_dirty"], False)
        self.assertEqual(clean_document["source_head"], SAMPLE_HEAD)
        self.assertEqual(clean_document["source_date_epoch"], SAMPLE_EPOCH)
        self.assertEqual(clean_document["version"], SAMPLE_VERSION)
        self.assertEqual(
            clean_document["inputs"]["narrivyd"], SAMPLE_BINARIES["narrivyd"]
        )
        self.assertEqual(clean_document["inputs"]["resident-worker"], SAMPLE_WORKER)
        self.assertEqual(
            clean_document["inputs"]["bundled-model"]["sha256"],
            installable.MODEL_SHA256,
        )
        self.assertNotIn("release-qualified", default_document["classification"])
        self.assertNotIn("release-qualified", clean_document["classification"])

    def test_cli_clean_source_requires_source_root(self) -> None:
        parsed = installable._arguments(
            _cli_argv("--clean-source", "--source-root", "/abs/source")
        )
        self.assertEqual(parsed.source_root, Path("/abs/source"))
        with self.assertRaises(SystemExit), redirect_stderr(io.StringIO()):
            installable._arguments(_cli_argv("--clean-source"))
        with self.assertRaises(SystemExit), redirect_stderr(io.StringIO()):
            installable._arguments(_cli_argv("--source-root", "/abs/source"))


if __name__ == "__main__":
    unittest.main()
