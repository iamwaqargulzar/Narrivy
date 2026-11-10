# Home Screen Contract

## Job
Answer “Can I dictate right now?” and provide one obvious start path.

## Hierarchy

1. readiness hero: `Ready`, `Needs setup`, `Degraded`, or `Unavailable` with plain-language reason;
2. current trigger and mode;
3. microphone + model/compute summary;
4. local/remote processing boundary;
5. primary `Start dictation`/trigger guidance action where useful;
6. recent recoverable Inbox item only when action is needed;
7. lightweight shortcuts to Models/Privacy/Diagnostics when those are blocking readiness.

Do not turn Home into a dashboard of engineering metrics.

## States

- first run routes to onboarding;
- model downloading/compiling shows progress and whether dictation with another model is still possible;
- permission missing shows one remediation action;
- degraded accelerator shows CPU fallback as usable if qualified;
- daemon reconnect preserves a stable shell and shows recovery, not a blank app.

## Keyboard/accessibility

Primary action appears early in focus order. Readiness text is semantic, not color-only. Avoid live-region chatter for every partial capability probe; announce meaningful readiness change.

## Tests

`UI-HOME-READY`, `UI-HOME-FIRST-RUN`, `UI-HOME-DEGRADED`, `UI-HOME-RECONNECT`, `A11Y-HOME-KEYBOARD`.
