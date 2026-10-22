# P9 — Reliability, Security, Accessibility & Performance Hardening

## Outcome
Attack the already-working product using failure injection, security tests, accessibility qualification, performance regressions, migration tests, lifecycle tests, resource exhaustion, and privacy leakage tests. Every later phase preserved P2/P6 vertical slice — P9 proves it under stress.

## Primary requirements
`PERF-010`, `SEC-001`, `SEC-003`, `SEC-006`, `ACC-001`–`ACC-007`, `TEST-006`, `TEST-007`.

## Required work
- Pipeline can prepare context/model/output concurrently (PERF-010) where beneficial.
- Plugin permissions explicit/enforced, network deny-by-default, untrusted archive handling (SEC-001, SEC-003, SEC-006).
- Full keyboard, screen-reader, reduced motion, scalable/HiDPI, high-contrast/focus, non-color state, hands-free roadmap without degrading defaults (ACC-001–007) — see `docs/product/ux/accessibility.md` and benchmark protocol.
- Failure injection: daemon/client loss, text-delivery failure (verify bounded fallback and transcript preservation), device loss, corrupt cache/model, interrupted migration/sessions without losing recoverable text (TEST-006).
- Accessibility/UI acceptance (TEST-007).
- Performance hardening against benchmark protocol regressions (p50/p95, failure rate).
- Security hardening respects Part M resource limits and Part G delivery capability truth.

## Exit evidence
All hardening scenarios have schema-valid evidence bound to native environment where required; no blocker remains. `python3 scripts/phase_gate.py --phase P9` exits 0.
