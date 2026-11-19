//! Secure Windows filesystem adapter for Narrivy's recovery journal.
//!
//! The Windows counterpart of `narrivy-journal-unix`, with the same public
//! API shape and retention behaviour. Unix modes become security
//! descriptors: the journal directory carries a protected DACL for the
//! current user and SYSTEM only (the analogue of 0700), entries inherit it
//! (0600), and owners are compared by SID. Symlinks and junctions are
//! reparse points and fail closed. Windows cannot flush a directory, so each
//! rename uses `MoveFileExW(MOVEFILE_WRITE_THROUGH)` instead of a directory
//! fsync.
//!
//! Unsafe policy: FFI lives in `security.rs` and [`move_file`]; each call is
//! minimal and carries a `SAFETY` note.

#![cfg(windows)]

mod security;

use std::fs::File;
use std::io::{Read, Write};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use narrivy_journal_api::{
    JournalEntry, JournalEntryState, JournalError, MAX_JOURNAL_ENTRY_BYTES, RecoveryJournal,
    retention_victims, retention_victims_preserving, valid_txid,
};
use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, ERROR_FILE_EXISTS};
use windows_sys::Win32::Storage::FileSystem::{
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
};

use security::{ANY_RIGHTS, Principals, REPLACE_RIGHTS, WRITE_RIGHTS};

const MAX_QUARANTINE_COLLISIONS: usize = 1_000;
const MAX_DIRECTORY_RECORDS: usize = 1_000;
const MAX_DIRECTORY_BYTES: usize = MAX_JOURNAL_ENTRY_BYTES * 128;
const MAX_QUARANTINE_FILES: usize = 16;
const MAX_QUARANTINE_BYTES: usize = MAX_JOURNAL_ENTRY_BYTES * MAX_QUARANTINE_FILES;

/// File-per-transaction recovery journal for one Windows user.
pub struct WindowsJournal {
    dir: PathBuf,
    principals: Principals,
    mutation: Mutex<()>,
    #[cfg(test)]
    fail_directory_sync: AtomicBool,
}

/// Production-facing recovery journal that applies TTL/count retention at
/// every access boundary, exactly like `RetainedUnixJournal`.
pub struct RetainedWindowsJournal {
    inner: WindowsJournal,
    operation: Mutex<()>,
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl RetainedWindowsJournal {
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self, JournalError> {
        Self::open_with_clock(dir, Arc::new(unix_now_ms))
    }

    fn open_with_clock(
        dir: impl Into<PathBuf>,
        clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    ) -> Result<Self, JournalError> {
        let journal = Self {
            inner: WindowsJournal::open(dir)?,
            operation: Mutex::new(()),
            clock,
        };
        journal.enforce_limits()?;
        Ok(journal)
    }

    pub fn dir(&self) -> &Path {
        self.inner.dir()
    }

    fn lock_operation(&self) -> Result<MutexGuard<'_, ()>, JournalError> {
        self.operation
            .lock()
            .map_err(|_| JournalError::Storage("journal retention lock poisoned".to_owned()))
    }

    fn now(&self) -> u64 {
        (self.clock)()
    }

    fn enforce_now(&self) -> Result<usize, JournalError> {
        self.inner.enforce_limits_at(self.now())
    }

    fn enforce_after_new_write(&self, txid: &str) -> Result<(), JournalError> {
        let now = self.now();
        let entries = self.inner.entries()?;
        for victim in retention_victims_preserving(&entries, now, Some(txid)) {
            self.inner.remove(&victim)?;
        }
        Ok(())
    }
}

impl WindowsJournal {
    /// Open a journal below an existing parent directory owned by the current
    /// user that no other principal may write.
    ///
    /// The journal directory itself always carries a protected DACL for the
    /// current user and SYSTEM only. Processes running as the same user
    /// remain the same security principal.
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self, JournalError> {
        let dir = dir.into();
        let principals = Principals::current().map_err(storage_error)?;
        ensure_private_dir(&dir, &principals)?;
        let journal = Self {
            dir,
            principals,
            mutation: Mutex::new(()),
            #[cfg(test)]
            fail_directory_sync: AtomicBool::new(false),
        };
        journal.cleanup_startup_artifacts_unlocked()?;
        Ok(journal)
    }

    /// Content-free location for diagnostics and qualification only.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn lock(&self) -> Result<MutexGuard<'_, ()>, JournalError> {
        self.mutation
            .lock()
            .map_err(|_| JournalError::Storage("journal mutation lock poisoned".to_owned()))
    }

    fn path_for(&self, txid: &str) -> Result<PathBuf, JournalError> {
        valid_txid(txid)?;
        Ok(self.dir.join(format!("{txid}.json")))
    }

    fn write_unlocked(&self, entry: &JournalEntry) -> Result<(), JournalError> {
        self.write_unlocked_with_mode(entry, true)
    }

    fn write_new_unlocked(&self, entry: &JournalEntry) -> Result<(), JournalError> {
        self.write_unlocked_with_mode(entry, false)
    }

    fn write_unlocked_with_mode(
        &self,
        entry: &JournalEntry,
        replace_existing: bool,
    ) -> Result<(), JournalError> {
        let path = self.path_for(&entry.txid)?;
        validate_private_dir_path(&self.dir, &self.principals)?;
        self.validate_existing_entry_path(&path)?;

        let mut temporary = tempfile::Builder::new()
            .prefix(".journal-")
            .tempfile_in(&self.dir)
            .map_err(storage_error)?;
        {
            let mut bounded = BoundedWriter::new(temporary.as_file_mut(), MAX_JOURNAL_ENTRY_BYTES);
            if let Err(error) = serde_json::to_writer_pretty(&mut bounded, entry) {
                if bounded.exceeded() {
                    return Err(JournalError::EntryTooLarge);
                }
                return Err(storage_error(error));
            }
            bounded.flush().map_err(storage_error)?;
        }
        temporary.as_file().sync_all().map_err(storage_error)?;
        // Close the handle before the rename; the TempPath still deletes the
        // temporary file if the move fails.
        let temporary = temporary.into_temp_path();
        match move_file(&temporary, &path, replace_existing) {
            Ok(()) => {
                // The file now lives at `path`; nothing is left to delete.
                let _kept = temporary.keep();
            }
            Err(error)
                if !replace_existing
                    && matches!(
                        error.raw_os_error(),
                        Some(code) if code == ERROR_ALREADY_EXISTS as i32
                            || code == ERROR_FILE_EXISTS as i32
                    ) =>
            {
                return Err(JournalError::DuplicateTransactionId);
            }
            Err(error) => return Err(storage_error(error)),
        }
        self.validate_existing_entry_path(&path)?;
        self.sync_directory()
    }

    fn read_unlocked(&self, txid: &str) -> Result<Option<JournalEntry>, JournalError> {
        validate_private_dir_path(&self.dir, &self.principals)?;
        let path = self.path_for(txid)?;
        let mut file = match open_entry_no_follow(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(storage_error(error)),
        };
        self.validate_open_entry(&file)?;
        let mut data = Vec::new();
        Read::by_ref(&mut file)
            .take((MAX_JOURNAL_ENTRY_BYTES + 1) as u64)
            .read_to_end(&mut data)
            .map_err(storage_error)?;
        if data.len() > MAX_JOURNAL_ENTRY_BYTES {
            return Err(JournalError::EntryTooLarge);
        }
        match serde_json::from_slice::<JournalEntry>(&data) {
            Ok(entry) if entry.txid == txid => Ok(Some(entry)),
            Ok(_) | Err(_) => {
                drop(file);
                self.quarantine_unlocked(txid, &path)?;
                Ok(None)
            }
        }
    }

    fn quarantine_unlocked(&self, txid: &str, source: &Path) -> Result<(), JournalError> {
        self.validate_existing_entry_path(source)?;
        for suffix in 0..MAX_QUARANTINE_COLLISIONS {
            let name = if suffix == 0 {
                format!("{txid}.corrupt")
            } else {
                format!("{txid}.corrupt-{suffix}")
            };
            let destination = self.dir.join(name);
            if std::fs::symlink_metadata(&destination).is_ok() {
                continue;
            }
            match move_file(source, &destination, false) {
                Ok(()) => {
                    self.sync_directory()?;
                    self.enforce_quarantine_limits_unlocked()?;
                    return Ok(());
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(storage_error(error)),
            }
        }
        Err(JournalError::Storage(
            "journal quarantine namespace exhausted".to_owned(),
        ))
    }

    fn entries_unlocked(&self) -> Result<Vec<JournalEntry>, JournalError> {
        validate_private_dir_path(&self.dir, &self.principals)?;
        let mut txids = Vec::new();
        let mut scanned = 0usize;
        for entry in std::fs::read_dir(&self.dir).map_err(storage_error)? {
            let entry = entry.map_err(storage_error)?;
            scanned += 1;
            if scanned > MAX_DIRECTORY_RECORDS {
                return Err(JournalError::UnsafeStorage(
                    "journal directory contains too many records",
                ));
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if name.starts_with('.') || !name.ends_with(".json") {
                continue;
            }
            let txid = name.trim_end_matches(".json");
            if valid_txid(txid).is_err() {
                continue;
            }
            txids.push(txid.to_owned());
        }
        txids.sort();

        let mut entries = Vec::new();
        let mut total_bytes = 0usize;
        for txid in txids {
            let path = self.path_for(&txid)?;
            let length = self.validate_entry_path(&path)?;
            let entry_bytes = usize::try_from(length).map_err(|_| JournalError::EntryTooLarge)?;
            total_bytes = total_bytes
                .checked_add(entry_bytes)
                .ok_or(JournalError::EntryTooLarge)?;
            if total_bytes > MAX_DIRECTORY_BYTES {
                return Err(JournalError::UnsafeStorage(
                    "journal directory content exceeds bounded scan limit",
                ));
            }
            if let Some(entry) = self.read_unlocked(&txid)? {
                entries.push(entry);
            }
        }
        entries.sort_by_key(|entry| entry.created_unix_ms);
        Ok(entries)
    }

    fn remove_unlocked(&self, txid: &str) -> Result<bool, JournalError> {
        validate_private_dir_path(&self.dir, &self.principals)?;
        let path = self.path_for(txid)?;
        match std::fs::symlink_metadata(&path) {
            Ok(_) => self.validate_existing_entry_path(&path)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(storage_error(error)),
        }
        std::fs::remove_file(&path).map_err(storage_error)?;
        self.sync_directory()?;
        Ok(true)
    }

    /// Windows has no directory fsync; renames are write-through instead.
    /// The test hook keeps the Unix adapter's failure-injection contract.
    fn sync_directory(&self) -> Result<(), JournalError> {
        #[cfg(test)]
        if self.fail_directory_sync.swap(false, Ordering::SeqCst) {
            return Err(JournalError::Storage(
                "controlled directory sync failure".to_owned(),
            ));
        }
        Ok(())
    }

    fn cleanup_startup_artifacts_unlocked(&self) -> Result<(), JournalError> {
        validate_private_dir_path(&self.dir, &self.principals)?;
        let mut removed = false;
        let mut scanned = 0usize;
        for entry in std::fs::read_dir(&self.dir).map_err(storage_error)? {
            let entry = entry.map_err(storage_error)?;
            scanned += 1;
            if scanned > MAX_DIRECTORY_RECORDS {
                return Err(JournalError::UnsafeStorage(
                    "journal directory contains too many records",
                ));
            }
            if !entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(".journal-"))
            {
                continue;
            }
            let path = entry.path();
            self.validate_entry_path(&path)?;
            std::fs::remove_file(path).map_err(storage_error)?;
            removed = true;
        }
        if removed {
            self.sync_directory()?;
        }
        self.enforce_quarantine_limits_unlocked()
    }

    fn enforce_quarantine_limits_unlocked(&self) -> Result<(), JournalError> {
        validate_private_dir_path(&self.dir, &self.principals)?;
        let mut quarantines = Vec::new();
        let mut scanned = 0usize;
        for entry in std::fs::read_dir(&self.dir).map_err(storage_error)? {
            let entry = entry.map_err(storage_error)?;
            scanned += 1;
            if scanned > MAX_DIRECTORY_RECORDS {
                return Err(JournalError::UnsafeStorage(
                    "journal directory contains too many records",
                ));
            }
            let name = entry.file_name();
            let Some(text) = name.to_str() else {
                continue;
            };
            if !is_quarantine_name(text) {
                continue;
            }
            let path = entry.path();
            let length = self.validate_entry_path(&path)?;
            let length = usize::try_from(length).map_err(|_| JournalError::EntryTooLarge)?;
            let modified = std::fs::symlink_metadata(&path)
                .and_then(|metadata| metadata.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            quarantines.push((modified, text.to_owned(), path, length));
        }
        quarantines.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
        let mut total_bytes = quarantines.iter().try_fold(0usize, |total, item| {
            total.checked_add(item.3).ok_or(JournalError::EntryTooLarge)
        })?;
        let mut remove_count = quarantines.len().saturating_sub(MAX_QUARANTINE_FILES);
        let mut removed = false;
        for (_, _, path, length) in quarantines {
            if remove_count == 0 && total_bytes <= MAX_QUARANTINE_BYTES {
                break;
            }
            std::fs::remove_file(path).map_err(storage_error)?;
            total_bytes = total_bytes.saturating_sub(length);
            remove_count = remove_count.saturating_sub(1);
            removed = true;
        }
        if removed {
            self.sync_directory()?;
        }
        Ok(())
    }

    fn validate_existing_entry_path(&self, path: &Path) -> Result<(), JournalError> {
        match std::fs::symlink_metadata(path) {
            Ok(_) => self.validate_entry_path(path).map(|_| ()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(storage_error(error)),
        }
    }

    /// Validate an entry by opening it without following a reparse point.
    /// Returns its length.
    fn validate_entry_path(&self, path: &Path) -> Result<u64, JournalError> {
        let file = security::open_object(path).map_err(storage_error)?;
        self.validate_open_entry(&file)
    }

    fn validate_open_entry(&self, file: &File) -> Result<u64, JournalError> {
        let facts = security::facts(file).map_err(storage_error)?;
        if facts.is_reparse_point || facts.is_directory {
            return Err(JournalError::UnsafeStorage(
                "journal entry is not a regular file",
            ));
        }
        if !security::owned_by_current_user(file, &self.principals).map_err(storage_error)? {
            return Err(JournalError::UnsafeStorage("journal entry owner mismatch"));
        }
        if facts.links != 1 {
            return Err(JournalError::UnsafeStorage(
                "journal entry has multiple links",
            ));
        }
        if !security::only_trusted_hold(file, &self.principals, ANY_RIGHTS)
            .map_err(storage_error)?
        {
            return Err(JournalError::UnsafeStorage(
                "journal entry is accessible to another user",
            ));
        }
        if facts.size > MAX_JOURNAL_ENTRY_BYTES as u64 {
            return Err(JournalError::EntryTooLarge);
        }
        Ok(facts.size)
    }

    #[cfg(test)]
    fn fail_next_directory_sync(&self) {
        self.fail_directory_sync.store(true, Ordering::SeqCst);
    }
}

fn is_quarantine_name(name: &str) -> bool {
    let Some(marker) = name.rfind(".corrupt") else {
        return false;
    };
    let (txid, suffix) = name.split_at(marker);
    if valid_txid(txid).is_err() {
        return false;
    }
    let suffix = &suffix[".corrupt".len()..];
    suffix.is_empty()
        || suffix.strip_prefix('-').is_some_and(|digits| {
            !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
        })
}

impl RecoveryJournal for WindowsJournal {
    fn write(&self, entry: &JournalEntry) -> Result<(), JournalError> {
        let _guard = self.lock()?;
        self.write_unlocked(entry)
    }

    fn write_new(&self, entry: &JournalEntry) -> Result<(), JournalError> {
        let _guard = self.lock()?;
        self.write_new_unlocked(entry)
    }

    fn read(&self, txid: &str) -> Result<Option<JournalEntry>, JournalError> {
        let _guard = self.lock()?;
        self.read_unlocked(txid)
    }

    fn entries(&self) -> Result<Vec<JournalEntry>, JournalError> {
        let _guard = self.lock()?;
        self.entries_unlocked()
    }

    fn remove(&self, txid: &str) -> Result<bool, JournalError> {
        let _guard = self.lock()?;
        self.remove_unlocked(txid)
    }

    fn mark_state(
        &self,
        txid: &str,
        state: JournalEntryState,
    ) -> Result<JournalEntry, JournalError> {
        let _guard = self.lock()?;
        let mut entry = self
            .read_unlocked(txid)?
            .ok_or_else(|| JournalError::Storage("unknown transaction id".to_owned()))?;
        entry.state = state;
        entry.updated_unix_ms = unix_now_ms();
        self.write_unlocked(&entry)?;
        Ok(entry)
    }

    fn recoverable_entries_at(&self, now_unix_ms: u64) -> Result<Vec<JournalEntry>, JournalError> {
        let _guard = self.lock()?;
        let mut entries: Vec<_> = self
            .entries_unlocked()?
            .into_iter()
            .filter(|entry| entry.is_recoverable_at(now_unix_ms))
            .collect();
        entries.sort_by_key(|entry| entry.created_unix_ms);
        Ok(entries)
    }

    fn enforce_limits_at(&self, now_unix_ms: u64) -> Result<usize, JournalError> {
        let _guard = self.lock()?;
        let entries = self.entries_unlocked()?;
        let mut removed = 0;
        for txid in retention_victims(&entries, now_unix_ms) {
            if self.remove_unlocked(&txid)? {
                removed += 1;
            }
        }
        Ok(removed)
    }
}

impl RecoveryJournal for RetainedWindowsJournal {
    fn write(&self, entry: &JournalEntry) -> Result<(), JournalError> {
        let _guard = self.lock_operation()?;
        self.inner.write(entry)?;
        self.enforce_now().map(|_| ())
    }

    fn write_new(&self, entry: &JournalEntry) -> Result<(), JournalError> {
        let _guard = self.lock_operation()?;
        self.inner.write_new(entry)?;
        self.enforce_after_new_write(&entry.txid)
    }

    fn read(&self, txid: &str) -> Result<Option<JournalEntry>, JournalError> {
        let _guard = self.lock_operation()?;
        self.enforce_now()?;
        self.inner.read(txid)
    }

    fn entries(&self) -> Result<Vec<JournalEntry>, JournalError> {
        let _guard = self.lock_operation()?;
        self.enforce_now()?;
        self.inner.entries()
    }

    fn remove(&self, txid: &str) -> Result<bool, JournalError> {
        let _guard = self.lock_operation()?;
        self.inner.remove(txid)
    }

    fn mark_state(
        &self,
        txid: &str,
        state: JournalEntryState,
    ) -> Result<JournalEntry, JournalError> {
        let _guard = self.lock_operation()?;
        let entry = self.inner.mark_state(txid, state)?;
        self.enforce_now()?;
        Ok(entry)
    }

    fn recoverable_entries_at(&self, now_unix_ms: u64) -> Result<Vec<JournalEntry>, JournalError> {
        let _guard = self.lock_operation()?;
        self.inner.enforce_limits_at(now_unix_ms)?;
        self.inner.recoverable_entries_at(now_unix_ms)
    }

    fn enforce_limits_at(&self, now_unix_ms: u64) -> Result<usize, JournalError> {
        let _guard = self.lock_operation()?;
        self.inner.enforce_limits_at(now_unix_ms)
    }
}

struct BoundedWriter<W> {
    inner: W,
    remaining: usize,
    exceeded: bool,
}

impl<W> BoundedWriter<W> {
    fn new(inner: W, maximum: usize) -> Self {
        Self {
            inner,
            remaining: maximum,
            exceeded: false,
        }
    }

    fn exceeded(&self) -> bool {
        self.exceeded
    }
}

impl<W: Write> Write for BoundedWriter<W> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        if buffer.len() > self.remaining {
            self.exceeded = true;
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "journal entry exceeds size limit",
            ));
        }
        let written = self.inner.write(buffer)?;
        self.remaining = self.remaining.saturating_sub(written);
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn unix_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Atomically rename `from` to `to` with write-through, replacing an existing
/// destination only when asked.
fn move_file(from: &Path, to: &Path, replace_existing: bool) -> std::io::Result<()> {
    let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut flags = MOVEFILE_WRITE_THROUGH;
    if replace_existing {
        flags |= MOVEFILE_REPLACE_EXISTING;
    }
    // SAFETY: both buffers are NUL-terminated UTF-16 paths that outlive the
    // call; MoveFileExW only reads them.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), flags) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn ensure_private_dir(dir: &Path, principals: &Principals) -> Result<(), JournalError> {
    validate_private_parent(dir, principals)?;
    reject_reparse_components(dir)?;
    match std::fs::symlink_metadata(dir) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(dir).map_err(storage_error)?;
            reject_reparse_components(dir)?;
            let created = security::open_object(dir).map_err(storage_error)?;
            let facts = security::facts(&created).map_err(storage_error)?;
            if facts.is_reparse_point || !facts.is_directory {
                return Err(JournalError::UnsafeStorage(
                    "journal directory is not a directory",
                ));
            }
            if !security::owned_by_current_user(&created, principals).map_err(storage_error)? {
                return Err(JournalError::UnsafeStorage(
                    "journal directory owner mismatch",
                ));
            }
            security::protect_private_directory(dir, principals).map_err(storage_error)?;
        }
        Err(error) => return Err(storage_error(error)),
    }
    validate_directory(dir, principals)
}

fn validate_private_dir_path(dir: &Path, principals: &Principals) -> Result<(), JournalError> {
    validate_private_parent(dir, principals)?;
    reject_reparse_components(dir)?;
    validate_directory(dir, principals)
}

fn parent_of(dir: &Path) -> &Path {
    dir.parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn validate_private_parent(dir: &Path, principals: &Principals) -> Result<(), JournalError> {
    let parent = parent_of(dir);
    let file = security::open_object(parent).map_err(storage_error)?;
    let facts = security::facts(&file).map_err(storage_error)?;
    if facts.is_reparse_point || !facts.is_directory {
        return Err(JournalError::UnsafeStorage(
            "journal parent directory is unsafe",
        ));
    }
    if !security::owned_by_current_user(&file, principals).map_err(storage_error)? {
        return Err(JournalError::UnsafeStorage(
            "journal parent directory owner mismatch",
        ));
    }
    if !security::only_trusted_hold(&file, principals, WRITE_RIGHTS).map_err(storage_error)? {
        return Err(JournalError::UnsafeStorage(
            "journal parent directory is writable by another user",
        ));
    }
    for ancestor in parent
        .ancestors()
        .skip(1)
        .filter(|path| !path.as_os_str().is_empty())
    {
        let file = security::open_object(ancestor).map_err(storage_error)?;
        let facts = security::facts(&file).map_err(storage_error)?;
        if facts.is_reparse_point || !facts.is_directory {
            return Err(JournalError::UnsafeStorage(
                "journal ancestor directory is unsafe",
            ));
        }
        if !security::only_trusted_hold(&file, principals, REPLACE_RIGHTS).map_err(storage_error)? {
            return Err(JournalError::UnsafeStorage(
                "journal ancestor directory permits replacement",
            ));
        }
    }
    Ok(())
}

fn reject_reparse_components(path: &Path) -> Result<(), JournalError> {
    for component in path
        .ancestors()
        .filter(|component| !component.as_os_str().is_empty())
    {
        match std::fs::symlink_metadata(component) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(JournalError::UnsafeStorage(
                    "journal path contains a symlink component",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(storage_error(error)),
        }
    }
    Ok(())
}

fn validate_directory(dir: &Path, principals: &Principals) -> Result<(), JournalError> {
    let file = security::open_object(dir).map_err(storage_error)?;
    let facts = security::facts(&file).map_err(storage_error)?;
    if facts.is_reparse_point || !facts.is_directory {
        return Err(JournalError::UnsafeStorage("journal directory is unsafe"));
    }
    if !security::owned_by_current_user(&file, principals).map_err(storage_error)? {
        return Err(JournalError::UnsafeStorage(
            "journal directory owner mismatch",
        ));
    }
    if !security::only_trusted_hold(&file, principals, ANY_RIGHTS).map_err(storage_error)? {
        return Err(JournalError::UnsafeStorage(
            "journal directory is accessible to another user",
        ));
    }
    Ok(())
}

/// Open an entry for reading without following a reparse point; the caller
/// then rejects the handle if it names one.
fn open_entry_no_follow(path: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_OPEN_REPARSE_POINT, READ_CONTROL};

    std::fs::OpenOptions::new()
        .access_mode(windows_sys::Win32::Foundation::GENERIC_READ | READ_CONTROL)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

fn storage_error(error: impl std::fmt::Display) -> JournalError {
    JournalError::Storage(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn journal_fixture()
    -> Result<(tempfile::TempDir, PathBuf, WindowsJournal), Box<dyn std::error::Error>> {
        let outer = tempfile::tempdir()?;
        let root = outer.path().join("journal");
        let journal = WindowsJournal::open(&root)?;
        Ok((outer, root, journal))
    }

    #[test]
    fn private_atomic_round_trip_and_restart() -> TestResult {
        let (_outer, root, journal) = journal_fixture()?;
        let mut entry = JournalEntry::new_pending("tx-roundtrip", 1);
        entry.variants.corrected = Some("private payload".to_owned());
        journal.write(&entry)?;
        assert!(std::fs::read_dir(&root)?.all(|entry| {
            !entry
                .map(|value| value.file_name().to_string_lossy().starts_with(".journal-"))
                .unwrap_or(false)
        }));
        drop(journal);
        let reopened = WindowsJournal::open(&root)?;
        assert_eq!(reopened.read("tx-roundtrip")?, Some(entry));
        Ok(())
    }

    #[test]
    fn atomic_new_write_never_replaces_an_existing_transaction() -> TestResult {
        let (_outer, _root, journal) = journal_fixture()?;
        let original = JournalEntry::new_pending("tx-no-clobber", 1);
        journal.write_new(&original)?;
        let replacement = JournalEntry::new_pending("tx-no-clobber", 2);
        assert_eq!(
            journal.write_new(&replacement),
            Err(JournalError::DuplicateTransactionId)
        );
        assert_eq!(journal.read("tx-no-clobber")?, Some(original));
        Ok(())
    }

    #[test]
    fn corrupt_entry_is_quarantined_not_served() -> TestResult {
        let (_outer, root, journal) = journal_fixture()?;
        journal.write(&JournalEntry::new_pending("tx-corrupt", 1))?;
        std::fs::write(root.join("tx-corrupt.json"), b"{not json")?;
        assert_eq!(journal.read("tx-corrupt")?, None);
        assert!(root.join("tx-corrupt.corrupt").exists());
        Ok(())
    }

    #[test]
    fn failed_directory_sync_is_reported() -> TestResult {
        let (_outer, _root, journal) = journal_fixture()?;
        journal.fail_next_directory_sync();
        assert!(
            journal
                .write(&JournalEntry::new_pending("tx-sync", 1))
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn quarantine_names_are_strict() {
        assert!(is_quarantine_name("tx-1.corrupt"));
        assert!(is_quarantine_name("tx-1.corrupt-3"));
        assert!(!is_quarantine_name("tx-1.corrupt-"));
        assert!(!is_quarantine_name("tx-1.json"));
    }
}
