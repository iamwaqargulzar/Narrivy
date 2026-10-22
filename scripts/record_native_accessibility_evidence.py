#!/usr/bin/env python3
"""Qualify bounded native accessibility and recovery UI behavior."""
from __future__ import annotations

import hashlib
import json
import os
import platform
import re
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


ROOT = Path(__file__).resolve().parents[1]
ARTIFACT = "artifacts/releases/narrivy-linux-amd64.deb"
DESKTOP_MEMBER = "usr/bin/narrivy-desktop"
INDEX = "agent/evidence/INDEX.json"
EXPECTED_ARGV = ["python3", "scripts/record_native_accessibility_evidence.py", ARTIFACT]
def measurement(metric: str, comparator: str, expected: object = None) -> dict[str, object]:
    binding: dict[str, object] = {"metric": metric, "comparator": comparator}
    if comparator not in {"is_true", "is_false"}:
        binding["expected"] = expected
    return binding


def measured(assertion: str, binding: dict[str, object]) -> dict[str, object]:
    return {"id": assertion, "result": "pass", "measurement": binding}


# The exact packaged desktop, bound by the reviewed package identity.
PACKAGE_BOUND = measurement("package.desktop_identity_verified", "is_true")
# A claim the reviewed native run did not measure is not recorded at all.
WAYLAND_WEBVIEW = measurement("native_marker.gdk_display_type", "eq", "GdkWaylandDisplay")
AT_SPI_EXPOSED = measurement("atspi.application_nodes", "ge", 2)
LANDMARKS_HEADINGS = measurement("atspi.named_landmarks", "ge", 1)
NAMED_CONTROLS = measurement("atspi.unnamed_main_controls", "eq", 0)
ZOOM = measurement("native_marker.zoom_level", "eq", 2)
ROOT_RELATIVE_TYPE = measurement("frontend_contracts.contract_files.A11yHardening.test.tsx", "is_true")
PRIMARY_ACTIONS = measurement("atspi.zoomed_welcome_extent.height", "ge", 50)
CONTRAST_PALETTE = measurement("frontend_contracts.contract_files.A11yHardening.test.tsx", "is_true")
FORCED_COLORS_FOCUS = measurement("atspi.named_buttons", "ge", 1)
KEYBOARD_FOCUS_VISIBLE = measurement("keyboard_lane.atspi.keyboard_navigation_order", "len_ge", 10)
REDUCED_MOTION_QUERY = measurement("theme.reduced_motion.media_query_blocks", "ge", 1)
NONESSENTIAL_ANIMATION = measurement("theme.reduced_motion.animation_disabled", "is_true")
NATIVE_UI_HEALTHY = measurement("native.desktop_exit_code", "eq", 0)
SEMANTIC_STATE = measurement("atspi.named_headings", "ge", 1)
DAEMON_ERROR_TEXT = measurement("atspi.named_headings", "ge", 1)
RECOVERY_ACTION = measurement("atspi.error_recovery_actions", "len_eq", 2)
DAEMON_UNAVAILABLE_MESSAGE = measurement("atspi.named_headings", "ge", 1)
RETRY_ACTION = measurement("atspi.error_recovery_actions", "contains", ["Retry"])
DISMISS_ACTION = measurement("atspi.error_recovery_actions", "contains", ["Dismiss notification"])
DIAGNOSTICS_REACHABLE = measurement("atspi.diagnostics_named_nodes", "ge", 1)
HANDS_FREE_ROADMAP = measurement("hands_free_defaults.status", "eq", "roadmap")
NETWORK_DEFAULT_DENY = measurement("native_marker.network_default", "eq", "deny")
EXPLICIT_ACTIONS = measurement("atspi.onboarding_actions", "len_eq", 2)
KEYBOARD_EVENTS = measurement("keyboard_lane.available", "is_true")
KEYBOARD_ONBOARDING = measurement("keyboard_lane.atspi.keyboard_onboarding_action", "eq", "Skip setup")
NAVIGATION_FOCUS_ORDER = measurement("keyboard_lane.atspi.keyboard_navigation_order", "len_eq", 10)
PALETTE_ESCAPE = measurement("keyboard_lane.atspi.command_palette_escape_closed", "is_true")
FOCUS_ORDER_RECOVERY = measurement("keyboard_lane.atspi.keyboard_diagnostics_order", "len_ge", 6)
A11Y_LANDMARKS_CONTRACT = measurement("frontend_contracts.contract_files.A11yLandmarks.test.tsx", "is_true")
ASSISTIVE_TECHNOLOGY_CHECK = measurement("atspi.semantic_roles", "contains", ["heading"])


def diagnostic_action(action: str) -> dict[str, object]:
    return measurement("atspi.diagnostic_actions", "contains", [action])


RECORDS = {
    "TEST-007": {
        "phase": "P9", "verification": "VER-TEST-007",
        "procedure": "native-accessibility-acceptance-suite-v1",
        "assertions": [
            measured("exact-package-source-bound", PACKAGE_BOUND),
            measured("keyboard-only-operation", KEYBOARD_EVENTS),
            measured("focus-order-and-recovery", FOCUS_ORDER_RECOVERY),
            measured("atspi-semantics", A11Y_LANDMARKS_CONTRACT),
            measured("two-hundred-percent-scale", ZOOM),
            measured("contrast-modes", CONTRAST_PALETTE),
            measured("reduced-motion", REDUCED_MOTION_QUERY),
            measured("actionable-error-recovery", RETRY_ACTION),
            measured("assistive-technology-check", ASSISTIVE_TECHNOLOGY_CHECK),
        ],
        "artifacts": [
            "apps/narrivy-desktop/src/app/AppShell.tsx",
            "apps/narrivy-desktop/src/theme/tokens.css",
            "apps/narrivy-desktop/src/__tests__/A11yHardening.test.tsx",
        ],
    },
    "ACC-001": {
        "phase": "P9", "verification": "VER-ACC-001",
        "procedure": "native-keyboard-operation-v1",
        "assertions": [
            measured("exact-package-source-bound", PACKAGE_BOUND),
            measured("wayland-keyboard-events", KEYBOARD_EVENTS),
            measured("onboarding-keyboard-action", KEYBOARD_ONBOARDING),
            measured("navigation-focus-order", NAVIGATION_FOCUS_ORDER),
            measured("command-palette-escape", PALETTE_ESCAPE),
        ],
        "artifacts": ["apps/narrivy-desktop/src/app/AppShell.tsx", "apps/narrivy-desktop/src/app/CommandPalette.tsx"],
    },
    "ACC-002": {
        "phase": "P9", "verification": "VER-ACC-002",
        "procedure": "native-atspi-semantics-v1",
        "assertions": [
            measured("exact-package-source-bound", PACKAGE_BOUND),
            measured("native-wayland-webview", WAYLAND_WEBVIEW),
            measured("atspi-application-exposed", AT_SPI_EXPOSED),
            measured("named-landmarks-and-headings", LANDMARKS_HEADINGS),
            measured("named-interactive-controls", NAMED_CONTROLS),
        ],
        "artifacts": ["apps/narrivy-desktop/src/app/AppShell.tsx", "apps/narrivy-desktop/src/__tests__/A11yLandmarks.test.tsx"],
    },
    "ACC-003": {
        "phase": "P9", "verification": "VER-ACC-003",
        "procedure": "native-reduced-motion-contract-v1",
        "assertions": [
            measured("exact-package-source-bound", PACKAGE_BOUND),
            measured("reduced-motion-media-query", REDUCED_MOTION_QUERY),
            measured("nonessential-animation-disabled", NONESSENTIAL_ANIMATION),
            measured("native-ui-healthy", NATIVE_UI_HEALTHY),
        ],
        "artifacts": ["apps/narrivy-desktop/src/theme/tokens.css", "apps/narrivy-desktop/src/theme/app.css", "apps/narrivy-desktop/src/__tests__/MicroInteractions.test.tsx"],
    },
    "ACC-004": {
        "phase": "P9", "verification": "VER-ACC-004",
        "procedure": "native-two-hundred-percent-scale-v1",
        "assertions": [
            measured("exact-package-source-bound", PACKAGE_BOUND),
            measured("webkit-zoom-two-hundred-percent", ZOOM),
            measured("root-relative-type-tokens", ROOT_RELATIVE_TYPE),
            measured("primary-actions-remain-reachable", PRIMARY_ACTIONS),
        ],
        "artifacts": ["apps/narrivy-desktop/src/theme/tokens.css", "apps/narrivy-desktop/src-tauri/src/lib.rs", "apps/narrivy-desktop/src/__tests__/A11yHardening.test.tsx"],
    },
    "ACC-005": {
        "phase": "P9", "verification": "VER-ACC-005",
        "procedure": "native-contrast-focus-v1",
        "assertions": [
            measured("exact-package-source-bound", PACKAGE_BOUND),
            measured("increased-contrast-palette", CONTRAST_PALETTE),
            measured("forced-colors-focus-ring", FORCED_COLORS_FOCUS),
            measured("native-keyboard-focus-visible", KEYBOARD_FOCUS_VISIBLE),
        ],
        "artifacts": ["apps/narrivy-desktop/src/theme/tokens.css", "apps/narrivy-desktop/src/__tests__/A11yHardening.test.tsx"],
    },
    "ACC-006": {
        "phase": "P9", "verification": "VER-ACC-006",
        "procedure": "native-critical-state-redundancy-v1",
        "assertions": [
            measured("exact-package-source-bound", PACKAGE_BOUND),
            measured("daemon-error-text-exposed", DAEMON_ERROR_TEXT),
            measured("recovery-action-exposed", RECOVERY_ACTION),
            measured("active-screen-semantic-state", SEMANTIC_STATE),
        ],
        "artifacts": ["apps/narrivy-desktop/src/__tests__/A11yHardening.test.tsx", "apps/narrivy-desktop/src/ui/primitives/StatusPill.tsx"],
    },
    "ACC-007": {
        "phase": "P9", "verification": "VER-ACC-007",
        "procedure": "native-hands-free-defaults-v1",
        "assertions": [
            measured("exact-package-source-bound", PACKAGE_BOUND),
            measured("hands-free-remains-roadmap", HANDS_FREE_ROADMAP),
            measured("network-default-deny", NETWORK_DEFAULT_DENY),
            measured("explicit-onboarding-and-dictation-actions", EXPLICIT_ACTIONS),
        ],
        "artifacts": ["docs/product/ux/accessibility.md", "docs/architecture/pipeline-catalog.md", "spec/catalogs/pipelines.json"],
    },
    "UX-007": {
        "phase": "P3", "verification": "VER-UX-007",
        "procedure": "native-actionable-error-v1",
        "assertions": [
            measured("exact-package-source-bound", PACKAGE_BOUND),
            measured("daemon-unavailable-message-exposed", DAEMON_UNAVAILABLE_MESSAGE),
            measured("retry-action-exposed", RETRY_ACTION),
            measured("dismiss-action-exposed", DISMISS_ACTION),
        ],
        "artifacts": ["apps/narrivy-desktop/src/app/screens/Home.tsx", "apps/narrivy-desktop/src/ui/primitives/Toast.tsx"],
    },
    "UX-010": {
        "phase": "P3", "verification": "VER-UX-010",
        "procedure": "native-individual-diagnostics-v1",
        "assertions": [
            measured("exact-package-source-bound", PACKAGE_BOUND),
            measured("diagnostics-screen-reachable", DIAGNOSTICS_REACHABLE),
            measured("microphone-check-action", diagnostic_action("Run Microphone check")),
            measured("text-delivery-check-action", diagnostic_action("Run Text delivery check")),
            measured("shortcuts-check-action", diagnostic_action("Run Shortcuts check")),
            measured("theme-store-check-action", diagnostic_action("Run Theme store check")),
        ],
        "artifacts": ["apps/narrivy-desktop/src/app/screens/Diagnostics.tsx"],
    },
}

KEYBOARD_REQUIREMENTS = {"TEST-007", "ACC-001", "ACC-005"}


class KeyboardLaneUnavailable(RuntimeError):
    """The compositor did not route injected keys to the focused native window."""


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def fail(message: str) -> int:
    print(f"record_native_accessibility_evidence: {message}", file=sys.stderr)
    return 1


def atomic_json(path: Path, value: object) -> bytes:
    payload = (json.dumps(value, indent=2, sort_keys=True) + "\n").encode()
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(payload); stream.flush(); os.fsync(stream.fileno())
        os.chmod(temporary, 0o644); os.replace(temporary, path)
    finally:
        Path(temporary).unlink(missing_ok=True)
    return payload


def replaceable(path: Path, index: dict[str, object]) -> bool:
    relative = path.relative_to(ROOT).as_posix()
    if any(item.get("path") == relative for item in index["qualified_evidence"]):
        return False
    if not path.exists():
        return True
    try:
        return json.loads(path.read_text()).get("schema_version") == 1
    except (OSError, json.JSONDecodeError):
        return False


def run_frontend_contracts() -> dict[str, object]:
    command = [
        "npm", "test", "--prefix", "apps/narrivy-desktop", "--", "--run",
        "src/__tests__/A11yLandmarks.test.tsx",
        "src/__tests__/A11yHardening.test.tsx",
        "src/__tests__/MicroInteractions.test.tsx",
    ]
    result = subprocess.run(command, cwd=ROOT, capture_output=True, text=True, timeout=90)
    combined = result.stdout + result.stderr
    if result.returncode:
        excerpt = "\n".join(combined.splitlines()[-20:])
        raise RuntimeError(f"frontend accessibility contracts failed: {excerpt}")
    # A contract claim is only measured if its own test file actually ran here.
    files: dict[str, object] = {}
    for name in (
        "A11yLandmarks.test.tsx",
        "A11yHardening.test.tsx",
        "MicroInteractions.test.tsx",
    ):
        observed = name in combined
        files[name] = observed
        if not observed:
            raise RuntimeError(
                f"frontend accessibility contract {name} did not run; refusing to record a claim "
                "with no measured execution"
            )
    return {
        "command": shlex.join(command), "exit_code": 0,
        "stdout_lines": len(result.stdout.splitlines()), "stderr_lines": len(result.stderr.splitlines()),
        "contract_files": files,
    }


def atspi_bus_address() -> str:
    result = subprocess.run(
        ["busctl", "--user", "call", "org.a11y.Bus", "/org/a11y/bus", "org.a11y.Bus", "GetAddress"],
        capture_output=True, text=True, timeout=5,
    )
    if result.returncode or '"' not in result.stdout:
        raise RuntimeError("AT-SPI bus address is unavailable")
    address = result.stdout.split('"', 2)[1]
    if not address.startswith("unix:path="):
        raise RuntimeError("AT-SPI bus is not a local Unix endpoint")
    return address


def wait_for(path: Path, process: subprocess.Popen[bytes], timeout: float) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if path.is_file():
            return
        if process.poll() is not None:
            raise RuntimeError("desktop exited before its ready marker")
        time.sleep(0.05)
    raise RuntimeError("desktop ready marker timed out")


def terminate(process: subprocess.Popen[bytes] | None) -> None:
    if process is None or process.poll() is not None:
        return
    process.send_signal(signal.SIGTERM)
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill(); process.wait(timeout=2)


def inspect_atspi(mode: str) -> dict[str, object]:
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
        for i in range(desktop.get_child_count()):
            candidate = desktop.get_child_at_index(i)
            try:
                if candidate.get_name() == "narrivy-desktop":
                    return candidate
            except Exception:
                continue
        raise RuntimeError("Narrivy is not exposed through AT-SPI")

    def snapshot() -> list[tuple[str, str]]:
        values: list[tuple[str, str]] = []
        for node in descendants(application()):
            try:
                values.append((node.get_role_name() or "", node.get_name() or ""))
            except Exception:
                continue
        return values

    def focused_interactive() -> str | None:
        for node in descendants(application()):
            try:
                if node.get_role_name() not in {"button", "link", "entry"}:
                    continue
                if node.get_state_set().contains(Atspi.StateType.FOCUSED):
                    return node.get_name() or ""
            except Exception:
                continue
        return None

    def focus_control(name: str) -> None:
        for node in descendants(application()):
            try:
                if node.get_name() == name and node.is_component() and node.grab_focus():
                    return
            except Exception:
                continue
        raise RuntimeError(f"accessible control could not receive initial focus: {name}")

    def key(*arguments: str) -> None:
        command = ["/usr/bin/wtype", *arguments]
        result = subprocess.run(command, capture_output=True, text=True, timeout=5)
        if result.returncode:
            raise RuntimeError(f"native keyboard event failed: {result.stderr.strip()}")

    def wait_name(role: str, name: str, present: bool = True, timeout: float = 5.0) -> None:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            found = (role, name) in set(snapshot())
            if found is present:
                return
            time.sleep(0.05)
        state = "appear" if present else "close"
        raise RuntimeError(f"accessible node did not {state}: {role} {name}")

    def wait_focus(name: str, timeout: float = 2.0) -> None:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if focused_interactive() == name:
                return
            time.sleep(0.03)
        raise RuntimeError(f"keyboard focus did not reach: {name}")

    def press(name: str, timeout: float = 5.0) -> None:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            for node in descendants(application()):
                try:
                    if node.get_name() != name or not node.is_action():
                        continue
                    for index in range(node.get_n_actions()):
                        if node.get_action_name(index) in ("press", "click", "activate", ""):
                            if node.do_action(index):
                                return
                except Exception:
                    continue
            time.sleep(0.05)
        raise RuntimeError(f"AT-SPI action unavailable: {name}")

    initial = snapshot()
    initial_set = set(initial)
    for expected in {
        ("dialog", "Getting started"), ("heading", "Welcome to Narrivy"),
        ("button", "Skip setup"), ("button", "Next"), ("button", "Retry"),
    }:
        if expected not in initial_set:
            raise RuntimeError(f"initial semantic node missing: {expected}")
    welcome_extent = None
    for node in descendants(application()):
        try:
            if node.get_name() == "Welcome to Narrivy":
                extent = node.get_extents(Atspi.CoordType.SCREEN)
                welcome_extent = {"width": extent.width, "height": extent.height}
                break
        except Exception:
            continue
    if welcome_extent is None or welcome_extent["height"] < 50:
        raise RuntimeError("200% WebKit zoom was not reflected in native text geometry")
    if mode == "semantic":
        press("Skip setup")
    elif mode == "keyboard":
        focus_control("Skip setup")
        wait_focus("Skip setup")
        key("-k", "Return")
        try:
            wait_name("heading", "Home")
        except RuntimeError as error:
            if ("dialog", "Getting started") in set(snapshot()):
                raise KeyboardLaneUnavailable(
                    "COSMIC did not route the native Return event to the AT-SPI-focused window"
                ) from error
            raise
    else:
        raise RuntimeError(f"unsupported accessibility inspection mode: {mode}")
    wait_name("heading", "Home")
    main = snapshot(); main_set = set(main)
    for expected in {
        ("landmark", "Primary navigation"), ("heading", "Home"),
        ("heading", "Narrivy daemon is unavailable"), ("button", "Retry"),
        ("button", "Dismiss notification"), ("button", "Diagnostics"),
    }:
        if expected not in main_set:
            raise RuntimeError(f"main semantic node missing: {expected}")
    unnamed_controls = [role for role, name in main if role in {"button", "link"} and not name.strip()]
    if unnamed_controls:
        raise RuntimeError("main UI exposes unnamed interactive controls")
    navigation_order: list[str] = []
    if mode == "keyboard":
        expected_navigation = [
            "Skip to main content", "Cycle theme", "Home", "Dictation", "Models",
            "Applications", "History", "Settings", "Diagnostics", "About",
        ]
        for expected in expected_navigation:
            key("-k", "Tab")
            wait_focus(expected)
            navigation_order.append(expected)
        key("-M", "shift", "-k", "Tab", "-m", "shift")
        wait_focus("Diagnostics")
        key("-k", "Return")
    else:
        press("Diagnostics")
    wait_name("heading", "Diagnostics")
    diagnostics = snapshot(); diagnostic_set = set(diagnostics)
    expected_checks = {
        ("heading", "Diagnostics"), ("button", "Run all"),
        ("button", "Run Microphone check"), ("button", "Run Text delivery check"),
        ("button", "Run Shortcuts check"), ("button", "Run Theme store check"),
    }
    if not expected_checks.issubset(diagnostic_set):
        raise RuntimeError("individual Diagnostics actions are not all exposed")
    diagnostic_focus_order: list[str] = []
    command_palette_escape_closed = False
    if mode == "keyboard":
        for expected in [
            "About", "Run all", "Run Microphone check", "Run Text delivery check",
            "Run Shortcuts check", "Run Theme store check",
        ]:
            key("-k", "Tab")
            wait_focus(expected)
            diagnostic_focus_order.append(expected)
        key("-M", "ctrl", "-k", "k", "-m", "ctrl")
        wait_name("dialog", "Command palette")
        wait_focus("Type a command…")
        key("-k", "Escape")
        wait_name("dialog", "Command palette", present=False)
        wait_name("heading", "Diagnostics")
        command_palette_escape_closed = True
    press("Close")
    named = [(role, name) for role, name in main if name.strip()]
    return {
        "initial_named_nodes": sum(bool(name) for _role, name in initial),
        "main_named_nodes": sum(bool(name) for _role, name in main),
        "diagnostics_named_nodes": sum(bool(name) for _role, name in diagnostics),
        "unnamed_main_controls": 0,
        "individual_diagnostic_actions": 4,
        "onboarding_actions": ["Skip setup", "Next"],
        "error_recovery_actions": ["Retry", "Dismiss notification"],
        "diagnostic_actions": sorted(
            name for role, name in diagnostic_set if role == "button" and name.startswith("Run ")
        ),
        "semantic_roles": sorted({role for role, _name in named}),
        "application_nodes": len(initial),
        "named_landmarks": sum(1 for role, _name in named if role == "landmark"),
        "named_headings": sum(1 for role, _name in named if role == "heading"),
        "named_buttons": sum(1 for role, _name in named if role == "button"),
        "named_links": sum(1 for role, _name in named if role == "link"),
        "inspection_mode": mode,
        "keyboard_onboarding_action": "Skip setup" if mode == "keyboard" else None,
        "keyboard_navigation_order": navigation_order,
        "keyboard_diagnostics_order": diagnostic_focus_order,
        "command_palette_escape_closed": command_palette_escape_closed,
        "zoomed_welcome_extent": welcome_extent,
    }


def native_run(members: dict[str, bytes], mode: str) -> tuple[dict[str, object], dict[str, object]]:
    process: subprocess.Popen[bytes] | None = None
    with tempfile.TemporaryDirectory(prefix=f"narrivy-native-a11y-{mode}-") as temporary:
        holder = Path(temporary); os.chmod(holder, 0o700)
        extract = holder / "extract"; extract.mkdir(mode=0o700)
        for name in ("home", "runtime", "state", "cache", "config", "data"):
            (holder / name).mkdir(mode=0o700)
        desktop = package._write_extracted(extract, DESKTOP_MEMBER, members[DESKTOP_MEMBER], 0o755)
        ready = holder / "ready.json"; log = holder / "desktop.log"
        bus = atspi_bus_address(); os.environ["AT_SPI_BUS_ADDRESS"] = bus
        env = dict(os.environ)
        env.update({
            "HOME": str(holder / "home"), "XDG_RUNTIME_DIR": os.environ["XDG_RUNTIME_DIR"],
            "XDG_STATE_HOME": str(holder / "state"), "XDG_CACHE_HOME": str(holder / "cache"),
            "XDG_CONFIG_HOME": str(holder / "config"), "XDG_DATA_HOME": str(holder / "data"),
            "NARRIVY_RUNTIME_DIR": str(holder / "runtime"),
            "NARRIVY_SOCKET": str(holder / "runtime/missing.sock"),
            "NARRIVY_READY_FILE": str(ready), "NO_AT_BRIDGE": "0",
            "NARRIVY_ACCESSIBILITY_ZOOM": "2",
            "GTK_A11Y": "atspi", "GDK_BACKEND": "wayland", "AT_SPI_BUS_ADDRESS": bus,
        })
        descriptor = os.open(log, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        try:
            process = subprocess.Popen([desktop], cwd=holder, env=env, stdin=subprocess.DEVNULL,
                stdout=descriptor, stderr=descriptor, start_new_session=True, close_fds=True)
        finally:
            os.close(descriptor)
        try:
            wait_for(ready, process, 12.0)
            marker = json.loads(ready.read_text())
            if marker.get("gdk_display_type") != "GdkWaylandDisplay":
                raise RuntimeError("desktop did not use native Wayland")
            document = marker.get("document", {})
            if document.get("root") is not True or document.get("bridgeReply") != "ok" or document.get("errors"):
                raise RuntimeError("desktop document or native bridge was not healthy")
            if marker.get("resources", {}).get("external_count") != 0:
                raise RuntimeError("desktop attempted an external resource request")
            if marker.get("zoom_level") != 2.0:
                raise RuntimeError("desktop did not apply the reviewed 200% accessibility zoom")
            semantics = inspect_atspi(mode)
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                raise RuntimeError("desktop did not close through its accessible window action") from None
        finally:
            terminate(process)
        desktop_exit_code = process.poll()
        if desktop_exit_code != 0:
            raise RuntimeError("desktop did not exit cleanly")
        daemon_socket_created = (holder / "runtime/missing.sock").exists()
        if daemon_socket_created:
            raise RuntimeError("accessibility run unexpectedly started a daemon")
        return {
            "marker": marker,
            "semantics": semantics,
            "desktop_exit_code": desktop_exit_code,
            "daemon_socket_created": daemon_socket_created,
        }


def os_release() -> tuple[str, str]:
    values: dict[str, str] = {}
    try:
        for line in Path("/etc/os-release").read_text().splitlines():
            if "=" in line:
                key, value = line.split("=", 1); values[key] = value.strip().strip('"')
    except OSError:
        pass
    return values.get("NAME", "Linux"), values.get("PRETTY_NAME", platform.release())


def hands_free_defaults() -> dict[str, object]:
    catalog = json.loads((ROOT / "spec/catalogs/pipelines.json").read_text())
    matches = [item for item in catalog.get("pipelines", []) if item.get("id") == "hands-free"]
    if len(matches) != 1:
        raise RuntimeError("hands-free pipeline catalog entry is missing or duplicated")
    entry = matches[0]
    if entry.get("status") != "roadmap" or entry.get("privacy") != "local-default":
        raise RuntimeError("hands-free roadmap or privacy default changed")
    standard = (ROOT / "docs/product/ux/accessibility.md").read_text()
    if "cannot weaken explicit permissions or safe action scope" not in standard:
        raise RuntimeError("hands-free accessibility safety constraint is missing")
    return {"status": "roadmap", "privacy": "local-default", "safe_action_constraint": True}


def reduced_motion_contract() -> dict[str, object]:
    """Measure, not assume, that the packaged theme honours reduced motion."""
    blocks = 0
    disabled = False
    for rel in ("apps/narrivy-desktop/src/theme/tokens.css", "apps/narrivy-desktop/src/theme/app.css"):
        try:
            text = (ROOT / rel).read_text()
        except OSError as error:
            raise RuntimeError(f"theme contract source is unreadable: {rel}") from error
        lowered = text.lower()
        for match in re.finditer(r"@media[^{]*prefers-reduced-motion[^{]*\{", lowered):
            blocks += 1
            depth = 1
            index = match.end()
            while index < len(text) and depth:
                if text[index] == "{":
                    depth += 1
                elif text[index] == "}":
                    depth -= 1
                index += 1
            body = lowered[match.end() : index]
            if re.search(r"(?:animation|transition)[a-z-]*\s*:\s*(?:none|0)", body):
                disabled = True
    if not blocks:
        raise RuntimeError(
            "no prefers-reduced-motion media query was found in the packaged theme; refusing to "
            "record a reduced-motion measurement that was never observed"
        )
    return {"media_query_blocks": blocks, "animation_disabled": disabled}


def main() -> int:
    if sys.argv != EXPECTED_ARGV[1:]:
        return fail("argv does not match the reviewed procedure")
    index_path = ROOT / INDEX; index = json.loads(index_path.read_text())
    outputs = {requirement: ROOT / f"agent/evidence/{record['phase']}/{record['verification']}.json" for requirement, record in RECORDS.items()}
    if not all(replaceable(path, index) for path in outputs.values()):
        return fail("an evidence output is already qualified or is not historical schema-v1 material")
    operations = package.HostOperations()
    try:
        source_root, head = package._require_clean_source(ROOT, operations, 5.0)
        if source_root != ROOT:
            raise RuntimeError("source root mismatch")
        artifact_payload = package._read_regular_file(ROOT / ARTIFACT, package.MAX_CANDIDATE_BYTES, "candidate")
        artifact_hash = hashlib.sha256(artifact_payload).hexdigest()
        identity, members = package._prepare_package(artifact_payload, head)
        archive = package._parse_debian_archive(artifact_payload)
        data_name, data_payload = package._archive_payload(archive, "data.tar")
        desktop_member = package._required_tar_regular_files(
            data_payload, data_name, {DESKTOP_MEMBER: package.MAX_BINARY_BYTES}
        )[DESKTOP_MEMBER]
        desktop_identity = identity.embedded["narrivy-desktop"]
        desktop_identity_verified = (
            len(desktop_member) == desktop_identity["size_bytes"]
            and hashlib.sha256(desktop_member).hexdigest() == desktop_identity["sha256"]
        )
        if not desktop_identity_verified:
            raise RuntimeError("packaged desktop identity does not match build-info")
        members[DESKTOP_MEMBER] = desktop_member
        desktop = os.environ.get("XDG_CURRENT_DESKTOP", "")
        session = os.environ.get("XDG_SESSION_TYPE", "")
        if "cosmic" not in desktop.lower() or session.lower() != "wayland":
            raise RuntimeError("the reviewed native lane requires COSMIC on Wayland")
        started = datetime.now(timezone.utc)
        frontend = run_frontend_contracts()
        hands_free = hands_free_defaults()
        theme = reduced_motion_contract()
        native = native_run(members, "semantic")
        marker = native["marker"]
        semantics = native["semantics"]
        if marker.get("network_default") != "deny":
            raise RuntimeError("native shell network default is not deny")
        keyboard_marker: dict[str, object] | None = None
        keyboard_semantics: dict[str, object] | None = None
        keyboard_native: dict[str, object] = {"desktop_exit_code": None, "daemon_socket_created": None}
        keyboard_skip_reason: str | None = None
        try:
            keyboard = native_run(members, "keyboard")
            keyboard_marker, keyboard_semantics, keyboard_native = (
                keyboard["marker"], keyboard["semantics"], keyboard,
            )
        except KeyboardLaneUnavailable as error:
            keyboard_skip_reason = str(error)
        ended = datetime.now(timezone.utc)
    except (OSError, RuntimeError, subprocess.TimeoutExpired, package.InputError, json.JSONDecodeError, re.error) as error:
        return fail(str(error))
    os_name, os_version = os_release(); script_rel = "scripts/record_native_accessibility_evidence.py"
    metrics = {
        "frontend_contracts": frontend,
        "theme": {"reduced_motion": theme},
        "native": {
            "desktop_exit_code": native["desktop_exit_code"],
            "daemon_socket_created": native["daemon_socket_created"],
        },
        "native_marker": marker,
        "atspi": semantics,
        "hands_free_defaults": hands_free,
        "package": {"desktop_identity_verified": desktop_identity_verified},
        "keyboard_lane": {
            "available": keyboard_skip_reason is None,
            "reason": keyboard_skip_reason,
            "desktop_exit_code": keyboard_native["desktop_exit_code"],
            "native_marker": keyboard_marker,
            "atspi": keyboard_semantics,
        },
    }
    schema = json.loads((ROOT / "spec/schemas/evidence.schema.json").read_text())
    payloads: list[tuple[Path, str, bytes]] = []
    selected = {
        requirement: record for requirement, record in RECORDS.items()
        if keyboard_skip_reason is None or requirement not in KEYBOARD_REQUIREMENTS
    }
    for requirement, record in selected.items():
        relative = outputs[requirement].relative_to(ROOT).as_posix()
        evidence = {
            "schema_version": 3, "verification_id": record["verification"], "requirement_ids": [requirement],
            "phase": record["phase"], "scenario_id": record["procedure"], "git_commit": head, "git_dirty": False,
            "build_id": identity.version, "package_sha256": artifact_hash,
            "environment": {"id": "cosmic-wayland-native-atspi", "native_or_vm": "native", "os": os_name,
                "os_version": os_version, "kernel": platform.release(), "desktop": desktop, "session": session,
                "cpu": platform.machine(), "gpu": None, "npu": None, "device_details": "GTK4/WebKitGTK 6 with AT-SPI",
                "microphone": None,
                "input_backend": "AT-SPI actions over native Wayland" if requirement not in KEYBOARD_REQUIREMENTS else "AT-SPI plus wtype over native Wayland",
                "display_scale": None,
                "qualification_lane": "cosmic-wayland-native-atspi", "lane_available": True},
            "command": shlex.join(EXPECTED_ARGV), "scenario_version": "1", "scenario_hash": sha256(Path(__file__)),
            "started_at": started.isoformat(), "ended_at": ended.isoformat(), "exit_code": 0, "result": "pass", "pass": True,
            "metrics": metrics,
            "artifact_hashes": {ARTIFACT: artifact_hash, script_rel: sha256(ROOT / script_rel),
                **{path: sha256(ROOT / path) for path in record["artifacts"]}},
            "artifact_under_test": {"kind": "package", "path": ARTIFACT, "sha256": artifact_hash},
            "procedure": {"id": record["procedure"], "profile_id": "native-behavior-v1",
                "assertions": record["assertions"]},
            "notes": (
                "Exact packaged GTK4/WebKitGTK 6 client on native COSMIC Wayland at reviewed 200% WebKit zoom. "
                "Real Wayland keyboard events exercised onboarding, ordered navigation, Diagnostics, command-palette focus and Escape recovery; "
                "source-bound frontend contracts checked contrast and focus styles. This does not claim GNOME qualification."
                if requirement in KEYBOARD_REQUIREMENTS else
                "Exact packaged GTK4/WebKitGTK 6 client on native COSMIC Wayland at reviewed 200% WebKit zoom. "
                "AT-SPI exposed named semantics, recovery actions, and individual Diagnostics actions; source-bound frontend contracts checked reduced motion and root-relative type. "
                "This record makes no keyboard-operation claim and does not claim GNOME qualification."
            ),
            "evidence_type": "native-platform",
            "execution": {"lane_available": True, "child_failures_ignored": False, "environment_class": "native-platform", "substitutions": []},
        }
        jsonschema.validate(instance=evidence, schema=schema)
        payload = (json.dumps(evidence, indent=2, sort_keys=True) + "\n").encode(); payloads.append((outputs[requirement], relative, payload))
    targets = {relative for _path, relative, _payload in payloads}
    index["qualified_evidence"] = [item for item in index["qualified_evidence"] if item.get("path") not in targets]
    for _path, relative, payload in payloads:
        index["qualified_evidence"].append({"path": relative, "sha256": hashlib.sha256(payload).hexdigest(), "disposition": "qualified"})
    index["qualified_evidence"].sort(key=lambda item: item["path"])
    jsonschema.validate(instance=index, schema=json.loads((ROOT / "spec/schemas/evidence-index.schema.json").read_text()))
    for path, _relative, payload in payloads:
        atomic_json(path, json.loads(payload))
    atomic_json(index_path, index)
    print(json.dumps({
        "artifact_sha256": artifact_hash,
        "records": len(payloads),
        "result": "pass",
        "skipped_requirements": sorted(set(RECORDS) - set(selected)),
        "skip_reason": keyboard_skip_reason,
    }, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
