# ADR-0040 — Volatile, identity-bound partial presentation

**Status:** Recovery 4J controlled composition accepted (2026-08-29);
native latency, acoustic quality, and release qualification remain blocked

## Context

The resident streaming engine already emits ordered partial hypotheses, but
Recovery 4H deliberately discarded them after validation. Presenting them
crosses a new privacy and lifecycle boundary: transcript content enters IPC
and a webview before final policy/correction/delivery. A naïve broadcast could
send content to every authenticated connection, let high-rate partials evict
terminal state, persist tentative text in UI state/history, or display a stale
hypothesis from another transaction.

## Decision

Core exposes an optional, transaction-scoped volatile observer after ASR event
order, non-empty/16-KiB size, and local raw-transcript policy checks. The
observer receives only sequence and text. It does not alter the final path:
only the validated final transcript enters correction, journal, intent, or
delivery. Observer failure is typed and uses the existing bounded teardown of
capture and ASR.

The daemon attaches exact session, transaction, and pipeline identity. Its
bounded queue coalesces a queued partial for the same transaction in place.
When full, a lifecycle event evicts a partial first; a new partial never evicts
a queue containing only lifecycle events. This bounds memory and favors
terminal truth without creating detached tasks or an unbounded channel.

IPC protocol minor 3 adds `transcript_partial`. Text is 1..=16,384 UTF-8 bytes
and redacted from `Debug`; the envelope already redacts its payload. A same-UID
handshake does not subscribe a client. The server sends events only after an
explicit accepted `Subscribe`, removes eligibility after accepted
`Unsubscribe`, bounds each client event write to 50 ms, and disconnects stalled
writers. The client requires a present, strictly increasing envelope
`event_seq`, preserves it for the desktop, and clears buffered traffic on
reconnect because that begins a new transport epoch.
Subscribers declaring protocol minor 2 continue receiving compatible
lifecycle events but are never sent the minor-3 partial variant.

The desktop holds tentative text only in React memory. It accepts current
tagged events only when the global envelope sequence is fresh and the session,
transaction, pipeline, and ASR partial sequence match the active identity.
It rejects stale/cross-transaction events, clears tentative text on terminal
states or a new session, never writes it to local storage/history, and exposes
it in a separate polite atomic live region. Final/history text remains a
separate explicit journal read.

## Verification boundary

Controlled tests cover policy and size checks, ordered volatile observation,
journal absence during partial delivery, callback-failure cleanup, daemon
coalescing and lifecycle priority, redacted debug output, explicit IPC
subscription, bounded frame values, transport sequence rejection, desktop
identity/stale rejection, terminal clearing, accessibility, and absence from
local storage.

No microphone, native partial latency, UI repaint latency, word stability,
accuracy, package, clean-machine, or release behavior was observed. The 250-ms
product target and all affected requirements remain blocked.

## Consequences and deferred decisions

- Latest-only coalescing may intentionally skip intermediate hypotheses; it
  never changes or suppresses the final transcript.
- A slow subscriber is disconnected rather than allowed to backpressure the
  recognition transaction.
- Tentative text is uncorrected and visually separate from committed history.
- Native timing must measure audio acceptance to model partial and webview
  presentation separately before any latency claim.
- Multi-window authorization, configurable partial presentation, overlay
  behavior, redaction policy beyond the current local boundary, and package
  upgrade compatibility require later work.

## Alternatives rejected

- broadcasting transcript events immediately after authentication;
- journaling partials or treating them as delivered/final text;
- an unbounded per-client channel or detached writer task;
- allowing partial pressure to evict lifecycle events;
- identifying a partial by sequence number alone; or
- claiming PERF-005 from controlled fixtures.
