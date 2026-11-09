#!/usr/bin/env python3
"""Write the live model list the Store repository publishes (`catalog/`).

The service fetches these four files from `narrivy/store` (see
`services/narrivyd/src/daemon/catalog_feed.rs`) and checks them exactly as it
checks the copies compiled into it. This script starts from those built-in
copies (`crates/narrivy-model-manager/seed/`), adds the entries in
`packaging/store/catalog-additions.json`, and pins every addition without a
revision to `--revision`: the Store commit that holds its files.

    scripts/build_store_catalog.py --revision <40-hex commit> --out <dir>/catalog
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SEED = ROOT / "crates/narrivy-model-manager/seed"
ADDITIONS = ROOT / "packaging/store/catalog-additions.json"

# File name, list key, key in the additions file.
FILES = (
    ("registry.json", "entries", "registry"),
    ("published-packs.json", "packs", "packs"),
    ("published-language-models.json", "models", "language_models"),
    ("published-runtimes.json", "runtimes", "runtimes"),
)


def build(revision: str, additions: dict) -> dict[str, str]:
    out = {}
    for name, key, extra_key in FILES:
        document = json.loads((SEED / name).read_text(encoding="utf-8"))
        entries = document[key]
        for entry in additions.get(extra_key, []):
            entry = dict(entry)
            if entry.get("source_revision") is None:
                entry["source_revision"] = revision
            entries[:] = [e for e in entries if e.get("id") != entry["id"]]
            entries.append(entry)
        out[name] = json.dumps(document, indent=2, ensure_ascii=False) + "\n"
    return out


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--revision", required=True)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args(argv)
    if not re.fullmatch(r"[0-9a-f]{40}", args.revision):
        parser.error("--revision must be a 40-character commit id")
    additions = json.loads(ADDITIONS.read_text(encoding="utf-8"))
    if additions.get("version") != 1:
        parser.error(f"{ADDITIONS} is not version 1")
    args.out.mkdir(parents=True, exist_ok=True)
    for name, text in build(args.revision, additions).items():
        (args.out / name).write_text(text, encoding="utf-8")
        print(f"wrote {args.out / name}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
