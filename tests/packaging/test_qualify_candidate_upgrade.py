from __future__ import annotations

import hashlib
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
sys.path.insert(0, str(Path(__file__).resolve().parent))

import qualify_candidate_upgrade as upgrade  # noqa: E402
import qualify_candidate as lifecycle  # noqa: E402
from qualify_candidate import CommandResult  # noqa: E402
from container_fake import ContainerFs, ok, unmodelled  # noqa: E402

PREVIOUS_BYTES = b"previous"
CURRENT_BYTES = b"current"
PREVIOUS_SHA = hashlib.sha256(PREVIOUS_BYTES).hexdigest()
CURRENT_SHA = hashlib.sha256(CURRENT_BYTES).hexdigest()
CONTAINER = "a" * 64


class Lifecycle(ContainerFs):
    """The container as a filesystem model: installing a package adds its
    files, removing or purging deletes them, the test user's sentinel files
    exist only once written and hash to what was written (F-M16: the old
    fake answered every sentinel check with the expected digest, so
    "preserved" could not fail)."""

    def __init__(self, *, fail_upgrade: bool = False) -> None:
        super().__init__()
        self.version = ""
        self.registered = False
        self.fail_upgrade = fail_upgrade
        self.users: dict[str, int] = {}
        # path -> (content, owner uid)
        self.contents: dict[str, tuple[bytes, int]] = {
            upgrade.PREVIOUS_IN_CONTAINER: (PREVIOUS_BYTES, 0),
            upgrade.CURRENT_IN_CONTAINER: (CURRENT_BYTES, 0),
        }
        self.files.update({path: 0o644 for path in self.contents})
        self.package = {
            path: 0o755 if path in lifecycle.EXECUTABLE_PATHS else 0o644
            for path in lifecycle.INSTALLED_PATHS
        }

    def write(self, path: str, content: bytes, uid: int) -> CommandResult:
        if path.rsplit("/", 1)[0] not in self.directories:
            return CommandResult(1, b"", b"No such file or directory")
        self.files[path] = 0o644
        self.contents[path] = (content, uid)
        return ok()

    def uninstall(self) -> None:
        for path in self.package:
            self.files.pop(path, None)

    def __call__(self, argv: tuple[str, ...]) -> CommandResult:
        if argv[:2] == ("sh", "-c"):
            return self.shell(argv[2])
        if argv[0] == "sha256sum":
            target = argv[1]
            if target not in self.contents or target not in self.files:
                return CommandResult(1, b"", b"No such file or directory")
            digest = hashlib.sha256(self.contents[target][0]).hexdigest()
            return ok(f"{digest}  {target}\n".encode())
        if argv[:2] == ("cat", "/etc/os-release"):
            return ok(b'ID=ubuntu\nVERSION_ID="24.04"\n')
        if argv[0] == "useradd":
            name = argv[-1]
            self.users[name] = 1000
            self.directories.add(f"/home/{name}")
            return ok()
        if argv[:2] == ("id", "-u"):
            if argv[2] not in self.users:
                return CommandResult(1, b"", b"no such user")
            return ok(f"{self.users[argv[2]]}\n".encode())
        if argv[0] == "runuser" and argv[1] == "-u" and argv[3] == "--":
            if argv[2] not in self.users:
                return CommandResult(1, b"", b"runuser: user does not exist")
            uid = self.users[argv[2]]
            command = argv[4:]
            if command[:2] == ("mkdir", "-p"):
                for path in command[2:]:
                    parts = path.split("/")
                    for end in range(2, len(parts) + 1):
                        self.directories.add("/".join(parts[:end]))
                return ok()
            if command[:3] == ("/bin/sh", "-c", upgrade.SENTINEL_WRITE):
                payload, path = command[4], command[5]
                return self.write(path, f"{payload}\n".encode(), uid)
            return unmodelled(command)
        if argv[0] == "apt-get" and argv[1] == "update":
            return ok()
        if argv[0] == "apt-get" and argv[1] == "install":
            package = argv[-1]
            if package not in self.files:
                return CommandResult(100, b"", b"E: Unable to locate package")
            if package == upgrade.CURRENT_IN_CONTAINER and self.fail_upgrade:
                return CommandResult(100, b"", b"")
            if package == upgrade.PREVIOUS_IN_CONTAINER and self.version == "0.1.0~rc2":
                if "--allow-downgrades" not in argv:
                    return CommandResult(100, b"", b"E: Packages were downgraded")
            self.version = "0.1.0~rc2" if package == upgrade.CURRENT_IN_CONTAINER else "0.1.0~rc1"
            self.registered = True
            self.files.update(self.package)
            return ok()
        if argv[0] == "apt-get" and argv[1] in ("remove", "purge") and argv[-1] == "narrivy":
            if not self.registered:
                return CommandResult(100, b"", b"E: Unable to locate package")
            self.uninstall()
            if argv[1] == "purge":
                self.registered = False
            return ok()
        if argv[0] == "dpkg-query" and "-f" in argv:
            return ok(self.version.encode()) if self.registered else CommandResult(1, b"", b"")
        if argv[0] == "dpkg-query":
            return ok() if self.registered else CommandResult(1, b"", b"")
        if argv[:3] == ("stat", "-c", "%u"):
            if argv[3] not in self.contents or argv[3] not in self.files:
                return CommandResult(1, b"", b"No such file or directory")
            return ok(f"{self.contents[argv[3]][1]}\n".encode())
        if argv[:2] == ("test", "-L"):
            return CommandResult(1, b"", b"")
        if argv[:2] == ("test", "-f"):
            return CommandResult(0 if argv[2] in self.files else 1, b"", b"")
        return unmodelled(argv)


class Runner:
    def __init__(self, lifecycle: Lifecycle) -> None:
        self.lifecycle = lifecycle
        self.removed: list[tuple[str, str]] = []
        self.copies: list[str] = []

    def ensure_image(self, image: str, *, inspect_timeout: float, pull_timeout: float) -> None:
        pass

    def create(self, image: str, name: str, labels: object, *, lifetime_seconds: int, timeout: float) -> str:
        return CONTAINER

    def copy(self, container: str, source: Path, destination: str, *, timeout: float) -> None:
        self.copies.append(destination)

    def start(self, container: str, *, timeout: float) -> None:
        pass

    def exec(self, container: str, argv: object, *, timeout: float, env: object = None, detached: bool = False) -> CommandResult:
        return self.lifecycle(tuple(argv))

    def remove(self, container: str, expected_candidate_sha256: str, *, timeout: float) -> None:
        self.removed.append((container, expected_candidate_sha256))


class UpgradeQualificationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.previous = self.root / "previous.deb"
        self.current = self.root / "current.deb"
        self.previous.write_bytes(PREVIOUS_BYTES)
        self.current.write_bytes(CURRENT_BYTES)
        self.result = self.root / "result.json"
        self.identities = ((PREVIOUS_SHA, len(PREVIOUS_BYTES), "0.1.0~rc1"), (CURRENT_SHA, len(CURRENT_BYTES), "0.1.0~rc2"))

    def tearDown(self) -> None:
        self.temp.cleanup()

    def run_flow(self, lifecycle: Lifecycle):
        runner = Runner(lifecycle)
        with patch.object(upgrade, "validate_inputs", return_value=self.identities):
            outcome = upgrade.qualify(self.previous, self.current, self.result, runner)
        return outcome, runner

    def test_successful_upgrade_rollback_and_removal_preserve_sentinels(self) -> None:
        outcome, runner = self.run_flow(Lifecycle())
        self.assertEqual(outcome["result"], "passed")
        self.assertEqual(outcome["previous"]["version"], "0.1.0~rc1")
        self.assertEqual(outcome["current"]["version"], "0.1.0~rc2")
        self.assertTrue(outcome["container"]["removed"])
        self.assertTrue(all(outcome["sentinels"][key] for key in (
            "preserved_after_upgrade", "preserved_after_rollback", "preserved_after_reupgrade",
            "preserved_after_remove", "preserved_after_purge",
        )))
        names = [step["step"] for step in outcome["steps"]]
        self.assertLess(names.index("confirm_upgrade"), names.index("confirm_rollback"))
        self.assertLess(names.index("confirm_rollback"), names.index("confirm_reupgrade"))
        self.assertEqual(names[-1], "container_remove")
        self.assertEqual(runner.removed, [(CONTAINER, upgrade._run_digest(PREVIOUS_SHA, CURRENT_SHA))])
        self.assertEqual(set(runner.copies), {upgrade.PREVIOUS_IN_CONTAINER, upgrade.CURRENT_IN_CONTAINER})

    def test_mid_upgrade_failure_is_truthful_and_container_is_removed(self) -> None:
        outcome, runner = self.run_flow(Lifecycle(fail_upgrade=True))
        self.assertEqual(outcome["result"], "failed")
        self.assertEqual(outcome["failed_step"], "apt_upgrade_current")
        self.assertTrue(outcome["container"]["removed"])
        self.assertEqual(len(runner.removed), 1)

    def test_lost_or_altered_user_data_and_leftover_files_fail(self) -> None:
        # F-M16: each of these passed against the old fake.
        class PurgeDeletesConfig(Lifecycle):
            def __call__(self, argv: tuple[str, ...]) -> CommandResult:
                result = super().__call__(argv)
                if argv[:2] == ("apt-get", "purge"):
                    self.files.pop(upgrade.SENTINEL_PATHS["config"], None)
                return result

        class UpgradeRewritesState(Lifecycle):
            def __call__(self, argv: tuple[str, ...]) -> CommandResult:
                result = super().__call__(argv)
                if argv[:2] == ("apt-get", "install") and argv[-1] == upgrade.CURRENT_IN_CONTAINER:
                    path = upgrade.SENTINEL_PATHS["state"]
                    if path in self.contents:
                        self.contents[path] = (b"migrated\n", self.contents[path][1])
                return result

        class UpgradeChownsCache(Lifecycle):
            def __call__(self, argv: tuple[str, ...]) -> CommandResult:
                result = super().__call__(argv)
                if argv[:2] == ("apt-get", "install") and argv[-1] == upgrade.CURRENT_IN_CONTAINER:
                    path = upgrade.SENTINEL_PATHS["cache"]
                    if path in self.contents:
                        self.contents[path] = (self.contents[path][0], 0)
                return result

        class RemoveLeavesTheCli(Lifecycle):
            def uninstall(self) -> None:
                super().uninstall()
                self.files["/usr/bin/narrivy-cli"] = 0o755

        class CurrentLacksTheWorker(Lifecycle):
            def __call__(self, argv: tuple[str, ...]) -> CommandResult:
                result = super().__call__(argv)
                if argv[:2] == ("apt-get", "install") and argv[-1] == upgrade.CURRENT_IN_CONTAINER:
                    self.files.pop("/usr/lib/narrivy/narrivy-transcribe-worker", None)
                return result

        for handler, step, detail in (
            (PurgeDeletesConfig(), "confirm_purge", "sentinel_missing=config"),
            (UpgradeRewritesState(), "confirm_upgrade", "sentinel_mismatch=state"),
            (UpgradeChownsCache(), "confirm_upgrade", "sentinel_owner=cache"),
            (RemoveLeavesTheCli(), "confirm_remove", "present=/usr/bin/narrivy-cli"),
            (
                CurrentLacksTheWorker(),
                "confirm_upgrade",
                "missing=/usr/lib/narrivy/narrivy-transcribe-worker",
            ),
        ):
            with self.subTest(handler=type(handler).__name__):
                self.result.unlink(missing_ok=True)
                outcome, runner = self.run_flow(handler)
                self.assertEqual(outcome["result"], "failed")
                self.assertEqual(outcome["failed_step"], step)
                self.assertIn(detail, outcome["failure_detail"])
                self.assertTrue(outcome["container"]["removed"])
                self.assertEqual(len(runner.removed), 1)

    def test_input_paths_fail_closed_before_container_work(self) -> None:
        self.result.write_text("occupied")
        with self.assertRaises(upgrade.InputError):
            upgrade.validate_inputs(self.previous.resolve(), self.current.resolve(), self.result.resolve())
        self.result.unlink()
        linked = self.root / "linked.deb"
        linked.symlink_to(self.previous)
        with self.assertRaises(upgrade.InputError):
            upgrade.validate_inputs(linked, self.current.resolve(), self.result.resolve())
        directory = self.root / "directory.deb"
        directory.mkdir()
        with self.assertRaises(upgrade.InputError):
            upgrade.validate_inputs(directory, self.current.resolve(), self.result.resolve())
        oversized = self.root / "oversized.deb"
        with oversized.open("wb") as stream:
            stream.truncate(upgrade.MAX_CANDIDATE_BYTES + 1)
        with self.assertRaises(upgrade.InputError):
            upgrade.validate_inputs(oversized, self.current.resolve(), self.result.resolve())
        malformed = self.root / "malformed.deb"
        malformed.write_bytes(b"not a deb")
        with self.assertRaises(upgrade.InputError):
            upgrade.validate_inputs(malformed.resolve(), self.current.resolve(), self.result.resolve())

    def test_same_or_non_increasing_versions_are_rejected(self) -> None:
        with self.assertRaises(upgrade.InputError):
            upgrade.validate_inputs(self.previous.resolve(), self.previous.resolve(), self.result.resolve())
        with patch.object(upgrade, "_package_identity", side_effect=[self.identities[1], self.identities[0]]):
            with self.assertRaises(upgrade.InputError):
                upgrade.validate_inputs(self.previous.resolve(), self.current.resolve(), self.result.resolve())

    def test_dpkg_version_order_handles_release_candidates(self) -> None:
        self.assertTrue(upgrade._version_is_greater("0.1.0~rc48+gabc", "0.1.0~rc47+gdef"))
        self.assertFalse(upgrade._version_is_greater("0.1.0~rc47", "0.1.0~rc48"))
        self.assertFalse(upgrade._version_is_greater("1.0", "1.0"))


if __name__ == "__main__":
    unittest.main()
