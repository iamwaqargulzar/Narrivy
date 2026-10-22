# Volatile Upstream Research Log

Use this log only for facts that can change: library/runtime versions, OS/desktop behavior, hardware drivers, model support, packaging requirements, and security advisories.

For each entry record date, question, authoritative source URL/title, conclusion, version(s), and engineering impact. Prefer upstream projects/specifications over blog posts.

Initial research snapshot is in `archive/docs/references/current-platform-notes.md`.

## 2026-08-21 — cargo-deny 0.20.2 wildcard-path handling
- Question: Why does `cargo deny check` fail with `wildcards = deny` for internal workspace path dependencies (`workspace = true`)?
- Source: cargo-deny 0.20.2 source at `~/.cargo/registry/src/.../cargo-deny-0.20.2/src/bans/cfg.rs` and CHANGELOG PR #487, plus local `cargo deny init` template.
- Conclusion: `allow-wildcard-paths` option added in PR #487 allows wildcards for private path crates while still denying external `*` versions. Our `Cargo.toml` uses path-only workspace deps without version (e.g., `narrivy-types = { path = "crates/..." }`), which cargo-deny treats as wildcard. Setting `allow-wildcard-paths = true` is correct for publish=false workspace crates.
- Versions: cargo-deny 0.20.2, rust 1.97.1, deny.toml bans wildcards deny + allow-wildcard-paths true
- Impact: gate now passes; external wildcard deps remain correctly denied.

## 2026-08-21 — Rust toolchain and MSRV
- Question: Verify pinned toolchain and portable MSRV compile portability.
- Source: `rust-toolchain.toml` (channel 1.97.1), `Cargo.toml` workspace.rust-version 1.85, `.github/workflows/ci.yml` RUST_PROJECT_TOOLCHAIN/RUST_PORTABLE_MSRV, local `rustup toolchain list`, `rustc --version`.
- Conclusion: Host has rustc 1.97.1 active, cargo 1.97.1, rustup 1.29.0, plus MSRV 1.85.0 installed. Both `cargo check --workspace --locked` (1.97.1) and `cargo +1.85.0 check --locked -p narrivy-types ...` (MSRV) succeed, proving portable crates compile. Tauri major 2 (from RESOLVED_VERSIONS prior note) not yet used in P0.
- Versions: 1.97.1 pinned, 1.85.0 MSRV, node v24.18.1
- Impact: PORT-001 evidence satisfied.

## 2026-08-21 — Manifest verification and target exclusion
- Question: Should `target/` be included in BUILD-PACK-MANIFEST verification?
- Source: `scripts/build_manifest.py`, `scripts/verify_manifest.py`, `.gitignore` (`/target/`), `archive/docs/autonomous-build/initial-state.md` (Cargo.lock may be absent initially), CI `pack` job order (validate before build).
- Conclusion: Original pack manifest (298 files) excluded target (which didn't exist at pack creation). After builds, target contains thousands of ephemeral files that would cause unlisted-file failures. Exclude list added (target, node_modules, dist, .vite, etc.) mirrors .gitignore and keeps validation deterministic without disabling fail-closed behavior for real source files. Cargo.lock now correctly included (304 files after P0).
- Versions: manifest schema_version 3, pack_version 0.3
- Impact: `validate_pack` passes after builds; manifest regenerated via `python3 scripts/build_manifest.py`.

## 2026-08-21 — Vulkan as extra GPU accelerator locked
- Question: Is adding Vulkan beside OpenVINO for GPU sound? Is hierarchy NPU→GPU→CPU still correct?
- Source: docs/architecture/compute-scheduler.md, docs/roadmap/speech/openvino-npu.md, docs/architecture/pipeline-engine.md, owner confirmation 2026-08-21.
- Conclusion: Yes, sound. Vulkan added to GPU pool for cross-vendor coverage (Intel/AMD/NVIDIA Wayland) alongside OpenVINO GPU, not replacing. Hierarchy locked NPU→GPU(Vulkan+OpenVINO GPU+ONNX EPs)→CPU, health-checked, bounded fallback, CPU never removed. Minimizes slow CPU fallback.
- Versions: OpenVINO 2026, ONNX Runtime, Vulkan (to be qualified in P5), Lunar Lake NPU present
- Impact: P5 will implement engines/asr-vulkan with same health/caching as OpenVINO; scheduler pools GPU accelerators.

## 2026-08-21 — Additional GPU runtimes beyond Vulkan/ OpenVINO for broad hardware coverage
- Question: Beside locked Vulkan extra GPU, what other runtimes could we add to maximize acceleration before CPU fallback?
- Sources:
  - ONNX Runtime Execution Providers official: https://onnxruntime.ai/docs/execution-providers/ — Default CPU, NVIDIA CUDA, NVIDIA TensorRT, Intel OpenVINO, Intel oneDNN, Windows DirectML, AMD ROCm (deprecated 1.23) / MIGraphX (ROCm 6+), Qualcomm QNN, XNNPACK, WebGPU, Arm Compute Lib
  - OpenVINO 2026 docs: https://docs.openvino.ai/systemrequirements, https://docs.openvino.ai/2026/openvino-workflow/running-inference/inference-devices-and-modes/npu-device.html — CPU/GPU/NPU on Core Ultra Lunar Lake, GenAI Whisper support, per-pair health required
  - Phoronix: Intel OpenVINO 2026.0 (2026-04) improved NPU for Core Ultra, LLM support
  - PORT-006: Compute backend contracts include CPU, Vulkan/CUDA/OpenVINO and future DirectML/Metal/CoreML
- Candidates evaluated for Linux 1.0 + roadmap:
  1. **Vulkan (LOCKED)** — cross-vendor GPU (Intel/AMD/NVIDIA) on Wayland, ships across ggml-based runtimes 2024-2026. 70-90% CUDA speed, fallback to CPU if no Vulkan driver. Low risk, broad payoff — **add now**.
  2. **CUDA (NVIDIA)** via ONNX Runtime CUDA EP (CUDA 11.8/12.2 + cuDNN 8.9) — best on NVIDIA discrete GPUs, ~2-3x vs Vulkan on NVIDIA. Isolated in engines/asr-onnx → **add now if NVIDIA present, health-check**.
  3. **TensorRT (NVIDIA)** via ONNX Runtime TensorRT EP — 10-30% faster than CUDA EP on same NVIDIA HW after engine build. Requires CUDA EP as fallback. Good for NVIDIA laptops/servers → **add as optional extra GPU EP (tried after CUDA)**.
  4. **MIGraphX (AMD RDNA)** — replacement for deprecated ROCm EP (removed ORT 1.23+). ROCm docs now recommend MIGraphX EP. For AMD Radeon 7000+ on Linux. Complements Vulkan on AMD (Vulkan generic, MIGraphX AMD-optimized) → **add as optional GPU EP, qualifies only on AMD**.
  5. **oneDNN (Intel CPU)** — already via OpenVINO CPU, but ONNX Runtime oneDNN EP gives extra CPU optimization (~15% vs default CPU EP) without GPU → **free CPU boost, add**.
  6. **WebGPU (future)** — ONNX Runtime WebGPU EP (browser/next-gen). Not needed for Linux 1.0 native, but roadmap for companion web → **defer to R14**.
  7. **DirectML / CoreML / QNN** — Windows/ macOS/ Qualcomm only → **roadmap R13** per BUILD_TARGET post-1.0, not Linux 1.0.
- Decision: Lock extra GPU capability set = `NPU (OpenVINO) → GPU pool [Vulkan + OpenVINO GPU + CUDA + TensorRT + MIGraphX] → CPU (default + oneDNN)`. Scheduler pools all qualified GPU EPs before CPU, per-device health. Vulkan + CUDA cover 90% Linux dGPUs; MIGraphX adds AMD-native path; TensorRT optional speed-up; oneDNN free CPU gain.
- Versions to record in P5: OpenVINO 2026.0, ONNX Runtime 1.22+ (check ORT 1.23 ROCm deprecation), CUDA 12.2/cuDNN 8.9 if installed, Vulkan driver from `vulkaninfo`, ROCm 6.1.3/MIGraphX if AMD present, Lunar Lake NPU driver (intel-npu-driver)
- Impact: P5 will implement with feature flags `cuda`, `tensorrt`, `migraphx`, `vulkan` under engines/asr-*; verify `cargo deny` licenses (CUDA EULA, TensorRT EULA) — redistribution not bundled, user must have driver. All extra EPs are optional, degrade gracefully to next GPU then CPU. No portable core dependency.

## 2026-08-21 — Model formats per accelerator locked
- Question: Which model kind for Intel NPU (needs OpenVINO)?
- Source: spec/schemas/model-manifest.schema.json, spec/examples/model-reference.example.json, docs/architecture/model-manager.md, docs/architecture/speech/model-catalog.md + openvino-npu.md
- Conclusion: Single source Whisper weights, 3 formats: OpenVINO IR (xml+bin) for Intel NPU/GPU (GenAI), ONNX QDQ for AMD NPU (Vitis AI), ONNX/GGML for GPU pool (CUDA/TensorRT/MIGraphX/Vulkan). Pinned immutable HF revision, conversion_recipe versioned, safe install transaction, per-device health.
- Versions: Whisper large-v3 + small, OpenVINO 2026.0, optimum-intel, RyzenAI 1.8
- Impact: P5 will publish 3 tiers with per-device health, no auto latest.

## 2026-08-21 — HF org narrivy + OpenVINO beyond a single ASR family locked
- Question: HF library strategy and OpenVINO single-family limitation?
- Source: openvinotoolkit.github.io/openvino.genai/docs/supported-models/ (GenAI supports speech models best, other ASR via similar arch may work), docs.openvino.ai/2023.3/omz_demos_speech_recognition_wav2vec_demo_python.html (Wav2Vec via OpenVINO IR), optimum-intel docs + model-catalog.md + PORT-006 (Vulkan/CUDA/OpenVINO + future)
- Conclusion: Own HF org `narrivy` locked as primary wherever possible, external HF as pinned candidate pool (no auto-promote). OpenVINO beyond any single family via generic IR (Wav2Vec2/Conformer/Parakeet etc via optimum/OVC) alongside GenAI Whisper, all in same NPU→GPU→CPU pool. Prevents external new-model auto-break.
- Versions: OpenVINO 2026.0 GenAI, optimum-intel 1.23+, large-v3/small-class weights + Wav2Vec2 as candidate
- Impact: P5 catalog will host narrivy/* converted IRs, Model Lab shows externals as unqualified candidates, health per model+runtime.

## 2026-08-21 — NPU quants across ASR families (Parakeet/Qwen3-class)
- Question: Which precisions besides 4-bit/8-bit for NPU? Is the single-family limitation true?
- Sources: docs.openvino.ai/2026/openvino-workflow-generative/inference-with-genai/inference-with-genai-on-npu.html (FP16 or INT8 on NPU), huggingface.co/OpenVINO/whisper-large-v3-int4-ov (INT4 via NNCF MatMulNBits), huggingface.co/OpenVINO/Qwen3-8B-int4-ov, medium.com/openvino-toolkit MiniCPM4 (fp32/fp16/int8 options), nncf docs (INT4_SYM mixed 4-bit, ratio 0.8 = 80% INT4+20% INT8), docs.openvino.ai/2025 npu-device.html (F16 + U8), openvino.genai/issues/1965 (FP16 vs INT8 on NPU)
- Conclusion: NPU supports FP32→FP16→INT8→INT4→mixed INT4/INT8; INT4 is weights compressed to 4-bit with per-group scaling (attention FP16). GenAI now supports that model family + Qwen3-VL + similar LLMs, generic OpenVINO covers Wav2Vec2/Conformer/Parakeet via OVC. Other precisions: BF16/FP8 emerging where HW exposes. All as separate narrivy/* manifests.
- Versions: OpenVINO 2026.0, NNCF 2.14+, optimum-intel 1.23+, large-v3/small-class weights, Parakeet-TDT 1.1B, Qwen3 8B
- Impact: P5 will expose 5 quants per model in catalog, user selects in Models & Hardware, cache invalidates on precision change.

## 2026-08-21 — NPU compiled cache locked
- Question: Need compile-once, cache file, fast reload on NPU pipeline?
- Source: docs/architecture/model-manager.md (Cache key includes model hash+recipe+runtime+device), docs/architecture/model-residency.md (Hot/Warm/Cold), docs/roadmap/speech/openvino-npu.md step 8 (persist health/cache keyed), docs/architecture/compute-scheduler.md (Hot/Warm/Cold)
- Conclusion: Locked — Cold first compile saves blob to cache, Warm loads blob, Hot resident. Key invalidates on any input change, dramatically cuts next load. Matches OpenVINO compiled_model cache behavior.
- Versions: OpenVINO 2026.0 compiled cache, ONNX Runtime EP context cache (Vitis AI EP supports it per relnotes 1.8)
- Impact: P5 implements atomic cache publish, measures Cold vs Warm in qualification.json

## 2026-08-21 — Cache default + quant recommendation UX locked
- Question: Should cache be default and clearable? How to guide quant choice?
- Source: docs/architecture/model-manager.md (cache key), model-residency.md (Hot/Warm/Cold), docs/product/ux/progressive disclosure + Models & Hardware screen, owner request 2026-08-21
- Conclusion: Locked cache default ON + Clear compiled cache button (per-model/all, size+invalidation). Quant picker asks on select with "Better for you" recommendation from hardware profile (RAM/NPU memory/battery). Not auto-lock, user overridable.
- Versions: OpenVINO 2026 cache, ONNX EP context cache
- Impact: P5 implements cache ON + clear UI, P7 Models & Hardware adds quant recommendation engine.

## 2026-08-21 — MIT license selected (time being)
- Question: Which open-source license for forkable use?
- Source: LICENSE_POLICY.md, LICENSE-TODO.md, owner choice 2026-08-21: MIT for time being
- Conclusion: MIT, Copyright 2026 Narrivy. THIRD_PARTY_NOTICES lists cargo-deny allow list + Whisper MIT / Parakeet CC-BY-4.0 / Qwen License. Crates remain publish=false until 1.0.
- Versions: MIT 1070 chars
- Impact: release_gate legal check now passes; full gate still requires P11 qualification + artifacts.

## 2026-08-21 — 8700G deferred plan for AMD NPU
- Question: Is Ryzen 7 8700G sufficient for AMD NPU qualification? Owner will provide later but want everything planned.
- Source: techpowerup.com/cpu-specs/ryzen-7-8700G (XDNA NPU up to 16 TOPS), servethehome.com (Phoenix Zen4+RDNA3 780M+XDNA), amd.com Ryzen 7 8700G drivers, community.frame.work amdxdna Linux 6.14 mainlined, ryzenai.docs.amd.com Linux STX/KRK (Phoenix supported)
- Conclusion: Yes, sufficient — 8700G has XDNA NPU (16 TOPS), works with amdxdna (Linux 6.14, Ubuntu 25.04) + XRT + Vitis AI EP for ONNX Runtime. XDNA1 (8700G) less LLM-optimized than XDNA2 (Strix) but Parakeet/Qwen3-class NPU will qualify; Vulkan/MIGraphX GPU also benefits from 780M. Deferred until hardware arrives, fallback to Intel NPU+GPU→CPU keeps Linux 1.0 passing.
- Versions: 8700G Phoenix, Linux 6.14+, Ryzen AI 1.8, Vitis AI EP
- Impact: B-HW-004 marked DEFERRED, hierarchy keeps AMD NPU slot, P5 will add qualification when hardware arrives.

## 2026-08-21 — Activation Hold PTT + Toggle locked
- Question: Need two activation kinds: hold vs toggle?
- Source: owner request 2026-08-21, legacy planning Part C P0F (prove activation/toggle vs PTT key-release), FUN-001
- Conclusion: Locked both via same shortcut, capability-dependent. PTT needs release event (portal may not provide), toggle works everywhere. P0F will qualify both on COSMIC+GNOME.
- Versions: Wayland portal GlobalShortcut, libei/EIS fallback
- Impact: Settings → Automation/Activation exposes Hold vs Toggle, P0F probes both, P2 walking skeleton supports both.

## 2026-08-21 — Overlay streaming vs non-streaming
- Question: Does overlay support both streaming live text and normal equalizer only?
- Source: owner request 2026-08-21, FUN-003 (Stream tentative where engine supports), docs/architecture/speech/asr-contract.md (Partial/Final events), supplemental Part 40 (floating Listening + live text), docs/product/ux/overlay.md
- Conclusion: Locked dual mode: streaming → equalizer + live partial, non-streaming → equalizer only. Registry marks streaming capability per artifact.
- Versions: Whisper Turbo streaming, Zipformer streaming vs Whisper Large non-streaming
- Impact: Overlay subscribes to Partial events if available, else shows voice only.

## 2026-08-21 — owner architecture clarification (normative supersession note)
- Earlier research entries use shorthand `NPU→GPU→CPU`. the current architecture refines this to **NPU→dGPU→iGPU→CPU** and makes runtime selection qualification/benchmark-driven rather than tier-only.
- Earlier runtime conclusions remain candidate research, not permanent support claims. Exact Intel/AMD/Vulkan/OpenVINO/vendor capabilities and current versions must be reverified from authoritative upstream sources at P5 implementation and P11 release qualification.
- New design adds a lean base installer, optional runtime packs, explicit manual override, and reference-aware cleanup of losing benchmark-generated artifacts.

## 2026-08-22 — module/pipeline ecosystem research snapshot
- Treat capability slots, not individual libraries, as the durable architecture: AEC, beamforming, dereverb, denoise, source separation, target-speaker extraction, VAD/endpointing, keyword spotting, diarization/speaker ID, LID/audio tagging, ASR, ITN/punctuation/spell/grammar, translation, LLM/provider and delivery.
- Candidate evaluation inventory is documented in `docs/roadmap/speech/module-candidate-catalog.md`; inclusion is not a redistribution/support commitment and code/model licenses must be re-verified.
- Resource distribution architecture prefers authoritative upstream sources with Narrivy-verified mirrors/conversions, immutable hashes/signatures, staged activation, reference counting and disposable qualification-cache cleanup.
- Local postprocessing can share or split accelerator resources with ASR; whole-graph qualification decides rather than assuming NPU co-residency is always optimal.


## 2026-08-22 — version-currency audit of catalog runtimes/modules (owner request: latest per node)
- Question: What are the current latest upstream versions for every pipeline node's module bindings?
- Sources (checked 2026-08-22): github.com/openvinotoolkit/openvino/releases; github.com/microsoft/onnxruntime/releases; github.com/ggml-org/llama.cpp/releases; pipewire.org + Wikipedia/Phoronix PipeWire 1.6; github.com/snakers4/silero-vad (releases + discussions #471/#678); github.com/Rikorose/DeepFilterNet; github.com/k2-fsa/sherpa-onnx/releases; gitlab.xiph.org/xiph/rnnoise (v0.2, Apr 2024); pypi.org/project/nemo-text-processing (r1.2.0); huggingface.co/openai/whisper-large-v3-turbo.
- Conclusions (latest upstream as of 2026-08-22):
  - OpenVINO **2026.3** (2026-08-04, development release); **2025.4.1** is the LTS/maintenance line (archive-verified). P5 must pick dev-2026.3 vs LTS-2025.4.1 deliberately (release-grade tradeoff), record choice here.
      - ONNX Runtime **1.28.x** (patch verified 2026-08-17). sherpa-onnx currently bundles ORT 1.27.x.
  - llama.cpp: rolling releases (semver v0.1.x / b10xxx, multiple per day; latest ~2026-08-21). Pin exact build when the optional text.llm-postprocess module lands.
  - PipeWire **1.6.8** (2026-07-09) upstream; support lanes pin distro versions instead (host lane: Pop!_OS 24.04 PipeWire). Narrivy targets the PipeWire API, not a floating version.
  - Silero VAD **v6.0** (v5: 3x faster, 6000+ langs; v6: 16% fewer errors on noisy data). Default VAD candidate at P4.
  - RNNoise **0.2** (2024-04, AVX2/SSE4.1; source of truth gitlab.xiph.org). Still latest.
  - DeepFilterNet **DeepFilterNet3 v0.5.x** — still latest official; no 2026 release.
  - sherpa-onnx **v1.13.6** (2026-08-17) — very active; covers VAD/punctuation/streaming ASR/diarization; strong candidate for true-streaming decision at P5.
  - nemo-text-processing **r1.2.0** (Apache-2.0, Pynini/WFST; Python) → isolated-worker/sidecar candidate only; lean default stays native-itn.
  - Whisper weights: **large-v3-turbo (2024-10) remains the newest open-weight Whisper** (no v4); 809M params, 4 decoder layers, ~5.4x faster than large-v3. Candidate for the Balanced/latency tier alongside whisper-small; large-v3 stays Accuracy tier. GPT-4o-transcribe is API-only (cloud lane, not 1.0 default).
  - Not yet verified this pass (verify when the phase that uses them starts): GTCRN, WebRTC APM/ns snapshot, LanguageTool, SymSpell, Hunspell, wtype, wl-clipboard, speexdsp versions.
- Impact: nothing external is pinned in the v1.3 tree yet (Cargo.lock = workspace-only; engine bindings land feature-gated). Locking happens at the implementing phase per the volatile-facts rule; product pins are always immutable revisions/tags, never floating "latest". Owner direction 2026-08-22: prefer latest stable upstream at lock time.

## 2026-08-22 — remaining utility versions + local-LLM post-processing lanes (owner follow-up)
- Sources (checked 2026-08-22): github.com/atx/wtype; github.com/bugaevc/wl-clipboard (+brew/freshports); dev.languagetool.org/roadmap + Wikipedia (LT 6.8, 2026-05); github.com/wolfgarbe/SymSpell; github.com/hunspell/hunspell; docs.openvino.ai/2026 GenAI-on-NPU page; phoronix.com OpenVINO 2026.0 release; openvino GitHub discussion #36484 (GPU-target+NPU-draft speculative decoding on Lunar Lake); reddit.com/r/LocalLLaMA Qwen3-8B-on-Intel-NPU project; HF blog "Understanding NPUs with OpenVINO" (static-shape constraint); reddit/Medium/Zen van Riel/Intel guides on llama.cpp Vulkan/SYCL on iGPUs; 2026 SLM comparisons (Gemma 4 E2B/E4B, Qwen3.5/3.6, Phi-4-mini, Gemma 3 4B).
- Utility versions: wtype **v0.4** (latest; stable/mature, no newer release); wl-clipboard **2.3.0** (~2026-03); LanguageTool core **6.8** (2026-05-05; project moved to snapshot-based releases); SymSpell active (pin exact tag when text.spell module lands); hunspell standard (pin at use); GTCRN still unverified (research-grade repo, candidate only); WebRTC APM ships as libwebrtc snapshots (no standalone version; qualify per distro build).
- **Local-LLM post-processing (text.llm-postprocess) findings:**
  - NPU lane is real and official: OpenVINO GenAI 2026 supports LLM inference on NPU starting exactly at our hardware (Core Ultra Series 2 / Lunar Lake baseline); Qwen3-8B-class models demonstrated on Intel NPU; OpenVINO 2026.0 release notes explicitly expanded LLM + NPU support; speculative decoding (GPU target + NPU draft) demonstrated on Lunar Lake (discussion #36484). Constraint: NPU compiler needs **static shapes** — our post-processing use case (bounded input window, bounded output budget) fits this well.
  - iGPU lane caveat: llama.cpp **Vulkan on Intel iGPUs is hit-or-miss** (reports of ~1 tok/s; Intel's own guidance prefers SYCL). On Lunar Lake the better iGPU LLM lane is **OpenVINO GPU from the same runtime pack** as the NPU path; llama.cpp/Vulkan remains the cross-vendor lane (strong on AMD APUs) and CPU fallback. → openvino-llm-postprocess (NPU+iGPU) is the primary local binding on this host class; llamacpp-llm-postprocess (Vulkan/GGUF) is the cross-vendor alternate; both already exist as catalog bindings.
  - Model candidates as of 2026-08 (revisit locked 2026-08-21 "Qwen3-8B INT4" decision at P5): for "fix/correct the text" (Faithful Cleanup — short prompt, short output, latency-critical), the 1.5–4B INT4 class is the sweet spot (fits NPU memory, meets release→committed budget): **Qwen3.5 ~4B-class INT4, Gemma 4 E2B/E4B (~2GB class), Phi-4-mini, Gemma 3 4B (140+ languages)**. Keep an 8B-class INT4 as optional accuracy tier only. Pin exact model+revision via narrivy/* HF conversions at P5 qualification.
- Integration state (truthful): contract/ADR/catalog level complete (ADR-0018 levels + semantic guard, module bindings openvino/llamacpp/cloud, Smart Local recipe, whole-pipeline qualification incl. NPU-coresidency-vs-split comparison); **no implementation exists yet by design** — lands with P5 runtime packs + P7 guarded post-processing contracts; v0.4 archive had no LLM post-processing (raw/clean deterministic only), so acquisition machinery (downloader/runtime-pack lifecycle) is salvaged, model+engine integration is new work.
- Impact: P5 must (a) pick OpenVINO 2026.3 vs LTS 2025.4.1, (b) qualify small-LLM candidates on NPU+iGPU with static shapes, (c) benchmark whole-graph ASR-on-NPU + post-on-iGPU/CPU vs co-residency, per compute-scheduler.md.

## 2026-08-22 — transcribe.cpp identified and scoped (owner question)
- Question: What is transcribe.cpp, and does it belong in our stack?
- Sources: blog.mozilla.ai/announcing-transcribe-cpp (official announcement, 2026-06-30); gigazine.net coverage 2026-08-11; github.com/handy-computer/transcribe.cpp; secondary HN/press.
- What it is: C/C++ STT inference library — "llama.cpp for STT" — on the ggml runtime, running multiple model families from GGUF (Whisper, Parakeet, Moonshine, Vosk, Canary, ZipVoice; secondary sources claim 16+ families / 60+ variants). Backends: Metal, Vulkan, CUDA (+CPU via ggml). Positioned as a near drop-in whisper.cpp successor with native streaming model support (Parakeet streaming/EOU variants); streaming details are not confirmed on the official announcement page — verify in-repo.
- Provenance/clean-room note: maintained under the `handy-computer` GitHub org. CLEAN_ROOM_POLICY prohibits copying the reference product's implementation/UI/assets, but using an upstream open-source *library* after provenance/license review is explicitly permitted (same rule as whisper.cpp/RNNoise). License must be verified at evaluation.
- Fit assessment: promising candidate for the true-streaming gap (FUN-003) in the GPU/CPU lanes — ggml/Vulkan aligns with our GPU pool; multi-family GGUF models align with the Parakeet/Moonshine candidates already in the engine catalog. NO NPU/OpenVINO path, so it cannot serve the NPU lane. Maturity risk: released 2026-06-30, no version/stability claims, young track record vs whisper.cpp (v1.9.3) and sherpa-onnx (v1.13.6).
- Decision (SUPERSEDED same day by owner): PROMOTED to the reference CPU/iGPU/dGPU ASR runtime outright. Streaming-lane candidates now: (1) sherpa-onnx Zipformer-class, (2) transcribe.cpp Parakeet-streaming, (3) whisper.cpp simulated partials (status quo fallback). Recorded in upstream-version-audit pending decision #3.

## 2026-08-22 — owner's Handy-Pop-OS fork audited (iamwaqargulzar/Handy-Pop-OS) — NPU knowledge salvage
- Source: the owner's public Handy-Pop-OS fork (MIT; owner's NPU work in commits 016b175..c366fbb, 2026-08-13), docs/openvino-npu-linux-design.md, experiments/openvino-gate1/2. Owner-authored surface now extracted to code/handy-npu/ (upstream tree and fork history removed per owner decision 2026-08-22).
- **Verified platform facts (Core Ultra 9 288V, OpenVINO GenAI 2026.3, NPU UMD 1.35.0, Level Zero loader 1.32.0, distro intel_vpu + render group, env DISABLE_OPENVINO_GENAI_NPU_L0=1):**
  - OpenVINO 2026.3 C++ ASRPipeline is the correct native boundary; the vendor's Rust 0.11 pipeline wrapper for those weights was unstable with forced language. → evidence for choosing 2026.3 over LTS at P5.
  - Whisper Large V3 INT8 on NPU: cold compile 144–170 s at ~6.9 GiB peak; compiled cache hit 7.63 s (weightless cache = 2.9 GB disk); warm RSS best-correct ~4.50 GiB (stable, no per-transcription growth; tested across allocator trimming, both speech APIs, NPU memory props, native L0, 2026.2/2026.3, stateless export); warm 11 s audio ≈ 1.36 s; installed-.deb JFK 11 s = 3.797 s.
  - Parakeet TDT 0.6B V3 on NPU (via Apache-2.0 Eddy decoder): cold load 27.2 s, **11 s audio transcribed in 220–240 ms** — fastest NPU ASR measured; streaming-capable architecture.
  - Qwen3-ASR 1.7B INT8 on NPU: 11 s JFK = 2.037–2.447 s; INT4 runs, labelled accuracy-tradeoff. Model sizes: INT8 2.1 GiB, INT4 1.4 GiB.
  - Runtime closure package (worker + private OpenVINO runtime, no weights): 122.6 MiB compressed.
- **Engineering recipes validated (reusable knowledge for Narrivy model-conversion pipeline):**
  - Qwen3-ASR official export cannot run on the 2026.3 NPU plugin (dynamic encoder/KV-cache shapes). Working conversion: keep speech encoder; map speech-trained decoder weights into standard Qwen3ForCausalLM; export stateful inputs_embeds/attention_mask/position_ids interface; chunk mel into fixed 100-frame encoder calls on NPU; merge audio/text embeddings on CPU; bounded decoder via NPUW on NPU; 1024-token prompt bucket ≈ 30 s ASR window. Symmetric INT8 per-channel / INT4 group-128 decoder variants.
  - Isolated-worker pattern: versioned Unix socket + framed JSON + raw 16 kHz mono f32; probe/load/transcribe/unload/status/shutdown protocol; cancellation by terminate-and-restart; PR_SET_PDEATHSIG parent supervision; per-process monotonic socket suffix; one reconnect/reload retry. Maps 1:1 onto Narrivy's sidecar trust class (ADR-0014) and ASR contract cancellation.
  - Honesty rules matching ours: actual-device reporting always; Eddy's silent NPU→CPU fallback disabled (compile failure = load failure); no silent CPU/GPU execution of NPU-labeled models; removability of the whole NPU route; weights on demand only; runtime-versioned compiled cache under the model dir (deleted with the model).
- **Catalogue**: 39 pinned official OpenVINO Whisper repos (Turbo/Large V2-V3/Distil/Medium/Small/Base/Tiny × INT4/INT8/FP16 × multilingual/en) + Parakeet TDT V3 + Qwen3-ASR 1.7B INT8/INT4, immutable revisions + SHA-256.
- Impact on Narrivy decisions:
  1. Streaming (audit decision #3) gains a fourth candidate that runs ON THE NPU LANE: Parakeet-TDT-via-Eddy/OpenVINO (transcribe.cpp is GPU/CPU-only; sherpa-onnx is ONNX-runtime-bound). Parakeet's 220–240 ms warm latency makes it a default-pipeline contender, not just a streaming experiment.
  2. Large-V3-INT8-on-NPU is validated but heavy (4.5 GiB RSS, 2.9 GB cache, 144–170 s cold) → confirms Narrivy needs the Cold→Warm cache pipeline as a background/first-run cost, and pushes the Balanced tier toward Parakeet/Qwen3-ASR/Turbo class on NPU.
  3. OpenVINO 2026.3 now has direct verified evidence on this hardware family (was previously "latest but unproven vs LTS").
- Clean-room handling: per CLEAN_ROOM_POLICY the fork is behavioral-oracle/knowledge only — the above is recorded as knowledge. CODE reuse (owner-authored NPU worker, Eddy integration, Qwen conversion scripts) is MIT-compatible and partly owner-authored, but requires an explicit owner policy amendment (like the old/ salvage decision) before any code is copied. Decision pending with owner.

## 2026-08-22 — owner's daily-driver reference configuration (owner statement)
- Owner clarification: their current daily dictation setup IS their Handy fork with their own NPU changes, running **Qwen3-ASR (1.7B, INT8) on the NPU** — actively used, not an archived experiment.
- Impact:
  1. The fork is the behavioral regression oracle in its exact daily configuration (trigger → mic → Qwen3-ASR on NPU → text).
  2. Qwen3-ASR 1.7B INT8 on NPU is an owner-validated, daily-accepted model — first-class candidate for the P5 ASR tier decision (measured in-fork: 11 s audio ≈ 2.0–2.4 s; INT4 = smaller opt-in).
  3. Parakeet TDT (220–240 ms) remains the measured-latency leader; Whisper large-v3/turbo the accuracy/library incumbents. P5 qualification compares all with owner preference weighted.
- Related: code/handy-npu/ (owner's extraction incl. Qwen3-ASR NPU conversion recipe — static-shape decoder remap into Qwen3ForCausalLM, 100-frame mel chunking, NPUW bounded decoder).

## 2026-08-22 — owner experiment pipeline: all-NPU (personal, non-default)
- Owner intent: a personal custom Pipeline with every ML node on the NPU — NPU denoiser + NPU ASR + NPU LLM text correction (capture/delivery remain platform I/O by nature). Not a default; an experiment to run once P5 lands.
- Feasibility per verified facts: ASR-on-NPU owner-proven (Qwen3-ASR 1.7B INT8 daily; Parakeet TDT 220-240 ms; Whisper large-v3 INT8); LLM-on-NPU official via OpenVINO GenAI 2026 (1.5-4B INT4 class); denoiser-on-NPU is the unbuilt piece (openvino-denoise binding: GTCRN/DFN3-class static-shape IR export + qualification).
- The experiment doubles as the co-residency test case for whole-pipeline qualification: Qwen3-ASR (2.1 GB) + 4B-INT4 LLM (~2.5 GB) + denoiser resident on NPU together vs. the split plan (ASR NPU + LLM iGPU). Either outcome produces the scheduler evidence compute-scheduler.md needs.
- Catalog gap exposed: speech.asr has no Qwen3-ASR binding yet — add when the owner's qwen3 NPU work is salvaged at P5 (engine qwen3-asr, runtime openvino).
- Schema fit: nodes+edges DAG supports it today (device_preference ["npu"], selection manual_locked per node); multiplicity rules unaffected.

## 2026-08-22 — full per-slot candidate sweep (research agent, web-verified)
- Question: better/other module options per pipeline node beyond the current catalog. Verified against project pages/HF cards/releases/arXiv; full table with sources in the module-candidate-catalog addendum.
- **Recommended additions** (verify unverified-flagged licenses before adoption):
  - ASR: Fun-ASR-Nano/MLT-Nano-2512 (Apache-2.0, official GGUF, streaming, built-in punct+ITN, 31 langs MLT); NVIDIA Nemotron Speech Streaming 0.6b (ONNX streaming, int4, CPU real-time proven at 0.67GB RAM / 8.2% streaming WER — best NPU/CPU-native streaming architecture, static shapes → OpenVINO); Moonshine v2 (tiny EN tier); Voxtral Mini 4B Realtime (roadmap/cloud-parity, no GGUF/ONNX yet); Urdu lanes: whisper-large-v3-urdu fine-tune (conversational WER 17.86) + AI4Bharat IndicConformer-STT-UR; Parakeet entry upgraded v2→v3 (CC-BY-4.0).
  - VAD/endpointing: TenVAD (306KB vs Silero 2MB, −32% RTF, ONNX — footprint tier); livekit/turn-detector + pipecat smart-turn (EOU endpointing polish, ONNX/HF).
  - Punctuation: CT-Transformer via sherpa-onnx/FunASR (zh/en ONNX slot).
  - LLM post-process: Gemma 4 E2B/E4B (Apache-2.0 now! official GGUF + OpenVINO INT4, audio-capable — best default cleanup candidate); Qwen3.5-2B (201 langs; avoid 0.8B thinking-loop); LFM2.5-350M/1.2B (LFM Open License, OpenVINO GenAI official, Encoders do targeted correction = our exact contract).
  - Wake-word (roadmap): openWakeWord + microWakeWord (both Apache-2.0).
  - Serving: transcribe.cpp is the reference in-process GGUF lane; Speaches (MIT, OpenAI-compatible realtime WebSocket) recorded as optional sidecar protocol; pykeio/ort (Rust ORT with OpenVINO/QNN EPs) is the ONNX substrate; sherpa-onnx has Whisper+QNN NPU path.
- **Architectural finding**: OpenVINO GenAI (2025.3+) loads GGUF directly — GGUF and OpenVINO-IR can share one runtime stack, materially simplifying the LLM/ASR lanes.
- **Rejections**: Porcupine (proprietary ~$6k), IBM Granite Speech 3.3 (no ONNX/GGUF, conversion fails), SenseVoice-Small (non-commercial), ZipEnhancer (no ONNX, license unverified, offline-only), FRCRN (license unverifiable), candle-as-primary (no NPU EPs), "UPSLEEP" (does not exist — garbled reference), TSDCA-BA/aTENNuate/Weighted-Lookahead (no artifacts / wrong NPU target).
- **Caveats for live-lane planning**: sherpa-onnx speech enhancement (GTCRN/DPDFNet) is offline-batch-only (issue #2670) — not for the live denoise lane; DeepFilterNet3 remains the top real-time denoiser per 2025 comparisons (current default stands).

## 2026-08-22 — Urdu/Hindi linguistic context (owner-provided) + language-lane verification
- Owner note: Hindi and Urdu are sister languages — conversational speech overlaps heavily (Hindustani register), but scripts differ entirely (Devanagari vs Perso-Arabic) and formal vocabulary diverges (Sanskrit vs Perso-Arabic loans).
- Engineering consequence: **"Hindi supported" must NOT be counted as Urdu support** — output script matters (FUN-029 keeps spoken language / written script / translation separate). Urdu-lane qualification requires Urdu-script output on the Urdu fixtures (fixture-short-urdu-4s.wav; RTL delivery-matrix fixtures).
- Verified status of Urdu lanes: Whisper family has native `ur`; community whisper-large-v3-urdu fine-tune = best conversational Urdu (WER 17.86, COLING 2025 benchmark); AI4Bharat IndicConformer-STT-UR = best South-Asian accent/code-switch lane; Qwen3-ASR = Urdu reported within its 30-language set (Qwen3-Omni lineage lists Urdu) but HF card table not yet directly verified — the owner, an Urdu speaker, dictates daily with Qwen3-ASR, which is strong empirical evidence; confirm with the Urdu fixture at P5 qualification. Fun-ASR-MLT-Nano: Hindi confirmed, Urdu NOT in the surfaced list → treat as not-Urdu-qualified.

## 2026-08-22 — fork overlay: authorship resolved, knowledge salvaged (owner question)
- Question: can the fork's overlay code be copied to Narrivy?
- Facts (git history of the public fork): overlay implementation (RecordingOverlay.tsx, overlay.rs) is upstream multi-author code; owner-side commits touched only CSS styling (dark neutral) and integration/checkpoint commits. Per the owner's clean-room amendment (upstream = behavioral reference only; no attribution-free copying) the CODE is not salvageable.
- Salvaged instead (owner-authored, verbatim): `docs/2026-08-01-overlay-changes-investigation.md` + companion README → `code/handy-npu/docs/`. Encodes: COSMIC wrong-monitor root cause (GDK pointer (0,0); XWayland scale mismatch; foreign-toplevel lacks output info) + the 1×1 layer-shell probe solution; stale-text clear-on-entry rule; monitor-relative overlay geometry policy (35% width, 400-760 clamp, 72px start, 50% max, 200ms first-text ease); hidden-start service-init lesson.
- Encoded as binding P3 overlay requirements in `docs/product/ux/overlay.md` § COSMIC/Wayland platform truths.
- Code candidate for Narrivy overlay remains its own v0.4 implementation (React overlay page + hidden Tauri WebviewWindow) in code/narrivy-v0.4 per the salvage inventory.

## 2026-08-22 — transcribe.cpp license VERIFIED (MIT); upstream adoption confirmed
- Sources (checked 2026-08-22): github.com/handy-computer/transcribe.cpp (LICENSE + repo README: "transcribe.cpp is MIT-licensed"; vendored ggml and miniz also MIT, attributed in its THIRD-PARTY-LICENSES.md); github.com/cjpais/Handy README.
- Conclusion: (1) transcribe.cpp is MIT — license review for the P5 lock PASSED; vendoring/binding is permitted with copyright+notice preservation; add it to THIRD_PARTY_NOTICES at P5/P10. (2) Upstream Handy now uses transcribe.cpp as its STT runtime — the owner's daily-driver lane and this decision are aligned with upstream direction.
- Impact: engines/transcribe-cpp `runtime` feature is cleared to link against the library once the immutable revision is pinned (P5); BUILD-NOTES license TODO resolved; maturity-risk note stands (young project, gate at P5).
## 2026-08-27 — Reis 0.7.1 and EI text-sender source audit

- Primary sources: current `ids1024/reis` master/Cargo manifest and MIT
  license; upstream EI protocol overview; `ei_text`; `ei_device`; libei sender
  API. Reis master and crates.io remain version 0.7.1 on the audit date.
- Host facts: the installed `libei.so.1` is package version 1.2.1 and does not
  export the text sender symbols introduced in libei 1.6; no libei development
  pkg-config entry is installed. COSMIC compositor protocol support is not
  inferred from that client-library version.
- Open-source comparison: wdotool and KDE Connect demonstrate portal/libei
  sender ownership patterns, but their keycode/token/platform-selection choices
  do not satisfy Narrivy's narrower privacy and capability rules. Helper
  execution would also reintroduce process and payload-lifecycle ambiguity.
- Reis findings: pure Rust, MIT, Rust 1.71.1 MSRV, `unsafe_code = forbid`.
  `REIS_DEBUG` prints wire arguments. The default handshaker advertises broad
  interfaces and ignores a flush result. The blocking iterator has no caller
  deadline. The Tokio wrapper lacks writable readiness. The high-level EI
  converter is genuinely non-`Send` because it stores non-`Send` callbacks.
  The low-level Context/proxies are `Send`, expose a nonblocking FD, retain
  buffered bytes across `EAGAIN`, and support a custom minimal handshake.
- Implementation consequence: ADR-0026 uses Reis only for low-level typed wire
  encoding/decoding, rejects `REIS_DEBUG`, polls readable/writable readiness via
  rustix under the caller deadline, and conservatively treats any incomplete
  post-flush operation as dispatch-uncertain because Reis does not expose exact
  partial-write progress. Controlled tests decode the real socket traffic with
  an EIS peer; they are not compositor or native target proof.

## 2026-08-27 — GTK3/Tauri parent export and desktop-runtime audit

- GTK 3.24 implements Wayland parent export asynchronously. Every accepted
  call increments an export count; unexport must occur the same number of
  times. Unexport destroys pending closures, invokes their destroy notifiers,
  removes an idle callback, and clears the opaque handle. Window finalization
  warns if callers did not unexport promptly.
- Tauri 2.11.5 exposes the invoking `WebviewWindow`, GTK
  `ApplicationWindow`, focus/visibility queries, and main-thread scheduling.
  The parent can therefore stay entirely in Rust; it never needs a
  TypeScript round-trip.
- ashpd 0.12's raw Wayland converter is not used. Its locked implementation
  starts an unowned standard thread, performs blocking Wayland roundtrips with
  no Narrivy deadline, reconstructs objects from foreign pointers, unexports
  only on `Drop`, and includes the handle in `Debug` output.
- `adapters/desktop-parent-linux` instead contains one reviewed GTK3 FFI
  module and an exact-count, main-thread registry. Independent review passed
  after tests proved main-thread wait rejection, wrong-ticket preservation,
  exact normal release, second-release failure, and idempotent orphan cleanup.
  It remains an isolated workspace member: no app/service depends on it.
- Supply-chain result is release-blocking: offline `cargo deny` reports the
  GTK3 Rust binding family as unmaintained, `glib 0.18.5` under
  RUSTSEC-2024-0429, and an unmaintained macro dependency. These exact packages
  already existed through Tauri/Wry at the base workspace; the adapter adds no
  new affected registry package and makes no dependency-qualification claim.
- Upstream Wry still documents GTK/WebKitGTK for Linux, its release audit shows
  the same GTK3 advisories, and the GTK4/WebKitGTK 6 migration PR remains open
  as of this review. A routine Tauri upgrade cannot currently clear this gate.
- Desktop-owner consequence: do not unexport after merely sending Cancel. A
  second authenticated connection must signal the exact operation, an
  unconditional revoke must close the pending-versus-Active race, and the
  original blocking permission request must reach terminal cleanup before
  ticket release. Tauri restart ignores normal exit prevention, so restart
  must be prohibited while authority is pending/active unless daemon cleanup
  becomes client-disconnect-owned.

## 2026-08-27 — Portable dependency-boundary failure is both false-positive and false-negative

- `verify_dependency_boundaries.py` reports `windows-sys` under core, IPC, and
  pipeline solely through `tempfile -> rustix/errno` target-specific metadata.
  IPC/pipeline use `tempfile` only for tests. Cargo metadata filtered to the
  Linux target and Linux normal/build trees contain no `windows-sys` edge.
- The source validator simultaneously misses real production coupling because
  it does not scan standard-library platform modules. Core journal imports Unix
  `PermissionsExt`; IPC directly owns Unix streams/listeners, endpoint modes,
  `SO_PEERCRED`, and `flock`, contradicting the documented wire-only portable
  IPC boundary.
- Removing or downgrading `tempfile` would not repair that architecture and
  would risk replacing reviewed atomic-file behavior with homegrown code.
  Target-filtering the existing gate alone would manufacture green status.
- ADR-0030 therefore proposes splitting Unix transport and secure journal
  persistence from portable contracts, strengthening the source scanner, and
  making metadata validation direct/dependency-kind aware while retaining a
  Linux/Windows/macOS portable compile matrix. The gate remains red until a
  dedicated implementation and review complete.

## 2026-08-28 — Open-source dictation latency and Linux delivery comparison

- Primary sources: [Nerd Dictation](https://github.com/ideasman42/nerd-dictation),
  [Handy](https://github.com/cjpais/Handy),
  [Speech Note](https://github.com/mkiol/dsnote),
  [whisper.cpp streaming example](https://github.com/ggml-org/whisper.cpp/tree/master/examples/stream),
  [whisper.cpp VAD documentation](https://github.com/ggml-org/whisper.cpp#voice-activity-detection-vad),
  [wtype](https://github.com/atx/wtype),
  [Wayland virtual-keyboard protocol](https://wayland.app/protocols/virtual-keyboard-unstable-v1),
  [ydotool](https://github.com/ReimuNotMoe/ydotool),
  [XDG RemoteDesktop portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.RemoteDesktop.html),
  and [libei sender API](https://libinput.pages.freedesktop.org/libei/api/group__libei-sender.html).
- Mature low-latency patterns converge on capture/recognition overlap, a
  resident model or process, bounded audio windows, and VAD endpointing. Nerd
  Dictation preserves loaded state through suspend/resume; Handy separates
  audio/VAD/model/transcription ownership and exposes idle model eviction;
  whisper.cpp documents a 500 ms streaming step as an example rather than a
  universal tuning value.
- Linux injection mechanisms submit input events; they do not acknowledge that
  a target widget accepted text. wtype uses the optional virtual-keyboard
  protocol, libei explicitly leaves event handling to the EIS implementation,
  and the portal is user-mediated. A successful process/protocol submission is
  therefore `SubmittedUnverified`, never verified delivery.
- ydotool requires a persistent `/dev/uinput` daemon and commonly elevated
  device access. It remains explicit opt-in only; Narrivy must not silently add
  privileges, install permissive udev rules, or start a root helper.
- Architectural consequence: preserve exactly-one Inbox recovery after
  unverified/uncertain dispatch and never try a second direct backend after any
  positive dispatch. The next latency program should prioritize a supervised
  resident ASR worker, in-memory PCM, VAD endpointing, and cold-versus-warm
  p50/p95 measurement before adding more delivery backends. Chunk/window values
  require Narrivy-native accuracy, duplication, CPU, and cancellation tests.

## 2026-08-28 — transcribe.cpp streaming capability is model-dependent

- Primary sources: transcribe.cpp 0.2.2 Rust binding and capability API;
  [Moonshine Streaming model documentation](https://github.com/handy-computer/transcribe.cpp/blob/main/docs/models/moonshine-streaming.md);
  [Parakeet model documentation](https://github.com/handy-computer/transcribe.cpp/blob/main/docs/models/parakeet.md);
  and the [whisper.cpp stream example](https://github.com/ggml-org/whisper.cpp/blob/master/examples/stream/README.md).
- transcribe.cpp exposes a genuine streaming API only when the loaded model
  reports streaming support. Moonshine Streaming documents cumulative
  right-context around 240 ms, 80 ms input cadence, and 20 ms output cadence.
  Standard Parakeet TDT v2/v3 models are offline; Parakeet Unified is a distinct
  streaming-capable variant. The installed Whisper tiny model used by the
  recovery lane is final-only.
- The whisper.cpp example repeatedly transcribes moving windows and optionally
  applies a simple energy threshold. It is a useful experiment, not proof of a
  model-native streaming contract or a production endpointer.
- Consequence: Narrivy keeps `capabilities().streaming = false`, removes the
  catalog's blanket Parakeet streaming claim, and does not manufacture partials
  from repeated final decodes. A future streaming phase must select and
  integrity-pin a streaming-capable model, bind the upstream stream API, extend
  the worker protocol and core lifecycle, implement VAD/endpoint ownership, and
  qualify latency, accuracy, cancellation, duplication, privacy, and UX.

## 2026-08-28 — Exact 0.2.2 binding contract for capability attestation

- The exact locked Rust binding exposes immutable
  `Model::capabilities().supports_streaming` after native model load. The
  separate stream lifecycle borrows a session across `begin`, zero or more
  `feed` calls, and `finalize`/reset/drop; it owns the model compute lease for
  that entire interval and refuses overlapping work.
- `feed` returns change metadata and text is retrieved as full, committed, and
  tentative snapshots. Native revision numbers and committed/tentative text
  semantics must be mapped deliberately; the one-shot decode protocol cannot
  be relabeled as streaming.
- Implementation consequence: Recovery 4D transports only the immutable model
  capability during startup. A later ADR must define multi-operation worker
  framing, live PipeWire chunk ownership, endpointer/VAD rules, partial rewrite
  semantics, compute admission, backpressure, cancellation during a native
  feed, and final/correction/privacy/delivery ordering.

## 2026-08-28 — Native stream snapshot and compute-lease mapping

- The upstream stream owns the model compute lease from begin through the
  earliest finalize/reset/drop. Narrivy therefore reserves the resident worker
  for the same opaque stream identity across every operation; per-feed locking
  without persistent admission would allow illegal one-shot overlap.
- Upstream revisions are monotonic but are native snapshot metadata, not the
  product event sequence. Narrivy validates non-regression and requires a
  strict revision increase for `result_changed`, while assigning its own
  monotonic sequence only to emitted, non-duplicate Partial/Final events.
- `committed + tentative` is the stable partial display surface; `full` is the
  authoritative final surface. Partial snapshots remain volatile memory only
  and do not enter journal, policy, correction, delivery, logs, or evidence in
  Recovery 4E.
- A synchronous native feed prevents the worker from reading a second cancel
  command while blocked. Process isolation remains essential: a clonable
  parent cancellation token interrupts the pipe wait and the parent kills and
  reaps the worker. Native callback cancellation would require a separately
  owned worker-control mechanism without detached threads or unsafe lifetimes.

## 2026-08-28 — Resident stream admission is part of the compute lease

- A successful native `session.stream(...)` may acquire model state before the
  worker's STARTED response reaches the parent. Therefore a failed, timed-out,
  cancelled, EOF, or malformed begin is an uncertain dispatch, not a harmless
  pre-admission failure. The only safe recovery is to terminate/reap the worker
  and re-attest a replacement.
- The portable ASR scaffold previously exposed cancellation only after
  `open_stream` returned. Passing the caller token into admission closes that
  gap without a detached control thread and preserves one token across the
  entire stream lifecycle.
- Process-memory-only design does not by itself prevent Linux core dump or swap
  persistence. Narrivy can claim no intentional payload files/logs now; worker
  dumpability, core limits, swap policy, and crash-path qualification remain
  separate security work.

## 2026-08-28 — Smallest exact-0.2.2 native streaming canary

- Primary sources: exact transcribe.cpp 0.2.2 cargo source metadata/tests/native
  Moonshine Streaming implementation; exact-commit upstream model docs; the
  official Useful Sensors checkpoint; and the official handy-computer Hugging
  Face GGUF API/LFS metadata.
- Moonshine Streaming Tiny Q8_0 is 50,462,816 bytes, MIT, English-only, and
  explicitly reports native streaming/cancellation at 16 kHz. Recommended feed
  cadence is 80 ms (1,280 samples), cumulative right context is approximately
  240 ms, and the natural emit unit is 20 ms. It has no VAD/endpointer,
  timestamps, translation, or language detection.
- Artifact revision `85ddff612fa3a2cf40b2f745abcfa90ef82f293b`, SHA-256
  `930e4622ad3a24158b91406c30c977fa6a26b34cb32d6ac3e57cfb23383a869e`;
  original checkpoint revision
  `f8e9dfd8c562c257c151a907b7b7f2fe8ff8511a`.
- Native current-host observation: capability true; begin/feed/finalize worked;
  11-second public fixture produced 31 changed snapshots at 80-ms chunking.
  First change required 720 ms of supplied audio. Accelerated-feed compute and
  per-call timings are not real-time microphone latency or PERF-005 evidence.
- Parakeet Unified and Nemotron streaming remain future comparators: their Q8
  artifacts are roughly 700 MB and their useful latency settings require
  family-specific options not yet represented by Narrivy's generic worker.

## 2026-08-28 — Recovery live-capture transport boundary

- The existing `PreRollRingBuffer` intentionally drops oldest samples at
  capacity. It is correct pre-roll behavior but cannot establish continuous
  ASR input or backpressure.
- A fixed eight-slot queue of 1,280 f32 samples bounds queued audio to 640 ms
  and payload bytes to 40 KiB at 16 kHz mono. This is a capacity fact, not a
  jitter or latency qualification.
- Because native ASR feeds are synchronous, capture must observe transaction
  controls independently of consumer polling. The Linux parser can safely
  signal its still-unreaped child PID: the owning parent retains the Child, so
  Linux cannot reuse that PID until reap. The parent alone performs wait/reap.
- Controlled tests established ordered chunking, exact safety completion,
  queued-audio finalization, cancellation at prefetch/receive/producer
  boundaries, terminal overflow, invalid and early output, explicit abort,
  and both-owner cleanup under one deadline. No microphone/native latency
  inference follows from these tests.

## 2026-08-28 — Capture-to-ASR composition and latency boundary

- The resident stream API is synchronous per feed. Therefore the capture
  producer must remain independently controlled and bounded; moving capture
  reads onto the orchestration thread would reintroduce cancellation delay and
  device-buffer dependence.
- The prior native 4F observation used 138 accelerated 80-ms feeds over an
  11-second public fixture. Feed-call compute was variable (15.54-ms p50,
  165.42-ms p95, 279.04-ms max) and the first changed snapshot followed 720 ms
  of supplied audio. The 4G eight-slot queue provides 640 ms of bounded slack,
  but those single-host observations do not prove sustained real-time behavior.
- Core must validate both contracts again at the composition boundary: capture
  format/sequence/summary agreement and strict ASR Started/Partial/Final order.
  Adapter-local validation alone cannot protect future implementations of the
  portable traits.
- Opening ASR before capture establishes model admission before microphone
  readiness, avoiding a post-capture model-load gap. Raw-audio policy is
  checked before either owner opens. Only a validated final snapshot enters the
  existing transcript transaction; partial text remains volatile.
- Status now distinguishes model support from product composition and native
  qualification. `composed:controlled_unqualified` is intentionally weaker
  than available/ready/verified until an authorized native window measures the
  full lane.

## 2026-08-28 — Conservative VAD/endpointer references

- Question: What minimal endpoint semantics should replace the fixed safety
  window without adding an unreviewed model/runtime or clipping audio?
- Primary sources: official whisper.cpp VAD options and documentation
  (`https://github.com/ggml-org/whisper.cpp`); its maintained VAD speech-segment
  example (`https://github.com/ggml-org/whisper.cpp/blob/master/examples/vad-speech-segments/speech.cpp`);
  and the official Silero VAD C++ example/state machine
  (`https://github.com/snakers4/silero-vad/blob/master/examples/c%2B%2B/silero.cc`).
- Conclusion: established implementations separate speech probability/activity
  from endpoint timing, use minimum-speech and minimum-silence/padding controls,
  and commonly apply threshold state rather than ending on one quiet chunk.
  Their exact thresholds are model/acoustic-environment choices and cannot be
  copied as Narrivy native qualification.
- Decision impact: Recovery 4I adopts only the architecture pattern: a
  replaceable VAD, a separate sample-count endpointer, hysteresis, minimum
  contiguous speech, and trailing-silence hangover. It adds no neural
  dependency, never trims accepted audio, and labels the result
  `controlled_unqualified`. Native calibration remains required.

## 2026-08-30 — Monotonic timing and the controlled streaming boundary

- Exact vendored transcribe.cpp documentation confirms Moonshine Streaming's
  model-native stream API, approximately 240-ms cumulative right context,
  80-ms feed cadence, and 20-ms natural emit unit. These are upstream/model
  design facts, not Narrivy end-to-end measurements.
- The existing P6 timer uses Unix-epoch timestamps, arbitrary string maps,
  completed-record retention, and default JSON persistence. Its historical
  runner accepts mock/aliased inputs and cannot establish a production latency
  requirement. Recovery timing must not reuse it.
- The ignored upstream JFK WAV is 352,078 bytes, PCM16 mono 16 kHz, SHA-256
  `59dfb9a4acb36fe2a2affc14bacbee2920ff435cb13cc314a08c13f66ba7860e`.
  It is shipped and used by the exact upstream source tree, but no separate
  audio-specific provenance/license record was located. It is therefore usable
  only as an explicitly labeled non-qualifying local diagnostic.
- Paced real-resident runs observed all ten recovery stages and durable Inbox
  completion. The final run reached origin-to-first-partial at about 773 ms and
  consumed the roughly 11-second fixture in about 11.9 seconds; an earlier run
  under concurrent build/test activity took about 25.2 seconds. This host-load
  variability, unannotated speech onset, and absent PipeWire/UI make the values
  diagnostic only—not PERF-005 or native latency evidence.
- Decision impact: use an optional `Instant`-based fixed trace only in
  controlled diagnostics; remove default timing persistence; require an
  annotated provenance-reviewed corpus plus physical native loopback/microphone
  protocol before any speech-to-partial percentile claim.

## 2026-08-31 — Strict Linux helper trust and lifecycle boundary

- Primary sources: Rust `CommandExt::process_group`; Linux `killpg(2)`,
  `PR_SET_PDEATHSIG`, `close_range(2)`, and kernel pathname-lookup
  documentation. These establish the process primitives, not Narrivy release
  qualification.
- Current development-host observation: `/usr/bin/pw-record` is a root-owned
  symlink to root-owned mode-0755 `/usr/bin/pw-cat`; `/usr/bin/wtype` is a
  root-owned mode-0755 regular executable. Their installed packages report
  `pipewire-bin 1.5.85-1ubuntu1pop1~1769189762~24.04~601113b` and `wtype 0.4-3`.
  `pw-record --version` reports compiled/linked PipeWire 1.5.85. The kernel is
  `7.1.5-76070105-generic`, and the no-op close-range CLOEXEC support preflight
  plus process-adapter tests pass.
- Security conclusion: a reviewed absolute alias is needed because executing
  canonical `pw-cat` may change argv0-selected behavior. Root-owned,
  non-writable path revalidation blocks same-user PATH substitution but does
  not prove package/SBOM provenance or prevent a root-authorized in-place race.
  Process groups contain ordinary descendants, not an adversarial `setsid`
  escape; cgroup/package qualification remains separate.
- Environment conclusion: PipeWire admits only `XDG_RUNTIME_DIR`,
  `PIPEWIRE_RUNTIME_DIR`, and `PIPEWIRE_REMOTE`, with
  `PIPEWIRE_NO_CONFIG=true`; wtype admits only `XDG_RUNTIME_DIR` and
  `WAYLAND_DISPLAY`. Loader, arbitrary PATH, locale, HOME, and config/plugin
  override variables are absent.
- Qualification boundary: all host observations are content-free and
  non-qualifying. No microphone, transcript, target insertion, package, or
  release behavior was exercised.

## 2026-09-01 — Controlled resident executable mechanics

- Linux executable memfds can be sealed against byte/size/execute changes, but
  execute sealing alone does not prevent every permission-bit transition.
  Recovery 4Q-A therefore enforces exact snapshot mode 0500 at revalidation and
  sets no-new-privileges in the artifact child.
- A short ELF magic/machine prefix is insufficient trust validation. The
  controlled boundary validates the complete host-class ELF header, program
  table bounds/count and at least one valid executable load segment before
  sealing.
- `File::try_clone` shares the open-file-description offset. Concurrent
  snapshot verification therefore uses positional `read_at`, not seek/read.
- Static pathname readiness is not live engine truth. One daemon-retained
  content-free snapshot must drive status, listing and admission; otherwise an
  API can report enabled while the actual resident is Active, Warming,
  restart-required or failed closed.
- Controlled digest binding proves exact executable bytes selected for this
  process only. Model identity, installed package membership, SBOM provenance,
  release-artifact identity and cgroup containment remain unproven.

## 2026-09-01 — Resident model descriptor mechanics

- The upstream `transcribe-cpp` 0.2.2 safe wrapper ultimately accepts only a
  filesystem path. On Linux, `/proc/self/fd/<n>` is a compatible path to an
  inherited descriptor; the controlled real-model diagnostic loaded the
  Moonshine streaming canary through this surface and restarted successfully
  after the isolated source copy was deleted.
- An ordinary retained file descriptor blocks rename substitution but not
  same-user in-place writes. A sealed memfd supplies the missing byte/size
  immutability. Exact mode 0400 plus WRITE/GROW/SHRINK/FUTURE_WRITE/EXEC/SEAL
  seals prevents payload or permission promotion after binding.
- The model-manager registry carries digest and size metadata, but current
  `InstallState::Installed` is derived from path existence. It is not a live
  descriptor, package identity or sufficient runtime attestation. This phase
  therefore consumes explicit controlled digest/size and does not promote the
  manager's existence state.
- Copy/hash occurs once at daemon startup; worker attestation rehashes the same
  retained snapshot only on worker generation startup. Ordinary sessions reuse
  the resident native model and incur neither operation.
- The successful diagnostic was content-free: model load, isolated source-copy
  deletion, worker restart from the retained snapshot and bounded shutdown. It
  exercised no microphone, audio decode, transcript, delivery, evidence or
  release path.

## 2026-09-01 — Installed model and package-composition audit

- The normative `model-manifest.schema.json` is not consumed by the live P5
  model manager. Its version-1 registry is narrower and previously derived
  `Installed` from `path.is_file()`, without proving size, sidecar identity, or
  digest at the point of observation.
- Acquisition previously omitted declared-size enforcement, accepted unsafe
  registry-controlled path components, and published the model leaf before its
  metadata leaf. Recovery 4Q-C1 closes the first two classes and makes the
  incomplete publication state explicitly unverified; a complete atomic
  directory transaction remains deferred.
- Read-only inspection found no resident worker or GGUF model in any existing
  Debian artifact. The current service unit supplies no resident worker/model
  binding, and the package script can stage skeleton/debug fallbacks and
  self-authored metadata. None is acceptable package or release provenance.
- A repository-root manifest cannot prove that an installed package contains
  the same runtime/model bytes. Installed runtime manifests, package membership,
  SBOM/provenance authority, and clean-machine qualification remain separate
  fail-closed work.

## 2026-09-01 — Fixed-root runtime manifest mechanics

- A minimal root-controlled manifest can safely select digest and size for one
  fixed worker path without creating a general executable-selection surface.
- Manifest and worker opens each use a freshly validated fixed-root descriptor
  with `openat2` beneath/no-symlink/no-magiclink constraints. They are separate
  local observations; root and kernel compromise remain outside this boundary.
- Generic trust-anchor handles must not carry authority-specific names. The
  daemon can attribute installed-manifest and registry-observed state only
  while retaining the opaque adapter newtypes that established those facts.
- Successful local worker/model composition still says nothing about dpkg file
  membership, the shipped Debian artifact, SBOM membership, signatures, build
  identity, or external release provenance. Those require independent later
  authorities and clean-machine qualification.

## 2026-09-01 — Isolated package-root staging audit

- The protected Debian artifact contains neither the resident worker nor the
  installed-runtime manifest. Its Debian and embedded build versions differ,
  and its builder identity is a stub rather than external provenance.
- The historical builder tolerates missing release binaries, can substitute
  debug binaries, emits stub supply-chain material, overwrites the protected
  artifact, and appends the protected build log. It is not a safe foundation
  for incremental modification.
- Descriptor-anchored package-root staging can prove exact fixed-path bytes,
  modes, schema, digest, and durability without constructing a package. It
  cannot prove archive membership, installation, ownership, or provenance.
- Cleanup needs its own authority model: precheck does not confer ownership;
  creation must be recorded, raced names must be identity-checked, and cleanup
  must never recursively traverse a pathname that an attacker can redirect.
- A restrictive process `umask` requires acquiring the newly created root via
  `O_PATH`, applying the exact private mode through that stable handle, then
  reopening and verifying identity before writing.

## 2026-09-01 — Disposable Debian composition audit

- `dpkg-deb --fsys-tarfile` and `--ctrl-tarfile` permit independent stream
  inspection of exact payload/control membership, modes, ownership, and bytes
  before a temporary archive is destroyed.
- The packaged daemon must be probed, not inferred from Cargo metadata: an
  empty-cache installed-mode launch distinguishes a resident-feature build
  from a binary that merely exists and is executable.
- The current model registry has one exact-digest non-unqualified entry,
  `whisper-tiny.en-Q8_0`, but marks it non-streaming. It is suitable only for a
  deterministic missing-model negative probe, not a final model default.
- Root package maintainer scripts cannot safely manage an arbitrary user's
  `systemctl --user` instance. Service enable/start, upgrades, and removal need
  a separate user-session lifecycle design and clean-machine qualification.
- Deterministic archive bytes and root ownership inside a tar stream still do
  not prove dpkg installation, external provenance, SBOM identity, or release
  behavior.

## 2026-09-02 — Offline model import and license boundary audit

- The live public model-manager API previously allowed acquisition from any
  validated `RegistryEntry`; using that shape for local provisioning would let
  a caller select its own digest. A distinct, non-constructible bundled-
  registry authority is required before an import may yield an
  `InstalledModelBinding`.
- Atomic leaf renames cannot make `model.gguf` plus `meta.json` visible as one
  unit. Linux descriptor-relative `RENAME_NOREPLACE` permits publication of the
  completed directory without overwriting another writer or an incomplete
  destination.
- Cleanup based on `remove_dir_all` after a pathname identity check remains
  raceable. Retaining the staging descriptor and unlinking only fixed known
  leaves prevents a renamed/replaced path from redirecting recursive deletion.
- The independently reviewed OpenAI Whisper upstream and conversion model cards
  identify the ordinary Whisper weights as Apache-2.0, conflicting with the
  seeded registry and ordinary-Whisper catalog declarations. Distil-Whisper is
  a separate family/license decision and is unchanged.
- Registry digest matching proves local byte identity, not lawful distribution,
  external provenance, a complete notice set, streaming behavior, or package
  membership. Those remain explicit pre-install/release blockers.
- Independent review rejected placing owner/mode/nofollow/rename policy in the
  portable model manager. The corrected split keeps only bundled authority and
  exact data verification portable; descriptor-relative root/source walks,
  owned-directory creation, sync ordering, publication, and cleanup are Linux
  adapter responsibilities.
- A generic `InstalledModelBinding` can legitimately represent a caller-parsed
  compatibility registry observation, so it cannot authorize installed daemon
  composition. A separate opaque `BundledInstalledModelBinding` now gates that
  Linux runtime seam without upgrading package or release provenance.

## 2026-09-03 — Installed startup degradation boundary

- The candidate systemd unit treats daemon exits 1 and 2 as non-restartable.
  Therefore exiting for an expected absent model makes status and any future
  daemon-owned provisioning surface unreachable; repeatedly restarting would
  be worse and is not proposed.
- The installed runtime and installed model have different authority and
  lifecycle meanings. A missing model can be provisionable only after the
  fixed-root runtime has already verified. Missing/invalid runtime authority,
  unknown bundled model identity, and invalid configuration cannot enter the
  degraded state.
- Retaining the verified runtime descriptor lets daemon status keep one stored
  runtime truth source while the model is unavailable. It does not start the
  resident worker and does not establish package or release provenance.
- Existing strict pipeline readiness already feeds both listing and admission.
  One resident lifecycle lock now owns awaiting/ready/closing state, and the
  additive typed status field is derived from the same snapshot that feeds
  capability strings and pipeline readiness. This keeps `GetStatus`,
  `ListPipelines`, `StartSession`, and `SelectPipeline` consistent without a
  parallel readiness flag.
- Treating every model-manager `Io`/`Json` error as a recoverable invalid model
  would hide ambiguous cache/filesystem failures. C5B1 therefore degrades only
  `InstalledModelMissing`; all other verification and immutable-binding errors
  are fatal and content-free at the daemon boundary.
- A real degraded systemd lifecycle requires installing the fixed-root worker
  and manifest. That is package mutation and was intentionally not performed in
  C5B1; socket cleanup remains covered only by the existing IPC endpoint lease
  contract until a disposable install phase is authorized.

## 2026-09-03 — Cooperative cancellation around atomic model publication

- A synchronous import cannot honestly promise a hard wall-clock timeout for
  filesystem syscalls that may block in the kernel. Fixed-size user-space work
  plus checks around each phase is a cooperative deadline contract; the future
  daemon owner still needs a separately bounded worker/shutdown policy.
- Cancellation observed before directory publication is not a clean outcome
  unless owned stage removal and staging-parent sync are confirmed. Returning
  cancelled while cleanup durability is unknown would hide residual sensitive
  model bytes.
- `RENAME_NOREPLACE` creates a crisp commit boundary. Once it succeeds,
  cancellation cannot retroactively change the result or authorize destination
  removal. Parent-sync and exact-verification failures must say that publication
  occurred but confirmation failed.
- Rehashing the staged leaf after its sync catches corruption at the actual
  publication input, rather than relying only on the digest accumulated while
  copying from the source.
- Deterministic checkpoint barriers can prove cancellation placement without
  sleeps. They remain controlled filesystem tests, not real-model throughput,
  storage-failure, installed-package, or release evidence.

## 2026-09-03 — Activation-chain cancellation audit

- A cancellable import is insufficient when immutable binding or native worker
  startup performs another large uncontrolled digest pass. The production
  launch path, rather than a test-only executable branch, must carry the same
  control through its final process-creation boundary.
- Sealed memfd construction already hashes the copied bytes. Rehashing again in
  the same construction call adds latency without increasing assurance once
  exact digest, identity, mode, and seals are established. A later launch still
  needs one complete controlled revalidation at the last safe boundary.
- Reserving cleanup time inside an absolute startup deadline avoids the false
  choice between extending the advertised budget and abandoning child
  ownership after handshake expiry.
- Holding a global ownership mutex while killing or reaping lets one caller's
  cleanup budget consume a different caller's earlier deadline. A short state
  transfer plus an explicit cleanup-in-progress marker preserves serialization
  while moving OS waits outside the critical section.
- Deterministic production digest-bound and overlapping-constructor tests are
  necessary because controlled-path test executables bypass immutable artifact
  revalidation and cannot establish the production claim.

## 2026-09-03 — Provisioning IPC authority and compatibility

- A client-provided registry entry, digest, cache path, or model ID would let a
  same-user caller manufacture the authority that the bundled registry is meant
  to provide. The safe command is therefore source-only; the daemon resolves
  every authoritative field.
- Long local import and native activation outlive ordinary IPC round trips.
  Disconnect cannot be cancellation authority; one daemon-owned operation ID,
  explicit cancel, and status polling make ownership visible without tying it
  to a socket lifetime.
- Publication and cleanup certainty are independent of terminal success. A
  single success/failure boolean would erase the difference between no publish,
  verified installed bytes, and committed-but-unconfirmed state.
- Adding a protocol event before notification ordering and compatibility are
  reviewed would widen the race surface. Minor 5 intentionally uses polling and
  preserves a later additive event option.

## 2026-09-04 — Daemon provisioning lifecycle and post-publication control

- Provisioning status, cancellation, resident ownership, and shutdown must be
  one mutex-owned authority. Separate locks can make status say awaiting while
  an old generation publishes Ready.
- A FIFO opened read-only can block before type validation. `O_NONBLOCK` is
  harmless for regular files and lets the adapter reject device-like sources
  before the worker loses cancellation responsiveness.
- Publication changes the meaning of cancellation, not the need for a bound.
  After `RENAME_NOREPLACE`, interruption cannot authorize deletion or a clean
  cancelled result; it means installed bytes exist but confirmation is unknown.
- Rehashing a large model once in the Linux transaction and again in the model
  manager doubles activation latency without adding authority. The production
  path now delegates the one post-publication controlled hash to the bundled
  installed-observation boundary.
- A cancel request may linearize immediately before an ordinary worker error.
  Normalizing a clean pre-publication result to Cancelled preserves the returned
  runtime and prevents a permanent Provisioning state; post-publication cancel
  remains fail-closed with verified-installed publication truth.

## 2026-09-04 — Desktop advisory graph split and safe transitive repair

- Question: Which of the 20 current cargo-deny findings can be removed without
  changing the desktop runtime or weakening the portable Rust 1.85 contract?
- Sources: fresh crates.io metadata from `cargo info`, the locked inverse Cargo
  graph, current RustSec data through cargo-deny 0.20.2, and the explicit
  portable package list in `.github/workflows/ci.yml`.
- Finding: `tauri-utils 2.9.3` accepts any `plist 1.x`. `plist 1.10.0` requires
  `quick-xml 0.41.0` and `time >=0.3.47`, eliminating RUSTSEC-2026-0194,
  RUSTSEC-2026-0195, and RUSTSEC-2026-0009. These desktop-only transitive
  packages require Rust 1.88, while Narrivy's project toolchain is 1.97.1 and
  its 1.85 promise is explicitly limited to nine portable packages.
- Verification: the locked all-target/all-feature workspace check passes on
  1.97.1 and the exact portable package set still passes on 1.85.0.
- Remaining boundary: Tauri 2.11.5 is current and pins `urlpattern 0.3`, whose
  old `unic-*` dependency family is unmaintained. Tauri/Wry and the current
  native parent adapter also retain GTK3 Rust bindings and the affected
  `glib 0.18.5`. Those 17 findings require the maintained desktop-runtime ADR;
  they were not ignored, patched locally, or mislabeled as resolved.

## 2026-09-04 — Linux IPC command-authorization boundary audit

- Question: Can narrivyd authorize privileged commands by desktop/CLI role or
  peer PID after Unix `SO_PEERCRED` authentication?
- Finding: no. The kernel credential establishes the effective UID; role and
  build identity are self-declared, while PID is diagnostic and changes on
  restart. Executable-path inspection is race-prone and does not produce a
  separately protected principal. Treating any of these as identity would
  create a false security claim.
- Repository audit: all 17 current commands now have an exhaustive daemon-side
  class and exact-command budget. Existing downstream state already owns the
  meaningful intent/lifecycle boundaries: active sessions, exact recovery
  transactions, attended portal approval, exact portal/provision operations,
  and awaiting/single-flight/cooldown/commit-aware provisioning.
- Verification: real Unix IPC proves that reconnecting under `desktop-test`,
  `cli-test`, then `admin` shares one budget and returns typed `busy`; a full
  daemon restart resets the in-memory window. The test invokes an uncomposed
  operation, so it imports no model and mutates no user data.
- Residual: a hostile same-UID process can consume the shared budget and can
  generally interfere with the user's daemon. Stronger isolation needs an
  independently protected broker or capability-transfer design and a separate
  threat review; additional pathname/role/PID checks are not a substitute.

## 2026-09-04 — Maintained GTK4/WebKitGTK 6 desktop-shell feasibility

- Question: Can the GTK3/Tauri advisory family be removed while retaining the
  existing React frontend instead of immediately rewriting the product UI?
- Current crates: crates.io reports `webkit6 0.6.1`, `gtk4/gdk4 0.11.4`, and
  `glib 0.22.9`, all MIT. The GTK4 generation requires Rust 1.92, which is
  compatible with Narrivy's pinned 1.97.1 project toolchain but not the
  separately tested portable-package 1.85 floor; the desktop must declare its
  own honest MSRV if this lane is selected.
- Capability finding: the maintained WebKit6 bindings expose a custom URI
  scheme, display/security scheme classification, script-message handlers with
  replies, JavaScript evaluation, and `load_uri`/`load_html`. Those are the
  primitives needed to retain the built React assets and replace Tauri invoke/
  event transport with a narrow reviewed bridge.
- Host/package finding: the host has the GTK4 runtime library but lacks the
  `gtk4` and `webkitgtk-6.0` pkg-config development metadata. The configured
  Pop!_OS/Ubuntu 24.04 repositories offer `libgtk-4-dev 4.14.5` and
  `libwebkitgtk-6.0-dev 2.52.6`; the base Noble lane lists 4.14.2/2.44.0.
  No package was installed, so a native prototype/build was not attempted.
- Remaining design risk: GTK4 does not expose the GTK3
  `gdk_wayland_window_export_handle` API used by ADR-0028. A replacement must
  own xdg-foreign parent export with explicit async cancellation/unexport and
  portal-operation ordering; generic raw-handle conversion cannot be adopted
  without re-auditing its thread, roundtrip and lifetime behaviour.
- Conclusion: GTK4/WebKitGTK 6 is the leading preservation-of-React candidate,
  not an accepted runtime yet. B-DESKTOP-008 stays open until development
  dependencies are available, the bridge/parent lifecycle is implemented and
  native-tested, the old Tauri/GTK3 graph is removed, and cargo-deny passes
  without exceptions.

## 2026-09-04 — Framework-neutral desktop bridge extraction verification

- The current desktop command surface can be split without protocol drift:
  all Tauri command attributes and macro-facing wrappers now reside in
  `tauri_commands.rs`; the reusable bridge, theme store, and event forwarder
  contain no Tauri API token.
- The split preserves the current command names and argument names registered
  by `generate_handler!`, including exact recovery lookup and the independent
  optional-History surface. The React bridge therefore required no transport
  contract change.
- The previously detached event thread now has one retained owner, explicit
  shutdown signal, bounded stop polling, and a join whose panic result reaches
  the shell's `run()` result.
- Focused verification passed: 35 Rust tests, all-feature warning-denying
  Clippy, 28 frontend tests, and the TypeScript/Vite production build.
- Sequential repository verification also passed: all-target/all-feature
  workspace check, tests and warning-denying Clippy; formatting and diff
  hygiene; traceability and both boundary validators; 29 governance tests and
  27 packaging tests.
- Dependency policy remains truthful: bans, licenses and sources pass;
  advisories fail on the same 17 GTK3/Tauri findings. No dependency was added,
  package installed, advisory ignored, evidence created, manifest resealed, or
  release claim changed.

## 2026-09-04 — Legacy package/evidence reachability audit

- `scripts/build_deb.sh` remained executable despite B-PACKAGE-015. Before
  quarantine it could tolerate missing release binaries, substitute debug
  binaries, generate stub builder identity, overwrite the retained `.deb`, and
  append the protected build log.
- P10 differed from the already quarantined P9/P11 lanes: its 925-line runner
  could choose the first stale package in `artifacts/releases`, write
  schema-v1 evidence, and generate release metadata with placeholder builder
  fields. `refresh_p10_evidence.py` ignored builder return status before
  rewriting records.
- ADR-0062 makes all three entry points fail before mutation. The focused
  governance checks invoke and import them, verify status 2 and fixed
  content-free output, and prove protected-file identities do not change.
- The disposable C3/C4 composition checks remain callable. They are explicitly
  install-blocked and do not become package, provenance, or release evidence.

## 2026-09-04 — Systemd descendant-containment configuration audit

- A helper that calls `setsid` escapes a daemon-owned process group but stays
  in the systemd service cgroup. The package's systemd user service is therefore
  the appropriate outer cleanup authority for the primary Linux 1.0 lifecycle.
- The prior recovery unit relied on implicit defaults and the historical
  package copies had drifted. ADR-0063 requires explicit `ExitType=cgroup`,
  `KillMode=control-group`, `TimeoutStopSec=3s`,
  `TimeoutStopFailureMode=kill`, and `SendSIGKILL=yes` in one byte-identical
  template.
- The current host's systemd 255 accepts the unit through
  `systemd-analyze verify --man=no`. Exact-template and negative mutation tests
  pass in the 27-test packaging suite; all 30 governance tests also pass.
- This did not start/install a unit or kill a native process. Actual cgroup
  membership and zero-descendant behavior after graceful stop, daemon crash,
  timeout, logout and restart remain package-artifact qualification work.

## 2026-09-04 — GTK4/WebKitGTK 6 prototype and content-free native smoke

- Installed host development metadata resolves GTK 4.14.5, WebKitGTK 6.0
  2.52.6 and JavaScriptCoreGTK 6.0 2.52.6. The exact Rust graph resolves
  `gtk4/gdk4 0.11.4`, `webkit6 0.6.1`, and `glib 0.22.9`; the new crate states
  Rust 1.92 while the workspace toolchain remains 1.97.1.
- A custom local/secure/display-isolated `narrivy://app/` scheme successfully
  serves embedded HTML/CSS. This WebKit did not run the external Vite module
  script from that scheme; injecting the exact embedded bundle as a document-
  end `UserScript` and removing its HTML script tag mounted the React root.
- The shell uses a reply-capable `narrivy` script-message handler. A bounded
  COSMIC/Wayland smoke completed one content-free `theme_get` round trip, found
  a complete document with a mounted root, reported no JavaScript errors, and
  observed only `index.html` plus the internal CSS request. No external request
  was observed.
- Defense in depth includes `default-src 'none'` / `connect-src 'none'`, an
  ephemeral WebKit network session with an unusable loopback proxy, ignored
  non-Narrivy navigation, and disabled browser media/capture/WebRTC/WebAudio/
  WebGL/JavaScript clipboard/window/database surfaces. These controls do not
  replace daemon IPC authentication or privacy policy.
- Focused checks passed: GTK crate check, 16 tests and warning-denying Clippy;
  retained Tauri all-feature 35 tests and warning-denying Clippy; frontend 30
  tests across 18 files and the production Vite build. Cargo-deny bans,
  licenses and sources pass. Advisories still fail on the same 17 retained
  Tauri/GTK3 findings because the parity shell and parent adapter remain.
- The smoke hook stores only sanitized backend/bridge/document/resource facts
  and exits after a short GTK drain. It processed no speech or user content and
  is diagnostic proof only. Overlay behavior and GTK4 xdg-foreign export are
  still unimplemented; no support or release qualification is inferred.

## 2026-09-05 — Owned GTK sources and shutdown

Checked installed glib 0.22.9 source and upstream main-loop documentation
(https://docs.gtk.org/glib/type_func.Source.remove.html and
https://gtk-rs.org/gtk-rs-core/stable/latest/docs/glib/source/index.html).
Removing an already-fired source is invalid. Overlay timeout clears its stored
ID before callback execution; replacement/drop removes only the live source.
Desktop worker ownership now includes its UI source; teardown drops the
receiver/overlay before joining workers. Main-window close requests app quit.
Both workers are joined even if the first panics, and the command worker has
an owner before the fallible event-worker spawn.

25 GTK tests, Clippy with warnings denied, six GTK governance tests and a native
content-free COSMIC smoke pass. The smoke mounted React, completed theme_get,
observed zero external requests and no JS errors, and exited 0 with empty stderr.
No portal grant, microphone, package or release qualification occurred.

## 2026-09-05 — Correct native display attribution and parent-export observation

Discovered inherited GDK_BACKEND=x11. Correct the preceding COSMIC shell smoke
attribution to XWayland; the desktop host session alone is insufficient to
identify the window backend. The parent adapter diagnostic truthfully returned
NotWayland on that inherited backend. With GDK_BACKEND=wayland it returned
exported=true/released=true, empty stderr and exit 0. No handle was logged.

Added gdk_display_type to the desktop readiness diagnostic. A fresh explicitly
Wayland run reports GdkWaylandDisplay, mounted React, theme_get reply ok, no JS
errors, zero external requests, empty stderr and exit 0. Temporary diagnostic
files were removed. The actual baseline adapter is already GTK4 despite stale
recovery descriptions; ADR-0064 is amended to reflect inspected source.
The four adapter tests pass. Portal executor/close reconciliation and production
delivery are still incomplete; these results are not registered release evidence.

## 2026-09-05 — Portal transport deadline prerequisite

ADR-0065 implements an absolute-deadline Unix Read/Write wrapper for handshake
and request frame exchange, recomputing remaining budget for every short-I/O
retry. Expired writes produce typed Timeout and send no bytes. Request errors
before a full correlated reply discard transport/buffers without replay.
Initial caller testing exposed that explicit daemon rejections must preserve
the connection; corrected the implementation to distinguish complete replies.

30 IPC tests, 97 affected daemon extension/recovery integration tests, warning-
denying Clippy and source/dependency boundaries pass. Regression tests exercise
slow frame delivery, expired writes and request invalidation/no replay. The
first broad caller run exposed two rejection-reuse failures, now both pass in
the 97-test focused rerun. Connect, event reads and active cancellation still
require work before asserting a bounded portal executor. No evidence promoted.

## 2026-09-05 — Bounded connect and event framing

Verified socket2 0.6.5 source and upstream API:
https://docs.rs/socket2/latest/socket2/struct.Socket.html#method.connect_timeout
The wrapper uses nonblocking connect and finite poll; failed sockets must not
be reused. Added the already-locked version as a direct Unix-adapter dependency,
with safe OwnedFd conversion. No new package version was resolved.

connect_as_until shares a deadline with authenticated handshake. Existing
connect/reconnect retain the five-second default as a total budget. Event reads
now share an absolute frame deadline and preserve only zero-byte idle timeouts;
partial frame failures discard the connection. 33 transport tests and Clippy pass.
This completes connection/event deadline prerequisites, not portal worker
composition, active request reconciliation, delivery or release qualification.

## 2026-09-05 — Portal transport and worker composition prerequisite

Added portal_transport in GTK and reused framework-neutral portal_parent.
Status classification requires consistent idle/closed or active/connected facts;
unknown, closing, pending and transport failures remain cleanup-unconfirmed.
Cancel uses at most 500 ms of the caller deadline and revoke is attempted even
after cancel failure on a fresh authenticated connection. Owned workers return
content-free classifications, are polled only when finished and remain joined
on drop. They are not wired to startup or UI, and no permission was requested.
Focused GTK tests, Clippy and six migration governance tests pass. Full executor
needs reviewed deadline caps, close ownership and daemon-terminal reconciliation;
a timed-out client exchange is not proof the daemon stopped using its parent.

## 2026-09-11 — GTK/daemon portal permission source composition

Connected the existing components without moving policy into the UI. GTK owns
the main-thread xdg-foreign begin/release calls and a finite worker per IPC
operation. The pure coordinator remains the only ordering authority for
permission completion, cancel followed by unconditional revoke, local release,
and close/exit replay. The frontend sees only a five-value sanitized outcome.

The initial UI placement in onboarding contradicted ADR-0024 and was removed.
The explicit action now lives under Text Delivery settings and is enabled only
from typed daemon status. This preserves the no-silent-prompt rule.

Daemon startup now maps the bounded read-only probe into typed eligibility,
constructs the direct zbus driver and portal service, uses the injected handler,
and owns event-pump and shutdown joins. A real daemon/CLI status observation on
this host returned unavailable facts because the direct probe reports
`KeyboardUnavailable { version: 2 }`. No attended portal call was made.

Frontend 31 tests/build, GTK 50 tests, the affected daemon/CLI Rust suite,
warning-denying Clippy, 24 governance tests, boundary/traceability validators,
and a fresh native Wayland shell smoke pass. The first parallel validation raced
Vite's hashed asset replacement against Cargo embedding; the required sequential
frontend-then-GTK rerun passed. No qualification evidence was promoted.

## 2026-09-11 — Retained Debian candidate and package-runtime smoke

The current host provides `libgtk-4-1` 4.14.5,
`libwebkitgtk-6.0-4` 2.52.6, PipeWire 1.6.8, systemd 255, and xdg-utils.
The maintained desktop ELF resolves GTK4 and WebKitGTK 6 and has no Tauri or
AppIndicator package requirement. Debian control metadata was corrected to
those runtime packages.

The first retained composition exposed data-archive root mode `0700`. That
archive was removed before testing. The builder now explicitly changes and
inspects the data root as `0755`, preventing dpkg from narrowing installed `/`
permissions. Publication uses create-exclusive output and a regression test
proves a pre-existing destination is neither overwritten nor deleted.

The exact bundled model is 45,904,544 bytes with SHA-256
`e8c9b73c06344307d8b346e07fbe93dd88d894627854bcff31523f1ce44394fa`.
The final candidate is 40,260,736 bytes with SHA-256
`7bfac882c901d4c2a690ceb963fe6b4b2fe9010b5169aa4dba8add93433258bc`;
a second build was byte-identical and the duplicate was removed. Archive
membership/modes/digests, 18 packaging tests, frontend 32 tests/build, GTK 50
tests, and strict GTK Clippy pass.

Without installing anything, a temporary extraction ran the packaged daemon,
resident worker, model, and CLI together. Status reported the controlled
digest-bound worker/model and the daemon exited cleanly. Bubblewrap could not
construct the missing fixed `/usr/lib/narrivy` mount point beneath a read-only
host `/usr`; therefore this diagnostic does not claim installed fixed-root
behavior. Clean-machine installation and native acceptance remain required.
## 2026-09-17 — Activation-bound target identity and portal limits

- Codex research for the task009 review confirmed that XDG Desktop Portal
  RemoteDesktop provides permissioned keyboard input and a caller-supplied
  `parent_window`, but it does not identify the application that currently owns
  keyboard focus. Its documented `Start` response reports selected devices,
  optional streams, clipboard state and a restore token. Therefore a portal
  session is delivery authority, not target-identity evidence:
  <https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.RemoteDesktop.html>.
- The standard `ext_foreign_toplevel_list_v1` protocol enumerates mapped
  toplevel handles but intentionally leaves state, including activation, to
  extension protocols and may be restricted by compositor policy. It cannot by
  itself prove the focused target:
  <https://wayland.app/protocols/ext-foreign-toplevel-list-v1>.
- COSMIC publishes a compositor-specific toplevel-info protocol alongside the
  standard foreign-toplevel list. Upstream's protocol inventory includes
  `cosmic-toplevel-info-unstable-v1.xml`; active-window state has also had
  compositor race fixes/issues, so any use requires exact-version native
  qualification rather than an OS-name assumption:
  <https://github.com/pop-os/cosmic-protocols/blob/main/Makefile> and
  <https://github.com/pop-os/cosmic-comp/issues/892>.
- AT-SPI exposes `FOCUSED` and `ACTIVE` states and application ancestry, which
  can provide a capability-driven target observation for accessible
  applications. It is not universal target proof because applications may be
  absent from the accessibility tree:
  <https://gnome.pages.gitlab.gnome.org/at-spi2-core/libatspi/enum.StateType.html>
  and
  <https://gnome.pages.gitlab.gnome.org/at-spi2-core/libatspi/method.Accessible.get_application.html>.
- Read-only host inspection on the current native COSMIC session found
  `org.freedesktop.portal.RemoteDesktop` version 2 but no
  `org.freedesktop.portal.GlobalShortcuts` interface. `wayland-info` is not
  installed. This is content-free environment evidence only, not release
  qualification.
- Consequence for Narrivy: target identity must be captured and frozen at the
  activation/session-admission boundary. A late `xprop` lookup at final
  delivery is unsafe and XWayland-only. If no trustworthy activation-time
  provider exists, target-requiring rungs must be skipped and exact text must
  remain recoverable in Inbox. The viable completion path is an atomic toggle
  activation (including an OS-configured command fallback where the portal is
  absent) that snapshots capability-derived target evidence before StartSession;
  native COSMIC/GNOME qualification decides which provider/support claim is
  retained.

## 2026-09-20 — Grok 4.6 worker harness and evidence controls

- Grok Build CLI 1.0.34 is authenticated through the owner's Grok subscription;
  `grok models` exposes `grok-4.6` as default and `grok-4.5` as an alternative.
- The installed CLI supports `low`, `medium`, `high`, and `xhigh` reasoning.
  `xhigh` is the maximum. Its built-in writable sandbox is named `workspace`;
  `workspace-write` is not a built-in profile and correctly fails closed unless
  separately defined.
- The project is persisted as trusted, so root `AGENTS.md` is loaded. Unattended
  tasks use a fresh `--prompt-file` session, `bypassPermissions` inside the
  `workspace`/`read-only` sandbox, web disabled, and subagents disabled.
- A bounded implementation evaluation at `xhigh` completed in 644.978 seconds,
  18 turns, 1,220,322 total tokens (1,048,192 cached input; 133,062 uncached
  input; 39,068 output including 19,874 reasoning), and reported $0.34837352.
  It created a functioning runner, 13 focused tests, and documentation. Codex
  independently reran all 13 tests, Python compilation, and diff checks.
- A live call through the integrated runner completed in 4.129 seconds with the
  exact requested response, a mode-0600 receipt, hidden task-file argv, and the
  expected model/sandbox flags. This proves the local CLI boundary, not product
  correctness.
- Grok 4.6 was materially more grounded than the earlier malformed run, but the
  evaluation still showed high analysis cost and one imprecise final claim
  (“no network calls” despite the required model-service connection). Worker
  summaries remain non-authoritative; Codex must inspect the diff and rerun
  focused checks.
- xAI states that 4.6 received longer supplemental training, filtered SFT
  trajectories, agentic RL, and improved long-run self-testing relative to 4.5:
  <https://x.ai/news/grok-4-6>. A reproducible current Grok Build issue also
  documents a 4.6 case that narrated a question-card invocation without a real
  tool call, so tool evidence remains mandatory:
  <https://github.com/xai-org/plugin-marketplace/issues/562>.

## 2026-09-21 — OpenVINO 2026.4, transcribe.cpp 0.2.3, XeBoostLM and OpenArc

- Authoritative OpenVINO 2026.4 release notes: <https://github.com/openvinotoolkit/openvino/releases/tag/2026.4.0>. The release lists Qwen3-ASR on CPU/GPU and does not list it for NPU. Generic NPU runtime availability is not a model-support claim.
- `transcribe-cpp` 0.2.3 source commit is `63a44d9239d610b3908e8a66b384924cd4a77217`; exact Rust wrapper/sys crates are 0.2.3. Compared with 0.2.2, it fixes buffered Parakeet end-of-stream tail loss, frees scheduler/compute scratch memory after each run, and permits linking a verified prebuilt install through `TRANSCRIBE_DIR`: <https://github.com/handy-computer/transcribe.cpp/compare/v0.2.2...v0.2.3>.
- XeBoostLM is an Apache-2.0, Windows-first C++ OpenVINO GenAI LLM CLI/server with a very small project history. Its relevant ideas are explicit device routing, model catalog/cache separation, warm model residency and TTFT/throughput measurement. It provides no ASR implementation and is not adopted as a dependency: <https://github.com/balaragavan2007/XeBoostLM>.
- OpenArc is an Apache-2.0 Python/FastAPI OpenVINO inference server with ASR endpoints, multi-model concurrency, failure unload and request metrics. Its useful ideas map to Narrivy's existing runtime adapters, process-isolated workers, health records and compute planner. Its broad Python/server dependency and loopback HTTP surface are inappropriate for the lean release path. It may be evaluated later as an optional isolated provider, never as a core dependency: <https://github.com/SearchSavior/OpenArc>.
- Clean-room outcome: no XeBoostLM or OpenArc source, tests, identifiers or prose were copied or mechanically translated. Narrivy retains neutral requirements and implements only independently designed behavior behind its own contracts. If source is ever incorporated, Apache-2.0 attribution and notices remain mandatory even when upstream names are absent from product UI.
