# Release 022 — resident ASR recognition options

Implement only the resident transcribe.cpp backend portion of the already-committed typed ASR request API. Work in the current tree. Do not redesign unrelated daemon/UI code.

Read only: `AGENTS.md`, `crates/narrivy-asr-api/src/request.rs`, `crates/narrivy-asr-api/src/lib.rs`, `engines/transcribe-cpp/src/resident_protocol.rs`, `engines/transcribe-cpp/src/resident.rs`, `engines/transcribe-cpp/src/bin/narrivy-transcribe-worker.rs`, and the installed transcribe-cpp 0.2.3 Rust API as needed.

Required:
1. Evolve the private bounded resident protocol so startup reports loaded-model languages, translation targets, language-detection, translation, streaming, and whether the loaded model accepts Whisper run options for context bias. Reject malformed/oversized capability payloads.
2. Carry a bounded recognition-options payload for one-shot decode: spoken language (unspecified, auto, explicit), optional translation target, and bounded context hints. Preserve default requests. Do not put output language/script/locale on the wire because the backend cannot apply them; portable validation must reject those first.
3. In the worker, validate settings against actual loaded-model capabilities before decode, construct `transcribe_cpp::RunOptions`, set `Task::Translate` only with an attested target, and use `RunExtension::Whisper(WhisperRunOptions { initial_prompt: ... })` only when the loaded model accepts the Whisper run extension. Never log or return hint text in errors.
4. In `ResidentWorkerEngine`, report the worker's actual capabilities and override `OneShotAsrPort::recognize_with` so it validates the full `AsrRequest`, rejects unapplied output/script/locale settings, then sends supported options. Keep streaming request settings conservative/unwired.
5. Protocol version may change. All payloads must be length/count bounded, parsing fail closed, and tests must cover round trips and malformed/oversized cases. Keep the current tiny.en lane truthful: explicit English/context bias may be accepted if the loaded model attests them; auto-detect/translation remain false if the model says false.
6. Run focused resident-protocol, resident-worker, and strict Clippy checks only. Do not run release gates, build packages, change evidence/traceability/state, commit, or push.

If a dependency API does not expose a stable extension-kind constant, do not guess it. Use a capability rule that can be defended from the pinned API and explain it.
