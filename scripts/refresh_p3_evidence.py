#!/usr/bin/env python3
"""Quarantined P3 evidence-refresh entry point.

The legacy implementation wrote schema-v1 evidence over current evidence
records (review finding P-6/B26, 2026-09-28). Git history retains it. This
module intentionally exposes no runner or writer callable. Release evidence
comes only from the reviewed scripts/record_*.py recorders.
"""
from __future__ import annotations


def main() -> int:
    print(
        "P3 evidence refresh is quarantined: it cannot produce qualification evidence. "
        "No files were changed."
    )
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
