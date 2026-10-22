# ADR-0025 — Controlled portal/EIS text-submission contract

**Status:** Accepted for Recovery Phase 2C; implemented by the non-composed
minimal sender in ADR-0026 (2026-08-27)

## Context

ADR-0023 established a non-composed RemoteDesktop/EIS permission owner and
ADR-0024 established its authenticated IPC vocabulary and controlled daemon
coordinator. Neither decision approved an EIS sender. Phase 2B also showed why
local command acceptance is not target delivery: `wtype` accepted bytes while
the observed COSMIC Edit target remained empty.

Upstream libei distinguishes transport submission from target behavior. An
`ei_text.utf8` request is sender-only, nonempty, limited to one request per
device frame, and may be routed through an input method without keyboard
activity. The sender must finish the logical event with `ei_device.frame`.
Consequently, even a fully written frame is only `SubmittedUnverified` until a
separate target-retention observation proves otherwise.

An exact source audit of `reis` 0.7.1 rejected its convenience APIs: the debug
path can expose protocol arguments, the blocking/Tokio integrations do not own
bounded writable progress as required, the default handshake is too broad, and
the high-level converter is not `Send`. ADR-0026 later approves only the
low-level typed codec behind a Narrivy-owned state machine; none of those
rejected paths is used.

## Decision

- Define a Linux-adapter-local `PortalEisTextPayload` that admits only nonempty
  printable single-line ASCII of at most 254 bytes. This intentionally narrow
  subset cannot contain a NUL, control byte, newline, or partial multibyte
  character and fits one upstream UTF-8 request. Unicode, multiline text,
  chunking, and retry are deferred rather than partially implemented.
- Payload debug output is always redacted. Stable failures contain codes only;
  they do not retain transport text, portal diagnostics, or payload content.
- The driver boundary runs on the existing single-owner portal actor. The
  trait default remains fail-closed. ADR-0026's direct-zbus implementation is
  the only concrete sender and remains unreachable from production clients.
- A future sender defines the point of no return as the first positive OS write
  of payload-bearing protocol bytes. Cancellation before that point may return
  `CancelledBeforeDispatch` only after cleanup is confirmed. Cancellation,
  timeout, revocation, or transport failure after that point is
  `DispatchUncertain`.
- `SubmittedUnverified` requires one complete UTF-8 request and its device
  frame to have been handed to the OS transport without a local error. It does
  not mean the compositor processed it or the focused target retained it.
- Admission, submission, cancellation, close, and reap share a caller-owned
  absolute deadline. Cleanup failure is reported as `CloseFailed` and leaves
  the lifecycle invalid; it can never be relabeled as clean cancellation.
- Submission-derived revocation or teardown is recorded as a generation-bound
  daemon lifecycle event. The permission coordinator therefore cannot remain
  stale `Active` after its actor has moved to `Idle` or `Invalid`.
- Define a controlled-only `portal-eis-then-inbox` report shape: exactly one
  direct `portal-eis-text` attempt followed by exactly one Inbox attempt under
  the same transaction ID. Direct success is `SubmittedUnverified`; direct
  rejection or uncertainty is `Failed` with a typed reason. Only a successful
  Inbox write may produce final `InboxFallback` / journal `InboxRecovered`.
- At this ADR's acceptance point, the recovery adapter constructor was
  test-only. ADR-0027 subsequently exposes the same constructor and a
  delivery-only daemon-service submitter, both requiring the caller-owned
  absolute deadline. There is still no production pipeline ID, capability
  promotion, default startup injection, or CLI/desktop delivery route.

## Consequences

ADR-0027 adds the production-shaped constructor and daemon-service submitter
for this exact report contract, but default startup and every native/release
claim remain disabled.

The repository now has a small, privacy-preserving contract with deterministic
first-write, deadline, cleanup, revocation, and Inbox-recovery semantics. The
wire sender can emit when explicitly composed, but the shipped daemon startup
does not compose it and no native target behavior is qualified.

ADR-0026 supplies the dependency-clean wire sender described above. Production
delivery remains blocked on parent-window integration, production composition,
attended portal behavior, and attended target retention. Those later steps may
not reinterpret controlled protocol tests as native or release evidence.

No native portal prompt, EIS connection, input event, microphone run, target
observation, evidence, requirement status, manifest, package, commit, or push
is authorized by this decision.

## Controlled verification

Tests cover payload bounds/redaction, the first-positive-write cancellation
boundary on both sides, cleanup failure, actor reply timeout, submission-time
revocation reconciliation, expired caller deadlines, exact direct-plus-Inbox
reports, malformed report rejection, and production-driver sender
unavailability. These are unit/contract observations only and do not qualify
platform delivery.

## Primary sources reviewed

- [XDG RemoteDesktop portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.RemoteDesktop.html)
- [libei sender API](https://libinput.pages.freedesktop.org/libei/api/group__libei-sender.html)
- [ei_text protocol](https://libinput.pages.freedesktop.org/libei/interfaces/ei_text/index.html)
- [ei_device frame protocol](https://libinput.pages.freedesktop.org/libei/interfaces/ei_device/index.html)
- [Reis source](https://github.com/ids1024/reis)

## Alternatives rejected

1. Treat portal permission or an EIS FD as sender readiness. Rejected because
   neither proves a compatible text device, successful frame transport, or
   target retention.
2. Add `reis` 0.7.1 directly. Rejected by the source audit above.
3. Use per-character key events as the first implementation. Rejected because
   it expands keymap/modifier state, event count, latency, and stuck-key risk.
4. Split long or multiline content into several frames. Rejected because
   partial dispatch makes replay, cancellation, and Inbox reconciliation more
   complex and needs a separate transaction design.
5. Promote the controlled bridge directly into default startup. Rejected
   because parent-window ownership and native permission/target behavior remain
   unqualified even after ADR-0026 supplied the wire sender.
