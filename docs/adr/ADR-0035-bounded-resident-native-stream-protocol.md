# ADR-0035 — Bounded resident native-stream protocol

**Status:** Recovery 4E controlled implementation complete (2026-08-28);
non-release, not live-capture composition or native streaming qualification

## Context

ADR-0034 established authoritative model capability attestation. A true
streaming-capable model uses transcribe.cpp's native session lifecycle:
`begin -> feed* -> finalize`, with reset/drop for abandonment. The native
stream owns the model compute lease for its entire lifetime and returns revision
metadata plus full, committed, and tentative text snapshots.

Narrivy's previous private protocol supported only one-shot final decode. It
could not safely represent persistent stream ownership, partial snapshot
rewrites, or cancellation of a synchronous native feed.

## Decision

1. Resident protocol v3 retains the readiness capability byte and adds
   `STREAM_BEGIN`, `STREAM_FEED`, `STREAM_FINALIZE`, and `STREAM_RESET`.
2. Begin is admitted only when the loaded model attests streaming support. One
   opaque request identity exclusively owns the worker until finalize, reset,
   cancellation/termination, or process shutdown. One-shot and second-stream
   requests fail fast rather than queue.
3. Each feed carries non-empty finite 16 kHz mono f32 PCM through the existing
   16 KiB transfer buffer. The cumulative stream is bounded to 480,000 samples
   (30 seconds). Calls are synchronous and caller-paced; there is no background
   audio or text queue.
4. Snapshot payloads are bounded to 1 MiB and contain fixed-width revision,
   change/finality flags, input/commit/buffer timing metadata, and exact lengths
   for UTF-8 full/committed/tentative text. Reserved bits/bytes, lengths, UTF-8,
   identity, status, finality, and revision ordering fail closed.
5. Narrivy emits a Partial only for a changed, non-empty, non-duplicate
   `committed + tentative` display snapshot. It assigns its own monotonic event
   sequence after validating native revision monotonicity. Final uses native
   `full` text and is always sequenced after any emitted Partial.
6. `AsrEngine::open_stream` receives the caller's shared cancellation token,
   and `AsrStream` exposes the same token after admission. Native
   `STREAM_BEGIN` has a two-second bound; a replacement worker/model startup
   retains its separate cancellable 30-second bound. Parent pipe I/O observes
   cancellation throughout startup/begin/feed/finalize; cancellation kills and
   reaps the isolated worker. A future request may spawn a
   capability-consistent replacement.
7. Finalize releases admission. Drop attempts one bounded reset; missing,
   malformed, late, or uncertain reset terminates/reaps the worker. Bounded
   `finish(timeout)` uses a checked caller deadline and rejects an unrepresentable
   duration as a typed error.
8. Any begin error, EOF, malformed response, cancellation, or timeout removes
   and terminates/reaps the worker before returning. A worker that may have
   entered native streaming is never retained without an active owner.

## Privacy and product boundaries

Narrivy intentionally keeps audio and snapshots in bounded process memory and
private pipes and creates no payload files or ordinary-log payloads. Recovery
4E does not configure OS core-dump or swap hardening, so crash-dump/swap
exposure remains a release blocker and the stronger claim that the OS can never
persist process memory is not made. Recovery 4E does not connect Partial events
to journals, policy, correction, delivery, IPC, desktop state, or evidence.

Engine/worker stream composition is not product streaming. Live PipeWire chunk
ownership, VAD/endpointer, daemon routing, desktop accessibility/partial UX,
and native model qualification are absent, so daemon status remains
`streaming=disabled:not_composed`.

## API decision

The ASR traits remain pre-stability scaffolds under
`docs/architecture/api-stability.md`. Adding `cancellation()` and passing the
same token into `open_stream` migrates all implementations together and closes
the pre-stream cancellation gap. It does not create a public or
protocol-stable API.

## Failure and recovery

- final-only model: typed rejection before stream admission;
- contention: typed immediate `asr_busy`;
- malformed/unordered response: terminate/reap and typed protocol failure;
- begin/feed/finalize cancellation, timeout, or pipe failure: terminate/reap;
  no partial becomes final;
- abandoned stream: reset, otherwise terminate/reap;
- changed capability after restart: reject replacement and terminate it.

## Controlled verification

- begin/feed/finalize event order and duplicate suppression;
- final-only pre-admission rejection;
- active-stream exclusion of one-shot work;
- reset-on-drop followed by healthy one-shot reuse;
- marker-synchronized cancellation of blocked begin/feed, old-PID reaping, and
  clean restart;
- timeout, EOF, and malformed begin invalidation/reaping;
- oversized finish timeout returns a typed error rather than panicking;
- malformed snapshot termination and clean restart;
- protocol snapshot round-trip and malformed length/flag rejection;
- maximum cumulative/request bounds and existing one-shot lifecycle checks; and
- full workspace, strict Clippy, boundary, governance, and cleanup checks.

## Deferred qualification

- integrity-pinned streaming model, license/language/accuracy selection;
- native stream feed/finalize observation using that model;
- live capture producer, bounded queue/backpressure policy, VAD/endpointer and
  explicit Stop semantics;
- daemon/client partial-event protocol and accessible desktop failure UX;
- latency/accuracy/duplication/resource/cancellation qualification; and
- package co-installation/integrity, clean-machine, upgrade, and rollback; and
- worker core-dump/swap containment and crash-path privacy qualification.
