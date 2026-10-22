#!/usr/bin/env python3
"""Quarantined P10 runner.

Recovery Phase 1 removed the legacy schema-v1 evidence-writing implementation.
Git history retains it; this module intentionally exposes no runner or writer
callable that could launder the stale historical package into passing evidence.
"""
from __future__ import annotations


def main() -> int:
    print(
        "P10 qualification runner is quarantined by Recovery Phase 1: it cannot "
        "produce qualification evidence or release metadata. No files were changed."
    )
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
