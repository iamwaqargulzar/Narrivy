"""Argument checks and dirty-tree refusal of the release-candidate build (T3.1).

The real build is never run here: every case stops before any build step.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "build_release_candidate.sh"


def _run(script: Path, *args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["/bin/bash", str(script), *args],
        capture_output=True,
        text=True,
        timeout=30,
        check=False,
        env={**os.environ, "NARRIVY_MODEL": "/nonexistent/model.gguf"},
    )


class ReleaseCandidateScriptTests(unittest.TestCase):
    def test_requires_exactly_one_version(self) -> None:
        for args in ((), ("1.0.0~rc1", "extra")):
            result = _run(SCRIPT, *args)
            self.assertEqual(result.returncode, 1)
            self.assertIn("usage:", result.stderr)

    def test_refuses_versions_outside_d3(self) -> None:
        for version in ("1.0.0~beta1", "1.0", "01.0.0", "1.0.0~rc0", "1.0.0+git", "v1.0.0"):
            result = _run(SCRIPT, version)
            self.assertEqual(result.returncode, 1, version)
            self.assertIn("X.Y.Z~rcN", result.stderr, version)

    def test_refuses_a_version_that_does_not_upgrade_0_2_3(self) -> None:
        result = _run(SCRIPT, "0.2.3")
        self.assertEqual(result.returncode, 1)
        self.assertIn("above 0.2.3", result.stderr)

    def test_refuses_a_dirty_tree(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary)
            git = ["git", "-C", str(repo), "-c", "user.name=t", "-c", "user.email=t@example.invalid"]
            subprocess.run([*git, "init", "-q"], check=True, timeout=10)
            (repo / "scripts").mkdir()
            copy = repo / "scripts" / SCRIPT.name
            shutil.copy2(SCRIPT, copy)
            subprocess.run([*git, "add", "."], check=True, timeout=10)
            subprocess.run([*git, "commit", "-qm", "init"], check=True, timeout=10)
            (repo / "untracked.txt").write_text("dirty\n")
            result = _run(copy, "1.0.0~rc1")
            self.assertEqual(result.returncode, 1)
            self.assertIn("not clean", result.stderr)
            # Clean again, the script passes the tree check and stops at the
            # app version, which must be the package version in semver form.
            (repo / "untracked.txt").unlink()
            conf = repo / "apps/narrivy-desktop/src-tauri/tauri.conf.json"
            conf.parent.mkdir(parents=True)
            conf.write_text('{"version": "0.2.3"}\n')
            manifests = [
                repo / "services/narrivyd/Cargo.toml",
                repo / "apps/narrivy-cli/Cargo.toml",
                repo / "apps/narrivy-desktop/src-tauri/Cargo.toml",
            ]
            for manifest in manifests:
                manifest.parent.mkdir(parents=True, exist_ok=True)
                manifest.write_text('[package]\nversion = "1.0.0-rc.1"\n')
            subprocess.run([*git, "add", "."], check=True, timeout=10)
            subprocess.run([*git, "commit", "-qm", "conf"], check=True, timeout=10)
            result = _run(copy, "1.0.0~rc1")
            self.assertEqual(result.returncode, 1)
            self.assertNotIn("not clean", result.stderr)
            self.assertIn("tauri.conf.json has version 0.2.3", result.stderr)
            self.assertIn("set every version above to 1.0.0-rc.1", result.stderr)
            self.assertNotIn("Cargo.toml has version", result.stderr)
            # With the matching version it stops at the next check (no model
            # at the given path): nothing is built.
            conf.write_text('{"version": "1.0.0-rc.1"}\n')
            subprocess.run([*git, "commit", "-qam", "match"], check=True, timeout=10)
            result = _run(copy, "1.0.0~rc1")
            self.assertEqual(result.returncode, 1)
            self.assertNotIn("has version", result.stderr)
            self.assertFalse((repo / "target").exists())


if __name__ == "__main__":
    unittest.main()
