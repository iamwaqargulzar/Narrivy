# ADR-0029 — Desktop portal-parent coordinator remains an inert state machine

**Status:** Accepted for controlled Recovery Phase 2C lifecycle modeling
(2026-08-27)

## Context

ADR-0028 isolates GTK3 Wayland export/unexport but intentionally does not own
Tauri events or authenticated portal IPC. Joining those parts prematurely
would make cleanup claims the current APIs cannot support:

- the permission request blocks on its original IPC connection;
- cancellation is sent on a second authenticated connection and initially
  reports `Closing`, not terminal cleanup;
- an unconditional revoke is still required to close the race where a pending
  request becomes Active while cancellation is in flight;
- the original request must reach a terminal result before the parent export
  can be released;
- Tauri restart ignores normal exit prevention, so ordinary close handling is
  not a complete restart policy.

These are concurrency contracts, not UI details. They should be proven before
a command can open a real portal dialog.

## Decision

- Add a Linux-only `portal_parent` module to the desktop crate containing only
  a pure, generic coordinator. The module is compiled and tested but is not
  constructed by setup, managed as Tauri state, registered in the invoke
  handler, connected to run/window events, or imported by the frontend.
- Opaque operation and ticket types require `Clone` only. They have no
  `Debug`, display, serialization, or string bounds. Coordinator/effect debug
  output shows stable phase/generation data and redacts opaque fields.
- Every operation receives a monotonically changing generation. Ordinary
  request, release, and revoke completions with another generation restore the
  current state unchanged and cannot resurrect authority.
- Native export is a two-step effect: schedule on the main thread, then acquire
  an exclusive `NativeExportAdmission` immediately before export. Its mutable
  borrow prevents cleanup from racing the short synchronous FFI acceptance.
  `begin_with` installs an accepted ticket before returning the callback
  request to an off-main worker; rejection returns to Idle. This API removes a
  stale-acceptance path instead of attempting to overwrite another authority
  state. Dropping an unused admission performs no native call and leaves the
  coordinator fail-closed.
- Timeout, callback cancellation, or invalid handle after native acceptance is
  a separate `export_wait_failed` transition. It preserves stale generations
  unchanged and schedules matching-ticket release; confirmed release is
  required before close/exit replay, while failed release remains locally
  fail-closed.
- Permission dispatch changes from `NotYet` to `Possible` immediately before
  serialization/write. Cleanup while `NotYet` skips remote work and releases
  locally. Cleanup while `Possible` requires both the original permission
  terminal and a cancel-then-unconditional-revoke terminal before release.
- An ambiguous original terminal after dispatch becomes `Cancel` intent even
  when the user did not race it: it always starts cancel-then-revoke. Confirmed
  remote cleanup permits local release; uncertain remote cleanup can never be
  promoted to success.
- Parent release must be acknowledged before sanitized outcome publication or
  close/exit replay. A failed local parent release enters
  `LocalReleaseUnconfirmed`, suppresses replay, and cannot admit new work. An
  uncertain remote cleanup after confirmed local release enters
  `RemoteCleanupUnconfirmed`, publishes only a sanitized failure, and cannot
  admit new work.
- A normal Active result releases the now-unneeded parent before publishing
  `PermissionActive`. If close/exit arrives after Active but before that release
  completes, the transition changes to release, revoke the grant, then replay;
  it never briefly publishes success.
- Cleanup intent is monotonic: `Exit > CloseMain > Cancel > None`. Duplicate
  events merge and never start a second cleanup effect. Granted authority must
  be revoked before close/exit replay even though its parent ticket was already
  released.
- Close/exit replay is an explicit two-step FSM handshake. `take_replay` arms
  exactly one event and `consume_replayed_intent` consumes only that event,
  preventing a replay loop. A stronger Exit racing an armed Close is queued as
  one follow-up replay. After remote-cleanup uncertainty, a confirmed local
  release permits bounded close/exit replay for UX, but consuming Close returns
  to `RemoteCleanupUnconfirmed`; it never reopens admission. Exit ends in
  `Stopped`. A later close/exit may start this replay from the remote-uncertain
  state, while local-release uncertainty never can.
- The future executor must own and join every worker/request handle, use one
  absolute deadline, and implement the Tauri restart prohibition or a stronger
  daemon disconnect cleanup contract. It is not part of this decision.

## Verification boundary

Controlled tests cover cancellation and close before queued native begin,
cancellation before IPC dispatch, both orderings of
original-request and remote-cleanup terminals after possible dispatch,
automatic cleanup after an ambiguous dispatched terminal, normal Active
release-before-publication, Active-versus-close during release, accepted-export
wait failure with typed `ParentExportFailed` after matching release,
dropped-admission fail-closed behavior,
local release failure, remote cleanup uncertainty with fail-closed replay,
duplicate close upgraded to exit, Exit racing an armed Close, granted
exit/revoke, stale request/wait/release/revoke completions, overlap rejection,
stop-from-idle, and canary absence from debug output.

These are state-machine tests only. They create no GTK window, exported handle,
IPC request, portal dialog, EIS connection, target event, evidence, capability,
or release claim.

## Required next decision

Before adding a production executor or UI action, independently review:

1. an owned cancellable request worker with bounded join;
2. cancel followed by unconditional revoke on a separate authenticated
   connection under the same deadline;
3. original-request terminal reconciliation;
4. main-thread release acknowledgement and window-destroy fail-safe;
5. prevent-and-replay close/exit behavior plus an explicit restart policy;
6. dependency policy for the advisory-blocked Tauri/GTK3 runtime.
