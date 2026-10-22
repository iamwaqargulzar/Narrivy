# ADR-0044 — Ephemeral monotonic recovery timing

**Status:** Recovery 4M-B controlled diagnostic accepted (2026-08-30);
native speech-onset/UI latency and release qualification remain blocked

## Context

The daemon inherited a P6 timing mechanism that placed Unix-epoch timestamps
in string-keyed maps, retained completed transaction timings in a global queue,
and wrote JSON below the runtime directory by default. The matching legacy
runner could combine unrelated aliases and mock observations into a passing
latency claim. That mechanism could neither prove monotonic duration nor keep
transaction timing ownership and retention narrow enough for recovery work.

Recovery 4M-A removed model loading from request admission. The next useful
question is where time is spent in an already-resident streaming transaction,
without turning instrumentation into another persistence or evidence path.

## Decision

Portable streaming orchestration owns a fixed, content-free timing vocabulary:

1. capture start requested;
2. capture ready;
3. ASR stream opened;
4. first audio block accepted by ASR;
5. first validated partial accepted by the optional observer;
6. capture ended;
7. non-empty final ASR text accepted;
8. corrected text and delivery intent durably committed;
9. delivery call returned; and
10. terminal journal state durably committed.

`RecoveryTimingObserver` is optional and infallible. The production daemon does
not install one. `RecoveryTimingTrace` owns one `Instant` origin and a fixed
array of atomics; it records the first observation of each enum stage as a
relative microsecond offset. It stores no wall-clock time, transaction/session
identity, audio, transcript, arbitrary keys, heap-backed samples, files, IPC
messages, logs, or evidence.

Stages describe completed boundaries, not intended work. A rejected partial is
not timed as accepted. Failed correction cannot claim corrected commit.
Delivery return is timed for either success or failure, while terminal commit
is recorded only after the successful terminal journal write.

The daemon's legacy timing writer, retention queue, runtime timing directory,
and internal recording calls are removed. `StageTimingsUs` remains only as an
optional protocol-minor-3 compatibility field; current daemons leave it absent.
Adding durable timings, client-visible timing IPC, retention, aggregation, or
qualification evidence requires a separate ADR and reviewed privacy schema.

## Controlled diagnostic

An ignored all-feature integration test accepts only explicit regular,
non-symlink fixture/model/worker paths and an exact fixture SHA-256. It parses a
bounded PCM16 mono 16-kHz WAV, rejects duplicate chunks and files over 30
seconds, and paces complete 80-ms blocks with cancellation checks. It uses the
real resident worker, real deterministic corrector, real Unix journal, and real
Inbox-only adapter inside one 0700 temporary root with 0600 payload files.

The diagnostic asserts all ten monotonic stages, one journal row, one Inbox
item, matching content hashes, private modes, explicit process cleanup, and
temporary-root removal. It prints only fixed stage names and relative offsets.

The currently available JFK WAV is useful for this engineering run but lacks a
separately reviewed audio-specific provenance/license record. Its paced
in-memory source also substitutes for PipeWire, and its speech-onset time is
not annotated. Therefore this observation is not microphone, OS-capture,
speech-to-partial, UI-repaint, percentile, PERF-005, or release evidence.

## Consequences

- Controlled work can locate gross orchestration/engine delay without creating
  a second sensitive data store.
- Production recovery behavior and IPC wire version are unchanged.
- Timing labels cannot be silently invented by arbitrary strings or wall-clock
  adjustment.
- The diagnostic exposes serial feed/backpressure behavior, but only a native
  microphone/loopback protocol with annotated onset can qualify perceived
  latency.
- Legacy P6 timing artifacts and runners remain untrusted historical material;
  removing additional dead P6 files is repository-cleanup work, not part of
  this decision.

## Alternatives rejected

- retaining epoch timestamps and subtracting them later;
- enabling disk timing JSON by default;
- publishing diagnostic timing through the existing IPC compatibility field;
- treating fixture start as speech onset;
- using mock ASR, a batch sidecar, or manufactured partials; or
- marking PERF-005 from a single controlled fixture run.
