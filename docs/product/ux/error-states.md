# Error and Recovery UX

Bad: `OpenVINO compile_model failed: DEVICE_BUSY`.

Good:
**Intel NPU is busy.** Narrivy switched this dictation to CPU. You can keep working. `View diagnostics`.

Every surfaced error should answer:
1. What happened?
2. Did Narrivy recover?
3. Was any speech/text lost?
4. What can the user do?
5. Is a technical detail view available?

Never show raw stack traces in primary UI.
