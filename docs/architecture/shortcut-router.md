# Shortcut and Trigger Router

## Linux preference order
1. XDG GlobalShortcuts portal when available and suitable for activation/deactivation semantics.
2. Native desktop/compositor binding/integration.
3. User-configured compositor shortcut invoking Narrivy IPC/CLI.
4. Low-level evdev only as an explicit fallback.

Avoid requiring membership in the Linux `input` group as the normal path.

## First-class trigger types
- keyboard **hold-to-talk**;
- keyboard **toggle**;
- CLI/local IPC trigger;
- mouse/hardware/accessibility trigger where supported;
- future app-rule/profile trigger;
- optional local wake phrase later.

Every binding targets a **Pipeline ID**. Different Pipelines may have different hotkeys and activation semantics. Hold mode requires proven press+release capability; toggle only requires reliable activation events. UI must not advertise hold where the platform cannot prove key-release semantics.

All triggers emit the same typed core intent and no trigger implementation owns session state.
