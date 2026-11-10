//! Windows stand-in for the daemon-owned RemoteDesktop portal service
//! (`portal.rs`, PORT-007).
//!
//! The XDG portal exists only on Linux desktops. The Windows composition
//! passes no portal service to the handler, so the portal commands answer
//! with the same typed "not composed" rejection a Linux daemon gives when it
//! is composed without one, and status reports the keyboard grant as
//! absent. The types keep the shapes the handler matches on; the service
//! itself can never be constructed.

#![forbid(unsafe_code)]

use std::convert::Infallible;
use std::time::Instant;

use narrivy_ipc::PortalKeyboardStatusDto;

/// Same shape as the Linux portal failure (content-free).
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

/// Same shape as the Linux cleanup status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortalCleanupStatus {
    NotNeeded,
    Completed,
    Failed,
}

/// Same shape as the Linux lifecycle error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortalSessionError {
    pub failure: PortalSessionFailure,
    pub cleanup: PortalCleanupStatus,
}

/// There are no XDG parent-window handles on Windows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortalParentWindow(Infallible);

impl PortalParentWindow {
    pub fn parse(_value: &str) -> Result<Self, PortalSessionFailure> {
        Err(PortalSessionFailure::PortalUnavailable)
    }
}

/// Same shape as the Linux actor failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortalActorFailure {
    AdmissionClosed,
    Busy,
    TimedOut,
    WorkerStopped,
    WorkerFailed,
    Lifecycle(PortalSessionError),
}

/// Same shape as the Linux service failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortalPermissionServiceFailure {
    AdmissionClosed,
    Busy,
    InvalidOperation,
    Actor(PortalActorFailure),
}

/// Never constructed on Windows.
pub struct PortalPermissionService {
    never: Infallible,
}

impl PortalPermissionService {
    pub fn snapshot(&self) -> PortalKeyboardStatusDto {
        match self.never {}
    }

    pub fn request(
        &self,
        _operation_id: String,
        _parent: PortalParentWindow,
        _deadline: Instant,
    ) -> Result<PortalKeyboardStatusDto, PortalPermissionServiceFailure> {
        match self.never {}
    }

    pub fn cancel(
        &self,
        _operation_id: &str,
    ) -> Result<PortalKeyboardStatusDto, PortalPermissionServiceFailure> {
        match self.never {}
    }

    pub fn revoke(
        &self,
        _deadline: Instant,
    ) -> Result<PortalKeyboardStatusDto, PortalPermissionServiceFailure> {
        match self.never {}
    }
}
