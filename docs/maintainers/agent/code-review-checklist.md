# Code Review Checklist

- No `unwrap`/panic in production flow.
- Ownership and cancellation of background tasks are explicit.
- Locks are not held across blocking/awaited work without strong justification.
- Audio callback remains real-time safe.
- Typed domain values replace ambiguous strings/booleans.
- Error variants retain actionable context without leaking sensitive content.
- Fallback loops are bounded.
- Platform FFI is isolated.
- Unsafe code, if unavoidable, documents invariants and has focused tests.
- No transcript/audio/context in default logs.
- New dependency license/maintenance/security considered.
