# ADR-0004: Long-running core service with portable local IPC

- Status: Accepted
- Date: 2026-08-20

## Decision
Use a service process for session/model/audio ownership; clients communicate over a transport-neutral protocol.

## Rationale
Separates UI lifecycle from dictation reliability and supports CLI/integrations.

## Consequences
Implementations that violate this decision require a superseding ADR.
