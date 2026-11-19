# ADR-0015: Lean base install with qualified optional runtime packs

- Status: Accepted
- Date: 2026-08-21

## Decision

Ship a lean base installation with a mandatory CPU correctness path. Acquire vendor/specialized accelerator runtimes as independently versioned packs only when compatible hardware is detected and policy/user choice permits.

Auto candidate preference is NPU → dGPU → iGPU → CPU, but selection is qualification/benchmark-driven and user-overridable. Benchmark-created artifacts for losing paths are cleaned when unreferenced; source models, selected optimized artifacts and CPU fallback assets are retained.

## Rationale

This minimizes installer size and stale vendor baggage while preserving out-of-box correctness and taking advantage of accelerator hardware when it is genuinely beneficial.

## Consequences

- P5 implements runtime-pack management, qualification, manual/auto selection and cache ownership.
- Runtime download is never an implicit privacy/network exception.
- Hardware enumeration is not a support claim.
- CPU fallback remains installed/tested even on accelerator systems.
