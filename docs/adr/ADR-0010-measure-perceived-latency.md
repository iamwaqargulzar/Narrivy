# ADR-0010: Optimize end-to-end perceived latency

- Status: Accepted
- Date: 2026-08-20

## Decision
Track trigger→listen, speech→partial, release→raw and release→commit independently.

## Rationale
Prevents model-only benchmarks from hiding UI/context/output delays.

## Consequences
Implementations that violate this decision require a superseding ADR.
