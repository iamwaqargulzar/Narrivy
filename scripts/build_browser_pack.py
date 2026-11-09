#!/usr/bin/env python3
"""Assemble the browser runtime pack from a Chrome for Testing headless shell.

The pack is what the Store installs for the assistant's own browser: the
`chrome-headless-shell` build of Chromium that the `view_page` tool opens
pages in, out of sight and with its own empty profile. It is optional (the
tool falls back to an installed Firefox). Nothing here is built from
source; every file is copied from the given `chrome-headless-shell-linux64`
folder, and `runtime-manifest.json` lists each with its size and SHA-256.

    scripts/build_browser_pack.py \\
        --shell ~/Downloads/chrome-headless-shell-linux64 \\
        --version 153.0.8010.12 \\
        --out ~/.cache/narrivy/hf-staging

The pack is written to `<out>/store/runtimes/browser/<version>-linux-x86_64`,
its folder in the Store repository (`narrivy/store`). The printed manifest
digest and sizes go into `crates/narrivy-model-manager/seed/
published-runtimes.json`; publishing is the owner's step.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
import sys
from pathlib import Path

# The executable the daemon starts; marked executable on install.
EXECUTABLE = "chrome-headless-shell"
# Packaging lists for Debian and RPM, not used.
LEFT_OUT = {"deb.deps", "rpm.deps"}


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--shell", type=Path, required=True,
                        help="the chrome-headless-shell-linux64 folder of a Chrome for Testing release")
    parser.add_argument("--version", required=True, help="its version, such as 153.0.8010.12")
    parser.add_argument("--out", type=Path, required=True, help="staging directory")
    args = parser.parse_args()

    if not re.fullmatch(r"\d+\.\d+\.\d+\.\d+", args.version):
        raise SystemExit(f"not a Chrome version: {args.version}")
    if not (args.shell / EXECUTABLE).is_file():
        raise SystemExit(f"no {EXECUTABLE} in {args.shell}")

    store_path = f"store/runtimes/browser/{args.version}-linux-x86_64"
    pack_id = f"browser-chromium-{args.version}-linux-x86_64"
    pack = args.out / store_path
    if pack.exists():
        shutil.rmtree(pack)
    pack.mkdir(parents=True)
    origin = f"Chrome for Testing {args.version}, chrome-headless-shell-linux64"
    origins: dict[str, str] = {}
    for source in sorted(args.shell.rglob("*")):
        relative = source.relative_to(args.shell).as_posix()
        if source.is_symlink():
            raise SystemExit(f"links are not packed: {relative}")
        if not source.is_file() or relative in LEFT_OUT:
            continue
        # The Store's packs hold files at most one folder deep.
        if relative.count("/") > 1:
            raise SystemExit(f"too deep for a pack: {relative}")
        target = pack / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, target)
        target.chmod(0o644)
        origins[relative] = origin

    readme = pack / "README.md"
    readme.write_text(
        f"# Browser for the assistant: Chromium {args.version} (Linux x86_64)\n\n"
        "The headless shell of Chromium, from Google's Chrome for Testing\n"
        "releases. Narrivy's assistant opens pages in it, out of sight and with\n"
        "a fresh profile each time, to look at pages and read their console.\n"
        "Installed from Narrivy's Store; every file is checked against\n"
        "`runtime-manifest.json` before use.\n\n"
        "Chromium is BSD-3-Clause, with the third-party licences listed in\n"
        "`LICENSE.headless_shell`.\n",
        encoding="utf-8",
    )
    origins["README.md"] = "written by this script"

    files = []
    for path in sorted(p for p in pack.rglob("*") if p.is_file()):
        relative = path.relative_to(pack).as_posix()
        files.append({
            "path": relative,
            "sha256": sha256(path),
            "size_bytes": path.stat().st_size,
            "origin": origins.get(relative, "unknown"),
        })
    manifest = {
        "schema_version": 1,
        "id": f"narrivy/{pack_id}",
        "kind": "runtime",
        "runtime": "browser",
        "version": args.version,
        "platform": "linux-x86_64",
        "license": "BSD-3-Clause",
        "executable": EXECUTABLE,
        "files": files,
    }
    (pack / "runtime-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    total = sum(entry["size_bytes"] for entry in files)
    manifest_path = pack / "runtime-manifest.json"
    print(f"{pack}: {len(files)} files, {total} bytes")
    print(f"id {pack_id}")
    print(f"source_path {store_path.removeprefix('store/')}")
    print(f"manifest sha256 {sha256(manifest_path)} size {manifest_path.stat().st_size}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
