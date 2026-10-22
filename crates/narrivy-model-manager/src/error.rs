//! Typed errors for the model manager. No panic-driven control flow.

use std::path::PathBuf;

/// Every failure mode of registry resolution, verified acquisition, residency
/// bookkeeping, and cleanup.
#[derive(Debug, thiserror::Error)]
pub enum ModelManagerError {
    #[error("registry file {path}: {message}")]
    RegistryParse { path: PathBuf, message: String },

    #[error("registry {path}: unsupported version {found} (supported: 1)")]
    RegistryVersion { path: PathBuf, found: u32 },

    #[error("unknown model id `{0}` in registry")]
    UnknownModel(String),

    #[error("model id is not present in the bundled registry")]
    UnknownBundledModel,

    #[error("registry entry `{id}` is invalid: {reason}")]
    InvalidRegistryEntry { id: String, reason: &'static str },

    #[error(
        "entry `{0}` has no sha256 pin; SEC-002 forbids activation without a \
         verifiable checksum"
    )]
    ChecksumRequired(String),

    #[error("entry `{0}` declares no usable source")]
    NoSource(String),

    #[error("refusing untrusted source url `{0}` (SEC-007: https or file only)")]
    UntrustedSourceUrl(String),

    #[error("transport error fetching `{url}`: {message}")]
    Transport { url: String, message: String },

    #[error("transport timed out after {timeout_secs}s fetching `{url}`")]
    Timeout { url: String, timeout_secs: u64 },

    #[error(
        "corrupt download for entry `{id}`: sha256 expected {expected}, got {actual}; \
         staged part removed"
    )]
    CorruptDownload {
        id: String,
        expected: String,
        actual: String,
        part_path: PathBuf,
    },

    #[error("installed model `{id}` is incomplete or unsafe")]
    UnsafeInstalledModel { id: String },

    #[error("installed model `{id}` is missing")]
    InstalledModelMissing { id: String },

    #[error("installed model `{id}` metadata does not match the registry identity")]
    InstalledMetadataMismatch { id: String },

    #[error("installed model `{id}` size does not match the registry identity")]
    InstalledSizeMismatch { id: String },

    #[error("installed model `{id}` digest does not match the registry identity")]
    InstalledDigestMismatch { id: String },

    #[error("installed model verification was cancelled")]
    InstalledVerificationCancelled,

    #[error("installed model verification timed out")]
    InstalledVerificationTimedOut,

    #[error("offline import bytes for model `{id}` do not match the bundled identity")]
    OfflineImportDigestMismatch { id: String },

    #[error("model cache root is relative, traversing, or contains a symlink")]
    UnsafeCacheRoot,

    #[error("cannot restrict {path} ({kind}) to owner-only access: {reason}")]
    CacheProtectionUnavailable {
        path: String,
        kind: &'static str,
        reason: String,
    },

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Convenience alias used across the crate surface.
pub type Result<T> = std::result::Result<T, ModelManagerError>;
