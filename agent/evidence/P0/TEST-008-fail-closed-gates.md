# TEST-008 Evidence — Phase and Release Gates Fail Closed

## Verification: VER-TEST-008 (ci/architecture)
**Requirement:** Phase and release gates fail closed: missing tests/evidence/status cannot be interpreted as a pass.

## Implementation
- `scripts/phase_gate.py`:
  - Fails if acceptance spec `implemented:false` (fail-closed until real commands)
  - Fails if `required_artifacts` missing
  - Fails if acceptance `primary_requirements` != traceability required set for phase
  - Fails if any primary requirement has status != passed or evidence missing/unsafe
  - Then executes each command in `spec/acceptance/PX.json` sequentially, failing on non-zero exit
- `scripts/release_gate.py`:
  - Runs validate_pack, checks all release_gate=required rows are passed with evidence file exists inside repo
  - Checks LICENSE and THIRD_PARTY_NOTICES resolved
  - Checks P11 report contains **Status:** passed
  - Checks release evidence artifacts: qualification.json (cosmic-wayland, gnome-wayland, cpu_fallback, accessibility, privacy_security), release-artifacts.json (.deb, AppImage, sha256, checksums), summary.md
  - Prints FAILED and exits non-zero if any missing
- `scripts/validate_pack.py` etc. also fail closed on invalid JSON/TOML, mismatched phases, etc.

## Local Verification
- Before evidence: `python3 scripts/phase_gate.py --phase P0` → FAILED before commands (missing artifact, status planned) — proves fail-closed
- After P0 evidence and status passed, gate passes with all 5 primary requirements closed and 7 commands executing successfully

## Evidence Files
- `scripts/phase_gate.py`
- `scripts/release_gate.py`
- `spec/acceptance/P0.json` (implemented:true with 7 real commands)
