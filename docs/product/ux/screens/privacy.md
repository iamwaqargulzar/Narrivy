# Privacy Screen Contract

## Job
Make retention and processing boundaries understandable and controllable.

Sections:
- Processing: local default, configured remote providers only when available;
- History retention: on/off/duration/clear with clear effect;
- Privacy Zones: scope, what they override, active indicator;
- Context permissions: provider-by-provider requested fields/sensitivity;
- Secrets/integrations: credential presence/removal without displaying secrets;
- Diagnostics data: preview categories included in support reports.

Recovery journal is explained separately from optional History: disabling History must not make an in-flight final transcript unrecoverable inside the short documented recovery window.

Any remote action has a data-use description before first use. “Local” is never displayed while a remote stage is active.

Tests: `PRIV-LOCAL-DEFAULT`, `PRIV-ZONE-OVERRIDE`, `PRIV-HISTORY-OFF`, `PRIV-REMOTE-PREVIEW`, `SEC-SECRET-NODISPLAY`.
