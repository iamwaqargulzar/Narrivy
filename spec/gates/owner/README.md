# Owner Gates

This directory contains the **minimum non-relaxable acceptance contract** for every phase, including P0F, per the legacy planning amendment Part E1.

- Files are controlled by the owner-gate contract. **Do not remove, weaken, skip, or rewrite minimum checks to make implementation pass.**
- You may add *stricter* checks.
- If a minimum is genuinely impossible or contradictory, record a blocker/ADR and identify the exact owner decision required — do not silently change the gate.
- `BASELINE.json` records SHA-256 of each owner-gate file; `scripts/verify_owner_gates.py` validates integrity.
- Modification of the owner baseline is an owner-level action, not a routine implementation action.
