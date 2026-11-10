# ADR-0039 — Conservative VAD and truthful endpoint origin

**Status:** Recovery 4I controlled composition accepted (2026-08-28);
native quality, latency, and release qualification remain blocked

## Context

ADR-0038 deliberately retained the bounded capture safety window. A streaming
model can overlap recognition with capture, but without an endpointer the user
still waits for that window to expire. The canonical module catalog already
separates `speech.vad` from `speech.endpoint`; neither responsibility belongs
to capture, ASR, the desktop, or the delivery transaction.

An automatic endpoint is also not cancellation. Treating the two alike can
discard accepted audio and misreport a normal utterance boundary as a user
abort. Concurrent user Stop and automatic endpoint requests need one truthful,
first-writer-owned graceful-finalization reason.

## Decision

Add two independently replaceable portable contracts in `narrivy-core`:

- `VoiceActivityDetector` classifies one validated audio chunk as speech or
  silence; and
- `SpeechEndpointer` consumes that classification plus sample counts and
  decides whether to continue or gracefully finalize.

The controlled Linux 1.0 candidate is an allocation-free energy VAD with
activation/release hysteresis, paired with a sample-count endpointer. Initial
defaults require 240 ms of contiguous classified speech followed by 560 ms of
classified silence. These values are conservative candidates, not calibrated
native thresholds. The endpointer ignores initial silence, prevents separated
sub-minimum noise bursts from accumulating, resets trailing silence when
speech resumes, and emits exactly one finalization decision.

Endpointing runs on the orchestration worker after audio leaves the PipeWire
producer. It performs no model inference, file or network I/O, journaling,
delivery, or payload logging. It observes each chunk but does not trim it: the
endpoint-triggering chunk is pushed into ASR before graceful finalization is
requested. Already-buffered chunks remain ordered and are also fed to ASR.

`SessionFinalization` stores the first graceful-stop reason atomically:
`UserRequested` or `EndpointDetected`. User cancellation remains an independent
token with its existing destructive semantics. The Linux capture owner maps an
endpoint-owned request to `CaptureEnd::EndpointDetected`; core requires that
typed summary when the endpoint won the race. If user Stop won first, the
endpoint cannot overwrite its origin.

Any VAD/endpointer error uses the existing one-deadline capture/ASR teardown
and returns its typed content-free code. There is no batch, mock, alternate
VAD, or delivery fallback.

## Capability truth

Only a strict resident lane whose loaded model attests native streaming adds:

- `audio.native-energy-vad=composed:controlled_unqualified`; and
- `speech.endpoint=composed:controlled_unqualified`.

Other lanes remain `disabled:not_composed`. These strings do not claim native
speech/noise accuracy, false-endpoint rate, first/last-word preservation,
latency, accessibility, device behavior, or release readiness.

## Amendment (2026-09-30): not composed for dictation

Plan task T2.6 records the product rule: a toggle dictation stops on the
second press, a hold dictation on key release, and neither stops on silence.
The daemon therefore passes `endpointing: None` for every dictation session.
A ready streaming lane reports `audio.native-energy-vad=disabled:stop_on_request`
and `speech.endpoint=disabled:stop_on_request`. The VAD, the endpointer, their
contracts and the `EndpointDetected` origin stay in `narrivy-core`, tested and
unused, for a later opt-in mode. The capture safety limit still ends a runaway
session.

## Verification boundary

Controlled tests cover configuration and malformed audio, VAD hysteresis,
initial silence, separated noise, speech resumption, exact endpoint thresholds,
chunk-partition invariance, first-origin preservation, typed Linux endpoint
completion, both-owner failure cleanup, and delivery through the shared final
transaction. The composition test counts all ten capture chunks at ASR,
including the endpoint-triggering silence chunk.

No microphone was used. No native acoustic environment, model accuracy,
latency, pre-roll, noisy-room, speaker-playback, accent/language, or package
behavior was qualified.

## Consequences and deferred decisions

- The strict streaming lane no longer requires its fixed safety window after a
  controlled endpoint, while final-only and non-strict lanes remain unchanged.
- Energy VAD adds no dependency or model artifact and remains replaceable by a
  qualified WebRTC/neural candidate.
- Native threshold calibration, false-start/false-end measurement, pre-roll,
  multi-utterance segmentation, maximum-utterance policy, device changes,
  adaptive/noise-aware VAD, speaker-playback/AEC behavior, configuration UX,
  partial IPC/UI, and clean-machine qualification remain blocked.
- Adopting a neural VAD, changing default endpoint timing after native
  qualification, or introducing continuous segmentation requires a follow-up
  ADR because each changes dependencies, resource ownership, or user-visible
  lifecycle.

## Alternatives rejected

- infer utterance completion from ASR text or partial punctuation;
- run VAD on the PipeWire producer or audio real-time thread;
- discard the endpoint-triggering or queued audio;
- classify automatic endpointing as cancellation;
- allow the endpoint to overwrite an earlier user Stop reason;
- add a neural runtime before controlled boundary and native-quality evidence;
  or
- claim native latency improvement from deterministic fixtures.
