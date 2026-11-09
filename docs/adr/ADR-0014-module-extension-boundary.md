# ADR-0014: Modular internal contracts; no public Rust dylib ABI

- Status: Accepted
- Date: 2026-08-21

## Decision

Narrivy is composed from replaceable internal modules behind capability contracts. Linux 1.0 may compile trusted implementations into the application, but domain logic cannot depend on concrete modules.

A future public extension ecosystem will prefer validated data packs, WASM/WASI for portable untrusted processors, and versioned sidecar IPC for native/high-performance extensions. Narrivy will not publish arbitrary Rust trait-object or `cdylib` loading as its stable plugin ABI.

## Rationale

The owner wants users to be able to replace components such as noise suppression, ASR/runtime and delivery implementations. Designing this boundary now prevents vendor/runtime coupling. Avoiding a Rust dylib ABI prevents long-term allocator/panic/compiler/dependency compatibility problems and allows stronger crash/security isolation.

## Consequences

- Add module/capability registry concepts during P1.
- Keep module API stability `internal-evolving` until exercised by P2–P5.
- Public plugin SDK remains post-1.0 unless explicitly promoted.
- Any in-process native third-party ABI requires a superseding ADR.
