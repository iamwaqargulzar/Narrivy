//! Daemon-owned serialization and shutdown for portal keyboard sessions.
//!
//! This actor owns the Linux adapter lifecycle on one supervised worker. The
//! direct driver can own the reviewed EI text sender, while daemon startup and
//! desktop prompting remain separate explicit composition decisions.

#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use narrivy_core::{WorkerHandle, WorkerStop, WorkerSupervisor};
use narrivy_ipc::{
    Event, PortalCleanupDto, PortalEligibilityDto, PortalFailureDto, PortalKeyboardStatusDto,
    PortalLifecycleDto,
};
use narrivy_ipc_unix::UnixIpcServer;
use narrivy_platform_linux::portal_eis::{
    CombinedPortalCancellation, PortalCleanupStatus, PortalCloseResult, PortalEisTextFailure,
    PortalEisTextPayload, PortalEisTextSubmission, PortalParentWindow, PortalSessionDriver,
    PortalSessionError, PortalSessionFailure, PortalSessionLifecycle, PortalSessionState,
};
use narrivy_platform_linux::recovery::PortalEisTextSubmitter;
use narrivy_types::SessionCancellation;

const ACTOR_POLL_INTERVAL: Duration = Duration::from_millis(10);
const ACTOR_COMMAND_CAPACITY: usize = 8;
const DRIVER_EVENT_CLOSE_BUDGET: Duration = Duration::from_millis(250);
const DROP_CLOSE_BUDGET: Duration = Duration::from_millis(250);
pub const PORTAL_EVENT_POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PortalDriverEventRecord {
    sequence: u64,
    activation_generation: u64,
    error: PortalSessionError,
}

/// Stable actor-level failure without D-Bus, environment, or payload text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortalActorFailure {
    AdmissionClosed,
    Busy,
    TimedOut,
    WorkerStopped,
    WorkerFailed,
    Lifecycle(PortalSessionError),
}

/// Truthful bounded daemon-shutdown result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortalActorShutdown {
    pub worker: WorkerStop,
    pub cleanup: PortalCleanupStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PortalActorTerminal {
    cleanup: PortalCleanupStatus,
}

enum PortalActorCommand {
    Activate {
        parent: PortalParentWindow,
        deadline: Instant,
        request_cancel: SessionCancellation,
        reply: SyncSender<Result<(), PortalSessionError>>,
    },
    SubmissionAllowed {
        deadline: Instant,
        request_cancel: SessionCancellation,
        reply: SyncSender<Result<(), PortalSessionError>>,
    },
    SubmitText {
        payload: PortalEisTextPayload,
        deadline: Instant,
        request_cancel: SessionCancellation,
        reply: SyncSender<PortalEisTextSubmission>,
    },
    Close {
        deadline: Instant,
        reply: SyncSender<Result<PortalCloseResult, PortalSessionError>>,
    },
    ObserveRevoked {
        reply: SyncSender<PortalSessionError>,
    },
    State {
        reply: SyncSender<PortalSessionState>,
    },
}

struct PortalActorAdmission {
    accepting: bool,
    commands: SyncSender<PortalActorCommand>,
}

#[cfg(test)]
struct ActorPause {
    reached: SyncSender<()>,
    resume: Receiver<()>,
}

#[cfg(test)]
struct ActorTestPause {
    reached: Receiver<()>,
    resume: SyncSender<()>,
}

#[cfg(test)]
#[derive(Default)]
struct PortalActorTestHooks {
    after_activation: Mutex<Option<ActorPause>>,
    after_submission: Mutex<Option<ActorPause>>,
    after_driver_poll: Mutex<Option<ActorPause>>,
    shutdown_closed: Mutex<Option<ActorPause>>,
}

struct PortalActorRuntime {
    owner_cancel: SessionCancellation,
    shutdown_deadline: Arc<Mutex<Option<Instant>>>,
    terminal: SyncSender<PortalActorTerminal>,
    last_driver_event: Arc<Mutex<Option<PortalDriverEventRecord>>>,
    activation_generation: Arc<AtomicU64>,
    #[cfg(test)]
    test_hooks: Arc<PortalActorTestHooks>,
}

/// Handle to one daemon-supervised portal-session owner.
///
/// Calls are serialized through the worker. `shutdown` closes admission before
/// cancelling the owner, so no later request can begin. A timed-out worker
/// remains in [`WorkerSupervisor`] for bounded reaping by daemon shutdown.
pub struct PortalSessionActor {
    admission: Mutex<PortalActorAdmission>,
    worker: Mutex<Option<WorkerHandle>>,
    owner_cancel: SessionCancellation,
    shutdown_deadline: Arc<Mutex<Option<Instant>>>,
    terminal: Mutex<Option<Receiver<PortalActorTerminal>>>,
    last_driver_event: Arc<Mutex<Option<PortalDriverEventRecord>>>,
    activation_generation: Arc<AtomicU64>,
    #[cfg(test)]
    test_hooks: Arc<PortalActorTestHooks>,
}

impl PortalSessionActor {
    pub fn spawn<D>(
        supervisor: &Arc<WorkerSupervisor>,
        driver: D,
    ) -> Result<Self, PortalActorFailure>
    where
        D: PortalSessionDriver + 'static,
    {
        let (command_tx, command_rx) = sync_channel(ACTOR_COMMAND_CAPACITY);
        let (terminal_tx, terminal_rx) = sync_channel(1);
        let owner_cancel = SessionCancellation::new();
        let worker_cancel = owner_cancel.clone();
        let shutdown_deadline = Arc::new(Mutex::new(None));
        let worker_deadline = Arc::clone(&shutdown_deadline);
        let last_driver_event = Arc::new(Mutex::new(None));
        let worker_last_driver_event = Arc::clone(&last_driver_event);
        let activation_generation = Arc::new(AtomicU64::new(0));
        let worker_activation_generation = Arc::clone(&activation_generation);
        #[cfg(test)]
        let test_hooks = Arc::new(PortalActorTestHooks::default());
        #[cfg(test)]
        let worker_test_hooks = Arc::clone(&test_hooks);
        let worker = supervisor
            .spawn_with_cancellation(
                "portal-session",
                owner_cancel.clone(),
                move |supervisor_cancel| {
                    run_actor(
                        PortalSessionLifecycle::new(driver),
                        command_rx,
                        PortalActorRuntime {
                            owner_cancel: supervisor_cancel,
                            shutdown_deadline: worker_deadline,
                            terminal: terminal_tx,
                            last_driver_event: worker_last_driver_event,
                            activation_generation: worker_activation_generation,
                            #[cfg(test)]
                            test_hooks: worker_test_hooks,
                        },
                    );
                },
            )
            .map_err(|_| PortalActorFailure::WorkerFailed)?;

        Ok(Self {
            admission: Mutex::new(PortalActorAdmission {
                accepting: true,
                commands: command_tx,
            }),
            worker: Mutex::new(Some(worker)),
            owner_cancel: worker_cancel,
            shutdown_deadline,
            terminal: Mutex::new(Some(terminal_rx)),
            last_driver_event,
            activation_generation,
            #[cfg(test)]
            test_hooks,
        })
    }

    pub fn activate(
        &self,
        parent: PortalParentWindow,
        deadline: Instant,
        request_cancel: SessionCancellation,
    ) -> Result<(), PortalActorFailure> {
        let (reply_tx, reply_rx) = sync_channel(1);
        self.enqueue(
            PortalActorCommand::Activate {
                parent,
                deadline,
                request_cancel: request_cancel.clone(),
                reply: reply_tx,
            },
            deadline,
        )?;
        match receive_until(reply_rx, deadline) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(PortalActorFailure::Lifecycle(error)),
            Err(error) => {
                request_cancel.cancel();
                Err(error)
            }
        }
    }

    /// Cancellation-aware pre-dispatch admission. The future sender's first
    /// accepted event must remain in this same actor to close the residual race.
    pub fn submission_allowed(
        &self,
        deadline: Instant,
        request_cancel: SessionCancellation,
    ) -> Result<(), PortalActorFailure> {
        let (reply_tx, reply_rx) = sync_channel(1);
        self.enqueue(
            PortalActorCommand::SubmissionAllowed {
                deadline,
                request_cancel: request_cancel.clone(),
                reply: reply_tx,
            },
            deadline,
        )?;
        match receive_until(reply_rx, deadline) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(PortalActorFailure::Lifecycle(error)),
            Err(error) => {
                request_cancel.cancel();
                Err(error)
            }
        }
    }

    /// Submit one validated frame on the actor owner. Once the command is
    /// admitted, loss of the reply is conservatively post-dispatch uncertain.
    pub fn submit_text(
        &self,
        payload: PortalEisTextPayload,
        deadline: Instant,
        request_cancel: SessionCancellation,
    ) -> PortalEisTextSubmission {
        let (reply_tx, reply_rx) = sync_channel(1);
        if let Err(error) = self.enqueue(
            PortalActorCommand::SubmitText {
                payload,
                deadline,
                request_cancel: request_cancel.clone(),
                reply: reply_tx,
            },
            deadline,
        ) {
            return PortalEisTextSubmission::RejectedBeforeDispatch(text_failure_from_actor(error));
        }
        match receive_until(reply_rx, deadline) {
            Ok(outcome) => outcome,
            Err(error) => {
                request_cancel.cancel();
                PortalEisTextSubmission::DispatchUncertain(text_failure_from_actor(error))
            }
        }
    }

    /// Explicit owner-requested close. This is distinct from observing that
    /// the portal revoked the session independently.
    pub fn close(&self, deadline: Instant) -> Result<PortalCloseResult, PortalActorFailure> {
        let (reply_tx, reply_rx) = sync_channel(1);
        self.enqueue(
            PortalActorCommand::Close {
                deadline,
                reply: reply_tx,
            },
            deadline,
        )?;
        match receive_until(reply_rx, deadline)? {
            Ok(result) => Ok(result),
            Err(error) => Err(PortalActorFailure::Lifecycle(error)),
        }
    }

    /// Record a portal-originated revocation as a typed terminal event and
    /// remove any remaining local authority.
    pub fn observe_revoked(
        &self,
        deadline: Instant,
    ) -> Result<PortalSessionError, PortalActorFailure> {
        let (reply_tx, reply_rx) = sync_channel(1);
        self.enqueue(
            PortalActorCommand::ObserveRevoked { reply: reply_tx },
            deadline,
        )?;
        receive_until(reply_rx, deadline)
    }

    pub fn state(&self, deadline: Instant) -> Result<PortalSessionState, PortalActorFailure> {
        let (reply_tx, reply_rx) = sync_channel(1);
        self.enqueue(PortalActorCommand::State { reply: reply_tx }, deadline)?;
        receive_until(reply_rx, deadline)
    }

    /// Latest content-free driver-originated lifecycle event, if any.
    #[must_use]
    pub fn last_driver_event(&self) -> Option<PortalSessionError> {
        self.last_driver_event
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .map(|record| record.error)
    }

    fn last_driver_event_record(&self) -> Option<PortalDriverEventRecord> {
        *self
            .last_driver_event
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Return the latest driver event only when it is newer than the caller's
    /// observed sequence. The event contains stable enums only.
    #[must_use]
    pub fn driver_event_since(
        &self,
        observed_sequence: u64,
    ) -> Option<(u64, u64, PortalSessionError)> {
        self.last_driver_event_record()
            .filter(|record| record.sequence > observed_sequence)
            .map(|record| (record.sequence, record.activation_generation, record.error))
    }

    #[must_use]
    pub fn current_activation_generation(&self) -> u64 {
        self.activation_generation.load(Ordering::Acquire)
    }

    /// Close admission and cancel the owner without waiting.
    ///
    /// The shared service shutdown deadline is recorded before cancellation so
    /// the actor's own bounded close uses the service budget instead of a fresh
    /// internal window. The bounded join remains
    /// [`PortalSessionActor::shutdown_until`].
    pub fn begin_shutdown(&self, deadline: Instant) {
        {
            let mut admission = self
                .admission
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            admission.accepting = false;
            #[cfg(test)]
            self.wait_at_test_pause(&self.test_hooks.shutdown_closed);
        }
        *self
            .shutdown_deadline
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(deadline);
        self.owner_cancel.cancel();
    }

    /// Close admission, cancel any in-flight portal operation, and bounded-join
    /// the owner. This method is idempotent only in the fail-closed sense: a
    /// second call reports `WorkerStopped` rather than manufacturing success.
    pub fn shutdown(&self, timeout: Duration) -> Result<PortalActorShutdown, PortalActorFailure> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .unwrap_or_else(Instant::now);
        self.shutdown_until(deadline)
    }

    /// [`PortalSessionActor::shutdown`] bounded by one absolute deadline.
    ///
    /// Admission closure and cancellation happen even when the deadline has
    /// already passed; only the bounded join reports the timeout truthfully.
    pub fn shutdown_until(
        &self,
        deadline: Instant,
    ) -> Result<PortalActorShutdown, PortalActorFailure> {
        self.begin_shutdown(deadline);

        let worker = self
            .worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
            .ok_or(PortalActorFailure::WorkerStopped)?;
        let worker_stop = worker
            .stop(remaining(deadline))
            .map_err(|_| PortalActorFailure::WorkerFailed)?;
        let cleanup = match worker_stop {
            WorkerStop::TimedOut => PortalCleanupStatus::Failed,
            WorkerStop::Stopped => self
                .terminal
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take()
                .and_then(|receiver| receiver.try_recv().ok())
                .map(|terminal| terminal.cleanup)
                .unwrap_or(PortalCleanupStatus::Failed),
        };
        Ok(PortalActorShutdown {
            worker: worker_stop,
            cleanup,
        })
    }

    fn enqueue(
        &self,
        command: PortalActorCommand,
        deadline: Instant,
    ) -> Result<(), PortalActorFailure> {
        let admission = self
            .admission
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !admission.accepting {
            return Err(PortalActorFailure::AdmissionClosed);
        }
        if Instant::now() >= deadline {
            return Err(PortalActorFailure::TimedOut);
        }
        match admission.commands.try_send(command) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(PortalActorFailure::Busy),
            Err(TrySendError::Disconnected(_)) => Err(PortalActorFailure::WorkerStopped),
        }
    }

    #[cfg(test)]
    fn pause_after_activation_for_test(&self) -> ActorTestPause {
        self.install_test_pause(&self.test_hooks.after_activation)
    }

    #[cfg(test)]
    fn pause_after_driver_poll_for_test(&self) -> ActorTestPause {
        self.install_test_pause(&self.test_hooks.after_driver_poll)
    }

    #[cfg(test)]
    fn pause_after_submission_for_test(&self) -> ActorTestPause {
        self.install_test_pause(&self.test_hooks.after_submission)
    }

    #[cfg(test)]
    fn pause_shutdown_after_admission_close_for_test(&self) -> ActorTestPause {
        self.install_test_pause(&self.test_hooks.shutdown_closed)
    }

    #[cfg(test)]
    fn install_test_pause(&self, slot: &Mutex<Option<ActorPause>>) -> ActorTestPause {
        let (reached_tx, reached_rx) = sync_channel(0);
        let (resume_tx, resume_rx) = sync_channel(0);
        let mut guard = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        assert!(guard.is_none(), "portal actor test pause already installed");
        *guard = Some(ActorPause {
            reached: reached_tx,
            resume: resume_rx,
        });
        ActorTestPause {
            reached: reached_rx,
            resume: resume_tx,
        }
    }

    #[cfg(test)]
    fn wait_at_test_pause(&self, slot: &Mutex<Option<ActorPause>>) {
        wait_at_test_pause(slot);
    }
}

/// Content-free service-level failure for explicitly injected IPC composition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortalPermissionServiceFailure {
    AdmissionClosed,
    Busy,
    InvalidOperation,
    Actor(PortalActorFailure),
}

struct PortalPermissionState {
    accepting: bool,
    pending_operation: Option<String>,
    pending_cancel: Option<SessionCancellation>,
    last_operation: Option<String>,
    revoking: bool,
    status: PortalKeyboardStatusDto,
    observed_driver_sequence: u64,
    active_generation: u64,
}

/// Generic daemon-owned permission coordinator.
///
/// Startup instantiates this type after a read-only eligibility probe. With
/// the direct driver it owns the reviewed permission transport, while status
/// still reports sender/delivery capability as false until native behavior is
/// separately qualified.
pub struct PortalPermissionService {
    actor: Arc<PortalSessionActor>,
    state: Mutex<PortalPermissionState>,
    #[cfg(test)]
    after_actor_result: Mutex<Option<ActorPause>>,
    #[cfg(test)]
    after_revoke_marked: Mutex<Option<ActorPause>>,
}

/// Delivery-side handle that can submit only through the daemon-owned portal
/// service. It cannot activate permission, forge a parent window, or outlive
/// the service's actor-owned lifecycle.
#[derive(Clone)]
pub struct PortalPermissionSubmitter {
    service: Arc<PortalPermissionService>,
}

impl PortalPermissionSubmitter {
    #[must_use]
    pub fn new(service: Arc<PortalPermissionService>) -> Self {
        Self { service }
    }
}

impl PortalEisTextSubmitter for PortalPermissionSubmitter {
    fn submit(
        &mut self,
        payload: PortalEisTextPayload,
        cancellation: &SessionCancellation,
        deadline: Instant,
    ) -> PortalEisTextSubmission {
        self.service
            .submit_text(payload, deadline, cancellation.clone())
    }
}

impl PortalPermissionService {
    pub fn spawn<D>(
        supervisor: &Arc<WorkerSupervisor>,
        driver: D,
        eligibility: PortalEligibilityDto,
    ) -> Result<Self, PortalActorFailure>
    where
        D: PortalSessionDriver + 'static,
    {
        let can_request = eligibility == PortalEligibilityDto::ReadyToRequest;
        Ok(Self {
            actor: Arc::new(PortalSessionActor::spawn(supervisor, driver)?),
            state: Mutex::new(PortalPermissionState {
                accepting: true,
                pending_operation: None,
                pending_cancel: None,
                last_operation: None,
                revoking: false,
                status: PortalKeyboardStatusDto {
                    eligibility,
                    lifecycle: if can_request {
                        PortalLifecycleDto::Idle
                    } else {
                        PortalLifecycleDto::Unavailable
                    },
                    can_request,
                    permission_active: false,
                    transport_connected: false,
                    sender_available: false,
                    delivery_qualified: false,
                    current_operation_id: None,
                    last_failure: None,
                    cleanup: PortalCleanupDto::NotRequired,
                    sequence: 0,
                },
                observed_driver_sequence: 0,
                active_generation: 0,
            }),
            #[cfg(test)]
            after_actor_result: Mutex::new(None),
            #[cfg(test)]
            after_revoke_marked: Mutex::new(None),
        })
    }

    #[must_use]
    pub fn snapshot(&self) -> PortalKeyboardStatusDto {
        self.refresh_driver_event();
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .status
            .clone()
    }

    pub fn request(
        &self,
        operation_id: String,
        parent: PortalParentWindow,
        deadline: Instant,
    ) -> Result<PortalKeyboardStatusDto, PortalPermissionServiceFailure> {
        if operation_id.is_empty()
            || operation_id.len() > narrivy_ipc::MAX_PORTAL_OPERATION_ID_BYTES
            || !operation_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(PortalPermissionServiceFailure::InvalidOperation);
        }
        self.refresh_driver_event();
        let request_cancel = SessionCancellation::new();
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !state.accepting {
                return Err(PortalPermissionServiceFailure::AdmissionClosed);
            }
            if let Some(pending) = &state.pending_operation {
                return if pending == &operation_id {
                    Ok(state.status.clone())
                } else {
                    Err(PortalPermissionServiceFailure::Busy)
                };
            }
            if state.last_operation.as_deref() == Some(operation_id.as_str()) {
                return Ok(state.status.clone());
            }
            if state.status.lifecycle == PortalLifecycleDto::Invalid {
                return Err(PortalPermissionServiceFailure::InvalidOperation);
            }
            if state.status.permission_active {
                return Err(PortalPermissionServiceFailure::Busy);
            }
            if !state.status.can_request {
                return Err(PortalPermissionServiceFailure::AdmissionClosed);
            }
            state.pending_operation = Some(operation_id.clone());
            state.pending_cancel = Some(request_cancel.clone());
            state.status.lifecycle = PortalLifecycleDto::Requesting;
            state.status.can_request = false;
            state.status.current_operation_id = Some(operation_id.clone());
            state.status.last_failure = None;
            state.status.cleanup = PortalCleanupDto::NotRequired;
            bump_status(&mut state.status);
        }

        let result = self.actor.activate(parent, deadline, request_cancel);
        let activation_generation = self.actor.current_activation_generation();
        #[cfg(test)]
        wait_at_test_pause(&self.after_actor_result);
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let owns_completion = state.pending_operation.as_deref() == Some(operation_id.as_str());
        if owns_completion {
            state.pending_operation = None;
            state.pending_cancel = None;
            state.last_operation = Some(operation_id);
        }
        if owns_completion && !state.revoking {
            match result {
                Ok(()) => {
                    state.active_generation = activation_generation;
                    set_active(&mut state.status);
                }
                Err(error) => set_actor_failure(&mut state.status, error),
            }
            bump_status(&mut state.status);
        }
        drop(state);
        self.refresh_driver_event();
        let status = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .status
            .clone();
        match result {
            Ok(()) => Ok(status),
            Err(error) => Err(PortalPermissionServiceFailure::Actor(error)),
        }
    }

    pub fn cancel(
        &self,
        operation_id: &str,
    ) -> Result<PortalKeyboardStatusDto, PortalPermissionServiceFailure> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match state.pending_operation.as_deref() {
            Some(pending) if pending != operation_id => {
                return Err(PortalPermissionServiceFailure::Busy);
            }
            Some(_) => {
                if let Some(cancel) = &state.pending_cancel {
                    cancel.cancel();
                }
                if state.status.lifecycle != PortalLifecycleDto::Closing {
                    state.status.lifecycle = PortalLifecycleDto::Closing;
                    state.status.can_request = false;
                    state.status.last_failure = Some(PortalFailureDto::Cancelled);
                    bump_status(&mut state.status);
                }
            }
            None => {}
        }
        Ok(state.status.clone())
    }

    /// Submit one validated text frame through the already-active grant.
    /// This method never requests permission and never upgrades local handoff
    /// to verified delivery. Lifecycle changes caused by transport failure or
    /// revocation are reconciled into the next status snapshot/event. Revoke
    /// closes service admission before actor close, so a submit cannot begin
    /// after revoke has linearized.
    pub fn submit_text(
        &self,
        payload: PortalEisTextPayload,
        deadline: Instant,
        request_cancel: SessionCancellation,
    ) -> PortalEisTextSubmission {
        self.refresh_driver_event();
        let active = {
            let state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.accepting
                && !state.revoking
                && state.pending_operation.is_none()
                && state.status.lifecycle == PortalLifecycleDto::Active
                && state.status.permission_active
                && state.status.transport_connected
        };
        if !active {
            return PortalEisTextSubmission::RejectedBeforeDispatch(
                PortalEisTextFailure::NotActive,
            );
        }
        let outcome = self.actor.submit_text(payload, deadline, request_cancel);
        self.refresh_driver_event();
        outcome
    }

    pub fn revoke(
        &self,
        deadline: Instant,
    ) -> Result<PortalKeyboardStatusDto, PortalPermissionServiceFailure> {
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.pending_operation.is_none()
                && !state.status.permission_active
                && state.status.lifecycle == PortalLifecycleDto::Idle
            {
                state.status.cleanup = PortalCleanupDto::NotRequired;
                return Ok(state.status.clone());
            }
            state.revoking = true;
            if let Some(cancel) = &state.pending_cancel {
                cancel.cancel();
            }
            state.status.lifecycle = PortalLifecycleDto::Closing;
            state.status.can_request = false;
            bump_status(&mut state.status);
        }
        #[cfg(test)]
        wait_at_test_pause(&self.after_revoke_marked);
        let result = self.actor.close(deadline);
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.revoking = false;
        state.pending_operation = None;
        state.pending_cancel = None;
        let driver_event_applied = result.is_ok() && self.apply_driver_event(&mut state);
        if !driver_event_applied {
            match result {
                Ok(PortalCloseResult::Closed) => {
                    set_idle(&mut state.status, PortalCleanupDto::ConfirmedClosed)
                }
                Ok(PortalCloseResult::AlreadyIdle) => {
                    set_idle(&mut state.status, PortalCleanupDto::NotRequired)
                }
                Err(error) => set_actor_failure(&mut state.status, error),
            }
            bump_status(&mut state.status);
        }
        match result {
            Ok(_) => Ok(state.status.clone()),
            Err(error) => Err(PortalPermissionServiceFailure::Actor(error)),
        }
    }

    /// Return a sequenced lifecycle event only when newer than `after`.
    #[must_use]
    pub fn event_after(&self, after: u64) -> Option<Event> {
        let status = self.snapshot();
        (status.sequence > after).then_some(Event::PortalKeyboardPermissionChanged {
            seq: status.sequence,
            status,
        })
    }

    /// Close service admission and cancel the actor owner without waiting.
    ///
    /// The shared shutdown deadline reaches the actor before cancellation, so
    /// the actor's own bounded close cannot start a fresh internal window.
    /// [`PortalPermissionService::shutdown_until`] performs the bounded join.
    pub fn begin_shutdown(&self, deadline: Instant) {
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.accepting = false;
            state.revoking = true;
            if let Some(cancel) = &state.pending_cancel {
                cancel.cancel();
            }
            state.status.lifecycle = PortalLifecycleDto::Closing;
            state.status.can_request = false;
            bump_status(&mut state.status);
        }
        self.actor.begin_shutdown(deadline);
    }

    pub fn shutdown(
        &self,
        timeout: Duration,
    ) -> Result<PortalActorShutdown, PortalPermissionServiceFailure> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .unwrap_or_else(Instant::now);
        self.shutdown_until(deadline)
    }

    /// [`PortalPermissionService::shutdown`] bounded by one absolute deadline.
    ///
    /// Admission closure and cancellation happen even when the deadline has
    /// already passed; the bounded join reports an unconfirmed actor stop as a
    /// failure instead of manufacturing success.
    pub fn shutdown_until(
        &self,
        deadline: Instant,
    ) -> Result<PortalActorShutdown, PortalPermissionServiceFailure> {
        self.begin_shutdown(deadline);
        let outcome = self
            .actor
            .shutdown_until(deadline)
            .map_err(PortalPermissionServiceFailure::Actor)?;
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.pending_operation = None;
        state.pending_cancel = None;
        if outcome.cleanup == PortalCleanupStatus::Failed {
            state.status.lifecycle = PortalLifecycleDto::Invalid;
            state.status.cleanup = PortalCleanupDto::Failed;
            state.status.last_failure = Some(PortalFailureDto::CloseFailed);
        } else {
            set_idle(&mut state.status, cleanup_dto(outcome.cleanup));
            state.status.can_request = false;
        }
        bump_status(&mut state.status);
        Ok(outcome)
    }

    fn refresh_driver_event(&self) {
        let (observed, accepting, revoking) = {
            let state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            (
                state.observed_driver_sequence,
                state.accepting,
                state.revoking,
            )
        };
        if !accepting || revoking {
            return;
        }
        let Some((driver_sequence, event_generation, event)) =
            self.actor.driver_event_since(observed)
        else {
            return;
        };
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if driver_sequence <= state.observed_driver_sequence {
            return;
        }
        // Activation and its matching portal event are one service-level
        // transition. Do not consume the event while the actor result is still
        // waiting to commit, or the later Active commit could overwrite a
        // revocation that has already closed the actor lifecycle.
        if state.pending_operation.is_some()
            && state.status.lifecycle == PortalLifecycleDto::Requesting
        {
            return;
        }
        self.apply_driver_event_record(&mut state, driver_sequence, event_generation, event);
    }

    fn apply_driver_event(&self, state: &mut PortalPermissionState) -> bool {
        let Some((driver_sequence, event_generation, event)) = self
            .actor
            .driver_event_since(state.observed_driver_sequence)
        else {
            return false;
        };
        self.apply_driver_event_record(state, driver_sequence, event_generation, event)
    }

    fn apply_driver_event_record(
        &self,
        state: &mut PortalPermissionState,
        driver_sequence: u64,
        event_generation: u64,
        event: PortalSessionError,
    ) -> bool {
        if driver_sequence <= state.observed_driver_sequence {
            return false;
        }
        state.observed_driver_sequence = driver_sequence;
        if !state.status.permission_active || event_generation != state.active_generation {
            return false;
        }
        state.pending_operation = None;
        state.pending_cancel = None;
        set_actor_error(&mut state.status, event);
        bump_status(&mut state.status);
        true
    }

    #[cfg(test)]
    fn pause_after_actor_result_for_test(&self) -> ActorTestPause {
        install_test_pause(&self.after_actor_result)
    }

    #[cfg(test)]
    fn pause_after_revoke_marked_for_test(&self) -> ActorTestPause {
        install_test_pause(&self.after_revoke_marked)
    }
}

/// Forward content-free portal lifecycle changes to connected IPC clients.
/// The caller owns the returned thread and must set `shutdown` and join it
/// before dropping the service. New clients recover current truth via
/// `GetStatus`; failed publication is not retried indefinitely.
pub fn spawn_portal_event_pump(
    server: Arc<UnixIpcServer>,
    service: Arc<PortalPermissionService>,
    shutdown: Arc<AtomicBool>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut observed = 0;
        while !shutdown.load(Ordering::Acquire) {
            let _ = pump_portal_event_once(&service, &mut observed, |event| {
                server.publish_event(event).map(|_| ())
            });
            std::thread::sleep(PORTAL_EVENT_POLL_INTERVAL);
        }
    })
}

fn pump_portal_event_once<E>(
    service: &PortalPermissionService,
    observed: &mut u64,
    mut publish: impl FnMut(&Event) -> Result<(), E>,
) -> Result<bool, E> {
    let Some(event) = service.event_after(*observed) else {
        return Ok(false);
    };
    let Event::PortalKeyboardPermissionChanged { seq, .. } = &event else {
        return Ok(false);
    };
    // Advance before publishing so a disconnected client set cannot turn one
    // lifecycle transition into an unbounded retry/log loop. GetStatus remains
    // the authoritative resynchronization path for later clients.
    *observed = *seq;
    publish(&event)?;
    Ok(true)
}

fn bump_status(status: &mut PortalKeyboardStatusDto) {
    status.sequence = status.sequence.saturating_add(1);
    status.sender_available = false;
    status.delivery_qualified = false;
}

fn set_active(status: &mut PortalKeyboardStatusDto) {
    status.lifecycle = PortalLifecycleDto::Active;
    status.can_request = false;
    status.permission_active = true;
    status.transport_connected = true;
    status.current_operation_id = None;
    status.last_failure = None;
    status.cleanup = PortalCleanupDto::NotRequired;
}

fn set_idle(status: &mut PortalKeyboardStatusDto, cleanup: PortalCleanupDto) {
    status.lifecycle = PortalLifecycleDto::Idle;
    status.can_request = status.eligibility == PortalEligibilityDto::ReadyToRequest;
    status.permission_active = false;
    status.transport_connected = false;
    status.current_operation_id = None;
    status.cleanup = cleanup;
}

fn set_actor_failure(status: &mut PortalKeyboardStatusDto, failure: PortalActorFailure) {
    match failure {
        PortalActorFailure::Lifecycle(error) => set_actor_error(status, error),
        PortalActorFailure::TimedOut => {
            status.last_failure = Some(PortalFailureDto::TimedOut);
            status.lifecycle = PortalLifecycleDto::Invalid;
            status.can_request = false;
            status.permission_active = false;
            status.transport_connected = false;
            status.current_operation_id = None;
            status.cleanup = PortalCleanupDto::Failed;
        }
        PortalActorFailure::AdmissionClosed | PortalActorFailure::WorkerStopped => {
            status.last_failure = Some(PortalFailureDto::Invalidated);
            status.lifecycle = PortalLifecycleDto::Invalid;
            status.can_request = false;
            status.permission_active = false;
            status.transport_connected = false;
            status.current_operation_id = None;
            status.cleanup = PortalCleanupDto::Failed;
        }
        PortalActorFailure::Busy => {
            status.last_failure = Some(PortalFailureDto::AlreadyActive);
        }
        PortalActorFailure::WorkerFailed => {
            status.last_failure = Some(PortalFailureDto::TransportFailed);
            status.lifecycle = PortalLifecycleDto::Invalid;
            status.can_request = false;
            status.permission_active = false;
            status.transport_connected = false;
            status.current_operation_id = None;
            status.cleanup = PortalCleanupDto::Failed;
        }
    }
}

fn set_actor_error(status: &mut PortalKeyboardStatusDto, error: PortalSessionError) {
    status.last_failure = Some(failure_dto(error.failure));
    status.permission_active = false;
    status.transport_connected = false;
    status.current_operation_id = None;
    status.cleanup = cleanup_dto(error.cleanup);
    if error.cleanup == PortalCleanupStatus::Failed
        || error.failure == PortalSessionFailure::Invalidated
    {
        status.lifecycle = PortalLifecycleDto::Invalid;
        status.can_request = false;
    } else {
        status.lifecycle = PortalLifecycleDto::Idle;
        status.can_request = status.eligibility == PortalEligibilityDto::ReadyToRequest;
    }
}

fn failure_dto(failure: PortalSessionFailure) -> PortalFailureDto {
    match failure {
        PortalSessionFailure::InvalidParentWindow => PortalFailureDto::InvalidParent,
        PortalSessionFailure::AlreadyActive => PortalFailureDto::AlreadyActive,
        PortalSessionFailure::Invalidated => PortalFailureDto::Invalidated,
        PortalSessionFailure::NotActive => PortalFailureDto::NotActive,
        PortalSessionFailure::Cancelled => PortalFailureDto::Cancelled,
        PortalSessionFailure::TimedOut => PortalFailureDto::TimedOut,
        PortalSessionFailure::PermissionDenied => PortalFailureDto::PermissionDenied,
        PortalSessionFailure::PortalUnavailable => PortalFailureDto::PortalUnavailable,
        PortalSessionFailure::RequestFailed => PortalFailureDto::RequestFailed,
        PortalSessionFailure::TransportFailed => PortalFailureDto::TransportFailed,
        PortalSessionFailure::Revoked => PortalFailureDto::Revoked,
        PortalSessionFailure::CloseFailed => PortalFailureDto::CloseFailed,
    }
}

fn cleanup_dto(cleanup: PortalCleanupStatus) -> PortalCleanupDto {
    match cleanup {
        PortalCleanupStatus::NotRequired => PortalCleanupDto::NotRequired,
        PortalCleanupStatus::ConfirmedClosed => PortalCleanupDto::ConfirmedClosed,
        PortalCleanupStatus::Failed => PortalCleanupDto::Failed,
    }
}

impl Drop for PortalSessionActor {
    fn drop(&mut self) {
        self.admission
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .accepting = false;
        let deadline = Instant::now() + DROP_CLOSE_BUDGET;
        *self
            .shutdown_deadline
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(deadline);
        self.owner_cancel.cancel();
    }
}

fn run_actor<D: PortalSessionDriver>(
    mut lifecycle: PortalSessionLifecycle<D>,
    commands: Receiver<PortalActorCommand>,
    runtime: PortalActorRuntime,
) {
    while !runtime.owner_cancel.is_cancelled() {
        let event_deadline = Instant::now() + DRIVER_EVENT_CLOSE_BUDGET;
        match lifecycle.poll_driver_event(event_deadline) {
            Ok(Some(event)) | Err(event) => {
                record_driver_event(
                    &runtime.last_driver_event,
                    runtime.activation_generation.load(Ordering::Acquire),
                    event,
                );
            }
            Ok(None) => {}
        }
        #[cfg(test)]
        wait_at_test_pause(&runtime.test_hooks.after_driver_poll);
        match commands.recv_timeout(ACTOR_POLL_INTERVAL) {
            Ok(command) => handle_command(
                &mut lifecycle,
                &runtime.owner_cancel,
                command,
                &runtime.last_driver_event,
                &runtime.activation_generation,
                #[cfg(test)]
                &runtime.test_hooks,
            ),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }

    let deadline = runtime
        .shutdown_deadline
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .unwrap_or_else(|| Instant::now() + DROP_CLOSE_BUDGET);
    let cleanup = match lifecycle.close(deadline) {
        Ok(PortalCloseResult::AlreadyIdle) => PortalCleanupStatus::NotRequired,
        Ok(PortalCloseResult::Closed) => PortalCleanupStatus::ConfirmedClosed,
        Err(error) => error.cleanup,
    };
    let _ = runtime.terminal.send(PortalActorTerminal { cleanup });
}

fn handle_command<D: PortalSessionDriver>(
    lifecycle: &mut PortalSessionLifecycle<D>,
    owner_cancel: &SessionCancellation,
    command: PortalActorCommand,
    last_driver_event: &Arc<Mutex<Option<PortalDriverEventRecord>>>,
    activation_generation: &Arc<AtomicU64>,
    #[cfg(test)] test_hooks: &PortalActorTestHooks,
) {
    match command {
        PortalActorCommand::Activate {
            parent,
            deadline,
            request_cancel,
            reply,
        } => {
            activation_generation.fetch_add(1, Ordering::AcqRel);
            let cancellation =
                CombinedPortalCancellation::new(request_cancel, owner_cancel.clone());
            let result = lifecycle.activate(&parent, deadline, &cancellation);
            #[cfg(test)]
            wait_at_test_pause(&test_hooks.after_activation);
            let activated = result.is_ok();
            if reply.send(result).is_err() && activated {
                let _ = lifecycle.close(deadline);
            }
        }
        PortalActorCommand::SubmissionAllowed {
            deadline,
            request_cancel,
            reply,
        } => {
            let cancellation =
                CombinedPortalCancellation::new(request_cancel, owner_cancel.clone());
            let result = lifecycle.submission_allowed(&cancellation, deadline);
            if let Err(error) = result
                && error.failure == PortalSessionFailure::Revoked
            {
                record_driver_event(
                    last_driver_event,
                    activation_generation.load(Ordering::Acquire),
                    error,
                );
            }
            let allowed = result.is_ok();
            if reply.send(result).is_err() && allowed {
                let _ = lifecycle.close(deadline);
            }
        }
        PortalActorCommand::SubmitText {
            payload,
            deadline,
            request_cancel,
            reply,
        } => {
            let cancellation =
                CombinedPortalCancellation::new(request_cancel, owner_cancel.clone());
            let outcome = lifecycle.submit_text(payload, &cancellation, deadline);
            if let Some(event) = submission_lifecycle_event(outcome, lifecycle.state()) {
                record_driver_event(
                    last_driver_event,
                    activation_generation.load(Ordering::Acquire),
                    event,
                );
            }
            #[cfg(test)]
            wait_at_test_pause(&test_hooks.after_submission);
            let dispatched = matches!(
                outcome,
                PortalEisTextSubmission::SubmittedUnverified
                    | PortalEisTextSubmission::DispatchUncertain(_)
            );
            if reply.send(outcome).is_err() && dispatched {
                let event = match lifecycle.close(deadline) {
                    Ok(PortalCloseResult::Closed) => PortalSessionError {
                        failure: PortalSessionFailure::TimedOut,
                        cleanup: PortalCleanupStatus::ConfirmedClosed,
                    },
                    Ok(PortalCloseResult::AlreadyIdle) => PortalSessionError {
                        failure: PortalSessionFailure::TimedOut,
                        cleanup: PortalCleanupStatus::NotRequired,
                    },
                    Err(error) => error,
                };
                record_driver_event(
                    last_driver_event,
                    activation_generation.load(Ordering::Acquire),
                    event,
                );
            }
        }
        PortalActorCommand::Close { deadline, reply } => {
            let _ = reply.send(lifecycle.close(deadline));
        }
        PortalActorCommand::ObserveRevoked { reply } => {
            let _ = reply.send(lifecycle.observe_revoked());
        }
        PortalActorCommand::State { reply } => {
            let _ = reply.send(lifecycle.state());
        }
    }
}

fn record_driver_event(
    last_driver_event: &Arc<Mutex<Option<PortalDriverEventRecord>>>,
    activation_generation: u64,
    event: PortalSessionError,
) {
    let mut last = last_driver_event
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let sequence = last
        .map(|record| record.sequence)
        .unwrap_or(0)
        .saturating_add(1);
    *last = Some(PortalDriverEventRecord {
        sequence,
        activation_generation,
        error: event,
    });
}

fn receive_until<T>(receiver: Receiver<T>, deadline: Instant) -> Result<T, PortalActorFailure> {
    match receiver.recv_timeout(remaining(deadline)) {
        Ok(value) => Ok(value),
        Err(RecvTimeoutError::Timeout) => Err(PortalActorFailure::TimedOut),
        Err(RecvTimeoutError::Disconnected) => Err(PortalActorFailure::WorkerStopped),
    }
}

fn remaining(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}

fn text_failure_from_actor(failure: PortalActorFailure) -> PortalEisTextFailure {
    match failure {
        PortalActorFailure::TimedOut => PortalEisTextFailure::TimedOut,
        PortalActorFailure::Lifecycle(PortalSessionError { failure, .. }) => match failure {
            PortalSessionFailure::Cancelled => PortalEisTextFailure::Cancelled,
            PortalSessionFailure::TimedOut => PortalEisTextFailure::TimedOut,
            PortalSessionFailure::Revoked => PortalEisTextFailure::Revoked,
            PortalSessionFailure::CloseFailed => PortalEisTextFailure::CloseFailed,
            PortalSessionFailure::NotActive | PortalSessionFailure::AlreadyActive => {
                PortalEisTextFailure::NotActive
            }
            _ => PortalEisTextFailure::TransportFailed,
        },
        PortalActorFailure::AdmissionClosed
        | PortalActorFailure::Busy
        | PortalActorFailure::WorkerStopped
        | PortalActorFailure::WorkerFailed => PortalEisTextFailure::TransportFailed,
    }
}

fn submission_lifecycle_event(
    outcome: PortalEisTextSubmission,
    state: PortalSessionState,
) -> Option<PortalSessionError> {
    let cleanup = match state {
        PortalSessionState::Idle => PortalCleanupStatus::ConfirmedClosed,
        PortalSessionState::Invalid => PortalCleanupStatus::Failed,
        _ => return None,
    };
    let failure = match outcome {
        PortalEisTextSubmission::CancelledBeforeDispatch => PortalSessionFailure::Cancelled,
        PortalEisTextSubmission::RejectedBeforeDispatch(reason)
        | PortalEisTextSubmission::DispatchUncertain(reason) => match reason {
            PortalEisTextFailure::Cancelled => PortalSessionFailure::Cancelled,
            PortalEisTextFailure::TimedOut => PortalSessionFailure::TimedOut,
            PortalEisTextFailure::Revoked => PortalSessionFailure::Revoked,
            PortalEisTextFailure::CloseFailed => PortalSessionFailure::CloseFailed,
            PortalEisTextFailure::PayloadEmpty
            | PortalEisTextFailure::PayloadTooLong
            | PortalEisTextFailure::PayloadUnsupported
            | PortalEisTextFailure::SenderUnavailable
            | PortalEisTextFailure::NotActive
            | PortalEisTextFailure::TransportFailed => PortalSessionFailure::TransportFailed,
        },
        PortalEisTextSubmission::SubmittedUnverified => return None,
    };
    Some(PortalSessionError { failure, cleanup })
}

#[cfg(test)]
fn install_test_pause(slot: &Mutex<Option<ActorPause>>) -> ActorTestPause {
    let (reached_tx, reached_rx) = sync_channel(0);
    let (resume_tx, resume_rx) = sync_channel(0);
    let mut guard = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    assert!(
        guard.is_none(),
        "portal service test pause already installed"
    );
    *guard = Some(ActorPause {
        reached: reached_tx,
        resume: resume_rx,
    });
    ActorTestPause {
        reached: reached_rx,
        resume: resume_tx,
    }
}

#[cfg(test)]
fn wait_at_test_pause(slot: &Mutex<Option<ActorPause>>) {
    let pause = slot
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    if let Some(pause) = pause {
        let _ = pause.reached.send(());
        let _ = pause.resume.recv();
    }
}

#[cfg(test)]
impl ActorTestPause {
    fn wait_until_reached(&self) {
        let result = self.reached.recv_timeout(Duration::from_secs(1));
        assert!(
            result.is_ok(),
            "portal actor test pause was not reached: {result:?}"
        );
    }

    fn release(&self) {
        let result = self.resume.send(());
        assert!(
            result.is_ok(),
            "portal actor test pause receiver dropped: {result:?}"
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use narrivy_platform_linux::portal_eis::{
        PortalCancellation, PortalDriverEvent, PortalSessionFailure,
    };
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    struct DriverFacts {
        calls: Mutex<Vec<&'static str>>,
        live: AtomicBool,
        request_entered: AtomicBool,
        aborts: AtomicUsize,
        next_event: Mutex<Option<Result<PortalDriverEvent, PortalSessionFailure>>>,
        submit_result: Mutex<PortalEisTextSubmission>,
        submitted_bytes: Mutex<Vec<Vec<u8>>>,
    }

    impl Default for DriverFacts {
        fn default() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                live: AtomicBool::new(false),
                request_entered: AtomicBool::new(false),
                aborts: AtomicUsize::new(0),
                next_event: Mutex::new(None),
                submit_result: Mutex::new(PortalEisTextSubmission::SubmittedUnverified),
                submitted_bytes: Mutex::new(Vec::new()),
            }
        }
    }

    struct ControlledDriver {
        facts: Arc<DriverFacts>,
        block_request_until_cancelled: bool,
        close_result: Result<(), PortalSessionFailure>,
    }

    impl ControlledDriver {
        fn normal(facts: Arc<DriverFacts>) -> Self {
            Self {
                facts,
                block_request_until_cancelled: false,
                close_result: Ok(()),
            }
        }
    }

    impl PortalSessionDriver for ControlledDriver {
        fn request_keyboard_grant(
            &mut self,
            _parent: &PortalParentWindow,
            deadline: Instant,
            cancellation: &dyn PortalCancellation,
        ) -> Result<(), PortalSessionFailure> {
            self.facts
                .calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push("request");
            self.facts.request_entered.store(true, Ordering::SeqCst);
            while self.block_request_until_cancelled && !cancellation.is_cancelled() {
                if Instant::now() >= deadline {
                    return Err(PortalSessionFailure::TimedOut);
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            if cancellation.is_cancelled() {
                return Err(PortalSessionFailure::Cancelled);
            }
            self.facts.live.store(true, Ordering::SeqCst);
            Ok(())
        }

        fn connect_transport(
            &mut self,
            _deadline: Instant,
            cancellation: &dyn PortalCancellation,
        ) -> Result<(), PortalSessionFailure> {
            self.facts
                .calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push("connect");
            if cancellation.is_cancelled() {
                Err(PortalSessionFailure::Cancelled)
            } else {
                Ok(())
            }
        }

        fn close(&mut self, _deadline: Instant) -> Result<(), PortalSessionFailure> {
            self.facts
                .calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push("close");
            if self.close_result.is_ok() {
                self.facts.live.store(false, Ordering::SeqCst);
            }
            self.close_result
        }

        fn poll_event(&mut self) -> Result<Option<PortalDriverEvent>, PortalSessionFailure> {
            self.facts
                .next_event
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take()
                .transpose()
        }

        fn submit_text(
            &mut self,
            payload: PortalEisTextPayload,
            _deadline: Instant,
            cancellation: &dyn PortalCancellation,
        ) -> PortalEisTextSubmission {
            self.facts
                .calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push("submit");
            if cancellation.is_cancelled() {
                return PortalEisTextSubmission::CancelledBeforeDispatch;
            }
            self.facts
                .submitted_bytes
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(payload.as_bytes().to_vec());
            *self
                .facts
                .submit_result
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        }

        fn abort_local(&mut self) {
            self.facts
                .calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push("abort");
            self.facts.live.store(false, Ordering::SeqCst);
            self.facts.aborts.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn parent() -> PortalParentWindow {
        PortalParentWindow::parse("wayland:controlled-parent").unwrap()
    }

    fn deadline() -> Instant {
        Instant::now() + Duration::from_secs(1)
    }

    fn wait_until(predicate: impl Fn() -> bool) {
        let deadline = deadline();
        while !predicate() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(predicate(), "controlled condition was not reached");
    }

    #[test]
    fn actor_serializes_activation_submission_close_and_shutdown() {
        let supervisor = Arc::new(WorkerSupervisor::new());
        let facts = Arc::new(DriverFacts::default());
        let actor =
            PortalSessionActor::spawn(&supervisor, ControlledDriver::normal(Arc::clone(&facts)))
                .unwrap();

        actor
            .activate(parent(), deadline(), SessionCancellation::new())
            .unwrap();
        assert_eq!(actor.state(deadline()).unwrap(), PortalSessionState::Active);
        actor
            .submission_allowed(deadline(), SessionCancellation::new())
            .unwrap();
        assert_eq!(actor.close(deadline()).unwrap(), PortalCloseResult::Closed);
        assert_eq!(actor.state(deadline()).unwrap(), PortalSessionState::Idle);

        assert_eq!(
            actor.shutdown(Duration::from_secs(1)).unwrap(),
            PortalActorShutdown {
                worker: WorkerStop::Stopped,
                cleanup: PortalCleanupStatus::NotRequired,
            }
        );
        assert_eq!(supervisor.live_worker_count(), 0);
        assert!(!facts.live.load(Ordering::SeqCst));
        assert_eq!(
            *facts
                .calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            vec!["request", "connect", "close"]
        );
    }

    #[test]
    fn actor_preserves_portal_originated_revocation_as_typed_failure() {
        let supervisor = Arc::new(WorkerSupervisor::new());
        let facts = Arc::new(DriverFacts::default());
        let actor =
            PortalSessionActor::spawn(&supervisor, ControlledDriver::normal(Arc::clone(&facts)))
                .unwrap();
        actor
            .activate(parent(), deadline(), SessionCancellation::new())
            .unwrap();

        assert_eq!(
            actor.observe_revoked(deadline()).unwrap(),
            PortalSessionError {
                failure: PortalSessionFailure::Revoked,
                cleanup: PortalCleanupStatus::ConfirmedClosed,
            }
        );
        assert_eq!(actor.state(deadline()).unwrap(), PortalSessionState::Idle);
        assert_eq!(
            actor.shutdown(Duration::from_secs(1)).unwrap().cleanup,
            PortalCleanupStatus::NotRequired
        );
    }

    #[test]
    fn driver_revocation_is_polled_and_closes_active_authority() {
        let supervisor = Arc::new(WorkerSupervisor::new());
        let facts = Arc::new(DriverFacts::default());
        let actor =
            PortalSessionActor::spawn(&supervisor, ControlledDriver::normal(Arc::clone(&facts)))
                .unwrap();
        actor
            .activate(parent(), deadline(), SessionCancellation::new())
            .unwrap();
        *facts
            .next_event
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(Ok(PortalDriverEvent::Revoked));

        wait_until(|| actor.last_driver_event().is_some());
        assert_eq!(
            actor.last_driver_event(),
            Some(PortalSessionError {
                failure: PortalSessionFailure::Revoked,
                cleanup: PortalCleanupStatus::ConfirmedClosed,
            })
        );
        assert_eq!(actor.state(deadline()).unwrap(), PortalSessionState::Idle);
        assert!(!facts.live.load(Ordering::SeqCst));
        assert_eq!(
            actor.shutdown(Duration::from_secs(1)).unwrap().worker,
            WorkerStop::Stopped
        );
    }

    #[test]
    fn revocation_after_periodic_poll_is_rejected_by_submission_gate() {
        let supervisor = Arc::new(WorkerSupervisor::new());
        let facts = Arc::new(DriverFacts::default());
        let actor =
            PortalSessionActor::spawn(&supervisor, ControlledDriver::normal(Arc::clone(&facts)))
                .unwrap();
        actor
            .activate(parent(), deadline(), SessionCancellation::new())
            .unwrap();

        let pause = actor.pause_after_driver_poll_for_test();
        pause.wait_until_reached();
        *facts
            .next_event
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(Ok(PortalDriverEvent::Revoked));
        let (reply, result) = sync_channel(1);
        actor
            .enqueue(
                PortalActorCommand::SubmissionAllowed {
                    deadline: deadline(),
                    request_cancel: SessionCancellation::new(),
                    reply,
                },
                deadline(),
            )
            .unwrap();
        pause.release();

        let revoked = PortalSessionError {
            failure: PortalSessionFailure::Revoked,
            cleanup: PortalCleanupStatus::ConfirmedClosed,
        };
        assert_eq!(
            result.recv_timeout(Duration::from_secs(1)).unwrap(),
            Err(revoked)
        );
        assert_eq!(actor.last_driver_event(), Some(revoked));
        assert_eq!(actor.state(deadline()).unwrap(), PortalSessionState::Idle);
        assert!(!facts.live.load(Ordering::SeqCst));
        assert_eq!(
            *facts
                .calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            vec!["request", "connect", "abort"]
        );
        assert_eq!(
            actor.shutdown(Duration::from_secs(1)).unwrap().worker,
            WorkerStop::Stopped
        );
    }

    #[test]
    fn request_timeout_cancels_the_operation_and_releases_actor_resources() {
        let supervisor = Arc::new(WorkerSupervisor::new());
        let facts = Arc::new(DriverFacts::default());
        let driver = ControlledDriver {
            facts: Arc::clone(&facts),
            block_request_until_cancelled: true,
            close_result: Ok(()),
        };
        let actor = PortalSessionActor::spawn(&supervisor, driver).unwrap();
        // One absolute deadline governs both sides. Whichever waiter observes
        // it first may report actor-level timeout or the lifecycle's typed
        // timeout after confirmed cleanup; neither outcome claims success.
        assert!(matches!(
            actor.activate(
                parent(),
                Instant::now() + Duration::from_millis(20),
                SessionCancellation::new(),
            ),
            Err(PortalActorFailure::TimedOut)
                | Err(PortalActorFailure::Lifecycle(PortalSessionError {
                    failure: PortalSessionFailure::TimedOut,
                    cleanup: PortalCleanupStatus::ConfirmedClosed,
                }))
        ));
        wait_until(|| {
            facts
                .calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .contains(&"close")
        });
        assert_eq!(actor.state(deadline()).unwrap(), PortalSessionState::Idle);
        assert!(!facts.live.load(Ordering::SeqCst));
        assert_eq!(
            actor.shutdown(Duration::from_secs(1)).unwrap().worker,
            WorkerStop::Stopped
        );
    }

    #[test]
    fn activation_completion_after_caller_timeout_cannot_leave_active_authority() {
        let supervisor = Arc::new(WorkerSupervisor::new());
        let facts = Arc::new(DriverFacts::default());
        let actor = Arc::new(
            PortalSessionActor::spawn(&supervisor, ControlledDriver::normal(Arc::clone(&facts)))
                .unwrap(),
        );
        let pause = actor.pause_after_activation_for_test();
        let request_actor = Arc::clone(&actor);
        let (result_tx, result_rx) = sync_channel(1);
        std::thread::spawn(move || {
            let result = request_actor.activate(
                parent(),
                Instant::now() + Duration::from_millis(30),
                SessionCancellation::new(),
            );
            let _ = result_tx.send(result);
        });

        pause.wait_until_reached();
        assert_eq!(
            result_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            Err(PortalActorFailure::TimedOut)
        );
        assert!(facts.live.load(Ordering::SeqCst));
        pause.release();
        wait_until(|| !facts.live.load(Ordering::SeqCst));

        assert_eq!(actor.state(deadline()).unwrap(), PortalSessionState::Idle);
        assert_eq!(
            *facts
                .calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            vec!["request", "connect", "close"]
        );
        assert_eq!(
            actor.shutdown(Duration::from_secs(1)).unwrap().worker,
            WorkerStop::Stopped
        );
    }

    #[test]
    fn admitted_submission_reply_timeout_is_uncertain_and_closes_authority() {
        let supervisor = Arc::new(WorkerSupervisor::new());
        let facts = Arc::new(DriverFacts::default());
        let actor = Arc::new(
            PortalSessionActor::spawn(&supervisor, ControlledDriver::normal(Arc::clone(&facts)))
                .unwrap(),
        );
        actor
            .activate(parent(), deadline(), SessionCancellation::new())
            .unwrap();
        let pause = actor.pause_after_submission_for_test();
        let submit_actor = Arc::clone(&actor);
        let submit = std::thread::spawn(move || {
            submit_actor.submit_text(
                PortalEisTextPayload::parse("controlled-frame").unwrap(),
                Instant::now() + Duration::from_millis(30),
                SessionCancellation::new(),
            )
        });

        pause.wait_until_reached();
        assert_eq!(
            submit.join().unwrap(),
            PortalEisTextSubmission::DispatchUncertain(PortalEisTextFailure::TimedOut)
        );
        assert_eq!(
            *facts
                .submitted_bytes
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            vec![b"controlled-frame".to_vec()]
        );
        pause.release();
        wait_until(|| !facts.live.load(Ordering::SeqCst));
        wait_until(|| actor.last_driver_event().is_some());
        assert_eq!(actor.state(deadline()).unwrap(), PortalSessionState::Idle);
        let event = actor.last_driver_event().unwrap();
        assert_eq!(event.failure, PortalSessionFailure::TimedOut);
        assert_eq!(event.cleanup, PortalCleanupStatus::ConfirmedClosed);
        assert_eq!(
            *facts
                .calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            vec!["request", "connect", "submit", "close"]
        );
        assert_eq!(
            actor.shutdown(Duration::from_secs(1)).unwrap().worker,
            WorkerStop::Stopped
        );
        assert_eq!(supervisor.live_worker_count(), 0);
    }

    #[test]
    fn admission_closed_by_shutdown_rejects_a_contending_activation() {
        let supervisor = Arc::new(WorkerSupervisor::new());
        let facts = Arc::new(DriverFacts::default());
        let actor = Arc::new(
            PortalSessionActor::spawn(&supervisor, ControlledDriver::normal(Arc::clone(&facts)))
                .unwrap(),
        );
        let pause = actor.pause_shutdown_after_admission_close_for_test();
        let shutdown_actor = Arc::clone(&actor);
        let shutdown = std::thread::spawn(move || shutdown_actor.shutdown(Duration::from_secs(1)));
        pause.wait_until_reached();

        let request_actor = Arc::clone(&actor);
        let request = std::thread::spawn(move || {
            request_actor.activate(parent(), deadline(), SessionCancellation::new())
        });
        pause.release();

        assert_eq!(
            request.join().unwrap(),
            Err(PortalActorFailure::AdmissionClosed)
        );
        assert_eq!(
            shutdown.join().unwrap().unwrap().worker,
            WorkerStop::Stopped
        );
        assert!(
            facts
                .calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_empty()
        );
        assert_eq!(supervisor.live_worker_count(), 0);
    }

    #[test]
    fn bounded_actor_queue_reports_busy_without_unbounded_growth() {
        let supervisor = Arc::new(WorkerSupervisor::new());
        let facts = Arc::new(DriverFacts::default());
        let driver = ControlledDriver {
            facts: Arc::clone(&facts),
            block_request_until_cancelled: true,
            close_result: Ok(()),
        };
        let actor = Arc::new(PortalSessionActor::spawn(&supervisor, driver).unwrap());
        let request_actor = Arc::clone(&actor);
        let request = std::thread::spawn(move || {
            request_actor.activate(parent(), deadline(), SessionCancellation::new())
        });
        wait_until(|| facts.request_entered.load(Ordering::SeqCst));

        for _ in 0..ACTOR_COMMAND_CAPACITY {
            let (reply, _receiver) = sync_channel(1);
            actor
                .enqueue(PortalActorCommand::State { reply }, deadline())
                .unwrap();
        }
        let (reply, _receiver) = sync_channel(1);
        assert_eq!(
            actor.enqueue(PortalActorCommand::State { reply }, deadline()),
            Err(PortalActorFailure::Busy)
        );

        assert_eq!(
            actor.shutdown(Duration::from_secs(1)).unwrap().worker,
            WorkerStop::Stopped
        );
        assert!(matches!(
            request.join().unwrap(),
            Err(PortalActorFailure::Lifecycle(PortalSessionError {
                failure: PortalSessionFailure::Cancelled,
                ..
            }))
        ));
    }

    #[test]
    fn daemon_shutdown_cancels_an_inflight_permission_operation_and_joins() {
        let supervisor = Arc::new(WorkerSupervisor::new());
        let facts = Arc::new(DriverFacts::default());
        let driver = ControlledDriver {
            facts: Arc::clone(&facts),
            block_request_until_cancelled: true,
            close_result: Ok(()),
        };
        let actor = Arc::new(PortalSessionActor::spawn(&supervisor, driver).unwrap());
        let request_actor = Arc::clone(&actor);
        let request = std::thread::spawn(move || {
            request_actor.activate(parent(), deadline(), SessionCancellation::new())
        });
        wait_until(|| facts.request_entered.load(Ordering::SeqCst));

        assert_eq!(
            actor.shutdown(Duration::from_secs(1)).unwrap().worker,
            WorkerStop::Stopped
        );
        assert!(matches!(
            request.join().unwrap(),
            Err(PortalActorFailure::Lifecycle(PortalSessionError {
                failure: PortalSessionFailure::Cancelled,
                cleanup: PortalCleanupStatus::ConfirmedClosed,
            }))
        ));
        assert_eq!(supervisor.live_worker_count(), 0);
        assert!(!facts.live.load(Ordering::SeqCst));
    }

    #[test]
    fn failed_remote_close_is_reported_and_drop_aborts_local_authority() {
        let supervisor = Arc::new(WorkerSupervisor::new());
        let facts = Arc::new(DriverFacts::default());
        let driver = ControlledDriver {
            facts: Arc::clone(&facts),
            block_request_until_cancelled: false,
            close_result: Err(PortalSessionFailure::TimedOut),
        };
        let actor = PortalSessionActor::spawn(&supervisor, driver).unwrap();
        actor
            .activate(parent(), deadline(), SessionCancellation::new())
            .unwrap();
        assert_eq!(
            actor.shutdown(Duration::from_secs(1)).unwrap(),
            PortalActorShutdown {
                worker: WorkerStop::Stopped,
                cleanup: PortalCleanupStatus::Failed,
            }
        );
        assert_eq!(facts.aborts.load(Ordering::SeqCst), 1);
        assert!(!facts.live.load(Ordering::SeqCst));
        assert_eq!(supervisor.live_worker_count(), 0);
    }

    #[test]
    fn dropped_handle_closes_admission_and_supervisor_reaps_the_owner() {
        let supervisor = Arc::new(WorkerSupervisor::new());
        let facts = Arc::new(DriverFacts::default());
        let actor =
            PortalSessionActor::spawn(&supervisor, ControlledDriver::normal(Arc::clone(&facts)))
                .unwrap();
        actor
            .activate(parent(), deadline(), SessionCancellation::new())
            .unwrap();
        drop(actor);

        let outcomes = supervisor.shutdown_all(Duration::from_secs(1)).unwrap();
        assert!(
            outcomes
                .iter()
                .all(|(_, stop)| *stop == WorkerStop::Stopped)
        );
        assert_eq!(supervisor.live_worker_count(), 0);
        assert!(!facts.live.load(Ordering::SeqCst));
    }

    fn service(
        supervisor: &Arc<WorkerSupervisor>,
        driver: ControlledDriver,
    ) -> PortalPermissionService {
        PortalPermissionService::spawn(supervisor, driver, PortalEligibilityDto::ReadyToRequest)
            .unwrap()
    }

    #[test]
    fn permission_service_single_flight_is_idempotent_by_operation_id() {
        let supervisor = Arc::new(WorkerSupervisor::new());
        let facts = Arc::new(DriverFacts::default());
        let driver = ControlledDriver {
            facts: Arc::clone(&facts),
            block_request_until_cancelled: true,
            close_result: Ok(()),
        };
        let service = Arc::new(service(&supervisor, driver));
        let pause = service.pause_after_actor_result_for_test();
        let request_service = Arc::clone(&service);
        let request = std::thread::spawn(move || {
            request_service.request("operation-one".into(), parent(), deadline())
        });
        wait_until(|| facts.request_entered.load(Ordering::SeqCst));

        let same = service
            .request("operation-one".into(), parent(), deadline())
            .unwrap();
        assert_eq!(same.lifecycle, PortalLifecycleDto::Requesting);
        let cancel_sequence = service.cancel("operation-one").unwrap().sequence;
        pause.wait_until_reached();
        assert_eq!(
            service.cancel("operation-one").unwrap().sequence,
            cancel_sequence
        );
        assert_eq!(
            service.request("operation-two".into(), parent(), deadline()),
            Err(PortalPermissionServiceFailure::Busy)
        );
        pause.release();
        assert!(matches!(
            request.join().unwrap(),
            Err(PortalPermissionServiceFailure::Actor(
                PortalActorFailure::Lifecycle(PortalSessionError {
                    failure: PortalSessionFailure::Cancelled,
                    cleanup: PortalCleanupStatus::ConfirmedClosed,
                })
            ))
        ));
        let status = service.snapshot();
        assert_eq!(status.lifecycle, PortalLifecycleDto::Idle);
        assert_eq!(status.last_failure, Some(PortalFailureDto::Cancelled));
        assert!(!status.sender_available && !status.delivery_qualified);
        assert_eq!(
            service.shutdown(Duration::from_secs(1)).unwrap().worker,
            WorkerStop::Stopped
        );
        assert_eq!(supervisor.live_worker_count(), 0);
    }

    #[test]
    fn permission_service_revoke_active_is_bounded_and_truthful() {
        let supervisor = Arc::new(WorkerSupervisor::new());
        let facts = Arc::new(DriverFacts::default());
        let service = service(&supervisor, ControlledDriver::normal(Arc::clone(&facts)));
        let active = service
            .request("operation-active".into(), parent(), deadline())
            .unwrap();
        assert!(active.permission_active && active.transport_connected);
        assert!(!active.sender_available && !active.delivery_qualified);
        let idempotent = service
            .request("operation-active".into(), parent(), deadline())
            .unwrap();
        assert_eq!(idempotent, active);
        assert_eq!(
            facts
                .calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .len(),
            2
        );
        let idle = service.revoke(deadline()).unwrap();
        assert_eq!(idle.lifecycle, PortalLifecycleDto::Idle);
        assert_eq!(idle.cleanup, PortalCleanupDto::ConfirmedClosed);
        assert!(!idle.permission_active && !idle.transport_connected);
        assert_eq!(
            *facts
                .calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            vec!["request", "connect", "close"]
        );
        service.shutdown(Duration::from_secs(1)).unwrap();
        assert_eq!(supervisor.live_worker_count(), 0);
    }

    #[test]
    fn permission_service_driver_revocation_emits_one_new_sequence()
    -> Result<(), Box<dyn std::error::Error>> {
        let supervisor = Arc::new(WorkerSupervisor::new());
        let facts = Arc::new(DriverFacts::default());
        let service = service(&supervisor, ControlledDriver::normal(Arc::clone(&facts)));
        let active = service
            .request("operation-revoked".into(), parent(), deadline())
            .unwrap();
        *facts
            .next_event
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(Ok(PortalDriverEvent::Revoked));
        wait_until(|| service.actor.last_driver_event().is_some());

        let Event::PortalKeyboardPermissionChanged { status, seq } =
            service.event_after(active.sequence).unwrap()
        else {
            return Err("unexpected event type".into());
        };
        assert_eq!(seq, status.sequence);
        assert_eq!(status.lifecycle, PortalLifecycleDto::Idle);
        assert_eq!(status.last_failure, Some(PortalFailureDto::Revoked));
        assert_eq!(status.cleanup, PortalCleanupDto::ConfirmedClosed);
        assert!(service.event_after(seq).is_none());
        service.shutdown(Duration::from_secs(1)).unwrap();
        assert_eq!(supervisor.live_worker_count(), 0);
        Ok(())
    }

    #[test]
    fn portal_event_pump_forwards_each_latest_sequence_at_most_once() {
        let supervisor = Arc::new(WorkerSupervisor::new());
        let facts = Arc::new(DriverFacts::default());
        let service = service(&supervisor, ControlledDriver::normal(Arc::clone(&facts)));
        let active = service
            .request("operation-event-pump".into(), parent(), deadline())
            .unwrap();
        let mut observed = 0;
        let mut published = Vec::new();

        assert!(
            pump_portal_event_once(&service, &mut observed, |event| {
                published.push(event.clone());
                Ok::<(), ()>(())
            })
            .unwrap()
        );
        assert_eq!(observed, active.sequence);
        assert_eq!(published.len(), 1);
        assert!(
            !pump_portal_event_once(&service, &mut observed, |_event| Ok::<(), ()>(())).unwrap()
        );

        service.revoke(deadline()).unwrap();
        service.shutdown(Duration::from_secs(1)).unwrap();
        assert_eq!(supervisor.live_worker_count(), 0);
    }

    #[test]
    fn permission_service_reconciles_revocation_reported_by_submission()
    -> Result<(), Box<dyn std::error::Error>> {
        let supervisor = Arc::new(WorkerSupervisor::new());
        let facts = Arc::new(DriverFacts::default());
        *facts
            .submit_result
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            PortalEisTextSubmission::DispatchUncertain(PortalEisTextFailure::Revoked);
        let service = service(&supervisor, ControlledDriver::normal(Arc::clone(&facts)));
        let active = service
            .request("operation-submit-revoked".into(), parent(), deadline())
            .unwrap();

        assert_eq!(
            service.submit_text(
                PortalEisTextPayload::parse("controlled-frame").unwrap(),
                deadline(),
                SessionCancellation::new(),
            ),
            PortalEisTextSubmission::DispatchUncertain(PortalEisTextFailure::Revoked)
        );
        let Event::PortalKeyboardPermissionChanged { status, seq } =
            service.event_after(active.sequence).unwrap()
        else {
            return Err("unexpected event type".into());
        };
        assert_eq!(seq, status.sequence);
        assert_eq!(status.lifecycle, PortalLifecycleDto::Idle);
        assert_eq!(status.last_failure, Some(PortalFailureDto::Revoked));
        assert_eq!(status.cleanup, PortalCleanupDto::ConfirmedClosed);
        assert!(!status.permission_active && !status.transport_connected);
        assert!(!facts.live.load(Ordering::SeqCst));
        assert!(service.event_after(seq).is_none());
        service.shutdown(Duration::from_secs(1)).unwrap();
        assert_eq!(supervisor.live_worker_count(), 0);
        Ok(())
    }

    #[test]
    fn delivery_submitter_requires_active_service_and_preserves_unverified_outcome() {
        let supervisor = Arc::new(WorkerSupervisor::new());
        let facts = Arc::new(DriverFacts::default());
        let service = Arc::new(service(
            &supervisor,
            ControlledDriver::normal(Arc::clone(&facts)),
        ));
        let mut submitter = PortalPermissionSubmitter::new(Arc::clone(&service));
        let payload = || PortalEisTextPayload::parse("controlled-frame").unwrap();

        assert_eq!(
            submitter.submit(payload(), &SessionCancellation::new(), deadline(),),
            PortalEisTextSubmission::RejectedBeforeDispatch(PortalEisTextFailure::NotActive)
        );
        assert!(
            facts
                .submitted_bytes
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_empty()
        );

        service
            .request("operation-submit".into(), parent(), deadline())
            .unwrap();
        assert_eq!(
            submitter.submit(payload(), &SessionCancellation::new(), deadline(),),
            PortalEisTextSubmission::SubmittedUnverified
        );
        assert_eq!(
            facts
                .submitted_bytes
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .len(),
            1
        );
        service.revoke(deadline()).unwrap();
        service.shutdown(Duration::from_secs(1)).unwrap();
        assert_eq!(supervisor.live_worker_count(), 0);
    }

    #[test]
    fn predispatch_transport_failure_closes_without_a_revocation_claim()
    -> Result<(), Box<dyn std::error::Error>> {
        let supervisor = Arc::new(WorkerSupervisor::new());
        let facts = Arc::new(DriverFacts::default());
        *facts
            .submit_result
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            PortalEisTextSubmission::RejectedBeforeDispatch(PortalEisTextFailure::TransportFailed);
        let service = service(&supervisor, ControlledDriver::normal(Arc::clone(&facts)));
        let active = service
            .request(
                "operation-submit-transport-failed".into(),
                parent(),
                deadline(),
            )
            .unwrap();

        assert_eq!(
            service.submit_text(
                PortalEisTextPayload::parse("controlled-frame").unwrap(),
                deadline(),
                SessionCancellation::new(),
            ),
            PortalEisTextSubmission::RejectedBeforeDispatch(PortalEisTextFailure::TransportFailed,)
        );
        let Event::PortalKeyboardPermissionChanged { status, seq } =
            service.event_after(active.sequence).unwrap()
        else {
            return Err("unexpected event type".into());
        };
        assert_eq!(seq, status.sequence);
        assert_eq!(status.lifecycle, PortalLifecycleDto::Idle);
        assert_eq!(status.last_failure, Some(PortalFailureDto::TransportFailed));
        assert_ne!(status.last_failure, Some(PortalFailureDto::Revoked));
        assert_eq!(status.cleanup, PortalCleanupDto::ConfirmedClosed);
        assert!(!status.permission_active && !status.transport_connected);
        assert!(!facts.live.load(Ordering::SeqCst));
        assert!(service.event_after(seq).is_none());
        assert_eq!(
            facts
                .calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_slice(),
            ["request", "connect", "submit", "close"]
        );
        service.shutdown(Duration::from_secs(1)).unwrap();
        assert_eq!(supervisor.live_worker_count(), 0);
        Ok(())
    }

    #[test]
    fn permission_service_reconciles_revocation_before_activation_commit() {
        let supervisor = Arc::new(WorkerSupervisor::new());
        let facts = Arc::new(DriverFacts::default());
        let service = Arc::new(service(
            &supervisor,
            ControlledDriver::normal(Arc::clone(&facts)),
        ));
        let pause = service.pause_after_actor_result_for_test();
        let request_service = Arc::clone(&service);
        let request = std::thread::spawn(move || {
            request_service.request("operation-early-revoked".into(), parent(), deadline())
        });

        pause.wait_until_reached();
        *facts
            .next_event
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(Ok(PortalDriverEvent::Revoked));
        wait_until(|| service.actor.last_driver_event().is_some());

        // A concurrent observer may report Requesting, but must not consume
        // the matching event before activation commits.
        let requesting = service.snapshot();
        assert_eq!(requesting.lifecycle, PortalLifecycleDto::Requesting);
        assert_eq!(
            service
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .observed_driver_sequence,
            0
        );
        pause.release();

        let reconciled = request.join().unwrap().unwrap();
        assert_eq!(reconciled.lifecycle, PortalLifecycleDto::Idle);
        assert_eq!(reconciled.last_failure, Some(PortalFailureDto::Revoked));
        assert_eq!(reconciled.cleanup, PortalCleanupDto::ConfirmedClosed);
        assert!(!reconciled.permission_active && !reconciled.transport_connected);
        assert_eq!(
            service.actor.state(deadline()).unwrap(),
            PortalSessionState::Idle
        );
        service.shutdown(Duration::from_secs(1)).unwrap();
        assert_eq!(supervisor.live_worker_count(), 0);
    }

    #[test]
    fn permission_service_reconciles_portal_revocation_during_explicit_revoke() {
        let supervisor = Arc::new(WorkerSupervisor::new());
        let facts = Arc::new(DriverFacts::default());
        let service = Arc::new(service(
            &supervisor,
            ControlledDriver::normal(Arc::clone(&facts)),
        ));
        service
            .request("operation-overlap".into(), parent(), deadline())
            .unwrap();

        let pause = service.pause_after_revoke_marked_for_test();
        let revoke_service = Arc::clone(&service);
        let revoke = std::thread::spawn(move || revoke_service.revoke(deadline()));
        pause.wait_until_reached();
        *facts
            .next_event
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(Ok(PortalDriverEvent::Revoked));
        wait_until(|| service.actor.last_driver_event().is_some());
        pause.release();

        let revoked = revoke.join().unwrap().unwrap();
        assert_eq!(revoked.lifecycle, PortalLifecycleDto::Idle);
        assert_eq!(revoked.last_failure, Some(PortalFailureDto::Revoked));
        assert_eq!(revoked.cleanup, PortalCleanupDto::ConfirmedClosed);
        assert!(!revoked.permission_active && !revoked.transport_connected);
        assert_eq!(
            service.actor.state(deadline()).unwrap(),
            PortalSessionState::Idle
        );
        service.shutdown(Duration::from_secs(1)).unwrap();
        assert_eq!(supervisor.live_worker_count(), 0);
    }

    #[test]
    fn permission_service_rejects_submission_after_revoke_linearizes() {
        let supervisor = Arc::new(WorkerSupervisor::new());
        let facts = Arc::new(DriverFacts::default());
        let service = Arc::new(service(
            &supervisor,
            ControlledDriver::normal(Arc::clone(&facts)),
        ));
        service
            .request("operation-revoke-submit".into(), parent(), deadline())
            .unwrap();

        let pause = service.pause_after_revoke_marked_for_test();
        let revoke_service = Arc::clone(&service);
        let revoke = std::thread::spawn(move || revoke_service.revoke(deadline()));
        pause.wait_until_reached();

        assert_eq!(
            service.submit_text(
                PortalEisTextPayload::parse("controlled-frame").unwrap(),
                deadline(),
                SessionCancellation::new(),
            ),
            PortalEisTextSubmission::RejectedBeforeDispatch(PortalEisTextFailure::NotActive)
        );
        assert_eq!(
            facts
                .calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_slice(),
            ["request", "connect"]
        );

        pause.release();
        let status = revoke.join().unwrap().unwrap();
        assert_eq!(status.lifecycle, PortalLifecycleDto::Idle);
        assert_eq!(
            facts
                .calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_slice(),
            ["request", "connect", "close"]
        );
        service.shutdown(Duration::from_secs(1)).unwrap();
        assert_eq!(supervisor.live_worker_count(), 0);
    }

    #[test]
    fn permission_service_reconciles_post_dispatch_reply_timeout() {
        let supervisor = Arc::new(WorkerSupervisor::new());
        let facts = Arc::new(DriverFacts::default());
        let service = Arc::new(service(
            &supervisor,
            ControlledDriver::normal(Arc::clone(&facts)),
        ));
        service
            .request("operation-submit-timeout".into(), parent(), deadline())
            .unwrap();
        let pause = service.actor.pause_after_submission_for_test();
        let submit_service = Arc::clone(&service);
        let submit = std::thread::spawn(move || {
            submit_service.submit_text(
                PortalEisTextPayload::parse("controlled-frame").unwrap(),
                Instant::now() + Duration::from_millis(30),
                SessionCancellation::new(),
            )
        });

        pause.wait_until_reached();
        assert_eq!(
            submit.join().unwrap(),
            PortalEisTextSubmission::DispatchUncertain(PortalEisTextFailure::TimedOut)
        );
        assert_eq!(service.snapshot().lifecycle, PortalLifecycleDto::Active);
        pause.release();

        wait_until(|| service.snapshot().lifecycle != PortalLifecycleDto::Active);
        let status = service.snapshot();
        assert_eq!(status.lifecycle, PortalLifecycleDto::Idle);
        assert_eq!(status.last_failure, Some(PortalFailureDto::TimedOut));
        assert_eq!(status.cleanup, PortalCleanupDto::ConfirmedClosed);
        assert!(!status.permission_active && !status.transport_connected);
        assert_eq!(
            facts
                .calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_slice(),
            ["request", "connect", "submit", "close"]
        );
        service.shutdown(Duration::from_secs(1)).unwrap();
        assert_eq!(supervisor.live_worker_count(), 0);
    }

    #[test]
    fn permission_service_cleanup_failure_is_terminally_invalid() {
        let supervisor = Arc::new(WorkerSupervisor::new());
        let facts = Arc::new(DriverFacts::default());
        let driver = ControlledDriver {
            facts: Arc::clone(&facts),
            block_request_until_cancelled: false,
            close_result: Err(PortalSessionFailure::CloseFailed),
        };
        let service = service(&supervisor, driver);
        service
            .request("operation-invalid".into(), parent(), deadline())
            .unwrap();
        assert!(matches!(
            service.revoke(deadline()),
            Err(PortalPermissionServiceFailure::Actor(
                PortalActorFailure::Lifecycle(PortalSessionError {
                    cleanup: PortalCleanupStatus::Failed,
                    ..
                })
            ))
        ));
        let invalid = service.snapshot();
        assert_eq!(invalid.lifecycle, PortalLifecycleDto::Invalid);
        assert_eq!(invalid.cleanup, PortalCleanupDto::Failed);
        assert!(!invalid.can_request);
        assert_eq!(
            service.request("operation-later".into(), parent(), deadline()),
            Err(PortalPermissionServiceFailure::InvalidOperation)
        );
        let shutdown = service.shutdown(Duration::from_secs(1)).unwrap();
        assert_eq!(shutdown.cleanup, PortalCleanupStatus::Failed);
        assert_eq!(supervisor.live_worker_count(), 0);
    }

    #[test]
    fn permission_service_shutdown_closes_admission_and_pending_work() {
        let supervisor = Arc::new(WorkerSupervisor::new());
        let facts = Arc::new(DriverFacts::default());
        let driver = ControlledDriver {
            facts: Arc::clone(&facts),
            block_request_until_cancelled: true,
            close_result: Ok(()),
        };
        let service = Arc::new(service(&supervisor, driver));
        let request_service = Arc::clone(&service);
        let request = std::thread::spawn(move || {
            request_service.request("operation-shutdown".into(), parent(), deadline())
        });
        wait_until(|| facts.request_entered.load(Ordering::SeqCst));
        let stopped = service.shutdown(Duration::from_secs(1)).unwrap();
        assert_eq!(stopped.worker, WorkerStop::Stopped);
        assert!(request.join().unwrap().is_err());
        assert_eq!(
            service.request("operation-after".into(), parent(), deadline()),
            Err(PortalPermissionServiceFailure::AdmissionClosed)
        );
        let terminal = service.snapshot();
        assert!(!terminal.can_request);
        assert!(!terminal.permission_active && !terminal.transport_connected);
        assert_eq!(supervisor.live_worker_count(), 0);
    }

    #[test]
    fn permission_service_rejects_invalid_operation_ids_before_actor_work() {
        let supervisor = Arc::new(WorkerSupervisor::new());
        let facts = Arc::new(DriverFacts::default());
        let service = service(&supervisor, ControlledDriver::normal(Arc::clone(&facts)));
        for invalid in ["", "contains space", "contains/slash"] {
            assert_eq!(
                service.request(invalid.into(), parent(), deadline()),
                Err(PortalPermissionServiceFailure::InvalidOperation)
            );
        }
        assert!(
            facts
                .calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_empty()
        );
        service.shutdown(Duration::from_secs(1)).unwrap();
    }

    #[test]
    fn permission_service_actor_timeout_never_claims_confirmed_cleanup() {
        let supervisor = Arc::new(WorkerSupervisor::new());
        let facts = Arc::new(DriverFacts::default());
        let driver = ControlledDriver {
            facts: Arc::clone(&facts),
            block_request_until_cancelled: true,
            close_result: Err(PortalSessionFailure::CloseFailed),
        };
        let service = service(&supervisor, driver);
        assert!(matches!(
            service.request(
                "operation-timeout".into(),
                parent(),
                Instant::now() + Duration::from_millis(20),
            ),
            Err(PortalPermissionServiceFailure::Actor(
                PortalActorFailure::TimedOut
                    | PortalActorFailure::Lifecycle(PortalSessionError {
                        cleanup: PortalCleanupStatus::Failed,
                        ..
                    })
            ))
        ));
        let status = service.snapshot();
        assert_eq!(status.lifecycle, PortalLifecycleDto::Invalid);
        assert_eq!(status.cleanup, PortalCleanupDto::Failed);
        assert!(!status.can_request);
        let shutdown = service.shutdown(Duration::from_secs(1)).unwrap();
        assert_eq!(shutdown.cleanup, PortalCleanupStatus::Failed);
        assert_eq!(supervisor.live_worker_count(), 0);
    }
}
