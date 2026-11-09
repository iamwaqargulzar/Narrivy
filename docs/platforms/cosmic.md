# COSMIC Reference Adapter/Qualification

COSMIC is a first-class Linux 1.0 environment, not “generic Wayland”. Keep compositor/desktop-specific code inside the Linux adapter.

P3–P6/P11 evidence must cover:

- GlobalShortcuts portal or selected supported trigger path, including press **and release** semantics for PTT;
- permission/onboarding flow and rebind behavior;
- PipeWire microphone capture, device switch/hotplug, suspend/resume;
- audio-policy restoration after cancellation/crash scenarios;
- overlay placement/focus on single/multi-monitor and mixed scaling;
- target discovery capabilities actually available;
- text-delivery matrix including COSMIC Terminal, browser, VS Code/Electron, LibreOffice, GTK and representative XWayland target;
- Unicode/multiline/selection replacement cases;
- daemon/autostart/package integration.

Record exact Pop!_OS/COSMIC/kernel/portal/PipeWire versions in release evidence instead of assuming behavior remains fixed across releases.

## GlobalShortcuts position

Read-only host inspection on the current native COSMIC session found the portal
exposing `org.freedesktop.portal.RemoteDesktop` v2 but not
`org.freedesktop.portal.GlobalShortcuts`; upstream COSMIC still tracks that
portal as missing, while COSMIC Settings supports user-created command
shortcuts. Until a real portal listener is composed, the truthful activation
path on this host class is the installed atomic command
`/usr/bin/narrivy-cli toggle`, which the user may bind in COSMIC's own shortcut
settings. The daemon reports the command, the portal interface probe, and the
(absent) live listener as three separate capability facts and never claims the
shortcut is configured. See `docs/platforms/linux-global-shortcut-matrix.md`.
