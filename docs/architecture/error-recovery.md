# Reliability and Error Recovery

## Reliability Supervisor
Track health independently for:
- core/session engine;
- audio;
- ASR runtime;
- shortcut backend;
- text delivery backend;
- overlay/UI channel;
- model/cache subsystem.

## Error classes
- transient/retryable;
- backend-degraded/fallback;
- user-action-required;
- policy-denied;
- corrupt artifact;
- programmer invariant violation.

## Rules
- bounded retry with jitter where appropriate;
- fallback must preserve the session and target when safe;
- no hidden infinite restarts;
- UI errors should explain the recovery taken;
- process isolation may be added for crash-prone runtimes after evidence, not pre-emptively everywhere.
