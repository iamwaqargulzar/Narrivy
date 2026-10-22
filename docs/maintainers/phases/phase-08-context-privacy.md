# P8 — Privacy, History & Dictionary Essentials

> Title historically "Context, Personalization & Privacy"; scoped to essentials per the legacy planning amendment.

## Outcome
Keep only privacy/personalization capabilities needed for a safe useful 1.0. Advanced context, correction learning, project indexing, shareable packs, and fragile selected-text workflows are not allowed to block core 1.0 unless they were independently proven and remain low risk.

## Primary requirements (current scope)
`FUN-041`, `FUN-064`, `PRIV-001`–`PRIV-006`.

Deferred: `FUN-040` advanced scoping, `FUN-042`/`FUN-043` (learning), `FUN-044` (snippets), `FUN-045` (packs), `FUN-060`–`FUN-063` (broad Context Fabric/project) — now R12/R14.

## Required work
- Local processing is default; optional history retention is user-configurable and can be disabled globally or by Privacy Zone (PRIV-001, PRIV-002).
- Privacy Zones override Mode/Profile/plugin cloud permissions (PRIV-003).
- Cloud-bound data is declared before use and visible in UI (PRIV-004).
- Secrets never in regular config files (PRIV-005); logs/diagnostics redact content by default (PRIV-006, FUN-064 — context never sent remotely unless policy explicitly permits it).
- Spoken aliases/pronunciations map to canonical forms where safe (FUN-041) as the minimal 1.0 dictionary/personalization.
- Every context provider that does exist declares sensitivity/permission requirements (if broad providers deferred, this is vacuously satisfied for the minimal set).

## Exit evidence
Privacy baseline enforced before any optional network boundary; history can be disabled without fragility; no advanced context is required for core dictation. Schema-valid evidence. `python3 scripts/phase_gate.py --phase P8` exits 0.
## v1.3 provider privacy split

Enforce PRIV-007: sending the transcript to a remote text processor does not imply permission to send surrounding context, application content, clipboard, or audio. These are separately declared/authorized data flows.
