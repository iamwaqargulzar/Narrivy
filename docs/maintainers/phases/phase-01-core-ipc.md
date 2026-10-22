# P1 — Core, IPC, Persistence & Privacy Baseline

## Outcome
Decide and implement explicit session state machine, cancellation/bounded timeouts, typed outcomes/errors/events, daemon/client protocol, same-user IPC security, crash/recovery journal, minimal privacy-policy enforcement before any optional network boundary exists, persistent store and migration strategy, daemon lifecycle, schema/version ownership, and deterministic mock test path.

## Primary requirements
`FUN-005`, `FUN-020`, `REL-001`, `REL-007`, `PORT-002`–`PORT-005`, `SEC-005`, `TEST-003`, `OPS-006`.

## ADRs required before implementation
- `docs/adr/ADR-0012-daemon-lifecycle.md` — systemd-user/service vs sidecar/autostart, single-instance ownership, socket lifecycle, crash restart, graceful shutdown, version skew, uninstall/data retention.
- `docs/adr/ADR-0013-persistence.md` — storage technology, locking/transactions, recovery after interrupted write/migration, backup/rollback, separation of active-session recovery vs optional history.

See those ADRs for authoritative decisions; this phase implements them.

## Required domain contracts
- Explicit session/transaction state machine with legal/illegal transition tests and monotonic event sequencing (REL-001).
- Raw/corrected/delivered transcript representations; delivery attempts/outcomes/fallback reason; cancellation; recoverable vs terminal failure (FUN-005).
- ASR stream contract capable of partial/final events, cancellation and optional timestamps/confidence/context capability — do not fossilize the Phase-0 synchronous scaffold.
- Declarative Mode pipeline schema/validation independent of UI (FUN-020).
- Capability model that drives clients without OS-name branching (PORT-005).
- Long-running worker ownership/cancellation/shutdown semantics with bounded timeouts (REL-007).
- IPC versioning: handshake/version, request IDs, bounded messages, cancellation, events, typed errors, disconnect/reconnect, stale endpoint cleanup; Unix same-user endpoint permissions + peer validation.
- Persistence: versioned config/state formats; atomic/recoverable migrations tested against interrupted migration; no silent destructive reset (OPS-006).
- Privacy baseline: policy engine that enforces local-only default and can gate any future network/data boundary before it exists.

## Exit evidence
UI/CLI test client can trigger a mock ASR→pipeline→delivery transaction through daemon/core/null platform. Contract/state/migration/IPC tests pass. Both ADRs are accepted. `python3 scripts/phase_gate.py --phase P1` exits 0.

## Module contract requirement

Introduce an internal `ModuleRegistry`/capability registry and typed module identity/health/version descriptors so activation, capture, audio processing, ASR/compute, context and delivery implementations can be replaced without domain coupling. Keep the contracts internal-evolving; do not publish a Rust dylib ABI.
## v1.3 pipeline/resource contracts

Implement the first-class Pipeline schema/registry (FUN-030) and distinct Module/Runtime/Model/Pipeline resource identities/dependencies (PORT-008). Trigger routing and UI may remain later phases, but P1 core must be able to validate/store/migrate multiple Pipeline definitions without depending on concrete module implementations or download URLs.
