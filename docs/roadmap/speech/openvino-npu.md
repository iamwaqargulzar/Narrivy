# OpenVINO / Intel NPU Strategy

> **Status: roadmap / not in 1.0.** This describes a planned design, not current behaviour. See [the roadmap](../README.md).

## Architectural rule

OpenVINO is an engine/runtime adapter, never a domain dependency. Its types, device strings, compiled-model objects, threading assumptions and FFI stay under `engines/asr-openvino` (plus conversion tooling under `tools/`).

## Current upstream reality

OpenVINO/OpenVINO GenAI 2026.4 is the selected runtime-pack baseline. The toolkit supports Intel CPU/GPU/NPU devices generally, but its 2026.4 release lists Qwen3-ASR support on CPU and GPU, not NPU. NPU usability depends on the exact platform, driver, runtime, model and configuration. Narrivy therefore **health-tests each model/device pair and always retains a proven CPU correctness fallback** rather than equating device enumeration with support. Qwen3-ASR-on-NPU remains candidate-only until native qualification proves it.

Authoritative links and the date they were checked are recorded in `archive/docs/references/current-platform-notes.md`; the coding agent must refresh them during P5 and P11.

## Integration preference

Prefer the maintained native OpenVINO/OpenVINO GenAI C++ API behind a narrow Rust FFI wrapper if it provides the required production path. Conversion/export tooling may use Python/Optimum/OpenVINO tooling, but generated artifacts are reproducible and production execution does not require a hidden Python environment unless an ADR explicitly approves that tradeoff.

## Qualification flow

For each candidate model and device:

1. enumerate runtime/device metadata;
2. validate driver/runtime prerequisites;
3. load/compile with deterministic options;
4. run a known non-sensitive audio fixture;
5. validate nonempty/sane output and required timestamps/features;
6. test cancellation;
7. measure cold/warm load and inference;
8. persist a health record keyed by model revision, runtime version, engine adapter version, device/driver identity and compile options;
9. invalidate the health/cache record when a key changes.

## Fallback

Recommended policy is not hard-coded by device name; it is a candidate order resolved from user preference + capability + health. A typical Intel system may prefer a qualified NPU/GPU route and fall back to CPU. On runtime error:

- preserve audio/finalization state within the session's recovery envelope;
- mark the failing model/device health degraded;
- make one bounded retry/fallback if policy/time budget allows;
- avoid duplicated final insertion using the transaction ID;
- expose a privacy-safe diagnostic reason.

## Reference model set

P5 must select exact immutable revisions for:

- Whisper large-v3 family accuracy reference;
- a smaller Whisper-family diagnostic/resource reference;
- a CPU-known-good fallback path.

Do not bundle/download any model automatically until license/provenance/redistribution metadata is complete.
