# Dictation Inbox Contract

## Job
Be the non-destructive destination whenever final text cannot be reliably delivered to the target.

An Inbox item shows final text, time, safe target metadata/reason, and actions: Copy, Retry delivery when still meaningful/safe, Open diagnostics, Delete. Retry is a new delivery transaction and cannot duplicate an already-committed result.

Inbox/recovery items must survive the specified process failure envelope even when user History is disabled. Apply a documented retention policy and warn before automatic expiration if appropriate.

Tests: `E2E-INBOX-FALLBACK`, `INBOX-RETRY-IDEMPOTENT`, `INBOX-HISTORY-OFF`, `INBOX-RECOVERY`.
