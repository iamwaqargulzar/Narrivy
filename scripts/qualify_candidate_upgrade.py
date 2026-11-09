#!/usr/bin/env python3
"""Qualify previous-to-current Narrivy .deb upgrade and rollback in a container.

The entrypoint accepts explicit absolute previous and current ``.deb`` paths
and a new result JSON path. It rejects unsafe inputs, identical or
non-increasing Debian versions, and malformed packages before any container
work. One fresh test-owned Ubuntu 24.04 Docker container then: installs the
previous package; writes content-safe sentinels in a non-root user's Narrivy
XDG config/data/state/cache directories; upgrades; confirms version, program
files, and byte-identical sentinels; downgrades as rollback and reconfirms;
re-upgrades and reconfirms; then ``apt remove`` and ``apt purge`` drop package
registration and program files while preserving user-owned data. Maintainer
scripts must not traverse user homes.

The JSON result is sanitized, atomic, non-overwriting, and unqualified. It
records step timings, both package SHA-256 values and versions, sentinel
hashes only, and owned-container removal. Transcript, config contents, tokens,
and environment dumps are never written. Only the container created by this
run is removed.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import re
import subprocess
import sys
import time
import uuid
from dataclasses import replace
from pathlib import Path
from typing import Any, Callable, Mapping, Sequence

import qualify_candidate as lifecycle
import qualify_candidate_known_phrase as archive

InputError = lifecycle.InputError
DockerError = lifecycle.DockerError
StepFailure = lifecycle.StepFailure
CommandResult = lifecycle.CommandResult
Limits = lifecycle.Limits
DockerRunner = lifecycle.DockerRunner
DEFAULT_LIMITS = lifecycle.DEFAULT_LIMITS
DEFAULT_IMAGE = lifecycle.DEFAULT_IMAGE
PACKAGE_NAME = lifecycle.PACKAGE_NAME
CONTAINER_ROOT = lifecycle.CONTAINER_ROOT
APT_ENV = lifecycle.APT_ENV
INSTALLED_PATHS_SCRIPT = lifecycle.INSTALLED_PATHS_SCRIPT
REMOVED_PATHS_SCRIPT = lifecycle.REMOVED_PATHS_SCRIPT
IMAGE_RE = lifecycle.IMAGE_RE
SHA256_RE = lifecycle.SHA256_RE
MAX_CANDIDATE_BYTES = lifecycle.MAX_CANDIDATE_BYTES
MAX_CONTROL_BYTES = 64 * 1024

PREVIOUS_IN_CONTAINER = f"{CONTAINER_ROOT}/previous.deb"
CURRENT_IN_CONTAINER = f"{CONTAINER_ROOT}/current.deb"
TEST_USER = "narrivy-test"
TEST_HOME = f"/home/{TEST_USER}"
VERSION_RE = re.compile(r"[0-9][0-9A-Za-z.+:~_-]{0,63}")

SENTINEL_PAYLOADS = {
    "config": "narrivy-sentinel-config-v1",
    "data": "narrivy-sentinel-data-v1",
    "state": "narrivy-sentinel-state-v1",
    "cache": "narrivy-sentinel-cache-v1",
}
SENTINEL_RELATIVE = {
    "config": ".config/narrivy/upgrade-sentinel",
    "data": ".local/share/narrivy/upgrade-sentinel",
    "state": ".local/state/narrivy/upgrade-sentinel",
    "cache": ".cache/narrivy/upgrade-sentinel",
}
SENTINEL_PATHS = {
    area: f"{TEST_HOME}/{relative}" for area, relative in SENTINEL_RELATIVE.items()
}
SENTINEL_DIRS = (
    f"{TEST_HOME}/.config/narrivy",
    f"{TEST_HOME}/.local/share/narrivy",
    f"{TEST_HOME}/.local/state/narrivy",
    f"{TEST_HOME}/.cache/narrivy",
)
SENTINEL_DIGESTS = {
    area: hashlib.sha256(f"{payload}\n".encode("ascii")).hexdigest()
    for area, payload in SENTINEL_PAYLOADS.items()
}
SENTINEL_WRITE = 'printf "%s\\n" "$1" > "$2"'
# Creates CONTAINER_ROOT plus the daemon's runtime/state/cache/data/home dirs.
DIRECTORY_SCRIPT = lifecycle.DIRECTORY_SCRIPT
# The daemon must start and answer `narrivy-cli status` after every install,
# not only have its files present.
DAEMON_CHECK_STEPS = (
    "daemon_status_previous",
    "daemon_status_upgrade",
    "daemon_status_rollback",
    "daemon_status_reupgrade",
)
DAEMON_POLL_SECONDS = 0.25


def _sanitize_detail(value: object, limit: int = 200) -> str:
    return lifecycle._sanitize_detail(value, limit)


def _require_absolute(path: Path, label: str) -> Path:
    if not path.is_absolute() or path.name in ("", ".", ".."):
        raise InputError(f"{label} path must be absolute")
    return path


def _require_package_path(path: Path, label: str) -> Path:
    _require_absolute(path, label)
    if path.suffix != ".deb":
        raise InputError(f"{label} path must end in .deb")
    return path


def _require_new_result(path: Path) -> Path:
    _require_absolute(path, "result")
    if os.path.lexists(path):
        raise InputError("result path already exists")
    if not path.parent.is_dir():
        raise InputError("result parent directory does not exist")
    return path


def _validate_debian_version(version: str) -> None:
    if not VERSION_RE.fullmatch(version):
        raise InputError("Debian version is invalid")


def _version_is_greater(current: str, previous: str) -> bool:
    """Use dpkg's canonical Debian version ordering; never reimplement it."""

    try:
        compared = subprocess.run(
            ["dpkg", "--compare-versions", current, "gt", previous],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=5,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise InputError("dpkg version comparison is unavailable") from error
    if compared.returncode not in (0, 1):
        raise InputError("dpkg rejected a package version")
    return compared.returncode == 0


def _parse_control(payload: bytes) -> tuple[str, str]:
    try:
        text = payload.decode("utf-8")
    except UnicodeDecodeError as error:
        raise InputError("Debian control is invalid") from error
    if "\0" in text or len(text) > MAX_CONTROL_BYTES:
        raise InputError("Debian control is invalid")
    package = ""
    version = ""
    for line in text.splitlines():
        if line.startswith("Package:"):
            if package:
                raise InputError("Debian control is invalid")
            package = line[len("Package:") :].strip()
        elif line.startswith("Version:"):
            if version:
                raise InputError("Debian control is invalid")
            version = line[len("Version:") :].strip()
    if package != PACKAGE_NAME or not version:
        raise InputError("Debian control identity is missing")
    _validate_debian_version(version)
    return package, version


def _package_identity(path: Path, label: str) -> tuple[str, int, str]:
    _require_package_path(path, label)
    try:
        payload = archive._read_regular_file(path, MAX_CANDIDATE_BYTES, label)
        members = archive._parse_debian_archive(payload)
        archive._archive_payload(members, "data.tar")
        control_name, control_payload = archive._archive_payload(members, "control.tar")
        files = archive._required_tar_regular_files(
            control_payload, control_name, {"control": MAX_CONTROL_BYTES}
        )
    except archive.InputError as error:
        raise InputError(_sanitize_detail(str(error))) from error
    digest = hashlib.sha256(payload).hexdigest()
    size = len(payload)
    _package, version = _parse_control(files["control"])
    return digest, size, version


def _same_inode(left: Path, right: Path) -> bool:
    try:
        first = left.lstat()
        second = right.lstat()
    except OSError as error:
        raise InputError("package is unavailable") from error
    return first.st_dev == second.st_dev and first.st_ino == second.st_ino


def validate_inputs(
    previous: Path, current: Path, result_path: Path
) -> tuple[tuple[str, int, str], tuple[str, int, str]]:
    """Reject mismatched or unsafe inputs before any container work."""

    previous = _require_package_path(previous, "previous")
    current = _require_package_path(current, "current")
    _require_new_result(result_path)
    if previous == current or _same_inode(previous, current):
        raise InputError("previous and current packages must be distinct")
    previous_identity = _package_identity(previous, "previous")
    current_identity = _package_identity(current, "current")
    if not _version_is_greater(current_identity[2], previous_identity[2]):
        raise InputError("current Debian version must be greater than previous")
    return previous_identity, current_identity


def _run_digest(previous_sha256: str, current_sha256: str) -> str:
    return hashlib.sha256(
        f"{previous_sha256}:{current_sha256}".encode("ascii")
    ).hexdigest()


def _package_record(path: Path, digest: str, size: int, version: str) -> dict[str, object]:
    return {
        "file": _sanitize_detail(path.name),
        "sha256": digest,
        "size_bytes": size,
        # The control parser already restricts this to Debian's bounded safe
        # version alphabet. Preserve '~' because it is ordering-significant.
        "version": version,
    }


def _base_result(
    previous_path: Path,
    previous_identity: tuple[str, int, str],
    current_path: Path,
    current_identity: tuple[str, int, str],
    image: str,
) -> dict[str, Any]:
    return {
        "schema_version": 1,
        "classification": "candidate-upgrade-rollback-unqualified",
        "result": "failed",
        "failed_step": None,
        "failure_detail": None,
        "previous": _package_record(previous_path, *previous_identity),
        "current": _package_record(current_path, *current_identity),
        "container": {
            "image": _sanitize_detail(image),
            "created": False,
            "os": None,
            "removed": False,
        },
        "test_user": {"name": TEST_USER, "uid": None},
        "sentinels": {
            "hashes": dict(SENTINEL_DIGESTS),
            "preserved_after_upgrade": False,
            "preserved_after_rollback": False,
            "preserved_after_reupgrade": False,
            "preserved_after_remove": False,
            "preserved_after_purge": False,
        },
        "daemon_checks": {step: False for step in DAEMON_CHECK_STEPS},
        "steps": [],
        "unqualified": True,
        "requirement_evidence": False,
        "native_hardware_qualified": False,
    }


def qualify(
    previous: Path,
    current: Path,
    result_path: Path,
    runner: lifecycle.Runner,
    *,
    image: str = DEFAULT_IMAGE,
    limits: Limits = DEFAULT_LIMITS,
    clock: Callable[[], float] = time.monotonic,
    sleep: Callable[[float], None] = time.sleep,
) -> dict[str, Any]:
    """Run the bounded upgrade/rollback qualification and return its result."""

    previous_identity, current_identity = validate_inputs(previous, current, result_path)
    lifecycle._validate_limits(limits)
    previous_digest, _previous_size, previous_version = previous_identity
    current_digest, _current_size, current_version = current_identity
    run_digest = _run_digest(previous_digest, current_digest)
    deadline = clock() + limits.total
    result = _base_result(previous, previous_identity, current, current_identity, image)
    steps: list[dict[str, object]] = []
    state: dict[str, Any] = {"current": "validate", "began": clock()}
    container: str | None = None
    test_uid: str | None = None

    def remaining(
        step: str, budget: float, local_deadline: float | None = None
    ) -> float:
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
        if state["current"] != step:
            state["began"] = clock()
        state["current"] = step

    def elapsed() -> float:
        return round(max(0.0, clock() - float(state["began"])), 3)

    def run_exec(
        step: str,
        argv: Sequence[str],
        budget: float,
        env: Mapping[str, str] | None = None,
        detached: bool = False,
    ) -> CommandResult:
        begin(step)
        if container is None:
            raise StepFailure(step, "container is unavailable")
        try:
            return runner.exec(
                container,
                argv,
                timeout=remaining(step, budget),
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
    ) -> CommandResult:
        outcome = run_exec(step, argv, budget, env=env, detached=detached)
        if outcome.returncode != 0:
            raise StepFailure(step, f"exit_status={outcome.returncode}")
        return outcome

    def passed(step: str) -> None:
        steps.append({"step": step, "status": "passed", "seconds": elapsed()})

    def fail_record(step: str) -> None:
        if not any(entry["step"] == step for entry in steps):
            steps.append({"step": step, "status": "failed", "seconds": elapsed()})

    def installed_version(step: str) -> str:
        outcome = run_ok(
            step,
            ["dpkg-query", "-W", "-f", "${Version}", PACKAGE_NAME],
            limits.command,
        )
        try:
            version = outcome.stdout.decode("ascii", "strict").strip()
        except UnicodeDecodeError as error:
            raise StepFailure(step, "installed version was unavailable") from error
        if not VERSION_RE.fullmatch(version):
            raise StepFailure(step, "installed version was unavailable")
        return version

    def confirm_program_files(step: str, *, present: bool) -> None:
        script = INSTALLED_PATHS_SCRIPT if present else REMOVED_PATHS_SCRIPT
        outcome = run_exec(step, ["sh", "-c", script], limits.command)
        if outcome.returncode != 0:
            raise StepFailure(step, lifecycle._exit_detail(outcome))

    def confirm_sentinels(step: str) -> None:
        if test_uid is None:
            raise StepFailure(step, "test user uid was unavailable")
        for area, path in SENTINEL_PATHS.items():
            linked = run_exec(step, ["test", "-L", path], limits.command)
            if linked.returncode == 0:
                raise StepFailure(step, f"sentinel_symlink={area}")
            present = run_exec(step, ["test", "-f", path], limits.command)
            if present.returncode != 0:
                raise StepFailure(step, f"sentinel_missing={area}")
            digest = lifecycle._digest_of(
                run_ok(step, ["sha256sum", path], limits.command).stdout,
                step,
            )
            if digest != SENTINEL_DIGESTS[area]:
                raise StepFailure(step, f"sentinel_mismatch={area}")
            uid = run_ok(step, ["stat", "-c", "%u", path], limits.command)
            try:
                owner = uid.stdout.decode("ascii", "strict").strip()
            except UnicodeDecodeError as error:
                raise StepFailure(step, f"sentinel_owner={area}") from error
            if owner != test_uid:
                raise StepFailure(step, f"sentinel_owner={area}")

    def confirm_installed(step: str, version: str, preserved_key: str) -> None:
        if installed_version(step) != version:
            raise StepFailure(step, "installed version mismatch")
        confirm_program_files(step, present=True)
        confirm_sentinels(step)
        result["sentinels"][preserved_key] = True
        passed(step)

    def confirm_removed(step: str, *, must_be_unregistered: bool, preserved_key: str) -> None:
        if must_be_unregistered:
            registered = run_exec(
                step, ["dpkg-query", "-W", PACKAGE_NAME], limits.command
            )
            if registered.returncode == 0:
                raise StepFailure(step, "package is still registered")
        confirm_program_files(step, present=False)
        confirm_sentinels(step)
        result["sentinels"][preserved_key] = True
        passed(step)

    def daemon_status_check(step: str) -> None:
        """Start the installed daemon, require `narrivy-cli status`, stop it cleanly."""
        run_ok(
            step,
            ["/bin/sh", "-c", lifecycle.DAEMON_START_SCRIPT],
            limits.command,
            lifecycle.DAEMON_ENV,
            detached=True,
        )
        local_deadline = clock() + remaining(step, limits.startup)
        while True:
            if clock() >= local_deadline:
                raise StepFailure(step, "daemon socket did not appear within the bound")
            if run_exec(step, ["test", "-S", lifecycle.SOCKET_PATH], limits.command).returncode == 0:
                break
            if run_exec(step, ["test", "-f", lifecycle.EXIT_FILE], limits.command).returncode == 0:
                raise StepFailure(step, "daemon exited before serving")
            sleep(min(DAEMON_POLL_SECONDS, max(0.0, local_deadline - clock())))
        status = run_exec(
            step,
            ["/usr/bin/narrivy-cli", "--socket", lifecycle.SOCKET_PATH, "status"],
            limits.command,
            lifecycle.DAEMON_ENV,
        )
        if status.returncode != 0:
            raise StepFailure(step, f"narrivy_cli_status_exit={status.returncode}")
        pid = run_ok(step, ["cat", lifecycle.PID_FILE], limits.command).stdout.decode(
            "ascii", "replace"
        ).strip()
        if not lifecycle.PID_RE.fullmatch(pid):
            raise StepFailure(step, "daemon pid was unavailable")
        run_ok(step, ["/bin/sh", "-c", 'kill -TERM "$1"', "kill", pid], limits.command)
        stop_deadline = clock() + remaining(step, limits.shutdown)
        while run_exec(step, ["test", "-f", lifecycle.EXIT_FILE], limits.command).returncode != 0:
            if clock() >= stop_deadline:
                raise StepFailure(step, "daemon did not exit within the bound")
            sleep(min(DAEMON_POLL_SECONDS, max(0.0, stop_deadline - clock())))
        code = run_ok(step, ["cat", lifecycle.EXIT_FILE], limits.command).stdout.decode(
            "ascii", "replace"
        ).strip()
        if code != "0":
            raise StepFailure(step, f"daemon_exit_status={code}")
        result["daemon_checks"][step] = True
        passed(step)

    def install_package(step: str, package_path: str, *extra: str) -> None:
        run_ok(
            step,
            ["apt-get", "install", "-y", "-qq", *extra, package_path],
            limits.install,
            APT_ENV,
        )
        passed(step)

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
                f"narrivy-qualify-upgrade-{uuid.uuid4().hex[:12]}",
                (
                    ("narrivy.qualification", "1"),
                    ("narrivy.candidate.sha256", run_digest),
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
                previous,
                PREVIOUS_IN_CONTAINER,
                timeout=remaining("container_start", limits.copy),
            )
            runner.copy(
                container,
                current,
                CURRENT_IN_CONTAINER,
                timeout=remaining("container_start", limits.copy),
            )
        except DockerError as error:
            raise StepFailure("container_start", _sanitize_detail(error)) from None
        transferred_previous = lifecycle._digest_of(
            run_ok(
                "container_start",
                ["sha256sum", PREVIOUS_IN_CONTAINER],
                limits.command,
            ).stdout,
            "container_start",
        )
        transferred_current = lifecycle._digest_of(
            run_ok(
                "container_start",
                ["sha256sum", CURRENT_IN_CONTAINER],
                limits.command,
            ).stdout,
            "container_start",
        )
        if transferred_previous != previous_digest or transferred_current != current_digest:
            raise StepFailure("container_start", "transferred package digest mismatch")
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
            "create_test_user",
            [
                "useradd",
                "--create-home",
                "--home-dir",
                TEST_HOME,
                "--user-group",
                "--shell",
                "/bin/sh",
                TEST_USER,
            ],
            limits.command,
        )
        uid = run_ok("create_test_user", ["id", "-u", TEST_USER], limits.command)
        try:
            observed_uid = uid.stdout.decode("ascii", "strict").strip()
        except UnicodeDecodeError as error:
            raise StepFailure("create_test_user", "test user uid mismatch") from error
        if not observed_uid.isdigit() or observed_uid == "0":
            raise StepFailure("create_test_user", "test user uid mismatch")
        test_uid = observed_uid
        result["test_user"]["uid"] = int(test_uid)
        passed("create_test_user")

        install_package("apt_install_previous", PREVIOUS_IN_CONTAINER)
        daemon_status_check("daemon_status_previous")

        run_ok(
            "create_sentinels",
            ["runuser", "-u", TEST_USER, "--", "mkdir", "-p", *SENTINEL_DIRS],
            limits.command,
        )
        for area, path in SENTINEL_PATHS.items():
            run_ok(
                "create_sentinels",
                [
                    "runuser",
                    "-u",
                    TEST_USER,
                    "--",
                    "/bin/sh",
                    "-c",
                    SENTINEL_WRITE,
                    "sentinel",
                    SENTINEL_PAYLOADS[area],
                    path,
                ],
                limits.command,
            )
        confirm_sentinels("create_sentinels")
        passed("create_sentinels")

        install_package("apt_upgrade_current", CURRENT_IN_CONTAINER)
        confirm_installed("confirm_upgrade", current_version, "preserved_after_upgrade")
        daemon_status_check("daemon_status_upgrade")

        install_package(
            "apt_rollback_previous", PREVIOUS_IN_CONTAINER, "--allow-downgrades"
        )
        confirm_installed(
            "confirm_rollback", previous_version, "preserved_after_rollback"
        )
        daemon_status_check("daemon_status_rollback")

        install_package("apt_reupgrade_current", CURRENT_IN_CONTAINER)
        confirm_installed(
            "confirm_reupgrade", current_version, "preserved_after_reupgrade"
        )
        daemon_status_check("daemon_status_reupgrade")

        run_ok(
            "apt_remove",
            ["apt-get", "remove", "-y", "-qq", PACKAGE_NAME],
            limits.install,
            APT_ENV,
        )
        passed("apt_remove")
        confirm_removed(
            "confirm_remove",
            must_be_unregistered=False,
            preserved_key="preserved_after_remove",
        )

        run_ok(
            "apt_purge",
            ["apt-get", "purge", "-y", "-qq", PACKAGE_NAME],
            limits.install,
            APT_ENV,
        )
        passed("apt_purge")
        confirm_removed(
            "confirm_purge",
            must_be_unregistered=True,
            preserved_key="preserved_after_purge",
        )

        result["result"] = "passed"
    except StepFailure as failure:
        result["result"] = "failed"
        result["failed_step"] = failure.step
        result["failure_detail"] = failure.detail
        fail_record(failure.step)
    except (DockerError, OSError, UnicodeDecodeError, ValueError, json.JSONDecodeError) as error:
        step = str(state["current"])
        result["result"] = "failed"
        result["failed_step"] = step
        result["failure_detail"] = _sanitize_detail(error)
        fail_record(step)
    finally:
        if container is not None:
            begin("container_remove")
            try:
                runner.remove(container, run_digest, timeout=limits.remove)
            except (DockerError, OSError) as error:
                result["container"]["removed"] = False
                if result["result"] == "passed":
                    result["result"] = "failed"
                    result["failed_step"] = "container_remove"
                    result["failure_detail"] = _sanitize_detail(error)
                    steps.append(
                        {
                            "step": "container_remove",
                            "status": "failed",
                            "seconds": elapsed(),
                        }
                    )
            else:
                result["container"]["removed"] = True
                if result["result"] == "passed":
                    steps.append(
                        {
                            "step": "container_remove",
                            "status": "passed",
                            "seconds": elapsed(),
                        }
                    )
        result["steps"] = steps
        lifecycle._write_result(result_path, result)
    return result


def _arguments(argv: list[str] | None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--previous", required=True, type=Path)
    parser.add_argument("--current", required=True, type=Path)
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
            values.previous,
            values.current,
            values.result,
            DockerRunner(docker=values.docker),
            image=values.image,
            limits=replace(DEFAULT_LIMITS, total=values.timeout),
        )
    except InputError as error:
        print(f"qualify_candidate_upgrade: {error}", file=sys.stderr)
        return 2
    except (DockerError, OSError, ValueError) as error:
        print(f"qualify_candidate_upgrade: {_sanitize_detail(error)}", file=sys.stderr)
        return 1
    if result["result"] == "passed":
        print(f"qualify_candidate_upgrade: passed -> {values.result}")
        return 0
    print(
        f"qualify_candidate_upgrade: failed step={result['failed_step']} -> {values.result}",
        file=sys.stderr,
    )
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
