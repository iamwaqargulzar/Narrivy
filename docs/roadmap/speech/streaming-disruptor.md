# Streaming Disruptor — Truthful capability split

> **Status: roadmap / not in 1.0.** This describes a planned design, not current behaviour. See [the roadmap](../README.md).

Narrivy distinguishes three separate facts:

1. the CLI sidecar and final-only models emit only one `Final`;
2. protocol v3 and the resident engine bind transcribe.cpp's genuine native
   stream lifecycle for a model that attests streaming support; and
3. strict recovery now has a controlled live-capture and conservative
   VAD/endpointer composition, while native acoustic/latency qualification,
   daemon/client partial routing, and accessible desktop UX remain blocked.

`StreamingSupport` therefore stays capability-driven. Repeated final-window
decode is not a substitute for native Partial events, and a native model bit is
not sufficient to claim PERF-005. Recovery 4E validates sequence/revision,
ownership, cancellation, reset, and malformed-frame behavior at the isolated
engine boundary. Native-model timing/accuracy and the live product path remain
separate qualification work.

Recovery 4I does not infer endpoints from ASR text. Its energy VAD and
sample-count endpointer are replaceable module boundaries, run outside the
capture producer, retain every accepted audio chunk, and report
`controlled_unqualified` until native calibration exists.

Legacy `stream_partial()` stubs continue returning
`streaming_not_yet_supported`; they are rollback/test surfaces, not the
resident protocol. Consumers must use monotonic event sequences and discard
stale transaction identities regardless of engine capability.
