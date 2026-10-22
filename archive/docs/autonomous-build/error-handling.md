# Error Handling for Autonomous Builds

See `agent/FAILURE-RECOVERY.md` for the normative loop.

## Build-error rule

Compiler/linker/test/runtime errors are expected signals. The agent must not stop and ask the user to interpret them. It should inspect the exact failing layer, determine whether the repository, dependency version, system prerequisite, API assumption or host is wrong, and fix/document it.

## Unknown API/version rule

Do not blindly retry code written for a stale API. Check authoritative upstream docs/releases/source for the version actually selected, then add a focused compile/integration test that prevents recurrence.

## Environment-dependent rule

Keep three states distinct:
- code defect;
- capability unavailable on this environment;
- human permission/hardware action required.

Only the third can become a human blocker, and independent code/tests continue.
