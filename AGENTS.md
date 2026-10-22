# AGENTS.md — Authoritative Engineering Rules

## Execution

For autonomous work, `agent/completion-plan/README.md` controls the overall loop (work packages, order and stop conditions). This file defines repository-wide engineering constraints.

Before each phase:

1. read `SOURCE_OF_TRUTH.md` and `BUILD_TARGET.json`;
2. inspect `agent/STATE.json`;
3. read the phase specification and corresponding prompt;
4. run `python3 scripts/doctor.py`, then inspect the current fail-closed gate state via `python3 scripts/phase_gate.py --phase <P>`; run `python3 scripts/release_gate.py --target linux-1.0` at the release boundary. Do **not** use `scripts/run_gate.py` — it is a quarantined no-op that returns exit 2 and performs no gate; the evidence↔HEAD refresh ritual from `archive/docs/management/autonomous-build-review-2026-08-22.md` is performed by the reviewed recorder scripts, one requirement at a time, never by a legacy bulk refresh;
5. inspect open blockers/decisions/research;
6. implement and verify the active phase until its gate passes.

For work inside an already active phase, follow
`docs/maintainers/agent/token-efficient-workflow.md`. Read only the live state and the
files relevant to the current blocker; archived history is lookup material, not
startup context. Use focused checks while editing, an affected-subsystem check
once before a local checkpoint, and broad/release validation only at the
checkpoint or exact-artifact boundary. Do not rerun a known-blocked gate unless
one of its recorded prerequisites changed.

When Ponytail is available, its minimalism rules are advisory. Narrivy's
architecture, security, privacy, quality, evidence, and publishing rules remain
authoritative. The recommended default is `lite`; use its review mode on a
bounded diff and turn it off for qualification/evidence work where the mandated
procedure must be followed exactly.

## Architecture boundaries

- `crates/narrivy-core` owns domain orchestration but no OS, UI framework, ASR runtime, network provider, or device-specific implementation.
- Portable contracts live in `crates/narrivy-*-api` / portable domain crates.
- OS-specific product behavior lives in `adapters/platform-*`. Narrow,
  reusable OS mechanics may live only in the enumerated infrastructure
  adapter families documented by `docs/architecture/component-boundaries.md`
  (currently `ipc-*`, `journal-*`, and `process-*`); those
  adapters own no product policy or portable contract.
- ASR/runtime-specific implementation lives in `engines/asr-*`.
- `services/narrivyd` composes dependencies and owns long-running service lifecycle.
- `apps/narrivy-desktop` and CLI are clients; they do not duplicate policy/domain decisions.
- Privacy policy runs before data leaves its allowed boundary.
- Active-session recovery journal is separate from optional user history.
- Frontend renders capabilities; it does not use OS names as feature flags.

## API stability

Existing Phase-0 Rust traits are **scaffolds, not frozen production APIs**. P1–P5 must evolve them to support cancellation, streaming/events, typed outcomes, delivery verification, and device/model capabilities without preserving a bad signature merely for compatibility.

After an API is marked stable in `docs/architecture/api-stability.md`, breaking changes require an ADR and migration.

## Rust rules

- Rust edition 2024.
- Project toolchain is pinned in `rust-toolchain.toml`; portable MSRV is separately tested.
- Portable/core crates: `unsafe_code = forbid`.
- Isolate unavoidable FFI `unsafe` inside the specific adapter/engine crate and document invariants.
- Typed errors in production paths; no panic-driven control flow.
- No blocking operations on UI/audio real-time threads.
- Long operations expose cancellation and bounded timeouts.
- Background tasks have explicit owner, lifetime, shutdown, and failure policy.
- Treat warnings as errors in release CI.

## Security/privacy rules

- Ordinary logs never include transcript text, audio, clipboard/selection/context content, tokens, secrets, or environment dumps.
- Same-user authenticated/authorized IPC only.
- Untrusted archives/model/plugin packs must reject path traversal and verify integrity metadata.
- Secrets use an OS credential facility or a documented secure abstraction, never ordinary config.
- Extension/plugin network access is deny-by-default.
- Never disable security/dependency gates simply to make CI green.

## Quality rules

A feature is incomplete without its tests, failure UX, diagnostics, accessibility behavior, documentation, and acceptance evidence. Mock-only success cannot satisfy a requirement that claims real platform/hardware behavior.

Select the smallest applicable check tier in `scripts/README.md`; do not run
every tier after every edit. Keep `spec/requirements-traceability.csv` and
`agent/STATE.json` truthful.

## Publishing

The GitHub remote (`origin`, public and intentionally empty) must never receive a push unless the owner explicitly signals push time; that push is the repository's first version (owner also chooses squash-vs-full-history then).


## v1.3 canonical architecture

Read `docs/architecture/README.md` and `spec/catalogs/` before changing pipelines, engines, modules, runtimes, models or providers.
