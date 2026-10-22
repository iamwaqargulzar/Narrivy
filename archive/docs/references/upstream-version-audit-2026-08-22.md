# Upstream Version & Node Map Audit — 2026-08-22

**Purpose:** single consolidated pre-build map of every pipeline node, its module
bindings, the latest verified upstream versions, and integration status. Owner
direction 2026-08-22: prefer the latest stable upstream at lock time.

**Rules that govern this map** (from `SOURCE_OF_TRUTH.md` volatile-facts rule and
`agent/RESOLVED_VERSIONS.json`):
- Nothing external is pinned in the v1.3 tree yet (`Cargo.lock` is workspace-only;
  engine bindings land feature-gated). Versions lock at the phase that
  implements each node.
- Product pins are always **immutable tags/revisions with hashes**, never floating
  "latest"; a runtime is never silently upgraded across a passing release candidate.
- This file is a dated snapshot. Re-verify before P5 implementation and before P11
  release qualification; durable findings live in `agent/RESEARCH_LOG.md`
  (entries dated 2026-08-22).

## 1. Reference pipeline: node → slot → bindings

Frozen-graph rule: each trigger selects one complete validated pipeline; nodes do
not swap mid-session; `auto` slots re-resolve between sessions; failure fallback
is degradation, not a pipeline edit (ADR-0016).

| # | Stage | Slot | Catalog bindings | Status |
|---|---|---|---|---|
| 1 | Trigger | activation.global-hotkey | linux-global-shortcut (portal), linux-x11-hotkey (fallback) | linux-1.0 |
| 2 | Capture | audio.capture | pipewire-capture | linux-1.0 |
| 3 | Normalize | audio.normalize | native-audio-normalizer | linux-1.0 (built-in) |
| 4 | Denoise | audio.denoise | rnnoise (default candidate), gtcrn, deepfilternet, dpdfnet, webrtc-ns, openvino-denoise | candidate/optional |
| 5 | VAD | speech.vad | silero-vad (default candidate), webrtc-vad, sherpa-vad, native-energy-vad | linux-1.0 uses native-energy-vad; silero candidate |
| 6 | Endpoint | speech.endpoint | narrivy-endpointer | linux-1.0 (built-in) |
| 7 | ASR | speech.asr | qwen3-asr-openvino (linux-1.0, NPU reference), parakeet-transcribecpp + gguf-whisper-models-transcribecpp (transcribe.cpp lanes) | Owner decision 2026-08-22 |
| 8 | Punctuation | text.punctuation | deterministic-punctuation, sherpa-punctuation | linux-1.0 deterministic |
| 9 | ITN | text.itn | native-itn, nemo-itn (sidecar candidate) | linux-1.0 native |
| 10 | Guard | text.semantic-guard | narrivy-semantic-guard | linux-1.0 (built-in) |
| 10b | Optional LLM | text.llm-postprocess | openvino-llm-postprocess, llamacpp-llm-postprocess, cloud-llm-postprocess | optional; P5/P7 |
| 11 | Delivery | delivery.direct / clipboard / inbox | platform-direct-delivery, transactional-clipboard, narrivy-inbox | linux-1.0 |

Delivery privilege ladder (P0F-004): semantic → wtype/clipboard transaction →
Inbox; uinput opt-in only, never default.

## 2. Verified latest upstream versions (checked 2026-08-22)

### Runtimes / substrate

| Component | Latest | Notes / action |
|---|---|---|
| OpenVINO | **2026.3** (2026-08-04, development release); **2025.4.1** = LTS | P5 must choose dev vs LTS deliberately; archive-verified 2025.4.1 |
| ONNX Runtime | **1.28.x** (patch 2026-08-17) | sherpa-onnx bundles 1.27.x currently |
| llama.cpp | rolling (semver v0.1.x; multiple/day; ~2026-08-21) | pin exact build when optional LLM module lands |
| transcribe.cpp | REFERENCE CPU/iGPU/dGPU ASR runtime (owner decision 2026-08-22) | MIT license verified 2026-08-22; multi-family GGUF incl. all variants of the large-v3 model family; Vulkan/CUDA/Metal(+CPU); no NPU lane (NPU = qwen3-asr-openvino); pin immutable revision at P5 |
| Vulkan | host-driver API 1.3 lane | not a bundled artifact |
| PipeWire | **1.6.8** upstream; lanes pin distro versions (Pop!_OS 24.04 host) | code to API, not upstream-latest |

### Engine / model artifacts

| Artifact | Latest | Notes |
|---|---|---|
| Whisper weights | **large-v3-turbo** (2024-10) is newest open-weight Whisper; no v4 | 809M params, 4 decoder layers, ~5.4× faster than large-v3 → **recommend adding turbo as Balanced/latency-tier candidate at P5**; large-v3 stays Accuracy; CPU-known-good stays Fallback. GPT-4o-transcribe is API/cloud-only, not 1.0 default |

### Audio modules

| Module | Latest | Notes |
|---|---|---|
| silero-vad | **v6.0** | v5: 3× faster, 6000+ langs; v6: −16% errors on noisy data; default VAD candidate at P4 |
| rnnoise | **0.2** (2024-04; canonical repo gitlab.xiph.org) | lean-path denoiser |
| deepfilternet | **DeepFilterNet3 v0.5.x** | no newer release; optional tier |
| gtcrn, webrtc-apm | unverified (research repo / libwebrtc snapshots) | verify when used |
| sherpa-onnx | **v1.13.6** (2026-08-17) | very active; strongest true-streaming ASR candidate for P5 decision |

### Text / delivery utilities

| Module | Latest | Notes |
|---|---|---|
| nemo-text-processing | **r1.2.0** (Apache-2.0, Pynini) | Python → isolated-worker/sidecar candidate only; lean default stays native-itn |
| LanguageTool | **6.8** (2026-05); snapshot release model | optional grammar module |
| SymSpell / hunspell | active / standard | pin exact tags at use |
| wtype | **v0.4** | stable; proven delivery mechanism on this host class |
| wl-clipboard | **2.3.0** | must be installed on host (P0F probe found absent) |

## 3. Local-LLM post-processing (text.llm-postprocess) — lane research

Design complete (ADR-0018: Faithful Cleanup / Polish / Rewrite + semantic guard +
deterministic fallback + provider separation; remote-transcript vs remote-context
permissions distinct). Implementation lands P5 (runtime packs, whole-pipeline
qualification) + P7 (guarded contracts). v0.4 archive has no LLM post-processing;
only acquisition machinery is salvageable.

- **NPU lane: real and official.** OpenVINO GenAI 2026 supports LLM inference on
  NPU with Core Ultra Series 2 (Lunar Lake) as the documented baseline;
  Qwen3-8B-class demonstrated on Intel NPU; OpenVINO 2026.0 expanded LLM+NPU.
  Constraint: NPU compiler requires **static shapes** — Faithful Cleanup's bounded
  input window/output budget fits. Speculative decoding (NPU draft + GPU target)
  demonstrated on Lunar Lake.
- **iGPU lane: prefer OpenVINO GPU on Intel.** llama.cpp Vulkan is hit-or-miss on
  Intel iGPUs (reports ~1 tok/s; Intel steers to SYCL). On Lunar Lake the iGPU LLM
  lane should come from the **same OpenVINO runtime pack**; llama.cpp/Vulkan stays
  the cross-vendor alternate (strong on AMD APUs) and CPU the fallback.
- **Model class for "fix the text": 1.5–4B INT4** (fits NPU memory, meets
  release→committed budget). Candidates at P5: Qwen3.5 ~4B-class INT4,
  Gemma 4 E2B/E4B, Phi-4-mini, Gemma 3 4B (140+ langs). 8B-class INT4 only as
  optional accuracy tier. Re-evaluates the older "Qwen3-8B" lock at P5.
- Placement is **measured, not assumed**: whole-graph qualification compares
  ASR-on-NPU + post-on-iGPU/CPU vs NPU co-residency.

## 4. Pending decisions at lock time

1. ~~OpenVINO version~~ **RESOLVED 2026-08-22: locked 2026.3** (owner decision;
   fork-verified on this hardware; LTS 2025.4.1 = bounded fallback).
2. ASR model tier: **owner's daily-driver reference is Qwen3-ASR 1.7B INT8 on
   NPU** (their fork, actively used daily — owner-validated quality/latency);
   candidates for P5 qualification: Qwen3-ASR 1.7B (owner preference),
   Parakeet TDT 0.6B V3 (fastest measured, 220–240 ms), Whisper large-v3
   (accuracy) / large-v3-turbo (Balanced/latency), whisper-small (lean).
3. True-streaming ASR: sherpa-onnx (Zipformer-class) vs transcribe.cpp
   (Parakeet-streaming GGUF, GPU/CPU lanes only) vs **Parakeet-TDT via
   Eddy/OpenVINO on the NPU lane** (owner-fork-verified: 11 s audio in 220–240 ms
   warm, cold load 27 s) — P5 qualification on evidence
   qualification decides what FUN-003 claims.
4. Small-LLM model+revision for Faithful Cleanup (narrivy/* HF conversions).

## 5. Source anchors

GitHub releases: openvinotoolkit/openvino,
microsoft/onnxruntime, ggml-org/llama.cpp, k2-fsa/sherpa-onnx, snakers4/silero-vad,
Rikorose/DeepFilterNet, xiph/rnnoise (GitLab canonical), atx/wtype,
bugaevc/wl-clipboard, wolfgarbe/SymSpell, hunspell/hunspell.
Docs: docs.openvino.ai/2026 (GenAI-on-NPU), Phoronix OpenVINO 2026.0,
HF openai/whisper-large-v3-turbo, pypi nemo-text-processing,
dev.languagetool.org roadmap. Full detail in `agent/RESEARCH_LOG.md` (2026-08-22
entries). Related: `archive/docs/management/salvage-inventory.md` (what lands when),
`agent/RESOLVED_VERSIONS.json` (host-verified pins).


## 6. Resolved owner decisions (2026-08-22 question round)
- OpenVINO locked 2026.3 (LTS fallback recorded).
- Real-speech evidence policy: owner's real-speech samples from the public
  Handy-Pop-OS fork history (provenance-verified per sample) + synthetic
  fixtures via virtual source = real-audio-path evidence; live human session
  per lane at P11.
- GNOME lane deferred to P11; P0F owner gate amended with consent.
- MIT final; no reservations remaining; squash approved; portal pre-grant +
  windows plan; HF org creation in progress.
