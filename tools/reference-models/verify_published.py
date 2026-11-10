#!/usr/bin/env python3
"""Check the published Narrivy Store (`narrivy/store`) against `packs.json`.

Reads the public file listing of the Store at the revision recorded in
`published.json` and confirms, for each pack, that every model file is in
the pack's folder with the pinned size and SHA-256, and that the manifest
and model card are there. Read-only: it fetches public metadata and downloads no
model file.

Usage:
    python3 tools/reference-models/verify_published.py
"""

from __future__ import annotations

import json
import sys
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
API = "https://huggingface.co/api/models/{repo}/tree/{revision}?recursive=true"


def listing(repo: str, revision: str) -> dict[str, dict]:
    with urllib.request.urlopen(API.format(repo=repo, revision=revision), timeout=30) as response:
        return {entry["path"]: entry for entry in json.load(response) if entry["type"] == "file"}


def main() -> int:
    packs = json.loads((HERE / "packs.json").read_text())["packs"]
    published = json.loads((HERE / "published.json").read_text())
    repo, revision = published["repository"], published["revision"]
    if not revision:
        print(f"{repo} is not published yet: no revision in published.json")
        return 2
    remote = listing(repo, revision)
    failed = 0
    for pack in packs:
        folder = pack["store_path"]
        problems = [
            name for name in ("model-manifest.json", "README.md") if f"{folder}/{name}" not in remote
        ]
        for entry in pack["files"]:
            found = remote.get(f"{folder}/{entry['path']}")
            if found is None:
                problems.append(f"missing {entry['path']}")
            elif found["size"] != entry["size_bytes"]:
                problems.append(f"size differs {entry['path']}")
            elif "lfs" in found and found["lfs"]["oid"] != entry["sha256"]:
                problems.append(f"sha256 differs {entry['path']}")
        if problems:
            failed += 1
            print(f"FAIL  {folder}@{revision[:12]}: {'; '.join(problems[:4])}")
        else:
            hashed = sum(1 for entry in pack["files"] if "lfs" in remote[f"{folder}/{entry['path']}"])
            print(f"OK    {folder}@{revision[:12]}: {len(pack['files'])} files present, {hashed} hash-checked")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
