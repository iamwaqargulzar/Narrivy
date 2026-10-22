#!/usr/bin/env python3
"""Quarantined legacy P6 evidence runner.

The historical runner used mock/synthetic substitutions, read the active
recovery journal through legacy History commands, directly rewrote recovery
records, and emitted schema-v1 evidence. Those behaviors cannot establish
production or release truth and conflict with ADR-0045. Historical evidence
is preserved read-only; a future replacement requires reviewed schema-v2
procedures and real production-path qualification.
"""


def main() -> int:
    print(
        "p6_runner.py is quarantined: legacy scenarios can rewrite historical "
        "evidence and do not satisfy the reviewed verification profiles. "
        "No files were changed."
    )
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
