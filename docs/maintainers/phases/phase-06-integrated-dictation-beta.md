# P6 — Integrated Dictation Beta


## Outcome
The real product already exists from P2. P6 turns it into a strong daily dictation beta and establishes **measured** latency/reliability baselines that every later phase must preserve.

## Primary requirements
`FUN-008`, `PERF-001`–`PERF-004`, `REL-006`, `REL-008`, `OPS-001`, `TEST-009` plus integrated revalidation of P2–P5.

## Mandatory real vertical slice (revalidated, not first time)
On a **reference Linux environment** (native where possible): launch daemon/client → onboarding/capability checks → choose microphone/model → global PTT/toggle → capture real speech → real local ASR → processing → reliable insertion into a supported external application; if delivery cannot complete, final text appears in Inbox. Cancellation must not insert stale final results.

## Recovery/diagnostics
Implement minimal active-session/recovery journal so recoverable final text survives the documented crash window; separate recovery retention from optional history (ADR-0013). Doctor/Diagnostics reports actionable platform/audio/portal/ASR/device/delivery state without transcript/audio/context content.

## Performance instrumentation (per Part I protocol)
Measure separately: hotkey→ack (<30 ms target), capture-ready (<50 ms warm), visible listening feedback (<100 ms), speech→first partial (<250 ms when streaming), release→raw, release→committed, committed→delivered/fallback-confirmed. Record reference hardware, model state (Hot/Warm/Cold), fixed fixture, sample count, p50/p95 and failure rate — see `docs/maintainers/qualification/performance-benchmark-protocol.md`. Failures to meet targets are real defects, not hidden averages.

## Hard gate
`TEST-009` cannot pass from mocks or source-only tests. Evidence must identify OS/session, target app, real microphone, model/runtime/device and observed delivery/fallback. `python3 scripts/phase_gate.py --phase P6` exits 0 and working-app regression gates P2–P5 still pass.
