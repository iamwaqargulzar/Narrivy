#!/usr/bin/env python3
"""Compile check of the portable crates for Windows (PORT-001, SQ2 step 1).

Runs `cargo check --locked --target x86_64-pc-windows-gnu --all-features
--all-targets` over the portable/core packages listed in
`scripts/boundary_policy.py`, with warnings as errors.

No Windows C toolchain is needed: `rusqlite` bundles SQLite's C source, and
`cargo check` never links, so a stand-in C compiler that writes empty objects
lets the Rust code be type-checked for Windows. This proves the Rust side
only; building and running on Windows is SQ2's later work.

Prerequisite: `rustup target add x86_64-pc-windows-gnu`.
Exit status: 0 when every package checks clean, else cargo's.
"""

from __future__ import annotations

import os
import stat
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))

from boundary_policy import PROTECTED_PACKAGES  # noqa: E402

TARGET = "x86_64-pc-windows-gnu"

# Writes an empty object for `-o PATH` (gcc style) or `-FoPATH` (the cc crate
# falls back to MSVC-style flags when it cannot identify the compiler).
STUB_CC = """#!/bin/sh
out=""
while [ $# -gt 0 ]; do
  case "$1" in
    -o) out="$2"; shift ;;
    -Fo*) out="${1#-Fo}" ;;
  esac
  shift
done
[ -n "$out" ] && : > "$out"
exit 0
"""


def main() -> int:
    installed = subprocess.run(
        ["rustup", "target", "list", "--installed"],
        capture_output=True,
        text=True,
        check=False,
    ).stdout.split()
    if TARGET not in installed:
        print(f"check_windows_portable: run `rustup target add {TARGET}` first", file=sys.stderr)
        return 2
    with tempfile.TemporaryDirectory(prefix="narrivy-wincheck-") as tmp:
        stub = Path(tmp) / "stub-cc"
        stub.write_text(STUB_CC)
        stub.chmod(stub.stat().st_mode | stat.S_IXUSR)
        env = dict(os.environ)
        key = TARGET.replace("-", "_")
        env[f"CC_{key}"] = str(stub)
        env[f"AR_{key}"] = "ar"
        env["RUSTFLAGS"] = (env.get("RUSTFLAGS", "") + " -D warnings").strip()
        command = ["cargo", "check", "--locked", "--target", TARGET, "--all-features", "--all-targets"]
        for package in sorted(PROTECTED_PACKAGES):
            command += ["-p", package]
        result = subprocess.run(command, cwd=ROOT, env=env, check=False)
    if result.returncode == 0:
        print(f"check_windows_portable: {len(PROTECTED_PACKAGES)} portable packages check clean for {TARGET}")
    return result.returncode


if __name__ == "__main__":
    sys.exit(main())
