# ADR-0023 — Permission-mediated portal/EIS delivery candidate

**Status:** Accepted for the non-interactive Recovery Phase 2C eligibility,
lifecycle, daemon-actor, and non-composed permission-driver foundation
(2026-08-27)

## Context

Recovery Phase 2A proved durable Inbox delivery. Recovery Phase 2B proved that
`wtype -` can accept transcript bytes without proving that COSMIC Edit retained
them; the owner-authorized native observation left the external target empty
and truthfully ended in Inbox recovery. Presence or a zero helper exit is not a
compatibility qualification.

The current COSMIC host advertises XDG RemoteDesktop version 2 with keyboard
devices and `ConnectToEIS`. Upstream specifies EIS as the recommended event
transport. It is permission-mediated and does not require `/dev/uinput` or
input-group membership, but it still represents powerful focused-window input.
libei 1.6 adds an optional `ei_text` interface that can carry one short UTF-8
string directly; a compositor may route that through an input method instead
of emulated keyboard activity. It is capability-dependent and does not make
target retention verifiable.

## Decision

- Portal RemoteDesktop plus EIS is a candidate bounded Linux delivery backend,
  not a qualified primary backend or release capability.
- The daemon will eventually own at most one keyboard-only portal session while
  an explicit grant remains active. Permission setup stays off the dictation
  hot path; a desktop client must initiate it with a parent-window reference
  over authenticated IPC. Revocation, portal disconnect, bounded daemon
  shutdown, or teardown failure must close or invalidate the session
  truthfully.
- The read-only system probe may inspect only portal version and advertised
  device types. The adjacent lifecycle contract defines `Idle -> Requesting ->
  Granted -> Connecting -> Active -> Closing -> Idle/Invalid`, and a bounded
  daemon actor serializes it on one supervised worker. A narrow direct-zbus
  permission driver now implements the approved Create/Select/Start/EIS and
  request/session cleanup boundary, but remains non-composed: no production
  IPC or UI can invoke it. ADR-0026 subsequently adds a minimal text-only
  sender behind the same uncomposed driver.
- `RemoteDesktop v2 + keyboard` means only `ReadyToRequestPermission`. It never
  means permission granted, EIS connected, target compatible, delivery ready,
  or qualified.
- A later EIS submission, if implemented, remains
  `SubmittedUnverified` unless controlled target readback proves retention.
  Cancellation after the first emitted event is delivery-uncertain and must
  retain exactly one durable Inbox recovery.
- `/dev/uinput`, `ydotool`, and input-group access remain opt-in advanced paths
  and are not enabled by this decision. `wtype` remains an unqualified bounded
  compatibility candidate.
- No portal restore token is persisted in this slice. Any persistence must use
  the OS credential abstraction and requires a later security/lifecycle ADR.
- `ashpd` 0.12.0 is exact-pinned for this slice because it supports the workspace
  Rust 1.85 MSRV; 0.13.x requires a newer compiler. `reis` is not added until a
  separate protocol/lifecycle review approves a real EIS sender implementation.
  ADR-0026 records that later review and exact `reis` 0.7.1 pin.
- `ashpd` 0.12.0's high-level `RemoteDesktop` request methods are not approved
  for the production permission driver. They retain the pending request and
  wait for its terminal `Response` before returning, so Narrivy cannot close
  the request when its cancellation/deadline fires. Its public parent-window
  API also cannot reconstruct the desktop client's opaque Wayland parent token
  inside a separate daemon without a live surface/raw window handle.
- The implemented driver boundary is narrow and Linux-adapter-local, using the
  zbus version already pinned by ashpd. It may call
  only `CreateSession`, keyboard-only/nonpersistent `SelectDevices`, `Start`
  with the validated raw parent token, `ConnectToEIS`, `Request.Close`, and
  `Session.Close`; it must subscribe to `Request.Response` before issuing each
  request and to `Session.Closed` before issuing `CreateSession`. It may not call
  any `Notify*` method, request clipboard/screen access, or persist a restore
  token. This narrow decision avoids an ashpd fork while keeping the high-level
  library for the safe read-only property probe.
- Portal request/session tokens use a CSPRNG-backed opaque suffix. Returned
  request or session paths are validated against the predicted path; a mismatch
  is rejected but the returned authoritative path is retained long enough for
  cleanup. Local cancellation before request dispatch creates no false close;
  ambiguity after dispatch requires acknowledged cleanup. A fixed reserve is
  taken from, never added to, the caller's absolute deadline.
- The driver subscribes to `Session.Closed` before `CreateSession` and polls it
  during every blocking request and EIS connection wait. Observed revocation
  immediately removes local authority and never issues a second remote session
  close. The EIS FD is owned but deliberately inaccessible to callers.
  ADR-0025 defines the controlled sender contract; ADR-0026 approves the
  dependency/security implementation while retaining non-composition.
- The daemon-owned lifecycle must use one absolute deadline across grant,
  connection, cancellation, submission admission, and close. Duplicate
  activation is rejected; cancellation after activation closes the session
  before submission admission; revocation is a distinct typed terminal event.
  Failed remote close invalidates the session permanently and immediately
  removes local FD/transport authority without falsely claiming that the portal
  acknowledged `Close`; reuse requires replacing the actor, never reviving the
  invalid owner.
- Actor command admission and shutdown are linearized by one mutex, its queue is
  bounded, and a caller response timeout cannot leave an unobserved active
  grant. Explicit close and portal-originated revocation remain distinct typed
  outcomes.

## Consequences

The product can distinguish a host that is eligible to ask for permission from
one that can actually deliver. No desktop name is used as a feature flag. The
probe is cold-path, bounded, content-free, and Linux-adapter-local. It adds no
input privilege and produces no evidence or requirement status.

The direct-zbus driver is implemented behind the existing actor contract but is
not composed or native-qualified. A later increment may define explicit
grant/revoke IPC and UX only after a controlled portal behavior check.
ADR-0026 approves a constrained use of Reis's low-level codec with a
Narrivy-owned handshake/event loop; the rejected convenience APIs and debug
behavior remain prohibited. ADR-0025 records the first-write, deadline, and
Inbox-recovery contract. The preferred fast path, if the compositor offers it, is one
nonempty `ei_text.utf8` frame within
the protocol's 254-byte limit; unsupported/longer content falls back to Inbox
without chunking or partial replay. Any EIS submission remains
`SubmittedUnverified` until controlled target readback qualifies it.

## Alternatives rejected

1. Continue retrying `wtype` against the microphone path. Rejected because the
   audio/ASR lane is already proven and the native target result was negative.
2. Import `eitype` directly. Rejected for this slice because its current debug
   path formats the text being typed and its lifecycle/deadline behavior has not
   passed Narrivy's privacy and cancellation contracts.
3. Use D-Bus `NotifyKeyboard*` as the production hot path. Rejected because the
   portal recommends EIS after connection and per-event D-Bus round trips are a
   poor latency architecture.
4. Enable `ydotool` or raw uinput. Rejected as a default because it broadens
   privilege beyond the permission-mediated candidate.
5. Use ashpd 0.12 high-level request methods behind a Tokio timeout. Rejected
   because dropping the local future does not give Narrivy ownership of the
   outstanding portal request needed to close its dialog/session truthfully.
6. Maintain an ashpd fork solely to expose pending request handles and a raw
   parent constructor. Rejected for now because the narrow direct-zbus adapter
   has less patch/supply-chain maintenance and can be kept behind one Linux-only
   contract.

## Verification boundary

Controlled tests cover absent interface, old version, missing keyboard, query
failure, timeout, the positive eligibility case, single-owner actor ordering,
bounded queue pressure, atomic admission/shutdown, response-timeout cleanup,
revocation, close failure, immediate local abort, bounded worker join, exact
portal method signatures, keyboard-only/nonpersistent options, CSPRNG token
format/uniqueness, typed cancellation/timeout, authoritative path-mismatch
retention, and revocation preemption during blocking operations. The real
system probe may corroborate advertised properties without opening a portal
request. Concrete D-Bus behavior remains compile/unit-only: no portal dialog,
grant, session, or EIS FD was opened. None of these tests is delivery evidence,
target qualification, or release evidence.

## Primary sources reviewed

- XDG Desktop Portal RemoteDesktop API (version 2, keyboard device selection,
  `ConnectToEIS`, permission/session lifecycle):
  <https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.RemoteDesktop.html>
- libei sender API (explicit device/event/frame dispatch rather than semantic
  text insertion, plus the optional direct UTF-8 text capability):
  <https://libinput.pages.freedesktop.org/libei/api/group__libei-sender.html>
- ei_text protocol (single UTF-8 request, 254-byte limit, input-method
  semantics):
  <https://libinput.pages.freedesktop.org/libei/interfaces/ei_text/index.html>
- Reis source/API review (unstable/incomplete checks and debug behavior):
  <https://github.com/ids1024/reis>
- ashpd 0.12 RemoteDesktop bindings used by the read-only probe:
  <https://docs.rs/ashpd/0.12.0/ashpd/desktop/remote_desktop/struct.RemoteDesktop.html>
- COSMIC compositor report showing a `wtype` regression class relevant to the
  negative Phase 2B observation:
  <https://github.com/pop-os/cosmic-comp/issues/2081>
