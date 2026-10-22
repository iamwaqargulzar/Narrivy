# Narrivy Autonomous Build — Start Here

This is the single entry point for a coding agent. Do not depend on the ChatGPT conversation that produced this pack.

## Session handoff (2026-08-22)

A full pre-build session completed: read `agent/SESSION-HANDOFF-2026-08-22.md`
first (state, owner decisions, next steps), then
`docs/13-management/gaps-improvements-register-2026-08-22.md` (product review)
and `docs/13-management/autonomous-build-review-2026-08-22.md` (build-machinery
review and the evidence discipline its recorders must preserve).

Gate state is reported by `python3 scripts/phase_gate.py --phase <P>`, which
changes nothing, and at the release boundary by
`python3 scripts/release_gate.py --target linux-1.0`. **Do not use
`scripts/run_gate.py`**: it is a quarantined no-op that exits 2 and performs no
gate. The evidence↔HEAD refresh ritual is performed one requirement at a time by
the reviewed recorder scripts, never by a legacy bulk refresh.

## Objective
Continue autonomously through `BUILD_TARGET.json.target_phase` until **Narrivy Linux 1.0** is a working installable desktop application and the release gate passes. The essential path is:

`trigger bound to Pipeline → microphone → real local ASR → deterministic/optional policy-allowed processing → reliable external text delivery → recovery/diagnostics`

## Do this now
1. Read `SOURCE_OF_TRUTH.md`.
2. Read `BUILD_TARGET.json`, `IMPLEMENTATION-PLAN-v1.3.md`, `PRODUCT_CHARTER.md`, `MASTER-AUTONOMOUS-BUILD-PROMPT.md`, `AGENTS.md`, `agent/AUTONOMY_POLICY.md`, and `agent/STATE.json`.
3. Run `python3 scripts/doctor.py` and `python3 scripts/validate_pack.py`.
4. Resume `agent/STATE.json.active_phase` (this v1.3 scaffold: P0F; P0 is historical foundation evidence).
5. Read the matching `agent/PHASES.json` spec/prompt/acceptance paths.
6. Work until that phase gate passes; update report/state and immediately continue.
7. Do not stop for normal build/test/API/platform errors; follow `agent/FAILURE-RECOVERY.md`.
8. From P2 onward, every completed working-app phase must define and pass its real `smoke_commands`; after P6 keep the daily-use dictation path runnable at every phase boundary.
9. Stop normally only when `python3 scripts/release_gate.py --target linux-1.0` exits 0.

Mocks, compilation, a launching UI, or implementation claims are not release completion.

## v1.3 non-negotiables

- Modules compose into first-class versioned Pipelines; triggers address Pipeline IDs.
- Default use stays lean and straightforward; Advanced complexity is progressive disclosure.
- Base path includes lightweight audio conditioning, deterministic correction, qualified general GPU/Vulkan path where useful and CPU fallback.
- Optional NPU/vendor runtimes, stronger audio modules and local/cloud LLM processors are acquired only when selected/qualified/policy-allowed.
- Resource identity is independent of download URL and verified before activation.
- Settings/Pipeline export excludes secrets and host-specific caches by default.
- Smart modules may fail; recoverable basic dictation may not.


## v1.3 canonical architecture

Read `docs/00-ARCHITECTURE-MAP.md` and `spec/catalogs/` before changing pipelines, engines, modules, runtimes, models or providers.
