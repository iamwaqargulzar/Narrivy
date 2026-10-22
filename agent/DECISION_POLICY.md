# Autonomous Decision Policy

## Decide without asking

The coding agent should make and record routine engineering choices itself: compatible libraries, internal abstractions, refactors, names, test frameworks, error types, build scripts, state stores, packaging implementation details, and dependency patch/minor upgrades that preserve requirements.

Use this priority order when multiple valid options exist:

1. correctness and data safety;
2. reliability/recoverability;
3. privacy/security;
4. compatibility with the reference Linux targets;
5. latency/resource efficiency;
6. maintainability/portability;
7. dependency/supply-chain simplicity;
8. UI polish.

Record material choices in `agent/DECISIONS.md`; use an ADR for architecture or compatibility commitments.

## May not decide silently

The agent must not silently:

- weaken/remove a product, privacy, security, accessibility, or release requirement;
- transmit user content remotely by default;
- introduce telemetry containing user content;
- copy third-party source/UI/assets;
- claim unsupported hardware/desktop compatibility;
- choose a legal license on behalf of the owner;
- publish/sign a release with credentials it was not explicitly given.

## Genuine human blockers

Only external actions that cannot be safely automated justify pausing the affected task: physical hardware attachment, interactive permission approval, unavailable signing credentials, mandatory account authentication, or legal/business decisions such as the source license.

Document the blocker, exact command/screen/action required, what work remains, and which independent tasks were completed. Continue all work not dependent on the blocker.
