# ADR-0027 — Portal service composition seam remains startup-disabled

**Status:** Accepted for controlled Recovery Phase 2C wiring (2026-08-27)

## Context

ADR-0024 reserved authenticated IPC commands and a daemon-owned permission
service. ADR-0025 defined portal submission followed by mandatory Inbox
recovery. ADR-0026 supplied the minimal real EI text sender inside the direct
driver. These independently reviewed parts still lacked a typed seam through
which a future desktop composition could connect them without moving portal,
delivery, or policy decisions into the UI.

Native startup composition is not yet authorized. A valid Wayland parent must
be exported from the initiating Tauri main window, kept out of TypeScript and
logs, and qualified in an attended permission window. Default daemon startup
must therefore remain unable to trigger a portal dialog.

## Decision

- Add `build_handler_with_portal`, which accepts an explicit optional
  `Arc<PortalPermissionService>`. The existing `build_handler` and daemon
  `main` pass no service and retain their exact fail-closed behavior.
- When a service is injected, `GetStatus` returns its typed snapshot and the
  three protocol-minor-2 permission commands route to it. Kernel-derived
  same-user credentials are checked before parsing or dispatch, as for every
  command. The self-declared client role remains informational, not an
  authorization factor.
- Parent identifiers are parsed again at the daemon adapter boundary. Invalid
  values return a fixed content-free error and never reach the driver. Parent
  values and operation IDs are not copied into errors, logs, persistence, or
  status fields beyond the already-bounded opaque current operation ID.
- A permission request has one 25-second absolute deadline, below the IPC
  connection idle bound. It can be cancelled by a separate authenticated
  client connection. Explicit revoke has a two-second bound. Stable service
  failures map to stable IPC codes without external error text.
- `PortalPermissionSubmitter` is a delivery-only handle to an injected
  service. It cannot request permission or create a parent identifier. It
  rejects submission while inactive and preserves `SubmittedUnverified`,
  clean rejection, and dispatch uncertainty from the actor.
- Submission admission requires an accepting, non-revoking, active service
  with no pending permission operation and a connected transport. Explicit
  revoke closes that admission under the service-state lock before actor
  close, so a later submit is rejected without reaching the driver.
- If a post-dispatch actor reply misses its caller deadline, the outcome stays
  `DispatchUncertain`; the actor closes its authority and records a typed,
  generation-bound terminal event. A subsequent status read/event therefore
  cannot leave the service falsely `Active` after the actor is idle or invalid.
- `PortalEisThenInboxDelivery::new` becomes a production-shaped constructor
  requiring that submitter and one caller-owned transaction deadline. Every
  non-cancelled portal attempt is still followed by exactly one atomic Inbox
  recovery write under the same transaction ID.
- Provide an explicitly owned portal event pump. It forwards the latest typed,
  content-free service transition, advances its observed sequence even when no
  client receives it, and relies on `GetStatus` for reconnection. The future
  startup owner must stop and join the pump before dropping the service.
- `sender_available` and `delivery_qualified` remain false in every status.
  This seam does not add a pipeline, capability, desktop command, default
  environment switch, or release claim.

## Security and lifecycle consequences

The UI cannot submit text directly and delivery cannot open a permission
dialog. The daemon continues to own single-flight admission, portal/EI state,
revocation monitoring, cancellation, deadlines, local authority teardown, and
shutdown. An unauthorized UID is rejected before the portal service observes
the request. A lost or invalid EIS transport cannot be mislabeled as portal
revocation or verified delivery.

The raw same-user IPC protocol can request permission once a service is
explicitly injected. That is the scope accepted by ADR-0024; desktop-only
authorization cannot be inferred from the untrusted handshake role. Actual
startup injection remains prohibited until the attended parent-window work
below is complete.

## Verification boundary

Controlled tests inject a fake driver and prove unauthorized rejection,
parent validation before driver work, typed request/status/revoke routing,
false sender/delivery capability, exact owner call order, bounded shutdown,
delivery-only active-state admission, revoke/submission linearization,
post-dispatch timeout reconciliation, unverified-outcome preservation, event
deduplication, and the existing exact portal-attempt-plus-Inbox report.

No production `main` construction, Tauri parent export, portal prompt, native
EIS connection, compositor event, target retention, microphone run, evidence,
requirement promotion, package, commit, or push is part of this decision.

## Required next decision

A later ADR must define the Tauri/Wry main-window parent export and lifetime,
the explicit user gesture and UX, startup ownership/shutdown order, and the
attended permission-only native protocol. It must resolve Wayland export using
safe dependencies or a narrowly reviewed adapter; an empty parent, TypeScript
round-trip, overlay-originated request, and desktop-name feature branch remain
forbidden.
