# Narrivy Engine Bindings

`engines/` contains concrete/test bindings to algorithm families. **Engine** names the algorithmic family; **Runtime** names how it executes.

ASR lanes per owner decision 2026-08-22:

- **OpenVINO:** Qwen3-ASR on qualified CPU/GPU tuples under OpenVINO 2026.4; NPU remains candidate-only until an exact model/runtime/driver/device tuple passes (`qwen3-asr-openvino`).
- **CPU / iGPU / dGPU:** transcribe.cpp GGUF runtime — runs all model variants supported by that runtime, including every Whisper model variant as GGUF weights and Parakeet-class models (`parakeet-transcribecpp`, `gguf-whisper-models-transcribecpp`).

There is no separate engine/runtime entity for any single model family beyond the transcribe.cpp GGUF lane; model weights are catalog content, not product entities.

Bindings in this tree (rc13):

- `asr-mock/` — deterministic test engine. `EngineConfig::new()` with no
  environment selects it (with a sine fixture and a null sink); the packaged
  unit and `scripts/dev-daemon.sh` select transcribe.cpp instead, so an
  installed Narrivy never uses it.
- `transcribe-cpp/` — transcribe.cpp GGUF ASR binding, the installed engine,
  behind the `asr-transcribe-cpp` / `asr-transcribe-resident` features.
- `openvino/` — the OpenVINO add-on worker (`narrivy-openvino-worker`, D25,
  ADR-0078); device claims are qualification-derived.
- `grammar-harper/` — Harper grammar checker for the text stages.

See `docs/architecture/README.md` and `spec/catalogs/`.
