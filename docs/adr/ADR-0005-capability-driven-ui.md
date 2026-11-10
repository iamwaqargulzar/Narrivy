# ADR-0005: UI renders capabilities, not OS branches

- Status: Accepted
- Date: 2026-08-20

## Decision
Frontend receives capability/status models and does not encode platform behavior.

## Rationale
Reduces divergent UX logic and handles Linux desktop variation cleanly.

## Consequences
Implementations that violate this decision require a superseding ADR.
