# ADR-0024 — Portal keyboard permission IPC contract

**Status:** Accepted; production permission composition added by ADR-0066,
attended behavior and delivery remain unqualified (2026-09-11)

## Context

ADR-0023 established a bounded RemoteDesktop/EIS eligibility probe, lifecycle,
actor, and direct-zbus permission driver. That implementation is deliberately
unreachable from production composition. Exposing it later requires an
additive, authenticated desktop-to-daemon control contract that cannot confuse
permission or an EIS connection with working text delivery.

The current IPC server authenticates the kernel-derived peer UID before
dispatch. The handshake `client_role` is self-declared and is not an
authorization factor. Therefore this contract may claim same-user access only;
it may not claim desktop-only authorization.

## Decision

- Protocol major remains 1 and minor becomes 2.
- The stable request tags are
  `request.request_portal_keyboard_permission`,
  `request.cancel_portal_keyboard_permission`, and
  `request.revoke_portal_keyboard_permission`. The request wording is
  intentional: only the user and system portal can grant permission.
- Request and cancel carry a collision-resistant operation identifier. Request
  also carries the desktop-created parent-window token. Operation identifiers
  are 1..=64 ASCII alphanumeric, hyphen, or underscore bytes. Parent tokens are
  1..=4096 bytes and reject control characters. Bounds apply on serialization
  and deserialization.
- The envelope request kind must equal the decoded command kind. A mismatched
  outer tag is a content-free `BadRequest`; it is never dispatched.
- Portal status is an optional, defaulted `StatusDto` field so protocol-minor
  0/1 messages remain readable. Eligibility, lifecycle, failure, and cleanup
  values preserve unknown future wire strings losslessly.
- Status distinguishes `permission_active`, `transport_connected`,
  `sender_available`, and `delivery_qualified`. A later implementation must
  keep the final two false until a sender exists and target retention is
  independently qualified.
- A typed response and sequenced change event are reserved for the permission
  lifecycle. The event is required for portal-originated revocation; polling
  alone is not sufficient lifecycle integration.
- `busy` is a stable error code for the future single-flight service. Unknown
  error codes remain losslessly preserved.
- Public `Command` debug output redacts portal operation and parent values.
  Public `Envelope` debug output redacts its entire payload. Production
  rejection details never echo either value.
- A generic daemon-owned `PortalPermissionService` implements the controlled
  lifecycle behind this contract. It owns one bounded actor, single-flight
  admission, operation-ID idempotence, cancellation, explicit revoke,
  portal-originated revocation reconciliation, sequenced status, and bounded
  shutdown. It always reports sender and delivery qualification false.
- Actor events carry their activation generation. A matching revocation that
  arrives after actor activation but before the service commits `Active` is
  deferred, then reconciled before the request returns. A portal revocation
  overlapping explicit revoke takes precedence over the actor's
  `AlreadyIdle` close result and remains `Revoked / ConfirmedClosed`.
- At this ADR's acceptance point, the generic service was constructed only by
  controlled tests and all three production IPC commands returned content-free
  `DaemonUnavailable`. ADR-0027 subsequently added an explicit injection seam.
  ADR-0066 later replaced the default uncomposed startup behavior.

## Implemented controlled-service guarantees

- The daemon-side coordinator, not IPC or the desktop, owns the permission
  lifecycle, at most one pending request, at most one active transport,
  revocation monitoring, cleanup certainty, and bounded shutdown.
- The same operation ID is idempotent while pending and after its terminal
  result; a different ID returns `Busy`. Cleanup failure makes the owner
  terminally invalid rather than silently creating a replacement.
- Shutdown closes admission before cancellation and join. Late request
  completion cannot overwrite revoke or shutdown state. Actor-level timeout
  never manufactures confirmed cleanup.
- Portal-originated revocation is a typed sequenced transition, including the
  two activation/revoke contention windows described above.

## Future production-composition requirements
- The desktop obtains the parent identifier in Rust from the main window,
  retain its lifetime through portal `Start`, and never expose it to
  TypeScript, logs, persistence, or errors. The overlay must not invoke the
  command. If a valid Wayland parent cannot be exported, the implementation
  stops; it does not fall back to an empty parent without another ADR.
- User-mediated portal waits use a dedicated bounded deadline and an explicit
  cancel path. A desktop timeout must request daemon cleanup before reporting
  completion.
- Permission-connected UX continues to state that direct delivery is
  unqualified. Permission is never requested silently or during onboarding.

ADR-0066 composes this permission-only path so an eligible desktop can perform
the attended check. Qualification remains a prerequisite for promoting sender
or delivery capability, not for exposing the explicit settings action.

## Consequences

The repository now has a forward-compatible, bounded wire vocabulary and a
controlled daemon coordinator for the permission lifecycle without making the
native driver reachable. This is an implementation contract, not evidence of
a portal grant, EIS capability, input submission, target behavior, or release
readiness.

## Subsequent controlled wiring

ADR-0027 added an explicitly injected handler, delivery submitter, and event
pump around this contract. At that acceptance point, default daemon startup
still supplied no service. ADR-0066 now composes the permission service while
retaining the attended-permission and delivery-qualification blockers.

The next production composition remains blocked on an attended permission-only
native check and explicit desktop parent-handle validation. ADR-0025 defines
the submission and Inbox-recovery contract, and ADR-0026 supplies its real wire
sender; neither authorizes default startup composition.

## Verification

Controlled tests cover exact tags, request classification, minor compatibility,
unknown-value round trips, both-direction field bounds, debug redaction,
disguised payload rejection, content-free unavailable responses, existing
same-user transport behavior, single-flight/idempotence, cancellation,
revocation, cleanup failure, shutdown, and deterministic activation/revoke
contention. Independent re-review passed after both event-linearization races
were fixed. Workspace compilation, tests, and touched daemon/IPC production
Clippy pass. No native portal behavior was exercised.
