# ADR-0003: Capability-driven platform API

- Status: Accepted
- Date: 2026-08-20

## Decision
All OS/desktop behavior is accessed through explicit platform capabilities/traits.

## Rationale
Linux-first implementation remains portable to Windows/macOS and UI degrades based on actual capability.

## Consequences
Implementations that violate this decision require a superseding ADR.
