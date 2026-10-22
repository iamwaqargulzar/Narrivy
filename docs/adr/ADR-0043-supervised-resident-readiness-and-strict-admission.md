# ADR-0043 — Supervised resident readiness and strict admission

**Status:** Recovery 4M-A controlled composition accepted (2026-08-30);
native latency, clipping, package, and release qualification remain blocked

## Context

Recovery 4L deliberately starts capture before resident stream admission and
retains a bounded 720 ms of contiguous early audio. That ordering removes the
period in which no capture owner existed, but it exposed a different failure:
after cancellation, timeout, EOF, or a protocol violation, the process-isolated
ASR worker is terminated. The next `open_stream` could then spawn and reload
the model synchronously while capture was already accumulating bounded audio.
Status also described cached model capabilities without proving that the
attesting worker process was still alive.

Healthy native measurements with the already-loaded Moonshine canary showed a
roughly 107 ms process/model startup, while `OP_STREAM_BEGIN` itself was below
one millisecond in the controlled sample. The observation is diagnostic only,
not evidence. It supports fixing failed-worker recovery rather than preopening
an anonymous stream or expanding the audio queue.

## Decision

The resident engine owns an explicit lifecycle:

```text
Ready -> Active -> Ready
  |        |
  +--------+-> RestartRequired -> Warming -> Ready
                                          +-> FailedClosed
```

- `Ready` and `Active` own the one attested worker process.
- `open_stream` and one-shot recognition never spawn or reload a worker.
- uncertain termination, process death, timeout, EOF, cancellation, or
  protocol failure removes the worker from request use and boundedly reaps it,
  then reports `RestartRequired`;
- if bounded kill/reap itself fails, the exact child handle moves to an owned
  retiring queue. A later recovery must retry and observe every retirement
  before it may spawn a replacement; child ownership is never dropped;
- `recover` elects one generation under the owner mutex, performs process/model
  startup outside that mutex, and installs only the elected generation;
- concurrent recovery observes `Warming` and cannot create another replacement;
- cancellation or startup failure returns to `RestartRequired`; changed model
  capabilities terminate the candidate and enter `FailedClosed`. Cancellation
  is rechecked after handshake and again under the install lock, so a ready
  frame cannot race shutdown into installing a replacement;
- healthy finalize or reset returns `Active` to `Ready`.

The daemon adds a separate atomic RAII admission permit for strict capture
pipelines. It acquires the permit before pending-journal creation and holds it
through the supervised session worker. A second strict request returns typed
`Busy` before journal, capture, or ASR side effects.

For resident strict lanes, live `Ready` is also required before
`begin_session`. `RestartRequired` schedules one engine-deduplicated recovery
task through the existing `WorkerSupervisor` and rejects the current request.
`Warming`, `Active`, missing, and `FailedClosed` states reject without capture.
After a resident terminal outcome, the daemon schedules recovery only when the
engine reports `RestartRequired`. Daemon shutdown therefore cancels and joins
recovery through the same bounded worker policy as sessions.

Status exposes the live worker state. Streaming, native-energy VAD, endpointing,
and strict pipeline enablement are composed only while the worker is `Ready`;
cached capability metadata alone is not a readiness claim.

## Consequences

- A replacement model is loaded before a later microphone transaction starts,
  so failed-worker recovery no longer consumes the capture admission buffer.
- Worker/model startup remains explicit and observable instead of being hidden
  inside an ASR request.
- Single strict-session admission closes the race where two requests could
  both start capture before one discovered ASR exclusivity.
- Recovery failure stays visible and fail-closed. There is no mock, batch,
  sidecar, model, or delivery substitution.
- This design does not preopen a native stream, pool workers, expand queues, or
  claim that healthy native inference/partial latency meets a release target.

## Verification boundary

Controlled tests cover no-lazy admission, idle-process liveness detection,
single-generation warming, cancellation and startup failure, capability drift,
bounded reaping, healthy finalize/reset, atomic strict admission, pre-journal
rejection, and live capability rendering. Full workspace validation remains
required before this recovery item is closed.

No microphone, transcript, audio, evidence, requirement status, manifest,
package, commit, push, or release action is authorized by this decision.

## Alternatives rejected

- respawning inside `open_stream` after capture has started;
- preopening an anonymous native stream without transaction/cancellation
  ownership;
- enlarging the bounded queue without native measurements;
- keeping status green from cached capability data after process death; or
- substituting a batch/sidecar/mock engine while the resident worker warms.
