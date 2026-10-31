# P7 — Core Modes & Simple Personalization Only

## Outcome
Linux 1.0 should prioritize **Raw and Clean** modes. Simple profiles/settings that do not depend on unreliable Wayland target introspection may remain. Advanced app-context-driven profiles are capability-dependent or moved post-1.0 unless P0F/P3 has proven them on both reference desktops.

## Primary requirements (current scope)
`FUN-021`, `FUN-027`, `UX-009`.

Deferred per Part D (now R12/R14, not 1.0 blockers): `FUN-023` (advanced App Profiles), `FUN-024` (selected-text replace+undo), `FUN-025` (smart correction grammar), `FUN-026` (confidence-directed repair), `FUN-040` advanced dictionary scoping, `FUN-042`/`FUN-043` (correction memory/decay), `FUN-044` (snippets), `FUN-045` (shareable pack), `FUN-060`–`FUN-063` (broad Context Fabric/project indexing) — see traceability post-1.0.

## Required work
- Built-in **Raw** and **Clean** modes as declarative pipelines (FUN-021, modes are pipelines not UI presets per P1).
- Deterministic text processing preferred over LLM for simple formatting/rules (FUN-027).
- Destructive AI transformations have one-step undo where they exist (UX-009).
- Simple personalization that is low-risk and not dependent on Wayland selection/surrounding-text introspection may ship; otherwise capability-gated or hidden.
- Do not expose deferred screens as if complete; label experimental or hide.

## Exit evidence
Raw and Clean work end-to-end on reference environments; no deferred feature is presented as complete in UI. Schema-valid evidence for P7 verification IDs. `python3 scripts/phase_gate.py --phase P7` exits 0.
## v1.3 first-class pipelines

Linux 1.0 requires the lean Clean default (`clean`, with `raw` beside it; the Fast Dictation recipe is roadmap) plus first-class saved Pipeline support, optional guarded post-processing stage contracts, and versioned settings/Pipeline import/export (FUN-032, FUN-033, UX-011). This does **not** require shipping every candidate denoiser/provider or the public plugin marketplace. The normal UI remains simple; graph/module controls are Advanced.
