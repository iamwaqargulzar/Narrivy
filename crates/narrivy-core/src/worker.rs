//! Worker supervision with explicit cancellation and bounded shutdown (REL-007).
//!
//! Every background worker is owned by the [`WorkerSupervisor`]: it gets a
//! shared [`CancelFlag`], a stable name, and a bounded join on shutdown.
//! Shutdown outcomes are truthful: [`WorkerStop::Stopped`],
//! [`WorkerStop::TimedOut`] (worker continues running and remains tracked for
//! later reaping), or a
//! typed failure for panics — never a re-panic, never an unbounded block.
//!
//! The native `JoinHandle::join` is uninterruptible, so it never runs on a
//! deadline-bound caller. Once a worker's completion is observed, the original
//! handle is handed to an owned join helper thread that publishes confirmation
//! only after that `join` returned; callers wait for the confirmation inside
//! the remaining absolute budget and keep the helper, the original handle and
//! the consumed typed outcome for a later bounded reap when the budget expires.
//! `JoinHandle::is_finished` is never used as a completion proof.
//!
//! On supervisor `Drop` no join is attempted (a drop cannot block or report
//! failure): a worker whose handle is already owned by a join helper stays
//! owned by that helper until the original thread is joined, while an entry
//! that still holds a raw handle is detached — never reported as successfully
//! reaped.

use crate::{CoreError, unix_now_ms};
use narrivy_types::SessionCancellation;
use std::collections::BTreeMap;
#[cfg(test)]
use std::sync::TryLockError;
#[cfg(test)]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError, channel};
#[cfg(test)]
use std::sync::mpsc::{SyncSender, sync_channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Cooperative cancellation shared between supervisor and worker.
pub type CancelFlag = SessionCancellation;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerId(pub u64);

/// Truthful outcome of a bounded stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerStop {
    /// Observed cancellation and finished within the budget.
    Stopped,
    /// Did not finish within the budget; still tracked, reapable later.
    TimedOut,
}

/// A unit of supervised work: receives the cancel flag and must poll it
/// between work units.
pub type WorkerJob = Box<dyn FnOnce(CancelFlag) + Send + 'static>;

const MAX_RECORDED_PANICS: usize = 16;

/// How long [`WorkerSupervisor::live_worker_count`] waits for join helpers that
/// already own a consumed completion to publish their confirmation. The wait is
/// event-driven and normally returns as soon as the helper is scheduled; a
/// worker whose native thread epilogue really is still running stays tracked and
/// is reported truthfully once this window expires.
const REAP_CONFIRMATION_WINDOW: Duration = Duration::from_millis(50);

#[derive(Clone)]
enum ThreadOutcome {
    Done,
    Panicked(String),
    /// The worker wrapper's completion sender was dropped without this caller
    /// observing a value. A later bounded reap observes this after an earlier
    /// attempt consumed the completion and its deadline expired while the OS
    /// thread was still running its native epilogue.
    Disconnected,
}

struct WorkerEntry {
    name: String,
    flag: CancelFlag,
    done_rx: Option<Receiver<ThreadOutcome>>,
    /// Completion already observed by a bounded reap that had to stop before
    /// the native join could be confirmed. Ownership of this typed outcome
    /// moves into the reap operation's join handoff; it only returns here when
    /// helper creation failed.
    completed: Option<ThreadOutcome>,
    handle: Option<std::thread::JoinHandle<()>>,
    reap: Option<Arc<ReapOperation>>,
}

/// Native ownership whose registry entry disappeared. Never detached: the next
/// supervisor observation delegates it to an owned join helper like any other
/// reap.
struct OrphanedWorker {
    id: u64,
    completed: Option<ThreadOutcome>,
    handle: Option<std::thread::JoinHandle<()>>,
    operation: Option<Arc<ReapOperation>>,
}

#[derive(Clone)]
enum ReapResolution {
    /// The claim was restored before any native join was handed to a helper;
    /// a later claim may retry inside its own remaining budget.
    Released,
    /// The worker's original native handle was really joined.
    Finished(WorkerStop),
    Panicked(String),
    /// The native join could not even be handed to an owned helper. Ownership
    /// was restored; coalesced waiters get this typed failure instead of
    /// spinning on immediate retries.
    Unavailable(String),
}

/// Shared state of one bounded reap attempt. It also owns the join helper that
/// performs the uninterruptible native `join` off every deadline-bound caller.
#[derive(Default)]
struct ReapOperation {
    /// Set only once the original native thread has really been joined, or when
    /// a claim was released before a helper existed.
    resolution: Mutex<Option<ReapResolution>>,
    changed: Condvar,
    /// Ownership held for the join helper: the completion already observed
    /// from the worker wrapper plus the helper's own handle. It stays here when
    /// a caller's deadline expires, so a later bounded reap observes the same
    /// confirmation and the consumed typed outcome.
    handoff: Mutex<JoinHandoff>,
}

#[derive(Default)]
struct JoinHandoff {
    completed: Option<ThreadOutcome>,
    joiner: Option<std::thread::JoinHandle<()>>,
}

/// Ownership returned when the join helper could not be created. Nothing is
/// dropped: the caller restores all of it under the worker entry.
struct JoinHandoffFailure {
    handle: std::thread::JoinHandle<()>,
    completed: Option<ThreadOutcome>,
    error: CoreError,
}

impl ReapOperation {
    fn wait(&self, timeout: Duration) -> Option<ReapResolution> {
        let resolution = self.resolution.lock().unwrap_or_else(|p| p.into_inner());
        let (resolution, _) = self
            .changed
            .wait_timeout_while(resolution, timeout, |resolution| resolution.is_none())
            .unwrap_or_else(|p| p.into_inner());
        resolution.clone()
    }

    /// Non-blocking view for opportunistic cleanup and coalesced observers.
    fn peek(&self) -> Option<ReapResolution> {
        self.resolution
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    fn resolve(&self, resolution: ReapResolution) {
        let mut state = self.resolution.lock().unwrap_or_else(|p| p.into_inner());
        if state.is_none() {
            *state = Some(resolution);
            drop(state);
            self.changed.notify_all();
        }
    }

    /// Take the completion already consumed from the worker wrapper. Only the
    /// join helper calls this, after the helper install released the handoff
    /// lock.
    fn take_completed(&self) -> Option<ThreadOutcome> {
        self.handoff
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .completed
            .take()
    }

    /// Take the join helper once its confirmation is published, so the
    /// confirming caller can prove its retirement. The helper's closure has
    /// already returned from the original `join`, so only its own thread
    /// teardown remains; this is never a wait on native worker completion.
    fn take_retired_joiner(&self) -> Option<std::thread::JoinHandle<()>> {
        if self
            .resolution
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_none()
        {
            return None;
        }
        self.handoff
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .joiner
            .take()
    }
}

enum ReapClaim {
    Owner {
        rx: Option<Receiver<ThreadOutcome>>,
        completed: Option<ThreadOutcome>,
        handle: std::thread::JoinHandle<()>,
        operation: Arc<ReapOperation>,
    },
    Waiter(Arc<ReapOperation>),
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum SupervisorLifecycle {
    #[default]
    Accepting,
    ShuttingDown,
}

#[cfg(test)]
struct LifecyclePause {
    reached: SyncSender<()>,
    resume: Receiver<()>,
}

#[cfg(test)]
struct TestPause {
    reached: Receiver<()>,
    resume: SyncSender<()>,
}

#[cfg(test)]
impl TestPause {
    fn wait_until_reached(&self) {
        self.reached
            .recv_timeout(Duration::from_secs(1))
            .unwrap_or_else(|err| test_failure(format!("lifecycle pause was not reached: {err}")));
    }

    fn release(&self) {
        self.resume
            .send(())
            .unwrap_or_else(|_| test_failure("lifecycle pause receiver unexpectedly dropped"));
    }
}

#[cfg(test)]
fn test_failure(message: impl Into<String>) -> ! {
    std::panic::resume_unwind(Box::new(message.into()))
}

#[cfg(test)]
fn resume_thread_panic<T>(payload: Box<dyn std::any::Any + Send>) -> T {
    std::panic::resume_unwind(payload)
}

/// Owns named background workers with cooperative cancellation and bounded,
/// truthful shutdown semantics (REL-007).
#[derive(Default)]
pub struct WorkerSupervisor {
    next_id: AtomicU64,
    /// Serializes worker admission with the transition into shutdown. A
    /// worker becomes visible in `workers` before admission releases this
    /// boundary; after shutdown closes it, no further worker is admitted.
    lifecycle: Mutex<SupervisorLifecycle>,
    workers: Mutex<BTreeMap<u64, WorkerEntry>>,
    /// Panic outcomes outlive the completed worker entry long enough for a
    /// caller holding its handle to observe the typed failure. The next
    /// supervisor observation or admission joins and removes normal
    /// completions.
    panics: Mutex<BTreeMap<u64, String>>,
    /// Threads whose registry entry disappeared before a bounded reap could
    /// restore ownership of them. They stay owned (never detached) and the
    /// next supervisor observation hands each one to an owned join helper like
    /// any other reap. No current interleaving produces one; the parking
    /// exists so a future registry change cannot turn a lost entry into a
    /// detached live thread or a silently dropped typed outcome.
    orphaned: Mutex<Vec<OrphanedWorker>>,
    #[cfg(test)]
    fail_next_thread_spawn: AtomicBool,
    /// Test-only switch that forces the next native-join helper creation to
    /// fail, so the ownership-preserving fallback is observable.
    #[cfg(test)]
    fail_next_native_join_helper: AtomicBool,
    #[cfg(test)]
    admission_pause: Mutex<Option<LifecyclePause>>,
    #[cfg(test)]
    shutdown_pause: Mutex<Option<LifecyclePause>>,
    #[cfg(test)]
    timeout_restore_pause: Mutex<Option<LifecyclePause>>,
    #[cfg(test)]
    after_take_parts_pause: Mutex<Option<LifecyclePause>>,
    #[cfg(test)]
    panic_published_pause: Mutex<Option<LifecyclePause>>,
    #[cfg(test)]
    coalesced_waiter_observer: Mutex<Option<SyncSender<()>>>,
    #[cfg(test)]
    shutdown_contention_observer: Mutex<Option<SyncSender<()>>>,
    #[cfg(test)]
    completed_joins: AtomicU64,
}

/// How a restored (unperformed) claim wakes coalesced waiters.
enum ReapWake {
    /// Ownership is back under the entry and a later claim may retry.
    Released,
    /// The native join could not be handed to a helper; waiters get the typed
    /// failure instead of spinning on immediate retries.
    Unavailable(String),
}

impl ReapWake {
    fn into_resolution(self) -> ReapResolution {
        match self {
            Self::Released => ReapResolution::Released,
            Self::Unavailable(message) => ReapResolution::Unavailable(message),
        }
    }
}

impl WorkerSupervisor {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Spawn a named worker. Panics inside jobs are contained via
    /// `catch_unwind` and surfaced as `CoreError::Worker` at stop/reap time.
    pub fn spawn(
        self: &Arc<Self>,
        name: impl Into<String>,
        job: impl FnOnce(CancelFlag) + Send + 'static,
    ) -> Result<WorkerHandle, CoreError> {
        self.spawn_with_cancellation(name, CancelFlag::new(), job)
    }

    /// Spawn a worker using the supplied owned cancellation signal. Recovery
    /// sessions pass their active-slot signal here, so user cancellation and
    /// supervisor shutdown are one mechanism rather than bridged flags.
    pub fn spawn_with_cancellation(
        self: &Arc<Self>,
        name: impl Into<String>,
        flag: CancelFlag,
        job: impl FnOnce(CancelFlag) + Send + 'static,
    ) -> Result<WorkerHandle, CoreError> {
        // Join workers that finished between supervisor operations. A worker
        // never removes or drops its own `JoinHandle`.
        self.reap_finished_workers();

        #[cfg(test)]
        if self.fail_next_thread_spawn.swap(false, Ordering::AcqRel) {
            return Err(CoreError::Worker(
                "controlled thread-spawn failure for test".to_owned(),
            ));
        }

        // Hold admission from the lifecycle check through OS-thread creation,
        // registry insertion, and release of the child start gate. Shutdown
        // uses this same boundary before taking its worker snapshot.
        let lifecycle = self.lifecycle.lock().unwrap_or_else(|p| p.into_inner());
        if *lifecycle == SupervisorLifecycle::ShuttingDown {
            return Err(CoreError::Worker(
                "worker admission rejected: supervisor is shutting down".to_owned(),
            ));
        }

        let id = WorkerId(self.next_id.fetch_add(1, Ordering::SeqCst));
        let tname = name.into();
        let (done_tx, done_rx) = channel::<ThreadOutcome>();
        let (start_tx, start_rx) = channel::<()>();
        let thread_flag = flag.clone();
        let owner = Arc::downgrade(self);
        let span_name = format!("narrivy-w-{tname}");
        let handle = std::thread::Builder::new()
            .name(span_name)
            .spawn(move || {
                // The worker is registered before this signal is sent. This
                // keeps both completion reaping and immediate shutdown from
                // observing an unregistered running thread.
                if start_rx.recv().is_err() {
                    return;
                }

                let outcome = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    job(thread_flag)
                })) {
                    Ok(()) => ThreadOutcome::Done,
                    Err(payload) => ThreadOutcome::Panicked(panic_message(&payload)),
                };
                if let Some(supervisor) = owner.upgrade() {
                    if let ThreadOutcome::Panicked(ref message) = outcome {
                        supervisor.record_panic(id.0, message.clone());
                    }
                    // Receiver may be gone (supervisor dropped); sending is best-effort.
                    let _ = done_tx.send(outcome);
                } else {
                    let _ = done_tx.send(outcome);
                }
            })
            .map_err(|e| CoreError::Worker(format!("spawn {tname}: {e}")))?;
        #[cfg(test)]
        self.wait_at_admission_pause_for_test();
        self.workers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(
                id.0,
                WorkerEntry {
                    name: tname,
                    flag,
                    done_rx: Some(done_rx),
                    completed: None,
                    handle: Some(handle),
                    reap: None,
                },
            );
        if start_tx.send(()).is_err() {
            let entry = self
                .workers
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&id.0);
            if let Some(mut entry) = entry {
                if let Some(handle) = entry.handle.take() {
                    // The start gate never opened, so no worker code ran. The
                    // native handle is parked for the owned join helper instead
                    // of being joined on this caller.
                    let completed = entry.completed.take();
                    self.park_orphaned(id.0, completed, handle);
                }
            }
            return Err(CoreError::Worker(format!(
                "worker {} exited before registration completed",
                id.0
            )));
        }
        drop(lifecycle);
        Ok(WorkerHandle {
            id,
            supervisor: Arc::clone(self),
        })
    }

    fn claim_reap(&self, id: u64) -> Result<ReapClaim, CoreError> {
        let mut workers = self.workers.lock().unwrap_or_else(|p| p.into_inner());
        let entry = workers
            .get_mut(&id)
            .ok_or_else(|| CoreError::Worker(format!("unknown worker {id}")))?;
        if let Some(operation) = &entry.reap {
            return Ok(ReapClaim::Waiter(Arc::clone(operation)));
        }
        let rx = entry.done_rx.take();
        let completed = entry.completed.take();
        // The claimed parts stay owned by this operation. A caller that must
        // stop at its deadline restores them instead of dropping the handle,
        // so no path can detach the thread.
        let Some(handle) = entry.handle.take() else {
            entry.done_rx = rx;
            entry.completed = completed;
            return Err(CoreError::Worker(format!(
                "worker {id} has no thread handle"
            )));
        };
        if rx.is_none() && completed.is_none() {
            entry.handle = Some(handle);
            return Err(CoreError::Worker(format!(
                "worker {id} has no completion receiver"
            )));
        }
        let operation = Arc::new(ReapOperation::default());
        entry.reap = Some(Arc::clone(&operation));
        Ok(ReapClaim::Owner {
            rx,
            completed,
            handle,
            operation,
        })
    }

    /// Return a claimed receiver, an already observed completion and the thread
    /// handle in one registry transaction, then release the reap operation so a
    /// later bounded attempt can retry.
    ///
    /// Restoring only part of the claimed ownership would let a concurrent reap
    /// consume an internally inconsistent entry, and dropping the handle would
    /// detach a live thread from every future reap.
    fn restore_parts(
        &self,
        id: u64,
        rx: Option<Receiver<ThreadOutcome>>,
        completed: Option<ThreadOutcome>,
        handle: std::thread::JoinHandle<()>,
        operation: &Arc<ReapOperation>,
        wake: ReapWake,
    ) {
        let resolution = wake.into_resolution();
        let mut parts = Some((rx, completed, handle));
        let restored = if let Some(entry) = self
            .workers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get_mut(&id)
        {
            if let Some((rx, completed, handle)) = parts.take() {
                entry.done_rx = rx;
                entry.completed = completed;
                entry.handle = Some(handle);
            }
            #[cfg(test)]
            self.wait_at_timeout_restore_pause_for_test();
            if entry
                .reap
                .as_ref()
                .is_some_and(|active| Arc::ptr_eq(active, operation))
            {
                entry.reap = None;
            }
            true
        } else {
            false
        };
        debug_assert!(parts.is_none() == restored);
        if let Some((_, completed, handle)) = parts {
            // Preserve ownership instead of detaching: see `orphaned`.
            self.park_orphaned(id, completed, handle);
        }
        operation.resolve(resolution);
    }

    fn park_orphaned(
        &self,
        id: u64,
        completed: Option<ThreadOutcome>,
        handle: std::thread::JoinHandle<()>,
    ) {
        if let Some(ThreadOutcome::Panicked(message)) = &completed {
            self.record_panic(id, message.clone());
        }
        self.orphaned
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(OrphanedWorker {
                id,
                completed,
                handle: Some(handle),
                operation: None,
            });
    }

    /// Retire parked native ownership. Each handle is delegated to the same
    /// owned join helper as a tracked worker: an original `join` never runs on
    /// this caller, and only a published confirmation releases a handle.
    fn reap_parked_orphans(&self) {
        let parked: Vec<OrphanedWorker> = {
            let mut slot = self.orphaned.lock().unwrap_or_else(|p| p.into_inner());
            std::mem::take(&mut *slot)
        };
        let mut still_owned = Vec::with_capacity(parked.len());
        for mut orphan in parked {
            if orphan.operation.is_none() {
                if let Some(handle) = orphan.handle.take() {
                    let operation = Arc::new(ReapOperation::default());
                    let completed = orphan.completed.take();
                    match self.delegate_native_join(orphan.id, &operation, completed, handle) {
                        Ok(()) => orphan.operation = Some(operation),
                        Err(failure) => {
                            orphan.handle = Some(failure.handle);
                            orphan.completed = failure.completed;
                        }
                    }
                }
            }
            if let Some(operation) = orphan.operation.as_ref() {
                if let Some(resolution) = operation.peek() {
                    if let ReapResolution::Panicked(message) = resolution {
                        self.record_panic(orphan.id, message);
                    }
                    self.confirm_helper_retirement(operation);
                    continue;
                }
            }
            still_owned.push(orphan);
        }
        if !still_owned.is_empty() {
            self.orphaned
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .extend(still_owned);
        }
    }

    /// Turn `ThreadOutcome::Disconnected` into the sticky typed panic this
    /// supervisor already recorded for the worker, if there is one. The worker
    /// wrapper records a panic before it drops its completion sender, so this
    /// observation is authoritative at receipt time.
    fn normalize_completion(&self, id: u64, outcome: ThreadOutcome) -> ThreadOutcome {
        match outcome {
            ThreadOutcome::Disconnected => match self.peek_panic(id) {
                Some(message) => ThreadOutcome::Panicked(message),
                None => ThreadOutcome::Disconnected,
            },
            outcome => outcome,
        }
    }

    /// Hand the original native handle to an owned join helper. The helper
    /// publishes confirmation only after `JoinHandle::join` on the original
    /// handle returned, so a published resolution proves native completion. On
    /// helper-creation failure every piece of ownership is returned, never
    /// dropped.
    fn delegate_native_join(
        &self,
        id: u64,
        operation: &Arc<ReapOperation>,
        completed: Option<ThreadOutcome>,
        handle: std::thread::JoinHandle<()>,
    ) -> Result<(), JoinHandoffFailure> {
        #[cfg(test)]
        if self
            .fail_next_native_join_helper
            .swap(false, Ordering::AcqRel)
        {
            return Err(JoinHandoffFailure {
                handle,
                completed,
                error: CoreError::Worker(format!(
                    "worker {id}: native join helper could not be started (controlled test failure)"
                )),
            });
        }
        // The original handle travels through a rendezvous channel so a failed
        // `Builder::spawn` (which drops the closure and anything captured
        // directly) cannot drop it.
        let (origin_tx, origin_rx) = channel::<std::thread::JoinHandle<()>>();
        let helper_operation = Arc::clone(operation);
        let mut handoff = operation.handoff.lock().unwrap_or_else(|p| p.into_inner());
        let spawned = std::thread::Builder::new()
            .name(format!("narrivy-reap-{id}"))
            .spawn(move || {
                // The handoff lock is held until the completion and the helper
                // handle are installed, so this cannot observe a
                // half-installed ownership. The original handle can only be
                // missing after spawn failure, in which case this closure
                // never runs.
                let completed = helper_operation.take_completed();
                let Ok(original) = origin_rx.recv() else {
                    return;
                };
                let resolution = native_join_resolution(completed, original.join());
                helper_operation.resolve(resolution);
            });
        match spawned {
            Ok(joiner) => {
                if let Err(std::sync::mpsc::SendError(handle)) = origin_tx.send(handle) {
                    drop(handoff);
                    return Err(JoinHandoffFailure {
                        handle,
                        completed,
                        error: CoreError::Worker(format!(
                            "worker {id}: native join helper disappeared before accepting its handle"
                        )),
                    });
                }
                handoff.completed = completed;
                handoff.joiner = Some(joiner);
                Ok(())
            }
            Err(err) => {
                drop(handoff);
                Err(JoinHandoffFailure {
                    handle,
                    completed,
                    error: CoreError::Worker(format!(
                        "worker {id}: native join helper could not be started: {err}"
                    )),
                })
            }
        }
    }

    /// Publish the sticky typed failure and confirm the helper's retirement,
    /// without removing the entry. Coalesced observers use this: the entry is
    /// removed once by the owner or by the next opportunistic observation,
    /// after the outcome has been published for everyone else.
    fn report_reap(
        &self,
        id: u64,
        operation: &Arc<ReapOperation>,
        resolution: &ReapResolution,
    ) -> Result<WorkerStop, CoreError> {
        match resolution {
            ReapResolution::Released | ReapResolution::Unavailable(_) => Ok(WorkerStop::TimedOut),
            ReapResolution::Finished(stop) => {
                self.confirm_helper_retirement(operation);
                Ok(*stop)
            }
            ReapResolution::Panicked(message) => {
                self.record_panic(id, message.clone());
                self.confirm_helper_retirement(operation);
                Err(CoreError::Worker(format!("worker panicked: {message}")))
            }
        }
    }

    /// Terminal observation that also retires the worker entry. Every removal
    /// happens after the typed outcome was published (and, in tests, after the
    /// publication pause), so coalesced and late reapers keep the same sticky
    /// failure.
    fn complete_reap(
        &self,
        id: u64,
        operation: &Arc<ReapOperation>,
        resolution: ReapResolution,
    ) -> Result<WorkerStop, CoreError> {
        let outcome = self.report_reap(id, operation, &resolution);
        match resolution {
            ReapResolution::Finished(_) | ReapResolution::Panicked(_) => {
                #[cfg(test)]
                if matches!(resolution, ReapResolution::Panicked(_)) {
                    self.wait_after_panic_published_for_test();
                }
                self.remove_worker(id);
            }
            ReapResolution::Released | ReapResolution::Unavailable(_) => {}
        }
        outcome
    }

    /// Join the owned helper after its confirmation is published: the helper
    /// has already returned from the original `join`, so only its own thread
    /// teardown remains. The helper handle is kept owned until this point.
    fn confirm_helper_retirement(&self, operation: &Arc<ReapOperation>) {
        if let Some(joiner) = operation.take_retired_joiner() {
            let _ = joiner.join();
            #[cfg(test)]
            self.completed_joins.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Bounded wait for one worker to finish after its flag was cancelled.
    /// Finished workers leave the supervisor; timed-out workers stay tracked
    /// for a later bounded [`WorkerSupervisor::reap`].
    pub(crate) fn join_one(&self, id: u64, timeout: Duration) -> Result<WorkerStop, CoreError> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .unwrap_or_else(Instant::now);
        self.join_one_until(id, deadline)
    }

    /// Bounded wait for one worker that never extends `deadline`.
    pub(crate) fn join_one_until(
        &self,
        id: u64,
        deadline: Instant,
    ) -> Result<WorkerStop, CoreError> {
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .unwrap_or(Duration::ZERO);
            match self.claim_reap(id) {
                Ok(ReapClaim::Waiter(operation)) => {
                    #[cfg(test)]
                    self.observe_coalesced_waiter_for_test();
                    let Some(resolution) = operation.wait(remaining) else {
                        return Ok(WorkerStop::TimedOut);
                    };
                    match resolution {
                        ReapResolution::Released => continue,
                        ReapResolution::Unavailable(message) => {
                            return Err(CoreError::Worker(message));
                        }
                        // A coalesced waiter reports the published outcome
                        // without retiring the entry: the owner (or the next
                        // opportunistic observation) removes it after the
                        // typed outcome is visible to everyone.
                        resolution => return self.report_reap(id, &operation, &resolution),
                    }
                }
                Ok(ReapClaim::Owner {
                    rx,
                    completed,
                    handle,
                    operation,
                }) => {
                    #[cfg(test)]
                    self.wait_after_take_parts_for_test();
                    let outcome = match completed {
                        Some(completed) => completed,
                        None => {
                            let Some(rx) = rx else {
                                // A claim without receiver, completion or
                                // handle is rejected by `claim_reap`; keep the
                                // handle owned rather than dropping it.
                                self.restore_parts(
                                    id,
                                    None,
                                    None,
                                    handle,
                                    &operation,
                                    ReapWake::Released,
                                );
                                return Err(CoreError::Worker(format!(
                                    "worker {id} lost its completion receiver"
                                )));
                            };
                            let owner_remaining = deadline
                                .checked_duration_since(Instant::now())
                                .unwrap_or(Duration::ZERO);
                            match rx.recv_timeout(owner_remaining) {
                                Ok(outcome) => outcome,
                                Err(RecvTimeoutError::Timeout) => {
                                    // The closure may still be running. Keep
                                    // receiver+handle parked for a later
                                    // bounded reap; report truthfully and let
                                    // longer-budget coalesced callers retry.
                                    self.restore_parts(
                                        id,
                                        Some(rx),
                                        None,
                                        handle,
                                        &operation,
                                        ReapWake::Released,
                                    );
                                    return Ok(WorkerStop::TimedOut);
                                }
                                Err(RecvTimeoutError::Disconnected) => ThreadOutcome::Disconnected,
                            }
                        }
                    };
                    let outcome = self.normalize_completion(id, outcome);
                    // A completion receipt — or its sender disappearing — does
                    // not prove the OS thread left its native epilogue:
                    // thread-local destructors can still be running, and
                    // `JoinHandle::join` is uninterruptible. Move that
                    // operation off this deadline-bound caller by handing the
                    // original handle to an explicitly owned helper thread.
                    if let Err(failure) =
                        self.delegate_native_join(id, &operation, Some(outcome), handle)
                    {
                        self.restore_parts(
                            id,
                            None,
                            failure.completed,
                            failure.handle,
                            &operation,
                            ReapWake::Unavailable(failure.error.to_string()),
                        );
                        return Err(failure.error);
                    }
                    // Wait only for the helper's confirmed join of the original
                    // handle, and only for the remaining absolute budget.
                    let remaining = deadline
                        .checked_duration_since(Instant::now())
                        .unwrap_or(Duration::ZERO);
                    return match operation.wait(remaining) {
                        Some(resolution) => self.complete_reap(id, &operation, resolution),
                        // The entry keeps the helper, the original handle it
                        // still owns and the consumed typed outcome.
                        None => Ok(WorkerStop::TimedOut),
                    };
                }
                // A prior supervisor observation may already have joined and
                // removed this completed worker.
                Err(err) if err.to_string().contains("unknown worker") => {
                    if let Some(message) = self.take_panic(id) {
                        return Err(CoreError::Worker(format!("worker panicked: {message}")));
                    }
                    return Ok(WorkerStop::Stopped);
                }
                Err(err) => return Err(err),
            }
        }
    }

    fn remove_worker(&self, id: u64) {
        self.workers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&id);
    }

    /// Opportunistic cleanup: observe completions without blocking and hand
    /// native joins to owned helpers. Nothing on this path ever joins an
    /// original `JoinHandle` on this caller, whatever `is_finished` reports.
    fn reap_finished_workers(&self) {
        let ids: Vec<u64> = self
            .workers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .keys()
            .copied()
            .collect();
        for id in ids {
            self.observe_worker_reap(id);
        }
        self.reap_parked_orphans();
    }

    /// One non-blocking observation of a single worker entry.
    fn observe_worker_reap(&self, id: u64) {
        enum Observation {
            Confirmed(Arc<ReapOperation>, ReapResolution),
            Delegate(
                Arc<ReapOperation>,
                Option<ThreadOutcome>,
                std::thread::JoinHandle<()>,
            ),
        }
        let observation = {
            let mut workers = self.workers.lock().unwrap_or_else(|p| p.into_inner());
            let Some(entry) = workers.get_mut(&id) else {
                return;
            };
            if let Some(operation) = entry.reap.as_ref() {
                // A reap is in flight. Its helper owns the native join; this
                // observation only records a confirmation that is already
                // published.
                match operation.peek() {
                    Some(resolution) => Observation::Confirmed(Arc::clone(operation), resolution),
                    None => return,
                }
            } else {
                // No reap in flight: consume whatever completion is available
                // without waiting, then delegate the native join.
                let outcome = match entry.completed.take() {
                    Some(completed) => completed,
                    None => {
                        let Some(rx) = entry.done_rx.take() else {
                            return;
                        };
                        match rx.try_recv() {
                            Ok(outcome) => outcome,
                            Err(TryRecvError::Empty) => {
                                entry.done_rx = Some(rx);
                                return;
                            }
                            Err(TryRecvError::Disconnected) => ThreadOutcome::Disconnected,
                        }
                    }
                };
                let Some(handle) = entry.handle.take() else {
                    entry.completed = Some(outcome);
                    return;
                };
                let operation = Arc::new(ReapOperation::default());
                entry.reap = Some(Arc::clone(&operation));
                Observation::Delegate(operation, Some(outcome), handle)
            }
        };
        match observation {
            Observation::Confirmed(operation, resolution) => {
                let _ = self.complete_reap(id, &operation, resolution);
            }
            Observation::Delegate(operation, completed, handle) => {
                let completed = completed.map(|outcome| self.normalize_completion(id, outcome));
                if let Err(failure) = self.delegate_native_join(id, &operation, completed, handle) {
                    self.restore_parts(
                        id,
                        None,
                        failure.completed,
                        failure.handle,
                        &operation,
                        ReapWake::Unavailable(failure.error.to_string()),
                    );
                }
            }
        }
    }

    fn record_panic(&self, id: u64, message: String) {
        let mut panics = self.panics.lock().unwrap_or_else(|p| p.into_inner());
        panics.insert(id, message);
        while panics.len() > MAX_RECORDED_PANICS {
            let Some(oldest_id) = panics.keys().next().copied() else {
                break;
            };
            panics.remove(&oldest_id);
        }
    }

    #[cfg(test)]
    fn fail_next_thread_spawn_for_test(&self) {
        self.fail_next_thread_spawn.store(true, Ordering::Release);
    }

    #[cfg(test)]
    fn fail_next_native_join_helper_for_test(&self) {
        self.fail_next_native_join_helper
            .store(true, Ordering::Release);
    }

    #[cfg(test)]
    fn pause_next_admission_for_test(&self) -> TestPause {
        self.install_lifecycle_pause_for_test(&self.admission_pause)
    }

    #[cfg(test)]
    fn pause_next_shutdown_for_test(&self) -> TestPause {
        self.install_lifecycle_pause_for_test(&self.shutdown_pause)
    }

    #[cfg(test)]
    fn pause_next_timeout_restore_for_test(&self) -> TestPause {
        self.install_lifecycle_pause_for_test(&self.timeout_restore_pause)
    }

    #[cfg(test)]
    fn pause_next_after_take_parts_for_test(&self) -> TestPause {
        self.install_lifecycle_pause_for_test(&self.after_take_parts_pause)
    }

    #[cfg(test)]
    fn pause_next_after_panic_published_for_test(&self) -> TestPause {
        self.install_lifecycle_pause_for_test(&self.panic_published_pause)
    }

    #[cfg(test)]
    fn observe_next_coalesced_waiter_for_test(&self) -> Receiver<()> {
        let (observed_tx, observed_rx) = sync_channel(0);
        let mut observer = self
            .coalesced_waiter_observer
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        assert!(
            observer.is_none(),
            "a coalesced-waiter observer is already installed"
        );
        *observer = Some(observed_tx);
        observed_rx
    }

    #[cfg(test)]
    fn observe_coalesced_waiter_for_test(&self) {
        let observer = self
            .coalesced_waiter_observer
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        if let Some(observer) = observer {
            let _ = observer.send(());
        }
    }

    #[cfg(test)]
    fn observe_next_shutdown_lifecycle_contention_for_test(&self) -> Receiver<()> {
        let (observed_tx, observed_rx) = sync_channel(0);
        let mut observer = self
            .shutdown_contention_observer
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        assert!(
            observer.is_none(),
            "a shutdown lifecycle-contention observer is already installed"
        );
        *observer = Some(observed_tx);
        observed_rx
    }

    #[cfg(test)]
    fn observe_shutdown_lifecycle_contention_for_test(&self) {
        let observer = self
            .shutdown_contention_observer
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        if let Some(observer) = observer {
            if matches!(self.lifecycle.try_lock(), Err(TryLockError::WouldBlock)) {
                let _ = observer.send(());
            }
        }
    }

    #[cfg(test)]
    fn install_lifecycle_pause_for_test(&self, slot: &Mutex<Option<LifecyclePause>>) -> TestPause {
        let (reached_tx, reached_rx) = sync_channel(0);
        let (resume_tx, resume_rx) = sync_channel(0);
        let mut guard = slot.lock().unwrap_or_else(|p| p.into_inner());
        assert!(
            guard.is_none(),
            "a lifecycle test pause is already installed"
        );
        *guard = Some(LifecyclePause {
            reached: reached_tx,
            resume: resume_rx,
        });
        TestPause {
            reached: reached_rx,
            resume: resume_tx,
        }
    }

    #[cfg(test)]
    fn wait_at_admission_pause_for_test(&self) {
        self.wait_at_lifecycle_pause_for_test(&self.admission_pause);
    }

    #[cfg(test)]
    fn wait_at_shutdown_pause_for_test(&self) {
        self.wait_at_lifecycle_pause_for_test(&self.shutdown_pause);
    }

    #[cfg(test)]
    fn wait_at_timeout_restore_pause_for_test(&self) {
        self.wait_at_lifecycle_pause_for_test(&self.timeout_restore_pause);
    }

    #[cfg(test)]
    fn wait_after_take_parts_for_test(&self) {
        self.wait_at_lifecycle_pause_for_test(&self.after_take_parts_pause);
    }

    #[cfg(test)]
    fn wait_after_panic_published_for_test(&self) {
        self.wait_at_lifecycle_pause_for_test(&self.panic_published_pause);
    }

    #[cfg(test)]
    fn wait_at_lifecycle_pause_for_test(&self, slot: &Mutex<Option<LifecyclePause>>) {
        let pause = slot.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let Some(pause) = pause {
            let _ = pause.reached.send(());
            let _ = pause.resume.recv();
        }
    }

    fn take_panic(&self, id: u64) -> Option<String> {
        self.panics
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&id)
    }

    /// Look at a recorded panic without consuming it: several reapers of the
    /// same worker may need the typed failure, and the worker's own handle can
    /// outlive the entry that first observed it.
    fn peek_panic(&self, id: u64) -> Option<String> {
        self.panics
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&id)
            .cloned()
    }

    /// Later bounded reap of a previously timed-out worker.
    pub fn reap(&self, id: u64, timeout: Duration) -> Result<WorkerStop, CoreError> {
        self.join_one(id, timeout)
    }

    /// Cancel every tracked worker without waiting for any of them.
    ///
    /// Shutdown ordering needs this separately from [`Self::shutdown_all_until`]:
    /// a session that is waiting on a dependent owner (for example the resident
    /// ASR engine) must observe cancellation before the dependent owner begins
    /// its own bounded cleanup, otherwise the session's join can consume the
    /// whole shared deadline first. Cancellation stays cooperative; every
    /// worker remains tracked and reapable.
    pub fn cancel_all(&self) {
        let workers = self.workers.lock().unwrap_or_else(|p| p.into_inner());
        workers.values().for_each(|e| e.flag.cancel());
    }

    /// Cancel all workers, then bounded-join each inside one deadline derived
    /// from `timeout`.
    pub fn shutdown_all(&self, timeout: Duration) -> Result<Vec<(u64, WorkerStop)>, CoreError> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .unwrap_or_else(Instant::now);
        self.shutdown_all_until(deadline)
    }

    /// Cancel all workers, then bounded-join each inside one shared deadline.
    ///
    /// Unlike [`Self::shutdown_all`], the per-worker budget is the time left
    /// until `deadline`, never a fresh window for each worker. A spent deadline
    /// still cancels every worker and reports each unfinished one truthfully as
    /// [`WorkerStop::TimedOut`], so the caller never mistakes an unconfirmed
    /// reap for success.
    pub fn shutdown_all_until(
        &self,
        deadline: Instant,
    ) -> Result<Vec<(u64, WorkerStop)>, CoreError> {
        #[cfg(test)]
        self.observe_shutdown_lifecycle_contention_for_test();
        {
            let mut lifecycle = self.lifecycle.lock().unwrap_or_else(|p| p.into_inner());
            *lifecycle = SupervisorLifecycle::ShuttingDown;
        }
        #[cfg(test)]
        self.wait_at_shutdown_pause_for_test();
        let ids: Vec<u64> = {
            let workers = self.workers.lock().unwrap_or_else(|p| p.into_inner());
            workers.values().for_each(|e| e.flag.cancel());
            workers.keys().copied().collect()
        };
        let mut out = Vec::with_capacity(ids.len());
        let mut first_error = None;
        for id in ids {
            match self.join_one_until(id, deadline) {
                Ok(stop) => out.push((id, stop)),
                Err(err) if first_error.is_none() => first_error = Some(err),
                Err(_) => {}
            }
        }
        if let Some(err) = first_error {
            return Err(err);
        }
        Ok(out)
    }

    /// Cancel + bounded-stop a worker, then spawn a replacement with the same
    /// name. Returns the new handle.
    pub fn restart(
        self: &Arc<Self>,
        handle: &WorkerHandle,
        stop_timeout: Duration,
        job: impl FnOnce(CancelFlag) + Send + 'static,
    ) -> Result<WorkerHandle, CoreError> {
        let name = {
            let workers = self.workers.lock().unwrap_or_else(|p| p.into_inner());
            workers
                .get(&handle.id.0)
                .map(|e| e.name.clone())
                .ok_or_else(|| CoreError::Worker("restart of unknown worker".to_string()))?
        };
        handle.flag().cancel();
        match self.join_one(handle.id.0, stop_timeout)? {
            WorkerStop::Stopped => {}
            WorkerStop::TimedOut => {
                return Err(CoreError::Worker(format!(
                    "restart timed out stopping worker {}; replacement was not started",
                    handle.id.0
                )));
            }
        }
        self.spawn(name, job)
    }

    #[must_use]
    pub fn live_worker_count(&self) -> usize {
        let deadline = Instant::now()
            .checked_add(REAP_CONFIRMATION_WINDOW)
            .unwrap_or_else(Instant::now);
        self.live_worker_count_until(deadline)
    }

    /// Observe tracked workers without extending the caller's cleanup deadline.
    #[must_use]
    pub fn live_worker_count_until(&self, deadline: Instant) -> usize {
        self.reap_finished_workers();
        self.await_pending_reap_confirmations(deadline);
        self.workers.lock().unwrap_or_else(|p| p.into_inner()).len()
    }

    /// Event-driven, bounded convergence for reaps whose native join is already
    /// owned by a helper. The wait returns as soon as every such operation is
    /// confirmed; a worker whose native thread epilogue really is still running
    /// stays tracked and is reported truthfully when the window expires. This
    /// function never joins an original handle itself.
    fn await_pending_reap_confirmations(&self, deadline: Instant) {
        loop {
            let pending: Vec<Arc<ReapOperation>> = {
                let workers = self.workers.lock().unwrap_or_else(|p| p.into_inner());
                workers
                    .values()
                    .filter_map(|entry| entry.reap.as_ref().map(Arc::clone))
                    .collect()
            };
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .unwrap_or(Duration::ZERO);
            if pending.is_empty() || remaining.is_zero() {
                return;
            }
            for operation in pending {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return;
                }
                let _ = operation.wait(remaining);
            }
            self.reap_finished_workers();
        }
    }

    #[cfg(test)]
    fn completed_join_count_for_test(&self) -> u64 {
        self.completed_joins.load(Ordering::SeqCst)
    }

    #[must_use]
    pub fn started_at_unix_ms(&self) -> u64 {
        unix_now_ms()
    }
}

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

/// Resolution for a native join that really returned.
///
/// The join helper publishes this only after `JoinHandle::join` on the original
/// worker handle returned. `JoinHandle::is_finished` is deliberately not used:
/// it can report completion while native thread-local destructors are still
/// running, which makes a following `join` unbounded.
fn native_join_resolution(
    completed: Option<ThreadOutcome>,
    joined: std::thread::Result<()>,
) -> ReapResolution {
    let join_panic = match joined {
        Ok(()) => None,
        Err(payload) => Some(panic_message(&payload)),
    };
    match completed {
        Some(ThreadOutcome::Panicked(message)) => ReapResolution::Panicked(message),
        Some(ThreadOutcome::Done) | Some(ThreadOutcome::Disconnected) | None => match join_panic {
            Some(message) => ReapResolution::Panicked(message),
            None => ReapResolution::Finished(WorkerStop::Stopped),
        },
    }
}

/// Handle to one running worker.
pub struct WorkerHandle {
    id: WorkerId,
    supervisor: Arc<WorkerSupervisor>,
}

impl WorkerHandle {
    #[must_use]
    pub const fn id(&self) -> WorkerId {
        self.id
    }

    /// The worker's cancel flag (also usable inside the job closure).
    #[must_use]
    pub fn flag(&self) -> CancelFlag {
        self.supervisor
            .workers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&self.id.0)
            .map(|e| e.flag.clone())
            .unwrap_or_default()
    }

    pub fn cancel(&self) {
        self.flag().cancel();
    }

    pub fn stop(&self, timeout: Duration) -> Result<WorkerStop, CoreError> {
        self.cancel();
        self.supervisor.join_one(self.id.0, timeout)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::mpsc::{Sender, sync_channel};
    use std::thread;

    /// A native thread-local destructor that blocks until the test releases it
    /// after observing that a bounded reap already returned.
    struct BlockedEpilogue {
        entered: Sender<()>,
        release: Receiver<()>,
    }

    impl Drop for BlockedEpilogue {
        fn drop(&mut self) {
            let _ = self.entered.send(());
            let _ = self.release.recv_timeout(Duration::from_secs(5));
        }
    }

    thread_local! {
        static BLOCKED_EPILOGUE: RefCell<Option<BlockedEpilogue>> = const { RefCell::new(None) };
    }

    /// Spawn a worker whose job returns immediately but whose native thread
    /// stays in a blocked thread-local destructor until `release` is used.
    fn spawn_blocked_epilogue_worker(
        sup: &Arc<WorkerSupervisor>,
        name: &str,
    ) -> (WorkerHandle, Receiver<()>, Sender<()>) {
        let (entered_tx, entered_rx) = channel();
        let (release_tx, release_rx) = channel();
        let handle = sup
            .spawn(name, move |_flag: CancelFlag| {
                BLOCKED_EPILOGUE.with(|slot| {
                    *slot.borrow_mut() = Some(BlockedEpilogue {
                        entered: entered_tx,
                        release: release_rx,
                    });
                });
            })
            .unwrap();
        (handle, entered_rx, release_tx)
    }

    fn wait_for_no_workers(supervisor: &WorkerSupervisor) {
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while supervisor.live_worker_count() != 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(supervisor.live_worker_count(), 0);
    }

    #[test]
    fn worker_observes_cancellation_mid_work() {
        let sup = Arc::new(WorkerSupervisor::new());
        let ticks = Arc::new(AtomicU32::new(0));
        let t2 = ticks.clone();
        let handle = sup
            .spawn("ticker", move |flag: CancelFlag| {
                while !flag.is_cancelled() && t2.load(Ordering::SeqCst) < 100 {
                    std::thread::sleep(Duration::from_millis(1));
                    t2.fetch_add(1, Ordering::SeqCst);
                }
            })
            .unwrap();
        assert_eq!(
            handle.stop(Duration::from_secs(2)).unwrap(),
            WorkerStop::Stopped
        );
        let ticks_final = ticks.load(Ordering::SeqCst);
        assert!(
            ticks_final < 100,
            "expected < 100, got {ticks_final}: cancellation not observed"
        );
    }

    #[test]
    fn normal_completion_reaps_its_supervisor_entry() {
        let sup = Arc::new(WorkerSupervisor::new());
        sup.spawn("one-shot", |_flag: CancelFlag| {}).unwrap();
        wait_for_no_workers(&sup);
        assert_eq!(sup.completed_join_count_for_test(), 1);
    }

    #[test]
    fn immediate_completion_leaves_no_registered_worker() {
        let sup = Arc::new(WorkerSupervisor::new());
        sup.spawn("immediate", |_flag: CancelFlag| {}).unwrap();
        wait_for_no_workers(&sup);
        assert_eq!(sup.completed_join_count_for_test(), 1);
    }

    #[test]
    fn shutdown_during_admission_cannot_miss_or_leak_the_worker() {
        let sup = Arc::new(WorkerSupervisor::new());
        let admission_pause = sup.pause_next_admission_for_test();
        let shutdown_pause = sup.pause_next_shutdown_for_test();
        let shutdown_contention = sup.observe_next_shutdown_lifecycle_contention_for_test();
        let ran = Arc::new(AtomicBool::new(false));
        let observed_cancellation = Arc::new(AtomicBool::new(false));
        let ran_by_worker = Arc::clone(&ran);
        let cancellation_by_worker = Arc::clone(&observed_cancellation);
        let spawn_supervisor = Arc::clone(&sup);
        let spawn_thread = thread::spawn(move || {
            spawn_supervisor.spawn("admission-race", move |flag: CancelFlag| {
                ran_by_worker.store(true, Ordering::SeqCst);
                while !flag.is_cancelled() {
                    std::thread::yield_now();
                }
                cancellation_by_worker.store(true, Ordering::SeqCst);
            })
        });

        admission_pause.wait_until_reached();
        assert_eq!(sup.live_worker_count(), 0);

        let shutdown_supervisor = Arc::clone(&sup);
        let shutdown_thread =
            thread::spawn(move || shutdown_supervisor.shutdown_all(Duration::from_secs(1)));
        shutdown_contention
            .recv_timeout(Duration::from_secs(1))
            .unwrap_or_else(|err| {
                test_failure(format!(
                    "shutdown did not observe the admission-held lifecycle lock: {err}"
                ))
            });

        admission_pause.release();
        shutdown_pause.wait_until_reached();
        assert_eq!(sup.live_worker_count(), 1);
        shutdown_pause.release();

        let handle = spawn_thread
            .join()
            .unwrap_or_else(resume_thread_panic)
            .unwrap();
        let stopped = shutdown_thread
            .join()
            .unwrap_or_else(resume_thread_panic)
            .unwrap();

        assert_eq!(stopped, vec![(handle.id().0, WorkerStop::Stopped)]);
        assert!(ran.load(Ordering::SeqCst));
        assert!(observed_cancellation.load(Ordering::SeqCst));
        assert_eq!(sup.live_worker_count(), 0);
    }

    #[test]
    fn admitted_worker_is_cancelled_and_joined_by_shutdown() {
        let sup = Arc::new(WorkerSupervisor::new());
        let (entered_tx, entered_rx) = sync_channel(0);
        let observed_cancellation = Arc::new(AtomicBool::new(false));
        let observed_by_worker = Arc::clone(&observed_cancellation);
        let handle = sup
            .spawn("registered-before-shutdown", move |flag: CancelFlag| {
                entered_tx
                    .send(())
                    .unwrap_or_else(|_| test_failure("worker entry receiver unexpectedly dropped"));
                while !flag.is_cancelled() {
                    std::thread::yield_now();
                }
                observed_by_worker.store(true, Ordering::SeqCst);
            })
            .unwrap();

        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap_or_else(|err| test_failure(format!("admitted worker did not start: {err}")));
        assert_eq!(sup.live_worker_count(), 1);
        let stopped = sup.shutdown_all(Duration::from_secs(1)).unwrap();
        assert_eq!(stopped, vec![(handle.id().0, WorkerStop::Stopped)]);
        assert!(observed_cancellation.load(Ordering::SeqCst));
        assert_eq!(sup.live_worker_count(), 0);
    }

    #[test]
    fn spawn_after_shutdown_begins_is_rejected_without_running_its_job() {
        let sup = Arc::new(WorkerSupervisor::new());
        let shutdown_pause = sup.pause_next_shutdown_for_test();
        let shutdown_supervisor = Arc::clone(&sup);
        let shutdown_thread =
            thread::spawn(move || shutdown_supervisor.shutdown_all(Duration::from_secs(1)));
        shutdown_pause.wait_until_reached();

        let ran = Arc::new(AtomicBool::new(false));
        let ran_by_job = Arc::clone(&ran);
        let result = sup.spawn("rejected-after-shutdown", move |_flag: CancelFlag| {
            ran_by_job.store(true, Ordering::SeqCst);
        });
        assert!(matches!(
            result,
            Err(ref err) if err.to_string().contains("admission rejected")
        ));
        assert!(!ran.load(Ordering::SeqCst));

        shutdown_pause.release();
        assert!(
            shutdown_thread
                .join()
                .unwrap_or_else(resume_thread_panic)
                .unwrap()
                .is_empty()
        );
        assert_eq!(sup.live_worker_count(), 0);
    }

    #[test]
    fn repeated_fast_workers_leave_no_supervisor_entries() {
        let sup = Arc::new(WorkerSupervisor::new());
        for _ in 0..256 {
            sup.spawn("fast", |_flag: CancelFlag| {}).unwrap();
        }
        wait_for_no_workers(&sup);
    }

    /// A completion receipt (and `is_finished`) can both be observable while a
    /// native thread-local destructor is still blocked. The bounded reap must
    /// return inside its deadline and keep the native handle under the join
    /// helper; a later reap confirms the real join.
    #[test]
    fn bounded_reap_keeps_blocking_native_destructor_owned_and_reapable() {
        let sup = Arc::new(WorkerSupervisor::new());
        let (handle, entered_rx, release_tx) =
            spawn_blocked_epilogue_worker(&sup, "blocked-epilogue");
        entered_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap_or_else(|err| test_failure(format!("worker epilogue did not start: {err}")));
        let started = Instant::now();
        assert_eq!(
            handle.stop(Duration::from_millis(50)).unwrap(),
            WorkerStop::TimedOut
        );
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "bounded stop waited for the native thread epilogue: {:?}",
            started.elapsed()
        );
        // The worker is still tracked and its helper owns the original handle.
        assert_eq!(sup.live_worker_count(), 1);

        release_tx
            .send(())
            .unwrap_or_else(|_| test_failure("epilogue release receiver dropped"));
        assert_eq!(
            sup.reap(handle.id().0, Duration::from_secs(2)).unwrap(),
            WorkerStop::Stopped
        );
        assert_eq!(sup.live_worker_count(), 0);
    }

    /// A second, longer-budget reaper coalesces on the helper that already owns
    /// the native join instead of starting a rival join, and confirms the real
    /// completion once the epilogue is released.
    #[test]
    fn coalesced_late_reap_waits_for_the_owned_native_join() {
        let sup = Arc::new(WorkerSupervisor::new());
        let (handle, entered_rx, release_tx) =
            spawn_blocked_epilogue_worker(&sup, "coalesced-epilogue");
        entered_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap_or_else(|err| test_failure(format!("worker epilogue did not start: {err}")));
        let id = handle.id().0;
        assert_eq!(
            handle.stop(Duration::from_millis(50)).unwrap(),
            WorkerStop::TimedOut
        );

        let reaper = Arc::clone(&sup);
        let late = thread::spawn(move || reaper.reap(id, Duration::from_secs(2)));
        thread::sleep(Duration::from_millis(20));
        release_tx
            .send(())
            .unwrap_or_else(|_| test_failure("epilogue release receiver dropped"));
        assert_eq!(
            late.join().unwrap_or_else(resume_thread_panic).unwrap(),
            WorkerStop::Stopped
        );
        assert_eq!(sup.live_worker_count(), 0);
    }

    /// A failed join-helper creation must not drop the original native handle
    /// or the already consumed typed outcome; the next bounded reap starts a
    /// fresh helper and confirms the join.
    #[test]
    fn native_join_helper_failure_preserves_ownership_until_later_reap() {
        let sup = Arc::new(WorkerSupervisor::new());
        sup.fail_next_native_join_helper_for_test();
        let handle = sup.spawn("helper-failure", |_flag: CancelFlag| {}).unwrap();
        let err = handle
            .stop(Duration::from_secs(1))
            .err()
            .unwrap_or_else(|| test_failure("controlled helper failure was not reported"));
        assert!(
            err.to_string()
                .contains("native join helper could not be started"),
            "{err}"
        );
        {
            let workers = sup.workers.lock().unwrap_or_else(|p| p.into_inner());
            let entry = workers
                .get(&handle.id().0)
                .unwrap_or_else(|| test_failure("helper failure lost the worker entry"));
            assert!(
                entry.handle.is_some(),
                "helper failure dropped the original native handle"
            );
            assert!(entry.reap.is_none(), "reap operation stayed claimed");
            assert!(
                entry.completed.is_some() || entry.done_rx.is_some(),
                "helper failure dropped the completion ownership"
            );
        }
        assert_eq!(
            sup.reap(handle.id().0, Duration::from_secs(2)).unwrap(),
            WorkerStop::Stopped
        );
        assert_eq!(sup.live_worker_count(), 0);
    }

    /// Dropping every supervisor owner must not strand the original native
    /// handle: the join helper keeps it owned and still publishes the real join
    /// result afterwards, without claiming success from a detach.
    #[test]
    fn dropped_supervisor_leaves_the_native_join_owned_by_its_helper() {
        let sup = Arc::new(WorkerSupervisor::new());
        let (handle, entered_rx, release_tx) =
            spawn_blocked_epilogue_worker(&sup, "drop-owned-epilogue");
        entered_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap_or_else(|err| test_failure(format!("worker epilogue did not start: {err}")));
        assert_eq!(
            handle.stop(Duration::from_millis(50)).unwrap(),
            WorkerStop::TimedOut
        );
        let operation = {
            let workers = sup.workers.lock().unwrap_or_else(|p| p.into_inner());
            Arc::clone(
                workers
                    .get(&handle.id().0)
                    .and_then(|entry| entry.reap.as_ref())
                    .unwrap_or_else(|| test_failure("timed-out worker lost its reap operation")),
            )
        };

        // Drop both owners of the supervisor while the helper owns the original
        // handle. Only the helper can complete the native join now.
        drop(handle);
        drop(sup);
        release_tx
            .send(())
            .unwrap_or_else(|_| test_failure("epilogue release receiver dropped"));
        assert!(
            matches!(
                operation.wait(Duration::from_secs(2)),
                Some(ReapResolution::Finished(WorkerStop::Stopped))
            ),
            "the helper did not join the original native thread after supervisor drop"
        );
    }

    #[test]
    fn supervisor_shutdown_all_joins_bounded() {
        let sup = Arc::new(WorkerSupervisor::new());
        for i in 0..4 {
            sup.spawn(format!("w{i}"), move |flag: CancelFlag| {
                while !flag.is_cancelled() {
                    std::thread::sleep(Duration::from_millis(1));
                }
            })
            .unwrap();
        }
        let results = sup.shutdown_all(Duration::from_secs(2)).unwrap();
        assert_eq!(results.len(), 4);
        assert!(results.iter().all(|(_, s)| *s == WorkerStop::Stopped));
        assert_eq!(sup.live_worker_count(), 0);
    }

    /// Several stubborn workers must share one deadline. Under the previous
    /// per-worker budget this could spend `worker_count * timeout`.
    #[test]
    fn shutdown_all_until_shares_one_deadline_across_workers() {
        let sup = Arc::new(WorkerSupervisor::new());
        let (observed_tx, observed_rx) = channel();
        let observed_tx = Arc::new(Mutex::new(observed_tx));
        for i in 0..3 {
            let observed = Arc::clone(&observed_tx);
            sup.spawn(format!("stubborn-{i}"), move |flag: CancelFlag| {
                while !flag.is_cancelled() {
                    std::thread::sleep(Duration::from_millis(1));
                }
                // Cancellation is observed, but completion deliberately waits
                // past the shared window.
                let _ = observed.lock().unwrap_or_else(|p| p.into_inner()).send(());
                std::thread::sleep(Duration::from_millis(600));
            })
            .unwrap();
        }
        let started = Instant::now();
        let outcomes = sup
            .shutdown_all_until(started + Duration::from_millis(150))
            .unwrap();
        let elapsed = started.elapsed();
        assert_eq!(outcomes.len(), 3);
        assert!(
            outcomes
                .iter()
                .all(|(_, stop)| *stop == WorkerStop::TimedOut)
        );
        assert_eq!(sup.live_worker_count(), 3, "timed-out workers stay owned");
        assert!(
            elapsed <= Duration::from_millis(450),
            "shared deadline accumulated per-worker windows: {elapsed:?}"
        );
        // Every owner observed cancellation even though the deadline expired.
        for _ in 0..3 {
            observed_rx
                .recv_timeout(Duration::from_millis(500))
                .unwrap_or_else(|error| {
                    test_failure(format!(
                        "cancelled worker did not observe cancellation: {error}"
                    ))
                });
        }

        // A later, explicitly bounded reap still resolves them truthfully.
        let reaped = sup
            .shutdown_all_until(Instant::now() + Duration::from_secs(2))
            .unwrap();
        assert!(reaped.iter().all(|(_, stop)| *stop == WorkerStop::Stopped));
        assert_eq!(sup.live_worker_count(), 0);
    }

    #[test]
    fn cancel_all_signals_every_tracked_worker_without_waiting() {
        let sup = Arc::new(WorkerSupervisor::new());
        let observed = Arc::new(AtomicBool::new(false));
        for i in 0..2 {
            let observed = Arc::clone(&observed);
            sup.spawn(format!("cancellable-{i}"), move |flag: CancelFlag| {
                while !flag.is_cancelled() {
                    std::thread::sleep(Duration::from_millis(1));
                }
                observed.store(true, Ordering::SeqCst);
                // Deliberately outlive the cancellation request so the caller
                // can prove cancel_all itself did not block on this work.
                std::thread::sleep(Duration::from_millis(300));
            })
            .unwrap();
        }
        let started = Instant::now();
        sup.cancel_all();
        let elapsed = started.elapsed();
        assert!(
            elapsed <= Duration::from_millis(100),
            "cancel_all waited for worker completion: {elapsed:?}"
        );
        assert_eq!(sup.live_worker_count(), 2, "ownership must be preserved");
        let deadline = Instant::now() + Duration::from_millis(500);
        while !observed.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(
            observed.load(Ordering::SeqCst),
            "a tracked worker never observed cancellation"
        );
        let results = sup.shutdown_all(Duration::from_secs(2)).unwrap();
        assert!(results.iter().all(|(_, stop)| *stop == WorkerStop::Stopped));
        assert_eq!(sup.live_worker_count(), 0);
    }

    #[test]
    fn shutdown_reports_timed_out_truthfully() {
        let sup = Arc::new(WorkerSupervisor::new());
        let handle = sup
            .spawn("stubborn", |_flag: CancelFlag| {
                // Ignores cancellation entirely; sleeps past any sane test budget.
                std::thread::sleep(Duration::from_millis(250));
            })
            .unwrap();
        let stop = handle.stop(Duration::from_millis(30)).unwrap();
        assert_eq!(stop, WorkerStop::TimedOut);
        // The worker is still tracked and reapable.
        let stop2 = sup.reap(handle.id().0, Duration::from_secs(1)).unwrap();
        assert_eq!(stop2, WorkerStop::Stopped);
    }

    #[test]
    fn concurrent_shutdown_retries_after_checked_out_stop_times_out() {
        let sup = Arc::new(WorkerSupervisor::new());
        let after_take_pause = sup.pause_next_after_take_parts_for_test();
        let coalesced_waiter = sup.observe_next_coalesced_waiter_for_test();
        let (entered_tx, entered_rx) = sync_channel(0);
        let (finish_tx, finish_rx) = sync_channel(0);
        let handle = sup
            .spawn("coalesced-stop-shutdown", move |_flag: CancelFlag| {
                entered_tx
                    .send(())
                    .unwrap_or_else(|_| test_failure("worker entry receiver dropped"));
                finish_rx
                    .recv()
                    .unwrap_or_else(|_| test_failure("worker finish sender dropped"));
            })
            .unwrap();
        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap_or_else(|err| test_failure(format!("worker did not start: {err}")));
        let id = handle.id().0;

        let stop_thread = thread::spawn(move || handle.stop(Duration::ZERO));
        after_take_pause.wait_until_reached();
        {
            let workers = sup.workers.lock().unwrap_or_else(|p| p.into_inner());
            let entry = workers
                .get(&id)
                .unwrap_or_else(|| test_failure("checked-out worker was not tracked"));
            assert!(entry.done_rx.is_none());
            assert!(entry.handle.is_none());
            assert!(entry.reap.is_some());
        }

        let shutdown_supervisor = Arc::clone(&sup);
        let shutdown_thread =
            thread::spawn(move || shutdown_supervisor.shutdown_all(Duration::from_secs(1)));
        coalesced_waiter
            .recv_timeout(Duration::from_secs(1))
            .unwrap_or_else(|err| {
                test_failure(format!("shutdown did not coalesce with active stop: {err}"))
            });

        after_take_pause.release();
        assert_eq!(
            stop_thread
                .join()
                .unwrap_or_else(resume_thread_panic)
                .unwrap(),
            WorkerStop::TimedOut
        );
        finish_tx
            .send(())
            .unwrap_or_else(|_| test_failure("worker finish receiver dropped"));
        assert_eq!(
            shutdown_thread
                .join()
                .unwrap_or_else(resume_thread_panic)
                .unwrap(),
            vec![(id, WorkerStop::Stopped)]
        );
        assert_eq!(sup.live_worker_count(), 0);
    }

    #[test]
    fn zero_timeout_coalesced_reaps_retain_no_waiter_registrations() {
        let sup = Arc::new(WorkerSupervisor::new());
        let after_take_pause = sup.pause_next_after_take_parts_for_test();
        let (entered_tx, entered_rx) = sync_channel(0);
        let (finish_tx, finish_rx) = sync_channel(0);
        let handle = sup
            .spawn("zero-timeout-coalescing", move |_flag: CancelFlag| {
                entered_tx
                    .send(())
                    .unwrap_or_else(|_| test_failure("worker entry receiver dropped"));
                finish_rx
                    .recv()
                    .unwrap_or_else(|_| test_failure("worker finish sender dropped"));
            })
            .unwrap();
        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap_or_else(|err| test_failure(format!("worker did not start: {err}")));
        let id = handle.id().0;

        let stop_thread = thread::spawn(move || handle.stop(Duration::from_secs(5)));
        after_take_pause.wait_until_reached();
        let operation = {
            let workers = sup.workers.lock().unwrap_or_else(|p| p.into_inner());
            Arc::clone(
                workers
                    .get(&id)
                    .and_then(|entry| entry.reap.as_ref())
                    .unwrap_or_else(|| test_failure("active reap operation was not tracked")),
            )
        };

        for _ in 0..256 {
            assert_eq!(sup.reap(id, Duration::ZERO).unwrap(), WorkerStop::TimedOut);
        }
        assert!(
            operation
                .resolution
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .is_none()
        );

        finish_tx
            .send(())
            .unwrap_or_else(|_| test_failure("worker finish receiver dropped"));
        after_take_pause.release();
        assert_eq!(
            stop_thread
                .join()
                .unwrap_or_else(resume_thread_panic)
                .unwrap(),
            WorkerStop::Stopped
        );
        assert_eq!(sup.live_worker_count(), 0);
    }

    #[test]
    fn timed_out_restore_is_atomic_against_concurrent_reap() {
        let sup = Arc::new(WorkerSupervisor::new());
        let restore_pause = sup.pause_next_timeout_restore_for_test();
        let (entered_tx, entered_rx) = sync_channel(0);
        let (finish_tx, finish_rx) = sync_channel(0);
        let handle = sup
            .spawn("atomic-restore-reap", move |_flag: CancelFlag| {
                entered_tx
                    .send(())
                    .unwrap_or_else(|_| test_failure("worker entry receiver dropped"));
                finish_rx
                    .recv()
                    .unwrap_or_else(|_| test_failure("worker finish sender dropped"));
            })
            .unwrap();
        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap_or_else(|err| test_failure(format!("worker did not start: {err}")));
        let id = handle.id().0;

        let stop_thread = thread::spawn(move || handle.stop(Duration::from_millis(1)));
        restore_pause.wait_until_reached();
        assert!(matches!(
            sup.workers.try_lock(),
            Err(TryLockError::WouldBlock)
        ));

        let (reap_started_tx, reap_started_rx) = sync_channel(0);
        let (reap_done_tx, reap_done_rx) = channel();
        let reap_supervisor = Arc::clone(&sup);
        let reap_thread = thread::spawn(move || {
            reap_started_tx
                .send(())
                .unwrap_or_else(|_| test_failure("reap-start receiver dropped"));
            let result = reap_supervisor.reap(id, Duration::from_secs(1));
            let _ = reap_done_tx.send(());
            result
        });
        reap_started_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap_or_else(|err| test_failure(format!("reaper did not start: {err}")));
        assert!(matches!(
            reap_done_rx.recv_timeout(Duration::from_millis(20)),
            Err(RecvTimeoutError::Timeout)
        ));

        restore_pause.release();
        assert_eq!(
            stop_thread
                .join()
                .unwrap_or_else(resume_thread_panic)
                .unwrap(),
            WorkerStop::TimedOut
        );
        finish_tx
            .send(())
            .unwrap_or_else(|_| test_failure("worker finish receiver dropped"));
        assert_eq!(
            reap_thread
                .join()
                .unwrap_or_else(resume_thread_panic)
                .unwrap(),
            WorkerStop::Stopped
        );
        assert_eq!(sup.live_worker_count(), 0);
    }

    #[test]
    fn timed_out_restore_is_atomic_against_shutdown_snapshot() {
        let sup = Arc::new(WorkerSupervisor::new());
        let restore_pause = sup.pause_next_timeout_restore_for_test();
        let (entered_tx, entered_rx) = sync_channel(0);
        let (finish_tx, finish_rx) = sync_channel(0);
        let handle = sup
            .spawn("atomic-restore-shutdown", move |_flag: CancelFlag| {
                entered_tx
                    .send(())
                    .unwrap_or_else(|_| test_failure("worker entry receiver dropped"));
                finish_rx
                    .recv()
                    .unwrap_or_else(|_| test_failure("worker finish sender dropped"));
            })
            .unwrap();
        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap_or_else(|err| test_failure(format!("worker did not start: {err}")));
        let id = handle.id().0;

        let stop_thread = thread::spawn(move || handle.stop(Duration::from_millis(1)));
        restore_pause.wait_until_reached();
        assert!(matches!(
            sup.workers.try_lock(),
            Err(TryLockError::WouldBlock)
        ));

        let (shutdown_started_tx, shutdown_started_rx) = sync_channel(0);
        let shutdown_supervisor = Arc::clone(&sup);
        let shutdown_thread = thread::spawn(move || {
            shutdown_started_tx
                .send(())
                .unwrap_or_else(|_| test_failure("shutdown-start receiver dropped"));
            shutdown_supervisor.shutdown_all(Duration::from_secs(1))
        });
        shutdown_started_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap_or_else(|err| test_failure(format!("shutdown did not start: {err}")));
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        loop {
            let shutting_down = *sup.lifecycle.lock().unwrap_or_else(|p| p.into_inner())
                == SupervisorLifecycle::ShuttingDown;
            if shutting_down {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "shutdown did not close worker admission"
            );
            thread::yield_now();
        }

        restore_pause.release();
        assert_eq!(
            stop_thread
                .join()
                .unwrap_or_else(resume_thread_panic)
                .unwrap(),
            WorkerStop::TimedOut
        );
        finish_tx
            .send(())
            .unwrap_or_else(|_| test_failure("worker finish receiver dropped"));
        assert_eq!(
            shutdown_thread
                .join()
                .unwrap_or_else(resume_thread_panic)
                .unwrap(),
            vec![(id, WorkerStop::Stopped)]
        );
        assert_eq!(sup.live_worker_count(), 0);
    }

    #[test]
    fn panicking_worker_maps_to_typed_error_not_process_crash() {
        let sup = Arc::new(WorkerSupervisor::new());
        let handle = sup
            .spawn("bomb", |_flag: CancelFlag| {
                test_failure("intentional test panic");
            })
            .unwrap();
        // Give the worker a moment to explode.
        std::thread::sleep(Duration::from_millis(50));
        let err = handle.stop(Duration::from_secs(1)).unwrap_err();
        assert!(err.to_string().contains("panicked"), "{err}");
    }

    #[test]
    fn panic_is_published_to_coalesced_and_late_reapers_before_removal() {
        let sup = Arc::new(WorkerSupervisor::new());
        let after_take_pause = sup.pause_next_after_take_parts_for_test();
        let panic_published_pause = sup.pause_next_after_panic_published_for_test();
        let coalesced_waiter = sup.observe_next_coalesced_waiter_for_test();
        let (entered_tx, entered_rx) = sync_channel(0);
        let (panic_tx, panic_rx) = sync_channel(0);
        let handle = sup
            .spawn("coalesced-panic", move |_flag: CancelFlag| {
                entered_tx
                    .send(())
                    .unwrap_or_else(|_| test_failure("worker entry receiver dropped"));
                panic_rx
                    .recv()
                    .unwrap_or_else(|_| test_failure("worker panic sender dropped"));
                test_failure("controlled coalesced worker panic");
            })
            .unwrap();
        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap_or_else(|err| test_failure(format!("worker did not start: {err}")));
        let id = handle.id().0;

        let stop_thread = thread::spawn(move || handle.stop(Duration::from_secs(1)));
        after_take_pause.wait_until_reached();
        let shutdown_supervisor = Arc::clone(&sup);
        let shutdown_thread =
            thread::spawn(move || shutdown_supervisor.shutdown_all(Duration::from_secs(1)));
        coalesced_waiter
            .recv_timeout(Duration::from_secs(1))
            .unwrap_or_else(|err| {
                test_failure(format!("shutdown did not coalesce with panic reap: {err}"))
            });

        panic_tx
            .send(())
            .unwrap_or_else(|_| test_failure("worker panic receiver dropped"));
        after_take_pause.release();
        panic_published_pause.wait_until_reached();

        let shutdown_error = shutdown_thread
            .join()
            .unwrap_or_else(resume_thread_panic)
            .err()
            .unwrap_or_else(|| test_failure("coalesced shutdown missed worker panic"));
        assert!(
            shutdown_error
                .to_string()
                .contains("controlled coalesced worker panic"),
            "{shutdown_error}"
        );
        {
            let workers = sup.workers.lock().unwrap_or_else(|p| p.into_inner());
            let operation = workers
                .get(&id)
                .and_then(|entry| entry.reap.as_ref())
                .unwrap_or_else(|| test_failure("worker was removed before panic publication"));
            assert!(matches!(
                operation
                    .resolution
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .as_ref(),
                Some(ReapResolution::Panicked(message))
                    if message == "controlled coalesced worker panic"
            ));
        }

        panic_published_pause.release();
        let stop_error = stop_thread
            .join()
            .unwrap_or_else(resume_thread_panic)
            .err()
            .unwrap_or_else(|| test_failure("owner stop missed worker panic"));
        assert!(
            stop_error
                .to_string()
                .contains("controlled coalesced worker panic"),
            "{stop_error}"
        );
        assert!(
            !sup.workers
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .contains_key(&id)
        );

        let late_error = sup
            .reap(id, Duration::ZERO)
            .err()
            .unwrap_or_else(|| test_failure("late reap lost retained panic diagnostic"));
        assert!(
            late_error
                .to_string()
                .contains("controlled coalesced worker panic"),
            "{late_error}"
        );
        assert_eq!(sup.live_worker_count(), 0);
    }

    #[test]
    fn repeated_panics_leave_only_bounded_diagnostic_residue() {
        let sup = Arc::new(WorkerSupervisor::new());
        for _ in 0..(MAX_RECORDED_PANICS + 1) {
            sup.spawn("panic-residue", |_flag: CancelFlag| {
                test_failure("intentional bounded-residue test panic");
            })
            .unwrap();
        }
        wait_for_no_workers(&sup);
        assert!(sup.panics.lock().unwrap_or_else(|p| p.into_inner()).len() <= MAX_RECORDED_PANICS);
    }

    #[test]
    fn controlled_spawn_failure_leaves_no_supervisor_residue() {
        let sup = Arc::new(WorkerSupervisor::new());
        sup.fail_next_thread_spawn_for_test();
        let result = sup.spawn("fails-before-registration", |_flag: CancelFlag| {});
        assert!(matches!(
            result,
            Err(ref err) if err.to_string().contains("thread-spawn failure")
        ));
        assert_eq!(sup.live_worker_count(), 0);
        assert!(
            sup.panics
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .is_empty()
        );
    }

    #[test]
    fn restart_replaces_handle_with_fresh_worker() {
        let sup = Arc::new(WorkerSupervisor::new());
        let counter = Arc::new(AtomicU32::new(0));
        let c1 = counter.clone();
        let h1 = sup
            .spawn("generation-worker", move |flag: CancelFlag| {
                while !flag.is_cancelled() {
                    std::thread::sleep(Duration::from_millis(1));
                }
                c1.fetch_add(1, Ordering::SeqCst);
            })
            .unwrap();
        let old_id = h1.id();
        let h2 = sup
            .restart(&h1, Duration::from_secs(1), move |flag: CancelFlag| {
                while !flag.is_cancelled() {
                    std::thread::sleep(Duration::from_millis(1));
                }
            })
            .unwrap();
        assert_ne!(h2.id(), old_id);
        h2.stop(Duration::from_secs(1)).unwrap();
        assert_eq!(
            counter.load(Ordering::SeqCst),
            1,
            "old worker observed cancel"
        );
    }

    #[test]
    fn restart_timeout_keeps_old_worker_and_does_not_start_replacement() {
        let sup = Arc::new(WorkerSupervisor::new());
        let old_running = Arc::new(AtomicBool::new(false));
        let replacement_runs = Arc::new(AtomicU32::new(0));
        let overlap_observed = Arc::new(AtomicBool::new(false));
        let (old_entered_tx, old_entered_rx) = sync_channel(0);
        let (old_finish_tx, old_finish_rx) = sync_channel(0);
        let running_by_old = Arc::clone(&old_running);
        let h1 = sup
            .spawn("restart-timeout", move |_flag: CancelFlag| {
                running_by_old.store(true, Ordering::SeqCst);
                old_entered_tx
                    .send(())
                    .unwrap_or_else(|_| test_failure("old-worker entry receiver dropped"));
                old_finish_rx
                    .recv()
                    .unwrap_or_else(|_| test_failure("old-worker finish sender dropped"));
                running_by_old.store(false, Ordering::SeqCst);
            })
            .unwrap();
        old_entered_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap_or_else(|err| test_failure(format!("old worker did not start: {err}")));

        let first_runs = Arc::clone(&replacement_runs);
        let first_overlap = Arc::clone(&overlap_observed);
        let first_old_running = Arc::clone(&old_running);
        let result = sup.restart(&h1, Duration::from_millis(1), move |_flag: CancelFlag| {
            first_runs.fetch_add(1, Ordering::SeqCst);
            first_overlap.store(first_old_running.load(Ordering::SeqCst), Ordering::SeqCst);
        });
        let err = match result {
            Ok(_) => test_failure("restart unexpectedly started a replacement"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("restart timed out"), "{err}");
        assert!(
            err.to_string().contains("replacement was not started"),
            "{err}"
        );
        assert_eq!(replacement_runs.load(Ordering::SeqCst), 0);
        assert!(!overlap_observed.load(Ordering::SeqCst));
        assert_eq!(sup.next_id.load(Ordering::SeqCst), 1);
        assert_eq!(sup.live_worker_count(), 1);

        old_finish_tx
            .send(())
            .unwrap_or_else(|_| test_failure("old-worker finish receiver dropped"));
        let (replacement_entered_tx, replacement_entered_rx) = sync_channel(0);
        let later_runs = Arc::clone(&replacement_runs);
        let later_overlap = Arc::clone(&overlap_observed);
        let later_old_running = Arc::clone(&old_running);
        let h2 = sup
            .restart(&h1, Duration::from_secs(1), move |flag: CancelFlag| {
                later_runs.fetch_add(1, Ordering::SeqCst);
                later_overlap.store(later_old_running.load(Ordering::SeqCst), Ordering::SeqCst);
                replacement_entered_tx
                    .send(())
                    .unwrap_or_else(|_| test_failure("replacement entry receiver dropped"));
                while !flag.is_cancelled() {
                    thread::yield_now();
                }
            })
            .unwrap();
        replacement_entered_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap_or_else(|err| test_failure(format!("replacement did not start: {err}")));
        assert_ne!(h2.id(), h1.id());
        assert_eq!(replacement_runs.load(Ordering::SeqCst), 1);
        assert!(!overlap_observed.load(Ordering::SeqCst));
        assert_eq!(
            h2.stop(Duration::from_secs(1)).unwrap(),
            WorkerStop::Stopped
        );
        assert_eq!(sup.live_worker_count(), 0);
    }
}
