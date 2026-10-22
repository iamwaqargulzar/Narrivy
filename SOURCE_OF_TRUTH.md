# Source of Truth and Precedence

When instructions conflict, apply this precedence order:

1. The owner's latest explicit instruction.
2. `BUILD_TARGET.json` for the configured autonomous target and stop policy.
3. `docs/product/vision-and-scope.md` (charter and vision) and the normative requirement catalog in `docs/product/requirements.md`.
4. `agent/completion-plan/` for the current owner-approved execution plan and owner decisions (`OWNER-DECISIONS.md`). The superseded v1.3 plan is in `archive/build-pack-v1.3/`.
5. `SECURITY.md`, privacy requirements, clean-room rules, and safety constraints.
6. Accepted ADRs and architecture contracts.
7. The active phase specification + agent work order + acceptance gate.
8. UI/UX specifications and design-system contracts.
9. Feature catalog, roadmap, research notes, and non-normative references.

## Conflict rule

Never silently weaken a higher-priority constraint to make implementation easier. If a lower-priority document is stale, fix the stale document in the same change. Record architectural choices in `agent/DECISIONS.md`; create an ADR when a decision changes a system boundary, public/internal contract, persistent format, security/privacy boundary, or cross-platform strategy.

## Volatile external facts

Tool/library/OS/driver/model details are not permanent truth. Re-verify volatile facts against current authoritative upstream documentation before first implementation and before release qualification. Record the exact resolved versions in `agent/RESOLVED_VERSIONS.json` and the supporting links/notes in `agent/RESEARCH_LOG.md`.


## v1.3 canonical architecture

Read `docs/architecture/README.md` and `spec/catalogs/` before changing pipelines, engines, modules, runtimes, models or providers.
