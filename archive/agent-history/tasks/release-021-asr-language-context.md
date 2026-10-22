# Release 021 — capability-driven ASR language and context request

Implement the smallest production-grade change that closes the actual code gaps behind FUN-028 and FUN-029 without making unsupported capability claims.

Read first: `AGENTS.md`, `SOURCE_OF_TRUTH.md`, `BUILD_TARGET.json`, `docs/00-ARCHITECTURE-MAP.md`, `docs/09-phases/phase-05-asr-compute.md`, `docs/10-agent/token-efficient-workflow.md`, and the relevant catalogs. Inspect current code before editing.

Required behavior:

1. Evolve the portable `narrivy-asr-api` contract so a recognition/session request can keep these distinct and typed: spoken-language selection (explicit language or auto-detect), written/output language or script preference, optional translation target, locale/formatting convention, and bounded context/bias hints. Preserve simple callers through a default request path; do not force unrelated crates to know transcribe.cpp.
2. Validate combinations against backend/model capabilities before audio is handed to the engine. Unsupported auto-detect, translation, language, script/locale, or context bias must return a typed, content-free `AsrError`, never guess or silently ignore a requested setting.
3. Wire the request through Narrivy's transcribe.cpp sidecar and process-isolated resident worker paths where the pinned upstream 0.2.3 API supports it. Use upstream model capabilities (`languages`, `translate_target_languages`, `supports_language_detect`, `supports_translate`) and `RunOptions`; Whisper initial-prompt biasing may be used only when the loaded model accepts that family extension. Do not advertise language detection, translation, hotword/context bias, or streaming when the loaded model does not attest it.
4. Keep the current qualified Whisper tiny.en package truthful. If it cannot auto-detect/translate, capability output and validation must say so while explicit supported language transcription continues to work.
5. Add focused tests for setting separation and every rejection path, plus protocol encode/decode bounds if the resident protocol changes. Do not add speculative UI controls or a public plugin API.
6. Follow repository privacy rules: no transcript/context text in logs or errors. Keep all fields bounded. No unsafe outside the existing FFI crate.
7. Run only focused affected tests and clippy. Do not run release/phase gates, do not build a package, do not update evidence, do not edit `agent/STATE.json`, and do not commit or push.

If fully wiring the resident protocol is too broad for one bounded run, implement a coherent safe subset that compiles and leaves capabilities false for unwired paths. In your final response list exactly what is complete, what remains, and the commands run.
