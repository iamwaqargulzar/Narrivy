# Agent Work Order — P0F Linux Platform Feasibility

> **Scope note (2026-09-28, owner):** the first release is qualified on Pop!_OS
> COSMIC only. Mentions below of Ubuntu/GNOME as a release lane, or of AppImage,
> are superseded; see `docs/roadmap/README.md`. This phase spec is kept as the
> historical definition of the phase.

## Mission
P0F uses disposable probes to prove or explicitly limit global activation, text delivery, target identity, microphone/PipeWire capture, and Tauri packaging on Pop!_OS/COSMIC and Ubuntu/GNOME Wayland before architecture hardens.

## Required requirement IDs
_No release-gated FUN requirements (capability-gating phase)_

## Execution
1. Read `docs/maintainers/phases/phase-0F-feasibility.md`, `agent/PHASES.json`, `docs/platforms/capability-matrix.md`, and the P0F prompt.
2. Create disposable probes under `tools/p0f-probes/` for activation, delivery, target identity, mic/PipeWire, and Tauri feasibility; do not build polished product code.
3. Run probes on the first-class lanes (owner decision 2026-08-22: GNOME lane deferred to P11 — run COSMIC-native probes and record the deferral; do not simulate GNOME): collect native vs VM truth, portal availability, key-release semantics, delivery backend capabilities, microphone hotplug, suspend/resume.
4. Record per-backend capability truth (permission model, Unicode/multiline, layout dependence, clipboard impact, fallback) and update capability matrix from evidence only.
5. Emit schema-valid JSON evidence per scenario (commit/build/env/scenario-hash/timestamps/metrics) and update `agent/phase-reports/P0F.md`.
6. If parity is not proven, truthfully limit 1.0 claims, defer post-1.0, or record an owner blocker — never fake parity.
7. Run `python3 scripts/phase_gate.py --phase P0F` until it passes; P0F must pass before P1 ADRs are finalized and before broad UI/architecture is built.

## Autonomy/failure rule
Build errors, test failures and API drift are work items. Follow `agent/FAILURE-RECOVERY.md`; do not ask the owner for a technical choice the repository, host, upstream documentation or a safe experiment can resolve.

## Non-regression
Do not claim real hardware/platform support from mocks.

## Done means
The phase specification's exit evidence exists, all primary release requirements for P0F are passed with schema-valid evidence bound to commit/build, the gate exits 0, and the phase report validates its schema and captures commands/results/measurements/deviations/known non-blockers.
