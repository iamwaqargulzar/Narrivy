# ADR-0008: Sandbox portable plugins where feasible

- Status: Proposed
- Date: 2026-08-20

## Decision
Use WASM/WASI for untrusted portable processors/modes; native extensions remain restricted/trusted.

## Rationale
Improves cross-platform safety while preserving performance for hardware/platform adapters.

## Consequences
Implementations that violate this decision require a superseding ADR.
