# ADR-0034 — Native model streaming-capability attestation

**Status:** Recovery 4D capability decision retained; protocol-v2 framing was
superseded by protocol v3 in ADR-0035 (2026-08-28). Non-release and no native
streaming qualification.

## Context

transcribe.cpp 0.2.2 provides both an immutable model capability
`supports_streaming` and a genuine session-borrowing stream API. Support is
model-specific: the current Whisper tiny recovery model is final-only, while
families such as Moonshine Streaming and particular Parakeet variants can
advertise streaming.

Narrivy previously reported only a compile-time final-only product capability.
That prevented a false product claim, but it also discarded authoritative model
metadata needed to select a future streaming lane. Inferring support from a
filename or family catalog would be unsafe and stale.

## Decision

1. Resident protocol v2 readiness carries exactly one capability byte. Bit 0
   is the loaded native model's `supports_streaming`; all other bits are
   reserved and rejected.
2. The worker populates the bit only after successful model load plus existing
   CPU, sample-rate, ABI/version, and cancellation checks. A missing/malformed
   payload or unknown bit is a startup protocol failure.
3. The parent stores the initial capability immutably. A replacement worker
   must attest the same capability set; drift is a typed failure and the
   replacement is terminated/reaped.
4. Status exposes two independent facts:
   `asr.transcribe-cpp.model-streaming=supported|unsupported` describes the
   loaded model, while
   `asr.transcribe-cpp.streaming=disabled:not_composed` describes Narrivy's
   current product path.
5. The capability bit does not enable stream operations, partial events,
   capture overlap, VAD, endpointing, delivery, UI, or a requirement. The
   existing one-shot decode behavior and final-only public binding remain
   unchanged.

## Privacy and lifecycle

The capability byte contains no model path, model identity, audio, transcript,
or user content. It crosses only the daemon-owned private worker pipes. Startup,
failure, restart, shutdown, and reaping retain ADR-0033's absolute bounds and
ownership. Ordinary logs contain neither the protocol frame nor payload data.

## Consequences

- Capability reporting is based on the loaded native model rather than names or
  catalog assumptions.
- A streaming-capable model can be detected without falsely advertising a
  usable streaming pipeline.
- Older protocol workers fail the exact current version check rather than being
  interpreted ambiguously; worker and daemon must ship as one package later.
- Model replacement at the configured path is still a package-integrity
  blocker. Capability consistency detects one form of drift but is not a model
  digest or package-ownership substitute.

## Required next ADR before streaming composition

The next streaming ADR must define:

- model/language/license/integrity selection and fallback policy;
- begin/feed/finalize/reset worker framing and bounded response snapshots;
- live PipeWire chunk ownership, queue bounds, backpressure, and worker
  admission for the full stream lifetime;
- cancellation/timeout/reap behavior during blocked native feed/finalize;
- native revision mapping, committed/tentative rewrite semantics, and stale or
  duplicate partial rejection;
- VAD/endpointer ownership and explicit Stop interaction;
- privacy/correction/delivery ordering so partial text is never persisted or
  delivered as final;
- desktop accessibility and failure UX; and
- controlled, native, clean-machine, latency, accuracy, resource, and package
  qualification.

## Verification

- supported and unsupported capability handshakes;
- malformed/unknown capability rejection;
- capability mismatch after worker restart;
- status never promotes native support to product support;
- real installed Whisper model reports unsupported and exits cleanly; and
- existing decode, cancellation, timeout, restart, privacy, and governance
  checks remain green/fail-closed as appropriate.

## References

- transcribe.cpp 0.2.2 Rust `Model::capabilities` and `Session::stream` APIs;
- [Moonshine Streaming model documentation](https://github.com/handy-computer/transcribe.cpp/blob/main/docs/models/moonshine-streaming.md);
- [Parakeet model documentation](https://github.com/handy-computer/transcribe.cpp/blob/main/docs/models/parakeet.md).

## 2026-09-21 dependency refresh

Version 0.2.3 retains the capability and stream contracts used by this ADR and
fixes buffered Parakeet end-of-stream tail loss. Qualification must still prove
the exact model capability; the version update does not promote streaming by
itself.
