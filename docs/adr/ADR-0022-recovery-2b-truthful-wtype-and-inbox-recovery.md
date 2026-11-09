# ADR-0022 — Recovery 2B truthful wtype submission and Inbox recovery

**Status:** Accepted for Recovery Phase 2B implementation (2026-08-27)

## Context

Recovery Phase 2A established a strict, Inbox-primary microphone-to-text
recovery path. Phase 2B adds a narrow, opt-in attempt to submit corrected text
to one focused Linux application through `wtype`, without allowing a helper
exit status to overstate what the application retained.

The direct submission boundary is privacy-sensitive: command-line arguments
are observable by other same-host processes, direct target retention is not
observable through `wtype`, and a cancellation after a positive stdin write
cannot truthfully guarantee that no text reached the focused application.

## Decision

- Phase 2B is a separate strict composition selected only by
  `NARRIVY_RECOVERY_MODE=linux-mic-wtype-inbox`. It requires PipeWire,
  transcribe.cpp, an explicit GGUF model, a resolved executable `wtype`, and
  a private Inbox directory. It rejects mocks, fixture/file audio, null
  delivery, the platform router, clipboard, uinput, forced delivery failure,
  and all other strict recovery modes.
- `StartSession` and `SelectPipeline` share one fail-closed selection rule.
  In either strict mode only its own pipeline id is eligible, and the request
  still requires strict runtime readiness. The opposite strict id and the
  mock-dictation, raw, and clean ids are rejected rather than inherited from
  the historical correction strategy.
- `DeliveryOutcome::Delivered` remains reserved for an adapter-verified,
  durable destination. A successful direct helper invocation is recorded as
  `SubmittedUnverified`, an attempt-only fact which can never be promoted to
  `Delivered`.
- The Linux direct helper uses an explicit executable and the fixed argv
  `wtype -`; the corrected payload is written only to bounded child stdin.
  It never uses a shell or puts payload text in argv, diagnostics, or normal
  logs. Spawn, stdin, wait, timeout, nonzero exit, cancellation, and bounded
  termination/reap cases have content-free typed outcomes.
- The Linux direct helper and reusable `StdCommandRunner` use a parent-owned
  nonblocking stdin pump rather than a writer thread. The sole owner keeps the
  pipe and child together, closes stdin at EOF, and uses one absolute deadline
  for submission, cancellation/error/timeout, SIGKILL, and reap. There is no
  detached transcript-bearing writer, writer-spawn path, ticker, or unbounded
  join. If a killed child cannot be observed reaped by that original deadline,
  the outcome is a distinct content-free termination-timeout failure rather
  than a successful or silently abandoned submission.
- Core persists corrected text and a `wtype-then-inbox` intent before the
  external submission starts. It persists the exact adapter report before the
  terminal transition. The strict adapter records exactly one direct attempt
  and, unless cancellation occurred before the OS accepts its first payload
  byte, exactly one atomic Inbox write under the same opaque transaction id
  and bytes.
- A successful Phase 2B completion is `InboxRecovered`, not `Delivered`:
  the report includes either an unverified or a failed direct attempt followed
  by the durable Inbox recovery attempt. Inbox persistence or post-delivery
  journal failure remains a typed recoverable uncertainty; no partial report
  is synthesized or silently discarded.
- Core accepts `InboxRecovered` only for one exact report shape: two attempts,
  `wtype-stdin` direct followed by `inbox`, a direct
  `SubmittedUnverified` without a fallback reason or a direct `Failed` with
  one, then InboxFallback without a fallback reason; the final backend,
  outcome, failure reason, and transaction identity must match the strict
  route. Any inconsistent report is persisted as recoverable failure.
- Cancellation before the parent-owned nonblocking pipe has accepted any
  payload byte returns a clean cancelled result with no direct attempt and no
  Inbox item, after terminating and reaping the child within the original
  deadline. Once one or more bytes are accepted, cancellation is a typed
  delivery-uncertain direct failure and the Inbox recovery path remains
  mandatory; it is never represented as clean cancellation.
- Capability reporting states only whether the executable submission
  mechanism is available, explicitly states that verified direct delivery is
  unavailable, and identifies Inbox recovery as enabled. It does not claim
  compatibility with applications beyond the owner-authorized COSMIC Edit
  observation target.

## Consequences

- `linux-mic-inbox` remains the unchanged Phase 2A Inbox-primary lane.
- `InboxRecovered` is terminal and does not make existing `Delivered` journal
  records incompatible or recoverable again.
- The broad legacy delivery router is not composed into the strict Phase 2B
  path. Its wtype helper follows the same stdin/unverified truth rule so no
  reused helper can expose transcript text in argv or call a helper exit a
  verified result.
- Controlled tests validate contracts only; they are neither native
  observation nor evidence. This ADR authorizes no evidence, requirement,
  package, release, or Phase 2C claim.

## Alternatives considered

1. Treat wtype zero exit as `Delivered`. Rejected: it proves helper execution,
   not that the focused target durably retained text.
2. Put text after `wtype --` in argv. Rejected: process arguments expose
   private text to the process table.
3. Use the existing broad router for strict recovery. Rejected: even a forced
   final Inbox rung admits unsupported routing/probing paths and cannot state
   the one-attempt/one-Inbox contract.
4. Call late cancellation clean cancellation. Rejected: stdin dispatch makes
   absence of external side effect unknowable.

## Verification

Controlled tests cover fixed stdin argv and exact bytes (including Unicode,
multiline, leading-dash, empty, and maximum permitted payloads), typed direct
failure reasons, parent-owned pipe failure and the former writer-panic/setup
failure boundaries, non-draining stdin, late cancellation, timeout, nonzero
exit, bounded child reaping, and synchronized cancellation before versus after
the first positive write, durable intent-before-attempt ordering, exact report
shape, and strict composition rejection. The phase baseline records
noninteractive command outcomes only; any native COSMIC Edit readback requires
a later owner-authorized window and remains non-evidence.

The later owner-authorized COSMIC Edit observation completed with one
`SubmittedUnverified` wtype attempt and exactly one matching Inbox recovery,
but the saved target remained empty. Therefore Phase 2B established bounded
attempt/recovery behavior only; it did not qualify target insertion. The
content-free details are recorded additively in the Phase 2B recovery baseline.
