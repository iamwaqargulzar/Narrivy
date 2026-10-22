# History and Learning

> **Status: roadmap / not in 1.0.** This describes a planned design, not current behaviour. See [the roadmap](../README.md).

Separate concerns:

## Session journal
Short-lived crash recovery and delivery transaction state.

## History
User-visible committed sessions with configurable retention. Raw/corrected/final text variants and optional audio only when enabled.

History **is composed** by `narrivyd` in a separate store below its durable
state root, with its own database, retention policy and privacy-zone
evaluation (`services/narrivyd/src/daemon/retained_history.rs`). Composition is
best-effort: when the store cannot be opened, `GetHistory`/`GetHistoryEntry`
return a typed not-composed rejection, every other command keeps serving, and
Status advertises `history.optional=disabled:not_composed`. A composed store
advertises `history.optional=enabled` plus content-free `history.retention=`
and `history.max_entries=` capabilities, so a client can trust the capability
as the gate before calling.

The journal remains the only holder of in-flight text. `GetRecoveryEntry` is
the exact-transaction-only recovery read; there is no recovery listing, and the
recovery journal is never read through a History command.

## Learning store
Structured corrections, aliases, scoped vocabulary, usage weights and routing observations. It must be independently clearable/exportable.

## Privacy rule
Disabling History must not silently disable all short-lived crash recovery, but recovery data must have a narrow TTL and explicit documentation.
