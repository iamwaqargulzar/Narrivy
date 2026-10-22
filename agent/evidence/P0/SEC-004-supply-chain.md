# SEC-004 Evidence — Supply-chain and Dependency-license Checks are Release Gates

## Verification: VER-SEC-004 (ci/architecture)
**Requirement:** Supply-chain and dependency-license checks are release gates.

## Implementation
- `deny.toml` configured with:
  - [graph] all-features = true
  - [licenses] allow list: Apache-2.0, LLVM-exception, BSD-2-Clause, BSD-3-Clause, ISC, MIT, MPL-2.0, Unicode-3.0, Zlib; confidence-threshold 0.90; private ignore true
  - [bans] multiple-versions warn, wildcards deny with allow-wildcard-paths true, highlight all
  - [sources] unknown-registry deny, unknown-git deny
- `deny.toml` enforced via CI job `supply-chain` in `.github/workflows/ci.yml`:
  - installs pinned toolchain 1.97.1
  - runs `cargo install cargo-deny --locked` and `cargo deny check`
- Local run: `cargo deny check` → advisories ok, bans ok, licenses ok, sources ok (2026-08-21)
- Traceability: SEC-004 primary_phase P0, release_gate required, status passed

## Evidence Files
- `deny.toml`
- `.github/workflows/ci.yml` (supply-chain job)
- `cargo deny check` output log (captured in phase report)

## Gate
- `cargo deny check` exits 0
