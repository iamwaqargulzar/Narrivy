# Session State Machine

Recommended logical states:

```text
Idle
  ↓ trigger
Preparing
  ↓
Listening
  ↓ stop/endpoint
FinalizingAudio
  ↓
Transcribing
  ↓
Transforming
  ↓
Delivering
  ↓
Committed
  ↓
Idle
```

Terminal/deviation paths:
- Cancelled
- RecoverableFailure → fallback/retry
- Failed → journal + user-visible recovery

## Invariants
- exactly one primary interactive dictation session initially;
- every state transition is explicit and logged content-free;
- cancellation is idempotent;
- no text delivery after cancellation unless user explicitly recovers;
- target context is captured with session identity, not read arbitrarily at delivery time;
- delivery result is journaled before session data is evicted.
