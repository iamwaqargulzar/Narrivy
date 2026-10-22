# ADR-0037 — Bounded live capture transport

**Status:** Recovery 4G adapter boundary accepted for controlled validation
(2026-08-28); composed by ADR-0038, still not native/release-qualified

## Context

The resident ASR worker can now execute a genuine model-native stream, but the
strict recovery pipeline still records an entire utterance before invoking
ASR. The existing P4 monitor writes to an overwrite-style pre-roll ring. That
behavior is appropriate for pre-roll but cannot prove continuous ASR input:
old samples may be replaced when a consumer is slow.

Stop and Cancel also need to reach capture while the orchestration worker is
synchronously waiting for a native ASR feed. Observing controls only when the
consumer asks for another chunk would add ASR compute time and queued-audio
depth to recorder shutdown.

## Decision

Add a portable streaming-capture boundary alongside, not in place of, the
existing batch capture API:

- `StreamingAudioCapturePort::start_stream` binds the transaction's
  `SessionControl` when the producer is created;
- `StreamingAudioCaptureSession` exposes readiness, ordered audio/end events,
  and explicit deadline-bound abort for downstream failures;
- chunks carry contiguous Narrivy sequence numbers and canonical 16 kHz mono
  f32 audio; and
- the terminal summary contains only end reason, sample count, and monotonic
  timing.

The Linux recovery adapter uses a dedicated non-real-time parser worker and a
fixed `sync_channel` of 64 1,280-sample chunks. This bounds queued payload
to 320 KiB (plus fixed owner/allocator overhead) and represents 5.12 s at the
80-ms cadence. These values are implementation bounds, not qualified
latency targets. (Amended 2026-09-30: the bound was eight chunks, 640 ms,
sized for the tiny streaming canary; with a 0.6B streaming model on the CPU
a step can exceed that, which ended dictations as `capture_backpressure`.
The queue is still fixed and saturation is still terminal.)

The producer does only nonblocking pipe read, finite-f32 decode, chunk assembly,
control observation, and nonblocking queue submission. It creates no payload
file or log and performs no inference, policy, correction, delivery, or network
work. A full queue is terminal `capture_backpressure`; saturation is never
silently treated as a shorter successful stream. Stop sends the
already-accepted partial chunk, stops
accepting new bytes, and terminates the recorder. Cancel discards pending audio
and terminates the recorder. Cancellation is rechecked at return boundaries.

The session retains ownership of the child and parser join handle. Normal End
is returned only after parser join and child reap. Downstream ASR failures must
call `abort` with their existing absolute deadline; Drop is last-resort cleanup,
not evidence of successful cleanup.

## Security and lifecycle notes

The producer signals only the still-parent-owned child PID. Because the parent
has not reaped that child, Linux cannot reuse the PID before this owner observes
termination. The parent remains solely responsible for `wait`/reap.

No transcript exists at this boundary. Audio remains in process memory and OS
pipes only. Linux core-dump and swap containment are not solved by this ADR and
remain explicit release blockers.

## Consequences and deferred work

- The batch recovery lane remains unchanged and truthful.
- The legacy pre-roll monitor is not promoted to lossless ASR transport.
- Recovery 4G alone did not enable product streaming. ADR-0038 now owns the
  fail-closed core/daemon transaction and capability distinction.
- VAD/endpointer, pre-roll admission, hotplug/fallback, 30-second segmentation,
  partial UI events, native microphone latency, RSS/thermal/accuracy/noise
  qualification, evidence, and packaging remain deferred.

## Alternatives rejected

- use the overwrite pre-roll ring as an ASR queue;
- use an unbounded channel;
- silently drop oldest/newest chunks under load;
- observe Stop/Cancel only after synchronous ASR returns;
- detach parser or cleanup threads; or
- enable streaming status from model capability before daemon composition.
