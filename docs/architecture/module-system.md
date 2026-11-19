# Modular Capability System

Narrivy is **module-oriented internally from the start**. “Plugin” is reserved for an externally distributable extension contract; internal module boundaries must not wait for a public plugin SDK.

## Goals

- replace implementations without rewriting domain logic;
- allow hardware/vendor/runtime diversity behind capability contracts;
- make audio, speech, text intelligence, providers, triggers, context, and delivery independently replaceable;
- compose modules into validated first-class Pipelines;
- isolate failure-prone/untrusted native code where evidence justifies it;
- keep user choice and Auto selection visible;
- avoid an unstable Rust dynamic-library ABI.

## Capability families

| Family | Examples | Typical trust/execution |
|---|---|---|
| trigger/activation | hold/toggle hotkey, CLI, future wake/hardware | trusted platform adapter |
| capture | PipeWire/WASAPI/CoreAudio | trusted adapter |
| audio preparation | resample, channel conversion, high-pass, limiter, AGC | built-in/native |
| AEC/spatial | echo cancellation, beamforming, dereverb | trusted native/worker |
| enhancement | denoise, source separation, target-speaker extraction | native/worker/accelerated |
| speech boundary | VAD, endpointing, keyword spotting | built-in/module |
| speech intelligence | language ID, diarization, speaker ID, audio tagging | module/worker |
| ASR engine | Qwen3-ASR/OpenVINO, transcribe.cpp GGUF families | trusted engine/worker |
| compute runtime | CPU, Vulkan GPU paths, OpenVINO NPU/vendor runtimes | trusted Runtime Pack |
| deterministic text | punctuation, ITN, spelling, grammar, dictionary | portable; WASM candidate |
| generative text | faithful cleanup, polish, rewrite | local/remote provider-backed |
| inference provider | local runtime/server or cloud API transport/auth | permission-scoped |
| context provider | target/app/selection/project metadata | permission-scoped |
| delivery backend | direct, clipboard, portal, virtual input, Inbox | trusted adapter |
| resource source | model/module/runtime catalog/mirror | data/network-scoped |

## Registry and selection

`narrivyd` owns a `ModuleRegistry`/capability registry. Modules expose stable identifiers, API version, capabilities, health, dependencies, trust level and qualification state. Core asks for a capability; Pipeline composition chooses an implementation according to policy and user override.

No module is considered healthy merely because it loaded. Health is separate from availability and may be `available-unqualified`, `qualified`, `degraded`, `failed`, or `permission-required`.

## Execution trust classes

1. **Trusted native/in-process** for project-controlled latency-critical code only.
2. **Sandboxed WASM/WASI** for portable third-party text/rule/config logic where feasible.
3. **Isolated worker/sidecar** for heavyweight, crash-prone, Python/ML, or third-party native components.

A manifest does not make unsafe native code safe. Public Rust dylib/trait-object loading is prohibited as a durable third-party ABI.

## Failure containment

- Module failure returns typed outcomes; it must not panic across a host boundary.
- Session/transaction IDs protect against stale output after module restart/cancellation.
- Fallback never silently gains permissions, network access, or a different privacy classification.
- Optional smart modules can fail without taking down the basic dictation path.

See `pipeline-profiles.md`, `plugin-architecture.md`, `resource-registry.md`, and ADR-0014/0016/0017.
