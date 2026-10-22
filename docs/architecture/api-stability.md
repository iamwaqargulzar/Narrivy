# API / Contract Stability Policy

## Phase-0 scaffold

Current small Rust traits/types exist to prove workspace boundaries and a mock vertical slice. They are **not frozen APIs**. P1–P5 should redesign insufficient signatures rather than build complexity around them.

Expected redesign areas include asynchronous/event semantics, cancellation, transaction identity, typed delivery outcomes, capability health, ASR streaming/timestamps/confidence, and versioned IPC serialization.

## Stability levels

- `scaffold` — freely redesign inside active phase; migrate tests/code together.
- `internal-stable` — breaking change requires repository-wide migration and decision log entry.
- `protocol-stable` — IPC/data format used across independently updated components; requires compatibility rules/migration and ADR for breaking changes.
- `public-stable` — external integration/plugin contract; not introduced for Linux 1.0 without an explicit ADR/support commitment.

Mark stability in module/protocol docs. Do not accidentally make a scaffold public by documenting it as permanent.
