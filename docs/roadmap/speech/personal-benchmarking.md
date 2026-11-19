# Model / Runtime / Device Qualification and Benchmarking

> **Status: roadmap / not in 1.0.** This describes a planned design, not current behaviour. See [the roadmap](../README.md).

## Goals

Choose a reliable default tuple on the user's machine, detect regressions and support explicit manual choice. Do not benchmark with private user dictation.

## Candidate policy

Auto candidate preference is **NPU → dGPU → iGPU → CPU**, filtered first by compatibility and existing qualification. The benchmark may choose a lower tier if it is demonstrably better for the selected model/policy. CPU is always tested as the correctness baseline/fallback.

A full benchmark is optional when a previously qualified tuple remains valid. Re-run only affected candidates after relevant model/runtime/driver/device changes.

## Fixed test material

Maintain redistributable or project-generated speech fixtures with documented license/provenance. Include short/long speech, silence, punctuation-like phrasing, names/technical vocabulary, multiple supported languages where declared, and representative noise levels. Never check in user recordings.

## Measures

For every qualified combination record:

- model/runtime/engine/device/build identity;
- device class (`npu`, `dgpu`, `igpu`, `cpu`);
- cold load/compile and warm load;
- first partial latency if supported;
- final latency/real-time factor;
- peak working memory and accelerator memory where available;
- fixed accuracy measure (for example WER/CER plus task-specific checks);
- timestamps validity when claimed;
- cancellation latency/behavior;
- stability/repeated-run result;
- fallback outcome after forced runtime failure;
- optional power/load observations when measured reliably.

Use repeated samples and report median/p95 where latency targets matter. Store raw measurement data and summarized evidence, not only screenshots.

## Selection

Scoring must have an explicit quality/stability floor. A fast but inaccurate or unstable accelerator does not beat a slower qualified path. The resolved result is visible and can be overridden by the user.

## Cache hygiene

Track all conversion/compile artifacts created by benchmark ownership. After selection, remove unreferenced losing benchmark variants and failed staging artifacts while preserving immutable source models, selected optimized assets, shared/pinned assets and CPU fallback requirements. Record reclaimed bytes; developer mode may retain artifacts for diagnostics.

## Release comparison

P11 compares the release candidate against the last accepted baseline. Material regressions require explanation/fix/explicit decision rather than silently updating the baseline.
