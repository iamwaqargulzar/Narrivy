# Narrivy Engine Bindings

`engines/` contains concrete/test bindings to algorithm families. **Engine** names the algorithmic family; **Runtime** names how it executes.

ASR lanes per owner decision 2026-08-22:

- **OpenVINO:** Qwen3-ASR on qualified CPU/GPU tuples under OpenVINO 2026.4; NPU remains candidate-only until an exact model/runtime/driver/device tuple passes (`qwen3-asr-openvino`).
- **CPU / iGPU / dGPU:** transcribe.cpp GGUF runtime — runs all model variants supported by that runtime, including every Whisper model variant as GGUF weights and Parakeet-class models (`parakeet-transcribecpp`, `gguf-whisper-models-transcribecpp`).

There is no separate engine/runtime entity for any single model family beyond the transcribe.cpp GGUF lane; model weights are catalog content, not product entities.

Planned bindings:

- `asr-mock/` — deterministic ASR engine (present). It is the **default** engine in a
  default `narrivyd` build (`EngineConfig::default()` resolves to `AsrEngineKind::Mock`
  with a sine fixture and a null sink), so it is not test-only in the sense of
  being unreachable; the catalog status records that it is not a product-quality
  recognition capability, not that nothing uses it.
- `transcribe-cpp/` — transcribe.cpp GGUF ASR binding; lands with P2/P5 behind a `runtime` feature flag so default workspace builds stay toolchain-free.
- `openvino-asr/` — planned native OpenVINO GenAI worker binding; device claims are qualification-derived.

See `docs/architecture/README.md` and `spec/catalogs/`.
