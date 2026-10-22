#!/usr/bin/env python3
"""Unified daemon + IPC + app harness for Narrivy qualify runners.

This module deduplicates the three previously duplicated harnesses in
p2_runner.py, p3_runner.py and p6_runner.py. It provides a single
``Harness`` class that covers:

* private ``XDG_RUNTIME_DIR`` and ``XDG_STATE_HOME`` tempdir lifecycle
* daemon spawn with ``NARRIVY_*`` environment propagation
* optional Tauri shell (app) spawn with filtered real display env
* length-prefixed JSON IPC client with broadcast-event buffering
  (``request``/``next_event``)

The implementation is intentionally conservative: behavior is kept
identical to the three originals, including the stale-socket probe
introduced in p6 and the non-focus-stealing overlay checks in p3.
"""
from __future__ import annotations

import glob
import json
import os
import re
import shutil
import socket
import struct
import subprocess
import tempfile
import time
from pathlib import Path

# Resolve repo root relative to this file (tools/narrivy-qualify/harness.py -> repo root)
ROOT = Path(__file__).resolve().parents[2]
DEFAULT_DAEMON_BIN = ROOT / "target/debug/narrivyd"
DEFAULT_CLI_BIN = ROOT / "target/debug/narrivy-cli"
DEFAULT_APP_BIN = ROOT / "target/debug/narrivy-desktop"

SOCKET_WAIT_S = 15.0
READY_WAIT_S = 40.0
IDLE_WAIT_S = 10.0  # default; callers pass their own budget where needed


class IpcError(Exception):
    pass


class IpcClient:
    """Minimal blocking client: length-prefixed JSON frames + handshake.

    Broadcast ``event`` frames that arrive while a ``request`` is outstanding
    are buffered in ``_pending_events`` so early transitions (preparing/
    listening) are never lost — the fix originally landed in p6_runner.
    """

    def __init__(self, socket_path: Path, role: str = "narrivy-qualify") -> None:
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.settimeout(10.0)
        self.sock.connect(str(socket_path))
        hello = {
            "protocol_major": 1,
            "protocol_minor": 1,
            "build_identity": "narrivy-qualify-harness",
            "client_role": role[:64],
        }
        self._send(hello)
        resp = self._recv()
        if "Accepted" not in resp:
            raise IpcError(f"handshake rejected: {resp}")
        self.daemon_build = resp["Accepted"].get("daemon_build", "?")
        self.protocol_minor = resp["Accepted"].get("protocol_minor", 0)
        self.next_request_id = 1
        self._pending_events: list[dict] = []

    def _send(self, obj: dict) -> None:
        data = json.dumps(obj).encode()
        self.sock.sendall(struct.pack("<I", len(data)) + data)

    def _recv(self) -> dict:
        prefix = b""
        while len(prefix) < 4:
            chunk = self.sock.recv(4 - len(prefix))
            if not chunk:
                raise IpcError("socket closed mid-frame")
            prefix += chunk
        (length,) = struct.unpack("<I", prefix)
        if length > 1_048_576:
            raise IpcError(f"oversized frame {length}")
        buf = b""
        while len(buf) < length:
            chunk = self.sock.recv(length - len(buf))
            if not chunk:
                raise IpcError("socket closed mid-frame")
            buf += chunk
        return json.loads(buf)

    def request(self, kind: str, payload: dict, timeout: float = 10.0) -> dict:
        rid = self.next_request_id
        self.next_request_id += 1
        envelope = {
            "version_major": 1,
            "version_minor": self.protocol_minor or 1,
            "kind": kind,
            "request_id": rid,
            "payload": payload,
        }
        self.sock.settimeout(timeout)
        self._send(envelope)
        while True:
            frame = self._recv()
            if frame.get("kind") == "response":
                if frame.get("request_id") != rid:
                    continue
                payload_out = frame.get("payload", {})
                result_tag = payload_out.get("result")
                inner = payload_out.get("payload")
                if result_tag == "rejected":
                    raise IpcError(f"rejected code={inner.get('code')} detail={inner.get('detail')}")
                return {"result": result_tag, "payload": inner}
            if frame.get("kind") == "event":
                self._pending_events.append(frame.get("payload", {}))
                continue

    def next_event(self, timeout: float = 5.0) -> dict | None:
        if self._pending_events:
            return self._pending_events.pop(0)
        old_timeout = self.sock.gettimeout()
        self.sock.settimeout(timeout)
        try:
            frame = self._recv()
        except socket.timeout:
            return None
        finally:
            self.sock.settimeout(old_timeout)
        if frame.get("kind") != "event":
            return None
        return frame.get("payload", {})

    def drain_events(self, seconds: float) -> list[dict]:
        events: list[dict] = []
        deadline = time.monotonic() + seconds
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                break
            ev = self.next_event(timeout=min(remaining, 0.5))
            if ev:
                events.append(ev)
        return events

    def close(self) -> None:
        try:
            self.sock.close()
        except OSError:
            pass


class Harness:
    """Unified harness for daemon-only and daemon+app (Tauri) modes.

    Parameters
    ----------
    extra_env:
        Additional ``NARRIVY_*`` overrides passed to the daemon (and, for
        app mode, to the app as well).
    daemon_bin:
        Path to ``narrivyd`` binary. Defaults to ``target/debug/narrivyd``.
    runtime_dir:
        Existing ``XDG_RUNTIME_DIR`` to reuse (p6 journal-recovery second
        daemon). Its private qualification state home is retained beneath it
        so durable journal state survives those daemon restarts. When
        ``None`` a fresh tempdir is created.
    app_bin:
        Path to the Tauri shell binary. When provided the harness also
        spawns the app after the daemon becomes reachable and waits for the
        readiness marker — the Stack behaviour from p3.
    theme_file:
        Override for ``NARRIVY_THEME_FILE``. When ``None`` and app mode is
        active, a file under the temp XDG is used.
    ready_file:
        Override for ``NARRIVY_READY_FILE``. Defaults to ``<xdg>/ready.json``.
    cli_bin:
        Path to ``narrivy-cli`` for ``cli()`` helper.
    socket_wait_s, ready_wait_s:
        Timeouts for socket reachability and readiness marker.
    prefix:
        Tempdir prefix when creating a fresh XDG tree.
    """

    def __init__(
        self,
        extra_env: dict[str, str] | None = None,
        daemon_bin: Path | None = None,
        runtime_dir: Path | None = None,
        *,
        app_bin: Path | None = None,
        theme_file: Path | None = None,
        ready_file: Path | None = None,
        cli_bin: Path | None = None,
        socket_wait_s: float = SOCKET_WAIT_S,
        ready_wait_s: float = READY_WAIT_S,
        prefix: str | None = None,
    ) -> None:
        self.extra_env = extra_env or {}
        self.daemon_bin = Path(daemon_bin) if daemon_bin else DEFAULT_DAEMON_BIN
        self.cli_bin = Path(cli_bin) if cli_bin else DEFAULT_CLI_BIN
        self.socket_wait_s = socket_wait_s
        self.ready_wait_s = ready_wait_s

        # App-mode detection: p3's Stack always spawns an app.
        # Callers that want daemon-only leave app_bin None.
        # For convenience, if prefix indicates p3 but no app_bin passed,
        # default to the standard desktop binary when it exists.
        self.app_bin: Path | None = Path(app_bin) if app_bin else None
        self._with_app = self.app_bin is not None

        # Tempdir / XDG layout.
        # When runtime_dir is supplied we reuse it (journal recovery).
        # Otherwise we create a fresh holder.
        self._tmp: str | None = None
        self._holder_prefix = prefix  # remember for diagnostics
        if runtime_dir is not None:
            self.runtime_dir = Path(runtime_dir)
            # Ensure parent dirs exist for file creation inside enter.
            self.runtime_dir.mkdir(parents=True, exist_ok=True)
        else:
            # Choose a descriptive prefix per caller when not specified.
            if prefix is None:
                prefix = "p3-shell-" if self._with_app else "narrivy-qualify-"
            self._tmp = tempfile.mkdtemp(prefix=prefix)
            self.runtime_dir = Path(self._tmp) / "xdg"
            self.runtime_dir.mkdir(parents=True)

        # Alias for p3 compatibility (Stack exposed .xdg)
        self.xdg = self.runtime_dir

        # Keep durable recovery state private and isolated from the invoking
        # user's real XDG state. Placing it beneath the qualification runtime
        # also preserves it when callers intentionally reuse runtime_dir for a
        # crash/restart scenario, while the harness owner still removes the
        # complete tree at the end of the scenario.
        self.state_home = self.runtime_dir / ".xdg-state"
        if self.state_home.is_symlink():
            raise RuntimeError(f"qualification state home is a symlink: {self.state_home}")
        self.state_home.mkdir(mode=0o700, parents=True, exist_ok=True)
        self.state_home.chmod(0o700)

        # Common socket / durable journal layout.
        self.socket = self.runtime_dir / "narrivy" / "narrivyd.sock"
        self.journal_dir = self.state_home / "narrivy" / "journal"
        self.timings_dir = self.runtime_dir / "narrivy" / "timings"
        self.inbox_dir = self.runtime_dir / "narrivy" / "inbox"

        # App-mode extras
        if self._with_app:
            # theme_file may be externally shared (p3 theme-switch sequence)
            if theme_file is not None:
                self.theme_file = Path(theme_file)
            else:
                self.theme_file = self.xdg / "theme.json"
            if ready_file is not None:
                self.ready = Path(ready_file)
            else:
                self.ready = self.xdg / "ready.json"
            # Snapshot of host env minus harness-controlled keys so the app
            # keeps the real Wayland/GTK display.
            self.app_env = {
                k: v
                for k, v in os.environ.items()
                if k not in ("NARRIVY_READY_FILE", "NARRIVY_THEME_FILE", "NARRIVY_SOCKET")
            }
            self.app: subprocess.Popen | None = None
        else:
            # Still expose theme_file/ready for uniform attribute access;
            # default under xdg.
            self.theme_file = Path(theme_file) if theme_file else self.xdg / "theme.json"
            self.ready = Path(ready_file) if ready_file else self.xdg / "ready.json"
            self.app = None  # type: ignore[assignment]

        self.daemon: subprocess.Popen | None = None
        # Legacy name expected by some callers: proc == daemon process
        self.proc: subprocess.Popen | None = None
        self.logs: list[str] = []

    # ------------------------------------------------------------------
    # Environment helpers
    # ------------------------------------------------------------------

    def _daemon_env(self) -> dict[str, str]:
        # p6/p2: isolated XDG roots + NARRIVY_* overrides + NARRIVY_SOCKET
        # p3 also sets NARRIVY_SOCKET and NARRIVY_THEME_FILE
        env = dict(os.environ, **self.extra_env)
        # Harness isolation is authoritative even if the parent environment
        # exports a real user state home or a caller accidentally supplies one.
        env["XDG_RUNTIME_DIR"] = str(self.runtime_dir)
        env["XDG_STATE_HOME"] = str(self.state_home)
        # Ensure daemon binds to our temp socket even if it prefers NARRIVY_SOCKET.
        env.setdefault("NARRIVY_SOCKET", str(self.socket))
        if self._with_app:
            env["NARRIVY_THEME_FILE"] = str(self.theme_file)
        return env

    def _app_env(self) -> dict[str, str]:
        assert self._with_app and self.app_bin is not None
        return dict(
            self.app_env,
            **self.extra_env,
            NARRIVY_SOCKET=str(self.socket),
            NARRIVY_READY_FILE=str(self.ready),
            NARRIVY_THEME_FILE=str(self.theme_file),
        )

    # ------------------------------------------------------------------
    # Lifecycle
    # ------------------------------------------------------------------

    def __enter__(self) -> "Harness":
        # Spawn daemon
        env = self._daemon_env()
        # For p3's old Stack, daemon stdout/stderr were DEVNULL; for p2/p6 they were PIPE.
        # Keep PIPE for daemon so logs are capturable; for app mode we still capture daemon via PIPE
        # but suppress app noise similarly? P3 app used PIPE for app, DEVNULL for daemon.
        # Using PIPE for both is compatible (p3 didn't read daemon logs).
        self.daemon = subprocess.Popen(
            [str(self.daemon_bin)],
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        self.proc = self.daemon

        # Wait until daemon is actually reachable (handles stale socket after SIGKILL).
        deadline = time.monotonic() + self.socket_wait_s
        last_err: Exception | None = None
        while time.monotonic() < deadline:
            # Try a real connect probe; falls back to existence check implicitly.
            try:
                if self.socket.exists():
                    probe = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                    probe.settimeout(0.5)
                    try:
                        probe.connect(str(self.socket))
                        probe.close()
                        break
                    except OSError as exc:
                        last_err = exc
                        try:
                            probe.close()
                        except OSError:
                            pass
                else:
                    last_err = FileNotFoundError(str(self.socket))
            except OSError as exc:
                last_err = exc
            if self.daemon.poll() is not None:
                # Surface early exit with logs if available
                stderr = ""
                try:
                    if self.daemon.stderr:
                        # non-blocking read attempt
                        pass
                except Exception:
                    pass
                raise RuntimeError(f"daemon exited early rc={self.daemon.returncode}")
            time.sleep(0.02)
        else:
            raise RuntimeError(f"daemon never became reachable at {self.socket}: {last_err}")

        if not self._with_app:
            return self

        # App mode: spawn Tauri shell after daemon is up
        assert self.app_bin is not None
        self.app = subprocess.Popen(
            [str(self.app_bin)],
            env=self._app_env(),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        deadline = time.monotonic() + self.ready_wait_s
        while time.monotonic() < deadline:
            if self.ready.exists():
                return self
            if self.app.poll() is not None:
                # Include app stderr for diagnostics
                err = ""
                try:
                    if self.app.stderr:
                        err = self.app.stderr.read()  # may block; keep short
                except Exception:
                    pass
                raise RuntimeError(f"app exited early rc={self.app.returncode} {err[:500]}")
            if self.daemon.poll() is not None:
                raise RuntimeError(f"daemon exited early while waiting for app rc={self.daemon.returncode}")
            time.sleep(0.1)
        raise RuntimeError("readiness marker never appeared")

    def marker(self) -> dict:
        """Read and parse the readiness marker (app mode)."""
        return json.loads(self.ready.read_text())

    # ------------------------------------------------------------------
    # IPC / CLI helpers
    # ------------------------------------------------------------------

    def cli(self, *args: str, timeout: float = 10.0) -> subprocess.CompletedProcess:
        proc = subprocess.run(
            [str(self.cli_bin), "--socket", str(self.socket), *args],
            capture_output=True,
            text=True,
            timeout=timeout,
        )
        self.logs.append(proc.stdout or "")
        self.logs.append(proc.stderr or "")
        return proc

    def connect(self, role: str = "narrivy-qualify") -> IpcClient:
        return IpcClient(self.socket, role=role)

    # Compatibility alias for older code using daemon.connect(role)
    # Already covered via connect() above.

    def status_active_state(self, proc: subprocess.CompletedProcess) -> tuple[bool, str | None]:
        out = proc.stdout or ""
        m = re.search(r"active session_id=\S+ state=(\w+)", out)
        return ("no active session" not in out), (m.group(1) if m else None)

    def wait_idle(self, budget: float = IDLE_WAIT_S) -> list[str]:
        states: list[str] = []
        deadline = time.monotonic() + budget
        while time.monotonic() < deadline:
            proc = self.cli("status")
            active, state = self.status_active_state(proc)
            if state:
                states.append(state)
            if not active:
                return states
            time.sleep(0.05)
        raise RuntimeError(f"daemon never went idle within {budget}s; last states {states}")

    def journal_entries(self) -> list[dict]:
        entries = []
        for f in sorted(glob.glob(str(self.journal_dir / "*.json"))):
            entries.append(json.loads(Path(f).read_text()))
        return entries

    def delivered_texts(self) -> list[str]:
        return [
            e["variants"]["delivered"]
            for e in self.journal_entries()
            if e.get("variants", {}).get("delivered") is not None
        ]

    def timing_records(self) -> list[dict]:
        records: list[dict] = []
        if self.timings_dir.is_dir():
            for f in sorted(self.timings_dir.glob("*.json")):
                records.append(json.loads(f.read_text()))
        return records

    # ------------------------------------------------------------------
    # Shutdown helpers
    # ------------------------------------------------------------------

    def kill(self) -> None:
        """SIGKILL — the crash-window primitive (never graceful terminate)."""
        target = self.daemon or self.proc
        if target and target.poll() is None:
            target.kill()
            try:
                target.wait(timeout=10)
            except subprocess.TimeoutExpired:
                pass

    def stop(self) -> None:
        # If we are attached to a pre-existing runtime_dir (journal recovery),
        # only stop processes; never delete the holder.
        if self._tmp is None:
            for proc in (self.app, self.daemon):
                if proc and proc.poll() is None:
                    proc.terminate()
                    try:
                        proc.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        proc.kill()
                        try:
                            proc.wait(timeout=5)
                        except Exception:
                            pass
            return

        # Fresh tempdir owner: terminate both and remove holder
        for proc in (self.app, self.daemon):
            if proc and proc.poll() is None:
                proc.terminate()
                try:
                    proc.wait(timeout=5 if proc is self.app else 10)
                except subprocess.TimeoutExpired:
                    proc.kill()
                    try:
                        proc.wait(timeout=5)
                    except Exception:
                        pass
        # Collect daemon stderr for log hygiene checks (p2/p6)
        target = self.daemon or self.proc
        if target and target.stderr:
            try:
                # Use non-blocking read; stderr may already be closed
                data = target.stderr.read()
                if data:
                    self.logs.append(data)
            except Exception:
                pass
        try:
            shutil.rmtree(self._tmp, ignore_errors=True)
        except Exception:
            pass

    def __exit__(self, *exc) -> None:
        self.stop()


# Backwards-compat aliases — old runners imported ``Daemon``/``Stack``.
Daemon = Harness
Stack = Harness
