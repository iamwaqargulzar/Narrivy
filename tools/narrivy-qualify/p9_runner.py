#!/usr/bin/env python3
"""Quarantined P9 runner.

Recovery Phase 1 removed all legacy evidence-writing functions. Git history
retains the historical implementation; this module intentionally exposes no
runner or writer callable that could produce passing qualification evidence.
"""
from __future__ import annotations


def main() -> int:
    print(
        "P9 qualification runner is quarantined by Recovery Phase 1: it cannot "
        "produce qualification evidence. No files were changed."
    )
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
