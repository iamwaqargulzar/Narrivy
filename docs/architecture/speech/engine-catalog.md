# Engine Catalog

**`spec/catalogs/engines.json` is authoritative.** It holds all **26 Engine
families**; the table below is a complete transcription of that catalog, not a
curated subset, so the two cannot drift. Engine `status` is a
*product-capability* status, not an implementation claim: `test-only` means no
release requirement, package, or platform qualification derives from that
engine. `asr-mock` is linked into `narrivyd` as a test and development
default only — see `docs/architecture/README.md`.

| Engine | Capabilities | Status | Purpose | Runtime notes | License |
|---|---|---|---|---|---|
| qwen3-asr | ASR | reference | LLM-based ASR 0.6B/1.7B with language identification; reference OpenVINO ASR engine. OpenVINO 2026.4 officially supports Qwen3-ASR on CPU/GPU; NPU is candidate-only pending exact qualification. | OpenVINO 2026.4 CPU/GPU; NPU only after exact tuple qualification; GGUF / sherpa-onnx alternatives remain model-specific | Apache-2.0 |
| parakeet | ASR | linux-1.0 | NVIDIA Parakeet family. Standard TDT v2/v3 models are offline; explicitly streaming variants such as Parakeet Unified have a separate streaming contract. Owner-fork measurements report 220-240ms warm for the TDT v3 NPU/Eddy lane; this is not a streaming qualification. | transcribe.cpp GGUF on CPU/iGPU/dGPU; openvino (Eddy) on NPU; ONNX via sherpa for earlier versions; capability is model-specific | CC-BY-4.0 weights; Apache-2.0 Eddy decoder |
| nemotron-speech | ASR | candidate | NVIDIA Nemotron Speech Streaming 0.6b; cache-aware conformer transducer, 80-160ms chunks, int4 ONNX real-time on CPU | onnxruntime (int4 community exports) | NVIDIA Open Model License |
| fun-asr | ASR/text.punctuation/text.itn | candidate | Alibaba Fun-ASR-Nano/MLT-Nano 0.8B LLM-based ASR with built-in punctuation+ITN; MLT 31 langs (Hindi yes, Urdu NOT in list — not Urdu-qualified) | official GGUF (llama.cpp prebuilts) | Apache-2.0 weights; MIT toolkit |
| moonshine | ASR | candidate | Moonshine family: offline tiny/base plus distinct Moonshine Streaming tiny/small/medium English variants | transcribe.cpp GGUF candidate; other runtimes remain separately evaluated | Moonshine Streaming Tiny canary MIT; every other artifact remains independently reviewed |
| vosk | ASR | candidate | Ultra-light native streaming ASR, 50MB models, 10+ years mature | native C++/C API | Apache-2.0 |
| asr-mock | speech.asr | test-only | Deterministic test engine | native-rust | Narrivy internal |
| rnnoise | audio.denoise | candidate | Tiny real-time neural denoise | native | BSD-3-Clause |
| gtcrn | audio.denoise | candidate | Lightweight streaming neural denoise | onnx/native-worker | MIT |
| deepfilternet | audio.denoise | optional | High-quality real-time speech enhancement | native/onnx/openvino | MIT/Apache-2.0 |
| dpdfnet | audio.denoise | optional | Streaming speech enhancement | onnx | Apache-2.0 |
| webrtc-apm | audio.aec/audio.denoise/audio.agc | optional | Echo cancellation and communications DSP | native | BSD-style upstream |
| odas | audio.beamform | roadmap | Source localization/beamforming for arrays | native | MIT |
| nara-wpe | audio.dereverb | roadmap | Dereverberation | worker | MIT |
| clearervoice | audio.separate/audio.target-speaker/audio.denoise | roadmap | Separation, enhancement and target-speaker extraction | worker/onnx | Apache-2.0 |
| silero-vad | speech.vad | optional | Neural voice activity detection | onnx | MIT |
| ten-vad | speech.vad | candidate | 306KB VAD, -32% RTF vs Silero, better ROC claims | onnx | unverified — check repo before adoption |
| turn-detector | speech.endpoint | candidate | EOU/turn classification (livekit turn-detector, pipecat smart-turn family) for endpointing polish | onnx / HF weights | Apache-2.0 (per card; confirm) |
| sherpa-onnx | speech.asr/speech.vad/speech.language-id/speech.audio-tagging/speech.diarization/text.punctuation/activation.wake-word | optional | Offline speech toolkit family | onnx | Apache-2.0 |
| pyannote-audio | speech.diarization | roadmap | Speaker diarization/embeddings | worker | MIT code; weights separately reviewed |
| nemo-text-processing | text.itn | optional | Deterministic text/inverse-text normalization | worker/fst | Apache-2.0 |
| symspell | text.spell | optional | Fast deterministic spell correction | native | MIT |
| hunspell | text.spell | optional | Dictionary/morphology spell checking | native | MPL/GPL/LGPL tri-license upstream |
| languagetool | text.grammar | optional | Local grammar/style checking | worker | LGPL |
| local-llm | text.llm-postprocess/text.translate | optional | Small local instruct model family | openvino/onnx/llama.cpp | model-specific |
| openwakeword | activation.wake-word | roadmap | openWakeWord / microWakeWord on-device wake-word family (roadmap slot) | onnx / tiny native models | Apache-2.0 |

## Implemented bindings

`engines/` holds four crates (checked at rc13):

- `asr-mock` — deterministic test engine. `narrivyd` always links it, and
  `EngineConfig::new()` with no environment selects it (with a sine fixture
  and a null sink). That is a test and development default only: the
  packaged unit and `scripts/dev-daemon.sh` set `NARRIVY_RECOVERY_MODE`,
  `NARRIVY_ASR_ENGINE=transcribe-cpp` and the resident runtime, so an
  installed Narrivy never uses the mock engine.
- `transcribe-cpp` — the GGUF ASR binding the installed daemon uses. It is
  optional, behind the `asr-transcribe-cpp` / `asr-transcribe-resident`
  features (`services/narrivyd/Cargo.toml`).
- `openvino` — the OpenVINO add-on engine (D25, ADR-0078). The daemon never
  loads OpenVINO; it starts `narrivy-openvino-worker`, which loads the
  runtime pack the user installs from the Store. Device claims still need
  qualification.
- `grammar-harper` — the Harper grammar checker used by the text stages.

## 2026-08-22 verified additions (research sweep)

Research provenance for the catalog rows added on 2026-08-22. Where a note
below disagrees with the table, the table and the catalog win.

- **qwen3-asr** (reference) — 0.6B/1.7B LLM-ASR, 52 langs/dialects; the custom older NPU tuple was owner-proven, while OpenVINO 2026.4 must be requalified and officially lists CPU/GPU support.
- **parakeet** (linux-1.0) — TDT v3 0.6B; 220–240 ms warm on NPU via Apache-2.0 Eddy (owner-fork-proven). The status was raised from `candidate` in a later catalog revision; see `spec/catalogs/engines.json`.
- **nemotron-speech** (candidate) — streaming conformer transducer 0.6b; int4 ONNX real-time on CPU; static shapes → NPU-friendly.
- **fun-asr** (candidate) — Nano/MLT-Nano 0.8B; built-in punct+ITN; official GGUF; Hindi yes / Urdu NOT (sister-language caveat: script divergence).
- **moonshine** (candidate) — offline family plus distinct native-streaming
  family; exact MIT Moonshine Streaming Tiny Q8_0 is the Recovery 4F canary,
  not a default or release-qualified model.
- **vosk** (candidate) — lowest-footprint native streaming fallback.
- **ten-vad** (candidate) — 306 KB VAD footprint tier (license check before adoption).
- **turn-detector** (candidate) — EOU/turn classification for endpointing.
- **openwakeword** (roadmap) — openWakeWord/microWakeWord family.
