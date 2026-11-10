//! Portal/EIS eligibility and owned-session lifecycle boundaries.
//!
//! The public system probe remains read-only. A separate, non-composed direct
//! portal driver can request an explicit grant, own an EIS transport, and use a
//! narrowly reviewed text sender, but none of that promotes probe eligibility
//! to delivery readiness or release qualification.

use std::fmt;
use std::future::Future;
use std::time::{Duration, Instant};

use ashpd::desktop::remote_desktop::{DeviceType, RemoteDesktop};
use narrivy_types::SessionCancellation;

/// Minimum portal version that exposes `ConnectToEIS`.
pub const REMOTE_DESKTOP_EIS_VERSION: u32 = 2;
/// The read-only probe must remain outside the dictation hot path and bounded.
pub const DEFAULT_PORTAL_PROBE_TIMEOUT: Duration = Duration::from_millis(750);

/// Content-free facts returned by a RemoteDesktop portal probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteDesktopFacts {
    InterfaceAbsent,
    Advertised {
        version: u32,
        keyboard_available: bool,
    },
}

/// Probe errors are stable codes only; D-Bus error text and environment data
/// are intentionally excluded from diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortalProbeFailure {
    RuntimeUnavailable,
    TimedOut,
    QueryFailed,
}

/// Minimal fakeable boundary used by startup diagnostics and controlled tests.
///
/// There is intentionally no permission or submission method on this trait.
pub trait RemoteDesktopProbe {
    fn inspect(&self) -> Result<RemoteDesktopFacts, PortalProbeFailure>;
}

/// Truthful eligibility state for the not-yet-qualified portal/EIS candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortalEisEligibility {
    InterfaceAbsent,
    VersionTooOld {
        version: u32,
    },
    KeyboardUnavailable {
        version: u32,
    },
    /// The UI may offer an explicit permission action. This does not mean an
    /// EIS session is granted, connected, compatible, or qualified.
    ReadyToRequestPermission {
        version: u32,
    },
    ProbeFailed(PortalProbeFailure),
}

impl PortalEisEligibility {
    /// Eligibility must never be presented as direct-delivery readiness.
    #[must_use]
    pub const fn is_delivery_ready(self) -> bool {
        false
    }

    #[must_use]
    pub const fn may_request_permission(self) -> bool {
        matches!(self, Self::ReadyToRequestPermission { .. })
    }
}

/// Classify a fake or real read-only probe without consulting desktop names.
#[must_use]
pub fn classify_portal_eis(probe: &dyn RemoteDesktopProbe) -> PortalEisEligibility {
    match probe.inspect() {
        Ok(RemoteDesktopFacts::InterfaceAbsent) => PortalEisEligibility::InterfaceAbsent,
        Ok(RemoteDesktopFacts::Advertised {
            version,
            keyboard_available: false,
        }) => PortalEisEligibility::KeyboardUnavailable { version },
        Ok(RemoteDesktopFacts::Advertised {
            version,
            keyboard_available: true,
        }) if version < REMOTE_DESKTOP_EIS_VERSION => {
            PortalEisEligibility::VersionTooOld { version }
        }
        Ok(RemoteDesktopFacts::Advertised {
            version,
            keyboard_available: true,
        }) => PortalEisEligibility::ReadyToRequestPermission { version },
        Err(err) => PortalEisEligibility::ProbeFailed(err),
    }
}

/// Opaque, validated parent-window reference for an explicit portal prompt.
///
/// The raw desktop handle is intentionally not printable through `Debug` and
/// never belongs in ordinary logs. A UI integration must obtain it from the
/// window that initiated the authenticated permission request.
#[derive(Clone, PartialEq, Eq)]
pub struct PortalParentWindow(Box<str>);

impl PortalParentWindow {
    /// Parse an XDG parent-window token without retaining parser diagnostics.
    pub fn parse(value: &str) -> Result<Self, PortalSessionFailure> {
        if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
            return Err(PortalSessionFailure::InvalidParentWindow);
        }
        let Some((kind, handle)) = value.split_once(':') else {
            return Err(PortalSessionFailure::InvalidParentWindow);
        };
        match kind {
            "x11" => {
                let handle = handle.trim_start_matches("0x");
                let xid = u64::from_str_radix(handle, 16)
                    .map_err(|_| PortalSessionFailure::InvalidParentWindow)?;
                if xid == 0 {
                    return Err(PortalSessionFailure::InvalidParentWindow);
                }
            }
            "wayland" if !handle.is_empty() => {}
            _ => return Err(PortalSessionFailure::InvalidParentWindow),
        }
        Ok(Self(value.into()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for PortalParentWindow {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PortalParentWindow(<redacted>)")
    }
}

/// Grant/transport lifecycle state. `Active` means only that a transport is
/// owned; it is not a delivery-readiness or target-compatibility claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortalSessionState {
    Idle,
    Requesting,
    Granted,
    Connecting,
    Active,
    Closing,
    /// Cleanup could not be confirmed. No submission is allowed in this state.
    Invalid,
}

/// Stable, content-free failures for the permission/session boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortalSessionFailure {
    InvalidParentWindow,
    AlreadyActive,
    Invalidated,
    NotActive,
    Cancelled,
    TimedOut,
    PermissionDenied,
    PortalUnavailable,
    RequestFailed,
    TransportFailed,
    Revoked,
    CloseFailed,
}

/// Whether an admitted permission/session operation was confirmed cleaned up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortalCleanupStatus {
    NotRequired,
    ConfirmedClosed,
    Failed,
}

/// A lifecycle failure plus the independent truth about cleanup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortalSessionError {
    pub failure: PortalSessionFailure,
    pub cleanup: PortalCleanupStatus,
}

/// Result of an explicit close or revocation operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortalCloseResult {
    AlreadyIdle,
    Closed,
}

/// Content-free events surfaced by the concrete portal transport owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortalDriverEvent {
    /// The portal emitted `org.freedesktop.portal.Session.Closed`.
    Revoked,
}

/// Validated single-frame text owned by the portal actor.
///
/// Recovery Phase 2C deliberately admits only the smallest protocol subset:
/// nonempty printable single-line ASCII that fits one `ei_text.utf8` request.
/// Unicode, multiline, chunking, and retry need separate qualification.
#[derive(Clone, PartialEq, Eq)]
pub struct PortalEisTextPayload(Box<str>);

impl PortalEisTextPayload {
    pub const MAX_BYTES: usize = 254;

    pub fn parse(value: impl Into<Box<str>>) -> Result<Self, PortalEisTextFailure> {
        let value = value.into();
        if value.is_empty() {
            return Err(PortalEisTextFailure::PayloadEmpty);
        }
        if value.len() > Self::MAX_BYTES {
            return Err(PortalEisTextFailure::PayloadTooLong);
        }
        if !value.bytes().all(|byte| (0x20..=0x7e).contains(&byte)) {
            return Err(PortalEisTextFailure::PayloadUnsupported);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for PortalEisTextPayload {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PortalEisTextPayload(<redacted>)")
    }
}

/// Stable content-free reason for rejecting or losing certainty about one
/// controlled EIS text submission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortalEisTextFailure {
    PayloadEmpty,
    PayloadTooLong,
    PayloadUnsupported,
    SenderUnavailable,
    NotActive,
    Cancelled,
    TimedOut,
    Revoked,
    TransportFailed,
    CloseFailed,
}

impl PortalEisTextFailure {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::PayloadEmpty => "PORTAL_EIS_PAYLOAD_EMPTY",
            Self::PayloadTooLong => "PORTAL_EIS_PAYLOAD_TOO_LONG",
            Self::PayloadUnsupported => "PORTAL_EIS_PAYLOAD_UNSUPPORTED",
            Self::SenderUnavailable => "PORTAL_EIS_SENDER_UNAVAILABLE",
            Self::NotActive => "PORTAL_EIS_NOT_ACTIVE",
            Self::Cancelled => "PORTAL_EIS_CANCELLED",
            Self::TimedOut => "PORTAL_EIS_TIMED_OUT",
            Self::Revoked => "PORTAL_EIS_REVOKED",
            Self::TransportFailed => "PORTAL_EIS_TRANSPORT_FAILED",
            Self::CloseFailed => "PORTAL_EIS_CLOSE_FAILED",
        }
    }
}

/// Progress-aware fact returned by the actor-owned sender boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortalEisTextSubmission {
    /// No payload-bearing transport byte was accepted and cleanup was
    /// confirmed. Cleanup failure is instead a rejected `CloseFailed` result.
    CancelledBeforeDispatch,
    /// Rejected before the point of no return.
    RejectedBeforeDispatch(PortalEisTextFailure),
    /// One complete UTF-8 request plus its device frame was handed to the OS
    /// transport; target processing or retention remains unknown.
    SubmittedUnverified,
    /// Payload-bearing transport progress may have occurred and cannot be
    /// ruled out, so absence of an external side effect cannot be claimed.
    DispatchUncertain(PortalEisTextFailure),
}

/// Cancellation view shared by the lifecycle and its concrete driver.
///
/// The daemon actor combines the user operation signal with its own worker
/// shutdown signal. This prevents daemon shutdown from waiting for a portal
/// operation that only observes a per-request cancellation flag.
pub trait PortalCancellation: Send + Sync {
    fn is_cancelled(&self) -> bool;
}

impl PortalCancellation for SessionCancellation {
    fn is_cancelled(&self) -> bool {
        SessionCancellation::is_cancelled(self)
    }
}

/// Logical OR of request cancellation and daemon-owner shutdown.
#[derive(Debug, Clone)]
pub struct CombinedPortalCancellation {
    request: SessionCancellation,
    owner: SessionCancellation,
}

impl CombinedPortalCancellation {
    #[must_use]
    pub const fn new(request: SessionCancellation, owner: SessionCancellation) -> Self {
        Self { request, owner }
    }
}

impl PortalCancellation for CombinedPortalCancellation {
    fn is_cancelled(&self) -> bool {
        self.request.is_cancelled() || self.owner.is_cancelled()
    }
}

/// Fakeable resource owner used by the daemon-owned lifecycle boundary.
///
/// A concrete driver owns the ashpd session and, once approved, the EIS file
/// descriptor/transport. Every method receives the same absolute deadline;
/// implementations must not create a new timeout budget per step. `close` is
/// idempotent and must drop all session/transport resources before success.
pub trait PortalSessionDriver: Send {
    fn request_keyboard_grant(
        &mut self,
        parent: &PortalParentWindow,
        deadline: Instant,
        cancellation: &dyn PortalCancellation,
    ) -> Result<(), PortalSessionFailure>;

    fn connect_transport(
        &mut self,
        deadline: Instant,
        cancellation: &dyn PortalCancellation,
    ) -> Result<(), PortalSessionFailure>;

    fn close(&mut self, deadline: Instant) -> Result<(), PortalSessionFailure>;

    /// Poll one driver-originated lifecycle event without blocking. A concrete
    /// driver must retain and poll the portal `Session.Closed` subscription;
    /// it may not privately consume revocation while leaving the lifecycle
    /// active. Long-running request methods must also observe the same signal
    /// while they own the actor thread.
    fn poll_event(&mut self) -> Result<Option<PortalDriverEvent>, PortalSessionFailure>;

    /// Submit one already-validated text frame on the actor-owner thread.
    ///
    /// The default is deliberately unavailable. A concrete implementation
    /// must define dispatch uncertainty at its first irreversible transport
    /// handoff, report submission only after the full UTF-8 request and device
    /// frame are handed off, and use the caller's absolute deadline for all
    /// cleanup. The reviewed direct-zbus driver supplies such an implementation,
    /// but default daemon startup does not compose it.
    fn submit_text(
        &mut self,
        _payload: PortalEisTextPayload,
        _deadline: Instant,
        _cancellation: &dyn PortalCancellation,
    ) -> PortalEisTextSubmission {
        PortalEisTextSubmission::RejectedBeforeDispatch(PortalEisTextFailure::SenderUnavailable)
    }

    /// Infallible, non-blocking last-resort teardown used only when the owner
    /// is dropped without a confirmed close. It must immediately drop local
    /// transport/file-descriptor authority; it cannot claim the remote portal
    /// session was cleanly closed.
    fn abort_local(&mut self);
}

/// Single-owner portal grant and sender lifecycle.
///
/// This object is deliberately not internally synchronized: the daemon must
/// place it behind one owned worker/actor rather than issue concurrent portal
/// calls. The trait default remains fail-closed; concrete emission requires a
/// separately reviewed driver and an explicitly authorized composition.
pub struct PortalSessionLifecycle<D: PortalSessionDriver> {
    driver: D,
    state: PortalSessionState,
    local_authority_aborted: bool,
}

impl<D: PortalSessionDriver> PortalSessionLifecycle<D> {
    #[must_use]
    pub const fn new(driver: D) -> Self {
        Self {
            driver,
            state: PortalSessionState::Idle,
            local_authority_aborted: false,
        }
    }

    #[must_use]
    pub const fn state(&self) -> PortalSessionState {
        self.state
    }

    /// Request a keyboard-only grant and connect its transport as one owned
    /// admission. Any failure after admission attempts exactly one close.
    pub fn activate(
        &mut self,
        parent: &PortalParentWindow,
        deadline: Instant,
        cancellation: &dyn PortalCancellation,
    ) -> Result<(), PortalSessionError> {
        if self.state != PortalSessionState::Idle {
            return Err(PortalSessionError {
                failure: if self.state == PortalSessionState::Invalid {
                    PortalSessionFailure::Invalidated
                } else {
                    PortalSessionFailure::AlreadyActive
                },
                cleanup: PortalCleanupStatus::NotRequired,
            });
        }
        if cancellation.is_cancelled() {
            return Err(PortalSessionError {
                failure: PortalSessionFailure::Cancelled,
                cleanup: PortalCleanupStatus::NotRequired,
            });
        }
        if Instant::now() >= deadline {
            return Err(PortalSessionError {
                failure: PortalSessionFailure::TimedOut,
                cleanup: PortalCleanupStatus::NotRequired,
            });
        }

        // Idle may follow a confirmed portal revocation whose local resources
        // were already aborted. A newly admitted session owns fresh authority.
        self.local_authority_aborted = false;
        self.state = PortalSessionState::Requesting;
        if let Err(failure) = self
            .driver
            .request_keyboard_grant(parent, deadline, cancellation)
        {
            if failure == PortalSessionFailure::Revoked {
                return Err(self.observe_revoked());
            }
            return Err(self.fail_and_close(failure, deadline));
        }
        self.state = PortalSessionState::Granted;
        if cancellation.is_cancelled() {
            return Err(self.fail_and_close(PortalSessionFailure::Cancelled, deadline));
        }
        if Instant::now() >= deadline {
            return Err(self.fail_and_close(PortalSessionFailure::TimedOut, deadline));
        }

        self.state = PortalSessionState::Connecting;
        if let Err(failure) = self.driver.connect_transport(deadline, cancellation) {
            if failure == PortalSessionFailure::Revoked {
                return Err(self.observe_revoked());
            }
            return Err(self.fail_and_close(failure, deadline));
        }
        if cancellation.is_cancelled() {
            return Err(self.fail_and_close(PortalSessionFailure::Cancelled, deadline));
        }
        if Instant::now() >= deadline {
            return Err(self.fail_and_close(PortalSessionFailure::TimedOut, deadline));
        }
        match self.driver.poll_event() {
            Ok(Some(PortalDriverEvent::Revoked)) => {
                return Err(self.observe_revoked());
            }
            Ok(None) => {}
            Err(failure) => return Err(self.fail_and_close(failure, deadline)),
        }
        self.state = PortalSessionState::Active;
        Ok(())
    }

    /// Poll and process one concrete-driver event on the actor-owner thread.
    /// The driver call is required to be nonblocking.
    pub fn poll_driver_event(
        &mut self,
        deadline: Instant,
    ) -> Result<Option<PortalSessionError>, PortalSessionError> {
        match self.driver.poll_event() {
            Ok(None) => Ok(None),
            Ok(Some(PortalDriverEvent::Revoked)) => Ok(Some(self.observe_revoked())),
            Err(failure) if self.state == PortalSessionState::Idle => Err(PortalSessionError {
                failure,
                cleanup: PortalCleanupStatus::NotRequired,
            }),
            Err(_) if self.state == PortalSessionState::Invalid => Err(PortalSessionError {
                failure: PortalSessionFailure::Invalidated,
                cleanup: PortalCleanupStatus::Failed,
            }),
            Err(failure) => Err(self.fail_and_close(failure, deadline)),
        }
    }

    /// Pre-dispatch gate used by a future sender in the same single-owner
    /// actor. Transport ownership alone is not proof of delivery. The sender
    /// must still linearize cancellation against its first accepted event and
    /// report post-dispatch uncertainty; this check cannot replace that
    /// progress-aware boundary.
    pub fn submission_allowed(
        &mut self,
        cancellation: &dyn PortalCancellation,
        deadline: Instant,
    ) -> Result<(), PortalSessionError> {
        if self.state == PortalSessionState::Active {
            match self.poll_driver_event(deadline) {
                Ok(None) => {}
                Ok(Some(event)) | Err(event) => return Err(event),
            }
        }
        match self.state {
            PortalSessionState::Active if cancellation.is_cancelled() => {
                Err(self.fail_and_close(PortalSessionFailure::Cancelled, deadline))
            }
            PortalSessionState::Active if Instant::now() >= deadline => {
                Err(self.fail_and_close(PortalSessionFailure::TimedOut, deadline))
            }
            PortalSessionState::Active => Ok(()),
            PortalSessionState::Invalid => Err(PortalSessionError {
                failure: PortalSessionFailure::Invalidated,
                cleanup: PortalCleanupStatus::NotRequired,
            }),
            _ => Err(PortalSessionError {
                failure: PortalSessionFailure::NotActive,
                cleanup: PortalCleanupStatus::NotRequired,
            }),
        }
    }

    /// Submit one validated frame without separating lifecycle admission from
    /// the driver's first-write linearization point.
    pub fn submit_text(
        &mut self,
        payload: PortalEisTextPayload,
        cancellation: &dyn PortalCancellation,
        deadline: Instant,
    ) -> PortalEisTextSubmission {
        if let Err(error) = self.submission_allowed(cancellation, deadline) {
            if error.cleanup == PortalCleanupStatus::Failed {
                return PortalEisTextSubmission::RejectedBeforeDispatch(
                    PortalEisTextFailure::CloseFailed,
                );
            }
            return match error.failure {
                PortalSessionFailure::Cancelled => PortalEisTextSubmission::CancelledBeforeDispatch,
                failure => PortalEisTextSubmission::RejectedBeforeDispatch(
                    text_failure_from_session(failure),
                ),
            };
        }

        let outcome = self.driver.submit_text(payload, deadline, cancellation);
        match outcome {
            PortalEisTextSubmission::DispatchUncertain(PortalEisTextFailure::Revoked) => {
                let _ = self.observe_revoked();
            }
            PortalEisTextSubmission::DispatchUncertain(reason) => {
                if self.close(deadline).is_err() {
                    return PortalEisTextSubmission::DispatchUncertain(
                        PortalEisTextFailure::CloseFailed,
                    );
                }
                return PortalEisTextSubmission::DispatchUncertain(reason);
            }
            PortalEisTextSubmission::RejectedBeforeDispatch(PortalEisTextFailure::Revoked) => {
                let _ = self.observe_revoked();
            }
            PortalEisTextSubmission::RejectedBeforeDispatch(
                PortalEisTextFailure::TransportFailed,
            ) => {
                let error = self.fail_and_close(PortalSessionFailure::TransportFailed, deadline);
                if error.cleanup == PortalCleanupStatus::Failed {
                    return PortalEisTextSubmission::RejectedBeforeDispatch(
                        PortalEisTextFailure::CloseFailed,
                    );
                }
            }
            PortalEisTextSubmission::RejectedBeforeDispatch(PortalEisTextFailure::CloseFailed) => {
                self.invalidate_local_authority();
            }
            PortalEisTextSubmission::CancelledBeforeDispatch
            | PortalEisTextSubmission::RejectedBeforeDispatch(_)
            | PortalEisTextSubmission::SubmittedUnverified => {}
        }
        outcome
    }

    /// Explicit user revoke and daemon shutdown use the same bounded close.
    pub fn close(&mut self, deadline: Instant) -> Result<PortalCloseResult, PortalSessionError> {
        if self.state == PortalSessionState::Idle {
            return Ok(PortalCloseResult::AlreadyIdle);
        }
        if self.state == PortalSessionState::Invalid {
            return Err(PortalSessionError {
                failure: PortalSessionFailure::Invalidated,
                cleanup: PortalCleanupStatus::Failed,
            });
        }
        self.state = PortalSessionState::Closing;
        match self.driver.close(deadline) {
            Ok(()) => {
                self.state = PortalSessionState::Idle;
                Ok(PortalCloseResult::Closed)
            }
            Err(_) => {
                self.invalidate_local_authority();
                Err(PortalSessionError {
                    failure: PortalSessionFailure::CloseFailed,
                    cleanup: PortalCleanupStatus::Failed,
                })
            }
        }
    }

    /// A portal revocation invalidates submission immediately, then drops the
    /// local authority represented by the already-closed remote session.
    pub fn observe_revoked(&mut self) -> PortalSessionError {
        if self.state == PortalSessionState::Idle {
            return PortalSessionError {
                failure: PortalSessionFailure::Revoked,
                cleanup: PortalCleanupStatus::NotRequired,
            };
        }
        if self.state == PortalSessionState::Invalid {
            return PortalSessionError {
                failure: PortalSessionFailure::Revoked,
                cleanup: PortalCleanupStatus::Failed,
            };
        }
        self.state = PortalSessionState::Closing;
        // `Revoked` is emitted only after the driver observed the portal's
        // authoritative `Session.Closed` signal. No second remote Close call is
        // needed or truthful; immediately drop local request/session/FD state.
        let cleanup = if self.abort_local_authority() {
            self.state = PortalSessionState::Idle;
            PortalCleanupStatus::ConfirmedClosed
        } else {
            self.state = PortalSessionState::Invalid;
            PortalCleanupStatus::Failed
        };
        PortalSessionError {
            failure: PortalSessionFailure::Revoked,
            cleanup,
        }
    }

    fn fail_and_close(
        &mut self,
        failure: PortalSessionFailure,
        deadline: Instant,
    ) -> PortalSessionError {
        self.state = PortalSessionState::Closing;
        let cleanup = match self.driver.close(deadline) {
            Ok(()) => {
                self.state = PortalSessionState::Idle;
                PortalCleanupStatus::ConfirmedClosed
            }
            Err(_) => {
                self.invalidate_local_authority();
                PortalCleanupStatus::Failed
            }
        };
        PortalSessionError { failure, cleanup }
    }

    fn invalidate_local_authority(&mut self) {
        self.state = PortalSessionState::Invalid;
        let _ = self.abort_local_authority();
    }

    fn abort_local_authority(&mut self) -> bool {
        if self.local_authority_aborted {
            return true;
        }
        // Remote close failure must not leave a live local EIS descriptor or
        // request/session handle inside a still-running actor. The driver abort
        // is nonblocking and infallible by contract; contain a faulty external
        // implementation without manufacturing remote-close confirmation.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.driver.abort_local();
        }));
        if result.is_ok() {
            self.local_authority_aborted = true;
            true
        } else {
            false
        }
    }
}

fn text_failure_from_session(failure: PortalSessionFailure) -> PortalEisTextFailure {
    match failure {
        PortalSessionFailure::NotActive | PortalSessionFailure::AlreadyActive => {
            PortalEisTextFailure::NotActive
        }
        PortalSessionFailure::Cancelled => PortalEisTextFailure::Cancelled,
        PortalSessionFailure::TimedOut => PortalEisTextFailure::TimedOut,
        PortalSessionFailure::Revoked => PortalEisTextFailure::Revoked,
        PortalSessionFailure::CloseFailed => PortalEisTextFailure::CloseFailed,
        PortalSessionFailure::InvalidParentWindow
        | PortalSessionFailure::Invalidated
        | PortalSessionFailure::PermissionDenied
        | PortalSessionFailure::PortalUnavailable
        | PortalSessionFailure::RequestFailed
        | PortalSessionFailure::TransportFailed => PortalEisTextFailure::TransportFailed,
    }
}

impl<D: PortalSessionDriver> Drop for PortalSessionLifecycle<D> {
    fn drop(&mut self) {
        if self.state == PortalSessionState::Idle || self.local_authority_aborted {
            return;
        }
        self.invalidate_local_authority();
    }
}

/// Real read-only portal probe. It reads two advertised properties and invokes
/// none of CreateSession, SelectDevices, Start, ConnectToEIS, or Notify*.
#[derive(Debug, Clone, Copy)]
pub struct SystemRemoteDesktopProbe {
    timeout: Duration,
}

impl Default for SystemRemoteDesktopProbe {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_PORTAL_PROBE_TIMEOUT,
        }
    }
}

impl SystemRemoteDesktopProbe {
    #[must_use]
    pub const fn with_timeout(timeout: Duration) -> Self {
        Self { timeout }
    }
}

impl RemoteDesktopProbe for SystemRemoteDesktopProbe {
    fn inspect(&self) -> Result<RemoteDesktopFacts, PortalProbeFailure> {
        // `Runtime::block_on` panics when entered from another Tokio runtime.
        // Capability diagnostics are synchronous today, so fail closed instead
        // of risking a daemon/client crash if a future caller invokes them from
        // async context. The eventual session owner will expose an async API.
        if tokio::runtime::Handle::try_current().is_ok() {
            return Err(PortalProbeFailure::RuntimeUnavailable);
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .map_err(|_| PortalProbeFailure::RuntimeUnavailable)?;

        runtime.block_on(async { run_bounded_query(self.timeout, query_remote_desktop()).await })
    }
}

async fn run_bounded_query<F>(
    timeout: Duration,
    query: F,
) -> Result<RemoteDesktopFacts, PortalProbeFailure>
where
    F: Future<Output = Result<RemoteDesktopFacts, PortalProbeFailure>>,
{
    match tokio::time::timeout(timeout, query).await {
        Ok(result) => result,
        Err(_) => Err(PortalProbeFailure::TimedOut),
    }
}

async fn query_remote_desktop() -> Result<RemoteDesktopFacts, PortalProbeFailure> {
    let portal = match RemoteDesktop::new().await {
        Ok(portal) => portal,
        Err(err) => return classify_remote_desktop_open_error(err),
    };
    let version = portal
        .get_property::<u32>("version")
        .await
        .map_err(|_| PortalProbeFailure::QueryFailed)?;
    let devices = portal
        .available_device_types()
        .await
        .map_err(|_| PortalProbeFailure::QueryFailed)?;
    Ok(RemoteDesktopFacts::Advertised {
        version,
        keyboard_available: devices.contains(DeviceType::Keyboard),
    })
}

/// Preserve the one absence condition ashpd exposes without retaining or
/// formatting D-Bus error details. Bus, transport, and property failures stay
/// indistinguishable content-free query failures.
fn classify_remote_desktop_open_error(
    error: ashpd::Error,
) -> Result<RemoteDesktopFacts, PortalProbeFailure> {
    match error {
        ashpd::Error::PortalNotFound(_) => Ok(RemoteDesktopFacts::InterfaceAbsent),
        _ => Err(PortalProbeFailure::QueryFailed),
    }
}

#[cfg(test)]
mod tests {
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::Barrier;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::task::{Context, Poll};

    use super::*;

    #[derive(Clone, Copy)]
    struct FixedProbe(Result<RemoteDesktopFacts, PortalProbeFailure>);

    impl RemoteDesktopProbe for FixedProbe {
        fn inspect(&self) -> Result<RemoteDesktopFacts, PortalProbeFailure> {
            self.0
        }
    }

    struct FakeSessionDriver {
        calls: Vec<&'static str>,
        request_result: Result<(), PortalSessionFailure>,
        connect_result: Result<(), PortalSessionFailure>,
        close_result: Result<(), PortalSessionFailure>,
        cancellation_after_request: Option<SessionCancellation>,
        cancellation_after_connect: Option<SessionCancellation>,
        next_event: Option<Result<PortalDriverEvent, PortalSessionFailure>>,
        submit_result: PortalEisTextSubmission,
        abort_observed: Option<Arc<AtomicBool>>,
        abort_count: Option<Arc<AtomicUsize>>,
        resource_live: bool,
    }

    impl Default for FakeSessionDriver {
        fn default() -> Self {
            Self {
                calls: Vec::new(),
                request_result: Ok(()),
                connect_result: Ok(()),
                close_result: Ok(()),
                cancellation_after_request: None,
                cancellation_after_connect: None,
                next_event: None,
                submit_result: PortalEisTextSubmission::RejectedBeforeDispatch(
                    PortalEisTextFailure::SenderUnavailable,
                ),
                abort_observed: None,
                abort_count: None,
                resource_live: false,
            }
        }
    }

    impl PortalSessionDriver for FakeSessionDriver {
        fn request_keyboard_grant(
            &mut self,
            _parent: &PortalParentWindow,
            _deadline: Instant,
            _cancellation: &dyn PortalCancellation,
        ) -> Result<(), PortalSessionFailure> {
            self.calls.push("request");
            if self.request_result.is_ok() {
                self.resource_live = true;
            }
            if let Some(cancellation) = self.cancellation_after_request.as_ref() {
                cancellation.cancel();
            }
            self.request_result
        }

        fn connect_transport(
            &mut self,
            _deadline: Instant,
            _cancellation: &dyn PortalCancellation,
        ) -> Result<(), PortalSessionFailure> {
            self.calls.push("connect");
            if let Some(cancellation) = self.cancellation_after_connect.as_ref() {
                cancellation.cancel();
            }
            self.connect_result
        }

        fn close(&mut self, _deadline: Instant) -> Result<(), PortalSessionFailure> {
            self.calls.push("close");
            if self.close_result.is_ok() {
                self.resource_live = false;
            }
            self.close_result
        }

        fn poll_event(&mut self) -> Result<Option<PortalDriverEvent>, PortalSessionFailure> {
            self.next_event.take().transpose()
        }

        fn submit_text(
            &mut self,
            _payload: PortalEisTextPayload,
            _deadline: Instant,
            _cancellation: &dyn PortalCancellation,
        ) -> PortalEisTextSubmission {
            self.calls.push("submit");
            self.submit_result
        }

        fn abort_local(&mut self) {
            self.calls.push("abort");
            self.resource_live = false;
            if let Some(observed) = self.abort_observed.as_ref() {
                observed.store(true, Ordering::SeqCst);
            }
            if let Some(count) = self.abort_count.as_ref() {
                count.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    struct FirstWriteBoundaryDriver {
        accept_before_pause: bool,
        reached: Arc<Barrier>,
        resume: Arc<Barrier>,
        accepted: Arc<AtomicBool>,
    }

    impl PortalSessionDriver for FirstWriteBoundaryDriver {
        fn request_keyboard_grant(
            &mut self,
            _parent: &PortalParentWindow,
            _deadline: Instant,
            _cancellation: &dyn PortalCancellation,
        ) -> Result<(), PortalSessionFailure> {
            Ok(())
        }

        fn connect_transport(
            &mut self,
            _deadline: Instant,
            _cancellation: &dyn PortalCancellation,
        ) -> Result<(), PortalSessionFailure> {
            Ok(())
        }

        fn close(&mut self, _deadline: Instant) -> Result<(), PortalSessionFailure> {
            Ok(())
        }

        fn poll_event(&mut self) -> Result<Option<PortalDriverEvent>, PortalSessionFailure> {
            Ok(None)
        }

        fn submit_text(
            &mut self,
            _payload: PortalEisTextPayload,
            _deadline: Instant,
            cancellation: &dyn PortalCancellation,
        ) -> PortalEisTextSubmission {
            if self.accept_before_pause {
                // Controlled equivalent of the first positive OS write. Tests
                // synchronize immediately on either side of this boundary.
                self.accepted.store(true, Ordering::SeqCst);
            }
            self.reached.wait();
            self.resume.wait();
            if cancellation.is_cancelled() {
                if self.accepted.load(Ordering::SeqCst) {
                    PortalEisTextSubmission::DispatchUncertain(PortalEisTextFailure::Cancelled)
                } else {
                    PortalEisTextSubmission::CancelledBeforeDispatch
                }
            } else {
                PortalEisTextSubmission::SubmittedUnverified
            }
        }

        fn abort_local(&mut self) {}
    }

    fn parent_window() -> PortalParentWindow {
        PortalParentWindow("wayland:controlled-parent".into())
    }

    fn text_payload(value: &str) -> PortalEisTextPayload {
        PortalEisTextPayload(value.into())
    }

    fn future_deadline() -> Instant {
        Instant::now() + Duration::from_secs(1)
    }

    #[test]
    fn combined_cancellation_observes_request_or_owner_shutdown() {
        let request = SessionCancellation::new();
        let owner = SessionCancellation::new();
        let combined = CombinedPortalCancellation::new(request.clone(), owner.clone());
        assert!(!combined.is_cancelled());
        request.cancel();
        assert!(combined.is_cancelled());

        let request = SessionCancellation::new();
        let owner = SessionCancellation::new();
        let combined = CombinedPortalCancellation::new(request, owner.clone());
        owner.cancel();
        assert!(combined.is_cancelled());
    }

    #[test]
    fn parent_window_validation_is_fail_closed_and_debug_is_redacted() {
        assert_eq!(
            PortalParentWindow::parse(""),
            Err(PortalSessionFailure::InvalidParentWindow)
        );
        assert_eq!(
            PortalParentWindow::parse("wayland:"),
            Err(PortalSessionFailure::InvalidParentWindow)
        );
        assert_eq!(
            PortalParentWindow::parse("x11:0"),
            Err(PortalSessionFailure::InvalidParentWindow)
        );
        assert_eq!(
            PortalParentWindow::parse("unknown:secret"),
            Err(PortalSessionFailure::InvalidParentWindow)
        );
        let parent = parent_window();
        assert_eq!(
            PortalParentWindow::parse("wayland:controlled-parent"),
            Ok(parent.clone())
        );
        assert_eq!(format!("{parent:?}"), "PortalParentWindow(<redacted>)");
    }

    #[test]
    fn text_payload_is_bounded_single_frame_ascii_and_debug_redacted() {
        for (value, failure) in [
            ("", PortalEisTextFailure::PayloadEmpty),
            ("line\nbreak", PortalEisTextFailure::PayloadUnsupported),
            ("non-ascii-é", PortalEisTextFailure::PayloadUnsupported),
        ] {
            assert_eq!(PortalEisTextPayload::parse(value), Err(failure));
        }
        assert_eq!(
            PortalEisTextPayload::parse("x".repeat(255)),
            Err(PortalEisTextFailure::PayloadTooLong)
        );
        let payload = text_payload("redaction-canary");
        assert_eq!(
            PortalEisTextPayload::parse("redaction-canary"),
            Ok(payload.clone())
        );
        assert_eq!(payload.as_bytes(), b"redaction-canary");
        assert_eq!(format!("{payload:?}"), "PortalEisTextPayload(<redacted>)");
    }

    #[test]
    fn lifecycle_submission_is_actor_owned_unverified_and_uncertainty_closes() {
        let driver = FakeSessionDriver {
            submit_result: PortalEisTextSubmission::SubmittedUnverified,
            ..FakeSessionDriver::default()
        };
        let mut lifecycle = PortalSessionLifecycle::new(driver);
        let cancellation = SessionCancellation::new();
        assert_eq!(
            lifecycle.activate(&parent_window(), future_deadline(), &cancellation),
            Ok(())
        );
        assert_eq!(
            lifecycle.submit_text(text_payload("controlled"), &cancellation, future_deadline(),),
            PortalEisTextSubmission::SubmittedUnverified
        );
        assert_eq!(lifecycle.state(), PortalSessionState::Active);
        assert_eq!(lifecycle.driver.calls, vec!["request", "connect", "submit"]);

        lifecycle.driver.submit_result =
            PortalEisTextSubmission::DispatchUncertain(PortalEisTextFailure::TimedOut);
        assert_eq!(
            lifecycle.submit_text(text_payload("second"), &cancellation, future_deadline(),),
            PortalEisTextSubmission::DispatchUncertain(PortalEisTextFailure::TimedOut)
        );
        assert_eq!(lifecycle.state(), PortalSessionState::Idle);
        assert_eq!(
            lifecycle.driver.calls,
            vec!["request", "connect", "submit", "submit", "close"]
        );
    }

    #[test]
    fn pre_dispatch_transport_failure_closes_without_manufacturing_revocation() {
        let driver = FakeSessionDriver {
            submit_result: PortalEisTextSubmission::RejectedBeforeDispatch(
                PortalEisTextFailure::TransportFailed,
            ),
            ..FakeSessionDriver::default()
        };
        let mut lifecycle = PortalSessionLifecycle::new(driver);
        let cancellation = SessionCancellation::new();
        assert_eq!(
            lifecycle.activate(&parent_window(), future_deadline(), &cancellation),
            Ok(())
        );

        assert_eq!(
            lifecycle.submit_text(text_payload("controlled"), &cancellation, future_deadline(),),
            PortalEisTextSubmission::RejectedBeforeDispatch(PortalEisTextFailure::TransportFailed,)
        );
        assert_eq!(lifecycle.state(), PortalSessionState::Idle);
        assert_eq!(
            lifecycle.driver.calls,
            vec!["request", "connect", "submit", "close"]
        );
        assert!(!lifecycle.local_authority_aborted);
    }

    #[test]
    fn pre_dispatch_cancellation_never_claims_quiescence_when_close_fails() {
        let cancellation = SessionCancellation::new();
        let driver = FakeSessionDriver {
            close_result: Err(PortalSessionFailure::CloseFailed),
            ..FakeSessionDriver::default()
        };
        let mut lifecycle = PortalSessionLifecycle::new(driver);
        assert_eq!(
            lifecycle.activate(&parent_window(), future_deadline(), &cancellation),
            Ok(())
        );
        cancellation.cancel();

        assert_eq!(
            lifecycle.submit_text(text_payload("controlled"), &cancellation, future_deadline(),),
            PortalEisTextSubmission::RejectedBeforeDispatch(PortalEisTextFailure::CloseFailed)
        );
        assert_eq!(lifecycle.state(), PortalSessionState::Invalid);
        assert_eq!(
            lifecycle.driver.calls,
            vec!["request", "connect", "close", "abort"]
        );
    }

    #[test]
    fn cancellation_linearizes_on_the_first_positive_payload_write() -> Result<(), String> {
        for accept_before_pause in [false, true] {
            let reached = Arc::new(Barrier::new(2));
            let resume = Arc::new(Barrier::new(2));
            let accepted = Arc::new(AtomicBool::new(false));
            let driver = FirstWriteBoundaryDriver {
                accept_before_pause,
                reached: Arc::clone(&reached),
                resume: Arc::clone(&resume),
                accepted: Arc::clone(&accepted),
            };
            let cancellation = SessionCancellation::new();
            let mut lifecycle = PortalSessionLifecycle::new(driver);
            assert_eq!(
                lifecycle.activate(&parent_window(), future_deadline(), &cancellation),
                Ok(())
            );
            let worker_cancel = cancellation.clone();
            let worker = std::thread::spawn(move || {
                let outcome = lifecycle.submit_text(
                    text_payload("controlled"),
                    &worker_cancel,
                    future_deadline(),
                );
                (outcome, lifecycle)
            });

            reached.wait();
            cancellation.cancel();
            resume.wait();
            let (outcome, mut lifecycle) = worker
                .join()
                .map_err(|_| "controlled submission worker panicked".to_owned())?;
            if accept_before_pause {
                assert!(accepted.load(Ordering::SeqCst));
                assert_eq!(
                    outcome,
                    PortalEisTextSubmission::DispatchUncertain(PortalEisTextFailure::Cancelled)
                );
                assert_eq!(lifecycle.state(), PortalSessionState::Idle);
            } else {
                assert!(!accepted.load(Ordering::SeqCst));
                assert_eq!(outcome, PortalEisTextSubmission::CancelledBeforeDispatch);
                assert_eq!(lifecycle.state(), PortalSessionState::Active);
                assert_eq!(
                    lifecycle.close(future_deadline()),
                    Ok(PortalCloseResult::Closed)
                );
            }
        }
        Ok(())
    }

    #[test]
    fn activation_owns_one_grant_and_one_connection_until_explicit_close() {
        let mut lifecycle = PortalSessionLifecycle::new(FakeSessionDriver::default());
        let cancellation = SessionCancellation::new();
        assert_eq!(lifecycle.state(), PortalSessionState::Idle);
        assert_eq!(
            lifecycle.submission_allowed(&cancellation, future_deadline()),
            Err(PortalSessionError {
                failure: PortalSessionFailure::NotActive,
                cleanup: PortalCleanupStatus::NotRequired,
            })
        );

        assert_eq!(
            lifecycle.activate(&parent_window(), future_deadline(), &cancellation),
            Ok(())
        );
        assert_eq!(lifecycle.state(), PortalSessionState::Active);
        assert_eq!(
            lifecycle.submission_allowed(&cancellation, future_deadline()),
            Ok(())
        );
        assert_eq!(lifecycle.driver.calls, vec!["request", "connect"]);
        assert!(lifecycle.driver.resource_live);

        assert_eq!(
            lifecycle.close(future_deadline()),
            Ok(PortalCloseResult::Closed)
        );
        assert_eq!(lifecycle.state(), PortalSessionState::Idle);
        assert_eq!(
            lifecycle.submission_allowed(&cancellation, future_deadline()),
            Err(PortalSessionError {
                failure: PortalSessionFailure::NotActive,
                cleanup: PortalCleanupStatus::NotRequired,
            })
        );
        assert_eq!(lifecycle.driver.calls, vec!["request", "connect", "close"]);
        assert!(!lifecycle.driver.resource_live);
    }

    #[test]
    fn duplicate_activation_is_rejected_without_touching_active_resources() {
        let mut lifecycle = PortalSessionLifecycle::new(FakeSessionDriver::default());
        let cancellation = SessionCancellation::new();
        assert_eq!(
            lifecycle.activate(&parent_window(), future_deadline(), &cancellation),
            Ok(())
        );

        assert_eq!(
            lifecycle.activate(&parent_window(), future_deadline(), &cancellation),
            Err(PortalSessionError {
                failure: PortalSessionFailure::AlreadyActive,
                cleanup: PortalCleanupStatus::NotRequired,
            })
        );
        assert_eq!(lifecycle.state(), PortalSessionState::Active);
        assert_eq!(lifecycle.driver.calls, vec!["request", "connect"]);
        assert!(lifecycle.driver.resource_live);
    }

    #[test]
    fn denied_request_performs_idempotent_cleanup_and_never_connects() {
        let driver = FakeSessionDriver {
            request_result: Err(PortalSessionFailure::PermissionDenied),
            ..FakeSessionDriver::default()
        };
        let mut lifecycle = PortalSessionLifecycle::new(driver);
        assert_eq!(
            lifecycle.activate(
                &parent_window(),
                future_deadline(),
                &SessionCancellation::new(),
            ),
            Err(PortalSessionError {
                failure: PortalSessionFailure::PermissionDenied,
                cleanup: PortalCleanupStatus::ConfirmedClosed,
            })
        );
        assert_eq!(lifecycle.state(), PortalSessionState::Idle);
        assert_eq!(lifecycle.driver.calls, vec!["request", "close"]);
        assert!(!lifecycle.driver.resource_live);
    }

    #[test]
    fn connection_failure_closes_the_grant_and_cannot_submit() {
        let driver = FakeSessionDriver {
            connect_result: Err(PortalSessionFailure::TransportFailed),
            ..FakeSessionDriver::default()
        };
        let mut lifecycle = PortalSessionLifecycle::new(driver);
        assert_eq!(
            lifecycle.activate(
                &parent_window(),
                future_deadline(),
                &SessionCancellation::new(),
            ),
            Err(PortalSessionError {
                failure: PortalSessionFailure::TransportFailed,
                cleanup: PortalCleanupStatus::ConfirmedClosed,
            })
        );
        assert_eq!(lifecycle.state(), PortalSessionState::Idle);
        assert_eq!(lifecycle.driver.calls, vec!["request", "connect", "close"]);
        assert_eq!(
            lifecycle.submission_allowed(&SessionCancellation::new(), future_deadline()),
            Err(PortalSessionError {
                failure: PortalSessionFailure::NotActive,
                cleanup: PortalCleanupStatus::NotRequired,
            })
        );
    }

    #[test]
    fn cancellation_after_grant_closes_before_connecting() {
        let cancellation = SessionCancellation::new();
        let driver = FakeSessionDriver {
            cancellation_after_request: Some(cancellation.clone()),
            ..FakeSessionDriver::default()
        };
        let mut lifecycle = PortalSessionLifecycle::new(driver);
        assert_eq!(
            lifecycle.activate(&parent_window(), future_deadline(), &cancellation),
            Err(PortalSessionError {
                failure: PortalSessionFailure::Cancelled,
                cleanup: PortalCleanupStatus::ConfirmedClosed,
            })
        );
        assert_eq!(lifecycle.driver.calls, vec!["request", "close"]);
        assert_eq!(lifecycle.state(), PortalSessionState::Idle);
        assert!(!lifecycle.driver.resource_live);
    }

    #[test]
    fn cancellation_after_connect_closes_before_becoming_active() {
        let cancellation = SessionCancellation::new();
        let driver = FakeSessionDriver {
            cancellation_after_connect: Some(cancellation.clone()),
            ..FakeSessionDriver::default()
        };
        let mut lifecycle = PortalSessionLifecycle::new(driver);
        assert_eq!(
            lifecycle.activate(&parent_window(), future_deadline(), &cancellation),
            Err(PortalSessionError {
                failure: PortalSessionFailure::Cancelled,
                cleanup: PortalCleanupStatus::ConfirmedClosed,
            })
        );
        assert_eq!(lifecycle.driver.calls, vec!["request", "connect", "close"]);
        assert_eq!(lifecycle.state(), PortalSessionState::Idle);
        assert_eq!(
            lifecycle.submission_allowed(&cancellation, future_deadline()),
            Err(PortalSessionError {
                failure: PortalSessionFailure::NotActive,
                cleanup: PortalCleanupStatus::NotRequired,
            })
        );
        assert!(!lifecycle.driver.resource_live);
    }

    #[test]
    fn revocation_between_connect_and_active_is_never_reported_as_active() {
        let driver = FakeSessionDriver {
            next_event: Some(Ok(PortalDriverEvent::Revoked)),
            ..FakeSessionDriver::default()
        };
        let mut lifecycle = PortalSessionLifecycle::new(driver);
        assert_eq!(
            lifecycle.activate(
                &parent_window(),
                future_deadline(),
                &SessionCancellation::new(),
            ),
            Err(PortalSessionError {
                failure: PortalSessionFailure::Revoked,
                cleanup: PortalCleanupStatus::ConfirmedClosed,
            })
        );
        assert_eq!(lifecycle.state(), PortalSessionState::Idle);
        assert_eq!(lifecycle.driver.calls, vec!["request", "connect", "abort"]);
    }

    #[test]
    fn revocation_during_permission_request_aborts_without_second_remote_close() {
        let driver = FakeSessionDriver {
            request_result: Err(PortalSessionFailure::Revoked),
            ..FakeSessionDriver::default()
        };
        let mut lifecycle = PortalSessionLifecycle::new(driver);
        assert_eq!(
            lifecycle.activate(
                &parent_window(),
                future_deadline(),
                &SessionCancellation::new(),
            ),
            Err(PortalSessionError {
                failure: PortalSessionFailure::Revoked,
                cleanup: PortalCleanupStatus::ConfirmedClosed,
            })
        );
        assert_eq!(lifecycle.state(), PortalSessionState::Idle);
        assert_eq!(lifecycle.driver.calls, vec!["request", "abort"]);
    }

    #[test]
    fn revocation_during_transport_connect_aborts_without_second_remote_close() {
        let driver = FakeSessionDriver {
            connect_result: Err(PortalSessionFailure::Revoked),
            ..FakeSessionDriver::default()
        };
        let mut lifecycle = PortalSessionLifecycle::new(driver);
        assert_eq!(
            lifecycle.activate(
                &parent_window(),
                future_deadline(),
                &SessionCancellation::new(),
            ),
            Err(PortalSessionError {
                failure: PortalSessionFailure::Revoked,
                cleanup: PortalCleanupStatus::ConfirmedClosed,
            })
        );
        assert_eq!(lifecycle.state(), PortalSessionState::Idle);
        assert_eq!(lifecycle.driver.calls, vec!["request", "connect", "abort"]);
    }

    #[test]
    fn active_lifecycle_processes_driver_revocation_on_owner_thread() {
        let mut lifecycle = PortalSessionLifecycle::new(FakeSessionDriver::default());
        assert_eq!(
            lifecycle.activate(
                &parent_window(),
                future_deadline(),
                &SessionCancellation::new(),
            ),
            Ok(())
        );
        lifecycle.driver.next_event = Some(Ok(PortalDriverEvent::Revoked));

        assert_eq!(
            lifecycle.poll_driver_event(future_deadline()),
            Ok(Some(PortalSessionError {
                failure: PortalSessionFailure::Revoked,
                cleanup: PortalCleanupStatus::ConfirmedClosed,
            }))
        );
        assert_eq!(lifecycle.state(), PortalSessionState::Idle);
        assert_eq!(lifecycle.driver.calls, vec!["request", "connect", "abort"]);
    }

    #[test]
    fn pre_cancel_and_expired_deadline_allocate_nothing() {
        let cancellation = SessionCancellation::new();
        cancellation.cancel();
        let mut cancelled = PortalSessionLifecycle::new(FakeSessionDriver::default());
        assert_eq!(
            cancelled.activate(&parent_window(), future_deadline(), &cancellation),
            Err(PortalSessionError {
                failure: PortalSessionFailure::Cancelled,
                cleanup: PortalCleanupStatus::NotRequired,
            })
        );
        assert!(cancelled.driver.calls.is_empty());

        let mut timed_out = PortalSessionLifecycle::new(FakeSessionDriver::default());
        assert_eq!(
            timed_out.activate(
                &parent_window(),
                Instant::now(),
                &SessionCancellation::new(),
            ),
            Err(PortalSessionError {
                failure: PortalSessionFailure::TimedOut,
                cleanup: PortalCleanupStatus::NotRequired,
            })
        );
        assert!(timed_out.driver.calls.is_empty());
    }

    #[test]
    fn close_failure_invalidates_the_transport_and_blocks_submission() {
        let abort_observed = Arc::new(AtomicBool::new(false));
        let driver = FakeSessionDriver {
            close_result: Err(PortalSessionFailure::TimedOut),
            abort_observed: Some(Arc::clone(&abort_observed)),
            ..FakeSessionDriver::default()
        };
        let mut lifecycle = PortalSessionLifecycle::new(driver);
        assert_eq!(
            lifecycle.activate(
                &parent_window(),
                future_deadline(),
                &SessionCancellation::new(),
            ),
            Ok(())
        );
        assert_eq!(
            lifecycle.close(future_deadline()),
            Err(PortalSessionError {
                failure: PortalSessionFailure::CloseFailed,
                cleanup: PortalCleanupStatus::Failed,
            })
        );
        assert_eq!(lifecycle.state(), PortalSessionState::Invalid);
        assert_eq!(
            lifecycle.submission_allowed(&SessionCancellation::new(), future_deadline()),
            Err(PortalSessionError {
                failure: PortalSessionFailure::Invalidated,
                cleanup: PortalCleanupStatus::NotRequired,
            })
        );
        assert_eq!(
            lifecycle.activate(
                &parent_window(),
                future_deadline(),
                &SessionCancellation::new(),
            ),
            Err(PortalSessionError {
                failure: PortalSessionFailure::Invalidated,
                cleanup: PortalCleanupStatus::NotRequired,
            })
        );
        assert!(abort_observed.load(Ordering::SeqCst));
        assert!(!lifecycle.driver.resource_live);
        let calls_after_abort = lifecycle.driver.calls.clone();
        assert_eq!(
            lifecycle.close(future_deadline()),
            Err(PortalSessionError {
                failure: PortalSessionFailure::Invalidated,
                cleanup: PortalCleanupStatus::Failed,
            })
        );
        assert_eq!(lifecycle.driver.calls, calls_after_abort);
        lifecycle.driver.next_event = Some(Err(PortalSessionFailure::RequestFailed));
        assert_eq!(
            lifecycle.poll_driver_event(future_deadline()),
            Err(PortalSessionError {
                failure: PortalSessionFailure::Invalidated,
                cleanup: PortalCleanupStatus::Failed,
            })
        );
        assert_eq!(lifecycle.driver.calls, calls_after_abort);
        drop(lifecycle);
    }

    #[test]
    fn cancellation_after_activation_is_rejected_at_the_submission_gate() {
        let cancellation = SessionCancellation::new();
        let mut lifecycle = PortalSessionLifecycle::new(FakeSessionDriver::default());
        assert_eq!(
            lifecycle.activate(&parent_window(), future_deadline(), &cancellation),
            Ok(())
        );

        // This is the deterministic barrier between activation and a future
        // sender's first accepted event.
        cancellation.cancel();
        assert_eq!(
            lifecycle.submission_allowed(&cancellation, future_deadline()),
            Err(PortalSessionError {
                failure: PortalSessionFailure::Cancelled,
                cleanup: PortalCleanupStatus::ConfirmedClosed,
            })
        );
        assert_eq!(lifecycle.state(), PortalSessionState::Idle);
        assert_eq!(lifecycle.driver.calls, vec!["request", "connect", "close"]);
    }

    #[test]
    fn dropping_an_active_owner_aborts_local_transport_authority() {
        let abort_observed = Arc::new(AtomicBool::new(false));
        let driver = FakeSessionDriver {
            abort_observed: Some(Arc::clone(&abort_observed)),
            ..FakeSessionDriver::default()
        };
        let mut lifecycle = PortalSessionLifecycle::new(driver);
        assert_eq!(
            lifecycle.activate(
                &parent_window(),
                future_deadline(),
                &SessionCancellation::new(),
            ),
            Ok(())
        );
        assert!(lifecycle.driver.resource_live);
        drop(lifecycle);
        assert!(abort_observed.load(Ordering::SeqCst));
    }

    #[test]
    fn observed_revocation_aborts_locally_and_returns_to_idle() {
        let mut lifecycle = PortalSessionLifecycle::new(FakeSessionDriver::default());
        assert_eq!(
            lifecycle.activate(
                &parent_window(),
                future_deadline(),
                &SessionCancellation::new(),
            ),
            Ok(())
        );
        assert_eq!(
            lifecycle.observe_revoked(),
            PortalSessionError {
                failure: PortalSessionFailure::Revoked,
                cleanup: PortalCleanupStatus::ConfirmedClosed,
            }
        );
        assert_eq!(lifecycle.state(), PortalSessionState::Idle);
        assert_eq!(lifecycle.driver.calls, vec!["request", "connect", "abort"]);
        assert!(!lifecycle.driver.resource_live);
    }

    #[test]
    fn fresh_session_after_revocation_owns_a_fresh_local_abort() {
        let abort_count = Arc::new(AtomicUsize::new(0));
        let driver = FakeSessionDriver {
            abort_count: Some(Arc::clone(&abort_count)),
            ..FakeSessionDriver::default()
        };
        let mut lifecycle = PortalSessionLifecycle::new(driver);
        assert_eq!(
            lifecycle.activate(
                &parent_window(),
                future_deadline(),
                &SessionCancellation::new(),
            ),
            Ok(())
        );

        assert_eq!(
            lifecycle.observe_revoked(),
            PortalSessionError {
                failure: PortalSessionFailure::Revoked,
                cleanup: PortalCleanupStatus::ConfirmedClosed,
            }
        );
        assert_eq!(abort_count.load(Ordering::SeqCst), 1);
        assert_eq!(lifecycle.state(), PortalSessionState::Idle);
        assert_eq!(
            lifecycle.activate(
                &parent_window(),
                future_deadline(),
                &SessionCancellation::new(),
            ),
            Ok(())
        );
        drop(lifecycle);
        assert_eq!(abort_count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn absent_interface_fails_closed() {
        let state = classify_portal_eis(&FixedProbe(Ok(RemoteDesktopFacts::InterfaceAbsent)));
        assert_eq!(state, PortalEisEligibility::InterfaceAbsent);
        assert!(!state.is_delivery_ready());
        assert!(!state.may_request_permission());
    }

    #[test]
    fn version_one_is_not_eis_eligible() {
        let state = classify_portal_eis(&FixedProbe(Ok(RemoteDesktopFacts::Advertised {
            version: 1,
            keyboard_available: true,
        })));
        assert_eq!(state, PortalEisEligibility::VersionTooOld { version: 1 });
        assert!(!state.is_delivery_ready());
    }

    #[test]
    fn missing_keyboard_fails_closed() {
        let state = classify_portal_eis(&FixedProbe(Ok(RemoteDesktopFacts::Advertised {
            version: 2,
            keyboard_available: false,
        })));
        assert_eq!(
            state,
            PortalEisEligibility::KeyboardUnavailable { version: 2 }
        );
        assert!(!state.is_delivery_ready());
    }

    #[test]
    fn portal_failure_is_not_availability() {
        let state = classify_portal_eis(&FixedProbe(Err(PortalProbeFailure::QueryFailed)));
        assert_eq!(
            state,
            PortalEisEligibility::ProbeFailed(PortalProbeFailure::QueryFailed)
        );
        assert!(!state.is_delivery_ready());
    }

    #[test]
    fn ashpd_portal_absence_reaches_the_production_absent_classification()
    -> Result<(), Box<dyn std::error::Error>> {
        let error =
            ashpd::Error::PortalNotFound("org.freedesktop.portal.RemoteDesktop".try_into()?);
        assert_eq!(
            classify_remote_desktop_open_error(error),
            Ok(RemoteDesktopFacts::InterfaceAbsent)
        );
        Ok(())
    }

    #[test]
    fn version_two_with_keyboard_only_allows_permission_request() {
        let state = classify_portal_eis(&FixedProbe(Ok(RemoteDesktopFacts::Advertised {
            version: 2,
            keyboard_available: true,
        })));
        assert_eq!(
            state,
            PortalEisEligibility::ReadyToRequestPermission { version: 2 }
        );
        assert!(state.may_request_permission());
        assert!(!state.is_delivery_ready());
    }

    #[test]
    fn timeout_cancels_and_drops_the_actual_pending_query() -> Result<(), Box<dyn std::error::Error>>
    {
        struct PendingQuery(Arc<AtomicBool>);

        impl Future for PendingQuery {
            type Output = Result<RemoteDesktopFacts, PortalProbeFailure>;

            fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
                Poll::Pending
            }
        }

        impl Drop for PendingQuery {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let dropped = Arc::new(AtomicBool::new(false));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()?;
        let result = runtime.block_on(run_bounded_query(
            Duration::ZERO,
            PendingQuery(Arc::clone(&dropped)),
        ));
        assert_eq!(result, Err(PortalProbeFailure::TimedOut));
        assert!(dropped.load(Ordering::SeqCst));

        let failure = match result {
            Err(failure) => failure,
            Ok(_) => return Err("timed query unexpectedly succeeded".into()),
        };
        let state = PortalEisEligibility::ProbeFailed(failure);
        let rendered = format!("{state:?}");
        assert_eq!(rendered, "ProbeFailed(TimedOut)");
        assert!(!rendered.contains("DBUS_SESSION_BUS_ADDRESS"));
        assert!(!state.is_delivery_ready());
        Ok(())
    }

    #[test]
    fn synchronous_probe_fails_closed_inside_an_existing_runtime()
    -> Result<(), Box<dyn std::error::Error>> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()?;
        runtime.block_on(async {
            assert_eq!(
                SystemRemoteDesktopProbe::default().inspect(),
                Err(PortalProbeFailure::RuntimeUnavailable)
            );
        });
        Ok(())
    }

    #[test]
    #[ignore = "read-only observation of the current desktop session"]
    fn current_session_portal_observation_never_claims_delivery_ready() {
        let state = classify_portal_eis(&SystemRemoteDesktopProbe::default());
        eprintln!("portal/EIS eligibility: {state:?}");
        assert!(!state.is_delivery_ready());
    }
}
