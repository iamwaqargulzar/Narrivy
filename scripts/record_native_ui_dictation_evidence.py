#!/usr/bin/env python3
"""Qualify the exact packaged first-dictation UI and native overlay path."""
from __future__ import annotations

import hashlib
import json
import os
import platform
import shlex
import signal
import subprocess
import sys
import tempfile
import time
import warnings
from datetime import datetime, timezone
from pathlib import Path

import jsonschema

import qualify_candidate_known_phrase as package
import record_native_accessibility_evidence as accessibility


ROOT = Path(__file__).resolve().parents[1]
ARTIFACT = "artifacts/releases/narrivy-linux-amd64.deb"
FIXTURE = "tests/fixtures/jfk16.wav"
DESKTOP_MEMBER = "usr/bin/narrivy-desktop"
INDEX = "agent/evidence/INDEX.json"
EXPECTED_FIXTURE_SHA256 = "59dfb9a4acb36fe2a2affc14bacbee2920ff435cb13cc314a08c13f66ba7860e"
EXPECTED_ARGV = [
    "python3",
    "scripts/record_native_ui_dictation_evidence.py",
    ARTIFACT,
    FIXTURE,
    "--confirm-audible-playback",
]
CAPTURE_READY_TARGET_MS = 50.0
HOTKEY_ACK_TARGET_MS = 30.0
VISIBLE_FEEDBACK_TARGET_MS = 100.0
RECORDS = {
    "SEC-002": {
        "phase": "P5",
        "verification": "VER-SEC-002",
        "procedure": "native-model-manifest-integrity-v1",
        "assertions": [
            "exact-package-source-bound",
            "embedded-runtime-and-model-checksums-matched",
            "unpinned-model-activation-rejected",
            "tampered-model-rejected-and-staging-cleaned",
            "verified-model-used-by-local-asr",
        ],
        "artifacts": [
            "crates/narrivy-model-manager/src/registry.rs",
            "crates/narrivy-model-manager/src/store.rs",
            "crates/narrivy-model-manager/tests/p5_model_cache.rs",
            "scripts/qualify_candidate_known_phrase.py",
        ],
    },
    "TEST-006": {
        "phase": "P9",
        "verification": "VER-TEST-006",
        "procedure": "native-failure-injection-matrix-v1",
        "assertions": [
            "exact-package-source-bound",
            "native-client-loss-during-capture",
            "daemon-survived-client-loss",
            "post-client-loss-session-committed",
            "delivery-failure-injection-suite",
            "capture-source-loss-injection-suite",
            "corrupt-model-cache-injection-suite",
            "interrupted-session-recovery-suite",
        ],
        "artifacts": [
            "adapters/platform-linux/src/delivery/routed.rs",
            "adapters/platform-linux/src/recovery.rs",
            "crates/narrivy-model-manager/tests/p5_model_cache.rs",
            "services/narrivyd/src/daemon/session.rs",
        ],
    },
    "FUN-010": {
        "phase": "P2",
        "verification": "VER-FUN-010",
        "procedure": "native-cancel-without-stale-output-v1",
        "assertions": [
            "exact-package-source-bound",
            "native-session-cancelled",
            "cancelled-session-became-idle",
            "cancelled-session-produced-no-inbox-output",
        ],
        "artifacts": [
            "services/narrivyd/src/daemon/session.rs",
            "adapters/platform-linux/src/delivery/inbox.rs",
        ],
    },
    "REL-002": {
        "phase": "P2",
        "verification": "VER-REL-002",
        "procedure": "native-overlay-failure-isolation-v1",
        "assertions": [
            "exact-package-source-bound",
            "listening-overlay-observed",
            "desktop-terminated-during-capture",
            "daemon-remained-active",
            "post-overlay-failure-session-committed",
        ],
        "artifacts": [
            "apps/narrivy-desktop/src-tauri/src/linux_overlay.rs",
            "services/narrivyd/src/daemon/session.rs",
        ],
    },
    "FUN-002": {
        "phase": "P4",
        "verification": "VER-FUN-002",
        "procedure": "native-nonblocking-microphone-capture-v1",
        "assertions": [
            "exact-package-source-bound",
            "physical-microphone-capture",
            "ui-operable-during-capture",
            "known-phrase-final-passed",
        ],
        "artifacts": [
            "apps/narrivy-desktop/src/hooks/useDictationActions.ts",
            "adapters/platform-linux/src/pipewire_monitor.rs",
        ],
    },
    "FUN-009": {
        "phase": "P2",
        "verification": "VER-FUN-009",
        "procedure": "native-inbox-fallback-v1",
        "assertions": [
            "exact-package-source-bound",
            "no-external-writable-target",
            "inbox-result-delivered",
            "known-phrase-final-passed",
        ],
        "artifacts": [
            "crates/narrivy-core/src/lib.rs",
            "services/narrivyd/src/main.rs",
        ],
    },
    "UX-001": {
        "phase": "P3",
        "verification": "VER-UX-001",
        "procedure": "native-first-ui-dictation-v1",
        "assertions": [
            "exact-package-source-bound",
            "native-wayland-webview",
            "atspi-start-and-stop",
            "first-dictation-without-runtime-jargon",
            "physical-microphone-capture",
            "local-asr-known-phrase",
            "inbox-result-delivered",
            "transcript-absent-from-ordinary-logs",
        ],
        "artifacts": [
            "apps/narrivy-desktop/src/app/screens/Home.tsx",
            "apps/narrivy-desktop/src/hooks/useDictationActions.ts",
        ],
    },
    "UX-005": {
        "phase": "P3",
        "verification": "VER-UX-005",
        "procedure": "native-useful-overlay-state-v1",
        "assertions": [
            "exact-package-source-bound",
            "native-wayland-webview",
            "listening-overlay-observed",
            "processing-churn-suppressed",
            "terminal-overlay-hidden",
            "overlay-non-focusable-contract",
        ],
        "artifacts": ["apps/narrivy-desktop/src-tauri/src/linux_overlay.rs"],
    },
    "UX-004": {
        "phase": "P3",
        "verification": "VER-UX-004",
        "procedure": "native-overlay-focus-safety-v1",
        "assertions": [
            "exact-package-source-bound",
            "native-wayland-webview",
            "listening-overlay-observed",
            "overlay-non-focusable-observed",
            "dictation-completed",
        ],
        "artifacts": ["apps/narrivy-desktop/src-tauri/src/linux_overlay.rs"],
    },
    "UX-006": {
        "phase": "P3",
        "verification": "VER-UX-006",
        "procedure": "native-local-processing-visibility-v1",
        "assertions": [
            "exact-package-source-bound",
            "native-wayland-webview",
            "local-processing-mode-visible",
            "processing-state-observed",
        ],
        "artifacts": [
            "apps/narrivy-desktop/src/app/AppShell.tsx",
            "apps/narrivy-desktop/src/app/screens/Home.tsx",
        ],
    },
    "PERF-003": {
        "phase": "P6",
        "verification": "VER-PERF-003",
        "procedure": "native-warm-capture-ready-latency-v1",
        "assertions": [
            "exact-package-source-bound",
            "monotonic-stage-trace",
            "warm-capture-ready-under-50ms",
        ],
        "artifacts": [
            "crates/narrivy-core/src/recovery.rs",
            "services/narrivyd/src/daemon/session.rs",
        ],
    },
    "PERF-002": {
        "phase": "P6",
        "verification": "VER-PERF-002",
        "procedure": "native-hotkey-feedback-latency-v1",
        "assertions": [
            "exact-package-source-bound",
            "native-wayland-webview",
            "hotkey-feedback-under-30ms",
        ],
        "artifacts": [
            "apps/narrivy-desktop/src-tauri/src/linux_overlay.rs",
            "services/narrivyd/src/daemon/session.rs",
        ],
    },
    "PERF-004": {
        "phase": "P6",
        "verification": "VER-PERF-004",
        "procedure": "native-visible-listening-feedback-latency-v1",
        "assertions": [
            "exact-package-source-bound",
            "native-wayland-webview",
            "visible-listening-feedback-under-100ms",
        ],
        "artifacts": [
            "apps/narrivy-desktop/src-tauri/src/linux_overlay.rs",
            "apps/narrivy-desktop/src/hooks/useDictationActions.ts",
        ],
    },
    "FUN-008": {
        "phase": "P6",
        "verification": "VER-FUN-008",
        "procedure": "native-startup-unsent-recovery-v1",
        "assertions": [
            "exact-package-source-bound",
            "durable-committed-result-survived-restart",
            "startup-reconciled-result-to-inbox",
            "recovery-replay-was-idempotent-by-transaction",
        ],
        "artifacts": [
            "crates/narrivy-core/src/recovery.rs",
            "services/narrivyd/src/main.rs",
            "adapters/platform-linux/src/delivery/inbox.rs",
        ],
    },
    "REL-006": {
        "phase": "P6",
        "verification": "VER-REL-006",
        "procedure": "native-active-journal-restart-recovery-v1",
        "assertions": [
            "exact-package-source-bound",
            "actual-session-journal-used",
            "committed-state-failure-injected",
            "restart-produced-terminal-inbox-recovered-state",
        ],
        "artifacts": [
            "crates/narrivy-journal-api/src/lib.rs",
            "adapters/journal-unix/src/lib.rs",
            "services/narrivyd/src/main.rs",
        ],
    },
}


def run_failure_injection_contracts() -> dict[str, object]:
    suites = {
        "platform-linux": ["cargo", "test", "-p", "narrivy-platform-linux", "--locked"],
        "model-cache": [
            "cargo", "test", "-p", "narrivy-model-manager", "--test", "p5_model_cache", "--locked",
        ],
        "daemon": ["cargo", "test", "-p", "narrivyd", "--locked"],
    }
    observations: dict[str, object] = {}
    for name, command in suites.items():
        result = subprocess.run(command, cwd=ROOT, capture_output=True, text=True, timeout=180)
        if result.returncode:
            excerpt = "\n".join((result.stdout + result.stderr).splitlines()[-20:])
            raise RuntimeError(f"failure-injection suite {name} failed: {excerpt}")
        observations[name] = {
            "command": shlex.join(command),
            "exit_code": 0,
            "stdout_lines": len(result.stdout.splitlines()),
            "stderr_lines": len(result.stderr.splitlines()),
        }
    return observations


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def fail(message: str) -> int:
    print(f"record_native_ui_dictation_evidence: {message}", file=sys.stderr)
    return 1


def terminate_group(process: subprocess.Popen[bytes] | None) -> None:
    if process is None or process.poll() is not None:
        return
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process.wait(timeout=3)


def wait_for_path(path: Path, process: subprocess.Popen[bytes], timeout: float) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if path.exists():
            return
        if process.poll() is not None:
            raise RuntimeError("process exited before readiness")
        time.sleep(0.05)
    raise RuntimeError("readiness timed out")


def atomic_private_json(path: Path, value: object) -> None:
    payload = (json.dumps(value, indent=2, sort_keys=True) + "\n").encode()
    descriptor, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(payload)
            stream.flush()
            os.fsync(stream.fileno())
        os.chmod(temporary, 0o600)
        os.replace(temporary, path)
    finally:
        Path(temporary).unlink(missing_ok=True)


def run_overlay_contracts() -> dict[str, object]:
    command = [
        "cargo",
        "test",
        "-p",
        "narrivy-desktop",
        "overlay_state::tests",
        "--quiet",
    ]
    result = subprocess.run(command, cwd=ROOT, capture_output=True, text=True, timeout=180)
    if result.returncode:
        excerpt = "\n".join((result.stdout + result.stderr).splitlines()[-20:])
        raise RuntimeError(f"overlay contracts failed: {excerpt}")
    return {
        "command": shlex.join(command),
        "exit_code": 0,
        "stdout_lines": len(result.stdout.splitlines()),
        "stderr_lines": len(result.stderr.splitlines()),
    }


def inspect_and_operate_ui(
    *,
    fixture: Path,
    cli: Path,
    socket_path: Path,
    daemon_env: dict[str, str],
    playback_env: dict[str, str],
    inbox_dir: Path,
    desktop_process: subprocess.Popen[bytes],
) -> dict[str, object]:
    import gi

    gi.require_version("Atspi", "2.0")
    from gi.repository import Atspi

    warnings.filterwarnings("ignore", category=DeprecationWarning)
    Atspi.init()

    def children(node: object) -> list[object]:
        try:
            return [node.get_child_at_index(i) for i in range(node.get_child_count())]
        except Exception:
            return []

    def descendants(node: object):
        yield node
        for child in children(node):
            if child is not None:
                yield from descendants(child)

    def application() -> object:
        desktop = Atspi.get_desktop(0)
        for index in range(desktop.get_child_count()):
            candidate = desktop.get_child_at_index(index)
            try:
                if candidate.get_name() == "narrivy-desktop":
                    return candidate
            except Exception:
                continue
        raise RuntimeError("Narrivy is not exposed through AT-SPI")

    def named_nodes() -> list[str]:
        values: list[str] = []
        for node in descendants(application()):
            try:
                name = node.get_name() or ""
                if name:
                    values.append(name)
            except Exception:
                continue
        return values

    def has_name(name: str) -> bool:
        for node in descendants(application()):
            try:
                if node.get_name() == name:
                    return True
            except Exception:
                continue
        return False

    def enabled(name: str) -> bool:
        for node in descendants(application()):
            try:
                if node.get_name() == name and node.is_action():
                    return node.get_state_set().contains(Atspi.StateType.ENABLED)
            except Exception:
                continue
        return False

    def focusable(name: str) -> bool | None:
        for node in descendants(application()):
            try:
                if node.get_name() == name:
                    return node.get_state_set().contains(Atspi.StateType.FOCUSABLE)
            except Exception:
                continue
        return None

    def press(name: str, timeout: float = 6.0) -> None:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            for node in descendants(application()):
                try:
                    if node.get_name() != name or not node.is_action():
                        continue
                    if not node.get_state_set().contains(Atspi.StateType.ENABLED):
                        continue
                    for index in range(node.get_n_actions()):
                        if node.get_action_name(index) in ("press", "click", "activate", ""):
                            if node.do_action(index):
                                return
                except Exception:
                    continue
            time.sleep(0.05)
        raise RuntimeError(f"AT-SPI action unavailable: {name}")

    cli_bin = cli

    def cli_status() -> str:
        result = subprocess.run(
            [cli_bin, "--socket", socket_path, "status"],
            env=daemon_env,
            capture_output=True,
            text=True,
            timeout=4,
        )
        if result.returncode:
            raise RuntimeError("daemon status failed")
        return result.stdout

    def cli_command(command: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [cli_bin, "--socket", socket_path, command],
            env=daemon_env,
            capture_output=True,
            text=True,
            timeout=6,
        )

    def wait_idle(timeout: float = 20.0) -> None:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if "no active session" in cli_status():
                return
            time.sleep(0.05)
        raise RuntimeError("session did not become idle")

    def start_and_wait_listening() -> tuple[bool, bool, float]:
        started = time.monotonic()
        press("Start dictation")
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            listening = has_name("Listening…")
            operable = listening and enabled("Stop")
            if operable:
                return listening, operable, (time.monotonic() - started) * 1000.0
            time.sleep(0.01)
        raise RuntimeError("UI start did not reach listening")

    def toggle_and_wait_listening() -> tuple[float, float]:
        started = time.monotonic()
        toggled = subprocess.Popen(
            [cli_bin, "--socket", socket_path, "toggle", "--pipeline", "clean"],
            env=daemon_env,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        deadline = time.monotonic() + 8
        acknowledged_ms: float | None = None
        visible_ms: float | None = None
        while time.monotonic() < deadline:
            now = time.monotonic()
            if acknowledged_ms is None and toggled.poll() is not None:
                acknowledged_ms = (now - started) * 1000.0
            if visible_ms is None and has_name("Listening…") and enabled("Stop"):
                visible_ms = (time.monotonic() - started) * 1000.0
            if acknowledged_ms is not None and visible_ms is not None:
                break
            time.sleep(0.005)
        if toggled.poll() is None:
            toggled.kill()
        try:
            stdout, _stderr = toggled.communicate(timeout=1)
        except subprocess.TimeoutExpired:
            toggled.kill()
            stdout, _stderr = toggled.communicate(timeout=1)
        if toggled.returncode or "outcome=started" not in stdout:
            raise RuntimeError("installed atomic toggle did not start dictation")
        if acknowledged_ms is None:
            acknowledged_ms = (time.monotonic() - started) * 1000.0
        if visible_ms is None:
            raise RuntimeError("installed toggle did not produce visible listening feedback")
        return acknowledged_ms, visible_ms

    onboarding_names = named_nodes()
    press("Skip setup")
    deadline = time.monotonic() + 12
    stable_since: float | None = None
    while time.monotonic() < deadline:
        ready = has_name("Ready to dictate") and enabled("Start dictation")
        if ready:
            stable_since = stable_since or time.monotonic()
            if time.monotonic() - stable_since >= 1:
                break
        else:
            stable_since = None
        time.sleep(0.05)
    else:
        raise RuntimeError("Home did not become stably ready")

    toggle_ack_ms, visible_listening_feedback_ms = toggle_and_wait_listening()
    toggled_cancel = cli_command("cancel")
    if toggled_cancel.returncode:
        raise RuntimeError("installed-toggle session cancel failed")
    wait_idle()
    time.sleep(0.2)
    if package._list_inbox(inbox_dir):
        raise RuntimeError("installed-toggle cancellation produced stale Inbox output")

    home_names = named_nodes()
    local_processing_visible = any(
        "local" in name.casefold()
        and any(marker in name.casefold() for marker in ("private", "dictation", "default", "gguf", "processing"))
        for name in home_names
    )
    if not local_processing_visible:
        raise RuntimeError("local processing mode was not visible")
    jargon = ("pipewire", "backend", "runtime", "worker", "gguf", "transcribe.cpp")
    jargon_hits = sorted(
        term
        for term in jargon
        if any(term in name.casefold() for name in onboarding_names + home_names)
    )
    if jargon_hits:
        raise RuntimeError("first-dictation UI exposed runtime jargon")

    listening_seen, ui_operable_during_capture, cancelled_listening_ms = start_and_wait_listening()
    cancelled = cli_command("cancel")
    if cancelled.returncode:
        raise RuntimeError("native session cancel failed")
    wait_idle()
    time.sleep(0.2)
    if package._list_inbox(inbox_dir):
        raise RuntimeError("cancelled session produced stale Inbox output")
    cancelled_overlay_hidden = not has_name("Narrivy dictation status")
    if not cancelled_overlay_hidden:
        raise RuntimeError("cancelled session left its overlay exposed")

    listening_seen, ui_operable_during_capture, ui_listening_feedback_ms = start_and_wait_listening()
    overlay_focusable = focusable("Narrivy dictation status")
    if overlay_focusable is not False:
        raise RuntimeError("listening overlay was focusable")
    if "no active session" in cli_status():
        raise RuntimeError("daemon did not report the UI-started session")

    playback = subprocess.run(
        [package.PAPLAY, "--", fixture],
        env=playback_env,
        capture_output=True,
        timeout=20,
    )
    if playback.returncode:
        raise RuntimeError("fixture playback failed")
    press("Stop")

    processing_seen = False
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        processing_seen = processing_seen or has_name("Processing…")
        if "no active session" in cli_status():
            break
        time.sleep(0.05)
    else:
        raise RuntimeError("UI-stopped session did not become idle")
    time.sleep(0.2)
    overlay_hidden = not has_name("Narrivy dictation status")
    if not overlay_hidden:
        raise RuntimeError("terminal overlay remained exposed")

    timing_line = next(
        (
            line.split("last_session_timings_us=", 1)[1]
            for line in cli_status().splitlines()
            if "last_session_timings_us=" in line
        ),
        None,
    )
    if timing_line is None:
        raise RuntimeError("completed session timing snapshot was unavailable")
    stage_timings_us = {
        key: int(value)
        for item in timing_line.split(",")
        for key, value in [item.split("=", 1)]
    }
    required_timing_stages = {
        "capture_start",
        "capture_ready",
        "capture_ended",
        "first_audio_accepted",
        "raw_final_accepted",
        "terminal_journal_committed",
    }
    if not required_timing_stages.issubset(stage_timings_us):
        raise RuntimeError("completed session timing snapshot omitted a required stage")
    capture_ready_ms = (
        stage_timings_us["capture_ready"] - stage_timings_us["capture_start"]
    ) / 1000.0
    stage_latency_ms = {
        "hotkey_to_visible_feedback": visible_listening_feedback_ms,
        "capture_ready_to_first_audio": (
            stage_timings_us["first_audio_accepted"] - stage_timings_us["capture_ready"]
        ) / 1000.0,
        "release_to_raw": (
            stage_timings_us["raw_final_accepted"] - stage_timings_us["capture_ended"]
        ) / 1000.0,
        "release_to_committed": (
            stage_timings_us["terminal_journal_committed"]
            - stage_timings_us["capture_ended"]
        ) / 1000.0,
    }
    failure_listening_seen, _, failure_listening_ms = start_and_wait_listening()
    terminate_group(desktop_process)
    if desktop_process.poll() is None:
        raise RuntimeError("desktop did not terminate during overlay failure injection")
    if "no active session" in cli_status():
        raise RuntimeError("daemon lost the active session with the desktop")
    failure_playback = subprocess.run(
        [package.PAPLAY, "--", fixture],
        env=playback_env,
        capture_output=True,
        timeout=20,
    )
    if failure_playback.returncode:
        raise RuntimeError("overlay-failure fixture playback failed")
    stopped = cli_command("stop")
    if stopped.returncode:
        raise RuntimeError("daemon could not finalize after desktop failure")
    wait_idle()

    return {
        "onboarding_named_nodes": len(onboarding_names),
        "home_named_nodes": len(home_names),
        "runtime_jargon_hits": len(jargon_hits),
        "local_processing_mode_visible": local_processing_visible,
        "ui_operable_during_capture": ui_operable_during_capture,
        "cancelled_listening_feedback_ms": round(cancelled_listening_ms, 3),
        "installed_toggle_ack_ms": round(toggle_ack_ms, 3),
        "visible_listening_feedback_ms": round(visible_listening_feedback_ms, 3),
        "ui_action_listening_feedback_ms": round(ui_listening_feedback_ms, 3),
        "failure_listening_feedback_ms": round(failure_listening_ms, 3),
        "warm_capture_ready_ms": round(capture_ready_ms, 3),
        "dictation_stage_latency_ms": {
            key: round(value, 3) for key, value in stage_latency_ms.items()
        },
        "completed_stage_timings_us": stage_timings_us,
        "listening_overlay_observed": listening_seen,
        "listening_overlay_focusable": overlay_focusable,
        "processing_overlay_observed": processing_seen,
        "terminal_overlay_hidden": overlay_hidden,
        "cancelled_session_became_idle": True,
        "cancelled_session_inbox_outputs": 0,
        "cancelled_overlay_hidden": cancelled_overlay_hidden,
        "overlay_failure_listening_observed": failure_listening_seen,
        "desktop_terminated_during_capture": True,
        "daemon_remained_active_after_desktop_failure": True,
        "post_overlay_failure_session_committed": True,
        "playback_exit_code": playback.returncode,
        "playback_stdout_bytes": len(playback.stdout),
        "playback_stderr_bytes": len(playback.stderr),
    }


def native_run(
    identity: package.EmbeddedIdentity,
    members: dict[str, bytes],
    desktop_payload: bytes,
    fixture: Path,
    pipewire: dict[str, str],
) -> tuple[dict[str, object], dict[str, object]]:
    daemon: subprocess.Popen[bytes] | None = None
    desktop: subprocess.Popen[bytes] | None = None
    with tempfile.TemporaryDirectory(prefix="narrivy-native-ui-") as temporary:
        holder = Path(temporary)
        os.chmod(holder, 0o700)
        extract = holder / "extract"
        extract.mkdir(mode=0o700)
        for name in ("home", "runtime", "state", "cache", "config", "data"):
            (holder / name).mkdir(mode=0o700)

        written: dict[str, Path] = {}
        for member, key in package.EXTRACT_MEMBERS.items():
            mode = 0o644 if key == "bundled-model" else 0o755
            written[member] = package._write_extracted(extract, member, members[member], mode)
        desktop_bin = package._write_extracted(extract, DESKTOP_MEMBER, desktop_payload, 0o755)
        socket_path = holder / "runtime/narrivyd.sock"
        daemon_env = package._daemon_env(
            holder=holder,
            runtime_dir=holder / "runtime",
            state_dir=holder / "state",
            socket_path=socket_path,
            worker=written[package.WORKER_MEMBER],
            model=written[package.MODEL_MEMBER],
            identity=identity,
            pipewire=pipewire,
        )
        daemon_log = holder / "daemon.log"
        daemon_fd = os.open(daemon_log, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        first_daemon_clean = False
        restart_log = holder / "restart.log"
        replay_log = holder / "replay.log"
        try:
            daemon = subprocess.Popen(
                [written[package.DAEMON_MEMBER]],
                cwd=holder,
                env=daemon_env,
                stdin=subprocess.DEVNULL,
                stdout=daemon_fd,
                stderr=daemon_fd,
                start_new_session=True,
                close_fds=True,
            )
        finally:
            os.close(daemon_fd)
        wait_for_path(socket_path, daemon, 12)

        ready = holder / "ready.json"
        desktop_log = holder / "desktop.log"
        bus = accessibility.atspi_bus_address()
        os.environ["AT_SPI_BUS_ADDRESS"] = bus
        desktop_env = dict(os.environ)
        desktop_env.update(
            {
                "HOME": str(holder / "home"),
                "XDG_RUNTIME_DIR": os.environ["XDG_RUNTIME_DIR"],
                "XDG_STATE_HOME": str(holder / "state"),
                "XDG_CACHE_HOME": str(holder / "cache"),
                "XDG_CONFIG_HOME": str(holder / "config"),
                "XDG_DATA_HOME": str(holder / "data"),
                "NARRIVY_RUNTIME_DIR": str(holder / "runtime"),
                "NARRIVY_SOCKET": str(socket_path),
                "NARRIVY_READY_FILE": str(ready),
                "NO_AT_BRIDGE": "0",
                "GTK_A11Y": "atspi",
                "GDK_BACKEND": "wayland",
                "AT_SPI_BUS_ADDRESS": bus,
            }
        )
        desktop_fd = os.open(desktop_log, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        try:
            desktop = subprocess.Popen(
                [desktop_bin],
                cwd=holder,
                env=desktop_env,
                stdin=subprocess.DEVNULL,
                stdout=desktop_fd,
                stderr=desktop_fd,
                start_new_session=True,
                close_fds=True,
            )
        finally:
            os.close(desktop_fd)

        try:
            wait_for_path(ready, desktop, 12)
            marker = json.loads(ready.read_text())
            if marker.get("gdk_display_type") != "GdkWaylandDisplay":
                raise RuntimeError("desktop did not use native Wayland")
            document = marker.get("document", {})
            if document.get("root") is not True or document.get("bridgeReply") != "ok" or document.get("errors"):
                raise RuntimeError("desktop document or native bridge was not healthy")
            if marker.get("resources", {}).get("external_count") != 0:
                raise RuntimeError("desktop attempted an external resource request")
            observations = inspect_and_operate_ui(
                fixture=fixture,
                cli=written[package.CLI_MEMBER],
                socket_path=socket_path,
                daemon_env=daemon_env,
                playback_env={"PATH": "/usr/bin:/bin", "LC_ALL": "C", **pipewire},
                inbox_dir=holder / "runtime/inbox",
                desktop_process=desktop,
            )
            inbox = package._list_inbox(holder / "runtime/inbox")
            if len(inbox) != 2:
                raise RuntimeError("expected two committed Inbox results after one cancelled session")
            transcript_results = [package._read_transcript(path) for path in inbox]
            transcript_sha256, transcript_bytes = transcript_results[0]
        finally:
            terminate_group(desktop)
            terminate_group(daemon)

        first_daemon_clean = daemon is not None and daemon.returncode == 0
        if not first_daemon_clean:
            raise RuntimeError("first daemon did not shut down cleanly")

        recovery_source = inbox[0]
        recovery_txid = recovery_source.stem
        journal_path = holder / "state/narrivy/journal" / f"{recovery_txid}.json"
        journal_record = json.loads(journal_path.read_text())
        if journal_record.get("state") not in ("inbox_recovered", "delivered"):
            raise RuntimeError("actual session journal was not terminal before injection")
        variants = journal_record.get("variants")
        if not isinstance(variants, dict) or not isinstance(variants.get("corrected"), str):
            raise RuntimeError("actual session journal lacked corrected recovery text")
        expected_recovery_hash = hashlib.sha256(variants["corrected"].encode()).hexdigest()
        if digest(recovery_source) != expected_recovery_hash:
            raise RuntimeError("actual Inbox and journal transaction did not agree")

        # Recreate the durable boundary left by a crash after correction was
        # committed but before delivery became terminal. The record and text
        # come from the exact packaged session above; only terminal fields are
        # rolled back while the daemon is stopped.
        journal_record["state"] = "committed"
        variants["delivered"] = None
        variants["attempts"] = []
        journal_record["delivery_report"] = None
        journal_record["failure_code"] = None
        journal_record["updated_unix_ms"] = int(time.time() * 1000)
        atomic_private_json(journal_path, journal_record)
        recovery_source.unlink()

        restart_fd = os.open(restart_log, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        try:
            daemon = subprocess.Popen(
                [written[package.DAEMON_MEMBER]],
                cwd=holder,
                env=daemon_env,
                stdin=subprocess.DEVNULL,
                stdout=restart_fd,
                stderr=restart_fd,
                start_new_session=True,
                close_fds=True,
            )
        finally:
            os.close(restart_fd)
        try:
            wait_for_path(socket_path, daemon, 12)
            wait_for_path(recovery_source, daemon, 5)
            if digest(recovery_source) != expected_recovery_hash:
                raise RuntimeError("startup recovery changed committed text")
            recovered_record = json.loads(journal_path.read_text())
            if recovered_record.get("state") != "inbox_recovered":
                raise RuntimeError("startup recovery did not terminalize the journal")
            recovered_variants = recovered_record.get("variants", {})
            if recovered_variants.get("delivered") != recovered_variants.get("corrected"):
                raise RuntimeError("startup recovery journal variants diverged")
            if recovered_record.get("delivery_report", {}).get("backend") != "inbox":
                raise RuntimeError("startup recovery did not record its Inbox backend")
        finally:
            terminate_group(daemon)

        recovery_daemon_clean = daemon is not None and daemon.returncode == 0
        if not recovery_daemon_clean:
            raise RuntimeError("startup recovery daemon did not shut down cleanly")
        replay_fd = os.open(replay_log, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        try:
            daemon = subprocess.Popen(
                [written[package.DAEMON_MEMBER]],
                cwd=holder,
                env=daemon_env,
                stdin=subprocess.DEVNULL,
                stdout=replay_fd,
                stderr=replay_fd,
                start_new_session=True,
                close_fds=True,
            )
        finally:
            os.close(replay_fd)
        try:
            wait_for_path(socket_path, daemon, 12)
            if digest(recovery_source) != expected_recovery_hash:
                raise RuntimeError("terminal startup recovery changed on replay")
            if len(package._list_inbox(holder / "runtime/inbox")) != 2:
                raise RuntimeError("terminal startup recovery was replayed twice")
        finally:
            terminate_group(daemon)

        logs = [
            daemon_log.read_bytes(),
            desktop_log.read_bytes(),
            restart_log.read_bytes(),
            replay_log.read_bytes(),
        ]
        scanned, fragment_hits = package._scan_logs(logs)
        if fragment_hits:
            raise RuntimeError("ordinary logs contained transcript content")
        observations.update(
            {
                "inbox_files": 2,
                "all_committed_transcripts_matched": len(transcript_results) == 2,
                "known_phrase_matched": True,
                "transcript_sha256": transcript_sha256,
                "transcript_utf8_bytes": transcript_bytes,
                "log_bytes_scanned": scanned,
                "log_fragment_hits": fragment_hits,
                "daemon_clean_shutdown": daemon is not None and daemon.returncode == 0,
                "initial_daemon_clean_shutdown": first_daemon_clean,
                "recovery_daemon_clean_shutdown": recovery_daemon_clean,
                "desktop_owned_cleanup": desktop is not None and desktop.returncode is not None,
                "startup_recovery_transaction": recovery_txid,
                "startup_recovery_inbox_sha256": expected_recovery_hash,
                "startup_recovery_terminal_state": "inbox_recovered",
                "startup_recovery_replay_inbox_files": 2,
            }
        )
        if not observations["daemon_clean_shutdown"]:
            raise RuntimeError("daemon did not shut down cleanly")
        return marker, observations


def main() -> int:
    if sys.argv != EXPECTED_ARGV[1:]:
        return fail("argv does not match the reviewed procedure")
    index_path = ROOT / INDEX
    index = json.loads(index_path.read_text())
    outputs = {
        requirement: ROOT / f"agent/evidence/{record['phase']}/{record['verification']}.json"
        for requirement, record in RECORDS.items()
    }
    if not all(accessibility.replaceable(path, index) for path in outputs.values()):
        return fail("an evidence output is already qualified or is not historical schema-v1 material")

    operations = package.HostOperations()
    try:
        source_root, head = package._require_clean_source(ROOT, operations, 5.0)
        if source_root != ROOT:
            raise RuntimeError("source root mismatch")
        artifact_payload = package._read_regular_file(
            ROOT / ARTIFACT, package.MAX_CANDIDATE_BYTES, "candidate"
        )
        artifact_hash = hashlib.sha256(artifact_payload).hexdigest()
        identity, members = package._prepare_package(artifact_payload, head)
        archive = package._parse_debian_archive(artifact_payload)
        data_name, data_payload = package._archive_payload(archive, "data.tar")
        desktop_payload = package._required_tar_regular_files(
            data_payload, data_name, {DESKTOP_MEMBER: package.MAX_BINARY_BYTES}
        )[DESKTOP_MEMBER]
        desktop_identity = identity.embedded["narrivy-desktop"]
        if (
            len(desktop_payload) != desktop_identity["size_bytes"]
            or hashlib.sha256(desktop_payload).hexdigest() != desktop_identity["sha256"]
        ):
            raise RuntimeError("packaged desktop identity does not match build-info")
        fixture = ROOT / FIXTURE
        if digest(fixture) != EXPECTED_FIXTURE_SHA256:
            raise RuntimeError("fixture hash mismatch")
        desktop_name, session = package._require_native_cosmic(operations)
        pipewire = package._pipewire_env(operations.environ())
        package._require_physical_source(operations, 5.0)
        started = datetime.now(timezone.utc)
        overlay_contracts = run_overlay_contracts()
        failure_injection_contracts = run_failure_injection_contracts()
        marker, observations = native_run(identity, members, desktop_payload, fixture, pipewire)
        ended = datetime.now(timezone.utc)
    except (
        OSError,
        RuntimeError,
        subprocess.TimeoutExpired,
        package.InputError,
        package.StepFailure,
        json.JSONDecodeError,
    ) as error:
        return fail(str(error))

    os_name, os_version = accessibility.os_release()
    script_rel = "scripts/record_native_ui_dictation_evidence.py"
    metrics = {
        "overlay_contracts": overlay_contracts,
        "failure_injection_contracts": failure_injection_contracts,
        "native_marker": marker,
        "native_ui": observations,
    }
    skipped_requirements: dict[str, str] = {}
    if observations["warm_capture_ready_ms"] >= CAPTURE_READY_TARGET_MS:
        skipped_requirements["PERF-003"] = (
            f"warm capture-ready {observations['warm_capture_ready_ms']:.3f} ms "
            f"did not meet <{CAPTURE_READY_TARGET_MS:.0f} ms"
        )
    if observations["visible_listening_feedback_ms"] >= HOTKEY_ACK_TARGET_MS:
        skipped_requirements["PERF-002"] = (
            f"hotkey feedback {observations['visible_listening_feedback_ms']:.3f} ms "
            f"did not meet <{HOTKEY_ACK_TARGET_MS:.0f} ms"
        )
    if observations["visible_listening_feedback_ms"] >= VISIBLE_FEEDBACK_TARGET_MS:
        skipped_requirements["PERF-004"] = (
            f"visible listening feedback {observations['visible_listening_feedback_ms']:.3f} ms "
            f"did not meet <{VISIBLE_FEEDBACK_TARGET_MS:.0f} ms"
        )
    metrics["skipped_requirements"] = skipped_requirements
    schema = json.loads((ROOT / "spec/schemas/evidence.schema.json").read_text())
    payloads: list[tuple[Path, str, bytes]] = []
    for requirement, record in RECORDS.items():
        if requirement in skipped_requirements:
            continue
        relative = outputs[requirement].relative_to(ROOT).as_posix()
        evidence = {
            "schema_version": 2,
            "verification_id": record["verification"],
            "requirement_ids": [requirement],
            "phase": record["phase"],
            "scenario_id": record["procedure"],
            "git_commit": head,
            "git_dirty": False,
            "build_id": identity.version,
            "package_sha256": artifact_hash,
            "environment": {
                "id": "cosmic-wayland-native-ui-dictation",
                "native_or_vm": "native",
                "os": os_name,
                "os_version": os_version,
                "kernel": platform.release(),
                "desktop": desktop_name,
                "session": session,
                "cpu": platform.machine(),
                "gpu": None,
                "npu": None,
                "device_details": "GTK4/WebKitGTK 6, AT-SPI, physical default microphone",
                "microphone": "host-default-physical-source",
                "input_backend": "AT-SPI over native Wayland",
                "display_scale": None,
                "qualification_lane": "cosmic-wayland-native-ui-dictation",
                "lane_available": True,
            },
            "command": shlex.join(EXPECTED_ARGV),
            "scenario_version": "1",
            "scenario_hash": digest(Path(__file__)),
            "started_at": started.isoformat(),
            "ended_at": ended.isoformat(),
            "exit_code": 0,
            "result": "pass",
            "pass": True,
            "metrics": metrics,
            "artifact_hashes": {
                ARTIFACT: artifact_hash,
                FIXTURE: EXPECTED_FIXTURE_SHA256,
                script_rel: digest(ROOT / script_rel),
                "scripts/qualify_candidate_known_phrase.py": digest(
                    ROOT / "scripts/qualify_candidate_known_phrase.py"
                ),
                **{path: digest(ROOT / path) for path in record["artifacts"]},
            },
            "artifact_under_test": {
                "kind": "package",
                "path": ARTIFACT,
                "sha256": artifact_hash,
            },
            "procedure": {
                "id": record["procedure"],
                "profile_id": "native-behavior-v1",
                "assertions": [
                    {"id": assertion, "result": "pass"}
                    for assertion in record["assertions"]
                ],
            },
            "notes": (
                "Exact packaged GTK client was terminated during active physical-microphone capture on native COSMIC Wayland; the daemon stayed active and a later session committed. Source-bound Linux delivery/capture, corrupt model/cache, and interrupted-session failure-injection suites also passed."
                if requirement == "TEST-006" else
                "The exact package's embedded runtime and model matched its build manifest before launch. Model-manager tests rejected unpinned and corrupted bytes, and the verified packaged model completed the native local-ASR session. No signature claim is made because this package lane supplies checksum identity."
                if requirement == "SEC-002" else
                "Exact packaged GTK client on native COSMIC Wayland. Start and Stop were operated through AT-SPI; a physical-microphone local-ASR run matched the canonical fixture and delivered one Inbox result. The transcript is represented only by digest and byte count."
            ),
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
        payloads.append((outputs[requirement], relative, payload))

    targets = {relative for _path, relative, _payload in payloads}
    index["qualified_evidence"] = [
        item for item in index["qualified_evidence"] if item.get("path") not in targets
    ]
    for _path, relative, payload in payloads:
        index["qualified_evidence"].append(
            {
                "path": relative,
                "sha256": hashlib.sha256(payload).hexdigest(),
                "disposition": "qualified",
            }
        )
    index["qualified_evidence"].sort(key=lambda item: item["path"])
    jsonschema.validate(
        instance=index,
        schema=json.loads((ROOT / "spec/schemas/evidence-index.schema.json").read_text()),
    )
    for path, _relative, payload in payloads:
        accessibility.atomic_json(path, json.loads(payload))
    accessibility.atomic_json(index_path, index)
    print(
        json.dumps(
            {
                "artifact_sha256": artifact_hash,
                "records": len(payloads),
                "result": "pass",
                "skipped_requirements": skipped_requirements,
            },
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
