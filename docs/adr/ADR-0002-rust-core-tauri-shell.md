# ADR-0002: Rust domain core with Tauri 2 desktop shell

- Status: Accepted
- Date: 2026-08-20

## Decision
Use Rust for service/domain/platform adapters and a Tauri 2 shell for the premium cross-platform settings application. The frontend remains a client of typed application APIs.

## Rationale
Balances native systems work, portability and high-end UI velocity.

## Consequences
Implementations that violate this decision require a superseding ADR.
