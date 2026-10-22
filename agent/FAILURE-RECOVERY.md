# Failure Recovery Protocol

Normal engineering failures are expected. Never stop the build merely because a command failed.

## 1. Capture

Record the command, exit code, shortest useful error excerpt, environment facts, and affected phase/work item. Do not paste secrets or transcript content into logs.

## 2. Classify

Use one primary class:

- `code` — type/logic/ownership/concurrency defect;
- `dependency` — version/feature/API incompatibility;
- `build` — compiler/linker/pkg-config/system library issue;
- `host` — missing tool/package or unsupported host configuration;
- `permission` — desktop portal/device/sandbox permission;
- `platform` — backend/compositor/application incompatibility;
- `network` — download/index/provider unavailable;
- `hardware` — device/driver capability problem;
- `flaky` — nondeterministic timing/race/environment failure;
- `spec` — contradictory or under-specified repository instruction.

## 3. Reduce

Run the narrowest failing test/command. Inspect versions and capability probes. Do not rerun the full build repeatedly without learning anything.

## 4. Verify assumptions

When an external API/runtime behavior may have changed, consult current authoritative upstream docs/releases/issues. Record useful findings in `agent/RESEARCH_LOG.md`.

## 5. Fix or degrade safely

Prefer root-cause fixes. If a capability is genuinely unavailable, use the documented bounded fallback and surface the degraded state through capabilities/diagnostics. Never create an infinite fallback loop.

Examples:

- NPU compilation/runtime failure → preserve session → one bounded fallback to tested GPU/CPU engine → diagnostic reason.
- text-delivery backend failure → try ordered compatible delivery backend(s) → Inbox as final safe sink.
- portal shortcut unavailable → capability reports unavailable/degraded and onboarding offers supported alternative; never fake PTT.

## 6. Prove

Add or improve a regression test when feasible. Run the narrow test, affected integration tests, architecture/security checks, then the phase gate.

## 7. Record

Append concise resolution to `agent/DECISIONS.md` or phase evidence. Persistent unresolved external blockers go to `agent/BLOCKERS.md`.

## Anti-patterns

Do not fix failures by commenting out code/tests, changing tests to assert the broken behavior, pinning random versions without evidence, using broad `sudo`, disabling sandbox/security, hiding stderr, swallowing errors, or substituting a mock for required real-platform evidence.
