//! POSIX cache file policy for the portable model-cache contract.
//!
//! This is the only place in the release graph that may set a permission bit
//! or read a POSIX owner for a model-cache object. It owns no store policy: the
//! paths that must be protected, and the obligation that they be owner-only,
//! are decided in `narrivy-model-manager`.
//!
//! 0600 files and 0700 directories are a privacy control, so every method here
//! reports a real failure rather than degrading to a no-op.

use std::fs::{File, Metadata, Permissions};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

use narrivy_model_manager::error::{ModelManagerError, Result};
use narrivy_model_manager::policy::{CacheEntryKind, CacheFilePolicy, CacheOwner};

/// Owner-only for a regular file: no group or other access at all.
const PRIVATE_FILE_MODE: u32 = 0o600;

/// Owner-only for a directory, and unsearchable by any other user.
const PRIVATE_DIRECTORY_MODE: u32 = 0o700;

/// The Linux model cache: real owner-only bits, a real uid, and real
/// device+inode identity. Stateless, cheap to clone, and safe to share.
#[derive(Debug, Clone, Copy, Default)]
pub struct PosixCacheFilePolicy;

impl PosixCacheFilePolicy {
    pub const fn new() -> Self {
        Self
    }

    fn failed(
        path: &Path,
        kind: CacheEntryKind,
        reason: impl std::fmt::Display,
    ) -> ModelManagerError {
        ModelManagerError::CacheProtectionUnavailable {
            path: path.display().to_string(),
            kind: match kind {
                CacheEntryKind::File => "file",
                CacheEntryKind::Directory => "directory",
            },
            reason: reason.to_string(),
        }
    }
}

impl CacheFilePolicy for PosixCacheFilePolicy {
    fn restrict_private(&self, path: &Path, kind: CacheEntryKind) -> Result<()> {
        let mode = match kind {
            CacheEntryKind::File => PRIVATE_FILE_MODE,
            CacheEntryKind::Directory => PRIVATE_DIRECTORY_MODE,
        };
        // An explicit mode, not one derived from the process umask: the
        // guarantee has to hold for every process that starts this daemon.
        std::fs::set_permissions(path, Permissions::from_mode(mode))
            .map_err(|error| Self::failed(path, kind, error))
    }

    fn create_private_file(&self, path: &Path) -> Result<File> {
        // The mode is applied by the open itself: a create-then-chmod window
        // would leave the sidecar briefly world-readable.
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(PRIVATE_FILE_MODE)
            .open(path)
            .map_err(|error| Self::failed(path, CacheEntryKind::File, error))
    }

    fn owner_of(&self, metadata: &Metadata) -> Result<CacheOwner> {
        Ok(CacheOwner::new(metadata.uid().to_string()))
    }

    fn same_object(&self, path: &Metadata, opened: &Metadata) -> bool {
        // Device plus inode is the only identity that closes the stat-then-open
        // race: a same-size file substituted in between has a different inode.
        path.dev() == opened.dev() && path.ino() == opened.ino()
    }
}

/// A policy for a host that cannot answer, so composition has something
/// explicit to hold. Every obligation fails with the reason it was given.
#[derive(Debug, Clone)]
pub struct UnavailableCacheFilePolicy {
    reason: String,
}

impl UnavailableCacheFilePolicy {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    fn failed(&self, kind: CacheEntryKind, reason: &str) -> ModelManagerError {
        ModelManagerError::CacheProtectionUnavailable {
            path: "<cache root>".to_owned(),
            kind: match kind {
                CacheEntryKind::Directory => "directory",
                CacheEntryKind::File => "file",
            },
            reason: reason.to_owned(),
        }
    }
}

impl CacheFilePolicy for UnavailableCacheFilePolicy {
    fn restrict_private(&self, _path: &Path, kind: CacheEntryKind) -> Result<()> {
        Err(self.failed(kind, &self.reason))
    }

    fn create_private_file(&self, _path: &Path) -> Result<File> {
        Err(self.failed(CacheEntryKind::File, &self.reason))
    }

    fn owner_of(&self, _metadata: &Metadata) -> Result<CacheOwner> {
        Err(ModelManagerError::CacheProtectionUnavailable {
            path: "<cache root>".to_owned(),
            kind: "directory",
            reason: self.reason.clone(),
        })
    }

    fn same_object(&self, _path: &Metadata, _opened: &Metadata) -> bool {
        // A platform that cannot prove identity must answer "not proven", so
        // the store refuses the bytes instead of assuming they are the ones
        // it inspected.
        false
    }
}
