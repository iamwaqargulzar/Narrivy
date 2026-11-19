# ADR-0006: Declarative Mode pipeline DAG

- Status: Accepted
- Date: 2026-08-20

## Decision
Modes compile to typed stage graphs that can be optimized and policy-checked.

## Rationale
Avoids hard-coded ASR→LLM pipelines and enables future media/actions.

## Consequences
Implementations that violate this decision require a superseding ADR.
