# Module Catalog

| Module | Capability | Engine/family | Runtime | Status |
|---|---|---|---|---|
| linux-global-shortcut | activation.global-hotkey | narrivy-platform | native | linux-1.0 |
| linux-x11-hotkey | activation.global-hotkey | narrivy-platform | native | fallback |
| sherpa-kws | activation.wake-word | sherpa-onnx | onnxruntime | roadmap |
| pipewire-capture | audio.capture | narrivy-platform | native | linux-1.0 |
| native-audio-normalizer | audio.normalize | narrivy-core | native | linux-1.0 |
| narrivy-lite-dsp | audio.dsp | narrivy-core | native | linux-1.0 |
| speexdsp | audio.dsp | speexdsp | native | candidate |
| rnnoise | audio.denoise | rnnoise | native | candidate |
| gtcrn | audio.denoise | gtcrn | onnxruntime | candidate |
| deepfilternet | audio.denoise | deepfilternet | native | optional |
| dpdfnet | audio.denoise | dpdfnet | onnxruntime | optional |
| webrtc-ns | audio.denoise | webrtc-apm | native | optional |
| openvino-denoise | audio.denoise | deepfilternet | openvino | optional |
| webrtc-aec3 | audio.aec | webrtc-apm | native | optional |
| pipewire-aec | audio.aec | pipewire | native | optional |
| webrtc-agc | audio.agc | webrtc-apm | native | optional |
| native-agc | audio.agc | narrivy-core | native | candidate |
| odas-beamformer | audio.beamform | odas | native | roadmap |
| nara-wpe | audio.dereverb | nara-wpe | isolated-worker | roadmap |
| clearervoice-separation | audio.separate | clearervoice | isolated-worker | roadmap |
| sherpa-source-separation | audio.separate | sherpa-onnx | onnxruntime | roadmap |
| clearervoice-target-speaker | audio.target-speaker | clearervoice | isolated-worker | roadmap |
| silero-vad | speech.vad | silero-vad | onnxruntime | optional |
| webrtc-vad | speech.vad | webrtc-apm | native | candidate |
| sherpa-vad | speech.vad | sherpa-onnx | onnxruntime | optional |
| native-energy-vad | speech.vad | narrivy-core | native | linux-1.0 |
| narrivy-endpointer | speech.endpoint | narrivy-core | native | linux-1.0 |
| sherpa-language-id | speech.language-id | sherpa-onnx | onnxruntime | optional |
| qwen3-asr-language-id | speech.language-id | qwen3-asr | openvino | linux-1.0 |
| sherpa-audio-tagging | speech.audio-tagging | sherpa-onnx | onnxruntime | roadmap |
| pyannote-diarization | speech.diarization | pyannote-audio | isolated-worker | roadmap |
| sherpa-diarization | speech.diarization | sherpa-onnx | onnxruntime | roadmap |
| sherpa-speaker-id | speech.speaker-id | sherpa-onnx | onnxruntime | roadmap |
| qwen3-asr-openvino | speech.asr | qwen3-asr | openvino | linux-1.0 |
| parakeet-transcribecpp | speech.asr | parakeet | transcribe.cpp | linux-1.0 |
| gguf-whisper-models-transcribecpp | speech.asr | transcribe-gguf-models | transcribe.cpp | linux-1.0 |
| sherpa-asr | speech.asr | sherpa-onnx | onnxruntime | optional |
| asr-mock | speech.asr | asr-mock | native | test-only |
| sherpa-punctuation | text.punctuation | sherpa-onnx | onnxruntime | optional |
| deterministic-punctuation | text.punctuation | narrivy-core | native | linux-1.0 |
| nemo-itn | text.itn | nemo-text-processing | isolated-worker | optional |
| native-itn | text.itn | narrivy-core | native | linux-1.0 |
| symspell | text.spell | symspell | native | optional |
| hunspell | text.spell | hunspell | native | optional |
| languagetool | text.grammar | languagetool | isolated-worker | optional |
| offline-translation | text.translate | local-llm | openvino/onnxruntime/llama.cpp | roadmap |
| llm-translation | text.translate | local-llm | provider | roadmap |
| openvino-llm-postprocess | text.llm-postprocess | local-llm | openvino | optional |
| llamacpp-llm-postprocess | text.llm-postprocess | local-llm | llama.cpp | optional |
| cloud-llm-postprocess | text.llm-postprocess | cloud-provider | remote | optional |
| narrivy-semantic-guard | text.semantic-guard | narrivy-core | native | linux-1.0 |
| app-context-provider | context.provider | narrivy-platform | native | optional |
| platform-direct-delivery | delivery.direct | narrivy-platform | native | linux-1.0 |
| transactional-clipboard | delivery.clipboard | narrivy-platform | native | linux-1.0 |
| narrivy-inbox | delivery.inbox | narrivy-core | native | linux-1.0 |
| narrivy-qualifier | quality.qualifier | narrivy-core | native | linux-1.0 |

### 2026-08-22 verified candidate bindings

speech.asr: qwen3-asr-openvino (custom NPU conversion owner-proven on the older tuple; OpenVINO 2026.4 official support is CPU/GPU and requires requalification), parakeet-eddy-openvino (220–240 ms NPU), parakeet-onnx, nemotron-onnx (int4 streaming), funasr-gguf (built-in punct+ITN; not Urdu-qualified), moonshine-onnx (tiny EN), vosk-native (ultra-light streaming).
speech.vad: tenvad-onnx (306 KB footprint tier). speech.endpoint: turn-detector-eou (EOU polish).
text.punctuation: ct-transformer-punct (zh/en ONNX via sherpa).
activation.wake-word (roadmap): openwakeword-onnx, microwakeword-native.
