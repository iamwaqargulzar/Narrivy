# Narrivy Build Pack v1.3 — Canonical Mapping & Naming Repair

## Changes

- Added one authoritative architecture map and machine-readable catalogs for Features, Pipelines, Engines, Modules and Runtimes.
- ASR engine plan later revised by owner decision 2026-08-22: reference lane is Qwen3-ASR via OpenVINO on NPU; transcribe.cpp runs all supported GGUF model variants on CPU/iGPU/dGPU.
- Reclassified OpenVINO, ONNX Runtime, Vulkan and llama.cpp/transcribe.cpp execution backends as Runtimes/bindings rather than ASR engines.
- ASR bindings: `qwen3-asr-openvino` (NPU reference), `parakeet-transcribecpp`, and `gguf-whisper-models-transcribecpp` (transcribe.cpp lanes; owner decision 2026-08-22).
- Canonicalized P2/P3/P6 filenames and registry names.
- Removed legacy planning/changelog/report artifacts from the shipped pack so only v1.3 build authority remains.
- Normalized active references from old version labels to the current v1.3 contract or `post-1.0`.
- Kept the default product lean while documenting advanced optional modules and pipeline recipes.
