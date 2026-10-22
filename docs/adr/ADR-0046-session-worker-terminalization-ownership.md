# ADR-0046: Session worker terminalization ownership

- Status: Accepted for controlled Recovery 4N-B review
- Date: 2026-08-30
- Release effect: none; all Linux 1.0 requirements remain blocked

## Context

An admitted session owns a durable Pending recovery record and an in-memory
session slot before its worker begins capture or ASR. The strict recovery lanes
already terminalized their expected initialization failures. The older
non-strict path still had early returns and propagated ASR errors that removed
only the slot, or sometimes removed neither resource. A worker panic could do
the same. Restart could then misclassify a never-completing operation as
recoverable, while live status could retain a dead active session.

## Decision

1. Every admitted session worker owns a scope-bound cleanup guard containing
   its daemon state, durable journal, session identity, and transaction
   identity.
2. Expected exits record their precise terminal journal state and content-free
   failure code before the guard runs.
3. If a return or panic reaches the guard while the transaction is still
   Pending, it is failed closed as `worker_exited_pending` or
   `worker_panicked`. The guard always removes the session slot. It never
   rewrites Committed, Delivered,
   InboxRecovered, Cancelled, FailedRecoverable, or FailedTerminal records.
4. Non-strict capture, fixture, model, runtime, and pipeline errors use the same
   terminal failure transition instead of silently deleting only in-memory
   state. Cancellation remains a distinct Cancelled terminal state.
5. ASR error events update the durable record as FailedTerminal as well as
   publishing Failed then Idle events.
6. Finalization of a transaction that is absent from the journal is an
   integrity failure. It is never treated as successful delivery or
   cancellation.
7. Durable terminalization precedes terminal event publication when the
   orchestrator controls both operations.
8. The live slot machine is synchronized at every emitted stage, so a later
   propagated error continues sequence numbers monotonically rather than
   publishing stale terminal events.
9. A cancellation-shaped component error is Cancelled only when the caller's
   cancellation token is set and the typed component code is `cancelled`.
10. A caller-held shared strict-permit lease spans worker spawn. On successful
    registration the worker cleanup guard retains the lease; on spawn failure
    the caller releases it only after synchronous journal and slot cleanup.

## Consequences

- Expected failures, propagated failures, and panics cannot leave an admitted
  Pending record plus a dead or permanently registered session slot under
  normal journal operation.
- Strict recovery behavior is unchanged except that the shared last-resort
  guard also owns slot removal.
- If journal storage itself is unavailable, the worker still releases its
  in-memory slot; no terminal success event is emitted, and the exact-recovery
  read then fails rather than fabricating a successful result.

## Deferred boundary (resolved by ADR-0047)

The legacy non-strict `capture_pipewire` helper still uses a temporary WAV and
does not share the strict fileless capture cancellation/reaping contract. It is
not a production-qualified path. Replacing or disabling it requires a separate
bounded phase with fake-child timeout/cancellation tests. Recovery 4N-B does
not claim native capture, delivery, package, or release behavior.

Recovery 4O subsequently chose the fail-closed option: ADR-0047 removes that
helper and rejects non-strict PipeWire before admission. Re-enabling a
non-strict live pipeline still requires a complete owned-capture migration.

## Verification

- deterministic invalid-fixture and missing-model worker tests;
- controlled unwind test proving Pending terminalization and slot cleanup;
- missing-journal finalization rejection;
- full daemon tests with all features;
- workspace check/test/Clippy and fail-closed governance validation before the
  phase is closed.

## Non-goals

- enabling the legacy PipeWire helper for release;
- native microphone or target-application observation;
- evidence, requirement transitions, manifest resealing, packaging, commits,
  or pushes.
