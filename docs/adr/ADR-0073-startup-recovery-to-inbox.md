# ADR-0073: Reconcile committed crash recovery records to the Inbox at startup

- Status: accepted for source composition; exact-package qualification pending
- Date: 2026-09-21
- Phase: P11 recovery

## Context

The durable recovery journal already survives daemon restarts and retains a
corrected transcript plus delivery intent before delivery begins. A client can
read that exact transaction, but the daemon did not reconcile it after a
restart. A crash between the committed journal write and the terminal delivery
write could therefore leave recoverable text stranded until a client knew the
transaction identifier.

Blindly retrying the original target backend is unsafe. The external insertion
may have succeeded before the crash, and the daemon cannot prove otherwise.

## Decision

1. Before accepting requests, the daemon scans the retained journal's bounded
   recoverable set. Only `Committed` and `FailedRecoverable` entries with
   corrected text and a matching persisted transaction identity are eligible.
   Incomplete `Pending` entries remain available through the exact recovery
   read and are never guessed into a final result.
2. Eligible text is written through the Inbox-only adapter under the original
   transaction identifier. Startup never retries an external target, keyboard,
   clipboard, portal, or helper backend.
3. After the Inbox write succeeds, the journal records the observed Inbox
   report, copies corrected to delivered, clears the failure code, and becomes
   `InboxRecovered`. A restart sees that terminal state and does not replay it.
4. An Inbox failure leaves the original record recoverable and startup
   continues. A journal read or write failure stops startup because durable
   recovery integrity is then unknown.
5. Ordinary logs contain only content-free recovered, incomplete and failed
   counts. They never include transaction identifiers or transcript content.

## Consequences

Recovery is bounded by the journal's existing 15-minute/100-record retention
policy and is idempotent because both the Inbox artifact and journal entry use
the transaction identifier. If a direct insertion succeeded immediately before
a crash, the user may also receive an Inbox copy, but Narrivy will not risk a
second external insertion.

The reviewed native qualification procedure recreates the committed boundary
from a real exact-package session while the daemon is stopped, removes its
terminal Inbox artifact, restarts the same packaged daemon, and verifies one
Inbox recovery plus a no-op second restart. It does not claim external target
delivery or optional History behavior.
