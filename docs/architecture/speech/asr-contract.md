# ASR Engine Contract

The ASR contract expresses speech-recognition semantics and capabilities without leaking OpenVINO/ONNX/model SDK types into core.

## Required concepts

### Engine descriptor
- stable engine ID/version;
- supported model formats/families;
- device classes and runtime constraints;
- streaming/partial capability;
- timestamp/word-timestamp capability;
- confidence/span-confidence capability;
- context bias/prompt capability;
- language/task capability;
- cancellation behavior.

### Session request
- transaction ID;
- model identity/revision;
- preferred/allowed device policy;
- language/task options;
- audio format or normalized stream contract;
- optional policy-approved decoding context;
- cancellation token/deadline;
- streaming/final preferences.

### Events
At minimum: `Started`, `Partial`, `Final`, `Cancelled`, `Degraded`, `Failed`. Every event carries transaction identity and sequence/order metadata. Text content remains application data and is excluded from generic logs.

`Partial`/`Final` may include typed optional spans/timestamps/confidence only when the engine can make a meaningful guarantee. Do not manufacture confidence values to satisfy a UI.

## Cancellation/stale output

Cancellation must attempt to stop compute promptly. Regardless of runtime cancellation support, core uses transaction identity to discard any result that arrives after the session has been cancelled/superseded.

## Device fallback

The ASR API does not silently choose random devices. The compute/model policy supplies allowed candidates; engine health determines feasibility; fallback emits a typed degradation event/reason. A device/model pair that failed qualification is not retried indefinitely in the same session.

## Audio ownership

Audio capture real-time code and inference run on different execution contexts with bounded buffering/backpressure. Define ownership/copying explicitly so the real-time path does not block on model inference.

## Scaffold warning

Any Phase-0 synchronous `transcribe(&AudioChunk)` trait is intentionally provisional and should be replaced in P1/P5 with an event/cancellation-capable contract.
