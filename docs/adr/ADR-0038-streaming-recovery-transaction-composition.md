# ADR-0038 — Streaming recovery transaction composition

**Status:** Recovery 4H controlled composition accepted (2026-08-28);
native behavior and release qualification remain blocked

## Context

ADR-0035 owns the model-native resident stream and ADR-0037 owns bounded live
capture, but the strict daemon still used the batch recovery transaction. A
correct composition must overlap capture and ASR without creating a second
privacy, correction, journal, or delivery implementation. It must also stop
both resource owners if either side fails while a synchronous native feed is
in progress.

Partial hypotheses cannot be treated as durable transcript truth. Native event
ordering, capture continuity, and cleanup outcomes must be validated before a
final transcript enters the established recovery transaction.

## Decision

Add `StreamingRecoverySlice` to portable core and select it only when all of
the following are true:

- the daemon is running one of the explicit strict Linux microphone recovery
  lanes;
- the configured ASR runtime is the already-initialized resident worker; and
- that loaded model attests `capabilities().streaming == true`.

A resident final-only model continues to use the existing batch slice. The
sidecar remains batch-only. A failure after streaming selection never retries
through batch ASR, a mock, or another delivery path.

The transaction orders work as follows:

1. create/update the opaque pending journal entry;
2. enforce the local raw-audio policy before opening capture or feeding ASR;
3. open the resident ASR stream under the transaction cancellation token;
4. start bounded capture, establish real capture readiness, then publish
   `Listening`;
5. validate and synchronously feed each contiguous capture chunk while
   accepting ASR `Started`/`Partial` events only in strict sequence;
6. on capture End, publish `FinalizingAudio` then `Transcribing`, finalize ASR,
   boundedly finish it, and require exactly one ordered final transcript; and
7. pass only that final transcript into the same raw-policy, deterministic
   correction, durable intent, delivery, and terminal-journal code used by the
   batch slice.

Capture format must match its opened stream identity, chunks must be nonempty
and contiguous, totals and first-frame measurements must agree with the final
summary, and ASR event sequences must strictly increase. A partial or premature
final never enters the journal, correction, delivery, ordinary logs, or
evidence.

## Cancellation and cleanup

User cancellation is captured before internal teardown so it remains a clean
`Cancelled` result only when the caller-owned token was already cancelled, the
component returned its typed `cancelled` outcome, and cleanup succeeds. A
component string/code alone cannot manufacture user cancellation. Internal
capture/ASR failures retain their original typed code unless cleanup itself
fails.

Either-side failure creates one absolute two-second cleanup deadline. ASR is
signalled immediately, capture abort/reap/join is attempted, and ASR finish is
then attempted with the remaining time. Both attempts occur even if the first
reports failure. A cleanup failure takes precedence over the initiating error;
Drop is not accepted as proof of cleanup.

Lifecycle observer rejection follows the same cleanup rule before the error
returns. Policy denial occurs before either owner is opened.

## Capability truth

Status separates three facts:

- model-native streaming support;
- whether the strict daemon composition exists; and
- release/native qualification.

For a strict resident lane with a streaming model, the product string is
`asr.transcribe-cpp.streaming=composed:controlled_unqualified`. Unsupported,
uninitialized, and non-composed cases remain explicitly disabled. This is not
a readiness, latency, accuracy, native-device, or release claim.

## Consequences and deferred work

- ASR computation can overlap capture, removing the architectural requirement
  to wait for the entire capture window before starting recognition.
- The fixed 80-ms/eight-slot bounds and earlier 4F compute observations are
  construction facts, not an end-to-end latency qualification.
- VAD/endpointer ownership and single-utterance automatic finalization are now
  specified by ADR-0039. Native calibration and continuous segmentation,
  partial IPC/UI presentation, hotplug/routing recovery, native
  microphone accuracy/latency/loss/duplication/RSS/thermal tests, crash-dump
  and swap containment, model-pack lifecycle, package validation, evidence,
  and release acceptance remain blocked.

## Alternatives rejected

- copy the downstream recovery logic into the daemon;
- persist, correct, deliver, or log partial hypotheses;
- silently accept capture gaps, mismatched summaries, or stale ASR events;
- wait for capture cleanup before signalling ASR cancellation;
- retry a failed stream through batch/mock ASR; or
- report streaming as ready merely because the model capability bit is true.
