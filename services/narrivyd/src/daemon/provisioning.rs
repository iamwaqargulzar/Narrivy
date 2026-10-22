//! Pure state coordinator for daemon-owned offline model provisioning.
//!
//! This module deliberately owns no mutex, worker, filesystem path, or
//! process. The daemon will place one coordinator behind its resident
//! lifecycle mutex in the integration work item. Keeping admission and every
//! terminal transition in this value makes it impossible for two separately
//! locked fields to disagree about the active operation.

#![forbid(unsafe_code)]
// This bounded work item establishes the coordinator and its tests before the
// following work item wires it into `DaemonState`.
#![allow(dead_code)]

use std::fmt;
use std::time::{Duration, Instant};

use narrivy_ipc::{
    ModelProvisionCleanupDto, ModelProvisionFailureDto, ModelProvisionPublicationDto,
    ModelProvisionStateDto, ModelProvisionStatusDto,
};
use narrivy_types::SessionCancellation;

/// One unforgeable-in-process operation identity.
///
/// The wire-visible id is daemon-generated and opaque. The private generation
/// additionally prevents a stale worker from committing into a later
/// operation even if an id generator is faulty or a caller replays an id.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ProvisionOperationKey {
    operation_id: String,
    generation: u64,
}

impl ProvisionOperationKey {
    pub(crate) fn operation_id(&self) -> &str {
        &self.operation_id
    }
}

impl fmt::Debug for ProvisionOperationKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProvisionOperationKey")
            .field("operation_id", &"<redacted>")
            .field("generation", &self.generation)
            .finish()
    }
}

/// Coarse resident lifecycle. Fine-grained provisioning progress is carried
/// by [`ModelProvisionStatusDto`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProvisionPhaseKind {
    Unconfigured,
    Awaiting,
    Provisioning,
    Ready,
    PublishedUnconfirmed,
    ActivationFailed,
    Closing,
}

enum ProvisionPhase<Runtime, Candidate> {
    Unconfigured,
    Awaiting(Runtime),
    Provisioning {
        key: ProvisionOperationKey,
        cancellation: SessionCancellation,
    },
    Ready(Candidate),
    PublishedUnconfirmed,
    ActivationFailed,
    Closing,
}

/// Admission granted under the daemon's single lifecycle lock.
///
/// The runtime moves to the worker. A clean pre-publication terminal result
/// returns it to the coordinator; fail-closed results intentionally do not.
pub(crate) struct ProvisionAdmission<Runtime> {
    pub(crate) key: ProvisionOperationKey,
    pub(crate) cancellation: SessionCancellation,
    pub(crate) runtime: Runtime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProvisionStartError {
    Unconfigured,
    Busy,
    AlreadyReady,
    FailClosed,
    Closing,
    GenerationExhausted,
    Cooldown { remaining: Duration },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProvisionTransitionError {
    StaleOperation,
    CancellationRequested,
    InvalidPhase,
    Closing,
    RevisionExhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProvisionCommitRejectReason {
    StaleOperation,
    InvalidPhase,
    Closing,
    RevisionExhausted,
}

/// Result of committing a newly activated candidate.
///
/// Rejection returns ownership so the worker can boundedly shut it down.
pub(crate) enum CandidateCommit<Candidate> {
    Committed,
    Rejected {
        candidate: Candidate,
        reason: ProvisionCommitRejectReason,
    },
}

/// Typed terminal worker outcomes. The variants encode which outcomes are
/// safe to retry and which must leave the daemon fail-closed.
pub(crate) enum ProvisionTerminal<Runtime> {
    CancelledBeforePublicationClean { runtime: Runtime },
    TimedOutBeforePublicationClean { runtime: Runtime },
    SourceRejectedBeforeStaging { runtime: Runtime },
    SourceRejectedAfterStagingClean { runtime: Runtime },
    ImportFailedClean { runtime: Runtime },
    WorkerFailedBeforePublicationClean { runtime: Runtime },
    ImportFailedClosed,
    ImportFailedClosedAfterStagingClean,
    CleanupFailed,
    PublishedUnconfirmed,
    BindingFailed,
    ActivationFailed,
    CancelledAfterPublication,
    TimedOutAfterPublication,
    WorkerFailedUncertain,
}

/// Result of applying a terminal worker outcome.
///
/// A stale/closing result is returned intact so its owner decides how to
/// clean any resources it contains.
pub(crate) enum TerminalCommit<Runtime> {
    Committed,
    Rejected {
        terminal: ProvisionTerminal<Runtime>,
        reason: ProvisionCommitRejectReason,
    },
}

/// One mutex-owner-compatible lifecycle value. It has no internal locking,
/// background work, OS handles, or callbacks.
pub(crate) struct ProvisionCoordinator<Runtime, Candidate> {
    phase: ProvisionPhase<Runtime, Candidate>,
    generation: u64,
    last_started_at: Option<Instant>,
    status: Option<ModelProvisionStatusDto>,
}

impl<Runtime, Candidate> ProvisionCoordinator<Runtime, Candidate> {
    pub(crate) fn unconfigured() -> Self {
        Self {
            phase: ProvisionPhase::Unconfigured,
            generation: 0,
            last_started_at: None,
            status: None,
        }
    }

    pub(crate) fn awaiting(runtime: Runtime) -> Self {
        Self {
            phase: ProvisionPhase::Awaiting(runtime),
            generation: 0,
            last_started_at: None,
            status: None,
        }
    }

    pub(crate) fn ready(candidate: Candidate) -> Self {
        Self {
            phase: ProvisionPhase::Ready(candidate),
            generation: 0,
            last_started_at: None,
            status: None,
        }
    }

    pub(crate) fn phase_kind(&self) -> ProvisionPhaseKind {
        match self.phase {
            ProvisionPhase::Unconfigured => ProvisionPhaseKind::Unconfigured,
            ProvisionPhase::Awaiting(_) => ProvisionPhaseKind::Awaiting,
            ProvisionPhase::Provisioning { .. } => ProvisionPhaseKind::Provisioning,
            ProvisionPhase::Ready(_) => ProvisionPhaseKind::Ready,
            ProvisionPhase::PublishedUnconfirmed => ProvisionPhaseKind::PublishedUnconfirmed,
            ProvisionPhase::ActivationFailed => ProvisionPhaseKind::ActivationFailed,
            ProvisionPhase::Closing => ProvisionPhaseKind::Closing,
        }
    }

    pub(crate) fn status(&self) -> Option<ModelProvisionStatusDto> {
        self.status.clone()
    }

    pub(crate) fn active_key(&self) -> Option<ProvisionOperationKey> {
        match &self.phase {
            ProvisionPhase::Provisioning { key, .. } => Some(key.clone()),
            _ => None,
        }
    }

    #[cfg(test)]
    #[allow(clippy::panic)]
    pub(crate) fn invalidate_active_generation_for_test(&mut self) {
        let Some(generation) = self.generation.checked_add(1) else {
            panic!("controlled test generation exhausted");
        };
        let ProvisionPhase::Provisioning { key, .. } = &mut self.phase else {
            panic!("controlled test requires an active provision");
        };
        key.generation = generation;
        self.generation = generation;
    }

    pub(crate) fn ready_candidate(&self) -> Option<&Candidate> {
        match &self.phase {
            ProvisionPhase::Ready(candidate) => Some(candidate),
            _ => None,
        }
    }

    /// Atomically applies lifecycle, single-flight, cooldown, and generation
    /// admission. The caller must invoke this while holding the one daemon
    /// lifecycle mutex.
    pub(crate) fn admit_start(
        &mut self,
        operation_id: String,
        now: Instant,
        cooldown: Duration,
    ) -> Result<ProvisionAdmission<Runtime>, ProvisionStartError> {
        match self.phase_kind() {
            ProvisionPhaseKind::Unconfigured => return Err(ProvisionStartError::Unconfigured),
            ProvisionPhaseKind::Provisioning => return Err(ProvisionStartError::Busy),
            ProvisionPhaseKind::Ready => return Err(ProvisionStartError::AlreadyReady),
            ProvisionPhaseKind::PublishedUnconfirmed | ProvisionPhaseKind::ActivationFailed => {
                return Err(ProvisionStartError::FailClosed);
            }
            ProvisionPhaseKind::Closing => return Err(ProvisionStartError::Closing),
            ProvisionPhaseKind::Awaiting => {}
        }

        if let Some(last_started_at) = self.last_started_at {
            let elapsed = now.saturating_duration_since(last_started_at);
            if elapsed < cooldown {
                return Err(ProvisionStartError::Cooldown {
                    remaining: cooldown - elapsed,
                });
            }
        }

        let generation = self
            .generation
            .checked_add(1)
            .ok_or(ProvisionStartError::GenerationExhausted)?;
        let cancellation = SessionCancellation::new();
        let key = ProvisionOperationKey {
            operation_id,
            generation,
        };
        let previous = std::mem::replace(
            &mut self.phase,
            ProvisionPhase::Provisioning {
                key: key.clone(),
                cancellation: cancellation.clone(),
            },
        );
        let ProvisionPhase::Awaiting(runtime) = previous else {
            // Phase was checked above while the same exclusive borrow is held.
            self.phase = previous;
            return Err(ProvisionStartError::Busy);
        };

        self.generation = generation;
        self.last_started_at = Some(now);
        self.status = Some(status(
            &key,
            ModelProvisionStateDto::Importing,
            ModelProvisionPublicationDto::NotPublished,
            ModelProvisionCleanupDto::NotRequired,
            None,
            1,
        ));
        Ok(ProvisionAdmission {
            key,
            cancellation,
            runtime,
        })
    }

    /// Requests cancellation only for the exact active operation. Repeated
    /// requests are idempotent and do not manufacture revision changes.
    pub(crate) fn request_cancel(
        &mut self,
        key: &ProvisionOperationKey,
    ) -> Result<ModelProvisionStatusDto, ProvisionTransitionError> {
        let ProvisionPhase::Provisioning {
            key: active_key,
            cancellation,
        } = &self.phase
        else {
            return Err(match self.phase_kind() {
                ProvisionPhaseKind::Closing => ProvisionTransitionError::Closing,
                _ => ProvisionTransitionError::StaleOperation,
            });
        };
        if active_key != key {
            return Err(ProvisionTransitionError::StaleOperation);
        }
        cancellation.cancel();

        let current = self.current_status_for(key)?;
        if current.state == ModelProvisionStateDto::CancelRequested {
            return Ok(current.clone());
        }
        let next = next_status(
            current,
            ModelProvisionStateDto::CancelRequested,
            current.publication.clone(),
            current.cleanup.clone(),
            None,
        )?;
        self.status = Some(next.clone());
        Ok(next)
    }

    /// Marks successful, verified publication and the beginning of activation.
    pub(crate) fn mark_activating(
        &mut self,
        key: &ProvisionOperationKey,
    ) -> Result<ModelProvisionStatusDto, ProvisionTransitionError> {
        self.require_active(key)?;
        let current = self.current_status_for(key)?;
        if current.state == ModelProvisionStateDto::CancelRequested {
            let next = next_status(
                current,
                ModelProvisionStateDto::CancelRequested,
                ModelProvisionPublicationDto::VerifiedInstalled,
                ModelProvisionCleanupDto::Confirmed,
                None,
            )?;
            self.status = Some(next);
            return Err(ProvisionTransitionError::CancellationRequested);
        }
        if current.state != ModelProvisionStateDto::Importing {
            return Err(ProvisionTransitionError::InvalidPhase);
        }
        let next = next_status(
            current,
            ModelProvisionStateDto::Activating,
            ModelProvisionPublicationDto::VerifiedInstalled,
            ModelProvisionCleanupDto::Confirmed,
            None,
        )?;
        self.status = Some(next.clone());
        Ok(next)
    }

    /// Commits a started resident candidate only for the exact active
    /// operation in `Activating`. Any stale or closing result returns the
    /// candidate to its caller for bounded cleanup.
    pub(crate) fn commit_ready(
        &mut self,
        key: &ProvisionOperationKey,
        candidate: Candidate,
    ) -> CandidateCommit<Candidate> {
        let rejection = match self.phase_kind() {
            ProvisionPhaseKind::Closing => Some(ProvisionCommitRejectReason::Closing),
            ProvisionPhaseKind::Provisioning => {
                let active_matches = matches!(
                    &self.phase,
                    ProvisionPhase::Provisioning { key: active, .. } if active == key
                );
                if !active_matches {
                    Some(ProvisionCommitRejectReason::StaleOperation)
                } else {
                    let valid_status = self.status.as_ref().is_some_and(|status| {
                        status.operation_id == key.operation_id
                            && status.state == ModelProvisionStateDto::Activating
                            && status.publication == ModelProvisionPublicationDto::VerifiedInstalled
                    });
                    (!valid_status).then_some(ProvisionCommitRejectReason::InvalidPhase)
                }
            }
            _ => Some(ProvisionCommitRejectReason::StaleOperation),
        };
        if let Some(reason) = rejection {
            return CandidateCommit::Rejected { candidate, reason };
        }

        let Some(current) = self.status.as_ref() else {
            return CandidateCommit::Rejected {
                candidate,
                reason: ProvisionCommitRejectReason::InvalidPhase,
            };
        };
        let Ok(next) = next_status(
            current,
            ModelProvisionStateDto::Ready,
            ModelProvisionPublicationDto::VerifiedInstalled,
            ModelProvisionCleanupDto::Confirmed,
            None,
        ) else {
            return CandidateCommit::Rejected {
                candidate,
                reason: ProvisionCommitRejectReason::RevisionExhausted,
            };
        };
        self.phase = ProvisionPhase::Ready(candidate);
        self.status = Some(next);
        CandidateCommit::Committed
    }

    /// Applies a typed terminal path for the exact operation. Stale results
    /// are returned instead of being silently dropped.
    pub(crate) fn complete_terminal(
        &mut self,
        key: &ProvisionOperationKey,
        terminal: ProvisionTerminal<Runtime>,
    ) -> TerminalCommit<Runtime> {
        let rejection = match self.phase_kind() {
            ProvisionPhaseKind::Closing => Some(ProvisionCommitRejectReason::Closing),
            ProvisionPhaseKind::Provisioning => {
                let matches = matches!(
                    &self.phase,
                    ProvisionPhase::Provisioning { key: active, .. } if active == key
                );
                (!matches).then_some(ProvisionCommitRejectReason::StaleOperation)
            }
            _ => Some(ProvisionCommitRejectReason::StaleOperation),
        };
        if let Some(reason) = rejection {
            return TerminalCommit::Rejected { terminal, reason };
        }

        let Some(current) = self.status.as_ref() else {
            return TerminalCommit::Rejected {
                terminal,
                reason: ProvisionCommitRejectReason::InvalidPhase,
            };
        };
        let terminal = normalize_cancelled_terminal(terminal, current);
        if !terminal_is_valid_for_status(&terminal, current) {
            return TerminalCommit::Rejected {
                terminal,
                reason: ProvisionCommitRejectReason::InvalidPhase,
            };
        }
        let Some(revision) = current.revision.checked_add(1) else {
            return TerminalCommit::Rejected {
                terminal,
                reason: ProvisionCommitRejectReason::RevisionExhausted,
            };
        };
        let (phase, state, publication, cleanup, failure) = match terminal {
            ProvisionTerminal::CancelledBeforePublicationClean { runtime } => (
                ProvisionPhase::Awaiting(runtime),
                ModelProvisionStateDto::Cancelled,
                ModelProvisionPublicationDto::NotPublished,
                ModelProvisionCleanupDto::Confirmed,
                None,
            ),
            ProvisionTerminal::TimedOutBeforePublicationClean { runtime } => (
                ProvisionPhase::Awaiting(runtime),
                ModelProvisionStateDto::TimedOut,
                ModelProvisionPublicationDto::NotPublished,
                ModelProvisionCleanupDto::Confirmed,
                None,
            ),
            ProvisionTerminal::SourceRejectedBeforeStaging { runtime } => (
                ProvisionPhase::Awaiting(runtime),
                ModelProvisionStateDto::Failed,
                ModelProvisionPublicationDto::NotPublished,
                ModelProvisionCleanupDto::NotRequired,
                Some(ModelProvisionFailureDto::SourceRejected),
            ),
            ProvisionTerminal::SourceRejectedAfterStagingClean { runtime } => (
                ProvisionPhase::Awaiting(runtime),
                ModelProvisionStateDto::Failed,
                ModelProvisionPublicationDto::NotPublished,
                ModelProvisionCleanupDto::Confirmed,
                Some(ModelProvisionFailureDto::SourceRejected),
            ),
            ProvisionTerminal::ImportFailedClean { runtime } => (
                ProvisionPhase::Awaiting(runtime),
                ModelProvisionStateDto::Failed,
                ModelProvisionPublicationDto::NotPublished,
                ModelProvisionCleanupDto::Confirmed,
                Some(ModelProvisionFailureDto::ImportFailed),
            ),
            ProvisionTerminal::WorkerFailedBeforePublicationClean { runtime } => (
                ProvisionPhase::Awaiting(runtime),
                ModelProvisionStateDto::Failed,
                ModelProvisionPublicationDto::NotPublished,
                ModelProvisionCleanupDto::Confirmed,
                Some(ModelProvisionFailureDto::WorkerFailed),
            ),
            ProvisionTerminal::ImportFailedClosed => (
                ProvisionPhase::ActivationFailed,
                ModelProvisionStateDto::Failed,
                ModelProvisionPublicationDto::NotPublished,
                ModelProvisionCleanupDto::NotRequired,
                Some(ModelProvisionFailureDto::ImportFailed),
            ),
            ProvisionTerminal::ImportFailedClosedAfterStagingClean => (
                ProvisionPhase::ActivationFailed,
                ModelProvisionStateDto::Failed,
                ModelProvisionPublicationDto::NotPublished,
                ModelProvisionCleanupDto::Confirmed,
                Some(ModelProvisionFailureDto::ImportFailed),
            ),
            ProvisionTerminal::CleanupFailed => (
                ProvisionPhase::ActivationFailed,
                ModelProvisionStateDto::Failed,
                ModelProvisionPublicationDto::NotPublished,
                ModelProvisionCleanupDto::Failed,
                Some(ModelProvisionFailureDto::CleanupFailed),
            ),
            ProvisionTerminal::PublishedUnconfirmed => (
                ProvisionPhase::PublishedUnconfirmed,
                ModelProvisionStateDto::Failed,
                ModelProvisionPublicationDto::PublishedUnconfirmed,
                ModelProvisionCleanupDto::Confirmed,
                Some(ModelProvisionFailureDto::PublishedUnconfirmed),
            ),
            ProvisionTerminal::BindingFailed => (
                ProvisionPhase::ActivationFailed,
                ModelProvisionStateDto::Failed,
                ModelProvisionPublicationDto::VerifiedInstalled,
                ModelProvisionCleanupDto::Confirmed,
                Some(ModelProvisionFailureDto::BindingFailed),
            ),
            ProvisionTerminal::ActivationFailed => (
                ProvisionPhase::ActivationFailed,
                ModelProvisionStateDto::Failed,
                ModelProvisionPublicationDto::VerifiedInstalled,
                ModelProvisionCleanupDto::Confirmed,
                Some(ModelProvisionFailureDto::ActivationFailed),
            ),
            ProvisionTerminal::CancelledAfterPublication => (
                ProvisionPhase::ActivationFailed,
                ModelProvisionStateDto::Cancelled,
                ModelProvisionPublicationDto::VerifiedInstalled,
                ModelProvisionCleanupDto::Confirmed,
                None,
            ),
            ProvisionTerminal::TimedOutAfterPublication => (
                ProvisionPhase::ActivationFailed,
                ModelProvisionStateDto::TimedOut,
                ModelProvisionPublicationDto::VerifiedInstalled,
                ModelProvisionCleanupDto::Confirmed,
                None,
            ),
            ProvisionTerminal::WorkerFailedUncertain => (
                ProvisionPhase::ActivationFailed,
                ModelProvisionStateDto::Failed,
                current.publication.clone(),
                ModelProvisionCleanupDto::Failed,
                Some(ModelProvisionFailureDto::WorkerFailed),
            ),
        };
        let next = ModelProvisionStatusDto {
            operation_id: current.operation_id.clone(),
            state,
            publication,
            cleanup,
            failure,
            revision,
        };
        self.phase = phase;
        self.status = Some(next);
        TerminalCommit::Committed
    }

    /// Atomically closes admission, signals an active operation, and extracts
    /// an installed candidate for bounded shutdown by its owner.
    pub(crate) fn close(&mut self) -> Option<Candidate> {
        let previous = std::mem::replace(&mut self.phase, ProvisionPhase::Closing);
        match previous {
            ProvisionPhase::Provisioning { key, cancellation } => {
                cancellation.cancel();
                if let Some(current) = self.status.as_ref() {
                    if current.operation_id == key.operation_id {
                        if let Ok(next) = next_status(
                            current,
                            ModelProvisionStateDto::Failed,
                            current.publication.clone(),
                            current.cleanup.clone(),
                            Some(ModelProvisionFailureDto::ShuttingDown),
                        ) {
                            self.status = Some(next);
                        }
                    }
                }
                None
            }
            ProvisionPhase::Ready(candidate) => Some(candidate),
            _ => None,
        }
    }

    fn require_active(&self, key: &ProvisionOperationKey) -> Result<(), ProvisionTransitionError> {
        match &self.phase {
            ProvisionPhase::Provisioning { key: active, .. } if active == key => Ok(()),
            ProvisionPhase::Closing => Err(ProvisionTransitionError::Closing),
            _ => Err(ProvisionTransitionError::StaleOperation),
        }
    }

    fn current_status_for(
        &self,
        key: &ProvisionOperationKey,
    ) -> Result<&ModelProvisionStatusDto, ProvisionTransitionError> {
        let current = self
            .status
            .as_ref()
            .ok_or(ProvisionTransitionError::InvalidPhase)?;
        if current.operation_id != key.operation_id {
            return Err(ProvisionTransitionError::StaleOperation);
        }
        Ok(current)
    }
}

fn normalize_cancelled_terminal<Runtime>(
    terminal: ProvisionTerminal<Runtime>,
    current: &ModelProvisionStatusDto,
) -> ProvisionTerminal<Runtime> {
    if current.state != ModelProvisionStateDto::CancelRequested {
        return terminal;
    }
    match terminal {
        ProvisionTerminal::CancelledBeforePublicationClean { runtime }
        | ProvisionTerminal::TimedOutBeforePublicationClean { runtime }
        | ProvisionTerminal::SourceRejectedBeforeStaging { runtime }
        | ProvisionTerminal::SourceRejectedAfterStagingClean { runtime }
        | ProvisionTerminal::ImportFailedClean { runtime }
        | ProvisionTerminal::WorkerFailedBeforePublicationClean { runtime } => {
            ProvisionTerminal::CancelledBeforePublicationClean { runtime }
        }
        ProvisionTerminal::BindingFailed
        | ProvisionTerminal::ActivationFailed
        | ProvisionTerminal::CancelledAfterPublication
        | ProvisionTerminal::TimedOutAfterPublication => {
            ProvisionTerminal::CancelledAfterPublication
        }
        other => other,
    }
}

fn terminal_is_valid_for_status<Runtime>(
    terminal: &ProvisionTerminal<Runtime>,
    current: &ModelProvisionStatusDto,
) -> bool {
    let before_publication = current.publication == ModelProvisionPublicationDto::NotPublished;
    let after_publication = current.publication == ModelProvisionPublicationDto::VerifiedInstalled;
    let cancellation_requested = current.state == ModelProvisionStateDto::CancelRequested;

    match terminal {
        ProvisionTerminal::CancelledBeforePublicationClean { .. }
        | ProvisionTerminal::TimedOutBeforePublicationClean { .. } => before_publication,
        ProvisionTerminal::SourceRejectedBeforeStaging { .. }
        | ProvisionTerminal::SourceRejectedAfterStagingClean { .. }
        | ProvisionTerminal::ImportFailedClean { .. } => {
            before_publication && !cancellation_requested
        }
        ProvisionTerminal::WorkerFailedBeforePublicationClean { .. }
        | ProvisionTerminal::ImportFailedClosed
        | ProvisionTerminal::ImportFailedClosedAfterStagingClean
        | ProvisionTerminal::CleanupFailed
        | ProvisionTerminal::PublishedUnconfirmed => before_publication,
        ProvisionTerminal::BindingFailed | ProvisionTerminal::ActivationFailed => {
            after_publication && !cancellation_requested
        }
        ProvisionTerminal::CancelledAfterPublication
        | ProvisionTerminal::TimedOutAfterPublication => after_publication,
        ProvisionTerminal::WorkerFailedUncertain => true,
    }
}

fn status(
    key: &ProvisionOperationKey,
    state: ModelProvisionStateDto,
    publication: ModelProvisionPublicationDto,
    cleanup: ModelProvisionCleanupDto,
    failure: Option<ModelProvisionFailureDto>,
    revision: u64,
) -> ModelProvisionStatusDto {
    ModelProvisionStatusDto {
        operation_id: key.operation_id.clone(),
        state,
        publication,
        cleanup,
        failure,
        revision,
    }
}

fn next_status(
    current: &ModelProvisionStatusDto,
    state: ModelProvisionStateDto,
    publication: ModelProvisionPublicationDto,
    cleanup: ModelProvisionCleanupDto,
    failure: Option<ModelProvisionFailureDto>,
) -> Result<ModelProvisionStatusDto, ProvisionTransitionError> {
    let revision = current
        .revision
        .checked_add(1)
        .ok_or(ProvisionTransitionError::RevisionExhausted)?;
    Ok(ModelProvisionStatusDto {
        operation_id: current.operation_id.clone(),
        state,
        publication,
        cleanup,
        failure,
        revision,
    })
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    const COOLDOWN: Duration = Duration::from_secs(30);

    fn admit(
        coordinator: &mut ProvisionCoordinator<&'static str, &'static str>,
        operation_id: &str,
        now: Instant,
    ) -> ProvisionAdmission<&'static str> {
        match coordinator.admit_start(operation_id.to_owned(), now, COOLDOWN) {
            Ok(admission) => admission,
            Err(error) => panic!("unexpected admission failure: {error:?}"),
        }
    }

    fn assert_committed<Runtime>(commit: TerminalCommit<Runtime>) {
        assert!(matches!(commit, TerminalCommit::Committed));
    }

    #[test]
    fn admission_is_single_flight_and_operation_keys_are_generation_bound() {
        let now = Instant::now();
        let mut coordinator = ProvisionCoordinator::<_, &str>::awaiting("runtime");
        let first = admit(&mut coordinator, "opaque", now);

        assert_eq!(coordinator.phase_kind(), ProvisionPhaseKind::Provisioning);
        assert_eq!(first.runtime, "runtime");
        assert!(!first.cancellation.is_cancelled());
        assert!(matches!(
            coordinator.admit_start("second".into(), now + COOLDOWN, COOLDOWN),
            Err(ProvisionStartError::Busy)
        ));

        let replayed_id_with_stale_generation = ProvisionOperationKey {
            operation_id: first.key.operation_id.clone(),
            generation: first.key.generation + 1,
        };
        assert_eq!(
            coordinator.request_cancel(&replayed_id_with_stale_generation),
            Err(ProvisionTransitionError::StaleOperation)
        );
        assert!(!first.cancellation.is_cancelled());
    }

    #[test]
    fn cooldown_is_exact_and_preserves_runtime_on_rejection() {
        let now = Instant::now();
        let mut coordinator = ProvisionCoordinator::<_, &str>::awaiting("runtime");
        let first = admit(&mut coordinator, "one", now);
        assert_committed(coordinator.complete_terminal(
            &first.key,
            ProvisionTerminal::SourceRejectedBeforeStaging {
                runtime: first.runtime,
            },
        ));

        assert!(matches!(
            coordinator.admit_start("two".into(), now + Duration::from_secs(29), COOLDOWN),
            Err(ProvisionStartError::Cooldown { remaining })
                if remaining == Duration::from_secs(1)
        ));
        let second = admit(&mut coordinator, "two", now + COOLDOWN);
        assert_eq!(second.runtime, "runtime");
        assert_ne!(first.key.generation, second.key.generation);
    }

    #[test]
    fn cancellation_is_exact_idempotent_and_revisioned() {
        let now = Instant::now();
        let mut coordinator = ProvisionCoordinator::<_, &str>::awaiting("runtime");
        let admission = admit(&mut coordinator, "cancel-me", now);

        let first = coordinator
            .request_cancel(&admission.key)
            .unwrap_or_else(|error| panic!("unexpected cancellation failure: {error:?}"));
        let second = coordinator
            .request_cancel(&admission.key)
            .unwrap_or_else(|error| panic!("unexpected cancellation failure: {error:?}"));
        assert!(admission.cancellation.is_cancelled());
        assert_eq!(first.state, ModelProvisionStateDto::CancelRequested);
        assert_eq!(first.revision, 2);
        assert_eq!(second.revision, 2);
        assert_eq!(
            coordinator.mark_activating(&admission.key),
            Err(ProvisionTransitionError::CancellationRequested)
        );
        let published = coordinator.status().expect("published cancellation status");
        assert_eq!(published.state, ModelProvisionStateDto::CancelRequested);
        assert_eq!(
            published.publication,
            ModelProvisionPublicationDto::VerifiedInstalled
        );
        assert_eq!(published.revision, 3);
    }

    #[test]
    fn cancellation_linearization_normalizes_concurrent_terminal_results() {
        let now = Instant::now();
        let mut before = ProvisionCoordinator::<_, &str>::awaiting("runtime");
        let admission = admit(&mut before, "before-publication", now);
        before
            .request_cancel(&admission.key)
            .expect("cancel request");
        assert_committed(before.complete_terminal(
            &admission.key,
            ProvisionTerminal::SourceRejectedBeforeStaging {
                runtime: admission.runtime,
            },
        ));
        let status = before.status().expect("cancelled status");
        assert_eq!(before.phase_kind(), ProvisionPhaseKind::Awaiting);
        assert_eq!(status.state, ModelProvisionStateDto::Cancelled);
        assert_eq!(
            status.publication,
            ModelProvisionPublicationDto::NotPublished
        );
        assert_eq!(status.cleanup, ModelProvisionCleanupDto::Confirmed);

        let mut after = ProvisionCoordinator::<_, &str>::awaiting("runtime");
        let admission = admit(&mut after, "after-publication", now);
        after.mark_activating(&admission.key).expect("activating");
        after
            .request_cancel(&admission.key)
            .expect("cancel request");
        assert_committed(after.complete_terminal(&admission.key, ProvisionTerminal::BindingFailed));
        let status = after.status().expect("cancelled status");
        assert_eq!(after.phase_kind(), ProvisionPhaseKind::ActivationFailed);
        assert_eq!(status.state, ModelProvisionStateDto::Cancelled);
        assert_eq!(
            status.publication,
            ModelProvisionPublicationDto::VerifiedInstalled
        );
        assert_eq!(status.cleanup, ModelProvisionCleanupDto::Confirmed);
    }

    #[test]
    fn activation_and_ready_status_revisions_are_monotonic() {
        let now = Instant::now();
        let mut coordinator = ProvisionCoordinator::awaiting("runtime");
        let admission = admit(&mut coordinator, "ready-op", now);
        let initial = coordinator.status().expect("start status");
        let activating = coordinator
            .mark_activating(&admission.key)
            .expect("mark activation");
        assert!(activating.revision > initial.revision);
        assert_eq!(
            activating.publication,
            ModelProvisionPublicationDto::VerifiedInstalled
        );
        assert!(matches!(
            coordinator.commit_ready(&admission.key, "candidate"),
            CandidateCommit::Committed
        ));
        let ready = coordinator.status().expect("ready status");
        assert!(ready.revision > activating.revision);
        assert_eq!(ready.state, ModelProvisionStateDto::Ready);
        assert_eq!(coordinator.phase_kind(), ProvisionPhaseKind::Ready);
    }

    #[test]
    fn stale_generation_returns_candidate_for_cleanup() {
        let now = Instant::now();
        let mut coordinator = ProvisionCoordinator::awaiting("runtime");
        let admission = admit(&mut coordinator, "same-id", now);
        coordinator
            .mark_activating(&admission.key)
            .expect("mark activation");
        let stale = ProvisionOperationKey {
            operation_id: admission.key.operation_id.clone(),
            generation: admission.key.generation + 1,
        };
        match coordinator.commit_ready(&stale, "candidate") {
            CandidateCommit::Rejected { candidate, reason } => {
                assert_eq!(candidate, "candidate");
                assert_eq!(reason, ProvisionCommitRejectReason::StaleOperation);
            }
            CandidateCommit::Committed => panic!("stale candidate committed"),
        }
    }

    #[test]
    fn close_cancels_active_operation_and_late_commit_returns_candidate() {
        let now = Instant::now();
        let mut coordinator = ProvisionCoordinator::awaiting("runtime");
        let admission = admit(&mut coordinator, "closing-op", now);
        coordinator
            .mark_activating(&admission.key)
            .expect("mark activation");

        assert_eq!(coordinator.close(), None);
        assert!(admission.cancellation.is_cancelled());
        assert_eq!(coordinator.phase_kind(), ProvisionPhaseKind::Closing);
        let status = coordinator.status().expect("closing status");
        assert_eq!(status.failure, Some(ModelProvisionFailureDto::ShuttingDown));

        match coordinator.commit_ready(&admission.key, "late-candidate") {
            CandidateCommit::Rejected { candidate, reason } => {
                assert_eq!(candidate, "late-candidate");
                assert_eq!(reason, ProvisionCommitRejectReason::Closing);
            }
            CandidateCommit::Committed => panic!("late candidate committed while closing"),
        }
    }

    #[test]
    fn close_extracts_ready_candidate_once() {
        let now = Instant::now();
        let mut coordinator = ProvisionCoordinator::awaiting("runtime");
        let admission = admit(&mut coordinator, "ready-close", now);
        coordinator
            .mark_activating(&admission.key)
            .expect("mark activation");
        assert!(matches!(
            coordinator.commit_ready(&admission.key, "candidate"),
            CandidateCommit::Committed
        ));
        assert_eq!(coordinator.close(), Some("candidate"));
        assert_eq!(coordinator.close(), None);
    }

    #[test]
    fn retryable_failure_classifications_return_to_awaiting() {
        let cases = [
            (
                ProvisionTerminal::CancelledBeforePublicationClean { runtime: "runtime" },
                ModelProvisionStateDto::Cancelled,
                None,
                ModelProvisionCleanupDto::Confirmed,
            ),
            (
                ProvisionTerminal::TimedOutBeforePublicationClean { runtime: "runtime" },
                ModelProvisionStateDto::TimedOut,
                None,
                ModelProvisionCleanupDto::Confirmed,
            ),
            (
                ProvisionTerminal::SourceRejectedBeforeStaging { runtime: "runtime" },
                ModelProvisionStateDto::Failed,
                Some(ModelProvisionFailureDto::SourceRejected),
                ModelProvisionCleanupDto::NotRequired,
            ),
            (
                ProvisionTerminal::SourceRejectedAfterStagingClean { runtime: "runtime" },
                ModelProvisionStateDto::Failed,
                Some(ModelProvisionFailureDto::SourceRejected),
                ModelProvisionCleanupDto::Confirmed,
            ),
            (
                ProvisionTerminal::ImportFailedClean { runtime: "runtime" },
                ModelProvisionStateDto::Failed,
                Some(ModelProvisionFailureDto::ImportFailed),
                ModelProvisionCleanupDto::Confirmed,
            ),
            (
                ProvisionTerminal::WorkerFailedBeforePublicationClean { runtime: "runtime" },
                ModelProvisionStateDto::Failed,
                Some(ModelProvisionFailureDto::WorkerFailed),
                ModelProvisionCleanupDto::Confirmed,
            ),
        ];
        for (index, (terminal, expected_state, expected_failure, expected_cleanup)) in
            cases.into_iter().enumerate()
        {
            let mut coordinator = ProvisionCoordinator::<_, &str>::awaiting("runtime");
            let admission = admit(&mut coordinator, &format!("retry-{index}"), Instant::now());
            assert_committed(coordinator.complete_terminal(&admission.key, terminal));
            assert_eq!(coordinator.phase_kind(), ProvisionPhaseKind::Awaiting);
            let status = coordinator.status().expect("terminal status");
            assert_eq!(status.state, expected_state);
            assert_eq!(status.failure, expected_failure);
            assert_eq!(status.cleanup, expected_cleanup);
            assert_eq!(
                status.publication,
                ModelProvisionPublicationDto::NotPublished
            );
        }
    }

    #[test]
    fn fail_closed_classifications_preserve_truthful_publication() {
        let cases = [
            (
                ProvisionTerminal::CleanupFailed,
                ProvisionPhaseKind::ActivationFailed,
                ModelProvisionPublicationDto::NotPublished,
                ModelProvisionFailureDto::CleanupFailed,
            ),
            (
                ProvisionTerminal::ImportFailedClosed,
                ProvisionPhaseKind::ActivationFailed,
                ModelProvisionPublicationDto::NotPublished,
                ModelProvisionFailureDto::ImportFailed,
            ),
            (
                ProvisionTerminal::ImportFailedClosedAfterStagingClean,
                ProvisionPhaseKind::ActivationFailed,
                ModelProvisionPublicationDto::NotPublished,
                ModelProvisionFailureDto::ImportFailed,
            ),
            (
                ProvisionTerminal::PublishedUnconfirmed,
                ProvisionPhaseKind::PublishedUnconfirmed,
                ModelProvisionPublicationDto::PublishedUnconfirmed,
                ModelProvisionFailureDto::PublishedUnconfirmed,
            ),
            (
                ProvisionTerminal::BindingFailed,
                ProvisionPhaseKind::ActivationFailed,
                ModelProvisionPublicationDto::VerifiedInstalled,
                ModelProvisionFailureDto::BindingFailed,
            ),
            (
                ProvisionTerminal::ActivationFailed,
                ProvisionPhaseKind::ActivationFailed,
                ModelProvisionPublicationDto::VerifiedInstalled,
                ModelProvisionFailureDto::ActivationFailed,
            ),
            (
                ProvisionTerminal::WorkerFailedUncertain,
                ProvisionPhaseKind::ActivationFailed,
                ModelProvisionPublicationDto::NotPublished,
                ModelProvisionFailureDto::WorkerFailed,
            ),
        ];
        for (index, (terminal, expected_phase, expected_publication, expected_failure)) in
            cases.into_iter().enumerate()
        {
            let mut coordinator = ProvisionCoordinator::<_, &str>::awaiting("runtime");
            let admission = admit(&mut coordinator, &format!("failed-{index}"), Instant::now());
            if matches!(
                terminal,
                ProvisionTerminal::BindingFailed | ProvisionTerminal::ActivationFailed
            ) {
                coordinator
                    .mark_activating(&admission.key)
                    .expect("mark activation");
            }
            assert_committed(coordinator.complete_terminal(&admission.key, terminal));
            assert_eq!(coordinator.phase_kind(), expected_phase);
            let status = coordinator.status().expect("terminal status");
            assert_eq!(status.publication, expected_publication);
            assert_eq!(status.failure, Some(expected_failure));
        }
    }

    #[test]
    fn stale_terminal_returns_runtime_to_caller() {
        let now = Instant::now();
        let mut coordinator = ProvisionCoordinator::<_, &str>::awaiting("runtime");
        let admission = admit(&mut coordinator, "terminal", now);
        let stale = ProvisionOperationKey {
            operation_id: admission.key.operation_id.clone(),
            generation: admission.key.generation + 1,
        };
        match coordinator.complete_terminal(
            &stale,
            ProvisionTerminal::ImportFailedClean {
                runtime: admission.runtime,
            },
        ) {
            TerminalCommit::Rejected { terminal, reason } => {
                assert_eq!(reason, ProvisionCommitRejectReason::StaleOperation);
                assert!(matches!(
                    terminal,
                    ProvisionTerminal::ImportFailedClean { runtime: "runtime" }
                ));
            }
            TerminalCommit::Committed => panic!("stale terminal result committed"),
        }
    }

    #[test]
    fn failure_classification_cannot_overstate_publication_stage() {
        let now = Instant::now();
        let mut coordinator = ProvisionCoordinator::<_, &str>::awaiting("runtime");
        let admission = admit(&mut coordinator, "stage-bound", now);
        match coordinator.complete_terminal(&admission.key, ProvisionTerminal::BindingFailed) {
            TerminalCommit::Rejected { terminal, reason } => {
                assert_eq!(reason, ProvisionCommitRejectReason::InvalidPhase);
                assert!(matches!(terminal, ProvisionTerminal::BindingFailed));
            }
            TerminalCommit::Committed => panic!("pre-publication binding failure was accepted"),
        }
        assert_eq!(
            coordinator.status().expect("unchanged status").publication,
            ModelProvisionPublicationDto::NotPublished
        );
    }
}
