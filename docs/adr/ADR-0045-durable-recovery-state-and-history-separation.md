# ADR-0045: Durable recovery state and optional History separation

- Status: Accepted for Recovery 4N-A
- Date: 2026-08-30
- Release effect: none; all Linux 1.0 requirements remain blocked

> **History composition amendment (2026-09-28):** the *separation* decided
> below still holds and is enforced: the recovery journal remains the only
> holder of in-flight text. Items 4, 6 and 7 are stale about History
> *availability*. `narrivyd` now composes a real optional History store below
> its durable state root, with its own database, retention policy and privacy
> zone evaluation
> (`services/narrivyd/src/daemon/retained_history.rs`). `GetHistory` and
> `GetHistoryEntry` are served when an open store exists, advertise
> `history.optional=enabled`, and return the typed not-composed rejection only
> when composition failed. Recovery is still never copied into History as a
> listing and `GetRecoveryEntry` remains the exact-transaction-only read.

## Context

ADR-0009 and ADR-0013 require the short-lived crash-recovery journal and
optional user History to be separate stores. The current daemon instead opens
the recovery journal below the session runtime directory and serves its rows
and transcript variants through `GetHistory` and `GetHistoryEntry`. That loses
recovery state at logout/reboot, gives recovery data the wrong product
semantics, and makes an unimplemented History feature appear functional.

The desktop also locates the newest journal entry after a terminal session
event rather than requesting the exact transaction reported by that event.
Concurrent or stale entries can therefore select the wrong result.

## Decision

1. The Linux platform adapter resolves the recovery state root from an
   absolute `$XDG_STATE_HOME`, otherwise absolute `$XDG_DATA_HOME`, otherwise
   `$HOME/.local/share`, and creates a private `narrivy` directory. Unsafe,
   relative, symlinked, wrongly owned, or group/other-writable storage fails
   closed.
2. `narrivyd` opens `journal/` below that durable state root. Runtime sockets,
   transient pipelines, and the current recovery Inbox remain under the
   runtime root until their own retention contracts are decided.
3. Startup enforces the 15-minute recovery TTL and strict 100-entry cap before
   accepting requests. The production journal is wrapped by one serialized
   retention boundary that enforces the same policy before reads and after
   mutations, so a long-running daemon cannot expose expired rows or drift
   above the cap. Expired records are removed regardless of terminal state.
   Cap pressure removes oldest non-recoverable records first, then the oldest
   remaining records only when unavoidable. A newly admitted, non-expired
   transaction is preserved when another victim can satisfy the cap.
4. Legacy History commands remain wire-decodable but return a typed,
   content-free fail-closed rejection while optional History is unavailable.
   They never read the recovery journal.
5. A protocol-minor additive `GetRecoveryEntry { txid }` request returns the
   exact short-lived recovery record for one transaction. It exists only for
   current-session/recovery UX and is not a History listing API.
6. The desktop requests the transaction ID carried by the terminal event. It
   never lists the recovery journal to infer a result. The History screen is
   capability-driven and explicitly reports that optional History is disabled.
7. No recovery record is copied into `narrivy-history`. A real optional History
   store, retention settings, Privacy Zone enforcement, migrations, and clear
   UX require a later dedicated phase.
8. Transaction identities contain 128 bits from the operating-system CSPRNG.
   Admission retries a bounded number of collisions and uses an atomic
   no-clobber journal creation operation. Entropy failure or collision-budget
   exhaustion occurs before session events or slots become visible.
9. Qualification daemons always override ambient `XDG_STATE_HOME` with a
   private 0700 root and inspect only that root's journal. Crash/restart
   scenarios retain their isolated state for the scenario lifetime.
10. The desktop accepts transcript-bearing recovery responses only when the
    returned transaction identity exactly matches the request and the state is
    a successful terminal recovery state. Mismatches remain content-free UX
    failures.
11. The obsolete P6 qualification runner is globally quarantined. Direct
    execution and imported `main()` return status 2 without reading recovery
    state, running legacy commands, or writing evidence. Any replacement must
    use a separately reviewed schema-v2 production procedure.

## Security and privacy consequences

- Recovery contents survive daemon/runtime restarts only for the bounded TTL.
- Ordinary diagnostics expose paths, counts, states, and typed error codes but
  never transcript variants.
- Same-user IPC remains necessary but does not turn recovery data into History.
- Disabling History never disables crash recovery.
- A colliding transaction cannot replace an earlier recovery record.
- Qualification cannot prune or inspect the invoking user's recovery journal.

## Compatibility

The additive recovery request increments the protocol minor. Older peers may
continue decoding known messages. Legacy History requests fail closed instead
of returning misleading recovery data.

## Non-goals

- enabling or qualifying optional History;
- moving or extending Inbox retention (startup reconciliation of committed
  records is specified separately by ADR-0073);
- migration of legacy runtime-journal entries;
- release evidence, requirement status, packaging, manifest sealing, commits,
  or pushes.
