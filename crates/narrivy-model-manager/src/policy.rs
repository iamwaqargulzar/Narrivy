//! The platform seam for cache file protection.
//!
//! A local model cache holds downloaded artifacts and an install inventory,
//! so owner-only permissions and single-owner directories are a privacy
//! control, not incidental hardening. This crate therefore performs no
//! permission work of its own: it decides *which* paths must be protected and
//! *what must be provable about them*, and composition injects a
//! [`CacheFilePolicy`] that owns every platform-specific way of establishing
//! it.
//!
//! The contract is stated as an obligation ("this path must be owner-only"),
//! never as a bit pattern. The portable crate keeps the requirement, and the
//! platform decides the representation, so composition cannot quietly weaken a
//! 0600 model cache into a no-op without failing a test.

use std::fs::{File, Metadata};
use std::path::Path;

use crate::error::{ModelManagerError, Result};

/// Which kind of cache object a protection obligation applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CacheEntryKind {
    /// A regular file: model bytes, and the installed sidecar beside it.
    File,
    /// A directory: the cache root and every component created below it.
    Directory,
}

impl CacheEntryKind {
    fn describe(self) -> &'static str {
        match self {
            CacheEntryKind::File => "file",
            CacheEntryKind::Directory => "directory",
        }
    }
}

/// An opaque, platform-defined owner identity: a POSIX uid, a Windows SID, or
/// whatever the host's security model actually answers with.
///
/// The store only ever compares two of these for equality. It must not learn
/// to parse one, because the moment it does, the platform boundary has leaked
/// into the portable crate.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CacheOwner(String);

impl CacheOwner {
    /// Wrap a platform identity for storage and comparison.
    pub fn new(identity: impl Into<String>) -> Self {
        Self(identity.into())
    }

    /// The platform's own textual form of the identity.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for CacheOwner {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Every platform operation the verified model cache needs.
pub trait CacheFilePolicy: Send + Sync {
    /// Reduce an existing `path` to owner-only access for `kind`.
    ///
    /// This is the privacy control. An implementation that cannot guarantee it
    /// on this platform must fail rather than report success: a cache the
    /// store believes is private and that is not is worse than a refusal.
    fn restrict_private(&self, path: &Path, kind: CacheEntryKind) -> Result<()>;

    /// Create a brand-new owner-only regular file.
    ///
    /// The protection is established by the platform at creation time, not by
    /// a follow-up `chmod`, so the bytes are never briefly readable by another
    /// user. Creating an existing path is an error.
    fn create_private_file(&self, path: &Path) -> Result<File>;

    /// The owner identity recorded in `metadata`.
    ///
    /// `metadata` must be the very same observation the store used to decide
    /// the object is the kind it expected, so a substituted path cannot slip a
    /// different answer past the check.
    fn owner_of(&self, metadata: &Metadata) -> Result<CacheOwner>;

    /// True when the `path` observation and an already-open handle's metadata
    /// describe the same filesystem object.
    ///
    /// This closes the stat-then-open race. `false` must be the answer whenever
    /// the platform cannot prove identity, because the store treats `false` as
    /// "refuse to trust these bytes".
    fn same_object(&self, path: &Metadata, opened: &Metadata) -> bool;
}

/// The failure every platform implementation should report when it cannot
/// honour an obligation, so the reason survives into diagnostics without
/// naming an OS-specific mechanism.
pub fn protection_unavailable(
    path: &Path,
    kind: CacheEntryKind,
    reason: &str,
) -> ModelManagerError {
    ModelManagerError::CacheProtectionUnavailable {
        path: path.display().to_string(),
        kind: kind.describe(),
        reason: reason.to_owned(),
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// One obligation the store asked the platform to satisfy.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Obligation {
        /// An existing path must become owner-only.
        Restrict { path: PathBuf, kind: CacheEntryKind },
        /// A new owner-only file must be created at this path.
        CreatePrivate { path: PathBuf },
    }

    /// A policy that records what it was asked and answers from a script, with
    /// no OS behaviour of its own. Every `src/` unit test uses this so the
    /// store's obligations can be asserted without a platform.
    pub struct RecordingPolicy {
        owner: String,
        restrict_failure: Option<String>,
        create_failure: Option<String>,
        objects_match: bool,
        foreign_descendant: bool,
        owner_queries: AtomicUsize,
        asked: Mutex<Vec<Obligation>>,
    }

    impl Default for RecordingPolicy {
        fn default() -> Self {
            Self {
                owner: "owner-0".to_owned(),
                restrict_failure: None,
                create_failure: None,
                objects_match: true,
                foreign_descendant: false,
                owner_queries: AtomicUsize::new(0),
                asked: Mutex::new(Vec::new()),
            }
        }
    }

    impl RecordingPolicy {
        /// Every obligation the store imposed, in order.
        pub fn asked(&self) -> Vec<Obligation> {
            self.asked
                .lock()
                .map(|asked| asked.clone())
                .unwrap_or_default()
        }

        /// Make every `restrict_private` call fail, as a platform without
        /// owner-only permissions would.
        pub fn refusing_to_restrict(mut self, reason: &str) -> Self {
            self.restrict_failure = Some(reason.to_owned());
            self
        }

        /// Make every `create_private_file` call fail.
        pub fn refusing_to_create(mut self, reason: &str) -> Self {
            self.create_failure = Some(reason.to_owned());
            self
        }

        /// Report every directory below the cache root as owned by somebody
        /// else. The store always asks about the root first, so the first
        /// answer is the root's own identity and the next one is foreign.
        pub fn with_foreign_descendant(mut self) -> Self {
            self.foreign_descendant = true;
            self
        }

        /// Report that the opened handle is not the observed path.
        pub fn with_substituted_objects(mut self) -> Self {
            self.objects_match = false;
            self
        }
    }

    impl CacheFilePolicy for RecordingPolicy {
        fn restrict_private(&self, path: &Path, kind: CacheEntryKind) -> Result<()> {
            let mut asked = self
                .asked
                .lock()
                .map_err(|_| protection_unavailable(path, kind, "obligation log is poisoned"))?;
            asked.push(Obligation::Restrict {
                path: path.to_path_buf(),
                kind,
            });
            drop(asked);
            match &self.restrict_failure {
                Some(reason) => Err(protection_unavailable(path, kind, reason)),
                None => Ok(()),
            }
        }

        fn create_private_file(&self, path: &Path) -> Result<File> {
            let mut asked = self.asked.lock().map_err(|_| {
                protection_unavailable(path, CacheEntryKind::File, "obligation log is poisoned")
            })?;
            asked.push(Obligation::CreatePrivate {
                path: path.to_path_buf(),
            });
            drop(asked);
            match &self.create_failure {
                Some(reason) => Err(protection_unavailable(path, CacheEntryKind::File, reason)),
                None => std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(path)
                    .map_err(Into::into),
            }
        }

        fn owner_of(&self, _metadata: &Metadata) -> Result<CacheOwner> {
            let index = self.owner_queries.fetch_add(1, Ordering::Relaxed);
            if self.foreign_descendant && index > 0 {
                return Ok(CacheOwner::new("somebody-else"));
            }
            Ok(CacheOwner::new(self.owner.clone()))
        }

        fn same_object(&self, _path: &Metadata, _opened: &Metadata) -> bool {
            self.objects_match
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_owner_identity_is_opaque_text_that_only_has_to_compare_equal() {
        let first = CacheOwner::new("1000");
        let same = CacheOwner::new("1000".to_owned());
        let other = CacheOwner::new("1001");
        assert_eq!(first, same);
        assert_ne!(first, other);
        assert_eq!(first.as_str(), "1000");
        assert_eq!(first.to_string(), "1000");
    }

    #[test]
    fn a_refusal_names_the_path_and_the_kind_it_could_not_protect() {
        let error = protection_unavailable(
            Path::new("/home/u/.cache/narrivy/staging/x.part"),
            CacheEntryKind::File,
            "no owner-only mode",
        );
        assert!(matches!(
            error,
            ModelManagerError::CacheProtectionUnavailable { .. }
        ));
        assert_eq!(
            error.to_string(),
            "cannot restrict /home/u/.cache/narrivy/staging/x.part (file) to owner-only access: \
             no owner-only mode"
        );
    }
}
