# ADR-0042 — Capture-first streaming admission

**Status:** Recovery 4L controlled composition accepted (2026-08-29);
native clipping, latency, package, and release qualification remain blocked

## Context

The strict streaming transaction previously opened the resident ASR stream
before starting PipeWire capture. Native stream admission may perform bounded
worker/model setup, so speech beginning immediately after `StartSession` could
occur before the microphone owner existed. The live Linux capture adapter
already has a safer primitive: `wait_ready` retains the first complete 80-ms
chunk while a bounded eight-chunk channel continues accepting contiguous
audio.

This is transaction-scoped capture admission. It is not continuous listening,
always-on pre-roll, wake-word capture, or composition of the legacy diagnostic
overwrite ring.

## Decision

After journal creation, raw-audio policy approval, and model streaming-
capability validation, core starts capture and waits for a real retained chunk.
Only then does it publish `Listening` and attempt ASR stream admission. The
retained sequence-zero chunk and every later queued chunk pass through the same
continuity validator and are fed to ASR exactly once.

The lifecycle and failure rules are:

- capture start or readiness failure opens no ASR owner;
- rejection of the truthful `Listening` transition aborts capture and opens no
  ASR owner;
- cancellation or ASR admission failure after capture readiness aborts and
  boundedly reaps capture;
- after ASR admission, either-side failure uses the existing shared absolute
  cleanup deadline and cleanup failure retains precedence;
- the bounded Linux channel remains eight 80-ms chunks. Admission that cannot
  keep up produces typed `capture_backpressure`; no audio is overwritten,
  skipped, or presented as successfully streamed. A known producer terminal
  error preempts even the retained first chunk, so no partial can escape after
  continuity has already been lost.

`Listening` means that the real capture source is flowing. It does not assert
that model admission is complete or that native latency/clipping targets pass.

## Verification boundary

Controlled tests prove capture-start → capture-ready → `Listening` → ASR-open
ordering, exact-once feed of both controlled chunks, no ASR owner on readiness
or `Listening` rejection, capture cleanup on admission failure/cancellation,
cleanup-failure precedence, and existing continuity/backpressure/finalization
behavior. A barrier-controlled Linux test retains sequence zero, then forces
admission-period overflow and proves the terminal error escapes before audio.

These tests do not measure native model admission duration, the effective
retention window, microphone clipping, speech/noise behavior, end-to-end
partial latency, CPU/RSS/thermal behavior, or clean-machine/package behavior.
No requirement or evidence status changes.

## Consequences

- A strict transaction owns capture before potentially slow model admission,
  closing the previous unbounded start-of-speech gap.
- The first real chunk is not used only as a readiness marker; it enters the
  normal sequence and ASR feed exactly once.
- Backpressure remains visible and terminal instead of expanding memory or
  silently losing early speech.
- Capture can be active while model admission is pending, so cancellation and
  every admission failure must retain bounded capture cleanup.

## Alternatives rejected

- opening ASR first and relying on the user to pause after session start;
- enlarging or unbounding the queue without native measurements;
- silently dropping oldest early chunks during admission;
- composing the legacy overwrite pre-roll ring into the release lane; or
- claiming that controlled ordering proves native no-clipping performance.
