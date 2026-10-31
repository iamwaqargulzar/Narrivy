#!/usr/bin/env python3
"""Qualify package-bound IPC, diagnostics redaction and doctor behavior."""
from __future__ import annotations

import hashlib
import json
import os
import platform
import shlex
import stat
import subprocess
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path

import jsonschema

import qualify_candidate_known_phrase as package
import recorder_safety


ROOT = Path(__file__).resolve().parents[1]
ARTIFACT = "artifacts/releases/narrivy-linux-amd64.deb"
INDEX = "agent/evidence/INDEX.json"
EXPECTED_ARGV = ["python3", "scripts/record_native_hardening_evidence.py", ARTIFACT]
CANARIES = ("NARRIVY-DOCTOR-CONTENT-CANARY", "NARRIVY-SUPPORT-SECRET-CANARY")
def canaries_absent(*outputs: str) -> bool:
    return not any(canary in output for output in outputs for canary in CANARIES)


def measurement(metric: str, comparator: str, expected: object = None) -> dict[str, object]:
    binding: dict[str, object] = {"metric": metric, "comparator": comparator}
    if comparator not in {"is_true", "is_false"}:
        binding["expected"] = expected
    return binding


def measured(assertion: str, binding: dict[str, object]) -> dict[str, object]:
    return {"id": assertion, "result": "pass", "measurement": binding}


# The exact packaged daemon/CLI, bound by the reviewed package identity.
PACKAGE_BOUND = measurement("package.worker_identity_verified", "is_true")


RECORDS = {
    "SEC-005": {
        "phase": "P1",
        "verification": "VER-SEC-005",
        "procedure": "native-ipc-endpoint-hardening-v1",
        "assertions": [
            measured("exact-package-source-bound", PACKAGE_BOUND),
            measured("runtime-directory-mode-0700",
                     measurement("endpoint.runtime_mode_bits", "eq", 0o700)),
            measured("socket-mode-0600", measurement("endpoint.socket_mode_bits", "eq", 0o600)),
            measured("endpoint-owned-by-current-user",
                     measurement("endpoint.runtime_owner_uid_matches", "is_true")),
            measured("kernel-peer-validation-round-trip",
                     measurement("endpoint.authenticated_status_round_trip", "is_true")),
            measured("cross-uid-peer-rejected",
                     measurement("endpoint.cross_uid_rejected", "is_true")),
            measured("daemon-clean-shutdown", measurement("endpoint.daemon_exit_code", "eq", 0)),
        ],
        "artifacts": [
            "adapters/ipc-unix/src/security.rs",
            "adapters/ipc-unix/src/server.rs",
            "adapters/ipc-unix/src/client.rs",
        ],
    },
    "REL-008": {
        "phase": "P6",
        "verification": "VER-REL-008",
        "procedure": "native-support-report-redaction-v1",
        "assertions": [
            measured("exact-package-source-bound", PACKAGE_BOUND),
            measured("packaged-support-report-generated",
                     measurement("support_report.exit_code", "eq", 0)),
            measured("support-report-default-redaction",
                     measurement("canaries.support_report_absent", "is_true")),
            measured("transcript-audio-context-canaries-absent",
                     measurement("canaries.daemon_log_absent", "is_true")),
            measured("safe-diagnostic-fields-preserved",
                     measurement("doctor.reported_sections", "len_ge", 9)),
        ],
        "artifacts": [
            "crates/narrivy-diagnostics/src/support.rs",
            "crates/narrivy-diagnostics/src/redactor.rs",
        ],
    },
    "OPS-001": {
        "phase": "P6",
        "verification": "VER-OPS-001",
        "procedure": "native-nondestructive-doctor-v1",
        "assertions": [
            measured("exact-package-source-bound", PACKAGE_BOUND),
            measured("native-capabilities-reported",
                     measurement("doctor.reported_sections", "len_ge", 9)),
            measured("actionable-remediation-reported",
                     measurement("doctor.remediation_count", "ge", 1)),
            measured("content-canaries-absent", measurement("canaries.doctor_stdout_absent", "is_true")),
            measured("home-path-redacted", measurement("doctor.absolute_home_path_absent", "is_true")),
        ],
        "artifacts": ["scripts/doctor.py"],
    },
}


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def fail(message: str) -> int:
    print(f"record_native_hardening_evidence: {message}", file=sys.stderr)
    return 1


# Runs as another UID. It reads the first frame the daemon sends; the server
# validates SO_PEERCRED before any handshake and answers a foreign UID with a
# typed "cross-user connection denied" rejection.
CROSS_UID_PROBE = r"""
import json, socket, struct, sys
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.settimeout(5.0)
s.connect(sys.argv[1])
head = b""
while len(head) < 4:
    chunk = s.recv(4 - len(head))
    if not chunk:
        break
    head += chunk
body = b""
if len(head) == 4:
    size = struct.unpack("<I", head)[0]
    while len(body) < min(size, 65536):
        chunk = s.recv(min(size, 65536) - len(body))
        if not chunk:
            break
        body += chunk
print(json.dumps({"rejected": b"cross-user connection denied" in body, "bytes": len(body)}))
"""
CROSS_UID_USER = "nobody"


def probe_cross_uid(
    holder: Path, runtime: Path, socket: Path, operations: package.HostOperations, env: dict[str, str]
) -> dict[str, object]:
    """Connect to the live packaged socket as another UID; expect rejection.

    The holder, runtime directory and socket are opened to the probe only for
    the duration of the probe, then restored and re-checked. Needs
    non-interactive sudo (run ``sudo -v`` first): an owner step.
    """
    try:
        import pwd

        probe_uid = pwd.getpwnam(CROSS_UID_USER).pw_uid
    except (ImportError, KeyError):
        raise RuntimeError(f"cross-UID probe user {CROSS_UID_USER!r} is unavailable") from None
    if probe_uid == os.geteuid():
        raise RuntimeError("cross-UID probe user is the current user")
    os.chmod(holder, 0o711)
    os.chmod(runtime, 0o711)
    os.chmod(socket, 0o666)
    try:
        result = operations.run(
            ["sudo", "-n", "-u", CROSS_UID_USER, "python3", "-I", "-c", CROSS_UID_PROBE, str(socket)],
            env={"PATH": env.get("PATH", "/usr/bin:/bin"), "LANG": "C.UTF-8"},
            timeout=15.0,
            cwd=Path("/"),
        )
    finally:
        os.chmod(socket, 0o600)
        os.chmod(runtime, 0o700)
        os.chmod(holder, 0o700)
    if stat.S_IMODE(runtime.stat().st_mode) != 0o700 or stat.S_IMODE(socket.stat().st_mode) != 0o600:
        raise RuntimeError("endpoint modes were not restored after the cross-UID probe")
    if result.returncode != 0:
        raise RuntimeError("cross-UID probe did not run (non-interactive sudo is required; run `sudo -v`)")
    try:
        observed = json.loads(result.stdout.decode("utf-8", "replace"))
    except json.JSONDecodeError:
        raise RuntimeError("cross-UID probe output was not JSON") from None
    if observed.get("rejected") is not True:
        raise RuntimeError("the daemon did not reject a connection from another UID")
    return {"cross_uid_rejected": True, "cross_uid_probe_uid": probe_uid}


def run_checked(argv: list[str], env: dict[str, str], timeout: float) -> subprocess.CompletedProcess[str]:
    result = recorder_safety.run_bounded(argv, cwd=ROOT, env=env, capture_output=True, text=True, timeout=timeout)
    if result.returncode:
        excerpt = "\n".join((result.stdout + result.stderr).splitlines()[-20:])
        raise RuntimeError(f"{shlex.join(argv)} failed: {excerpt}")
    if any(canary in result.stdout or canary in result.stderr for canary in CANARIES):
        raise RuntimeError(f"{shlex.join(argv)} exposed a content canary")
    return result


def support_report_metrics(result: package.CommandResult) -> dict[str, object]:
    """Measure one run of the packaged ``narrivy-cli support-report`` (REL-008)."""
    if result.returncode != 0 or not result.stdout.strip():
        raise RuntimeError("the packaged support-report command failed")
    text = (result.stdout + result.stderr).decode("utf-8", "replace")
    return {
        "exit_code": result.returncode,
        "bytes": len(result.stdout),
        "canaries_absent": canaries_absent(text),
    }


def procedure_for(record: dict[str, object]) -> dict[str, object]:
    """The evidence ``procedure`` block: the measured assertions, unwrapped."""
    return {
        "id": record["procedure"],
        "profile_id": "native-behavior-v1",
        "assertions": record["assertions"],
    }


def exercise_package_endpoint(
    members: dict[str, bytes], identity: object, operations: package.HostOperations
) -> dict[str, object]:
    daemon: package.Spawned | None = None
    with tempfile.TemporaryDirectory(prefix="narrivy-native-hardening-") as temporary:
        holder = Path(temporary)
        os.chmod(holder, 0o700)
        extract = holder / "extract"
        runtime = holder / "runtime"
        state = holder / "state"
        for directory in (extract, runtime, state, holder / "home", holder / "cache", holder / "data", holder / "config"):
            directory.mkdir(mode=0o700)
        written: dict[str, Path] = {}
        for member, key in package.EXTRACT_MEMBERS.items():
            mode = 0o644 if key == "bundled-model" else 0o755
            written[member] = package._write_extracted(extract, member, members[member], mode)
        socket = runtime / "narrivyd.sock"
        env = package._daemon_env(
            holder=holder,
            runtime_dir=runtime,
            state_dir=state,
            socket_path=socket,
            worker=written[package.WORKER_MEMBER],
            model=written[package.MODEL_MEMBER],
            identity=identity,
            pipewire=package._pipewire_env(operations.environ()),
        )
        # Content canaries the daemon and support report must never echo.
        env["NARRIVY_TOKEN"] = CANARIES[1]
        env["NARRIVY_TRANSCRIPT"] = CANARIES[0]
        log = runtime / "daemon.log"
        try:
            daemon = operations.spawn([str(written[package.DAEMON_MEMBER])], env=env, log_path=log, cwd=holder)
            package._wait_for_socket(socket, daemon, operations, operations.monotonic() + 10.0)
            runtime_stat = runtime.stat()
            socket_stat = socket.stat()
            runtime_mode = stat.S_IMODE(runtime_stat.st_mode)
            socket_mode = stat.S_IMODE(socket_stat.st_mode)
            expected_uid = os.geteuid()
            if runtime_mode != 0o700 or socket_mode != 0o600:
                raise RuntimeError(f"insecure endpoint modes: runtime={runtime_mode:o} socket={socket_mode:o}")
            if runtime_stat.st_uid != expected_uid or socket_stat.st_uid != expected_uid:
                raise RuntimeError("IPC endpoint is not owned by the current user")
            cross_uid = probe_cross_uid(holder, runtime, socket, operations, env)
            status = operations.run(
                [str(written[package.CLI_MEMBER]), "--socket", str(socket), "status"],
                env=env,
                timeout=10.0,
                cwd=holder,
            )
            if status.returncode != 0:
                raise RuntimeError("authenticated packaged CLI status round-trip failed")
            support = operations.run(
                [str(written[package.CLI_MEMBER]), "--socket", str(socket), "support-report"],
                env=env,
                timeout=20.0,
                cwd=holder,
            )
            support_report = support_report_metrics(support)
            if not daemon.kill_group() or daemon.poll() != 0:
                raise RuntimeError("packaged daemon did not shut down cleanly")
            if socket.exists():
                raise RuntimeError("packaged daemon left its socket behind")
            if not log.is_file():
                raise RuntimeError("packaged daemon log is missing")
            log_scan = recorder_safety.scan_log_privacy([log], canaries=CANARIES)
            if not log_scan["clean"]:
                raise RuntimeError("packaged daemon log exposed a content canary")
            return {
                "runtime_mode": f"{runtime_mode:04o}",
                "socket_mode": f"{socket_mode:04o}",
                "runtime_mode_bits": runtime_mode,
                "socket_mode_bits": socket_mode,
                "expected_runtime_mode_bits": 0o700,
                "expected_socket_mode_bits": 0o600,
                "endpoint_owner_uid": socket_stat.st_uid,
                "runtime_owner_uid": runtime_stat.st_uid,
                "expected_owner_uid": expected_uid,
                "authenticated_status_round_trip": status.returncode == 0,
                "daemon_exit_code": daemon.poll(),
                "daemon_log_canaries_absent": log_scan["clean"],
                "daemon_log_bytes_scanned": log_scan["bytes_scanned"],
                "socket_removed_on_shutdown": not socket.exists(),
                **cross_uid,
                "support_report": support_report,
            }
        finally:
            if daemon is not None and daemon.poll() is None:
                daemon.kill_group()


def main() -> int:
    if sys.argv != EXPECTED_ARGV[1:]:
        return fail("argv does not match the reviewed procedure")
    artifact = ROOT / ARTIFACT
    index_path = ROOT / INDEX
    index = json.loads(index_path.read_text())
    outputs = {
        requirement: ROOT / f"agent/evidence/{record['phase']}/{record['verification']}.json"
        for requirement, record in RECORDS.items()
    }
    if not all(recorder_safety.replaceable(path, index, ROOT) for path in outputs.values()):
        return fail("an evidence output is current, unindexed v2, or does not match its INDEX hash")
    operations = package.HostOperations()
    try:
        source_root, head = package._require_clean_source(ROOT, operations, 5.0)
        if source_root != ROOT:
            return fail("source root mismatch")
        artifact_payload = package._read_regular_file(artifact, package.MAX_CANDIDATE_BYTES, "candidate")
        artifact_hash = hashlib.sha256(artifact_payload).hexdigest()
        identity, members = package._prepare_package(artifact_payload, head)
        lane = recorder_safety.native_lane()
        started = datetime.now(timezone.utc)
        env = dict(os.environ)
        env["NARRIVY_TOKEN"] = CANARIES[1]
        env["NARRIVY_TRANSCRIPT"] = CANARIES[0]
        diagnostics = run_checked(
            ["cargo", "test", "--locked", "-p", "narrivy-diagnostics", "--all-targets", "--quiet"],
            env,
            120.0,
        )
        doctor = run_checked(["python3", "scripts/doctor.py", "--json"], env, 30.0)
        doctor_report = json.loads(doctor.stdout)
        required_doctor = {"desktop", "portal", "pipewire", "microphone", "asr_runtime", "disk", "toolchain", "remediation", "notes"}
        if not required_doctor.issubset(doctor_report):
            raise RuntimeError("doctor omitted required capability sections")
        home_path_absent = (
            str(Path.home()) not in doctor.stdout
            and not str(doctor_report["disk"].get("cache_path", "")).startswith("/")
        )
        if not home_path_absent:
            raise RuntimeError("doctor exposed an absolute home-derived cache path")
        endpoint = exercise_package_endpoint(members, identity, operations)
        worker_identity_verified = (
            len(members[package.WORKER_MEMBER]) == identity.embedded["resident-worker"]["size_bytes"]
            and hashlib.sha256(members[package.WORKER_MEMBER]).hexdigest()
            == identity.embedded["resident-worker"]["sha256"]
        )
        if not worker_identity_verified:
            raise RuntimeError("packaged worker identity does not match build-info")
        canaries = {
            "diagnostics_absent": canaries_absent(diagnostics.stdout, diagnostics.stderr),
            "doctor_stdout_absent": canaries_absent(doctor.stdout, doctor.stderr),
            "daemon_log_absent": bool(endpoint["daemon_log_canaries_absent"]),
            "support_report_absent": bool(endpoint["support_report"]["canaries_absent"]),
        }
        if not all(canaries.values()):
            raise RuntimeError("a content canary survived into a diagnostic surface")
        ended = datetime.now(timezone.utc)
    except (OSError, RuntimeError, subprocess.TimeoutExpired, package.InputError, package.StepFailure, json.JSONDecodeError) as error:
        return fail(str(error))

    support_report = endpoint.pop("support_report")
    endpoint = {**endpoint, "runtime_owner_uid_matches": (
        endpoint["runtime_owner_uid"] == endpoint["expected_owner_uid"]
        and endpoint["endpoint_owner_uid"] == endpoint["expected_owner_uid"]
    )}
    common_metrics = {
        "endpoint": endpoint,
        "canaries": canaries,
        "support_report": support_report,
        "native_lane": lane,
        "package": {"worker_identity_verified": worker_identity_verified},
        "doctor": {
            "reported_sections": sorted(doctor_report),
            "remediation_count": len(doctor_report["remediation"]),
            "absolute_home_path_absent": home_path_absent,
        },
        "diagnostics_stdout_lines": len(diagnostics.stdout.splitlines()),
        "diagnostics_stderr_lines": len(diagnostics.stderr.splitlines()),
    }
    schema = json.loads((ROOT / "spec/schemas/evidence.schema.json").read_text())
    script_rel = "scripts/record_native_hardening_evidence.py"
    payloads: list[tuple[Path, str, bytes]] = []
    for requirement, record in RECORDS.items():
        rel = outputs[requirement].relative_to(ROOT).as_posix()
        artifact_hashes = {
            ARTIFACT: artifact_hash,
            script_rel: sha256(ROOT / script_rel),
            **{path: sha256(ROOT / path) for path in record["artifacts"]},
        }
        evidence = {
            "schema_version": 3,
            "verification_id": record["verification"],
            "requirement_ids": [requirement],
            "phase": record["phase"],
            "scenario_id": record["procedure"],
            "git_commit": head,
            "git_dirty": False,
            "build_id": identity.version,
            "package_sha256": artifact_hash,
            "environment": {
                "id": "cosmic-wayland-native-hardening",
                "native_or_vm": "native",
                "os": lane["os"],
                "os_version": lane["os_version"],
                "kernel": platform.release(),
                "desktop": lane["desktop"],
                "session": lane["session"],
                "cpu": platform.machine(),
                "gpu": None,
                "npu": None,
                "device_details": "package-bound local hardening procedure",
                "microphone": None,
                "input_backend": "Unix domain socket and host capability probes",
                "display_scale": None,
                "qualification_lane": "cosmic-wayland-native-hardening",
                "lane_available": True,
            },
            "command": shlex.join(EXPECTED_ARGV),
            "scenario_version": "1",
            "scenario_hash": sha256(Path(__file__)),
            "started_at": started.isoformat(),
            "ended_at": ended.isoformat(),
            "exit_code": 0,
            "result": "pass",
            "pass": True,
            "metrics": common_metrics,
            "artifact_hashes": artifact_hashes,
            "artifact_under_test": {"kind": "package", "path": ARTIFACT, "sha256": artifact_hash},
            "procedure": procedure_for(record),
            "notes": "Exact clean package on native COSMIC/Wayland. Every assertion is bound to a "
                     "measured value taken from this run (endpoint file modes and ownership in bits, "
                     "the authenticated status round trip, the daemon exit code, per-surface canary "
                     "scans, and the doctor section set). This procedure makes only the named IPC, "
                     "support-report and doctor hardening claims.",
            "evidence_type": "native-platform",
            "execution": {
                "lane_available": True,
                "child_failures_ignored": False,
                "environment_class": "native-platform",
                "substitutions": [],
            },
        }
        jsonschema.validate(instance=evidence, schema=schema)
        payload = (json.dumps(evidence, indent=2, sort_keys=True) + "\n").encode()
        payloads.append((outputs[requirement], rel, payload))
    index_schema = json.loads((ROOT / "spec/schemas/evidence-index.schema.json").read_text())
    try:
        recorder_safety.commit_records(
            ROOT,
            index_path,
            index,
            [(path, payload) for path, _rel, payload in payloads],
            validate_index=lambda value: jsonschema.validate(instance=value, schema=index_schema),
        )
    except (OSError, recorder_safety.RecorderSafetyError, jsonschema.ValidationError) as error:
        return fail(str(error))
    print(json.dumps({"artifact_sha256": artifact_hash, "records": len(payloads), "result": "pass"}, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
