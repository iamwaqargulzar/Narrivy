# Release task 007: usable short and long dictation capture

DeepSeek V4.1 Flash at provider reasoning=max implements; Codex independently reviews/tests/accepts. Read authoritative repository rules, architecture/catalogs, active P11 scenario requirements and capture contracts. Use apply_patch. No commits/pushes, host changes, secret access, fabricated evidence, qualification promotion or weakened security.

Repair the real installed batch microphone path's three-second default window and one-to-thirty-second hard capture budget. A user starting ordinary toggle/manual dictation must be able to speak a long passage and finalize with Stop, with Cancel discarding per existing recovery semantics. Keep a documented bounded maximum and explicit truthful end reason/failure UX at the maximum; do not just silently clip at three/thirty seconds or introduce unbounded capture/memory. Inspect batch and streaming capture/session controls and factor a correct production capture policy separately from short developer fixture controls where needed. Resolve timing/buffering policy in the daemon/platform boundary; frontend renders reported limit/state, no OS policy duplication. Document persistent/config/API changes with an ADR and migrate callers if needed.

Preserve trusted PipeWire helper execution, private/fileless capture, bounded queues and memory, sample identity/order and sequence zero, sample/frame accounting, no producer loss hidden as success, graceful Stop tail behavior, cancellation responsiveness, recorder-process ownership/reap and truthful capture readiness. Batch Whisper unsupported-streaming truth must remain; long batch capture is not fake streaming. Streaming-model endpoint/VAD paths must keep their configured semantics. Maximum termination must preserve all recoverable finalized text and surface how capture ended.

Meaningful regressions: input over thirty seconds of equivalent sample count reaches ASR without clipping, Stop before limit retains complete bounded samples, Cancel/daemon shutdown reaps capture, overflow/producer failure fails closed, maximum end is explicit, memory sizing rejects invalid/extreme inputs, and short developer probes remain bounded. Use deterministic clock/controlled paced helpers rather than thirty-second sleeps in unit tests; distinguish controlled fixtures from native mic acceptance. Run focused default/resident-feature tests and strict Clippy, frontend tests/build if affected. Stop after concise exact checks and remaining native blockers.

Codex end-to-end source-review map (recheck final source): EngineConfig in
daemon/policy.rs defaults pipewire_capture_seconds=3.0 and validates the
developer NARRIVY_CAPTURE_SECONDS override within1..30. Capture mechanics are
adapters/platform-linux/src/capture.rs. Independently of capture, the resident
protocol currently defines MAX_SAMPLES=16000*30 in resident_protocol.rs;
protocol header decoding, parent batch validation and stream accumulation in
resident.rs, and worker decode/feed handling in
src/bin/narrivy-transcribe-worker.rs all enforce that bound. Raising only
capture duration cannot satisfy long dictation: a real resident worker would
reject the samples after capture. Evolve these boundaries coherently with a
documented finite memory/frame/IPC budget and protocol compatibility decision,
or implement a correct explicitly owned segmentation strategy preserving all
audio order, correction/finalization semantics and cancellation. Do not
silently truncate or prove success only with a mock ASR implementation.

Further Codex inspection: the installed strict path actually constructs
`narrivy_platform_linux::recovery::PipeWireCapture`, not the general capture
adapter. In `adapters/platform-linux/src/recovery.rs`, both its constructor
and `capture_frame_budget` independently reject durations outside1..30;
the owned session tracks expected_samples and a safety_deadline. Audit the
strict batch and producer-owned streaming implementations, their helper
argv/frame limits, and typed `CaptureEndReason` all the way into session
finalization. Changing only `capture.rs` would leave installed dictation
unchanged. Preserve the existing Stop-tail and cancellation/readiness tests.

The actual portable enum is `narrivy_platform_api::CaptureEnd`. Core batch
recovery currently takes `captured.audio` and discards `captured.end`; the
streaming branch checks endpoint consistency and sample accounting but does
not propagate a safety-limit reason into the durable final outcome. Existing
platform SafetyLimit tests therefore cannot prove the user is told why
recording ended. Carry the typed reason through portable finalization/status
and client rendering without losing recoverable text, for both paths.

Task006's in-progress identity migration introduces `AdmittedPipeline` and
`PipelineLane::StrictCapture` in daemon policy: real selectable mode IDs are
raw/clean, while linux-mic-inbox and linux-mic-wtype-inbox remain configuration
lane IDs. Recheck its accepted final source before implementing; preserve the
single admitted correction value, journal/partial identity and deferred
Listening rule when adding capture controls and end-reason propagation.
