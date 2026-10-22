#!/usr/bin/env python3
"""Quarantined P9 evidence-refresh entry point.

Recovery Phase 1 removed the legacy evidence-writing implementation. Git
history retains it; this module exposes no runner or writer callable.
"""
from __future__ import annotations


def main() -> int:
    print(
        "P9 evidence refresh is quarantined by Recovery Phase 1: it cannot "
        "produce qualification evidence. No files were changed."
    )
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
