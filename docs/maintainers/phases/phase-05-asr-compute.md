# P5 — ASR, Models & Adaptive Compute

## Outcome
Integrate real local ASR behind `narrivy-asr-api`, safe model acquisition/cache, **lean runtime-pack provisioning**, streaming where supported and proven CPU fallback. OpenVINO is a first-class Intel adapter — not a core dependency. Vulkan is an execution/runtime API inside GPU pools. Auto candidate hierarchy is **NPU → dGPU → iGPU → CPU**, but the planner chooses only qualified tuples and may select a lower tier when it is better for stability/quality/performance/policy. Explicit user selection overrides Auto.

## Primary requirements
`FUN-003`, `FUN-028`, `FUN-029`, `PERF-005`, `PERF-007`–`PERF-009`, `PORT-006`, `SEC-002`, `TEST-005`.

## Reference model / runtime policy (legacy planning Part C §5 + Supplemental §2–§9)
- Choose **one reference local ASR engine/runtime path for 1.0** rather than making multiple engines equally mandatory; additional engines are optional until the reference is stable.
- CPU is mandatory correctness fallback and must be sufficient to use Narrivy.
- OpenVINO/NPU or other accelerator support may ship in 1.0 **if** a specific tuple `hardware+driver+runtime+model+quant+OS+Narrivy version` passes qualification; accelerator support must **not** be a release prerequisite. Never advertise a device merely because APIs enumerate it.
- Compute hierarchy: **NPU pool [Intel/AMD/future qualified packs] → dGPU pool → iGPU pool → CPU fallback**. Each vendor/runtime sits behind a replaceable module/runtime pack; core does not hard-code vendors.
- Vulkan is an execution/runtime choice within GPU candidates, not a separate hardware tier.
- Automatic policies: Balanced, Best Accuracy, Lowest Latency, Lowest Power; manual has preferred and locked semantics. Planner evaluates hierarchy, model/quant/runtime compatibility, qualification, accuracy, latency/RTF, compile/load cost, memory, power/load where measurable, stability and user preference.
- Model registry: extensible catalog covering all GGUF model variants supported by transcribe.cpp (including every variant of the Whisper model family as weights), Qwen3-ASR OpenVINO tiers, Zipformer/streaming/multilingual families and future architectures; multiple quantizations (FP32/FP16/BF16/INT8/INT4/mixed/vendor formats) as separate artifacts only when runtime supports conversion and accuracy is acceptable.
- Automated ingestion pipeline: importer → validator → backend compatibility → converter/quantizer → accuracy/performance/qualification → registry; catalog metadata updateable separately from app.
- Do not bundle every model or every vendor runtime. Ship a lean base plus registry/manager/metadata/reference CPU path; acquire relevant model/runtime packs on demand. UI distinguishes Installed/Available/Recommended/Qualified/Compatible/Experimental/Unsupported.
- Model Manager UI filters (family/size/language/hardware/NPU/GPU/Vulkan/runtime/quant/installed/qualified/experimental) and detail page (size/runtime/accuracy/latency/RTF/memory/power).
- Every automatic decision is visible and overridable (e.g., `Auto → Intel NPU`), with Reset to Auto per setting and per-app overrides.
- Qualification per artifact tuple, not generic model name; store latency/RTF/memory/stability and compare quant vs baseline.

## Multilingual contract
Spoken language, output language/script, translation target and locale formatting are separate settings; auto-detect/code-switch exposed only when engine/model supports and is qualified; unsupported combos produce clear capability state (FUN-029).

## Runtime/device policy
- Capability is `model + runtime + driver + device + successful self-test`, never mere enumeration.
- Record exact OpenVINO/GenAI/NPU driver/Vulkan/ONNX Runtime versions before claiming accelerator support; recheck upstream at implementation and release.
- CPU fallback remains tested even on accelerator machines.
- Hot/Warm/Cold residency and compiled artifact cache use versioned invalidation + ownership keys (model hash, runtime, device/plugin/compiler properties, preprocessing contract). After Auto benchmark, clean unreferenced losing temporary/converted/compiled variants while retaining source, selected, shared/pinned and CPU fallback assets. Provide Clear Unused Optimized Cache and developer retain-cache controls.

## Tests/evidence
Fixed audio corpus + hashes; expected normalized transcripts/quality threshold; partial/final/timestamp/cancellation; warm/cold latency; runtime-pack staged install/verification; auto candidate benchmark and manual override; benchmark cleanup/restart reconciliation; corrupt download/cache; device failure→CPU fallback; memory/resource record; quant validation vs baseline. Schema-valid evidence per verification IDs; `python3 scripts/phase_gate.py --phase P5` exits 0.
## v1.3 resource registry rule

Implement trusted resource resolution/staged install for Model/Runtime/Module artifacts used in this phase (SEC-007). Upstream and mirror locations are sources, not identity; immutable revision/hash/signature policy controls activation. Whole-pipeline qualification must be able to compare sharing versus splitting ASR/postprocessor workloads across NPU/dGPU/iGPU/CPU.
