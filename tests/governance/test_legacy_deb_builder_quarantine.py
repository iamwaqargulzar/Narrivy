from __future__ import annotations

import hashlib
import subprocess
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
BUILDER = ROOT / "scripts/build_deb.sh"
PROTECTED = (
    ROOT / "artifacts/releases/narrivy_0.2.3_amd64.deb",
    ROOT / "packaging/deb/BUILD_LOG.md",
)


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


class LegacyDebBuilderQuarantineTests(unittest.TestCase):
    def test_legacy_builder_fails_before_mutating_protected_outputs(self) -> None:
        before = {path: digest(path) for path in PROTECTED}

        result = subprocess.run(
            ["bash", str(BUILDER), "9.9.9-quarantine-probe"],
            cwd=ROOT,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
            timeout=5,
        )

        self.assertEqual(result.returncode, 2)
        self.assertEqual(result.stdout, b"")
        self.assertIn(b"legacy Debian builder is quarantined", result.stderr)
        self.assertNotIn(str(ROOT).encode(), result.stderr)
        for path, expected in before.items():
            self.assertEqual(digest(path), expected)


if __name__ == "__main__":
    unittest.main()
