# Linux Platform Architecture

## 1.0 reference scope

First-class session for 1.0 (owner decision 2026-09-28):
- Pop!_OS 24.04 / COSMIC on Wayland.

Ubuntu / GNOME on Wayland is the first **post-release** lane (see the
[roadmap](../roadmap/README.md)).

XWayland/X11 target applications are compatibility targets where the selected backend can safely address them. Additional desktops/distributions require their own evidence before being advertised.

## Capability model

Do not expose a single `linux_supported` boolean. Publish granular capabilities with status (`available`, `degraded`, `unavailable`, `permission_required`, `unhealthy`) and reason/source:

- global shortcut activation;
- global shortcut deactivation/release suitable for PTT;
- toggle shortcut;
- microphone enumeration/capture;
- audio policy control/restoration;
- active application/target discovery;
- direct text commit;
- clipboard-paste delivery;
- selected-text replacement;
- Unicode/multiline support;
- suspend/resume/hotplug;
- secure secret storage;
- per-device compute capabilities.

Capabilities are probed at runtime and can change during a session. The UI consumes these states; it does not infer them from desktop name.

## Preferred Linux stack

- XDG Desktop Portal for standardized desktop-mediated capabilities where the relevant portal implementation is sufficient.
- PipeWire/WirePlumber for audio capture/device lifecycle.
- Desktop/input-method/native mechanisms for text delivery where practical.
- Least-privilege explicit fallbacks only after capability testing.
- Secret Service or an abstraction backed by the desktop credential store for secrets.

Low-level broad input device access must not be the default onboarding route.

## Runtime detection

Capture session type, desktop/compositor identity/version where obtainable, portal implementation/version, PipeWire availability, input-method environment, and fallback executable availability as **diagnostic metadata**, not as application policy by itself. Avoid trusting environment variables without capability probes.

## Testing rule

A backend/desktop/target combination is `supported` only after a repeatable capability or E2E test. Keep results in the matrices beside this document and release evidence. Unknown combinations remain unknown/degraded rather than assumed compatible.
