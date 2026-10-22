"""The Tauri 2 desktop host (ADR-0075) stays a thin, complete daemon client.

Replaces test_desktop_gtk_migration.py after the GTK4 shell was retired.
The lockdown (CSP, capabilities) is covered by test_tauri_lockdown.py.
"""
from __future__ import annotations

import json
import re
import tomllib
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TAURI_ROOT = ROOT / "apps/narrivy-desktop/src-tauri"

EXPECTED_COMMANDS = {
    "daemon_status",
    "daemon_start",
    "daemon_stop",
    "daemon_cancel",
    "daemon_list_pipelines",
    "daemon_history",
    "daemon_history_entry",
    "daemon_history_delete",
    "daemon_models",
    "daemon_model_select",
    "daemon_pipeline_engine_set",
    "daemon_model_download",
    "daemon_model_download_cancel",
    "daemon_store_list",
    "daemon_store_remove",
    "daemon_assistant_send",
    "daemon_assistant_cancel",
    "daemon_assistant_library",
    "daemon_voice_mode",
    "daemon_assistant_tool_decision",
    "daemon_set_meeting_screen",
    "daemon_meeting_slide",
    "daemon_assistant_unload",
    "daemon_language_models",
    "daemon_language_model_select",
    "daemon_shortcuts_list",
    "daemon_shortcut_set",
    "daemon_shortcut_remove",
    "daemon_recovery_entry",
    "daemon_capability_check",
    "pipelines_export",
    "pipelines_import",
    "model_install_bundled",
    "model_install_cancel",
    "theme_get",
    "theme_set",
    "overlay_appearance",
    "app_autostart",
    "app_set_autostart",
    "daemon_audio_options",
    "daemon_clear_history",
    "daemon_forget_voice",
    "daemon_hold_key",
    "daemon_meeting",
    "daemon_meeting_control",
    "daemon_meeting_delete",
    "daemon_meeting_import",
    "daemon_meeting_options",
    "daemon_meeting_process",
    "daemon_meeting_rename",
    "daemon_meeting_sources",
    "daemon_meeting_start",
    "daemon_meeting_stop",
    "daemon_meeting_summarize",
    "daemon_meetings",
    "daemon_screen_start",
    "daemon_set_auto_notes",
    "daemon_set_cleanup_idle",
    "daemon_set_delivery_method",
    "daemon_set_echo_cancel",
    "daemon_set_history_retention",
    "daemon_set_hold_key",
    "daemon_set_meeting_devices",
    "daemon_set_microphone",
    "daemon_set_noise_suppression",
    "daemon_set_numbers_as_digits",
    "daemon_set_only_my_voice",
    "daemon_set_pipeline_voice",
    "daemon_set_process_on_stop",
    "daemon_set_sounds",
    "daemon_set_spoken_breaks",
    "daemon_set_vocabulary",
    "daemon_set_voice_strictness",
    "daemon_text_options",
    "daemon_train_voice",
    "daemon_vocabulary",
    "open_meeting_folder",
    "pick_folder",
}
# The frontend reads and writes the theme through its own settings layer.
EXPECTED_FRONTEND_COMMANDS = EXPECTED_COMMANDS - {"theme_get", "theme_set"}
FRAMEWORK_NEUTRAL = ("bridge.rs", "session_events.rs", "theme.rs", "overlay_state.rs", "capability_ui.rs")


class DesktopTauriHostTests(unittest.TestCase):
    def test_tauri_is_the_only_desktop_host_in_the_workspace(self) -> None:
        workspace = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]
        self.assertIn("apps/narrivy-desktop/src-tauri", workspace["members"])
        self.assertNotIn("exclude", workspace)
        for retired in ("apps/narrivy-desktop/src-gtk", "adapters/desktop-parent-linux"):
            self.assertNotIn(retired, workspace["members"])
            self.assertFalse((ROOT / retired).exists(), retired)
        manifest = tomllib.loads((TAURI_ROOT / "Cargo.toml").read_text())
        self.assertEqual(manifest["package"]["rust-version"], "1.92")
        self.assertEqual(manifest["bin"][0]["name"], "narrivy-desktop")
        self.assertIn("tauri", manifest["dependencies"])

    def test_shared_modules_stay_framework_neutral(self) -> None:
        for name in FRAMEWORK_NEUTRAL:
            source = (TAURI_ROOT / "src" / name).read_text()
            self.assertNotRegex(source, r"\btauri::", name)

    def test_every_command_is_registered_async_and_used_by_the_frontend(self) -> None:
        commands_source = (TAURI_ROOT / "src/tauri_commands.rs").read_text()
        declared = set(re.findall(r"^pub (?:async )?fn ([a-z_]+)\(", commands_source, re.MULTILINE))
        self.assertEqual(declared, EXPECTED_COMMANDS)
        # Every command runs off the UI thread: `(async)` on a plain fn, or an
        # `async fn` (which Tauri always runs on its async runtime).
        commands = re.findall(r"(#\[tauri::command[^\]]*\])\s*\npub (async )?fn", commands_source)
        self.assertEqual(len(commands), len(EXPECTED_COMMANDS))
        self.assertTrue(
            all(attr == "#[tauri::command(async)]" or is_async for attr, is_async in commands), commands
        )

        library = (TAURI_ROOT / "src/lib.rs").read_text()
        handler = library.split("generate_handler![", 1)[1].split("])", 1)[0]
        registered = set(re.findall(r"tauri_commands::([a-z_]+)", handler))
        self.assertEqual(registered, EXPECTED_COMMANDS)

        frontend = (ROOT / "apps/narrivy-desktop/src/lib/bridge.ts").read_text()
        tauri_body = frontend.split("class TauriBridge", 1)[1].split("export const bridge", 1)[0]
        used = set(re.findall(r"""invoke(?:<[^>]+>)?\(\s*['"]([a-z_]+)['"]""", tauri_body))
        self.assertEqual(used, EXPECTED_FRONTEND_COMMANDS)
        self.assertIn("new TauriBridge()", frontend)
        self.assertNotIn("WebKitBridge", frontend)

    def test_linux_overlay_never_takes_focus(self) -> None:
        source = (TAURI_ROOT / "src/linux_overlay.rs").read_text()
        for required in (
            "set_keyboard_mode(KeyboardMode::None)",
            "set_layer(Layer::Overlay)",
            "set_exclusive_zone(0)",
            "gtk_layer_shell::is_supported()",
            "monitor_at_window",
        ):
            self.assertIn(required, source)
        self.assertNotIn("present(", source)
        self.assertNotIn("grab_focus", source)

    def test_linux_overlay_runs_no_web_view(self) -> None:
        # On Linux the overlay is a native window; a configured overlay web
        # view would keep a WebKit process running for nothing.
        linux = json.loads((TAURI_ROOT / "tauri.linux.conf.json").read_text())
        self.assertEqual([window["label"] for window in linux["app"]["windows"]], ["main"])
        source = (TAURI_ROOT / "src/linux_overlay.rs").read_text()
        self.assertIn("gtk::Window::new(", source)
        self.assertNotIn("get_webview_window", source)

    def test_ci_installs_tauri_linux_packages_and_runs_this_suite(self) -> None:
        workflow = (ROOT / ".github/workflows/ci.yml").read_text()
        for required in (
            "libwebkit2gtk-4.1-dev",
            "libgtk-3-dev",
            "libgtk-layer-shell-dev",
            "npm ci --prefix apps/narrivy-desktop",
            "npm run build --prefix apps/narrivy-desktop",
            "tests.governance.test_desktop_tauri",
            "tests.governance.test_tauri_lockdown",
        ):
            self.assertIn(required, workflow)

    def test_daemon_composes_the_reviewed_portal_service_and_shutdown(self) -> None:
        source = (ROOT / "services/narrivyd/src/main.rs").read_text()
        for required in (
            "DirectZbusPortalDriver::new()",
            "PortalPermissionService::spawn(",
            "spawn_portal_event_pump(",
            "build_handler_with_portal(",
            "portal_service.shutdown_until(shutdown_deadline)",
            "join_thread_bounded(&mut portal_pump, shutdown_deadline)",
        ):
            self.assertIn(required, source)


if __name__ == "__main__":
    unittest.main()
