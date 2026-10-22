from __future__ import annotations

import hashlib
import json
import sys
import tempfile
import unittest
from unittest.mock import patch
from pathlib import Path, PurePosixPath

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

import probe_installed_status as probe  # noqa: E402
import qualify_candidate as qualification  # noqa: E402
import qualify_installed_model as installed  # noqa: E402
from qualify_candidate import CommandResult, DockerError, InputError  # noqa: E402


CANDIDATE = b"narrivy candidate package bytes"
CANDIDATE_SHA = hashlib.sha256(CANDIDATE).hexdigest()
WORKER_SHA = "4accd33ce8576dcce14299ce4fd10d599b08ebabac24c4fbf9764b3bad81e9ba"
CONTAINER_ID = "a" * 64
REQUIRED_STEPS = (
    "image",
    "container_create",
    "container_start",
    "container_os",
    "apt_update",
    "apt_install_package",
    "apt_install_python3",
    "installed_paths",
    "installed_runtime_binding",
    "installed_model",
    "daemon_start",
    "offline_provision",
    "model_cache",
    "sigterm_shutdown",
    "restart_ready",
    "restart_stop",
    "remove",
    "remove_preserves_model",
    "purge",
    "purge_preserves_model",
    "container_remove",
)


def ok(stdout: bytes = b"") -> CommandResult:
    return CommandResult(0, stdout, b"")


class Lifecycle:
    """Scripted in-container behavior for one successful lifecycle."""

    def __init__(self) -> None:
        self.provisioned = False
        self.exited = False
        self.purged = False
        self.supervisor_starts = 0
        self.pipeline_enabled = True

    def digest(self, target: str) -> str:
        if target == qualification.CANDIDATE_IN_CONTAINER:
            return CANDIDATE_SHA
        if target in (qualification.CACHE_MODEL_PATH, qualification.INSTALLED_MODEL):
            return qualification.MODEL_SHA256
        if target.endswith("narrivy-transcribe-worker"):
            return WORKER_SHA
        return "f" * 64

    def __call__(self, argv: tuple[str, ...], detached: bool) -> CommandResult:
        if argv[0] == "sha256sum":
            target = argv[-1]
            return ok(f"{self.digest(target)}  {target}\n".encode())
        if argv[0] == "cat":
            if argv[1] == "/etc/os-release":
                return ok(b'ID=ubuntu\nVERSION_ID="24.04"\n')
            if argv[1] == qualification.PID_FILE:
                return ok(b"4242\n")
            if argv[1] == qualification.EXIT_FILE:
                return ok(b"0\n")
            return CommandResult(1, b"", b"")
        if argv[0] == "python3" and argv[1] == "-c":
            return ok(f"{WORKER_SHA}\n".encode())
        if argv[0] == "python3" and argv[1].endswith("probe_installed_status.py"):
            if self.provisioned:
                return ok(json.dumps({"installed_model": "ready", "worker_ready": True,
                    "pipeline": "clean", "pipeline_enabled": self.pipeline_enabled}).encode())
            return ok(b'{"installed_model":"awaiting_model","worker_ready":false,"pipeline":"clean","pipeline_enabled":false}\n')
        if argv[0] == "python3" and argv[1].endswith("qualify_installed_model.py"):
            self.provisioned = True
            return ok(json.dumps({"cleanup": "confirmed", "installed_model": "ready",
                "publication": "verified", "state": "ready", "worker_ready": True,
                "pipeline": "clean", "pipeline_enabled": self.pipeline_enabled}).encode())
        if argv[0] == "/bin/sh" and argv[1] == "-c" and "kill -TERM" in argv[2]:
            self.exited = True
            return ok()
        if argv[0] == "/bin/sh" and argv[1] == "-c" and "/usr/bin/narrivyd" in argv[2]:
            self.exited = False
            self.supervisor_starts += 1
            return ok()
        if argv[0] == "test":
            if argv[1] == "-S":
                return CommandResult(0 if not self.exited else 1, b"", b"")
            if argv[1] == "-f" and argv[2] == qualification.EXIT_FILE:
                return CommandResult(0 if self.exited else 1, b"", b"")
            return ok()
        if argv[0] == "dpkg-query":
            return CommandResult(0 if not self.purged else 1, b"", b"")
        if argv[0] == "apt-get" and "purge" in argv:
            self.purged = True
            return ok()
        if argv[0] == "stat":
            return ok(f"{qualification.MODEL_SIZE}\n".encode())
        return ok()


class AptInstallFails(Lifecycle):
    def __call__(self, argv: tuple[str, ...], detached: bool) -> CommandResult:
        if argv[0] == "apt-get" and argv[-1] == qualification.CANDIDATE_IN_CONTAINER:
            return CommandResult(100, b"", b"")
        return super().__call__(argv, detached)


class FakeRunner:
    def __init__(self, handler: Lifecycle) -> None:
        self.handler = handler
        self.calls: list[dict[str, object]] = []
        self.timeouts: list[tuple[tuple[str, ...], float]] = []
        self.copies: list[tuple[str, Path, str]] = []
        self.removals: list[tuple[str, str]] = []
        self.created: list[tuple[object, ...]] = []
        self.image_checks = 0
        self.starts = 0

    def ensure_image(self, image: str, *, inspect_timeout: float, pull_timeout: float) -> None:
        self.image_checks += 1

    def create(
        self,
        image: str,
        name: str,
        labels: object,
        *,
        lifetime_seconds: int,
        timeout: float,
    ) -> str:
        self.created.append((image, name, tuple(labels), lifetime_seconds))
        return CONTAINER_ID

    def copy(self, container: str, source: Path, destination: str, *, timeout: float) -> None:
        self.copies.append((container, Path(source), destination))

    def start(self, container: str, *, timeout: float) -> None:
        self.starts += 1

    def exec(
        self,
        container: str,
        argv: object,
        *,
        timeout: float,
        env: object = None,
        detached: bool = False,
    ) -> CommandResult:
        frozen = tuple(argv)
        self.calls.append({"argv": frozen, "detached": detached, "env": dict(env or {})})
        self.timeouts.append((frozen, timeout))
        return self.handler(frozen, detached)

    def remove(
        self, container: str, expected_candidate_sha256: str, *, timeout: float
    ) -> None:
        self.removals.append((container, expected_candidate_sha256))


class StrictParentRunner(FakeRunner):
    """Fake that rejects operations a container could not actually perform.

    ``docker cp`` into a path whose parent does not exist fails, and no
    container command can run before the container is started. This runner
    records the observed operation order and raises instead of silently
    accepting impossible moves.
    """

    def __init__(self, handler: Lifecycle) -> None:
        super().__init__(handler)
        self.started = False
        self.directories: set[str] = {"/"}
        self.events: list[str] = []

    def start(self, container: str, *, timeout: float) -> None:
        self.started = True
        self.events.append("start")
        super().start(container, timeout=timeout)

    def _materialize(self, path: str) -> None:
        for parent in reversed(PurePosixPath(path).parents):
            self.directories.add(str(parent))
        self.directories.add(path)

    def exec(
        self,
        container: str,
        argv: object,
        *,
        timeout: float,
        env: object = None,
        detached: bool = False,
    ) -> CommandResult:
        frozen = tuple(argv)
        if not self.started:
            raise AssertionError("container command before container start")
        if frozen[0] in ("sh", "/bin/sh") and "-c" in frozen:
            script = frozen[frozen.index("-c") + 1]
            for line in script.splitlines():
                if line.startswith("mkdir -p "):
                    for target in line[len("mkdir -p ") :].split():
                        self._materialize(target)
                        self.events.append(f"mkdir:{target}")
        if frozen[0] == "sha256sum" and frozen[-1] == qualification.CANDIDATE_IN_CONTAINER:
            self.events.append(f"verify:{frozen[-1]}")
        return super().exec(container, argv, timeout=timeout, env=env, detached=detached)

    def copy(self, container: str, source: Path, destination: str, *, timeout: float) -> None:
        if not self.started:
            raise AssertionError("container copy before container start")
        parent = str(PurePosixPath(destination).parent)
        if parent not in self.directories:
            raise AssertionError(f"copy destination parent {parent} does not exist")
        self.events.append(f"copy:{destination}")
        super().copy(container, source, destination, timeout=timeout)


class RecordingDockerRunner(qualification.DockerRunner):
    """DockerRunner with the docker process replaced by scripted results."""

    def __init__(self, responses: dict[tuple[str, ...], CommandResult] | None = None) -> None:
        super().__init__()
        self.runs: list[list[str]] = []
        self.responses = responses or {}

    def _run(self, arguments: object, timeout: float) -> CommandResult:
        frozen = list(arguments)
        self.runs.append(frozen)
        for prefix, outcome in self.responses.items():
            if frozen[: len(prefix)] == list(prefix):
                return outcome
        return CommandResult(0, b"", b"")


class CandidateQualificationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.candidate = self.root / "narrivy_0.1.0_amd64.deb"
        self.candidate.write_bytes(CANDIDATE)
        self.result_path = self.root / "lifecycle.json"

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def run_qualification(self, handler: Lifecycle, **kwargs: object):
        runner = FakeRunner(handler)
        outcome = qualification.qualify(
            self.candidate.resolve(),
            CANDIDATE_SHA,
            self.result_path,
            runner,
            **kwargs,
        )
        return outcome, runner

    def test_registry_layout_constants_match_the_seed_registry(self) -> None:
        registry = json.loads(
            (ROOT / "crates/narrivy-model-manager/seed/registry.json").read_text()
        )
        entry = next(
            item for item in registry["entries"] if item["id"] == qualification.MODEL_ID
        )
        self.assertEqual(entry["family"], qualification.MODEL_FAMILY)
        self.assertEqual(entry["sha256"], qualification.MODEL_SHA256)
        self.assertEqual(entry["size_bytes"], qualification.MODEL_SIZE)
        expected = (
            f"narrivy/models/{entry['family']}/{entry['id']}/"
            f"{entry['sha256'][:12]}/model.gguf"
        )
        self.assertEqual(qualification.MODEL_CACHE_RELATIVE, expected)
        self.assertEqual(
            Path(qualification.MODEL_CACHE_RELATIVE).parts,
            ("narrivy", "models", entry["family"], entry["id"], entry["sha256"][:12], "model.gguf"),
        )
        self.assertNotEqual(
            qualification.MODEL_CACHE_RELATIVE, f"narrivy/models/{entry['id']}.gguf"
        )

    def test_daemon_environment_mirrors_the_packaged_user_unit(self) -> None:
        unit = (ROOT / "packaging/deb/narrivy-daemon.service").read_text()
        replicas = 0
        for line in unit.splitlines():
            if not line.startswith("Environment="):
                continue
            key, _, value = line.removeprefix("Environment=").partition("=")
            if "%" in value:
                continue
            self.assertEqual(qualification.DAEMON_ENV.get(key), value, key)
            replicas += 1
        self.assertGreaterEqual(replicas, 8)
        self.assertEqual(
            qualification.DAEMON_ENV["NARRIVY_TRANSCRIBE_MODEL_ID"],
            qualification.MODEL_ID,
        )

    def test_mismatched_digest_is_rejected_before_any_execution(self) -> None:
        runner = FakeRunner(Lifecycle())
        with self.assertRaises(InputError):
            qualification.qualify(
                self.candidate.resolve(), "0" * 64, self.result_path, runner
            )
        self.assertEqual(runner.calls, [])
        self.assertEqual(runner.created, [])
        self.assertEqual(runner.copies, [])
        self.assertFalse(self.result_path.exists())

    def test_relative_inputs_and_existing_result_are_rejected(self) -> None:
        runner = FakeRunner(Lifecycle())
        with self.assertRaises(InputError):
            qualification.qualify(
                Path("relative.deb"), CANDIDATE_SHA, self.result_path, runner
            )
        self.result_path.write_text("historical")
        with self.assertRaises(InputError):
            qualification.qualify(
                self.candidate.resolve(), CANDIDATE_SHA, self.result_path, runner
            )
        self.assertEqual(self.result_path.read_text(), "historical")
        self.assertEqual(runner.calls, [])

    def test_main_rejects_rejected_inputs_without_touching_docker(self) -> None:
        code = qualification.main(
            [
                "--candidate",
                str(self.candidate.resolve()),
                "--expected-sha256",
                "1" * 64,
                "--result",
                str(self.result_path),
            ]
        )
        self.assertEqual(code, 2)
        self.assertFalse(self.result_path.exists())

    def test_successful_lifecycle_writes_unqualified_result(self) -> None:
        outcome, runner = self.run_qualification(Lifecycle())

        self.assertEqual(outcome["result"], "passed")
        self.assertIsNone(outcome["failed_step"])
        self.assertIsNone(outcome["failure_detail"])
        self.assertEqual(outcome["candidate"]["sha256"], CANDIDATE_SHA)
        self.assertEqual(outcome["candidate"]["size_bytes"], len(CANDIDATE))
        self.assertEqual(outcome["container"]["os"], "ubuntu-24.04")
        self.assertTrue(outcome["container"]["created"])
        self.assertTrue(outcome["container"]["removed"])
        self.assertTrue(outcome["unqualified"])
        self.assertFalse(outcome["requirement_evidence"])
        self.assertFalse(outcome["native_hardware_qualified"])

        names = [entry["step"] for entry in outcome["steps"]]
        for required in REQUIRED_STEPS:
            self.assertIn(required, names)
        self.assertEqual(names[-1], "container_remove")
        self.assertTrue(all(entry["status"] == "passed" for entry in outcome["steps"]))

        self.assertEqual(outcome["startup"]["installed_model"], "awaiting_model")
        self.assertEqual(outcome["provisioning"]["state"], "ready")
        self.assertIs(outcome["provisioning"]["worker_ready"], True)
        self.assertEqual(outcome["provisioning"]["pipeline"], "clean")
        self.assertIs(outcome["provisioning"]["pipeline_enabled"], True)
        self.assertEqual(
            outcome["model_cache"]["path"], qualification.MODEL_CACHE_RELATIVE
        )
        self.assertEqual(outcome["model_cache"]["sha256"], qualification.MODEL_SHA256)
        self.assertEqual(outcome["model_cache"]["size_bytes"], qualification.MODEL_SIZE)
        self.assertTrue(outcome["model_cache"]["preserved_after_remove"])
        self.assertTrue(outcome["model_cache"]["preserved_after_purge"])
        self.assertEqual(outcome["shutdown"]["exit_code"], 0)
        self.assertTrue(outcome["shutdown"]["socket_removed"])
        self.assertTrue(outcome["shutdown"]["bounded"])
        self.assertEqual(outcome["restart"]["installed_model"], "ready")
        self.assertIs(outcome["restart"]["worker_ready"], True)
        self.assertIs(outcome["startup"]["worker_ready"], False)
        self.assertEqual(outcome["restart_stop"]["exit_code"], 0)
        self.assertTrue(outcome["restart_stop"]["socket_removed"])
        self.assertTrue(outcome["restart_stop"]["bounded"])
        self.assertEqual(
            outcome["removal"],
            {
                "daemon_stopped_before_removal": True,
                "user_service_stop_qualified": False,
                "systemd_init": False,
            },
        )
        names = [entry["step"] for entry in outcome["steps"]]
        self.assertLess(names.index("restart_ready"), names.index("restart_stop"))
        self.assertLess(names.index("restart_stop"), names.index("remove"))

        self.assertEqual(json.loads(self.result_path.read_text()), outcome)
        self.assertEqual(runner.removals, [(CONTAINER_ID, CANDIDATE_SHA)])
        self.assertEqual(runner.created[0][0], "ubuntu:24.04")
        labels = dict(runner.created[0][2])
        self.assertEqual(labels["narrivy.qualification"], "1")
        self.assertEqual(labels["narrivy.candidate.sha256"], CANDIDATE_SHA)
        self.assertEqual(len(runner.created), 1)
        copied = [destination for _, _, destination in runner.copies]
        self.assertIn(qualification.CANDIDATE_IN_CONTAINER, copied)

    def test_user_inputs_never_enter_a_shell_string(self) -> None:
        outcome, runner = self.run_qualification(Lifecycle())
        self.assertEqual(outcome["result"], "passed")
        for call in runner.calls:
            argv = call["argv"]
            if "-c" not in argv:
                continue
            script = argv[argv.index("-c") + 1]
            self.assertNotIn(str(self.candidate), script)
            self.assertNotIn(CANDIDATE_SHA, script)
            self.assertNotIn(qualification.MODEL_SHA256, script)
        self.assertIn(
            qualification.CANDIDATE_IN_CONTAINER,
            [destination for _, _, destination in runner.copies],
        )

    def test_failed_step_is_named_and_failure_truth_is_preserved(self) -> None:
        outcome, runner = self.run_qualification(AptInstallFails())

        self.assertEqual(outcome["result"], "failed")
        self.assertEqual(outcome["failed_step"], "apt_install_package")
        self.assertEqual(outcome["failure_detail"], "exit_status=100")
        names = [entry["step"] for entry in outcome["steps"]]
        self.assertIn("apt_install_package", names)
        self.assertNotIn("offline_provision", names)
        failed = next(
            entry for entry in outcome["steps"] if entry["step"] == "apt_install_package"
        )
        self.assertEqual(failed["status"], "failed")
        self.assertTrue(outcome["container"]["removed"])
        self.assertEqual(runner.removals, [(CONTAINER_ID, CANDIDATE_SHA)])
        stored = json.loads(self.result_path.read_text())
        self.assertEqual(stored["result"], "failed")
        self.assertEqual(stored["failed_step"], "apt_install_package")

    def test_sigterm_timeout_identifies_the_shutdown_step(self) -> None:
        class NeverExits(Lifecycle):
            def __call__(self, argv: tuple[str, ...], detached: bool) -> CommandResult:
                if argv[0] == "/bin/sh" and argv[1] == "-c" and "kill -TERM" in argv[2]:
                    return ok()
                return super().__call__(argv, detached)

        outcome, _ = self.run_qualification(
            NeverExits(),
            limits=qualification.Limits(total=900.0, shutdown=0.05),
            sleep=lambda _seconds: None,
        )
        self.assertEqual(outcome["result"], "failed")
        self.assertEqual(outcome["failed_step"], "sigterm_shutdown")
        self.assertIn("bound", outcome["failure_detail"])

    def test_qualification_creates_directories_before_copying_inputs(self) -> None:
        runner = StrictParentRunner(Lifecycle())
        outcome = qualification.qualify(
            self.candidate.resolve(), CANDIDATE_SHA, self.result_path, runner
        )

        self.assertEqual(outcome["result"], "passed")
        events = runner.events
        self.assertEqual(events[0], "start")
        self.assertEqual(events[1], f"mkdir:{qualification.CONTAINER_ROOT}")
        verify = events.index(f"verify:{qualification.CANDIDATE_IN_CONTAINER}")
        self.assertLess(
            events.index(f"copy:{qualification.CANDIDATE_IN_CONTAINER}"), verify
        )
        self.assertLess(
            events.index("start"), events.index(f"mkdir:{qualification.CONTAINER_ROOT}")
        )
        self.assertLess(events.index(f"mkdir:{qualification.CONTAINER_ROOT}"), verify)
        for script in (qualification.QUALIFY_MODEL_SCRIPT, qualification.PROBE_STATUS_SCRIPT):
            self.assertIn(
                f"copy:{qualification.CONTAINER_ROOT}/{script.name}", events
            )

    def test_strict_fake_rejects_operations_a_container_could_not_perform(self) -> None:
        runner = StrictParentRunner(Lifecycle())
        with self.assertRaises(AssertionError):
            runner.exec(CONTAINER_ID, ["sha256sum", "/anything"], timeout=1)
        with self.assertRaises(AssertionError):
            runner.copy(
                CONTAINER_ID,
                self.candidate.resolve(),
                qualification.CANDIDATE_IN_CONTAINER,
                timeout=1,
            )
        runner.start(CONTAINER_ID, timeout=1)
        with self.assertRaises(AssertionError):
            runner.copy(
                CONTAINER_ID,
                self.candidate.resolve(),
                qualification.CANDIDATE_IN_CONTAINER,
                timeout=1,
            )

    def test_polling_respects_the_local_startup_deadline(self) -> None:
        class NeverReady(Lifecycle):
            def __call__(self, argv: tuple[str, ...], detached: bool) -> CommandResult:
                if argv[0] == "test" and argv[1] == "-S":
                    return CommandResult(1, b"", b"")
                if (
                    argv[0] == "test"
                    and argv[1] == "-f"
                    and argv[2] == qualification.EXIT_FILE
                ):
                    return CommandResult(1, b"", b"")
                return super().__call__(argv, detached)

        limits = qualification.Limits(total=900.0, startup=0.1, command=120.0)
        outcome, runner = self.run_qualification(
            NeverReady(), limits=limits, sleep=lambda _seconds: None
        )
        self.assertEqual(outcome["result"], "failed")
        self.assertEqual(outcome["failed_step"], "daemon_start")
        self.assertIn("bound", outcome["failure_detail"])
        probes = [
            timeout
            for argv, timeout in runner.timeouts
            if argv[0] == "test" and argv[1] == "-S"
        ]
        self.assertTrue(probes)
        self.assertLessEqual(max(probes), limits.startup)
        self.assertLess(max(probes), limits.command)

    def test_shutdown_polling_respects_the_local_shutdown_deadline(self) -> None:
        class NeverExits(Lifecycle):
            def __call__(self, argv: tuple[str, ...], detached: bool) -> CommandResult:
                if argv[0] == "/bin/sh" and argv[1] == "-c" and "kill -TERM" in argv[2]:
                    return ok()
                return super().__call__(argv, detached)

        limits = qualification.Limits(total=900.0, shutdown=0.05, command=120.0)
        outcome, runner = self.run_qualification(
            NeverExits(), limits=limits, sleep=lambda _seconds: None
        )
        self.assertEqual(outcome["failed_step"], "sigterm_shutdown")
        polls = [
            timeout
            for argv, timeout in runner.timeouts
            if argv[0] == "test" and argv[1] == "-f" and argv[2] == qualification.EXIT_FILE
        ]
        self.assertTrue(polls)
        self.assertLessEqual(max(polls), limits.shutdown)
        self.assertLess(max(polls), limits.command)

    def test_non_finite_timeouts_are_rejected_before_execution(self) -> None:
        for value in ("nan", "inf", "-inf", "0", "-1"):
            with self.subTest(timeout=value):
                result_path = self.root / f"lifecycle-{value}.json"
                code = qualification.main(
                    [
                        "--candidate",
                        str(self.candidate.resolve()),
                        "--expected-sha256",
                        CANDIDATE_SHA,
                        "--result",
                        str(result_path),
                        f"--timeout={value}",
                    ]
                )
                self.assertEqual(code, 2)
                self.assertFalse(result_path.exists())

    def test_non_finite_limit_budgets_are_rejected_before_container_work(self) -> None:
        for limits in (
            qualification.Limits(total=float("nan")),
            qualification.Limits(total=float("inf")),
            qualification.Limits(shutdown=float("inf")),
            qualification.Limits(command=0.0),
        ):
            with self.subTest(limits=limits):
                runner = FakeRunner(Lifecycle())
                result_path = self.root / "lifecycle-limits.json"
                with self.assertRaises(InputError):
                    qualification.qualify(
                        self.candidate.resolve(),
                        CANDIDATE_SHA,
                        result_path,
                        runner,
                        limits=limits,
                    )
                self.assertEqual(runner.calls, [])
                self.assertEqual(runner.copies, [])
                self.assertFalse(result_path.exists())

    def test_docker_runner_bounds_capture_and_rejects_invalid_timeouts(self) -> None:
        runner = qualification.DockerRunner(docker=sys.executable, max_capture_bytes=1024)
        outcome = runner._run(
            [
                "-c",
                "import sys; sys.stdout.write('x' * 100000); "
                "sys.stderr.write('y' * 100000)",
            ],
            timeout=30.0,
        )
        self.assertEqual(outcome.returncode, 0)
        self.assertEqual(outcome.stdout, b"x" * 1024)
        self.assertEqual(outcome.stderr, b"y" * 1024)
        for value in (float("nan"), float("inf"), 0.0, -1.0):
            with self.subTest(timeout=value):
                with self.assertRaises(DockerError):
                    runner._run(["-c", "pass"], timeout=value)

    def test_docker_runner_kills_a_command_that_exceeds_its_bound(self) -> None:
        runner = qualification.DockerRunner(docker=sys.executable)
        with self.assertRaises(DockerError):
            runner._run(["-c", "import time; time.sleep(30)"], timeout=0.2)

    def test_create_pins_the_test_owned_image_and_labels(self) -> None:
        runner = RecordingDockerRunner(
            {("create",): CommandResult(0, ("b" * 64).encode() + b"\n", b"")}
        )
        container = runner.create(
            "ubuntu:24.04",
            "narrivy-qualify-abc",
            (
                ("narrivy.qualification", "1"),
                ("narrivy.candidate.sha256", "c" * 64),
            ),
            lifetime_seconds=3000,
            timeout=5,
        )
        self.assertEqual(container, "b" * 64)
        argv = runner.runs[0]
        self.assertEqual(argv[:2], ["create", "--rm"])
        self.assertIn("--pull=never", argv)
        self.assertIn("ubuntu:24.04", argv)
        self.assertEqual(argv[-4:], ["timeout", "3000", "sleep", "infinity"])
        self.assertEqual(argv.count("--label"), 2)
        self.assertIn("narrivy.qualification=1", argv)
        self.assertIn(f"narrivy.candidate.sha256={'c' * 64}", argv)

    def test_removal_refuses_any_container_not_created_by_this_run(self) -> None:
        other = "d" * 64
        runner = RecordingDockerRunner(
            {
                ("inspect",): CommandResult(
                    0, f"{CONTAINER_ID}|1|{other}\n".encode(), b""
                )
            }
        )
        with self.assertRaises(DockerError):
            runner.remove(CONTAINER_ID, "c" * 64, timeout=5)
        self.assertEqual(len(runner.runs), 1)
        self.assertEqual(runner.runs[0][0], "inspect")

        runner = RecordingDockerRunner(
            {
                ("inspect",): CommandResult(
                    0, f"{CONTAINER_ID}|1|{'c' * 64}\n".encode(), b""
                )
            }
        )
        runner.remove(CONTAINER_ID, "c" * 64, timeout=5)
        self.assertEqual(runner.runs[-1], ["rm", "-f", CONTAINER_ID])

    def test_exec_builds_argv_without_a_shell(self) -> None:
        runner = RecordingDockerRunner()
        runner.exec(
            CONTAINER_ID,
            ["sha256sum", qualification.CANDIDATE_IN_CONTAINER],
            timeout=5,
            env={"DEBIAN_FRONTEND": "noninteractive"},
            detached=True,
        )
        self.assertEqual(
            runner.runs[0],
            [
                "exec",
                "-d",
                "-e",
                "DEBIAN_FRONTEND=noninteractive",
                CONTAINER_ID,
                "sha256sum",
                qualification.CANDIDATE_IN_CONTAINER,
            ],
        )

    def test_probe_summarizes_worker_readiness_without_raw_status(self) -> None:
        awaiting = probe.summarize({"installed_model": "awaiting_model", "capabilities": []})
        self.assertEqual(awaiting, {"installed_model": "awaiting_model", "worker_ready": False})
        ready = probe.summarize(
            {
                "installed_model": "ready",
                "capabilities": ["asr.transcribe-cpp.worker=ready", "unrelated"],
            }
        )
        self.assertEqual(ready, {
            "installed_model": "ready", "worker": "ready", "worker_ready": True,
        })

    def test_pipeline_probe_uses_wire_contract_and_discards_names(self) -> None:
        class Client:
            def request(self, kind, command):
                self.called = (kind, command)
                return {"result": "pipelines", "payload": [
                    {"id": "clean", "enabled": True, "name": "private name"},
                    {"id": "raw", "enabled": True, "name": "another private name"},
                ]}
        client = Client()
        self.assertEqual(installed.pipeline_facts(client), {
            "pipeline": "clean", "pipeline_enabled": True,
        })
        self.assertEqual(client.called, ("request.list_pipelines", {"command": "list_pipelines"}))

    def test_ready_worker_cannot_qualify_a_disabled_pipeline(self) -> None:
        lifecycle = Lifecycle()
        lifecycle.pipeline_enabled = False
        outcome, _ = self.run_qualification(lifecycle)
        self.assertEqual(outcome["result"], "failed")
        self.assertEqual(outcome["failed_step"], "offline_provision")
        self.assertTrue(outcome["container"]["removed"])

    def test_pipeline_probe_rejects_missing_ambiguous_or_untyped_enabled(self) -> None:
        class Client:
            def request(self, kind, command):
                return {"result": "pipelines", "payload": self.rows}
        client = Client()
        clean = {"id": "clean", "enabled": True}
        raw = {"id": "raw", "enabled": True}
        rows = [
            [],
            [clean],  # a single mode is not the advertised composition
            [clean, raw, {"id": "raw", "enabled": True}],  # duplicate mode
            [clean, {"id": "raw", "enabled": "true"}],  # untyped enabled
            [clean, {"id": "raw"}],  # missing enabled
        ]
        for row_set in rows:
            client.rows = row_set
            with self.assertRaises(installed.QualificationError):
                installed.pipeline_facts(client)
        # Unrelated advertised entries are ignored, not treated as modes.
        client.rows = [{"id": "unrelated", "enabled": True, "name": "x"}, clean, raw]
        self.assertEqual(installed.pipeline_facts(client), {
            "pipeline": "clean", "pipeline_enabled": True,
        })

    def test_model_qualification_rejects_unbounded_timeout_before_ipc(self) -> None:
        with patch.object(installed, "Client", side_effect=AssertionError("unexpected IPC")):
            for timeout in (0.0, -1.0, float("nan"), float("inf"), -float("inf")):
                with self.subTest(timeout=timeout):
                    with self.assertRaises(installed.QualificationError) as error:
                        installed.qualify(Path("/absent/socket"), Path("/absent/model"), timeout)
                    self.assertEqual(error.exception.reason, "invalid_timeout")

    def test_probe_keeps_lifecycle_evidence_and_excludes_user_content(self) -> None:
        summary = probe.summarize({
            "installed_model": "ready",
            "capabilities": [
                "asr.transcribe-cpp.worker=restart-required",
                "asr.transcribe-cpp.worker-artifact=ready:installed-runtime-manifest-bound",
                "unrelated=user transcript should stay private",
            ],
            "transcript": "user transcript should stay private",
            "clipboard": "private clipboard",
            "model_provisioning": {"detail": "private diagnostic content"},
        })
        self.assertEqual(summary, {
            "installed_model": "ready", "worker": "restart-required",
            "worker_ready": False,
            "worker_artifact": "ready:installed-runtime-manifest-bound",
        })
        self.assertNotIn("private", json.dumps(summary))

    def test_helper_diagnosis_retains_typed_cause_without_arbitrary_content(self) -> None:
        payload = {
            "schema_version": 1, "reason": "resident_worker_not_ready",
            "facts": {
                "installed_model": "ready", "worker": "restart-required",
                "worker_ready": False, "transcript": "private transcript",
                "provisioning_failure": "private error message",
                "model_artifact": float("nan"),
            },
        }
        result = qualification.CommandResult(
            1, b"", (qualification.DIAGNOSIS_PREFIX + json.dumps(payload)).encode(),
        )
        self.assertEqual(qualification._helper_diagnosis(result), {
            "helper_reason": "resident_worker_not_ready", "helper_facts": {
                "installed_model": "ready", "worker": "restart-required",
                "worker_ready": False,
            },
        })
        payload["reason"] = "private transcript"
        invalid = qualification.CommandResult(
            1, b"", (qualification.DIAGNOSIS_PREFIX + json.dumps(payload)).encode(),
        )
        self.assertEqual(qualification._helper_diagnosis(invalid), {})

    def test_binary_integrity_never_proves_live_worker_readiness(self) -> None:
        artifact = "asr.transcribe-cpp.worker-artifact=ready:installed-runtime-manifest-bound"
        for model in ("awaiting_model", "ready"):
            with self.subTest(model=model):
                self.assertFalse(probe.summarize({
                    "installed_model": model, "capabilities": [artifact],
                })["worker_ready"])
        for lifecycle in ("warming", "active", "restart-required", "failed-closed"):
            with self.subTest(lifecycle=lifecycle):
                self.assertFalse(probe.summarize({
                    "installed_model": "ready",
                    "capabilities": [artifact, f"asr.transcribe-cpp.worker={lifecycle}"],
                })["worker_ready"])
        self.assertFalse(probe.summarize({
            "installed_model": "awaiting_model",
            "capabilities": ["asr.transcribe-cpp.worker=ready"],
        })["worker_ready"])

    def test_malformed_capabilities_fail_closed_without_a_probe_crash(self) -> None:
        for capabilities in (None, "asr.transcribe-cpp.worker=ready", [{"untrusted": True}]):
            with self.subTest(capabilities=capabilities):
                self.assertFalse(probe.summarize({
                    "installed_model": "ready", "capabilities": capabilities,
                })["worker_ready"])


if __name__ == "__main__":
    unittest.main()
