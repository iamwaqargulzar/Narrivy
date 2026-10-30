from __future__ import annotations

import hashlib
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

import qualify_candidate_upgrade as upgrade  # noqa: E402
from qualify_candidate import CommandResult  # noqa: E402

PREVIOUS_BYTES = b"previous"
CURRENT_BYTES = b"current"
PREVIOUS_SHA = hashlib.sha256(PREVIOUS_BYTES).hexdigest()
CURRENT_SHA = hashlib.sha256(CURRENT_BYTES).hexdigest()
CONTAINER = "a" * 64


def ok(stdout: bytes = b"") -> CommandResult:
    return CommandResult(0, stdout, b"")


class Lifecycle:
    def __init__(self, *, fail_upgrade: bool = False, status_fails_on: str | None = None) -> None:
        self.version = ""
        self.registered = False
        self.fail_upgrade = fail_upgrade
        self.status_fails_on = status_fails_on
        self.status_versions: list[str] = []

    def __call__(self, argv: tuple[str, ...]) -> CommandResult:
        if argv[0] == "/usr/bin/narrivy-cli" and argv[-1] == "status":
            self.status_versions.append(self.version)
            if self.version == self.status_fails_on:
                return CommandResult(1, b"", b"")
            return ok(b"narrivy-cli status: no active session\n")
        if argv[:2] == ("cat", upgrade.lifecycle.PID_FILE):
            return ok(b"4242\n")
        if argv[:2] == ("cat", upgrade.lifecycle.EXIT_FILE):
            return ok(b"0\n")
        if argv[0] == "sha256sum":
            target = argv[1]
            if target == upgrade.PREVIOUS_IN_CONTAINER:
                digest = PREVIOUS_SHA
            elif target == upgrade.CURRENT_IN_CONTAINER:
                digest = CURRENT_SHA
            else:
                area = next(k for k, v in upgrade.SENTINEL_PATHS.items() if v == target)
                digest = upgrade.SENTINEL_DIGESTS[area]
            return ok(f"{digest}  {target}\n".encode())
        if argv[:2] == ("cat", "/etc/os-release"):
            return ok(b'ID=ubuntu\nVERSION_ID="24.04"\n')
        if argv[0] == "apt-get" and "install" in argv:
            package = argv[-1]
            if package == upgrade.CURRENT_IN_CONTAINER and self.fail_upgrade:
                return CommandResult(100, b"", b"")
            self.version = "0.1.0~rc2" if package == upgrade.CURRENT_IN_CONTAINER else "0.1.0~rc1"
            self.registered = True
            return ok()
        if argv[0] == "apt-get" and "remove" in argv:
            return ok()
        if argv[0] == "apt-get" and "purge" in argv:
            self.registered = False
            return ok()
        if argv[0] == "dpkg-query" and "-f" in argv:
            return ok(self.version.encode())
        if argv[0] == "dpkg-query":
            return ok() if self.registered else CommandResult(1, b"", b"")
        if argv[:2] == ("id", "-u"):
            return ok(b"1000\n")
        if argv[0] == "stat":
            return ok(b"1000\n")
        if argv[:2] == ("test", "-L"):
            return CommandResult(1, b"", b"")
        return ok()


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

    def test_daemon_status_is_checked_after_install_upgrade_rollback_and_reupgrade(self) -> None:
        # P-4: the upgrade procedure must start the daemon and get a status answer.
        lifecycle = Lifecycle()
        outcome, _runner = self.run_flow(lifecycle)
        self.assertEqual(outcome["result"], "passed")
        self.assertEqual(outcome["daemon_checks"], {step: True for step in upgrade.DAEMON_CHECK_STEPS})
        self.assertEqual(lifecycle.status_versions, ["0.1.0~rc1", "0.1.0~rc2", "0.1.0~rc1", "0.1.0~rc2"])
        names = [step["step"] for step in outcome["steps"]]
        self.assertLess(names.index("confirm_upgrade"), names.index("daemon_status_upgrade"))

    def test_daemon_status_failure_fails_the_run(self) -> None:
        lifecycle = Lifecycle(status_fails_on="0.1.0~rc1")
        outcome, runner = self.run_flow(lifecycle)
        self.assertEqual(outcome["result"], "failed")
        self.assertEqual(outcome["failed_step"], "daemon_status_previous")
        self.assertFalse(outcome["daemon_checks"]["daemon_status_previous"])
        self.assertEqual(len(runner.removed), 1)

    def test_mid_upgrade_failure_is_truthful_and_container_is_removed(self) -> None:
        outcome, runner = self.run_flow(Lifecycle(fail_upgrade=True))
        self.assertEqual(outcome["result"], "failed")
        self.assertEqual(outcome["failed_step"], "apt_upgrade_current")
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
