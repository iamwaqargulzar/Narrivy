#!/usr/bin/env python3
"""Check that relative links in the repository's Markdown resolve.

Scope: every tracked *.md file except vendored third-party sources, which keep
their upstream links, and historical records. Exit 1 if any link is
broken. External links (scheme:) and pure anchors are not checked.
"""
from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SKIP = ("vendor/", "archive/", "agent/evidence/")
LINK = re.compile(r"\]\(([^)\s]+)\)")


def main() -> int:
    files = subprocess.check_output(["git", "ls-files", "*.md"], cwd=ROOT, text=True).split()
    broken = []
    for rel in files:
        if rel.startswith(SKIP):
            continue
        text = (ROOT / rel).read_text(errors="replace")
        text = re.sub(r"```.*?```", "", text, flags=re.S)
        for target in LINK.findall(text):
            if re.match(r"^[a-z][a-z0-9+.-]*:", target) or target.startswith("#"):
                continue
            path = target.split("#", 1)[0]
            if not path:
                continue
            resolved = (ROOT / rel).parent / path
            if not resolved.exists():
                broken.append(f"{rel}: {target}")
    for line in broken:
        print(line)
    print(f"docs links: {len(broken)} broken" if broken else "docs links OK")
    return 1 if broken else 0


if __name__ == "__main__":
    sys.exit(main())
