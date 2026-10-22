//! Daemon-owned composition of the optional retained History store (PRIV-002).
//!
//! This module is the only place in `narrivyd` that touches
//! `narrivy_history`. It exists because the recovery journal and the
//! optional user History are different stores with different lifecycles
//! (ADR-0013, ADR-0045):
//!
//! - it composes the real SQLite store below the daemon's private durable
//!   state root, with a 0700 directory and a 0600 database;
//! - it owns the effective retention policy (TTL plus a count cap) and
//!   applies it at every access boundary, exactly like the recovery
//!   journal's retention decorator;
//! - it mirrors one FINALIZED recovery result into History, so the recovery
//!   journal remains the only holder of in-flight text;
//! - it answers the privacy-sensitive reads with the existing protocol DTOs
//!   and fails closed with a typed error when the store is absent or
//!   unusable.
//!
//! The store's public API can only remove rows as a whole table, so the
//! count cap is applied by keeping exactly the newest `MAX_HISTORY_ENTRIES`
//! rows. That invariant is what makes an exact-by-id read a single bounded
//! read instead of an unbounded scan.
//!
//! Blocking SQLite I/O below runs on IPC request handlers and on an owned
//! session worker. It never runs on an audio or UI real-time path.

#![forbid(unsafe_code)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use narrivy_core::{JournalEntry, JournalEntryState};
use narrivy_history::{HistoryEntry, HistoryError, HistoryStore, RetentionPolicy, ZoneMatcher};
use narrivy_ipc::protocol::{HistoryEntryDetailDto, HistoryEntryDto};
use narrivy_policy::{DataClass, PolicyDocument, PolicyEngine, ProcessingPolicy};
use narrivy_types::ProcessingBoundary;

/// Count cap applied to the retained optional History. This mirrors the
/// recovery journal's bounded retention (`MAX_JOURNAL_ENTRIES`) so a
/// long-running daemon cannot grow transcript storage without bound even
/// under `forever` retention.
pub const MAX_HISTORY_ENTRIES: usize = 200;

/// Bounded read window served by one `GetHistory` request. It equals
/// `MAX_HISTORY_ENTRIES`, so one request can observe the whole retained
/// store and never a partial illusion of completeness.
pub const HISTORY_LIST_LIMIT: usize = MAX_HISTORY_ENTRIES;

/// User-facing retention hook, read exactly once while composing.
pub const HISTORY_RETENTION_ENV: &str = "NARRIVY_HISTORY_RETENTION";

/// Daemon default when the retention hook is absent or empty.
///
/// Bounded rather than `forever`, because an unbounded transcript store is
/// the one retention choice a user cannot undo implicitly. `off` is always
/// available and always wins over an older persisted value (see
/// [`open_retained_history`]).
const DEFAULT_HISTORY_RETENTION: RetentionPolicy = RetentionPolicy::Days7;

const PRIVATE_DIR_MODE: u32 = 0o700;
const PRIVATE_FILE_MODE: u32 = 0o600;

/// Content-free rejection detail for a daemon that has no History store.
/// Stable: clients and evidence match on this exact text.
pub const HISTORY_NOT_COMPOSED_DETAIL: &str = "optional history is disabled and not composed";
/// Content-free rejection detail for a composed store whose storage is
/// unusable. It is a typed failure, never an empty success.
pub const HISTORY_UNAVAILABLE_DETAIL: &str = "optional history storage is unavailable";

/// Outcome of one mirroring attempt. Content-free: it never carries
/// transcript text or the denied application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordOutcome {
    /// The row is now retained.
    Recorded,
    /// PRIV-002 global disable: retention is `Off`, so nothing is written.
    DisabledByPolicy,
    /// The policy engine denied the transcript at this store's boundary.
    DeniedByPolicy,
    /// A Privacy Zone denied the target application. A zone override wins
    /// over every other source, including an allow zone for the same app.
    DeniedByZone,
    /// The transaction retained no transcript, so there is nothing to
    /// mirror.
    NotRetained,
}

/// One daemon-generation-owned optional History store.
///
/// It owns no thread, timer, or file handle between calls: every operation
/// opens and closes its own connection, so the store's shutdown is the drop
/// of its last owner (the composition root's reference, released after the
/// serve window closes).
pub struct RetainedHistory {
    store: HistoryStore,
    zones: ZoneMatcher,
    /// The effective, daemon-authoritative retention policy. It is written
    /// to the store at composition time and is the only value write
    /// decisions consult, so a global disable can never be widened by a
    /// fallible read-back.
    retention: std::sync::RwLock<RetentionPolicy>,
    /// Retention decisions read this clock. Injecting it is what makes TTL
    /// evidence deterministic without deleting fixtures with the wall clock,
    /// exactly as the recovery journal's retention decorator is tested.
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    /// Serializes retention enforcement against writes so a bounded read
    /// can never observe a partially compacted store.
    operation: Mutex<()>,
}

/// Compose the real store below the daemon's private durable state root,
/// selecting the retention policy from [`HISTORY_RETENTION_ENV`].
///
/// A malformed hook is an error rather than a silent fallback, so a typo can
/// never widen retention.
pub fn open_retained_history(
    state_dir: &Path,
    retention: RetentionPolicy,
) -> Result<RetainedHistory, String> {
    compose_retained_history(state_dir, retention, Arc::new(wall_clock_unix_ms))
}

/// Deterministic-time composition, mirroring the recovery journal's
/// synthetic-time seam. Every retention decision below then uses `now_ms`
/// instead of the wall clock, so a fixture timestamp is neither expired nor
/// fresh by accident.
#[cfg(test)]
pub fn open_retained_history_with_clock(
    state_dir: &Path,
    retention: RetentionPolicy,
    now_ms: u64,
) -> Result<RetainedHistory, String> {
    compose_retained_history(state_dir, retention, Arc::new(move || now_ms))
}

fn compose_retained_history(
    state_dir: &Path,
    retention: RetentionPolicy,
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
) -> Result<RetainedHistory, String> {
    let db_path = retained_history_db_path(state_dir);
    secure_retained_history_dir(&db_path)?;
    let store = HistoryStore::open(&db_path, retention)
        .map_err(|error| format!("open retained history failed: {error}"))?;
    // `HistoryStore::open` keeps an already-persisted policy and ignores the
    // requested one. Re-assert the composed value on every start so a
    // user-selected `off` is a global disable and never merely a request
    // that a previous daemon generation outranked.
    store
        .set_retention(retention)
        .map_err(|error| format!("apply retained history retention failed: {error}"))?;
    // SQLite created the database through its own defaults; re-assert the
    // private mode now that the file exists.
    harden_retained_history_files(&db_path);
    let zones = ZoneMatcher::load_from_db(store.path())
        .map_err(|error| format!("load privacy zones failed: {error}"))?;
    let service = RetainedHistory {
        store,
        zones,
        retention: std::sync::RwLock::new(retention),
        clock,
        operation: Mutex::new(()),
    };
    service
        .enforce_retention(service.now())
        .map_err(|error| format!("retained history retention enforcement failed: {error}"))?;
    Ok(service)
}

/// Read the user-facing retention hook. An absent or empty hook selects the
/// daemon default; an unsupported value is a composition error.
pub fn retention_from_env() -> Result<RetentionPolicy, String> {
    let Some(raw) = std::env::var_os(HISTORY_RETENTION_ENV) else {
        return Ok(DEFAULT_HISTORY_RETENTION);
    };
    if raw.is_empty() {
        return Ok(DEFAULT_HISTORY_RETENTION);
    }
    let value = raw.to_string_lossy().to_ascii_lowercase();
    value.parse::<RetentionPolicy>().map_err(|_| {
        format!("{HISTORY_RETENTION_ENV}={value:?} is unsupported (expected off|7d|30d|forever)")
    })
}

/// The retention to compose with: the environment hook when set (tests and
/// qualification), otherwise the user's saved choice, otherwise the default.
pub fn retention_preference(state_dir: &Path) -> Result<RetentionPolicy, String> {
    if std::env::var_os(HISTORY_RETENTION_ENV).is_some_and(|v| !v.is_empty()) {
        return retention_from_env();
    }
    Ok(std::fs::read_to_string(preference_path(state_dir))
        .ok()
        .and_then(|text| text.trim().parse::<RetentionPolicy>().ok())
        .unwrap_or(DEFAULT_HISTORY_RETENTION))
}

/// Save the user's retention choice for the next start.
pub fn save_retention_preference(state_dir: &Path, wire: &str) -> std::io::Result<()> {
    std::fs::write(preference_path(state_dir), wire)
}

fn preference_path(state_dir: &Path) -> PathBuf {
    state_dir.join("history").join("retention")
}

/// Durable location of the retained store, matching the journal's
/// `state_dir/<store>` layout.
pub fn retained_history_db_path(state_dir: &Path) -> PathBuf {
    state_dir.join("history").join("history.db")
}

impl RetainedHistory {
    /// Content-free location for diagnostics and qualification only.
    pub fn db_path(&self) -> &Path {
        self.store.path()
    }

    /// The effective retention policy for this daemon generation.
    pub fn retention(&self) -> RetentionPolicy {
        *self
            .retention
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Change how long History is kept, and apply it now: a shorter policy
    /// removes what it no longer keeps. `off` keeps what is there and adds
    /// nothing new.
    pub fn set_retention(&self, policy: RetentionPolicy) -> Result<usize, HistoryError> {
        let _guard = self.lock_operation();
        self.store.set_retention(policy)?;
        *self
            .retention
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = policy;
        self.enforce_retention_locked(self.now())
    }

    /// Remove every History entry.
    pub fn clear(&self) -> Result<usize, HistoryError> {
        let _guard = self.lock_operation();
        self.store.clear_all()
    }

    /// Content-free wire token for the retention capability fact.
    pub fn retention_wire(&self) -> &'static str {
        match self.retention() {
            RetentionPolicy::Off => "off",
            RetentionPolicy::Days7 => "7d",
            RetentionPolicy::Days30 => "30d",
            RetentionPolicy::Forever => "forever",
        }
    }

    /// Number of retained rows right now. Used by content-free
    /// diagnostics and tests.
    pub fn count(&self) -> Result<usize, HistoryError> {
        let _guard = self.lock_operation();
        self.enforce_retention_locked(self.now())?;
        self.store.count()
    }

    /// Newest-first listing window served by `GetHistory`.
    ///
    /// Retention is enforced first, so a read can never report a row the
    /// effective policy has already expired, and a storage failure is a
    /// typed error rather than an empty list that looks like success.
    ///
    /// `off` stops retention rather than erasing it, matching the store's
    /// documented semantics ("off disables inserts but keeps prior rows").
    /// A read therefore still serves rows that were lawfully retained under an
    /// earlier policy, while nothing new is ever recorded. The capability set
    /// reports that state as `history.retention=off`, and a client must not
    /// read an empty list as "there is nothing to show".
    pub fn list(&self, limit: usize) -> Result<Vec<HistoryEntry>, HistoryError> {
        let _guard = self.lock_operation();
        self.enforce_retention_locked(self.now())?;
        self.store.list(limit.clamp(1, HISTORY_LIST_LIMIT))
    }

    /// Exactly one retained row, or `None` when the store never retained
    /// that transaction.
    ///
    /// The count cap keeps the store at or below
    /// [`MAX_HISTORY_ENTRIES`] rows at every access boundary, so one
    /// bounded read is exact for the whole store.
    pub fn entry(&self, txid: &str) -> Result<Option<HistoryEntry>, HistoryError> {
        let _guard = self.lock_operation();
        self.enforce_retention_locked(self.now())?;
        Ok(self
            .store
            .list(MAX_HISTORY_ENTRIES)?
            .into_iter()
            .find(|entry| entry.id == txid))
    }

    /// Delete exactly one retained row on explicit user request. Returns
    /// whether the row existed. The recovery journal is never touched.
    pub fn delete(&self, txid: &str) -> Result<bool, HistoryError> {
        let _guard = self.lock_operation();
        self.store.delete_entry(txid)
    }

    /// Mirror one FINALIZED recovery result into the retained History.
    ///
    /// Precedence, strongest first:
    ///
    /// 1. a Privacy Zone deny for the target application, which wins over
    ///    every other source including an allow zone for the same app;
    /// 2. the global retention policy, where `off` writes nothing;
    /// 3. the policy engine's transcript gate at this store's `Local`
    ///    boundary — the same `narrivy-policy` machinery the recovery slice
    ///    consults before it places raw text on any durable store.
    ///
    /// `app_id` is the admission-time target application, or an empty
    /// string when no trustworthy pre-activation target was bound. An
    /// absent application simply matches no zone; it is never guessed.
    pub fn record_committed(
        &self,
        result: &JournalEntry,
        app_id: &str,
    ) -> Result<RecordOutcome, HistoryError> {
        self.record_committed_timed(result, app_id, None)
    }

    /// [`Self::record_committed`] with the measured transcription time, a
    /// content-free duration kept beside the entry so History can show it.
    pub fn record_committed_timed(
        &self,
        result: &JournalEntry,
        app_id: &str,
        transcribe_us: Option<u64>,
    ) -> Result<RecordOutcome, HistoryError> {
        if !self.zones.is_allowed(app_id) {
            return Ok(RecordOutcome::DeniedByZone);
        }
        if self.retention() == RetentionPolicy::Off {
            return Ok(RecordOutcome::DisabledByPolicy);
        }
        let Some(entry) = retained_entry(result, app_id) else {
            return Ok(RecordOutcome::NotRetained);
        };
        let entry = entry.with_transcribe_us(transcribe_us);
        if self.policy_denied() {
            return Ok(RecordOutcome::DeniedByPolicy);
        }
        let _guard = self.lock_operation();
        // Make room under the count cap before writing, and re-assert both
        // limits after it, so the invariant holds at every boundary.
        self.enforce_retention_locked(self.now())?;
        if !self.store.insert(&entry)? {
            return Ok(RecordOutcome::DisabledByPolicy);
        }
        self.enforce_retention_locked(self.now())?;
        Ok(RecordOutcome::Recorded)
    }

    /// Transcript gate for the retained store, evaluated with the portable
    /// policy engine rather than a parallel rule.
    fn policy_denied(&self) -> bool {
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        policy
            .allow_data(DataClass::TranscriptRaw, ProcessingBoundary::Local)
            .is_err()
            || policy
                .allow_data(DataClass::TranscriptCorrected, ProcessingBoundary::Local)
                .is_err()
    }

    fn now(&self) -> u64 {
        (self.clock)()
    }

    fn lock_operation(&self) -> std::sync::MutexGuard<'_, ()> {
        // A poisoned retention lock must not turn a privacy read into a
        // panic; the store's own writes are individually atomic.
        self.operation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Apply the effective retention policy to the real store.
    fn enforce_retention(&self, now_ms: u64) -> Result<usize, HistoryError> {
        let _guard = self.lock_operation();
        self.enforce_retention_locked(now_ms)
    }

    /// Caller holds the operation lock.
    fn enforce_retention_locked(&self, now_ms: u64) -> Result<usize, HistoryError> {
        let mut removed = self.store.prune_expired(now_ms)?;
        removed += self.enforce_count_cap()?;
        Ok(removed)
    }

    /// Count cap for the retained store.
    ///
    /// The store's public API removes rows only as a whole table, so the cap
    /// keeps exactly the newest [`MAX_HISTORY_ENTRIES`] rows: the survivor
    /// window is materialized before the table is cleared, and a reinsert
    /// failure is reported as a typed storage error rather than silently
    /// shrinking retention.
    ///
    /// `off` is exempt. It disables new writes but deliberately preserves
    /// already-retained rows, so the cap must not run in that mode.
    fn enforce_count_cap(&self) -> Result<usize, HistoryError> {
        if self.retention() == RetentionPolicy::Off {
            return Ok(0);
        }
        let total = self.store.count()?;
        if total <= MAX_HISTORY_ENTRIES {
            return Ok(0);
        }
        let survivors = self.store.list(MAX_HISTORY_ENTRIES)?;
        let excess = total.saturating_sub(survivors.len());
        self.store.clear_all()?;
        for survivor in &survivors {
            if !self.store.insert(survivor)? {
                return Err(HistoryError::Storage(
                    "retained history compaction was refused by the store".to_owned(),
                ));
            }
        }
        Ok(excess)
    }
}

/// Deterministic mapping of one finalized recovery result onto its retained
/// History row.
///
/// `None` when the transaction retained no transcript, so an in-flight,
/// cancelled or failed transaction is never mirrored into History.
///
/// The retained schema owns the transcript variants and the target
/// application only. It has no `capture_end` column, so the detail DTO
/// reports that field as absent rather than inventing one.
fn retained_entry(result: &JournalEntry, app_id: &str) -> Option<HistoryEntry> {
    if !matches!(
        result.state,
        JournalEntryState::Committed
            | JournalEntryState::Delivered
            | JournalEntryState::InboxRecovered
    ) {
        return None;
    }
    result
        .variants
        .delivered
        .as_deref()
        .filter(|text| !text.is_empty())
        .or_else(|| {
            result
                .variants
                .corrected
                .as_deref()
                .filter(|text| !text.is_empty())
        })?;

    Some(HistoryEntry::new(
        result.txid.clone(),
        result.updated_unix_ms.max(result.created_unix_ms),
        result.pipeline_id.clone().unwrap_or_default(),
        result.variants.raw.clone().unwrap_or_default(),
        result.variants.corrected.clone().unwrap_or_default(),
        result.variants.delivered.clone().unwrap_or_default(),
        app_id.to_owned(),
    ))
}

/// Content-free state token derived from what the store actually retained.
///
/// The retained schema has no state column, so the token is a deterministic
/// function of the retained variants: a delivered transcript is `delivered`,
/// anything else retained is `committed`. A recovered-but-undelivered
/// transaction therefore reports `committed`, never a recovered delivery it
/// cannot prove.
pub fn retained_state_wire(entry: &HistoryEntry) -> &'static str {
    if entry.delivered.is_empty() {
        "committed"
    } else {
        "delivered"
    }
}

/// Content-safe listing row for one retained entry.
pub fn listing_row(entry: &HistoryEntry) -> HistoryEntryDto {
    HistoryEntryDto {
        txid: entry.id.clone(),
        state: retained_state_wire(entry).to_owned(),
        pipeline_id: Some(entry.pipeline_id.clone()).filter(|id| !id.is_empty()),
        has_delivered_text: !entry.delivered.is_empty(),
        updated_unix_ms: entry.ts,
        transcribe_us: entry.transcribe_us,
    }
}

/// Full transcript variants for one explicitly named retained row.
pub fn entry_detail(entry: &HistoryEntry) -> HistoryEntryDetailDto {
    HistoryEntryDetailDto {
        txid: entry.id.clone(),
        state: retained_state_wire(entry).to_owned(),
        raw: Some(entry.raw.clone()).filter(|text| !text.is_empty()),
        corrected: Some(entry.corrected.clone()).filter(|text| !text.is_empty()),
        delivered: Some(entry.delivered.clone()).filter(|text| !text.is_empty()),
        pipeline_id: Some(entry.pipeline_id.clone()).filter(|id| !id.is_empty()),
        // The retained schema does not store a capture end; reporting it as
        // absent is truthful and stays wire-compatible with older peers.
        capture_end: None,
    }
}

/// Create the store's private directory with 0700 before any database file
/// can appear inside it.
fn secure_retained_history_dir(db_path: &Path) -> Result<(), String> {
    let parent = db_path
        .parent()
        .ok_or("retained history database path has no parent".to_owned())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("create retained history dir failed: {error}"))?;
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(PRIVATE_DIR_MODE))
        .map_err(|error| format!("secure retained history dir failed: {error}"))
}

/// Re-assert the private mode on the database file. The containing
/// directory is already 0700, which is what keeps the SQLite `-wal` and
/// `-shm` sidecars unreachable by any other Unix user.
fn harden_retained_history_files(db_path: &Path) {
    if let Ok(metadata) = std::fs::metadata(db_path) {
        let mut permissions = metadata.permissions();
        permissions.set_mode(PRIVATE_FILE_MODE);
        let _ = std::fs::set_permissions(db_path, permissions);
    }
}

/// Monotonic wall clock in milliseconds. Duplicated from the crate-private
/// daemon helper so this module has no import cycle.
fn wall_clock_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{
        HISTORY_LIST_LIMIT, MAX_HISTORY_ENTRIES, RecordOutcome, entry_detail, listing_row,
        open_retained_history, open_retained_history_with_clock, retained_entry,
        retained_history_db_path, retained_state_wire, retention_from_env, HISTORY_RETENTION_ENV,
        retention_preference, save_retention_preference,
    };
    use std::os::unix::fs::MetadataExt;

    use narrivy_core::{JournalEntry, JournalEntryState};
    use narrivy_history::{HistoryEntry, PrivacyZone, RetentionPolicy, ZoneMatcher};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// Fixed instant every retention decision in this module's tests reads.
    const NOW: u64 = 1_700_000_000_000;

    fn private_tempdir() -> Result<tempfile::TempDir, std::io::Error> {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir()?;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
        Ok(dir)
    }

    fn committed(txid: &str, delivered: Option<&str>, updated_ms: u64) -> JournalEntry {
        let mut entry = JournalEntry::new_pending(txid, updated_ms);
        entry.state = JournalEntryState::Delivered;
        entry.pipeline_id = Some("mock-dictation".to_owned());
        entry.variants.raw = Some("raw".to_owned());
        entry.variants.corrected = Some("corrected".to_owned());
        entry.variants.delivered = delivered.map(str::to_owned);
        entry.updated_unix_ms = updated_ms;
        entry
    }

    #[test]
    fn retention_hook_rejects_an_unsupported_value_instead_of_widening() {
        // The hook is absent in the test environment, so the daemon default
        // applies. A malformed value must never be treated as `forever`.
        assert_eq!(
            retention_from_env().unwrap_or(RetentionPolicy::Off),
            RetentionPolicy::Days7
        );
        assert!(
            "NARRIVY_HISTORY_RETENTION"
                .parse::<RetentionPolicy>()
                .is_err()
        );
    }

    #[test]
    fn composed_store_uses_private_modes() -> TestResult {
        let dir = private_tempdir()?;
        let service = open_retained_history_with_clock(dir.path(), RetentionPolicy::Days7, NOW)?;
        let db_path = retained_history_db_path(dir.path());
        assert_eq!(service.db_path(), db_path.as_path());
        let dir_mode = std::fs::metadata(db_path.parent().ok_or("missing parent")?)?.mode() & 0o777;
        assert_eq!(dir_mode, 0o700, "retained history dir must be 0700");
        let file_mode = std::fs::metadata(&db_path)?.mode() & 0o777;
        assert_eq!(file_mode, 0o600, "retained history db must be 0600");
        Ok(())
    }

    #[test]
    fn only_a_finalized_result_with_text_is_mirrored() -> TestResult {
        let dir = private_tempdir()?;
        let service = open_retained_history_with_clock(dir.path(), RetentionPolicy::Days7, NOW)?;
        let mut pending = committed("tx-pending", Some("text"), NOW);
        pending.state = JournalEntryState::Pending;
        assert_eq!(
            service.record_committed(&pending, "")?,
            RecordOutcome::NotRetained
        );

        let mut cancelled = committed("tx-cancelled", Some("text"), NOW);
        cancelled.state = JournalEntryState::Cancelled;
        assert_eq!(
            service.record_committed(&cancelled, "")?,
            RecordOutcome::NotRetained
        );

        // A transaction that reached a final state but produced no text at all.
        let mut empty = committed("tx-empty", Some("ignored"), NOW);
        empty.variants.raw = None;
        empty.variants.corrected = None;
        empty.variants.delivered = None;
        assert_eq!(
            service.record_committed(&empty, "")?,
            RecordOutcome::NotRetained
        );
        assert_eq!(service.count()?, 0);

        assert_eq!(
            service.record_committed(&committed("tx-live", Some("text"), NOW), "")?,
            RecordOutcome::Recorded
        );
        assert_eq!(service.count()?, 1);
        Ok(())
    }

    #[test]
    fn the_listing_row_carries_the_measured_transcription_time() -> TestResult {
        let dir = private_tempdir()?;
        let service = open_retained_history_with_clock(dir.path(), RetentionPolicy::Days7, NOW)?;
        service.record_committed_timed(
            &committed("tx-timed", Some("text"), NOW),
            "",
            Some(840_000),
        )?;
        service.record_committed(&committed("tx-untimed", Some("text"), NOW - 1), "")?;
        let rows: Vec<_> = service.list(10)?.iter().map(listing_row).collect();
        assert_eq!(rows[0].txid, "tx-timed");
        assert_eq!(rows[0].transcribe_us, Some(840_000));
        // An unmeasured entry reports no time rather than a zero.
        assert_eq!(rows[1].transcribe_us, None);
        Ok(())
    }

    #[test]
    fn retention_changes_at_runtime_and_history_clears() -> TestResult {
        let dir = private_tempdir()?;
        let history = open_retained_history_with_clock(dir.path(), RetentionPolicy::Days7, NOW)?;
        history.record_committed(&committed("tx-one", Some("one"), NOW), "")?;
        history.record_committed(&committed("tx-two", Some("two"), NOW), "")?;
        history.set_retention(RetentionPolicy::Off)?;
        assert_eq!(history.retention_wire(), "off");
        // Off adds nothing new and keeps what is there.
        history.record_committed(&committed("tx-three", Some("three"), NOW), "")?;
        assert_eq!(history.count()?, 2);
        history.set_retention(RetentionPolicy::Days30)?;
        assert_eq!(history.clear()?, 2);
        assert_eq!(history.count()?, 0);
        // The saved choice is used next time, unless the environment says otherwise.
        save_retention_preference(dir.path(), "30d")?;
        if std::env::var_os(HISTORY_RETENTION_ENV).is_none() {
            assert_eq!(retention_preference(dir.path())?, RetentionPolicy::Days30);
        }
        Ok(())
    }

    #[test]
    fn global_disable_writes_nothing_and_preserves_prior_rows() -> TestResult {
        let dir = private_tempdir()?;
        let enabled = open_retained_history_with_clock(dir.path(), RetentionPolicy::Days7, NOW)?;
        enabled.record_committed(&committed("tx-prior", Some("prior"), NOW), "")?;
        assert_eq!(enabled.count()?, 1);
        drop(enabled);

        // Re-composing with `off` must win over the persisted `7d` and must
        // not erase what was lawfully retained earlier.
        let disabled = open_retained_history_with_clock(dir.path(), RetentionPolicy::Off, NOW)?;
        assert_eq!(disabled.retention(), RetentionPolicy::Off);
        assert_eq!(disabled.retention_wire(), "off");
        assert_eq!(
            disabled.record_committed(&committed("tx-after", Some("after"), NOW + 1), "")?,
            RecordOutcome::DisabledByPolicy
        );
        assert_eq!(disabled.count()?, 1, "off preserves prior rows");
        assert_eq!(
            disabled.entry("tx-after")?.map(|entry| entry.id),
            None,
            "a disabled store must retain nothing new"
        );
        Ok(())
    }

    #[test]
    fn ttl_and_count_bounds_are_both_enforced() -> TestResult {
        let dir = private_tempdir()?;
        let service = open_retained_history_with_clock(dir.path(), RetentionPolicy::Days7, NOW)?;
        let day = 24 * 60 * 60 * 1000;

        // TTL: an entry older than the policy's window is not reported.
        service.record_committed(&committed("tx-expired", Some("old"), NOW - 8 * day), "")?;
        assert_eq!(
            service.record_committed(&committed("tx-fresh", Some("new"), NOW), "")?,
            RecordOutcome::Recorded
        );
        let listed = service.list(HISTORY_LIST_LIMIT)?;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "tx-fresh");

        // Count: the cap keeps exactly the newest rows, so the two oldest of
        // a deliberately over-full store fall outside it.
        for index in 0..=MAX_HISTORY_ENTRIES {
            let id = format!("tx-cap-{index:04}");
            service.record_committed(&committed(&id, Some("x"), NOW + index as u64 + 1), "")?;
        }
        assert_eq!(service.count()?, MAX_HISTORY_ENTRIES);
        for expired in ["tx-fresh", "tx-cap-0000"] {
            assert_eq!(
                service.entry(expired)?.map(|entry| entry.id),
                None,
                "{expired} is outside the count cap"
            );
        }
        assert!(
            service.entry("tx-cap-0200")?.is_some(),
            "the newest row must survive the count cap"
        );
        Ok(())
    }

    #[test]
    fn privacy_zone_deny_overrides_an_allow_zone_for_the_same_app() -> TestResult {
        let dir = private_tempdir()?;
        let service = open_retained_history_with_clock(dir.path(), RetentionPolicy::Days7, NOW)?;
        ZoneMatcher::new(vec![
            PrivacyZone::new_deny("allow-bank", vec!["com.bank.app".to_owned()], false),
            PrivacyZone::new_deny("deny-bank", vec!["com.bank.app".to_owned()], true),
        ])
        .save_to_db(service.db_path())?;
        drop(service);

        let zones = open_retained_history_with_clock(dir.path(), RetentionPolicy::Days7, NOW)?;
        assert_eq!(
            zones.record_committed(&committed("tx-bank", Some("secret"), NOW), "com.bank.app")?,
            RecordOutcome::DeniedByZone
        );
        assert_eq!(
            zones.record_committed(&committed("tx-other", Some("fine"), NOW), "com.other.app")?,
            RecordOutcome::Recorded
        );
        assert_eq!(zones.count()?, 1);
        assert_eq!(
            zones.entry("tx-bank")?.map(|entry| entry.id),
            None,
            "a denied application must produce no retained row"
        );
        Ok(())
    }

    #[test]
    fn listing_rows_never_carry_transcript_text() {
        let entry = HistoryEntry::new(
            "tx-1",
            42,
            "mock-dictation",
            "raw secret",
            "corrected secret",
            "delivered secret",
            "com.example.app",
        );
        let row = listing_row(&entry);
        assert_eq!(row.txid, "tx-1");
        assert_eq!(row.state, "delivered");
        assert!(row.has_delivered_text);
        assert_eq!(row.updated_unix_ms, 42);
        assert_eq!(row.pipeline_id.as_deref(), Some("mock-dictation"));
        let rendered = format!("{row:?}");
        for secret in ["raw secret", "corrected secret", "delivered secret"] {
            assert!(!rendered.contains(secret), "listing row leaked {secret:?}");
        }

        let detail = entry_detail(&entry);
        assert_eq!(detail.raw.as_deref(), Some("raw secret"));
        assert!(detail.capture_end.is_none());
        assert!(
            format!("{detail:?}").contains("<redacted>"),
            "detail debug must stay redacted"
        );
    }

    #[test]
    fn state_is_derived_from_retained_variants_only() {
        let undelivered = HistoryEntry::new("tx-1", 1, "p", "r", "c", "", "app");
        assert_eq!(retained_state_wire(&undelivered), "committed");
        assert!(!listing_row(&undelivered).has_delivered_text);
        assert_eq!(entry_detail(&undelivered).delivered, None);

        let undelivered_id = HistoryEntry::new("tx-2", 1, "", "r", "c", "d", "app");
        assert_eq!(listing_row(&undelivered_id).pipeline_id, None);
    }

    #[test]
    fn retained_entry_uses_the_result_timestamp_and_variants() -> TestResult {
        let mut result = committed("tx-1", Some("done"), 5_000);
        result.created_unix_ms = 9_000;
        result.updated_unix_ms = 4_000;
        let mapped = retained_entry(&result, "com.example.app").ok_or("expected a retained row")?;
        assert_eq!(mapped.id, "tx-1");
        assert_eq!(mapped.ts, 9_000, "the older update must not rewind the row");
        assert_eq!(mapped.raw, "raw");
        assert_eq!(mapped.corrected, "corrected");
        assert_eq!(mapped.delivered, "done");
        assert_eq!(mapped.app_id, "com.example.app");
        Ok(())
    }
}
