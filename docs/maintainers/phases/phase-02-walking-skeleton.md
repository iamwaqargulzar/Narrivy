# P2 — Walking Skeleton

> **Scope note (2026-09-28, owner):** the first release is qualified on Pop!_OS
> COSMIC only. Mentions below of Ubuntu/GNOME as a release lane, or of AppImage,
> are superseded; see `docs/roadmap/README.md`. This phase spec is kept as the
> historical definition of the phase.


## Outcome
Deliver the first **real** vertical slice — intentionally plain but working on **both** first-class desktop lanes (Pop!_OS/COSMIC Wayland and Ubuntu/GNOME Wayland) with actual platform adapters and a real ASR engine:

> **trigger → microphone → real local ASR → final text → external target or Inbox fallback**

Do not wait until P6 for the first working application. Premium UI is not required here.

## Primary requirements
`FUN-001`, `FUN-004`, `FUN-009`, `FUN-010`, `FUN-013`, `REL-002`, `REL-003`, `TEST-004`.

## Required work
- Real global trigger (toggle must work; PTT only where P0F proved key-release semantics) via portal/GlobalShortcuts with capability truth (FUN-001, TEST-004).
- Real PipeWire capture (basic) without blocking UI, with cancellation and no transcript/audio leakage to ordinary logs (REL-002, REL-003).
- Real local ASR (reference engine, CPU correctness path) producing final text; streaming tentative where engine supports it.
- Capability-based text delivery router with bounded fallback: semantic/direct → transactional clipboard → portal/libei → virtual keyboard → Inbox. Verify Unicode/multiline per backend capability and deterministic target-aware spacing/punctuation (FUN-004, FUN-013).
- Recoverable final text: journal before risky delivery so crash does not lose transcript; Inbox as final sink (FUN-009).
- Cancellation must not insert stale output (FUN-010).
- Basic diagnostics and bounded support claim (evidence identifies OS/kernel/desktop/session, microphone, model/runtime/device).
- Evidence from real target applications per P0F matrix (native Wayland, browser, Electron, terminal, XWayland) on both lanes.

## Non-requirements for this phase
- Premium UI/design system (deferred to P3)
- Advanced modes/profiles/history/context (deferred to P7/P8)
- Full audio policy matrix (P4 will harden; for P2 `leave` is sufficient)

## Exit evidence
On both reference lanes (native where possible, VM supplement allowed but at least one lane native for trigger/mic/delivery): manual or `narrivy-qualify` scenario evidence (schema-valid) showing trigger→capture→ASR→delivery/Inbox with cancellation coverage. Every required verification ID has schema-valid evidence bound to commit/build. `python3 scripts/phase_gate.py --phase P2` exits 0 and the working app remains runnable (`working_app_required: true`).
## v1.3 trigger-to-pipeline rule

Walking-skeleton activation events carry a `pipeline_id`. Prove independent trigger bindings and capability-gated toggle/hold semantics (FUN-031); do not build a singleton global dictation action that must later be rewritten.
