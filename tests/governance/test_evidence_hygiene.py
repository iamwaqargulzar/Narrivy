"""Evidence hygiene checks (review findings P-2 and P-3).

P-2: a phase gate validates every record that carries a verification ID, so a
retired record next to the live one blocks the gate. Phases listed in
CLEAN_PHASES must keep exactly one live record per verification ID.

P-3: a runner pin that no longer matches its file makes every record from that
runner fail. Every pinned runner hash must match the file on disk.
"""
from __future__ import annotations

import collections
import hashlib
import json
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PROFILES = ROOT / "spec/evidence-verification-profiles.json"
# Phases whose retired duplicates have been moved to agent/history/. Add a
# phase here once its duplicates are moved.
CLEAN_PHASES = ("P10",)


def runner_pins(node: object) -> list[tuple[str, str, str]]:
    pins: list[tuple[str, str, str]] = []
    if isinstance(node, dict):
        runner = node.get("runner")
        if isinstance(runner, dict) and "sha256" in runner:
            pins.append((str(node.get("id")), runner["path"], runner["sha256"]))
        for value in node.values():
            pins.extend(runner_pins(value))
    elif isinstance(node, list):
        for value in node:
            pins.extend(runner_pins(value))
    return pins


class EvidenceHygieneTests(unittest.TestCase):
    def test_every_runner_pin_matches_its_file(self) -> None:
        pins = runner_pins(json.loads(PROFILES.read_text()))
        self.assertTrue(pins)
        for profile, rel, pinned in pins:
            with self.subTest(profile=profile, runner=rel):
                path = ROOT / rel
                self.assertTrue(path.is_file(), f"{profile}: runner {rel} is missing")
                actual = hashlib.sha256(path.read_bytes()).hexdigest()
                self.assertEqual(actual, pinned, f"{profile}: runner pin for {rel} is stale")

    def test_clean_phases_have_one_live_record_per_verification_id(self) -> None:
        for phase in CLEAN_PHASES:
            seen: dict[str, list[str]] = collections.defaultdict(list)
            for path in sorted((ROOT / "agent/evidence" / phase).glob("*.json")):
                data = json.loads(path.read_text())
                if isinstance(data, dict) and data.get("verification_id"):
                    seen[data["verification_id"]].append(path.name)
            with self.subTest(phase=phase):
                duplicates = {vid: files for vid, files in seen.items() if len(files) > 1}
                self.assertEqual(duplicates, {})


if __name__ == "__main__":
    unittest.main()
