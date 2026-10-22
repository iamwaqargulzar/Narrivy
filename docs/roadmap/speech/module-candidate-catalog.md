# Candidate Module Catalog — Evaluation Inventory

> **Status: roadmap / not in 1.0.** This describes a planned design, not current behaviour. See [the roadmap](../README.md).

> This is an **evaluation inventory**, not a redistribution or support commitment. Re-check source activity, code license, model-weight license, patents/redistribution terms, security posture, API stability and measured Narrivy benefit at implementation time. Code and pretrained weights may have different licenses.

Narrivy should define capability slots first and evaluate multiple implementations per slot.

| Capability slot | Candidate families/projects to evaluate | Narrivy use |
|---|---|---|
| minimal DSP | native filters / SpeexDSP-class processing | rumble/hum/basic AGC/ultra-low-resource fallback |
| lightweight neural denoise | RNNoise, GTCRN | default/lean neural option candidate |
| enhanced denoise | DeepFilterNet, DPDFNet, OpenVINO-compatible enhancement models | noisy office/high-quality profiles |
| echo cancellation | WebRTC APM/AEC3, PipeWire echo-cancel facilities | dictate while speakers/playback remain active |
| beamforming/spatial | ODAS-class array processing, future dual-channel models | multi-mic laptops/arrays |
| dereverberation | WPE/NARA-WPE-class processing | far-field/echoey rooms |
| source separation | ClearerVoice/Asteroid/sherpa-onnx-class separation | speech/music/competing-source isolation |
| target-speaker extraction | speaker-conditioned extraction models | “focus on my voice” noisy shared spaces |
| VAD | lightweight energy/WebRTC VAD, Silero VAD, sherpa-onnx VAD | speech/no-speech |
| endpointing | dedicated endpoint policy over VAD + hotkey/session cues | decide utterance completion |
| wake phrase/KWS | sherpa-onnx keyword spotting; other wake engines after model-license review | optional hands-free trigger |
| speaker diarization | pyannote/sherpa-onnx-class diarization | meeting transcript speaker turns |
| speaker ID/verification | embedding/verification modules | labels/convenience filtering, not security auth |
| language ID | ASR/LID models | automatic spoken-language routing |
| audio tagging | local acoustic classifiers | detect music/background speech/keyboard/scene for Auto policy |
| ASR | Qwen3-ASR (OpenVINO 2026.4 CPU/GPU; NPU candidate pending qualification), transcribe.cpp GGUF families, sherpa-onnx | primary transcription |
| inverse text normalization | NeMo ITN-class rules/FSTs or equivalent | numbers/dates/currency without LLM |
| punctuation | sherpa/standalone punctuation models or deterministic rules | post-ASR punctuation |
| spelling | SymSpell/Hunspell-class modules | deterministic corrections/dictionaries |
| grammar | LanguageTool-class module | deterministic/local proofreading option |
| translation | offline translation engines or LLM provider | speech-to-translated-text pipelines |
| local LLM | OpenVINO/ONNX/llama.cpp-compatible small instruct models | faithful cleanup/polish |
| cloud LLM | provider adapters | optional fast/large post-processing |
| anti-spoofing | future replay/synthetic detectors | convenience guard for hands-free modes only |

## Pipeline examples

### Lean Default
`capture → lightweight denoise → VAD → ASR → deterministic corrector → delivery`

### Speaker-safe
`capture + playback reference → AEC → denoise → VAD → ASR → corrector → delivery`

### Far-field
`mic array → beamforming → dereverb → enhanced denoise → VAD → ASR`

### Focus-on-me
`capture → target-speaker extraction → denoise → VAD → ASR`

### Smart Local
`capture → denoise → VAD → ASR → deterministic cleanup → local LLM faithful cleanup → guard → delivery`

### Meeting (roadmap)
`capture/system audio → enhancement → VAD → ASR → diarization → timestamps → optional summary/export`

The coding agent should not implement this entire table at once. It should preserve capability contracts and add candidates only when a phase or explicit roadmap target calls for them.

## 2026-08-22 verified candidate sweep (addendum)

Web-verified additions and rejections beyond the original inventory. Full
sources in `agent/RESEARCH_LOG.md` (2026-08-22 sweep entry). License fields
marked *unverified* require confirmation before any adoption.

### Additions
- **speech.asr**: Fun-ASR-Nano-2512 / Fun-ASR-MLT-Nano-2512 (Apache-2.0, GGUF, built-in punct+ITN; MLT 31 langs); NVIDIA Nemotron Speech Streaming 0.6b (NVIDIA Open Model License, streaming); Moonshine Streaming Tiny Q8_0 (MIT, English, 50.5 MB, exact Recovery 4F canary pin); Voxtral Mini Realtime (roadmap); Urdu: whisper-large-v3-urdu fine-tune, AI4Bharat IndicConformer-STT-UR; Parakeet TDT upgraded to v3 (CC-BY-4.0); Qwen3-ASR artifacts confirmed. Every non-canary candidate still requires exact artifact/license/runtime qualification.
- **speech.vad / speech.endpoint**: TenVAD (ONNX, 306 KB, −32% RTF vs Silero; license unverified); livekit/turn-detector (ONNX EOU, 135M); pipecat smart-turn (audio-native turn detection); parakeet-realtime-eou-120m ONNX (niche).
- **text.punctuation**: CT-Transformer via sherpa-onnx/FunASR (zh/en ONNX).
- **text.llm-postprocess models**: Gemma 4 E2B/E4B (Apache-2.0, official GGUF + OpenVINO INT4, audio-capable); Qwen3.5-2B (Apache-2.0, 201 langs — prefer over 0.8B); LFM2.5-350M/1.2B + Encoders (LFM Open License, OpenVINO GenAI official; Encoders = targeted correction matching our Faithful-Cleanup contract); Phi-4-mini (MIT, older gen).
- **activation.wake-word (roadmap)**: openWakeWord, microWakeWord (both Apache-2.0).
- **Serving/substrate**: OpenVINO GenAI ≥2025.3 loads GGUF directly (one runtime stack for GGUF+IR); pykeio/`ort` Rust crate (ORT 1.17–1.24, OpenVINO/QNN EPs); Speaches (MIT OpenAI-compatible realtime WebSocket) as optional sidecar protocol; sherpa-onnx Whisper+QNN path noted.

### Rejections (do not add)
Porcupine (proprietary); IBM Granite Speech 3.3 (no deployable artifacts); SenseVoice-Small (non-commercial license); ZipEnhancer (offline-only, no ONNX, license unverified); FRCRN (license unverifiable); candle as primary ASR lane (no NPU EPs); "UPSLEEP" (nonexistent); TSDCA-BA / aTENNuate / Weighted Lookahead Scoring (no released artifacts or wrong hardware target).

### Live-lane cautions
sherpa-onnx speech enhancement is offline-batch-only (upstream issue #2670) — not a live denoise lane. DeepFilterNet3 remains the verified real-time denoise default.
