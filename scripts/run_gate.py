#!/usr/bin/env python3
"""Quarantined legacy evidence-refresh entry point.

Recovery Phase 1 prohibits this helper from generating evidence or resealing the
manifest. Use the individual, reviewed qualification procedure only after its
runner has been upgraded to evidence schema v2; use phase_gate.py to inspect
the current fail-closed result.
"""
from __future__ import annotations

import argparse


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--phase", required=True)
    args = parser.parse_args()
    print(
        f"run_gate.py is quarantined for {args.phase}: legacy runners can rewrite evidence "
        "and are not permitted to establish qualification. No files were changed."
    )
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
