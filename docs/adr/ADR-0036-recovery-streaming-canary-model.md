# ADR-0036 — Recovery streaming canary model

**Status:** Recovery 4F candidate accepted for controlled qualification
(2026-08-28); not a default, package asset, or release-qualified model

## Context

Protocol v3 and the resident engine require a model that natively attests
streaming support. The existing Whisper tiny recovery model is final-only.
Larger Parakeet Unified and Nemotron streaming candidates add roughly 700 MB
Q8 artifacts and family-specific latency options; they are useful later
comparators, but not the smallest way to validate Narrivy's new boundary.

The exact `transcribe-cpp` 0.2.2 source uses Moonshine Streaming Tiny in its
own Rust smoke path and reports native 16 kHz, cancellation, and streaming
support. Default stream options require no family-specific extension.

## Decision

Use `moonshine-streaming-tiny-Q8_0.gguf` only as Recovery 4F's minimal native
streaming canary:

- artifact repository: `handy-computer/moonshine-streaming-tiny-gguf`;
- immutable repository revision:
  `85ddff612fa3a2cf40b2f745abcfa90ef82f293b`;
- file size: `50,462,816` bytes;
- LFS SHA-256:
  `930e4622ad3a24158b91406c30c977fa6a26b34cb32d6ac3e57cfb23383a869e`;
- original checkpoint revision:
  `f8e9dfd8c562c257c151a907b7b7f2fe8ff8511a`;
- weights license: MIT;
- language/features: English transcription only; no VAD/endpointer,
  timestamps, translation, or language detection; and
- native cadence: 16 kHz mono f32, recommended 80 ms feeds, approximately
  240 ms cumulative right context, and a 20 ms natural emit unit.

The artifact is acquired from the immutable URL into the user model cache via
a mode-0600 `.part`, hash/size verified, then atomically renamed. The engine
still never downloads implicitly. The cache file is intentionally retained as
a verified local candidate and is not a repository or release artifact.

## Controlled native observation

The exact artifact loaded in the current resident worker, attested
`supports_streaming=true`, and completed begin/feed/finalize/shutdown over
protocol v3. Synthetic silence produced valid empty snapshots and exit 0.

The upstream public JFK WAV was then fed from memory in 138 80-ms chunks. It
produced 31 changed native updates and a final snapshot without printing or
persisting transcript text. The first changed update followed chunk 9 (720 ms
of supplied audio). Accelerated-feed compute time to that update was about
180 ms; this is not real-time microphone or PERF-005 latency. Feed-call compute
was 15.54 ms p50, 165.42 ms p95, and 279.04 ms max; finalize was 145.01 ms.
These are one non-release host observation, not a benchmark qualification.

## Boundaries and deferred work

- The canary is not the product default or a catalog-qualified model pack.
- Fixture accuracy, final text, microphone latency, RSS, thermal behavior,
  noise, duplication, and long-session behavior are not qualified.
- Narrivy must own bounded capture buffering, backpressure, VAD/endpointing,
  explicit Stop/finalize, stale-output rejection, and partial-event routing.
- The current 30-second engine cap needs deliberate segmentation semantics.
- Package install/upgrade/removal, offline acquisition policy, model registry,
  core-dump/swap containment, and clean-machine validation remain blocked.

## Alternatives rejected

- relabel the final-only Whisper model as streaming;
- repeated final-window decode as simulated partials;
- select a ~700 MB model before validating the smallest native contract; or
- use a floating `main` model URL or load before integrity verification.

## 2026-09-21 dependency refresh

The runtime is now transcribe.cpp 0.2.3. The prior 0.2.2 canary observations are
historical and do not transfer; any release claim requires a fresh exact-source
native run.
