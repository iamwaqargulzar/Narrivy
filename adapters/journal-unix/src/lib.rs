//! Secure Unix filesystem adapter for Narrivy's recovery journal.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use narrivy_journal_api::{
    JournalEntry, JournalEntryState, JournalError, MAX_JOURNAL_ENTRY_BYTES, RecoveryJournal,
    retention_victims, retention_victims_preserving, valid_txid,
};

const PRIVATE_DIR_MODE: u32 = 0o700;
const PRIVATE_FILE_MODE: u32 = 0o600;
const MAX_QUARANTINE_COLLISIONS: usize = 1_000;
const MAX_DIRECTORY_RECORDS: usize = 1_000;
const MAX_DIRECTORY_BYTES: usize = MAX_JOURNAL_ENTRY_BYTES * 128;
const MAX_QUARANTINE_FILES: usize = 16;
const MAX_QUARANTINE_BYTES: usize = MAX_JOURNAL_ENTRY_BYTES * MAX_QUARANTINE_FILES;

/// File-per-transaction recovery journal for one Unix user.
pub struct UnixJournal {
    dir: PathBuf,
    mutation: Mutex<()>,
    #[cfg(test)]
    fail_directory_sync: AtomicBool,
}

/// Production-facing recovery journal that applies TTL/count retention at
/// every access boundary. Keeping this policy decorator separate leaves the
/// raw adapter available to deterministic migrations and synthetic-time
/// tests without silently deleting their fixtures using the wall clock.
pub struct RetainedUnixJournal {
    inner: UnixJournal,
    operation: Mutex<()>,
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl RetainedUnixJournal {
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self, JournalError> {
        Self::open_with_clock(dir, Arc::new(unix_now_ms))
    }

    fn open_with_clock(
        dir: impl Into<PathBuf>,
        clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    ) -> Result<Self, JournalError> {
        let journal = Self {
            inner: UnixJournal::open(dir)?,
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

impl UnixJournal {
    /// Open a journal below an existing same-user parent directory that is not
    /// writable by group or other users.
    ///
    /// The journal itself is always exactly 0700. Denying group/other writes on
    /// its parent prevents a different Unix user from replacing it between
    /// operations.
    /// Processes running as the same UID remain the same security principal.
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self, JournalError> {
        let dir = dir.into();
        ensure_private_dir(&dir)?;
        let journal = Self {
            dir,
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
        validate_private_dir_path(&self.dir)?;
        validate_existing_entry_path(&path)?;

        let mut temporary = tempfile::Builder::new()
            .prefix(".journal-")
            .tempfile_in(&self.dir)
            .map_err(storage_error)?;
        temporary
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(PRIVATE_FILE_MODE))
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
        let persisted = if replace_existing {
            temporary.persist(&path)
        } else {
            temporary.persist_noclobber(&path)
        };
        match persisted {
            Ok(file) => drop(file),
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(JournalError::DuplicateTransactionId);
            }
            Err(error) => return Err(storage_error(error.error)),
        }
        validate_existing_entry_path(&path)?;
        self.sync_directory()
    }

    fn read_unlocked(&self, txid: &str) -> Result<Option<JournalEntry>, JournalError> {
        validate_private_dir_path(&self.dir)?;
        let path = self.path_for(txid)?;
        let mut file = match open_entry_no_follow(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) if error.raw_os_error() == Some(libc::ELOOP) => {
                return Err(JournalError::UnsafeStorage("journal entry is a symlink"));
            }
            Err(error) => return Err(storage_error(error)),
        };
        validate_open_entry(&file)?;
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
        validate_existing_entry_path(source)?;
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
            match std::fs::rename(source, &destination) {
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
        validate_private_dir_path(&self.dir)?;
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
            let metadata = std::fs::symlink_metadata(&path).map_err(storage_error)?;
            validate_entry_metadata(&metadata)?;
            let entry_bytes =
                usize::try_from(metadata.len()).map_err(|_| JournalError::EntryTooLarge)?;
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
        validate_private_dir_path(&self.dir)?;
        let path = self.path_for(txid)?;
        match std::fs::symlink_metadata(&path) {
            Ok(_) => validate_existing_entry_path(&path)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(storage_error(error)),
        }
        std::fs::remove_file(&path).map_err(storage_error)?;
        self.sync_directory()?;
        Ok(true)
    }

    fn sync_directory(&self) -> Result<(), JournalError> {
        #[cfg(test)]
        if self.fail_directory_sync.swap(false, Ordering::SeqCst) {
            return Err(JournalError::Storage(
                "controlled directory sync failure".to_owned(),
            ));
        }
        sync_directory(&self.dir)
    }

    fn cleanup_startup_artifacts_unlocked(&self) -> Result<(), JournalError> {
        validate_private_dir_path(&self.dir)?;
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
            if !entry.file_name().as_bytes().starts_with(b".journal-") {
                continue;
            }
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path).map_err(storage_error)?;
            validate_entry_metadata(&metadata)?;
            std::fs::remove_file(path).map_err(storage_error)?;
            removed = true;
        }
        if removed {
            self.sync_directory()?;
        }
        self.enforce_quarantine_limits_unlocked()
    }

    fn enforce_quarantine_limits_unlocked(&self) -> Result<(), JournalError> {
        validate_private_dir_path(&self.dir)?;
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
            if !is_quarantine_name(name.as_bytes()) {
                continue;
            }
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path).map_err(storage_error)?;
            validate_entry_metadata(&metadata)?;
            let length =
                usize::try_from(metadata.len()).map_err(|_| JournalError::EntryTooLarge)?;
            let modified = metadata.modified().unwrap_or(std::time::UNIX_EPOCH);
            quarantines.push((modified, name, path, length));
        }
        quarantines.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.as_bytes().cmp(right.1.as_bytes()))
        });
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

    #[cfg(test)]
    fn fail_next_directory_sync(&self) {
        self.fail_directory_sync.store(true, Ordering::SeqCst);
    }
}

fn is_quarantine_name(name: &[u8]) -> bool {
    let Ok(name) = std::str::from_utf8(name) else {
        return false;
    };
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

impl RecoveryJournal for UnixJournal {
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

impl RecoveryJournal for RetainedUnixJournal {
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

fn current_uid() -> u32 {
    // SAFETY: geteuid takes no arguments, cannot fail, and does not access
    // caller memory.
    unsafe { libc::geteuid() }
}

fn ensure_private_dir(dir: &Path) -> Result<(), JournalError> {
    validate_private_parent(dir)?;
    reject_symlink_components(dir)?;
    let parent = dir
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    match std::fs::symlink_metadata(dir) {
        Ok(metadata) => validate_directory_metadata(&metadata)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(dir).map_err(storage_error)?;
            reject_symlink_components(dir)?;
            let metadata = std::fs::symlink_metadata(dir).map_err(storage_error)?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(JournalError::UnsafeStorage(
                    "journal directory is not a directory",
                ));
            }
            if metadata.uid() != current_uid() {
                return Err(JournalError::UnsafeStorage(
                    "journal directory owner mismatch",
                ));
            }
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(PRIVATE_DIR_MODE))
                .map_err(storage_error)?;
            sync_directory(parent).map_err(storage_error)?;
        }
        Err(error) => return Err(storage_error(error)),
    }
    let metadata = std::fs::symlink_metadata(dir).map_err(storage_error)?;
    validate_directory_metadata(&metadata)
}

fn validate_private_dir_path(dir: &Path) -> Result<(), JournalError> {
    validate_private_parent(dir)?;
    reject_symlink_components(dir)?;
    let metadata = std::fs::symlink_metadata(dir).map_err(storage_error)?;
    validate_directory_metadata(&metadata)
}

fn validate_private_parent(dir: &Path) -> Result<(), JournalError> {
    let parent = dir
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let metadata = std::fs::symlink_metadata(parent).map_err(storage_error)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(JournalError::UnsafeStorage(
            "journal parent directory is unsafe",
        ));
    }
    if metadata.uid() != current_uid() {
        return Err(JournalError::UnsafeStorage(
            "journal parent directory owner mismatch",
        ));
    }
    if metadata.mode() & 0o022 != 0 {
        return Err(JournalError::UnsafeStorage(
            "journal parent directory is writable by another user",
        ));
    }
    for ancestor in parent.ancestors().skip(1) {
        let metadata = std::fs::symlink_metadata(ancestor).map_err(storage_error)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(JournalError::UnsafeStorage(
                "journal ancestor directory is unsafe",
            ));
        }
        let writable_by_others = metadata.mode() & 0o022 != 0;
        let sticky = metadata.mode() & 0o1000 != 0;
        if writable_by_others && !sticky {
            return Err(JournalError::UnsafeStorage(
                "journal ancestor directory permits replacement",
            ));
        }
    }
    Ok(())
}

fn reject_symlink_components(path: &Path) -> Result<(), JournalError> {
    for component in path.ancestors() {
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

fn validate_directory_metadata(metadata: &std::fs::Metadata) -> Result<(), JournalError> {
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(JournalError::UnsafeStorage("journal directory is unsafe"));
    }
    if metadata.uid() != current_uid() {
        return Err(JournalError::UnsafeStorage(
            "journal directory owner mismatch",
        ));
    }
    if metadata.mode() & 0o777 != PRIVATE_DIR_MODE {
        return Err(JournalError::UnsafeStorage(
            "journal directory mode is not 0700",
        ));
    }
    Ok(())
}

fn validate_existing_entry_path(path: &Path) -> Result<(), JournalError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(storage_error(error)),
    };
    validate_entry_metadata(&metadata)
}

fn validate_open_entry(file: &File) -> Result<(), JournalError> {
    validate_entry_metadata(&file.metadata().map_err(storage_error)?)
}

fn validate_entry_metadata(metadata: &std::fs::Metadata) -> Result<(), JournalError> {
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(JournalError::UnsafeStorage(
            "journal entry is not a regular file",
        ));
    }
    if metadata.uid() != current_uid() {
        return Err(JournalError::UnsafeStorage("journal entry owner mismatch"));
    }
    if metadata.nlink() != 1 {
        return Err(JournalError::UnsafeStorage(
            "journal entry has multiple links",
        ));
    }
    if metadata.mode() & 0o777 != PRIVATE_FILE_MODE {
        return Err(JournalError::UnsafeStorage(
            "journal entry mode is not 0600",
        ));
    }
    if metadata.len() > MAX_JOURNAL_ENTRY_BYTES as u64 {
        return Err(JournalError::EntryTooLarge);
    }
    Ok(())
}

fn open_entry_no_follow(path: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
}

fn sync_directory(dir: &Path) -> Result<(), JournalError> {
    File::open(dir)
        .and_then(|directory| directory.sync_all())
        .map_err(storage_error)
}

fn storage_error(error: impl std::fmt::Display) -> JournalError {
    JournalError::Storage(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use narrivy_journal_api::{JOURNAL_TTL_MS, JournalEntryState, MAX_JOURNAL_ENTRIES};
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::sync::atomic::AtomicU64;
    use std::sync::{Arc, Barrier};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn private_tempdir() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
        let outer = tempfile::tempdir()?;
        std::fs::set_permissions(outer.path(), std::fs::Permissions::from_mode(0o700))?;
        Ok(outer)
    }

    fn journal_fixture()
    -> Result<(tempfile::TempDir, PathBuf, UnixJournal), Box<dyn std::error::Error>> {
        let outer = private_tempdir()?;
        let root = outer.path().join("journal");
        let journal = UnixJournal::open(&root)?;
        Ok((outer, root, journal))
    }

    #[test]
    fn private_atomic_round_trip_and_restart() -> TestResult {
        let outer = private_tempdir()?;
        let root = outer.path().join("journal");
        let journal = UnixJournal::open(&root)?;
        let mut entry = JournalEntry::new_pending("tx-roundtrip", 1);
        entry.variants.corrected = Some("private payload".to_owned());
        journal.write(&entry)?;
        assert_eq!(
            std::fs::metadata(&root)?.permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(root.join("tx-roundtrip.json"))?
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert!(std::fs::read_dir(&root)?.all(|entry| {
            !entry
                .map(|value| value.file_name().to_string_lossy().starts_with(".journal-"))
                .unwrap_or(false)
        }));
        drop(journal);
        let reopened = UnixJournal::open(&root)?;
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
    fn directory_and_entry_symlinks_fail_closed() -> TestResult {
        let outer = private_tempdir()?;
        let real = outer.path().join("real");
        let linked = outer.path().join("linked");
        std::fs::create_dir(&real)?;
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o700))?;
        symlink(&real, &linked)?;
        assert!(UnixJournal::open(&linked).is_err());

        let journal = UnixJournal::open(&real)?;
        let outside = outer.path().join("outside");
        std::fs::write(&outside, b"unchanged")?;
        symlink(&outside, real.join("tx-link.json"))?;
        assert!(
            journal
                .write(&JournalEntry::new_pending("tx-link", 1))
                .is_err()
        );
        assert!(journal.read("tx-link").is_err());
        assert_eq!(std::fs::read(&outside)?, b"unchanged");
        Ok(())
    }

    #[test]
    fn ancestor_symlinks_and_insecure_existing_directories_fail_closed() -> TestResult {
        let outer = private_tempdir()?;
        let real_parent = outer.path().join("real-parent");
        std::fs::create_dir(&real_parent)?;
        std::fs::set_permissions(&real_parent, std::fs::Permissions::from_mode(0o700))?;
        let linked_parent = outer.path().join("linked-parent");
        symlink(&real_parent, &linked_parent)?;
        assert!(UnixJournal::open(linked_parent.join("journal")).is_err());
        assert!(!real_parent.join("journal").exists());

        let insecure = outer.path().join("insecure");
        std::fs::create_dir(&insecure)?;
        std::fs::set_permissions(&insecure, std::fs::Permissions::from_mode(0o750))?;
        assert!(UnixJournal::open(&insecure).is_err());
        assert_eq!(
            std::fs::metadata(&insecure)?.permissions().mode() & 0o777,
            0o750
        );

        let writable_ancestor = outer.path().join("writable-ancestor");
        std::fs::create_dir(&writable_ancestor)?;
        std::fs::set_permissions(&writable_ancestor, std::fs::Permissions::from_mode(0o777))?;
        let otherwise_private_parent = writable_ancestor.join("private-parent");
        std::fs::create_dir(&otherwise_private_parent)?;
        std::fs::set_permissions(
            &otherwise_private_parent,
            std::fs::Permissions::from_mode(0o700),
        )?;
        assert!(UnixJournal::open(otherwise_private_parent.join("journal")).is_err());
        Ok(())
    }

    #[test]
    fn post_open_directory_drift_fails_closed_for_every_operation() -> TestResult {
        let (_outer, root, journal) = journal_fixture()?;
        let entry = JournalEntry::new_pending("mode-drift", 1);
        journal.write(&entry)?;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o750))?;
        assert!(journal.read("mode-drift").is_err());
        assert!(journal.write(&entry).is_err());
        assert!(journal.entries().is_err());
        assert!(journal.remove("mode-drift").is_err());
        assert!(root.join("mode-drift.json").is_file());
        Ok(())
    }

    #[test]
    fn hard_links_wrong_modes_and_wrong_types_are_rejected() -> TestResult {
        let (_outer, root, journal) = journal_fixture()?;
        let entry = JournalEntry::new_pending("tx-hard", 1);
        journal.write(&entry)?;
        std::fs::hard_link(root.join("tx-hard.json"), root.join("second-link"))?;
        assert!(journal.read("tx-hard").is_err());
        assert!(journal.write(&entry).is_err());

        std::fs::create_dir(root.join("tx-dir.json"))?;
        assert!(journal.read("tx-dir").is_err());

        let loose = root.join("tx-loose.json");
        std::fs::write(
            &loose,
            serde_json::to_vec(&JournalEntry::new_pending("tx-loose", 1))?,
        )?;
        std::fs::set_permissions(&loose, std::fs::Permissions::from_mode(0o644))?;
        assert!(journal.read("tx-loose").is_err());
        Ok(())
    }

    #[test]
    fn malformed_and_identity_mismatched_json_are_quarantined() -> TestResult {
        let (_outer, root, journal) = journal_fixture()?;
        let malformed = root.join("bad.json");
        std::fs::write(&malformed, b"{bad json")?;
        std::fs::set_permissions(&malformed, std::fs::Permissions::from_mode(0o600))?;
        assert!(journal.read("bad")?.is_none());
        assert!(root.join("bad.corrupt").is_file());

        let mismatch = root.join("expected.json");
        std::fs::write(
            &mismatch,
            serde_json::to_vec(&JournalEntry::new_pending("different", 1))?,
        )?;
        std::fs::set_permissions(&mismatch, std::fs::Permissions::from_mode(0o600))?;
        assert!(journal.read("expected")?.is_none());
        assert!(root.join("expected.corrupt").is_file());

        let collision = root.join("collision.json");
        std::fs::write(&collision, b"{")?;
        std::fs::set_permissions(&collision, std::fs::Permissions::from_mode(0o600))?;
        let existing_quarantine = root.join("collision.corrupt");
        std::fs::write(&existing_quarantine, b"existing")?;
        std::fs::set_permissions(
            &existing_quarantine,
            std::fs::Permissions::from_mode(PRIVATE_FILE_MODE),
        )?;
        assert!(journal.read("collision")?.is_none());
        assert!(root.join("collision.corrupt-1").is_file());
        Ok(())
    }

    #[test]
    fn listings_ignore_temp_and_retention_removes_every_expired_state() -> TestResult {
        let (_outer, root, journal) = journal_fixture()?;
        let fresh = JournalEntry::new_pending("fresh", 100);
        journal.write(&fresh)?;
        let mut old = JournalEntry::new_pending("old", 0);
        old.state = JournalEntryState::InboxRecovered;
        journal.write(&old)?;
        std::fs::write(root.join(".journal-stray"), b"ignored")?;
        assert_eq!(journal.recoverable_entries_at(200)?, vec![fresh]);
        assert_eq!(journal.enforce_limits_at(JOURNAL_TTL_MS + 1)?, 1);
        assert!(journal.read("old")?.is_none());
        assert!(root.join(".journal-stray").exists());
        Ok(())
    }

    #[test]
    fn retention_enforces_a_strict_total_count_with_terminal_first() -> TestResult {
        let (_outer, _root, journal) = journal_fixture()?;
        let now = 100_000;
        for index in 0..MAX_JOURNAL_ENTRIES {
            journal.write(&JournalEntry::new_pending(
                format!("recoverable-{index:03}"),
                now - 1_000 + index as u64,
            ))?;
        }
        let mut terminal = JournalEntry::new_pending("terminal-oldest", now - 2_000);
        terminal.state = JournalEntryState::FailedTerminal;
        journal.write(&terminal)?;

        assert_eq!(journal.enforce_limits_at(now)?, 1);
        assert_eq!(journal.entries()?.len(), MAX_JOURNAL_ENTRIES);
        assert!(journal.read("terminal-oldest")?.is_none());
        assert!(journal.read("recoverable-000")?.is_some());
        Ok(())
    }

    #[test]
    fn retained_journal_enforces_cap_and_ttl_without_restart() -> TestResult {
        let outer = private_tempdir()?;
        let root = outer.path().join("journal");
        let now = Arc::new(AtomicU64::new(100_000));
        let clock = Arc::clone(&now);
        let journal = RetainedUnixJournal::open_with_clock(
            &root,
            Arc::new(move || clock.load(Ordering::SeqCst)),
        )?;
        for index in 0..MAX_JOURNAL_ENTRIES {
            journal.write_new(&JournalEntry::new_pending(
                format!("z-retained-{index:03}"),
                100_000,
            ))?;
        }
        journal.write_new(&JournalEntry::new_pending("a-new", 100_000))?;
        assert_eq!(journal.entries()?.len(), MAX_JOURNAL_ENTRIES);
        assert!(journal.read("a-new")?.is_some());
        assert!(journal.read("z-retained-000")?.is_none());

        now.store(100_000 + JOURNAL_TTL_MS, Ordering::SeqCst);
        assert!(journal.read("a-new")?.is_none());
        assert!(journal.entries()?.is_empty());
        Ok(())
    }

    #[test]
    fn retained_journal_serializes_concurrent_mutation_and_pruning() -> TestResult {
        let outer = private_tempdir()?;
        let journal = Arc::new(RetainedUnixJournal::open_with_clock(
            outer.path().join("journal"),
            Arc::new(|| 200_000),
        )?);
        let barrier = Arc::new(Barrier::new(9));
        let mut workers = Vec::new();
        for worker in 0..8 {
            let journal = Arc::clone(&journal);
            let barrier = Arc::clone(&barrier);
            workers.push(std::thread::spawn(move || -> Result<(), JournalError> {
                barrier.wait();
                for index in 0..20 {
                    journal.write_new(&JournalEntry::new_pending(
                        format!("concurrent-{worker:02}-{index:02}"),
                        200_000,
                    ))?;
                }
                Ok(())
            }));
        }
        barrier.wait();
        for worker in workers {
            worker
                .join()
                .map_err(|_| "retained journal worker panicked")??;
        }
        assert_eq!(journal.entries()?.len(), MAX_JOURNAL_ENTRIES);
        Ok(())
    }

    #[test]
    fn writes_are_size_bounded_and_failure_leaves_no_temporary_payload() -> TestResult {
        let (_outer, root, journal) = journal_fixture()?;
        let mut oversized = JournalEntry::new_pending("too-large", 1);
        oversized.variants.corrected = Some("x".repeat(MAX_JOURNAL_ENTRY_BYTES));
        assert_eq!(journal.write(&oversized), Err(JournalError::EntryTooLarge));
        assert!(!root.join("too-large.json").exists());
        assert!(std::fs::read_dir(&root)?.all(|entry| {
            !entry
                .map(|value| value.file_name().to_string_lossy().starts_with(".journal-"))
                .unwrap_or(false)
        }));

        let oversized_path = root.join("oversized-file.json");
        let file = File::create(&oversized_path)?;
        file.set_len(MAX_JOURNAL_ENTRY_BYTES as u64 + 1)?;
        std::fs::set_permissions(&oversized_path, std::fs::Permissions::from_mode(0o600))?;
        assert_eq!(
            journal.read("oversized-file"),
            Err(JournalError::EntryTooLarge)
        );
        Ok(())
    }

    #[test]
    fn replacement_is_atomic_and_directory_sync_failures_are_typed() -> TestResult {
        let (_outer, root, journal) = journal_fixture()?;
        let mut entry = JournalEntry::new_pending("replace", 1);
        entry.variants.corrected = Some("first".to_owned());
        journal.write(&entry)?;
        entry.variants.corrected = Some("second".to_owned());
        journal.write(&entry)?;
        assert_eq!(journal.read("replace")?, Some(entry));

        journal.fail_next_directory_sync();
        let error = match journal.write(&JournalEntry::new_pending("sync-uncertain", 2)) {
            Ok(()) => return Err("directory sync failure was not propagated".into()),
            Err(error) => error,
        };
        assert!(matches!(error, JournalError::Storage(_)));
        assert!(std::fs::read_dir(&root)?.all(|entry| {
            !entry
                .map(|value| value.file_name().to_string_lossy().starts_with(".journal-"))
                .unwrap_or(false)
        }));
        Ok(())
    }

    #[test]
    fn concurrent_read_write_is_serialized_and_restart_safe() -> TestResult {
        let outer = private_tempdir()?;
        let root = outer.path().join("journal");
        let journal = Arc::new(UnixJournal::open(&root)?);
        let barrier = Arc::new(Barrier::new(9));
        let mut threads = Vec::new();
        for worker in 0..8 {
            let journal = Arc::clone(&journal);
            let barrier = Arc::clone(&barrier);
            threads.push(std::thread::spawn(move || -> Result<(), JournalError> {
                barrier.wait();
                for item in 0..16 {
                    let txid = format!("worker-{worker}-{item}");
                    let entry = JournalEntry::new_pending(&txid, (worker * 16 + item) as u64);
                    journal.write(&entry)?;
                    if journal.read(&txid)? != Some(entry) {
                        return Err(JournalError::Storage(
                            "concurrent round trip mismatch".to_owned(),
                        ));
                    }
                }
                Ok(())
            }));
        }
        barrier.wait();
        for thread in threads {
            thread
                .join()
                .map_err(|_| "journal test thread panicked")??;
        }
        assert_eq!(journal.entries()?.len(), 128);
        assert_eq!(journal.enforce_limits_at(127)?, 28);
        assert_eq!(journal.entries()?.len(), MAX_JOURNAL_ENTRIES);
        drop(journal);
        assert_eq!(
            UnixJournal::open(&root)?.entries()?.len(),
            MAX_JOURNAL_ENTRIES
        );
        Ok(())
    }

    #[test]
    fn hundred_entry_torn_write_recovery_remains_content_correct() -> TestResult {
        let (_outer, root, journal) = journal_fixture()?;
        let base_ms = 1_000_000_u64;
        for index in 0..100 {
            let txid = format!("tx-{index:03}");
            let mut entry = JournalEntry::new_pending(&txid, base_ms + index * 100);
            entry.pipeline_id = Some("default".to_owned());
            entry.variants.raw = Some(format!("raw fixture {index}"));
            entry.variants.corrected = Some(format!("corrected fixture {index}"));
            if index % 3 == 0 {
                entry.state = JournalEntryState::Committed;
            }
            journal.write(&entry)?;
        }
        assert_eq!(journal.recoverable_entries_at(base_ms + 20_000)?.len(), 100);

        let torn_half = [3, 17, 42, 71, 88];
        let torn_empty = [5, 19, 44, 73, 90];
        let torn_one = [7, 21, 46, 75, 92];
        for index in torn_half {
            let path = root.join(format!("tx-{index:03}.json"));
            let data = std::fs::read(&path)?;
            std::fs::write(path, &data[..data.len() / 2])?;
        }
        for index in torn_empty {
            std::fs::write(root.join(format!("tx-{index:03}.json")), b"")?;
        }
        for index in torn_one {
            std::fs::write(root.join(format!("tx-{index:03}.json")), b"{")?;
        }
        std::fs::write(root.join(".interrupted.tmp"), b"partial")?;

        let recovered = journal.recoverable_entries_at(base_ms + 20_000)?;
        assert_eq!(recovered.len(), 85);
        assert!(
            recovered
                .windows(2)
                .all(|pair| pair[0].created_unix_ms <= pair[1].created_unix_ms)
        );
        assert_eq!(
            recovered
                .iter()
                .find(|entry| entry.txid == "tx-010")
                .and_then(|entry| entry.variants.raw.as_deref()),
            Some("raw fixture 10")
        );
        for index in torn_half.into_iter().chain(torn_empty).chain(torn_one) {
            assert!(root.join(format!("tx-{index:03}.corrupt")).is_file());
        }
        drop(journal);
        assert_eq!(
            UnixJournal::open(&root)?.recoverable_entries_at(base_ms + 20_000)?,
            recovered
        );
        Ok(())
    }

    #[test]
    fn unrelated_json_names_are_ignored() -> TestResult {
        let (_outer, root, journal) = journal_fixture()?;
        std::fs::write(root.join("not valid!.json"), b"unrelated")?;
        assert!(journal.entries()?.is_empty());
        Ok(())
    }

    #[test]
    fn restart_reaps_private_orphans_and_rejects_malicious_lookalikes() -> TestResult {
        let outer = private_tempdir()?;
        let root = outer.path().join("journal");
        let journal = UnixJournal::open(&root)?;
        drop(journal);

        let orphan = root.join(".journal-orphan");
        std::fs::write(&orphan, b"private interrupted payload")?;
        std::fs::set_permissions(&orphan, std::fs::Permissions::from_mode(0o600))?;
        let reopened = UnixJournal::open(&root)?;
        assert!(!orphan.exists());
        drop(reopened);

        let outside = outer.path().join("outside");
        std::fs::write(&outside, b"unchanged")?;
        let malicious = root.join(".journal-malicious");
        symlink(&outside, &malicious)?;
        assert!(UnixJournal::open(&root).is_err());
        assert!(malicious.symlink_metadata()?.file_type().is_symlink());
        assert_eq!(std::fs::read(&outside)?, b"unchanged");
        Ok(())
    }

    #[test]
    fn quarantine_retention_is_bounded_and_safe() -> TestResult {
        let (_outer, root, journal) = journal_fixture()?;
        for index in 0..(MAX_QUARANTINE_FILES + 4) {
            let txid = format!("corrupt-{index:02}");
            let path = root.join(format!("{txid}.json"));
            std::fs::write(&path, b"{")?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
            assert!(journal.read(&txid)?.is_none());
        }
        let quarantines = std::fs::read_dir(&root)?
            .filter_map(Result::ok)
            .filter(|entry| is_quarantine_name(entry.file_name().as_bytes()))
            .count();
        assert_eq!(quarantines, MAX_QUARANTINE_FILES);
        assert!(journal.entries()?.is_empty());
        Ok(())
    }
}
