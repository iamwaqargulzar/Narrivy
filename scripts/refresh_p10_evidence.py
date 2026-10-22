#!/usr/bin/env python3
"""Quarantined P10 evidence-refresh entry point.

Recovery Phase 1 removed the legacy schema-v1 evidence and release-metadata
writers. Git history retains them; this module exposes no runner or writer
callable.
"""
from __future__ import annotations


def main() -> int:
    print(
        "P10 evidence refresh is quarantined by Recovery Phase 1: it cannot "
        "rewrite evidence, artifacts, or release metadata. No files were changed."
    )
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
