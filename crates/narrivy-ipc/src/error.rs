//! Typed IPC errors shared by the protocol, transport, client and server
//! layers.
//!
//! # Wire representation and unknown-value handling
//!
//! [`IpcErrorCode`] serializes as a bare snake_case string (external string
//! tag), e.g. `"permission_denied"`. Newer peers may send tags this build
//! does not know. Choice (documented per P1 requirements): unknown strings
//! deserialize **losslessly** into `IpcErrorCode::Unknown(raw)`, a
//! `Unknown(String)`-style fallback variant that preserves the original
//! wire detail; [`IpcErrorCode::normalized`] collapses it to
//! [`IpcErrorCode::Internal`] for consumers that only classify severity.
//! Serializing an `Unknown` writes its inner string back verbatim, so
//! proxying peers keep full fidelity across protocol upgrades.

#![forbid(unsafe_code)]

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Stable wire identifier for one IPC failure category.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum IpcErrorCode {
    IncompatibleProtocol,
    PermissionDenied,
    PayloadTooLarge,
    UnknownCommand,
    BadRequest,
    Timeout,
    Internal,
    DaemonUnavailable,
    Cancelled,
    Busy,
    /// Unrecognized wire tag preserved verbatim for forward compatibility.
    Unknown(String),
}

impl IpcErrorCode {
    /// Stable external string tag for this code.
    ///
    /// Known variants return their fixed snake_case tag; `Unknown` returns
    /// the original wire string.
    pub fn wire_tag(&self) -> &str {
        match self {
            Self::IncompatibleProtocol => "incompatible_protocol",
            Self::PermissionDenied => "permission_denied",
            Self::PayloadTooLarge => "payload_too_large",
            Self::UnknownCommand => "unknown_command",
            Self::BadRequest => "bad_request",
            Self::Timeout => "timeout",
            Self::Internal => "internal",
            Self::DaemonUnavailable => "daemon_unavailable",
            Self::Cancelled => "cancelled",
            Self::Busy => "busy",
            Self::Unknown(raw) => raw.as_str(),
        }
    }

    /// Decode a wire tag into an error code with lossless unknown fallback.
    pub fn from_wire_tag(tag: &str) -> Self {
        match tag {
            "incompatible_protocol" => Self::IncompatibleProtocol,
            "permission_denied" => Self::PermissionDenied,
            "payload_too_large" => Self::PayloadTooLarge,
            "unknown_command" => Self::UnknownCommand,
            "bad_request" => Self::BadRequest,
            "timeout" => Self::Timeout,
            "internal" => Self::Internal,
            "daemon_unavailable" => Self::DaemonUnavailable,
            "cancelled" => Self::Cancelled,
            "busy" => Self::Busy,
            other => Self::Unknown(other.to_owned()),
        }
    }

    /// Severity-normalized view: unknown codes count as [`IpcErrorCode::Internal`].
    pub fn normalized(&self) -> Self {
        match self {
            Self::Unknown(_) => Self::Internal,
            known => known.clone(),
        }
    }
}

impl Serialize for IpcErrorCode {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.wire_tag())
    }
}

impl<'de> Deserialize<'de> for IpcErrorCode {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let tag = String::deserialize(deserializer)?;
        Ok(Self::from_wire_tag(&tag))
    }
}

impl std::fmt::Display for IpcErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.wire_tag())
    }
}

/// Typed IPC failure: a stable error category plus a diagnostic message.
///
/// Messages are generic diagnostics only — they must never carry transcript
/// text, audio, clipboard content or secrets (workspace security rules).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("ipc error [{code}]: {message}")]
pub struct IpcError {
    pub code: IpcErrorCode,
    pub message: String,
}

impl IpcError {
    pub fn new(code: IpcErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(IpcErrorCode::BadRequest, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(IpcErrorCode::Internal, message)
    }

    pub fn permission_denied(message: impl Into<String>) -> Self {
        Self::new(IpcErrorCode::PermissionDenied, message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipc_error_code_known_tags_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
        let known = [
            IpcErrorCode::IncompatibleProtocol,
            IpcErrorCode::PermissionDenied,
            IpcErrorCode::PayloadTooLarge,
            IpcErrorCode::UnknownCommand,
            IpcErrorCode::BadRequest,
            IpcErrorCode::Timeout,
            IpcErrorCode::Internal,
            IpcErrorCode::DaemonUnavailable,
            IpcErrorCode::Cancelled,
            IpcErrorCode::Busy,
        ];
        for code in known {
            let json = serde_json::to_string(&code)?;
            assert_eq!(json, format!("\"{code}\""));
            let back: IpcErrorCode = serde_json::from_str(&json)?;
            assert_eq!(back, code);
        }
        Ok(())
    }

    #[test]
    fn ipc_error_code_unknown_string_preserved() -> Result<(), Box<dyn std::error::Error>> {
        // A future daemon sends a code this build has never seen.
        let raw = "\"pipeline_quota_exceeded_2038\"";
        let decoded: IpcErrorCode = serde_json::from_str(raw)?;
        assert_eq!(
            decoded,
            IpcErrorCode::Unknown("pipeline_quota_exceeded_2038".into())
        );
        // Lossless re-serialization and severity normalization.
        let encoded = serde_json::to_string(&decoded)?;
        assert_eq!(encoded, raw);
        assert_eq!(decoded.normalized(), IpcErrorCode::Internal);
        Ok(())
    }

    #[test]
    fn ipc_error_display_and_helpers() {
        let err = IpcError::permission_denied("cross-user connection denied");
        assert_eq!(err.code, IpcErrorCode::PermissionDenied);
        assert!(err.to_string().contains("permission_denied"));
        assert!(err.to_string().contains("cross-user connection denied"));
    }
}
