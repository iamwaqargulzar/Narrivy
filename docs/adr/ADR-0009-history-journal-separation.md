# ADR-0009: Separate crash journal, history and learning stores

- Status: Accepted
- Date: 2026-08-20

## Decision
Recovery, user-visible history and learned personalization have distinct lifecycles and deletion semantics.

## Rationale
Prevents privacy and reliability requirements from becoming entangled.

## Consequences
Implementations that violate this decision require a superseding ADR.
