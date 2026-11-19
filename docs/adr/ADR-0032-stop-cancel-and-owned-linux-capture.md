# ADR-0032 — Separate Stop from Cancel and own Linux capture

**Status:** Recovery 4A controlled implementation complete (2026-08-28); no release
requirement or native qualification is implied

## Context

The strict Linux recovery lane previously recorded a fixed-duration WAV in a
temporary directory and used the same cancellation flag for `StopSession` and
`CancelSession`. A user Stop therefore discarded audio instead of finalizing
the utterance. The recorder was also observed as `Listening` before the
concrete source had produced a frame. Elsewhere, the general PipeWire monitor
dropped a parser thread handle and decoded each single frame into a new vector
under a shared lock.

Those behaviors are both slow and semantically false. Stop must preserve audio
already accepted and continue through local policy, ASR, correction, durable
intent, and delivery. Cancel must stop and reap all owners, prohibit later
stages, and write no delivery. Merely spawning `pw-record` proves neither
permission nor source readiness.

Upstream implementations support a lower-latency direction, but do not remove
the need for truthful local contracts. The pinned transcribe.cpp v0.2.1 Rust
binding exposes an owned `Model`/`Session`, direct 16 kHz mono `f32` input,
stream/cancellation APIs, and capability queries. Its documentation also says
that `Model` is shared while a `Session` is mutable and that a model serializes
in-flight runs. whisper.cpp's microphone example uses periodic inference and
offers a basic VAD sliding-window mode. nerd-dictation keeps its recognizer
resident and exposes distinct `end` and `cancel` commands. These are design
inputs, not evidence that Narrivy implements equivalent behavior.

## Decision

1. Portable session control has two independently shared signals:
   finalization and cancellation. Stop requests finalization. Cancel requests
   cancellation. Cancellation wins a concurrent observation race.
2. `AudioCapturePort` is a factory for one owned `AudioCaptureSession`.
   Process spawn and capture readiness are separate operations. The session is
   ready only after the selected source yields one complete frame, which is
   retained for collection.
3. The strict Linux adapter invokes `pw-record` with an explicit PipeWire
   default-source selection, raw 16 kHz mono f32 output on a pipe, a bounded
   maximum frame count, null stdin, and non-content stderr handling. It does
   not create a WAV or audio temporary file.
4. The adapter owns the child and stdout for their full lifetimes. It decodes
   fixed-size batches with bounded carry, rejects non-finite, partial, and
   overflowing output, and uses one absolute cleanup deadline without a
   blocking `wait` fallback. A typed termination timeout is never upgraded to
   `cancelled` merely because the cancellation flag is set.
5. A dynamic selection expression such as `@DEFAULT_AUDIO_SOURCE@` is reported
   as `source_selection`, not as a resolved hardware identity. Resolved device
   identity requires a separate truthful capability contract.
6. `Listening` is published only after readiness. `FinalizingAudio` is
   published after capture collection and before raw-audio policy/ASR.
7. The general Linux monitor follows the same ownership rules: retained join
   handle, bounded child reaping, batched decoding, and no detached parser.
8. The current one-shot transcribe.cpp sidecar remains in Recovery 4A. A
   resident in-process model/session is a separate Recovery 4B decision and
   implementation because it changes dependency, unsafe/FFI, packaging,
   concurrency, cancellation, and model-lifecycle boundaries.
9. Recovery stage observation is fallible. A rejected daemon transition is
   persisted as `state_observation_failed` and stops orchestration before the
   corresponding side effect. Before commit this is terminal; after durable
   corrected text and delivery intent it is recoverable.
10. The retained one-shot sidecar has no unbounded cleanup fallback. Cancel,
    wait failure, and decode timeout kill and reap under the explicit cleanup
    bound; inability to do so is a typed termination failure.

## Consequences

- The strict path no longer writes a capture WAV and can finalize as soon as a
  Stop request is observed, removing the fixed-window wait from normal use.
- A successful Stop can continue to delivery; a successful Cancel cannot.
- Startup/source failure cannot produce a false `Listening` event.
- Rejected state publication cannot permit unreported ASR or delivery work.
- Maximum-window completion remains a safety fallback, not VAD or genuine
  streaming ASR.
- `pw-record` remains a process boundary and the model remains cold-loaded per
  utterance, so the largest remaining latency reduction belongs to Recovery
  4B. No streaming capability is advertised from this change.

## Required controlled verification

- finalization after readiness preserves exact prefetched samples and reaps
  the recorder;
- cancellation before/after finalization observation produces `cancelled`, no
  ASR/delivery, and no child;
- cleanup timeout remains a typed failure rather than a clean-cancel claim;
- split raw frames decode exactly; non-finite, overflow, trailing partial, and
  unexpected early source termination fail closed;
- strict state events remain `Preparing` until real frame readiness, then
  proceed through finalization and later stages in order;
- daemon Stop and Cancel exercise distinct shared signals;
- controlled state-transition rejection stops before ASR/delivery and leaves
  the correct durable terminal or recoverable journal state;
- sidecar cancellation reaps under its deadline with no blocking `wait` escape;
- ordinary logs contain no audio or transcript, and capture creates no payload
  file;
- focused and workspace checks pass without evidence, manifest, requirement,
  package, commit, or push changes.

## Deferred ADR decisions

- resident ASR ownership was resolved as a process-isolated worker by ADR-0033;
- model-worker cardinality and queue policy given per-model run serialization;
- model-specific streaming/VAD capability selection and partial-transcript UX;
- resolved PipeWire device identity and device-change semantics;
- end-to-end monotonic latency schema across capture, ASR, policy, and delivery.

## Upstream design references

- transcribe.cpp v0.2.1 safe Rust binding and lifecycle:
  <https://github.com/handy-computer/transcribe.cpp/blob/v0.2.1/bindings/rust/transcribe-cpp/README.md>
- transcribe.cpp v0.2.1 C API contract:
  <https://github.com/handy-computer/transcribe.cpp/blob/v0.2.1/include/transcribe.h>
- whisper.cpp real-time and VAD example:
  <https://github.com/ggml-org/whisper.cpp/blob/master/examples/stream/README.md>
- nerd-dictation resident recognizer and distinct end/cancel behavior:
  <https://github.com/ideasman42/nerd-dictation/blob/main/readme.rst>
