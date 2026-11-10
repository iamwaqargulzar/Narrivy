# ADR-0071: Stop-controlled bounded dictation capture and typed capture end reason

- Status: accepted for recovery; release qualification pending
- Date: 2026-09-15
- Phase: P11 recovery, release task 007 (usable short and long dictation capture)

## Context

Ordinary toggle/manual dictation could not capture a long passage:

- `EngineConfig.pipewire_capture_seconds` defaulted to `3.0` seconds — a
  controlled-recorder probe value, not a product window. Every strict
  composition passed that default straight into
  `narrivy_platform_linux::recovery::PipeWireCapture::new`.
- The `NARRIVY_CAPTURE_SECONDS` developer hook accepted only `1..=30` and both
  the `PipeWireCapture` constructor and its `capture_frame_budget` independently
  rejected any window outside `1..=30`. The installed batch path therefore could
  not be configured to record longer than thirty seconds even by an operator.
- Independently, the private resident protocol pinned
  `MAX_SAMPLES = 16_000 * 30` and the same bound was enforced by protocol header
  decoding, the parent batch validation/stream accumulation in `resident.rs`,
  and the worker's decode/feed handling. Raising only the capture window would
  still have made a real resident worker reject the samples after capture.
- The typed `CaptureEnd` reason already existed on the platform API, but the
  batch recovery slice took `captured.audio` and discarded `captured.end`, and
  the streaming slice validated endpoint consistency and sample accounting
  without carrying a safety-limit reason into durable finalization. The user
  therefore could not be told why recording ended, and platform `SafetyLimit`
  unit tests could not prove that they were.

Audio capture must stay bounded: one transaction's frames are staged in memory
and sent to the resident worker over a private protocol, so an unbounded window
is an unbounded memory/IPC commitment.

## Decision

1. **One portable, documented capture bound.** `narrivy-types` owns
   `CAPTURE_SAMPLE_RATE_HZ = 16_000`, `MAX_CAPTURE_SECONDS = 300` (five
   minutes) and the derived `MAX_CAPTURE_SAMPLES = 4_800_000` frames. The
   documented per-transaction staging budget is
   `MAX_CAPTURE_SAMPLES * size_of::<f32>() = 19,200,000` bytes per full mono
   f32 buffer copy. Five minutes comfortably exceeds ordinary dictation while
   keeping one session's frames, IPC payload and resident-ASR staging finite;
   meeting-length capture (FUN-100) is post-1.0 and must not raise this bound
   implicitly.
2. **Production default is Stop-driven at the bound; the developer hook may
   only shorten.** `EngineConfig::pipewire_capture_seconds` defaults to the
   documented maximum. Ordinary dictation ends when the user requests Stop,
   when endpointing fires, or when the user cancels; the bound only guarantees
   that a forgotten session cannot grow without limit. `NARRIVY_CAPTURE_SECONDS`
   is reclassified as a controlled developer-probe override and validated by
   `apply_developer_capture_seconds` within `1..=DEVELOPER_CAPTURE_SECONDS_MAX`
   (30 s). It can shorten one session for deterministic fixtures and can never
   extend capture past the documented bound.
3. **The adapter accepts exactly `[1, MAX_CAPTURE_SECONDS]`.** Both the batch
   `PipeWireCapture` and the producer-owned streaming session accept any
   whole or fractional window inside that range
   (`max_capture_seconds()` is the single source), and the recorder argv frame
   count is `round(seconds * 16_000)` with `<= MAX_CAPTURE_SAMPLES` enforced
   before any process side effect. Invalid, non-finite, non-positive or
   over-bound windows fail typed (`capture_duration_invalid`) with no spawn.
4. **One shared frame bound across every boundary.** The resident protocol's
   `MAX_SAMPLES` becomes `narrivy_types::MAX_CAPTURE_SAMPLES` rather than a
   transport-local 30-second constant. Protocol header decoding, parent batch
   validation, stream accumulation, worker decode/feed and the portable core
   slices all enforce that same finite budget. The wire format is unchanged:
   the existing `u32` sample-count field is not modified, so compatibility is a
   peer-capability decision — a peer built against the smaller historical bound
   rejects a larger frame fail-closed (`invalid_audio` / `audio_too_large`)
   instead of truncating it. The resident decode deadline scales with audio
   length (`decode_budget`) with the existing floor and a bounded maximum, so a
   five-minute utterance is not decoded under a short-utterance timeout.
   `narrivy-core` additionally fails closed with `capture_output_overflow`
   before ASR if a misbehaving capture port exceeds the portable bound, and the
   streaming slice terminates with `capture_sample_count_overflow`.
5. **The typed end reason is durable transaction truth.** `CaptureEnd` moves to
   `narrivy-types` (still re-exported by `narrivy-platform-api`) and is
   serde-serializable. Batch and streaming finalization record it on the
   journal entry before ASR/correction/delivery can run, so a later failure
   still tells the user how recording ended. The four reasons keep their
   meaning:
   - `FinalizationRequested` — graceful user Stop; the buffered tail is kept;
   - `EndpointDetected` — independent endpointing ended the utterance;
   - `SafetyLimit` — the documented bound was reached and **every** expected
     frame arrived from a recorder that exited success or its exact
     budget-exit code; this is explicit termination, never a silent clip;
   - `SourceEnded` — the source ended before the bound or the frame count did
     not match; the slice fails closed and returns no partial audio.
   Existing behavior is unchanged: a mismatched frame count, a failed recorder
   exit, a poisoned stream or an overflow still fails typed rather than being
   reported as success.
6. **The reason reaches the client additively.** `GetRecoveryEntry` gains an
   optional `capture_end` field on its payload (protocol minor 6, with the new
   `RECOVERY_ENTRY_CAPTURE_END_PROTOCOL_MINOR`). The field is additive and
   optional, so older peers keep parsing the payload and simply do not render
   how capture ended. `StatusDto` capability strings publish the effective
   window (`capture.window_seconds=…`) and the documented bound
   (`capture.max_seconds=…`) separately, so a client can distinguish a shortened
   controlled probe from production composition.
7. **The frontend renders, it does not set policy.** The Dictation screen reads
   the reported `capture.window_seconds`/`capture.max_seconds` capabilities,
   renders the active limit, and renders the typed `capture_end` notice for
   `safety_limit`/`source_ended` after a session. It never invents a limit and
   never lengthens or duplicates an OS-side capture window. A missing or
   unrecognized report renders nothing rather than a guess.

## Alternatives considered

- **Raise only the adapter window to 300 s and leave the resident protocol at
  30 s.** The user-facing batch path would capture 300 s and then either fail
  in the resident worker or require silent truncation. Rejected: it moves the
  failure to the least truthful place.
- **Implement daemon-owned segmentation of long captures.** Correct long-term
  direction for meeting-length capture, but it changes correction/finalization
  and cancellation semantics for ordinary dictation and is explicitly FUN-100
  scope. Rejected for this repair.
- **Keep the 3-second default and require an explicit user setting.** The
  product behavior under test is ordinary toggle dictation; a user who speaks
  for longer than three seconds must not be cut off by default.
- **Make the maximum unbounded and rely on Stop alone.** Unbounded memory and
  IPC payload cannot be accepted, and a lost Stop must not create an
  unbounded session.
- **Reinterpret `SourceEnded` as a successful finalization.** Fails closed
  semantics are the safety property; the typed failure and durable reason are
  the truthful outcome.

## Consequences

- A user can dictate for well beyond thirty seconds and finalize with Stop; the
  complete bounded tail is retained and reaches ASR exactly once.
- Reaching the five-minute bound is explicit: finalization reports
  `SafetyLimit`, all 4.8 M frames are delivered, the reason is journaled and the
  client renders it. No silent clip at three or thirty seconds remains.
- Cancel and daemon shutdown keep their existing semantics: the owned recorder
  process group is terminated and reaped, and no partial capture is published.
- Memory and IPC stay bounded by a documented finite budget (19.2 MB per full
  staging copy at the maximum), and extreme/invalid windows are rejected before
  a process side effect.
- Compatibility: the resident protocol wire shape is unchanged; a peer built
  against the historical bound rejects larger frames fail-closed. `GetRecoveryEntry`
  payloads are backwards compatible; protocol minor 6 documents the additive
  `capture_end` field.
- Batch Whisper unsupported-streaming truth is unchanged, and the streaming
  endpoint/VAD path keeps its configured semantics; long batch capture is not
  reclassified as streaming.
- The regressions are deterministic controlled-fixture and portable-slice
  tests (paced recorder helpers and a counting ASR spy), not native microphone,
  installed model, delivery or release qualification.

## Verification

- `adapters/platform-linux`: the documented maximum window is accepted and
  extremes are rejected before side effects; a paced batch capture of 40 s of
  samples finalizes with `FinalizationRequested` retaining every sample; a
  recorder that writes exactly the maximum frame count terminates explicitly
  with `SafetyLimit` and all 4 800 000 samples; a streaming window above 30 s
  delivers every sample once with contiguous sequences and an explicit
  `SafetyLimit` summary.
- `narrivy-core`: a 40-second batch capture reaches ASR with the exact sample
  count, records `CaptureEnd::SafetyLimit` durably and still delivers; an
  over-bound capture fails closed with `capture_output_overflow` before ASR
  while still recording the end reason; the existing streaming success test
  asserts the durable end reason.
- `narrivy-types`, `narrivy-ipc`, `narrivy-ipc-unix`: bound constants, the
  snake_case wire tag round-trip, the additive optional field (legacy payload
  decodes with `capture_end = None`), and the pinned protocol minor.
- Daemon policy: production default and capability reporting, and the
  short-probe override rejecting extremes and non-finite input.
- Frontend: capture-policy parsing/rendering regressions over daemon-reported
  capabilities and typed end reasons.

These are controlled fixtures and portable-slice checks. Native microphone
capture, installed-model transcription, delivery and release qualification
remain open.
