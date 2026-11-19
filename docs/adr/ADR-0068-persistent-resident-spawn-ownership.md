# ADR-0068: Persistent resident worker spawn ownership

- Status: accepted for recovery; release qualification pending
- Date: 2026-09-14
- Phase: P11 recovery

## Context

Resident worker hardening installs `PR_SET_PDEATHSIG(SIGKILL)` on every
production launch (`install_child_hardening`). Linux binds that relationship to
the thread that calls `clone`/`exec`, not to the daemon process or to whichever
supervisor thread requested startup. A direct host probe confirmed it: a child
that armed the signal before its spawning thread exited was delivered `SIGKILL`
when that thread exited, while the rest of the process stayed alive
(`artifacts/release-011/`).

Offline model provisioning and resident recovery start the worker from bounded
supervisor tasks that exit as soon as they commit readiness. The published
worker was therefore killed immediately after publication: installed model
lifecycle `ready`, live worker lifecycle `restart-required`. The earlier
short-lived-owner regression did not detect this. Its `/proc` presence and
cached readiness checks immediately after the owner thread exited raced the
asynchronous signal delivery (the worker read as running, then died), and it
never required a worker protocol operation. With direct spawning restored it
still passed, so it could not attribute the packaged activation cause.

The first spawn-owner revision also left ownership gaps: the rendezvous send and
reply wait were unbounded (a busy owner could silently exceed the caller's
absolute deadline and ignore cancellation), the owner thread was never joined,
and shutdown could report completion while an owner operation was still running.

A later fallback revision still had two demonstrated gaps. Its courier thread
discarded its own join handle, so the thread that performed an exhausted-drop
hand-off was detached and could not be reaped; and when the OS refused to create
that courier, the drop fell back to an unbounded blocking install after the
deadline was already spent. A pending hand-off counter is not thread ownership,
and preserving ownership does not authorize reopening an unbounded wait.

## Decision

Keep parent-death hardening enabled and make the kernel lifetime relationship
truthful instead of weakening it. Every resident worker process creation now
runs on one persistent per-engine **spawn-owner thread** whose lifetime is the
engine's `SharedWorker`:

- `SpawnAnchor::start` creates the thread once per engine. All initial and
  recovery launches go through `SpawnAnchor::spawn`; no production path creates
  a child on a provisioning or recovery supervisor thread.
- Both the rendezvous hand-off and the reply wait are bounded by the caller's
  absolute deadline and poll cancellation. Failure surfaces as the caller's
  existing narrow typed control outcomes (`resident_worker_startup_timeout`,
  `cancelled`) or `resident_worker_spawn_owner_unavailable` when the owner is
  gone. No blocking call can exceed the deadline.
- A child whose completion races caller cancellation, timeout, or abandonment
  stays owned by the anchor: it terminates the process group and reaps the
  child before taking the next request. If that bounded reap cannot confirm
  termination, the child handle is retained and resolved by a later bounded
  reap (shutdown, teardown, or an explicit retry); it is never silently
  dropped. If no retry succeeds, the owner thread's own exit remains the
  parent-death final net.
- A dropped anchor hands the **shared** abandoned-child registry to the
  process-wide fallback owner together with any still-running owner thread,
  not a snapshot of its current contents. The owner can still publish a
  failed-reap child after the drop; that child stays owned, and the sticky
  unconfirmed count only clears once the writer has retired and a bounded
  cleanup confirmed the reap.
- Contended fallback locks never extend a drop deadline and never make `Drop`
  create a thread. `SpawnAnchor::start` first parks a **retained fallback
  hand-off coordinator** for the engine; if that thread cannot be created,
  startup fails with `resident_worker_handoff_owner_unavailable` before the
  spawn-owner thread or any child exists, so the resource failure is closed
  before child ownership. `Drop` tries the process-wide registry only inside
  its existing bound; when that bound is spent it submits the hand-off to the
  already-owned coordinator through an unbounded queue (which never blocks the
  drop), and the non-deadline-bound coordinator installs it once the lock
  frees. `PENDING_HANDOFFS` keeps a bounded cleanup from reporting quiescence
  while an install is still in flight.
- The coordinator's ownership is explicit for its whole lifetime. It receives
  its own join handle at startup; when its engine drops the only sender it
  publishes that handle to the process-wide fallback owner before exiting, so a
  later bounded cleanup joins the thread instead of detaching it.
  `COORDINATOR_RETIREMENTS` is raised before the sender can be dropped and
  cleared only after the handle is registered, so a concurrent cleanup can
  neither report clean before registration nor ignore the handle; cleanup joins
  each registered coordinator only after its main function returned and keeps
  still-retiring handles owned. The only way a submission fails is a panicked
  coordinator, in which case the caller keeps ownership and installs under the
  lock itself; nothing is detached, dropped, or reported clean while pending.
- An `in_flight` counter covers spawn plus abandoned-child reap. Shutdown waits
  for both a quiescent recovery admission and a quiescent owner, and never
  reports success while a generation or owner operation could still own a live
  process.
- `stop` closes the request channel, waits on a retirement latch, and joins the
  owner thread under a bounded teardown budget. Retirement failures are typed
  (`resident_worker_spawn_owner_still_active` / `..._failed`), not silent.
- `narrivy-process-linux` exposes
  `install_resident_worker_lifetime_hardening` so engine regressions can apply
  the exact production parent-death contract to controlled workers. This is a
  test-facing entry point over the existing private hardening; production
  launches still reach it through `VerifiedArtifactExecutable`.

## Security and privacy

No hardening is relaxed: immutable snapshots, digest/size binding,
descriptor-only model inheritance, scrubbed environment, process-group
containment, descendant cleanup, and `PR_SET_PDEATHSIG` remain mandatory. The
anchor adds no IPC surface, accepts no payload or transcript content, and emits
only stable error codes. Children of an engine still cannot outlive that engine
by more than its bounded teardown; a child can no longer be killed by the exit
of an unrelated bounded supervisor thread.

## Consequences

Each resident engine now keeps two parked threads — the spawn owner and the
fallback hand-off coordinator — instead of creating threads on failure paths.
The spawn owner is joined during teardown within the existing two-second
termination budget; the coordinator retires when the engine drops, registers
its own handle, and is joined by the next bounded cleanup (`with_config`
startup cleanup or an explicit retry) after its completion. Systemd's
three-second stop budget is unchanged. Regressions now pin the contract instead
of racing it:
`worker_spawned_for_a_short_lived_owner_answers_after_that_owner_exits` and
`recovery_replacement_answers_after_its_temporary_recovery_thread_exits`
require a bounded real protocol operation after the requesting thread exits,
`abandoned_spawn_request_is_reaped_by_the_persistent_owner` and
`unconfirmed_abandoned_worker_stays_owned_until_a_later_reap` pin abandoned-child
ownership, `late_failed_child_publication_after_drop_stays_owned_until_reap`
pins the shared-registry fallback hand-off,
`contended_fallback_lock_does_not_extend_the_drop_deadline` pins the bounded
drop hand-off, `failed_handoff_owner_initialization_prevents_child_admission`
pins the closed startup/resource failure,
`dropped_anchor_retains_its_handoff_owner_until_a_bounded_cleanup_reaps_it`
pins the coordinator registration/reap lifecycle,
`anchor_rendezvous_respects_callers_absolute_deadline_and_cancellation`
pins the bounded rendezvous, and
`resident_lifetime_hardening_binds_child_to_the_spawning_thread` pins the kernel
contract next to the hardening primitive. With direct spawning temporarily
restored, both engine regressions fail with the worker process state `Z`.
