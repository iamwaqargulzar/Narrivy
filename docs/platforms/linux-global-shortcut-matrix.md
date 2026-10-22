# Linux Global Shortcut / PTT Qualification Matrix

This is a qualification contract, not a list of presumed working APIs. The coding agent updates measured results during P3/P11.

| Candidate path | Intended role | Activation | Release/deactivation | Wayland | Risk/notes | 1.0 policy |
|---|---|---:|---:|---:|---|---|
| XDG GlobalShortcuts portal | preferred standards-based route | API supports activation | API supports deactivation signal; desktop implementation must be measured | yes | portal support/semantics vary by implementation; user permission/setup UX may apply | prefer when reliable |
| desktop/compositor-specific supported API | first-class adapter fallback | qualify | qualify | yes | keep isolated in Linux adapter; version/capability probe | allowed with evidence |
| X11/XWayland-compatible shortcut path | compatibility | qualify | qualify | X11 path | do not confuse XWayland target compatibility with Wayland compositor control | compatibility only |
| low-level input/device access | last-resort advanced fallback | qualify | qualify | can bypass compositor | broad permissions/security/support burden | opt-in only; not default |

## PTT acceptance

A backend can advertise `push_to_talk=true` only if tests prove:

1. activation occurs once for the intended press/hold gesture;
2. deactivation/release is received reliably;
3. cancellation/session transitions do not stick after lost focus/overlay/client changes;
4. repeated rapid presses do not leave capture active;
5. suspend/resume and portal/backend restart recover predictably;
6. latency is measured against `PERF-002`/`PERF-004` on reference hardware.

If release semantics are not reliable, advertise toggle-only instead of pretending PTT works.

## Current measured position (content-free environment observations)

Read-only inspection on the current native COSMIC session (see
`agent/RESEARCH_LOG.md`) found `org.freedesktop.portal.RemoteDesktop` version 2
but **no** `org.freedesktop.portal.GlobalShortcuts` interface on the canonical
portal object `/org/freedesktop/portal/desktop`. Upstream COSMIC still tracks
that portal as missing, while COSMIC Settings supports user-created command
shortcuts. This is environment evidence only, not release qualification.

Consequences encoded in the product:

- the portal GlobalShortcuts route is probed by one bounded, direct D-Bus
  query of the exact interface/property and reports unavailable when absent —
  never inferred from a desktop name;
- an installed atomic command fallback is available for a user-managed
  shortcut: `/usr/bin/narrivy-cli toggle` (optional `--pipeline raw|clean`).
  Narrivy does not configure that shortcut, and the daemon does not claim a
  listener merely because the interface (or the command) is available.

## Evidence columns to add during implementation

For each COSMIC/GNOME reference image record: OS, desktop version, portal backend/version, shortcut path, permission flow, activation p50/p95, release p50/p95, 100-cycle reliability, suspend/resume result, and evidence path.
