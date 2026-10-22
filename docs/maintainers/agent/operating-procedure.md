# Coding Agent Operating Procedure

For every work item:
1. Identify active phase and requirement IDs from durable state/traceability.
2. Verify assumptions from repository, machine probes, compiler/tests or authoritative upstream sources; do not ask the user for routine engineering choices.
3. Check controlling ADRs/contracts.
4. Design/evolve portable interfaces before coupling concrete adapters.
5. For regressions/state/policy logic, reproduce with a test before or alongside the fix where practical.
6. Implement a vertical slice and keep changes reviewable/revertible.
7. Run the narrow failing test, affected suite, then phase gate.
8. Update docs/schemas/migrations/evidence in the same work.
9. Record durable decisions/research only when they help a future contextless agent resume.
10. If a normal error occurs, follow `agent/FAILURE-RECOVERY.md` and continue.

When external APIs/libraries are needed, verify current upstream documentation/releases/licenses at implementation time. Do not rely on the pack creation date for volatile versions.
