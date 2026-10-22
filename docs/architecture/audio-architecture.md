# Audio Architecture

## Capture path

Audio capture uses a bounded lock-free or low-contention ring-buffer strategy appropriate to the platform implementation. The real-time callback must never perform model inference, filesystem access, network access, or long locks.

Recommended logical pipeline:

`Capture → normalize/canonicalize once → optional enhancement → VAD/endpoint → ASR consumer`

## Bounded dictation window

One live dictation session is Stop-driven and bounded. `narrivy-types` owns the
portable contract: 16 kHz mono f32, `MAX_CAPTURE_SECONDS = 300` and the derived
`MAX_CAPTURE_SAMPLES` (4,800,000 frames, i.e. 19.2 MB per full staging copy).
The same frame budget is enforced by the Linux capture adapter, portable
recovery finalization, the resident IPC protocol, parent stream accumulation and
the resident worker, so no boundary can silently accept or truncate a longer
utterance. Meeting-length capture (FUN-100) is a separate post-1.0 capability
and must not raise this bound implicitly.

The daemon owns the policy: production composition uses the documented maximum
and ends on Stop, endpoint or cancel; `NARRIVY_CAPTURE_SECONDS` is a controlled
short-window developer probe (1..30 s) that may only shorten one session.
Capture implementations must not add their own independent limit, and clients
render the daemon-reported `capture.window_seconds` / `capture.max_seconds`
capabilities rather than defining an OS-side window.

Every finalized capture reports a typed, content-free `CaptureEnd`
(`finalization_requested`, `endpoint_detected`, `safety_limit`,
`source_ended`). `safety_limit` is an explicit termination at the documented
bound with the complete expected frame count, never a silent clip; an early
`source_ended` or a frame-count mismatch fails closed. The reason is journaled
as durable transaction truth before ASR/correction/delivery and is surfaced to
clients. See ADR-0071.

## Pre-roll
Maintain a short RAM-only pre-roll buffer to prevent clipped first phonemes.

## Audio policy
Linux initially supports PipeWire-aware `leave`, `duck`, `pause`, `mute`, and later `smart`. Use event-driven PipeWire graph observation so newly created streams are handled.

## Device lifecycle
Support ranking, hotplug, fallback and explicit user pinning. Never silently switch to a clearly lower-trust input source if policy says microphone must be pinned.
