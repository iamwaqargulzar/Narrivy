//! Inert desktop ownership state machine for a future portal-parent operation.
//!
//! This module deliberately contains no Tauri command, GTK call, IPC client,
//! thread, or startup hook. It models the ordering an eventual executor must
//! obey without letting UI code own platform handles or daemon policy.

#![forbid(unsafe_code)]

use std::fmt;

/// Monotonic cleanup intent. A later, stronger intent cannot be downgraded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CleanupIntent {
    None,
    Cancel,
    CloseMain,
    Exit,
}

/// Whether the original permission request may have reached the daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchState {
    NotYet,
    Possible,
}

/// Content-free terminal result from the original permission request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionTerminal {
    Active,
    InactiveConfirmed,
    CleanupUnconfirmed,
}

/// Content-free terminal result from cancel followed by unconditional revoke.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteCleanupTerminal {
    Confirmed,
    Unconfirmed,
}

/// Sanitized result safe to publish after local parent release is acknowledged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SanitizedOutcome {
    PermissionActive,
    PermissionInactive,
    ParentExportFailed,
    Cancelled,
    CleanupUnconfirmed,
}

/// Side effects for a future owned executor. Opaque values intentionally have
/// no `Debug` or serialization requirement.
pub enum CoordinatorEffect<O, T> {
    ScheduleBegin { generation: u64 },
    WaitExportOffMain { generation: u64 },
    SendPermission { generation: u64, operation: O },
    CancelThenRevoke { generation: u64, operation: O },
    ScheduleRelease { generation: u64, ticket: T },
    RevokeGranted { generation: u64 },
    ReplayCloseOnce,
    ReplayExitOnce,
    PublishSanitizedOutcome(SanitizedOutcome),
}

impl<O, T> fmt::Debug for CoordinatorEffect<O, T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ScheduleBegin { generation } => formatter
                .debug_struct("ScheduleBegin")
                .field("generation", generation)
                .finish(),
            Self::WaitExportOffMain { generation } => formatter
                .debug_struct("WaitExportOffMain")
                .field("generation", generation)
                .finish(),
            Self::SendPermission { generation, .. } => formatter
                .debug_struct("SendPermission")
                .field("generation", generation)
                .field("operation", &"<redacted>")
                .finish(),
            Self::CancelThenRevoke { generation, .. } => formatter
                .debug_struct("CancelThenRevoke")
                .field("generation", generation)
                .field("operation", &"<redacted>")
                .finish(),
            Self::ScheduleRelease { generation, .. } => formatter
                .debug_struct("ScheduleRelease")
                .field("generation", generation)
                .field("ticket", &"<opaque>")
                .finish(),
            Self::RevokeGranted { generation } => formatter
                .debug_struct("RevokeGranted")
                .field("generation", generation)
                .finish(),
            Self::ReplayCloseOnce => formatter.write_str("ReplayCloseOnce"),
            Self::ReplayExitOnce => formatter.write_str("ReplayExitOnce"),
            Self::PublishSanitizedOutcome(outcome) => formatter
                .debug_tuple("PublishSanitizedOutcome")
                .field(outcome)
                .finish(),
        }
    }
}

enum State<O, T> {
    Idle,
    ExportScheduled {
        generation: u64,
        operation: O,
        intent: CleanupIntent,
    },
    ExportPending {
        generation: u64,
        operation: O,
        ticket: T,
        intent: CleanupIntent,
    },
    PermissionPending {
        generation: u64,
        operation: O,
        ticket: T,
        dispatch: DispatchState,
        intent: CleanupIntent,
        cleanup_started: bool,
        original_terminal: Option<PermissionTerminal>,
        remote_terminal: Option<RemoteCleanupTerminal>,
    },
    Releasing {
        generation: u64,
        ticket: T,
        intent: CleanupIntent,
        next: PostRelease,
    },
    Granted {
        generation: u64,
    },
    RevokingGranted {
        generation: u64,
        intent: CleanupIntent,
    },
    ReplayPending {
        intent: CleanupIntent,
        after_close: PostReplay,
    },
    ReplayArmed {
        armed: CleanupIntent,
        follow_up: Option<CleanupIntent>,
        after_close: PostReplay,
    },
    LocalReleaseUnconfirmed,
    RemoteCleanupUnconfirmed,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PostRelease {
    Granted,
    RevokeGranted,
    Inactive,
    ParentExportFailed,
    Cancelled,
    CleanupUnconfirmed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PostReplay {
    Idle,
    RemoteCleanupUnconfirmed,
}

/// Pure single-owner coordinator. A production executor is intentionally not
/// constructed until IPC cancellation/join and Tauri restart are resolved.
pub struct PortalParentCoordinator<O, T> {
    next_generation: u64,
    state: State<O, T>,
}

/// Exclusive permission to perform the short, nonblocking native export call.
///
/// The mutable borrow prevents cleanup or another export transition until
/// [`NativeExportAdmission::begin_with`] either installs the accepted ticket
/// or records a synchronous rejection. Dropping an unused admission performs
/// no native work and leaves the coordinator fail-closed.
#[must_use = "native export must be resolved with begin_with or deliberately abandoned fail-closed"]
pub struct NativeExportAdmission<'a, O, T> {
    coordinator: &'a mut PortalParentCoordinator<O, T>,
    generation: u64,
    operation: O,
}

/// Result of checking the queued main-thread export immediately before FFI.
#[must_use]
pub enum NativeExportStep<'a, O, T> {
    Begin(NativeExportAdmission<'a, O, T>),
    Skip,
    Stale,
}

impl<O, T: Clone> NativeExportAdmission<'_, O, T> {
    /// Perform the synchronous native begin while admission is exclusively
    /// borrowed, install its ticket, and only then return the callback request
    /// to the future worker owner.
    pub fn begin_with<R, E>(
        self,
        begin_export: impl FnOnce() -> Result<(T, R), E>,
    ) -> Result<(Vec<CoordinatorEffect<O, T>>, R), E> {
        let (ticket, request) = match begin_export() {
            Ok(accepted) => accepted,
            Err(error) => {
                self.coordinator.state = State::Idle;
                return Err(error);
            }
        };
        self.coordinator.state = State::ExportPending {
            generation: self.generation,
            operation: self.operation,
            ticket: ticket.clone(),
            intent: CleanupIntent::None,
        };
        Ok((
            vec![CoordinatorEffect::WaitExportOffMain {
                generation: self.generation,
            }],
            request,
        ))
    }
}

impl<O, T> fmt::Debug for PortalParentCoordinator<O, T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PortalParentCoordinator")
            .field("state", &self.phase_name())
            .finish()
    }
}

impl<O, T> PortalParentCoordinator<O, T> {
    #[must_use]
    pub fn phase_name(&self) -> &'static str {
        match self.state {
            State::Idle => "idle",
            State::ExportScheduled { .. } => "export_scheduled",
            State::ExportPending { .. } => "export_pending",
            State::PermissionPending { .. } => "permission_pending",
            State::Releasing { .. } => "releasing",
            State::Granted { .. } => "granted",
            State::RevokingGranted { .. } => "revoking_granted",
            State::ReplayPending { .. } => "replay_pending",
            State::ReplayArmed { .. } => "replay_armed",
            State::LocalReleaseUnconfirmed => "local_release_unconfirmed",
            State::RemoteCleanupUnconfirmed => "remote_cleanup_unconfirmed",
            State::Stopped => "stopped",
        }
    }
}

impl<O: Clone, T: Clone> Default for PortalParentCoordinator<O, T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<O: Clone, T: Clone> PortalParentCoordinator<O, T> {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            next_generation: 1,
            state: State::Idle,
        }
    }

    /// Admit one explicit user operation. Overlap fails closed.
    pub fn begin(&mut self, operation: O) -> Option<CoordinatorEffect<O, T>> {
        if !matches!(self.state, State::Idle) {
            return None;
        }
        let generation = self.next_generation;
        self.next_generation = self.next_generation.wrapping_add(1).max(1);
        self.state = State::ExportScheduled {
            generation,
            operation,
            intent: CleanupIntent::None,
        };
        Some(CoordinatorEffect::ScheduleBegin { generation })
    }

    /// Exclusively admit the short main-thread native begin call.
    ///
    /// Cleanup intent queued before this check skips export. While `Begin` is
    /// held, Rust's mutable borrow prevents cleanup from racing the synchronous
    /// native acceptance and ticket installation.
    pub fn prepare_native_export(&mut self, generation: u64) -> NativeExportStep<'_, O, T> {
        let state = std::mem::replace(&mut self.state, State::LocalReleaseUnconfirmed);
        match state {
            State::ExportScheduled {
                generation: current,
                operation,
                intent,
            } if current == generation => {
                if intent == CleanupIntent::None {
                    NativeExportStep::Begin(NativeExportAdmission {
                        coordinator: self,
                        generation,
                        operation,
                    })
                } else {
                    self.state = if matches!(intent, CleanupIntent::CloseMain | CleanupIntent::Exit)
                    {
                        State::ReplayPending {
                            intent,
                            after_close: PostReplay::Idle,
                        }
                    } else {
                        State::Idle
                    };
                    NativeExportStep::Skip
                }
            }
            other => {
                self.state = other;
                NativeExportStep::Stale
            }
        }
    }

    /// The validated handle remains outside this state machine and may be
    /// borrowed only by the executor's Rust-to-IPC call.
    pub fn export_ready(&mut self, generation: u64) -> Vec<CoordinatorEffect<O, T>> {
        let state = std::mem::replace(&mut self.state, State::LocalReleaseUnconfirmed);
        match state {
            State::ExportPending {
                generation: current,
                operation,
                ticket,
                intent,
            } if current == generation && intent == CleanupIntent::None => {
                let effect_operation = operation.clone();
                self.state = State::PermissionPending {
                    generation,
                    operation,
                    ticket,
                    dispatch: DispatchState::NotYet,
                    intent,
                    cleanup_started: false,
                    original_terminal: None,
                    remote_terminal: None,
                };
                vec![CoordinatorEffect::SendPermission {
                    generation,
                    operation: effect_operation,
                }]
            }
            State::ExportPending {
                generation: current,
                ticket,
                intent,
                ..
            } if current == generation => {
                self.state = State::Releasing {
                    generation,
                    ticket: ticket.clone(),
                    intent,
                    next: PostRelease::Cancelled,
                };
                vec![CoordinatorEffect::ScheduleRelease { generation, ticket }]
            }
            other => {
                self.state = other;
                Vec::new()
            }
        }
    }

    /// Reconcile callback timeout, cancellation, or invalid-handle failure
    /// after native export was accepted. The accepted ticket still requires a
    /// matching main-thread release before any replay or reusable idle state.
    pub fn export_wait_failed(&mut self, generation: u64) -> Vec<CoordinatorEffect<O, T>> {
        let state = std::mem::replace(&mut self.state, State::LocalReleaseUnconfirmed);
        match state {
            State::ExportPending {
                generation: current,
                ticket,
                intent,
                ..
            } if current == generation => {
                self.state = State::Releasing {
                    generation,
                    ticket: ticket.clone(),
                    intent,
                    next: PostRelease::ParentExportFailed,
                };
                vec![CoordinatorEffect::ScheduleRelease { generation, ticket }]
            }
            other => {
                self.state = other;
                Vec::new()
            }
        }
    }

    /// Mark dispatch uncertainty immediately before IPC serialization/write.
    pub fn permission_dispatch_started(&mut self, generation: u64) -> bool {
        let State::PermissionPending {
            generation: current,
            dispatch,
            ..
        } = &mut self.state
        else {
            return false;
        };
        if *current != generation || *dispatch != DispatchState::NotYet {
            return false;
        }
        *dispatch = DispatchState::Possible;
        true
    }

    /// Merge cancel/window-close/exit intent and return only newly required
    /// effects. Duplicate notifications never start a second cleanup.
    pub fn request_cleanup(&mut self, intent: CleanupIntent) -> Vec<CoordinatorEffect<O, T>> {
        if intent == CleanupIntent::None {
            return Vec::new();
        }
        let state = std::mem::replace(&mut self.state, State::LocalReleaseUnconfirmed);
        match state {
            State::Idle => {
                self.state = State::Idle;
                Vec::new()
            }
            State::ExportScheduled {
                generation,
                operation,
                intent: current,
            } => {
                self.state = State::ExportScheduled {
                    generation,
                    operation,
                    intent: current.max(intent),
                };
                Vec::new()
            }
            State::ExportPending {
                generation,
                operation: _,
                ticket,
                intent: current,
            } => {
                let intent = current.max(intent);
                self.state = State::Releasing {
                    generation,
                    ticket: ticket.clone(),
                    intent,
                    next: PostRelease::Cancelled,
                };
                vec![CoordinatorEffect::ScheduleRelease { generation, ticket }]
            }
            State::PermissionPending {
                generation,
                operation: _,
                ticket,
                dispatch: DispatchState::NotYet,
                intent: current,
                ..
            } => {
                let intent = current.max(intent);
                self.state = State::Releasing {
                    generation,
                    ticket: ticket.clone(),
                    intent,
                    next: PostRelease::Cancelled,
                };
                vec![CoordinatorEffect::ScheduleRelease { generation, ticket }]
            }
            State::PermissionPending {
                generation,
                operation,
                ticket,
                dispatch,
                intent: current,
                cleanup_started,
                original_terminal,
                remote_terminal,
            } => {
                let intent = current.max(intent);
                let start_cleanup = !cleanup_started;
                let effect_operation = operation.clone();
                self.state = State::PermissionPending {
                    generation,
                    operation,
                    ticket,
                    dispatch,
                    intent,
                    cleanup_started: true,
                    original_terminal,
                    remote_terminal,
                };
                if start_cleanup {
                    vec![CoordinatorEffect::CancelThenRevoke {
                        generation,
                        operation: effect_operation,
                    }]
                } else {
                    Vec::new()
                }
            }
            State::Releasing {
                generation,
                ticket,
                intent: current,
                next,
            } => {
                let intent = current.max(intent);
                let next = if next == PostRelease::Granted {
                    PostRelease::RevokeGranted
                } else {
                    next
                };
                self.state = State::Releasing {
                    generation,
                    ticket,
                    intent,
                    next,
                };
                Vec::new()
            }
            State::Granted { generation } => {
                self.state = State::RevokingGranted { generation, intent };
                vec![CoordinatorEffect::RevokeGranted { generation }]
            }
            State::RevokingGranted {
                generation,
                intent: current,
            } => {
                self.state = State::RevokingGranted {
                    generation,
                    intent: current.max(intent),
                };
                Vec::new()
            }
            State::ReplayPending {
                intent: current,
                after_close,
            } => {
                self.state = State::ReplayPending {
                    intent: current.max(intent),
                    after_close,
                };
                Vec::new()
            }
            State::ReplayArmed {
                armed,
                follow_up,
                after_close,
            } => {
                let follow_up = if intent > armed {
                    Some(follow_up.map_or(intent, |current| current.max(intent)))
                } else {
                    follow_up
                };
                self.state = State::ReplayArmed {
                    armed,
                    follow_up,
                    after_close,
                };
                Vec::new()
            }
            State::LocalReleaseUnconfirmed => {
                self.state = State::LocalReleaseUnconfirmed;
                Vec::new()
            }
            State::RemoteCleanupUnconfirmed => {
                self.state = State::RemoteCleanupUnconfirmed;
                self.defer_replay(intent, PostReplay::RemoteCleanupUnconfirmed);
                Vec::new()
            }
            State::Stopped => {
                self.state = State::Stopped;
                Vec::new()
            }
        }
    }

    pub fn permission_terminal(
        &mut self,
        generation: u64,
        terminal: PermissionTerminal,
    ) -> Vec<CoordinatorEffect<O, T>> {
        let State::PermissionPending {
            generation: current,
            operation,
            dispatch,
            intent,
            cleanup_started,
            original_terminal,
            ..
        } = &mut self.state
        else {
            return Vec::new();
        };
        if *current != generation || original_terminal.is_some() {
            return Vec::new();
        }
        *original_terminal = Some(terminal);
        if terminal == PermissionTerminal::CleanupUnconfirmed
            && *dispatch == DispatchState::Possible
            && *intent == CleanupIntent::None
        {
            *intent = CleanupIntent::Cancel;
            *cleanup_started = true;
            return vec![CoordinatorEffect::CancelThenRevoke {
                generation,
                operation: operation.clone(),
            }];
        }
        self.finish_permission_if_terminal(generation)
    }

    pub fn remote_cleanup_terminal(
        &mut self,
        generation: u64,
        terminal: RemoteCleanupTerminal,
    ) -> Vec<CoordinatorEffect<O, T>> {
        let State::PermissionPending {
            generation: current,
            cleanup_started,
            remote_terminal,
            ..
        } = &mut self.state
        else {
            return Vec::new();
        };
        if *current != generation || !*cleanup_started || remote_terminal.is_some() {
            return Vec::new();
        }
        *remote_terminal = Some(terminal);
        self.finish_permission_if_terminal(generation)
    }

    fn finish_permission_if_terminal(&mut self, generation: u64) -> Vec<CoordinatorEffect<O, T>> {
        let ready = match &self.state {
            State::PermissionPending {
                intent: CleanupIntent::None,
                original_terminal: Some(_),
                ..
            } => true,
            State::PermissionPending {
                intent,
                original_terminal: Some(_),
                remote_terminal: Some(_),
                ..
            } if *intent != CleanupIntent::None => true,
            _ => false,
        };
        if !ready {
            return Vec::new();
        }

        let state = std::mem::replace(&mut self.state, State::LocalReleaseUnconfirmed);
        let (current, ticket, intent, Some(original), remote_terminal) = (match state {
            State::PermissionPending {
                generation,
                ticket,
                intent,
                original_terminal,
                remote_terminal,
                ..
            } => (
                generation,
                ticket,
                intent,
                original_terminal,
                remote_terminal,
            ),
            other => {
                self.state = other;
                return Vec::new();
            }
        }) else {
            self.state = State::LocalReleaseUnconfirmed;
            return Vec::new();
        };
        if current != generation {
            self.state = State::LocalReleaseUnconfirmed;
            return Vec::new();
        }
        let next = if intent == CleanupIntent::None {
            match original {
                PermissionTerminal::Active => PostRelease::Granted,
                PermissionTerminal::InactiveConfirmed => PostRelease::Inactive,
                PermissionTerminal::CleanupUnconfirmed => PostRelease::CleanupUnconfirmed,
            }
        } else if remote_terminal == Some(RemoteCleanupTerminal::Unconfirmed) {
            PostRelease::CleanupUnconfirmed
        } else {
            PostRelease::Cancelled
        };
        self.state = State::Releasing {
            generation,
            ticket: ticket.clone(),
            intent,
            next,
        };
        vec![CoordinatorEffect::ScheduleRelease { generation, ticket }]
    }

    pub fn release_terminal(
        &mut self,
        generation: u64,
        confirmed: bool,
    ) -> Vec<CoordinatorEffect<O, T>> {
        let state = std::mem::replace(&mut self.state, State::LocalReleaseUnconfirmed);
        let (current, ticket, intent, next) = match state {
            State::Releasing {
                generation,
                ticket,
                intent,
                next,
            } => (generation, ticket, intent, next),
            other => {
                self.state = other;
                return Vec::new();
            }
        };
        if current != generation {
            self.state = State::Releasing {
                generation: current,
                ticket,
                intent,
                next,
            };
            return Vec::new();
        }
        if !confirmed {
            self.state = State::LocalReleaseUnconfirmed;
            return Vec::new();
        }

        let replay_after_release = next != PostRelease::RevokeGranted;
        let effects = match next {
            PostRelease::Granted => {
                self.state = State::Granted { generation };
                vec![CoordinatorEffect::PublishSanitizedOutcome(
                    SanitizedOutcome::PermissionActive,
                )]
            }
            PostRelease::RevokeGranted => {
                self.state = State::RevokingGranted { generation, intent };
                vec![CoordinatorEffect::RevokeGranted { generation }]
            }
            PostRelease::Inactive => {
                self.state = State::Idle;
                vec![CoordinatorEffect::PublishSanitizedOutcome(
                    SanitizedOutcome::PermissionInactive,
                )]
            }
            PostRelease::ParentExportFailed => {
                self.state = State::Idle;
                vec![CoordinatorEffect::PublishSanitizedOutcome(
                    SanitizedOutcome::ParentExportFailed,
                )]
            }
            PostRelease::Cancelled => {
                self.state = State::Idle;
                vec![CoordinatorEffect::PublishSanitizedOutcome(
                    SanitizedOutcome::Cancelled,
                )]
            }
            PostRelease::CleanupUnconfirmed => {
                self.state = State::RemoteCleanupUnconfirmed;
                vec![CoordinatorEffect::PublishSanitizedOutcome(
                    SanitizedOutcome::CleanupUnconfirmed,
                )]
            }
        };
        if replay_after_release {
            let after_close = if next == PostRelease::CleanupUnconfirmed {
                PostReplay::RemoteCleanupUnconfirmed
            } else {
                PostReplay::Idle
            };
            self.defer_replay(intent, after_close);
        }
        effects
    }

    pub fn granted_revoke_terminal(
        &mut self,
        generation: u64,
        confirmed: bool,
    ) -> Vec<CoordinatorEffect<O, T>> {
        let state = std::mem::replace(&mut self.state, State::LocalReleaseUnconfirmed);
        let State::RevokingGranted {
            generation: current,
            intent,
        } = state
        else {
            self.state = state;
            return Vec::new();
        };
        if current != generation {
            self.state = State::RevokingGranted {
                generation: current,
                intent,
            };
            return Vec::new();
        }
        if !confirmed {
            self.state = State::RemoteCleanupUnconfirmed;
            self.defer_replay(intent, PostReplay::RemoteCleanupUnconfirmed);
            return vec![CoordinatorEffect::PublishSanitizedOutcome(
                SanitizedOutcome::CleanupUnconfirmed,
            )];
        }
        self.state = State::Idle;
        let effects = vec![CoordinatorEffect::PublishSanitizedOutcome(
            SanitizedOutcome::Cancelled,
        )];
        self.defer_replay(intent, PostReplay::Idle);
        effects
    }

    fn defer_replay(&mut self, intent: CleanupIntent, after_close: PostReplay) {
        if matches!(intent, CleanupIntent::CloseMain | CleanupIntent::Exit) {
            self.state = State::ReplayPending {
                intent,
                after_close,
            };
        }
    }

    /// Atomically arm exactly one close/exit replay before the executor calls
    /// back into Tauri. Duplicate intent can merge while replay is pending.
    pub fn take_replay(&mut self) -> Option<CoordinatorEffect<O, T>> {
        let (intent, after_close) = match &self.state {
            State::ReplayPending {
                intent,
                after_close,
            } => (*intent, *after_close),
            _ => return None,
        };
        self.state = State::ReplayArmed {
            armed: intent,
            follow_up: None,
            after_close,
        };
        match intent {
            CleanupIntent::CloseMain => Some(CoordinatorEffect::ReplayCloseOnce),
            CleanupIntent::Exit => Some(CoordinatorEffect::ReplayExitOnce),
            CleanupIntent::None | CleanupIntent::Cancel => None,
        }
    }

    /// Consume the Tauri event caused by the armed replay without beginning a
    /// second cleanup/replay cycle.
    pub fn consume_replayed_intent(&mut self, intent: CleanupIntent) -> bool {
        let (armed, follow_up, after_close) = match &self.state {
            State::ReplayArmed {
                armed,
                follow_up,
                after_close,
            } => (*armed, *follow_up, *after_close),
            _ => return false,
        };
        if armed != intent {
            return false;
        }
        self.state = if let Some(follow_up) = follow_up {
            State::ReplayPending {
                intent: follow_up,
                after_close,
            }
        } else if intent == CleanupIntent::Exit {
            State::Stopped
        } else {
            match after_close {
                PostReplay::Idle => State::Idle,
                PostReplay::RemoteCleanupUnconfirmed => State::RemoteCleanupUnconfirmed,
            }
        };
        true
    }

    pub fn stop_if_idle(&mut self) -> bool {
        if !matches!(self.state, State::Idle) {
            return false;
        }
        self.state = State::Stopped;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Coordinator = PortalParentCoordinator<String, String>;
    type TestResult<T = ()> = Result<T, String>;

    fn begin_and_accept(coordinator: &mut Coordinator) -> TestResult<u64> {
        let Some(CoordinatorEffect::ScheduleBegin { generation }) =
            coordinator.begin("operation-canary".to_owned())
        else {
            return Err("expected begin effect".to_owned());
        };
        let NativeExportStep::Begin(admission) = coordinator.prepare_native_export(generation)
        else {
            return Err("expected native export admission".to_owned());
        };
        let (effects, ()) = admission
            .begin_with(|| Ok::<_, ()>(("ticket-canary".to_owned(), ())))
            .map_err(|_| "native export admission was rejected".to_owned())?;
        assert!(matches!(
            effects.as_slice(),
            [CoordinatorEffect::WaitExportOffMain { .. }]
        ));
        Ok(generation)
    }

    fn dispatch(coordinator: &mut Coordinator) -> TestResult<u64> {
        let generation = begin_and_accept(coordinator)?;
        let effects = coordinator.export_ready(generation);
        assert!(matches!(
            effects.as_slice(),
            [CoordinatorEffect::SendPermission { .. }]
        ));
        assert!(coordinator.permission_dispatch_started(generation));
        Ok(generation)
    }

    fn assert_take_and_consume_replay(
        coordinator: &mut Coordinator,
        intent: CleanupIntent,
    ) -> TestResult {
        let Some(effect) = coordinator.take_replay() else {
            return Err("expected pending replay".to_owned());
        };
        match intent {
            CleanupIntent::CloseMain => {
                assert!(matches!(effect, CoordinatorEffect::ReplayCloseOnce));
            }
            CleanupIntent::Exit => {
                assert!(matches!(effect, CoordinatorEffect::ReplayExitOnce));
            }
            CleanupIntent::None | CleanupIntent::Cancel => {
                return Err("cleanup intent is not replayable".to_owned());
            }
        }
        assert_eq!(coordinator.phase_name(), "replay_armed");
        assert!(coordinator.take_replay().is_none());
        assert!(coordinator.consume_replayed_intent(intent));
        assert!(!coordinator.consume_replayed_intent(intent));
        Ok(())
    }

    #[test]
    fn queued_begin_observes_cancel_before_native_export() -> TestResult {
        let mut coordinator = Coordinator::new();
        let Some(CoordinatorEffect::ScheduleBegin { generation }) =
            coordinator.begin("op".to_owned())
        else {
            return Err("begin was not scheduled".to_owned());
        };
        assert!(
            coordinator
                .request_cleanup(CleanupIntent::Cancel)
                .is_empty()
        );
        assert!(matches!(
            coordinator.prepare_native_export(generation),
            NativeExportStep::Skip
        ));
        assert_eq!(coordinator.phase_name(), "idle");
        Ok(())
    }

    #[test]
    fn queued_close_skips_native_export_before_replay() -> TestResult {
        let mut coordinator = Coordinator::new();
        let Some(CoordinatorEffect::ScheduleBegin { generation }) =
            coordinator.begin("op".to_owned())
        else {
            return Err("begin was not scheduled".to_owned());
        };
        coordinator.request_cleanup(CleanupIntent::CloseMain);
        assert!(matches!(
            coordinator.prepare_native_export(generation),
            NativeExportStep::Skip
        ));
        assert_take_and_consume_replay(&mut coordinator, CleanupIntent::CloseMain)?;
        Ok(())
    }

    #[test]
    fn possible_dispatch_requires_remote_and_original_terminal_before_release() -> TestResult {
        let mut coordinator = Coordinator::new();
        let generation = dispatch(&mut coordinator)?;
        let effects = coordinator.request_cleanup(CleanupIntent::Cancel);
        assert!(matches!(
            effects.as_slice(),
            [CoordinatorEffect::CancelThenRevoke { .. }]
        ));
        assert!(
            coordinator
                .remote_cleanup_terminal(generation, RemoteCleanupTerminal::Confirmed)
                .is_empty()
        );
        assert_eq!(coordinator.phase_name(), "permission_pending");
        let effects =
            coordinator.permission_terminal(generation, PermissionTerminal::InactiveConfirmed);
        assert!(matches!(
            effects.as_slice(),
            [CoordinatorEffect::ScheduleRelease { .. }]
        ));
        let effects = coordinator.release_terminal(generation, true);
        assert!(matches!(
            effects.as_slice(),
            [CoordinatorEffect::PublishSanitizedOutcome(
                SanitizedOutcome::Cancelled
            )]
        ));
        Ok(())
    }

    #[test]
    fn possible_dispatch_waits_when_original_finishes_before_remote_cleanup() -> TestResult {
        let mut coordinator = Coordinator::new();
        let generation = dispatch(&mut coordinator)?;
        coordinator.request_cleanup(CleanupIntent::Cancel);
        assert!(
            coordinator
                .permission_terminal(generation, PermissionTerminal::Active)
                .is_empty()
        );
        assert_eq!(coordinator.phase_name(), "permission_pending");
        let effects =
            coordinator.remote_cleanup_terminal(generation, RemoteCleanupTerminal::Confirmed);
        assert!(matches!(
            effects.as_slice(),
            [CoordinatorEffect::ScheduleRelease { .. }]
        ));
        Ok(())
    }

    #[test]
    fn cancellation_before_dispatch_skips_remote_cleanup_and_releases() -> TestResult {
        let mut coordinator = Coordinator::new();
        let generation = begin_and_accept(&mut coordinator)?;
        coordinator.export_ready(generation);
        let effects = coordinator.request_cleanup(CleanupIntent::Cancel);
        assert!(matches!(
            effects.as_slice(),
            [CoordinatorEffect::ScheduleRelease { .. }]
        ));
        assert!(!coordinator.permission_dispatch_started(generation));
        let effects = coordinator.release_terminal(generation, true);
        assert!(matches!(
            effects.as_slice(),
            [CoordinatorEffect::PublishSanitizedOutcome(
                SanitizedOutcome::Cancelled
            )]
        ));
        Ok(())
    }

    #[test]
    fn normal_active_terminal_releases_parent_before_granted_publication() -> TestResult {
        let mut coordinator = Coordinator::new();
        let generation = dispatch(&mut coordinator)?;
        let effects = coordinator.permission_terminal(generation, PermissionTerminal::Active);
        assert!(matches!(
            effects.as_slice(),
            [CoordinatorEffect::ScheduleRelease { .. }]
        ));
        assert_eq!(coordinator.phase_name(), "releasing");
        let effects = coordinator.release_terminal(generation, true);
        assert!(matches!(
            effects.as_slice(),
            [CoordinatorEffect::PublishSanitizedOutcome(
                SanitizedOutcome::PermissionActive
            )]
        ));
        assert_eq!(coordinator.phase_name(), "granted");
        Ok(())
    }

    #[test]
    fn release_failure_blocks_success_and_replay() -> TestResult {
        let mut coordinator = Coordinator::new();
        let generation = dispatch(&mut coordinator)?;
        coordinator.request_cleanup(CleanupIntent::CloseMain);
        coordinator.remote_cleanup_terminal(generation, RemoteCleanupTerminal::Confirmed);
        coordinator.permission_terminal(generation, PermissionTerminal::InactiveConfirmed);
        assert!(coordinator.release_terminal(generation, false).is_empty());
        assert_eq!(coordinator.phase_name(), "local_release_unconfirmed");
        assert!(coordinator.request_cleanup(CleanupIntent::Exit).is_empty());
        assert!(coordinator.take_replay().is_none());
        Ok(())
    }

    #[test]
    fn close_after_active_terminal_but_before_release_revokes_before_replay() -> TestResult {
        let mut coordinator = Coordinator::new();
        let generation = dispatch(&mut coordinator)?;
        coordinator.permission_terminal(generation, PermissionTerminal::Active);
        assert_eq!(coordinator.phase_name(), "releasing");
        assert!(
            coordinator
                .request_cleanup(CleanupIntent::CloseMain)
                .is_empty()
        );

        let effects = coordinator.release_terminal(generation, true);
        assert!(matches!(
            effects.as_slice(),
            [CoordinatorEffect::RevokeGranted { .. }]
        ));
        assert_eq!(coordinator.phase_name(), "revoking_granted");
        let effects = coordinator.granted_revoke_terminal(generation, true);
        assert!(matches!(
            effects.as_slice(),
            [CoordinatorEffect::PublishSanitizedOutcome(
                SanitizedOutcome::Cancelled
            )]
        ));
        assert_take_and_consume_replay(&mut coordinator, CleanupIntent::CloseMain)?;
        Ok(())
    }

    #[test]
    fn duplicate_cleanup_merges_to_exit_and_starts_one_remote_cleanup() -> TestResult {
        let mut coordinator = Coordinator::new();
        let generation = dispatch(&mut coordinator)?;
        assert_eq!(
            coordinator.request_cleanup(CleanupIntent::CloseMain).len(),
            1
        );
        assert!(coordinator.request_cleanup(CleanupIntent::Exit).is_empty());
        coordinator.remote_cleanup_terminal(generation, RemoteCleanupTerminal::Confirmed);
        coordinator.permission_terminal(generation, PermissionTerminal::InactiveConfirmed);
        let effects = coordinator.release_terminal(generation, true);
        assert!(matches!(
            effects.as_slice(),
            [CoordinatorEffect::PublishSanitizedOutcome(
                SanitizedOutcome::Cancelled
            )]
        ));
        assert_take_and_consume_replay(&mut coordinator, CleanupIntent::Exit)?;
        Ok(())
    }

    #[test]
    fn granted_exit_requires_revoke_before_replay() -> TestResult {
        let mut coordinator = Coordinator::new();
        let generation = dispatch(&mut coordinator)?;
        coordinator.permission_terminal(generation, PermissionTerminal::Active);
        coordinator.release_terminal(generation, true);
        let effects = coordinator.request_cleanup(CleanupIntent::Exit);
        assert!(matches!(
            effects.as_slice(),
            [CoordinatorEffect::RevokeGranted { .. }]
        ));
        let effects = coordinator.granted_revoke_terminal(generation, true);
        assert!(matches!(
            effects.as_slice(),
            [CoordinatorEffect::PublishSanitizedOutcome(
                SanitizedOutcome::Cancelled
            )]
        ));
        assert_take_and_consume_replay(&mut coordinator, CleanupIntent::Exit)?;
        Ok(())
    }

    #[test]
    fn stale_completions_cannot_overwrite_newer_or_cleanup_state() -> TestResult {
        let mut coordinator = Coordinator::new();
        let generation = dispatch(&mut coordinator)?;
        assert!(
            coordinator
                .permission_terminal(generation + 1, PermissionTerminal::Active)
                .is_empty()
        );
        assert_eq!(coordinator.phase_name(), "permission_pending");
        coordinator.request_cleanup(CleanupIntent::Cancel);
        coordinator.remote_cleanup_terminal(generation, RemoteCleanupTerminal::Unconfirmed);
        coordinator.permission_terminal(generation, PermissionTerminal::InactiveConfirmed);
        coordinator.release_terminal(generation, true);
        assert_eq!(coordinator.phase_name(), "remote_cleanup_unconfirmed");
        assert!(
            coordinator
                .permission_terminal(generation, PermissionTerminal::Active)
                .is_empty()
        );
        assert_eq!(coordinator.phase_name(), "remote_cleanup_unconfirmed");
        Ok(())
    }

    #[test]
    fn ambiguous_dispatch_forces_remote_cleanup_before_local_release() -> TestResult {
        let mut coordinator = Coordinator::new();
        let generation = dispatch(&mut coordinator)?;
        let effects =
            coordinator.permission_terminal(generation, PermissionTerminal::CleanupUnconfirmed);
        assert!(matches!(
            effects.as_slice(),
            [CoordinatorEffect::CancelThenRevoke { .. }]
        ));
        assert_eq!(coordinator.phase_name(), "permission_pending");

        let effects =
            coordinator.remote_cleanup_terminal(generation, RemoteCleanupTerminal::Confirmed);
        assert!(matches!(
            effects.as_slice(),
            [CoordinatorEffect::ScheduleRelease { .. }]
        ));
        let effects = coordinator.release_terminal(generation, true);
        assert!(matches!(
            effects.as_slice(),
            [CoordinatorEffect::PublishSanitizedOutcome(
                SanitizedOutcome::Cancelled
            )]
        ));
        assert_eq!(coordinator.phase_name(), "idle");
        Ok(())
    }

    #[test]
    fn remote_cleanup_uncertainty_replays_close_but_remains_fail_closed() -> TestResult {
        let mut coordinator = Coordinator::new();
        let generation = dispatch(&mut coordinator)?;
        coordinator.permission_terminal(generation, PermissionTerminal::CleanupUnconfirmed);
        let effects =
            coordinator.remote_cleanup_terminal(generation, RemoteCleanupTerminal::Unconfirmed);
        assert!(matches!(
            effects.as_slice(),
            [CoordinatorEffect::ScheduleRelease { .. }]
        ));
        let effects = coordinator.release_terminal(generation, true);
        assert!(matches!(
            effects.as_slice(),
            [CoordinatorEffect::PublishSanitizedOutcome(
                SanitizedOutcome::CleanupUnconfirmed
            )]
        ));
        assert_eq!(coordinator.phase_name(), "remote_cleanup_unconfirmed");
        assert!(coordinator.begin("blocked".to_owned()).is_none());

        assert!(
            coordinator
                .request_cleanup(CleanupIntent::CloseMain)
                .is_empty()
        );
        assert_take_and_consume_replay(&mut coordinator, CleanupIntent::CloseMain)?;
        assert_eq!(coordinator.phase_name(), "remote_cleanup_unconfirmed");
        assert!(coordinator.begin("still-blocked".to_owned()).is_none());
        Ok(())
    }

    #[test]
    fn stale_release_terminals_preserve_matching_release_state() -> TestResult {
        let mut coordinator = Coordinator::new();
        let generation = dispatch(&mut coordinator)?;
        coordinator.permission_terminal(generation, PermissionTerminal::InactiveConfirmed);
        assert!(
            coordinator
                .release_terminal(generation + 1, true)
                .is_empty()
        );
        assert_eq!(coordinator.phase_name(), "releasing");
        assert!(matches!(
            coordinator.release_terminal(generation, true).as_slice(),
            [CoordinatorEffect::PublishSanitizedOutcome(
                SanitizedOutcome::PermissionInactive
            )]
        ));

        let next_generation = dispatch(&mut coordinator)?;
        coordinator.permission_terminal(next_generation, PermissionTerminal::Active);
        coordinator.release_terminal(next_generation, true);
        coordinator.request_cleanup(CleanupIntent::CloseMain);
        assert!(
            coordinator
                .granted_revoke_terminal(next_generation + 1, true)
                .is_empty()
        );
        assert_eq!(coordinator.phase_name(), "revoking_granted");
        assert!(matches!(
            coordinator
                .granted_revoke_terminal(next_generation, true)
                .as_slice(),
            [CoordinatorEffect::PublishSanitizedOutcome(
                SanitizedOutcome::Cancelled
            )]
        ));
        Ok(())
    }

    #[test]
    fn dropped_unused_native_admission_is_fail_closed_without_native_work() -> TestResult {
        let mut coordinator = Coordinator::new();
        let Some(CoordinatorEffect::ScheduleBegin { generation }) =
            coordinator.begin("old".to_owned())
        else {
            return Err("begin was not scheduled".to_owned());
        };
        let NativeExportStep::Begin(admission) = coordinator.prepare_native_export(generation)
        else {
            return Err("native export admission was not prepared".to_owned());
        };
        drop(admission);
        assert_eq!(coordinator.phase_name(), "local_release_unconfirmed");
        assert!(coordinator.begin("blocked".to_owned()).is_none());
        assert!(coordinator.take_replay().is_none());
        Ok(())
    }

    #[test]
    fn accepted_export_wait_failure_releases_before_close_replay() -> TestResult {
        let mut coordinator = Coordinator::new();
        let generation = begin_and_accept(&mut coordinator)?;
        let effects = coordinator.export_wait_failed(generation);
        assert!(matches!(
            effects.as_slice(),
            [CoordinatorEffect::ScheduleRelease { .. }]
        ));
        assert!(
            coordinator
                .request_cleanup(CleanupIntent::CloseMain)
                .is_empty()
        );
        let effects = coordinator.release_terminal(generation, true);
        assert!(matches!(
            effects.as_slice(),
            [CoordinatorEffect::PublishSanitizedOutcome(
                SanitizedOutcome::ParentExportFailed
            )]
        ));
        assert_take_and_consume_replay(&mut coordinator, CleanupIntent::CloseMain)?;
        Ok(())
    }

    #[test]
    fn stale_wait_failure_preserves_ticket_and_release_failure_suppresses_replay() -> TestResult {
        let mut coordinator = Coordinator::new();
        let generation = begin_and_accept(&mut coordinator)?;
        assert!(coordinator.export_wait_failed(generation + 1).is_empty());
        assert_eq!(coordinator.phase_name(), "export_pending");
        assert!(matches!(
            coordinator.export_wait_failed(generation).as_slice(),
            [CoordinatorEffect::ScheduleRelease { .. }]
        ));
        coordinator.request_cleanup(CleanupIntent::Exit);
        assert!(coordinator.release_terminal(generation, false).is_empty());
        assert_eq!(coordinator.phase_name(), "local_release_unconfirmed");
        assert!(coordinator.take_replay().is_none());
        Ok(())
    }

    #[test]
    fn stronger_exit_queued_after_close_is_replayed_exactly_once() -> TestResult {
        let mut coordinator = Coordinator::new();
        let generation = dispatch(&mut coordinator)?;
        coordinator.request_cleanup(CleanupIntent::CloseMain);
        coordinator.remote_cleanup_terminal(generation, RemoteCleanupTerminal::Confirmed);
        coordinator.permission_terminal(generation, PermissionTerminal::InactiveConfirmed);
        coordinator.release_terminal(generation, true);

        assert!(matches!(
            coordinator.take_replay(),
            Some(CoordinatorEffect::ReplayCloseOnce)
        ));
        coordinator.request_cleanup(CleanupIntent::Exit);
        assert!(coordinator.consume_replayed_intent(CleanupIntent::CloseMain));
        assert!(matches!(
            coordinator.take_replay(),
            Some(CoordinatorEffect::ReplayExitOnce)
        ));
        assert!(coordinator.consume_replayed_intent(CleanupIntent::Exit));
        assert_eq!(coordinator.phase_name(), "stopped");
        assert!(coordinator.take_replay().is_none());
        assert!(!coordinator.consume_replayed_intent(CleanupIntent::Exit));
        Ok(())
    }

    #[test]
    fn debug_output_never_contains_operation_or_ticket() -> TestResult {
        let mut coordinator = Coordinator::new();
        let generation = begin_and_accept(&mut coordinator)?;
        let coordinator_debug = format!("{coordinator:?}");
        assert!(!coordinator_debug.contains("operation-canary"));
        assert!(!coordinator_debug.contains("ticket-canary"));

        let effects = coordinator.export_ready(generation);
        let effect_debug = format!("{:?}", effects[0]);
        assert!(!effect_debug.contains("operation-canary"));
        assert!(!effect_debug.contains("ticket-canary"));
        Ok(())
    }

    #[test]
    fn overlap_is_rejected_and_idle_can_stop() {
        let mut coordinator = Coordinator::new();
        assert!(coordinator.begin("one".to_owned()).is_some());
        assert!(coordinator.begin("two".to_owned()).is_none());
        let mut idle = Coordinator::new();
        assert!(idle.stop_if_idle());
        assert_eq!(idle.phase_name(), "stopped");
        assert!(idle.begin("late".to_owned()).is_none());
    }
}
