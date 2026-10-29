//! Inbox fallback store: the always-available durable sink (FUN-009).
//!
//! Salvage-adapted from the v0.4 `InboxStore`: writes are atomic (temp file +
//! rename) into a 0700 directory, and filenames follow `<txid>.txt` where the
//! transaction id is validated against a strict charset so a hostile id can
//! never escape the inbox directory. Payloads are stored verbatim as UTF-8
//! text — Unicode and multiline content preserved byte-for-byte.

use std::fs::File;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use narrivy_types::TransactionId;
use tempfile::Builder;

/// Errors surfaced by the inbox store. Variants never carry payload text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboxError {
    InvalidTxid,
    Io(String),
}

impl std::fmt::Display for InboxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidTxid => write!(f, "transaction id failed filename validation"),
            Self::Io(e) => write!(f, "inbox io error: {e}"),
        }
    }
}

impl std::error::Error for InboxError {}

/// Validate a transaction id for safe use as a filename stem.
///
/// Rules: non-empty, at most 128 bytes, only `[A-Za-z0-9._-]`, and the stem
/// must not start with `.` (hidden/temp namespace) or `-` (option-like).
/// This makes `<txid>.txt` impossible to turn into a traversal or dotfile.
pub fn validate_txid(txid: &str) -> Result<(), InboxError> {
    if txid.is_empty() || txid.len() > 128 {
        return Err(InboxError::InvalidTxid);
    }
    let first = txid.as_bytes()[0];
    if first == b'.' || first == b'-' {
        return Err(InboxError::InvalidTxid);
    }
    if !txid
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_')
    {
        return Err(InboxError::InvalidTxid);
    }
    Ok(())
}

/// Filesystem inbox: one `<txid>.txt` file per transaction, payload verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxStore {
    pub dir: PathBuf,
}

impl InboxStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// Validated final path for a transaction id.
    pub fn path_for(&self, txid: &TransactionId) -> Result<PathBuf, InboxError> {
        validate_txid(txid.as_str())?;
        Ok(self.dir.join(format!("{}.txt", txid.as_str())))
    }

    /// Establish the private Inbox root before a caller crosses an external
    /// delivery boundary. This contains no payload I/O.
    pub fn ensure_private_directory(&self) -> Result<(), InboxError> {
        ensure_private_dir(&self.dir)
    }

    /// Atomically store `text` as `<txid>.txt`: write a unique temp file in
    /// the inbox directory, fsync it, rename it into place, then fsync the
    /// directory best-effort. Readers therefore see either the old state or
    /// the complete new file — never a partial write — and a failed attempt
    /// leaves no temp artifact behind.
    ///
    /// PRIVACY: payload bytes go only into the inbox file; nothing is logged.
    pub fn store_text(&self, txid: &TransactionId, text: &str) -> Result<PathBuf, InboxError> {
        self.store_text_if(txid, text, || true)
    }

    /// Atomic write with a cancellation/authorization guard checked before
    /// the irreversible rename. A false guard leaves neither final artifact
    /// nor temporary payload file behind.
    pub fn store_text_if<F>(
        &self,
        txid: &TransactionId,
        text: &str,
        allow_commit: F,
    ) -> Result<PathBuf, InboxError>
    where
        F: FnOnce() -> bool,
    {
        // Validate BEFORE touching the filesystem so invalid ids create nothing.
        let final_path = self.path_for(txid)?;
        self.ensure_private_directory()?;
        if std::fs::symlink_metadata(&final_path)
            .map(|meta| meta.file_type().is_symlink())
            .unwrap_or(false)
        {
            return Err(InboxError::Io("inbox target is a symlink".to_owned()));
        }
        // `NamedTempFile` performs exclusive random creation in the trusted
        // Inbox directory. Its RAII destructor removes payloads on every
        // error/panic path before the atomic persist.
        let mut tmp = Builder::new()
            .prefix(".inbox-")
            .tempfile_in(&self.dir)
            .map_err(|e| InboxError::Io(e.to_string()))?;
        tmp.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|e| InboxError::Io(e.to_string()))?;
        tmp.write_all(text.as_bytes())
            .map_err(|e| InboxError::Io(e.to_string()))?;
        tmp.as_file()
            .sync_all()
            .map_err(|e| InboxError::Io(e.to_string()))?;
        if !allow_commit() {
            return Err(InboxError::Io("inbox commit cancelled".to_owned()));
        }
        tmp.persist(&final_path)
            .map_err(|e| InboxError::Io(e.error.to_string()))?;

        if let Ok(dir) = File::open(&self.dir) {
            let _ = dir.sync_all(); // best-effort durability for the rename
        }
        Ok(final_path)
    }

    /// Read back a stored payload verbatim.
    pub fn read_text(&self, txid: &TransactionId) -> Result<String, InboxError> {
        let p = self.path_for(txid)?;
        std::fs::read_to_string(p).map_err(|e| InboxError::Io(e.to_string()))
    }

    /// Content-free listing of stored artifacts (paths only, sorted).
    pub fn list(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&self.dir) {
            for entry in rd.flatten() {
                let p = entry.path();
                if p.extension().and_then(|e| e.to_str()) == Some("txt") {
                    out.push(p);
                }
            }
        }
        out.sort();
        out
    }

    /// Remove recovered-text files older than `max_age` (F-M4): the Inbox
    /// keeps undelivered text only as long as the recovery journal does;
    /// History is the durable copy. Only regular `<txid>.txt` files are
    /// touched, never through a symlink. Returns how many were removed.
    pub fn prune_older_than(
        &self,
        max_age: std::time::Duration,
        now: std::time::SystemTime,
    ) -> usize {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return 0;
        };
        let mut removed = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("txt") {
                continue;
            }
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if !metadata.file_type().is_file() {
                continue;
            }
            let expired = metadata
                .modified()
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .is_some_and(|age| age > max_age);
            if expired && std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
        removed
    }
}

/// Reject a symlinked/non-directory Inbox root and enforce private ownership
/// mode before placing transcript bytes below it.
fn ensure_private_dir(dir: &std::path::Path) -> Result<(), InboxError> {
    match std::fs::symlink_metadata(dir) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
            return Err(InboxError::Io("inbox directory is unsafe".to_owned()));
        }
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(dir).map_err(|e| InboxError::Io(e.to_string()))?;
            let meta = std::fs::symlink_metadata(dir).map_err(|e| InboxError::Io(e.to_string()))?;
            if meta.file_type().is_symlink() || !meta.is_dir() {
                return Err(InboxError::Io("inbox directory is unsafe".to_owned()));
            }
        }
        Err(err) => return Err(InboxError::Io(err.to_string())),
    }
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| InboxError::Io(e.to_string()))
}

#[cfg(test)]
mod prune_tests {
    use super::InboxStore;
    use std::time::{Duration, SystemTime};

    #[test]
    fn only_old_recovered_text_files_are_removed() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let old = dir.path().join("tx-old.txt");
        let fresh = dir.path().join("tx-fresh.txt");
        let other = dir.path().join("notes.md");
        for path in [&old, &fresh, &other] {
            std::fs::write(path, "text")?;
        }
        let an_hour_ago = SystemTime::now() - Duration::from_secs(3600);
        for path in [&old, &other] {
            std::fs::File::options()
                .write(true)
                .open(path)?
                .set_modified(an_hour_ago)?;
        }
        let target = dir.path().join("target.txt");
        std::fs::write(&target, "outside")?;
        std::fs::File::options()
            .write(true)
            .open(&target)?
            .set_modified(an_hour_ago)?;
        let link = dir.path().join("tx-link.txt");
        std::os::unix::fs::symlink(&target, &link)?;

        let store = InboxStore::new(dir.path().to_path_buf());
        let removed = store.prune_older_than(Duration::from_secs(15 * 60), SystemTime::now());
        // The old file and the old `target.txt` go; the fresh file, the
        // non-.txt file and the symlink stay.
        assert_eq!(removed, 2);
        assert!(!old.exists());
        assert!(fresh.exists());
        assert!(other.exists());
        assert!(std::fs::symlink_metadata(&link).is_ok());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn inbox_private_modes_cancel_cleanup_and_symlinks_fail_closed()
    -> Result<(), Box<dyn std::error::Error>> {
        fn test_io<T, E: std::fmt::Display>(result: Result<T, E>) -> Result<T, std::io::Error> {
            result.map_err(|error| std::io::Error::other(error.to_string()))
        }

        let outer = tempfile::tempdir()?;
        let root = outer.path().join("inbox");
        let store = InboxStore::new(root.clone());
        let txid = TransactionId::new("tx-private");
        let final_path = test_io(store.store_text(&txid, "controlled payload"))?;
        assert_eq!(
            std::fs::metadata(&root)?.permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&final_path)?.permissions().mode() & 0o777,
            0o600
        );
        let entries = std::fs::read_dir(&root)?.collect::<Result<Vec<_>, _>>()?;
        assert!(
            entries
                .iter()
                .all(|entry| !entry.file_name().to_string_lossy().starts_with(".inbox-")),
            "successful persistence left an inbox temporary file"
        );

        let cancelled = TransactionId::new("tx-cancelled");
        assert!(
            store
                .store_text_if(&cancelled, "must-not-persist", || false)
                .is_err()
        );
        assert!(!root.join("tx-cancelled.txt").exists());
        let entries = std::fs::read_dir(&root)?.collect::<Result<Vec<_>, _>>()?;
        assert!(
            entries
                .iter()
                .all(|entry| !entry.file_name().to_string_lossy().starts_with(".inbox-")),
            "cancelled persistence left sensitive temporary text"
        );

        let outside = outer.path().join("outside.txt");
        std::fs::write(&outside, b"do-not-replace")?;
        symlink(&outside, root.join("tx-link.txt"))?;
        assert!(
            store
                .store_text(&TransactionId::new("tx-link"), "replacement")
                .is_err()
        );
        assert_eq!(std::fs::read(&outside)?, b"do-not-replace");

        let linked_root = outer.path().join("linked-inbox");
        symlink(&root, &linked_root)?;
        assert!(
            InboxStore::new(linked_root)
                .store_text(&TransactionId::new("tx-other"), "blocked")
                .is_err()
        );
        Ok(())
    }
}
