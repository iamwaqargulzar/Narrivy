# ADR-0066: Production portal permission composition

Status: Accepted for source composition; attended behavior and delivery remain unqualified (2026-09-11)

Status note (2026-10-30): the GTK shell named below was removed (ADR-0075);
the Tauri app is the desktop client. Portal window parenting is dormant.

## Context

The daemon already owns a tested single-flight portal permission service, a
direct zbus RemoteDesktop/EIS driver, cancellation, revocation reconciliation,
event publication, and bounded shutdown. The GTK desktop now owns a GTK4
xdg-foreign parent export and a pure coordinator that orders permission,
cancel/revoke, local release, and close replay. Leaving both halves disconnected
prevents an eligible host from performing the attended qualification required
by ADR-0024.

## Decision

`narrivyd` performs the existing bounded, read-only RemoteDesktop probe during
startup and maps its result to the typed IPC eligibility state. It constructs
the existing direct driver and `PortalPermissionService`, supplies that service
to `build_handler_with_portal`, and owns the portal event pump. Daemon shutdown
closes portal admission and requires portal cleanup and event-pump join before
the general worker supervisor completes.

The GTK shell exposes request and cancel only through its native bridge. A
request is available only from the Text Delivery settings screen after daemon
status reports `can_request`; onboarding never requests permission. The raw
parent token and operation identifier remain in Rust and are absent from
JavaScript, logs, persistence, and diagnostic output.

Permission success continues to report `sender_available=false` and
`delivery_qualified=false`. Composition therefore enables an attended
permission experiment without claiming working text insertion.

## Failure and lifecycle rules

- Unavailable, old, keyboard-less, and failed probes never enable the request action.
- Every IPC exchange has one absolute deadline. Cancel reserves a short budget and always attempts revoke on a fresh authenticated connection.
- GTK close/exit waits for remote cleanup and local parent release when the current desktop process owns an operation or grant.
- Unconfirmed local or remote cleanup remains terminally fail-closed.
- Daemon shutdown failure is sticky when portal cleanup or the portal event worker cannot be confirmed.

## Verification boundary

Controlled service, state-machine, transport, frontend, and shutdown tests pass.
A native GTK process runs on `GdkWaylandDisplay` and the existing xdg-foreign
diagnostic exported and released a parent handle. The current COSMIC portal
reports `KeyboardUnavailable { version: 2 }`; therefore no attended permission
dialog, EIS connection, text submission, target behavior, package, or release
qualification occurred. No requirement or schema-v2 evidence record advances.
