# ADR-0069: One bounded daemon shutdown budget

- Status: accepted for recovery; release qualification pending
- Date: 2026-09-14
- Phase: P11 recovery

## Context

The packaged `narrivy-daemon.service` sets `TimeoutStopSec=3s`. The daemon's
stop path did not have one bound that fits inside it:

- `UnixIpcServer::serve` finished connection cleanup with its own fixed
  `CONNECTION_SHUTDOWN_TIMEOUT` (2 s), started after the serve loop had already
  observed the signal.
- `narrivyd` then ran portal cleanup (2 s), supervised-session cleanup (2 s),
  resident ASR cleanup (2 s), a worker retry (500 ms) and a resident retry
  (500 ms), each with a fresh window, and finally joined the signal listener,
  portal pump and event pump without any bound at all.
- `WorkerSupervisor::shutdown_all(timeout)` granted every worker its own full
  `timeout`, so `n` workers could spend `n * timeout`.
- `ResidentWorkerEngine::shutdown(timeout)` acquired its admission lock with a
  blocking `try_lock` loop that returned before closing admission when the
  budget was already gone, and `Drop` re-opened a fresh two-second window after
  an exhausted retirement attempt.

A stop request could therefore exceed `TimeoutStopSec` without any owner
reporting failure; systemd would `SIGKILL` the process and hide the failed
cleanup. Separately, the ordering of the joins mattered: a supervised session
waiting on the resident ASR owner was only cancelled after the session loop had
already spent its own timeout, delaying the resident cancellation that would
have released it.

Independent review of the resident spawn anchor (ADR-0068) also found:

- `reap_retained_until` waited on the retained-registry mutex with a blocking
  acquisition, so one caller could consume another caller's whole reap window
  before observing its own deadline (and never notice it had expired);
- `SpawnAnchor::drop` dropped a still-running owner `JoinHandle` and any
  retained `Child` handles, detaching live work behind the parent-death signal,
  which is a containment net rather than a reap proof;
- retry/Drop paths re-established fresh two-second budgets.

## Decision

Introduce **one monotonic shutdown budget** and make every cleanup owner spend
it:

1. `narrivy-types::ShutdownBudget` holds one total and one start instant that is
   armed at most once (`arc`-shared so a transport adapter and the composing
   service cannot disagree). It exposes `deadline()`, `remaining()`,
   `remaining_at()` and `expired()`, all saturating at zero, plus `arm_at()` so
   regressions can pin the epoch without sleeping through real time.
2. `narrivyd` creates one budget with
   `daemon::shutdown::DAEMON_SHUTDOWN_TOTAL = 2500 ms`, deliberately below the
   packaged `TimeoutStopSec=3s`. `UnixIpcServer::bind_with_shutdown_budget`
   receives the shared instance; `request_shutdown()` arms it, so the
   connection cleanup performed inside `serve` and every later owner share the
   same epoch. `bind` keeps the previous behaviour with a default total.
3. `UnixIpcServer::shutdown_and_join` caps its caller-supplied timeout by the
   shared remaining time; `serve` keeps calling it with the constant, so no
   call site can grant a fresh window.
4. New deadline-based owner APIs replace fresh windows:
   `WorkerSupervisor::shutdown_all_until(deadline)` (joining every worker inside
   one deadline, with `shutdown_all(timeout)` kept as a wrapper),
   `ResidentWorkerEngine::shutdown_until(deadline)` (with `shutdown(timeout)`
   kept as a wrapper), `PortalPermissionService::begin_shutdown(deadline)` /
   `shutdown_until(deadline)` and `PortalSessionActor::begin_shutdown` /
   `shutdown_until`. `WorkerSupervisor::cancel_all()` separates immediate
   cooperative cancellation from the bounded join.
5. `narrivyd`'s stop order is: arm-on-signal (inside the server) → close
   portal admission and cancel the portal actor with the shared deadline →
   `cancel_all()` every supervised worker → resident cleanup → portal join →
   supervised-session join → retries on the same deadline → close the event bus
   and bounded-join both pumps and the signal listener. A spent budget still
   closes every admission and cancels every owner; only the confirmed reap can
   fail, and it is reported through the exit code.
6. Thread joins use `daemon::shutdown::join_thread_bounded`, which joins only a
   provably finished thread and otherwise leaves the handle owned by the
   caller while reporting failure. `TerminationSignalListener::finish_until`
   and its `Drop` follow the same rule (no unbounded join anywhere in the stop
   path).
7. Resident anchor hardening (extends ADR-0068, no contract removal):
   - `reap_retained_until` acquires the retained registry with a deadline-bounded
     `try_lock` loop, makes one final non-blocking observation, and otherwise
     returns the typed `resident_worker_reap_unconfirmed` without mutating
     ownership;
   - `SpawnAnchor::stop_until` records the tightest caller deadline; `Drop`
     spends at most `min(recorded deadline, now + TERMINATION_TIMEOUT)` and never
     opens a fresh full window after a service deadline;
   - a `Drop` that cannot confirm retirement or reap hands every unfinished
     `JoinHandle` and `Child` to the process-wide fallback owner
     (`initial_retiring`), which records a sticky `unconfirmed` count. Only a
     later bounded cleanup that really reaps the children and observes the owner
     threads finished (joining only then) clears it. Nothing is detached or
     conceals failure;
   - `shutdown_until` stores `admission_closed` before acquiring any lock and
     `begin_recovery` re-checks it after taking its permit, so closure is
     permanent even when the lifecycle lock is contended; an exhausted budget
     still signals every owned handle and keeps it in the retiring/retained
     registries.

## Alternatives considered

- Raise `TimeoutStopSec`: rejected by the work order; it hides unbounded or
  cumulative cleanup instead of bounding it.
- Detach cleanup threads and exit success: rejected; it converts unconfirmed
  reaps into false success.
- Give each owner `min(remaining, owner_budget)` computed independently without
  a shared epoch: rejected; the epoch would move with each owner and admission
  closure inside `serve` would remain outside the budget.
- Cancel-only shutdown with no joins: rejected; cleanup ownership and truthful
  failure require proving or reporting the reap.

## Consequences

- Worst-case stop time is bounded by 2500 ms plus signal-observation and
  process-exit overhead, inside the unchanged 3 s unit bound. A stop that cannot
  confirm an owner exits non-zero instead of being killed by systemd.
- `narrivy-types` gains a portable lifecycle primitive, `narrivy-ipc-unix`
  depends on it, and the supervisor/resident/portal owners gain additive
  deadline-based entry points. The previous duration-based signatures remain as
  wrappers, so existing callers and tests compile unchanged.
- An exhausted budget is no longer silent: tests assert that every owner still
  observes cancellation, that IPC admission still closes and unlinks the
  endpoint, and that unconfirmed workers stay tracked and reapable.

## Verification

Shutdown observations share the cleanup deadline too. The daemon calls
`WorkerSupervisor::live_worker_count_until` for final/retry observations; it
does not open the convenience API's new 50 ms convergence window after the
deadline is spent. Pending helper waits recompute remaining time before each
wait, so multiple helpers cannot multiply one stale relative timeout. An
eight-worker blocked native-destructor regression verifies this observation
bound while preserving tracked ownership and later reap.

- `narrivy-types`: the budget arms once, stays monotonic and saturates at zero.
- `narrivy-ipc-unix`: a blocked handler cannot make connection cleanup spend
  past the shared budget; idle restart behaviour and endpoint identity cleanup
  are unchanged.
- `narrivy-core`: several stubborn workers share one deadline (no `n * timeout`
  accumulation), report `TimedOut` truthfully, stay tracked, and `cancel_all`
  signals every worker without waiting.
- `engines/transcribe-cpp` (resident): concurrent `reap_retained_until` returns
  inside its own deadline without losing the child; a forced unconfirmed reap
  keeps the child owned with the same typed failure until a real reap; an
  exhausted retirement hands the owner thread to the fallback owner and a later
  bounded cleanup proves completion before joining; an exhausted shutdown still
  closes admission, refuses recovery with `resident_worker_shutdown`, keeps the
  live worker in an explicit registry, and confirms the reap on a later bounded
  pass.
- `services/narrivyd` (real subprocess): idle SIGTERM/SIGINT/restart cases keep
  passing, and a daemon that owns an actively supervised session stops with
  success, removes its socket and exits inside the packaged budget for both
  TERM and INT. All of these run against the default mock lane; they are not
  native model/platform qualification.
