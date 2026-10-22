# Autonomous Build Policy

The coding agent owns routine implementation decisions and must continue through `BUILD_TARGET.json.target_phase` without waiting for approval between phases.

## Per-phase loop

1. Read `SOURCE_OF_TRUTH.md`, `BUILD_TARGET.json`, `agent/STATE.json`, active phase spec, and active phase work order.
2. Run doctor + pack validation + the smallest relevant baseline tests.
3. Create/update the phase report with a short execution plan and environment facts.
4. Build in vertical slices. Keep already-working user paths working.
5. Run focused tests continuously, then the phase gate.
6. Diagnose failures instead of documenting them away.
7. Record measurements, resolved dependency/API versions, decisions and evidence.
8. Mark requirements passed only when evidence exists.
9. Advance state only after the gate passes; immediately continue.

## Do not stop for

Compiler/linter/test errors, dependency API drift, ordinary platform bugs, a reasonable library choice, optional later-feature absence, or a failed first/second implementation approach.

## Legitimate external blockers

Pause only for something the agent cannot safely obtain/infer: owner-reserved legal/license choice; interactive OS permission requiring the user; unavailable physical certification hardware after all independent work is complete; signing/store credentials; mandatory account authentication; or an irreversible external action requiring authorization. Record the blocker and exact unblock steps, then continue every independent task.

## Recovery loop

`reproduce → classify → isolate → verify assumptions/upstream API → smallest valid fix/fallback → regression test → focused retest → phase gate`

Three failed approaches are a signal to reduce the problem and consult authoritative upstream sources—not to weaken a requirement.

## Forbidden shortcuts

Never disable/skip tests to pass a gate, fake evidence, lower a quality/security threshold without an ADR and requirement decision, broaden permissions, log user content for convenience, hard-code a developer-machine path, use mock-only evidence for a real-hardware requirement, or mark unsupported capability as supported.
