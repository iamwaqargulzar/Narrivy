"""The Tauri desktop host keeps the web view locked down (ADR-0075).

The GTK4 shell served assets from a local scheme with connect-src 'none'
and no JavaScript clipboard access. The Tauri host must hold the same
line: a strict CSP that only allows Tauri's own IPC, no remote URLs, and
a minimal capability set.
"""
from __future__ import annotations

import json
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TAURI = ROOT / "apps/narrivy-desktop/src-tauri"
ALLOWED_PERMISSIONS = {"core:event:allow-listen", "core:event:allow-unlisten"}


class TauriLockdownTests(unittest.TestCase):
    def setUp(self) -> None:
        self.config = json.loads((TAURI / "tauri.conf.json").read_text())

    def test_csp_is_strict_and_allows_only_tauri_ipc(self) -> None:
        csp = self.config["app"]["security"]["csp"]
        directives = {
            part.strip().split()[0]: part.strip().split()[1:]
            for part in csp.split(";")
            if part.strip()
        }
        self.assertEqual(directives["default-src"], ["'none'"])
        self.assertEqual(directives["script-src"], ["'self'"])
        self.assertEqual(sorted(directives["connect-src"]), ["http://ipc.localhost", "ipc:"])
        for name, values in directives.items():
            for value in values:
                self.assertFalse(
                    value.startswith("https:") or value == "*" or "'unsafe-" in value,
                    f"{name} allows {value}",
                )

    def test_windows_load_only_bundled_pages(self) -> None:
        for window in self.config["app"]["windows"]:
            url = window.get("url", "index.html")
            self.assertNotIn("://", url, window["label"])
        self.assertFalse(self.config["app"].get("withGlobalTauri", False))
        self.assertNotIn("devUrl", self.config.get("build", {}))

    def test_capabilities_are_minimal(self) -> None:
        self.assertEqual(self.config["app"]["security"]["capabilities"], ["default"])
        files = sorted((TAURI / "capabilities").glob("*.json"))
        self.assertEqual([f.name for f in files], ["default.json"])
        capability = json.loads(files[0].read_text())
        self.assertEqual(set(capability["permissions"]), ALLOWED_PERMISSIONS)
        self.assertEqual(sorted(capability["windows"]), ["main", "overlay"])
        self.assertNotIn("remote", capability)


if __name__ == "__main__":
    unittest.main()
