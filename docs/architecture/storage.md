# Storage Model

## Separation of concerns

### Configuration
Small user preferences and IDs; versioned/migrated; no plaintext secrets.

### Recovery journal
Durable short-lived transaction state needed to prevent loss/duplication of final text. It is independent of optional History and has a bounded retention policy.

### History
Optional private transcript content under user retention controls. Disabling History does not disable in-flight crash recovery.

### Learning/dictionary/snippets
User-controlled personalization data, separately exportable/clearable as specified.

### Technical cache
Models, compiled artifacts, health results and benchmark metadata contain no ordinary user transcript/audio. Cache is replaceable and integrity-keyed.

## Filesystem locations

Linux recovery state resolves through the platform adapter from absolute
`$XDG_STATE_HOME`, then `$XDG_DATA_HOME`, then `$HOME/.local/share`, and lives
below a same-user 0700 `narrivy` root. Socket/runtime data remains in the
secure per-user runtime location. Unsafe, relative, symlinked, wrongly owned,
or group/other-writable state roots fail closed.

Recovery retention is 15 minutes from creation and at most 100 valid records.
The production journal enforces this at startup and across subsequent access
and mutation, while explicit synthetic-time fixtures use the undecorated
adapter. Expired records are removed in every state; count pressure removes
terminal/non-recoverable records before live recoverable records. This does
not yet constitute automatic restart-to-Inbox recovery.

## Atomicity

Use atomic replace/transactional storage for small metadata where feasible.
New recovery transactions use 128 bits of OS CSPRNG identity and atomic
no-clobber creation; a collision cannot replace an existing record. Never
expose a partially downloaded/converted model as installed. Delivery final
text must be durably recoverable before risky insertion attempts where the
recovery policy requires it.


> **ADR linkage:** See ADR-0012 and ADR-0013 for lifecycle and persistence decisions.
