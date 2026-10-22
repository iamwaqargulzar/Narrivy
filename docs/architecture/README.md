# Narrivy v1.3 — Authoritative Architecture Map

This is the navigation authority for the autonomous build package. Normative release scope remains in `docs/product/requirements.md` and `spec/requirements-traceability.csv`; the catalogs here prevent taxonomy drift.

## Canonical terms

- **Feature** — user-visible or operational capability with requirement ownership.
- **Pipeline** — persisted, versioned processing graph selected by a trigger/profile/policy.
- **Module** — Narrivy-compatible implementation of one capability slot.
- **Engine** — algorithm/model family (for example Qwen3-ASR, Parakeet, RNNoise, GTCRN, DeepFilterNet).
- **Runtime** — execution technology (for example transcribe.cpp, OpenVINO, ONNX Runtime, Vulkan, llama.cpp).
- **Model Pack** — weights/data plus immutable identity/license/provenance.
- **Provider** — local or remote inference service adapter.
- **Trigger** — event that selects/starts a Pipeline (hold hotkey, toggle hotkey, app rule, CLI, wake phrase, etc.).

## Reference default

`Hotkey → PipeWire capture → light denoise → VAD/endpointing → local ASR (OpenVINO on a qualified device/model tuple; transcribe.cpp GGUF on CPU/iGPU/dGPU) → deterministic punctuation/ITN/correction → semantic guard → transactional delivery → Inbox fallback`

The normal installer stays lean. Advanced modules/runtimes/models are acquired only when selected/qualified. CPU correctness remains available even when Auto prefers **NPU → dGPU → iGPU → CPU**.

## Catalog inventory

- **20 feature groups** covering all **136 normative requirements exactly once**.
- **12 built-in/reserved Pipeline recipes**.
- **26 Engine families**; the OpenVINO ASR lane is device-qualified per model (OpenVINO 2026.4 officially lists Qwen3-ASR for CPU/GPU; NPU remains candidate-only), with transcribe.cpp as the reference CPU/iGPU/dGPU ASR runtime running all supported GGUF model variants.
- **66 Module bindings** across **31 capability slots** (2026-08-22: ASR lanes rewired to qwen3-asr-openvino / parakeet-transcribecpp / gguf-whisper-models-transcribecpp).
- **8 Runtime families**.

Catalog status versus implementation status differ and must not be conflated.
`spec/catalogs/engines.json` and `modules.json` mark `asr-mock` `test-only`,
which is a *product-capability* claim: no release requirement, package, or
platform qualification derives from it. It is **not** a claim that the crate is
absent from the product. `narrivy-asr-mock` is a non-optional `narrivyd`
dependency (`services/narrivyd/Cargo.toml:47`) and the daemon's default
composition: `EngineConfig::default()` selects `AsrEngineKind::Mock`,
`AudioSourceKind::SineFixture`, `DeliveryBackendKind::NullSink`
(`services/narrivyd/src/daemon/policy.rs:422-442`). `engines/` contains exactly
two ASR bindings: `asr-mock`, and `transcribe-cpp` behind the
`asr-transcribe-cpp` / `asr-transcribe-resident` features
(`services/narrivyd/Cargo.toml:49,55-62`). There is **no OpenVINO
inference engine in the tree**: OpenVINO appears only as absence detection
(`crates/narrivy-compute/src/discovery.rs:188` `probe_openvino_install`, and
the same suggestion in `crates/narrivy-model-manager/src/manager.rs:170`)
returning an install hint when `libopenvino.so` is not present.

Machine-readable catalogs live in `spec/catalogs/`; human indexes live beside the relevant architecture/ML docs.

## Required reading order for coding agents

1. `AGENTS.md`
2. `SOURCE_OF_TRUTH.md`
3. this file
4. `agent/completion-plan/README.md`
5. `spec/requirements-traceability.csv`
6. `agent/PHASES.json` and active phase spec
7. `spec/catalogs/`

## Non-negotiable separation

ASR model weights are catalog content executed through runtimes; they are not product entities. OpenVINO execution is admitted only for an exact model/runtime/driver/device tuple that has passed qualification; generic NPU discovery is insufficient. CPU/iGPU/dGPU lanes run through transcribe.cpp (all supported GGUF model variants, including every model variant of that family). OpenVINO/ONNX Runtime/transcribe.cpp are Runtimes/bindings. A Pipeline references capability slots and Module IDs; the policy/qualifier resolves a compatible engine/runtime/model/device tuple. Optional smart stages may fail or fall back without making basic dictation unavailable.
