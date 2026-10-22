# P0 — Foundation & Autonomous Control Plane

## Outcome
Create a deterministic repository an autonomous agent can build, test, diagnose and resume without conversational memory. No Linux feasibility is claimed here.

## Primary requirements
`SEC-004`, `TEST-001`, `TEST-002`, `TEST-008`.

`PORT-001` (Windows/macOS compile proof) moved to post-1.0 roadmap phase R13 by owner decision 2026-09-28; see `agent/DECISIONS.md`.

## Required work
- Pin toolchain via `rust-toolchain.toml` (1.97.1) and keep portable MSRV (1.85) separately tested; `Cargo.lock` is committed and required for release builds.
- Make portable crates compile on native Linux/Windows/macOS CI runners (compile portability, not feature support).
- Enforce architecture with both source-boundary and `cargo metadata` dependency-graph checks.
- Keep GitHub Actions immutable-pinned; run fmt/check/test/clippy, traceability/schema/manifest/owner-gate checks, `cargo-deny` (pinned), and portable CI.
- **Validate all normative schemas/examples with real JSON Schema validation** (build-target, UI tokens, mode/profile/model-manifest/privacy-policy/plugin-manifest, phase-report, evidence, qualification, release-artifacts, owner-gate).
- Make `doctor` non-destructive and actionable; it may install only explicitly safe/dev-local prerequisites, never mutate system policy unexpectedly. Doctor must support `--json` and is extended progressively per Part N.
- Ensure `SOURCE_OF_TRUTH.md`, `BUILD_TARGET.json`, dynamic state, phase reports, metrics, failure recovery and gates are coherent; agent state uses atomic write+fsync+rename with validation.
- Define update-policy decision explicitly (no unsigned auto-updater for 1.0 unless signed design exists) — see `docs/maintainers/operations/update-policy.md`.
- Define bounded support lanes (exact OS/desktop versions qualified, not open-ended “+” version ranges) — see `docs/platforms/bounded-support.md`.
- Define reproducible builder/package plan with immutable builder baselines — see `docs/maintainers/operations/reproducible-builder.md`.
- Define resource/network policy for the autonomous agent — see `docs/maintainers/agent/resource-network-policy.md`.
- Define early legal milestone/blocker before RC — see `agent/BLOCKERS.md` (LICENSE/THIRD_PARTY_NOTICES).
- Establish owner-gate minima under `spec/gates/owner/` with baseline manifest/hash and integrity validator `scripts/verify_owner_gates.py`.
- Establish JSON Schemas for evidence and phase-report under `spec/schemas/`.
- Establish qualification runner scaffold `tools/narrivy-qualify`.

## Exit evidence
P0 report includes toolchain/host facts, CI/gate outputs, generated lockfile, schema validation transcript, and owner-gate baseline hash. `python3 scripts/phase_gate.py --phase P0` exits 0 and `python3 scripts/validate_pack.py` passes with schema checks.

## Prohibited shortcuts
Floating toolchain for release builds; unpinned Actions; fake lockfile; guessed license; warnings/tests suppressed to make CI green; a gate that converts missing evidence into success; empty placeholder acceptance specs.
