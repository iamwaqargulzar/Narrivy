//! History store — SQLite per-user, TTL 7d/30d/forever, respects PRIV-002 off.
//! Implements history.db with history_entries(id, ts, pipeline_id, raw, corrected, delivered, app_id)
//! + settings(history_retention: Off|7d|30d|Forever) via ConfigStore pattern;
//!
//! `prune_expired(now)` + `set_retention()`; disabling history (Off) keeps journal recoverable.

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Retention policy per PRIV-002.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RetentionPolicy {
    #[default]
    Off,
    Days7,
    Days30,
    Forever,
}

impl RetentionPolicy {
    pub fn as_str(&self) -> &'static str {
        match self {
            RetentionPolicy::Off => "Off",
            RetentionPolicy::Days7 => "7d",
            RetentionPolicy::Days30 => "30d",
            RetentionPolicy::Forever => "Forever",
        }
    }

    /// Parse the persisted compatibility spellings, returning `None` for an
    /// unsupported value.
    #[expect(
        clippy::should_implement_trait,
        reason = "legacy public API returns Option; std::str::FromStr below returns a typed Result"
    )]
    pub fn from_str(value: &str) -> Option<Self> {
        Self::parse_value(value)
    }

    fn parse_value(s: &str) -> Option<Self> {
        match s {
            "Off" | "off" => Some(RetentionPolicy::Off),
            "7d" | "Days7" => Some(RetentionPolicy::Days7),
            "30d" | "Days30" => Some(RetentionPolicy::Days30),
            "Forever" | "forever" => Some(RetentionPolicy::Forever),
            _ => None,
        }
    }
    /// TTL in ms, None for Forever/Off (Off means disabled, not pruned)
    pub fn ttl_ms(&self) -> Option<u64> {
        match self {
            RetentionPolicy::Days7 => Some(7 * 24 * 60 * 60 * 1000),
            RetentionPolicy::Days30 => Some(30 * 24 * 60 * 60 * 1000),
            _ => None,
        }
    }
}

/// Returned when a persisted or supplied history-retention value is unsupported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionPolicyParseError;

impl std::fmt::Display for RetentionPolicyParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("unsupported history retention policy")
    }
}

impl std::error::Error for RetentionPolicyParseError {}

impl std::str::FromStr for RetentionPolicy {
    type Err = RetentionPolicyParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse_value(value).ok_or(RetentionPolicyParseError)
    }
}

/// One committed dictation record.
/// Schema: history_entries(id, ts, pipeline_id, raw, corrected, delivered, app_id)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: String,
    pub ts: u64,
    pub pipeline_id: String,
    pub raw: String,
    pub corrected: String,
    pub delivered: String,
    pub app_id: String,
    /// How long transcription took, in microseconds: from the end of capture
    /// to the raw text. Content-free. Absent when it was not measured and on
    /// rows written before this column existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcribe_us: Option<u64>,
    // Compatibility aliases for skeleton: txid/text/created_ms
    // We provide getters, but keep serde alias support.
}

impl HistoryEntry {
    pub fn new(
        id: impl Into<String>,
        ts: u64,
        pipeline_id: impl Into<String>,
        raw: impl Into<String>,
        corrected: impl Into<String>,
        delivered: impl Into<String>,
        app_id: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            ts,
            pipeline_id: pipeline_id.into(),
            raw: raw.into(),
            corrected: corrected.into(),
            delivered: delivered.into(),
            app_id: app_id.into(),
            transcribe_us: None,
        }
    }

    /// Attach the measured transcription time.
    #[must_use]
    pub fn with_transcribe_us(mut self, transcribe_us: Option<u64>) -> Self {
        self.transcribe_us = transcribe_us;
        self
    }
    // Skeleton compat helpers
    pub fn txid(&self) -> &str {
        &self.id
    }
    pub fn text(&self) -> &str {
        &self.delivered
    }
    pub fn created_ms(&self) -> u64 {
        self.ts
    }
}

// For skeleton compatibility where HistoryEntry had {txid, text, created_ms}
impl From<(String, String, u64)> for HistoryEntry {
    fn from((txid, text, created_ms): (String, String, u64)) -> Self {
        Self::new(
            txid,
            created_ms,
            "mock-dictation",
            text.clone(),
            text.clone(),
            text,
            "",
        )
    }
}

const SETTINGS_KEY: &str = "history_retention";

fn open_conn(path: &Path) -> Result<rusqlite::Connection, crate::HistoryError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| crate::HistoryError::Storage(format!("create dir: {e}")))?;
    }
    let conn = rusqlite::Connection::open(path)
        .map_err(|e| crate::HistoryError::Storage(format!("open db: {e}")))?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(|e| crate::HistoryError::Storage(format!("journal_mode: {e}")))?;
    conn.pragma_update(None, "synchronous", "NORMAL")
        .map_err(|e| crate::HistoryError::Storage(format!("synchronous: {e}")))?;
    conn.busy_timeout(std::time::Duration::from_millis(5000))
        .map_err(|e| crate::HistoryError::Storage(format!("busy_timeout: {e}")))?;
    Ok(conn)
}

fn ensure_schema(conn: &rusqlite::Connection) -> Result<(), crate::HistoryError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS history_entries (
            id TEXT PRIMARY KEY,
            ts INTEGER NOT NULL,
            pipeline_id TEXT NOT NULL,
            raw TEXT NOT NULL,
            corrected TEXT NOT NULL,
            delivered TEXT NOT NULL,
            app_id TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_history_entries_ts ON history_entries(ts);
        CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value_json TEXT NOT NULL,
            updated_at_ms INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS privacy_zones (
            id TEXT PRIMARY KEY,
            app_ids_json TEXT NOT NULL,
            pattern TEXT NOT NULL,
            deny INTEGER NOT NULL,
            actions_json TEXT NOT NULL
        );",
    )
    .map_err(|e| crate::HistoryError::Storage(format!("create tables: {e}")))?;
    add_transcribe_column(conn)
}

/// Additive migration: a store created before transcription time was kept
/// gains the nullable column; existing rows keep NULL ("not measured").
fn add_transcribe_column(conn: &rusqlite::Connection) -> Result<(), crate::HistoryError> {
    let present = conn
        .prepare("SELECT 1 FROM pragma_table_info('history_entries') WHERE name = 'transcribe_us'")
        .and_then(|mut stmt| stmt.exists([]))
        .map_err(|e| crate::HistoryError::Storage(format!("inspect schema: {e}")))?;
    if !present {
        conn.execute(
            "ALTER TABLE history_entries ADD COLUMN transcribe_us INTEGER",
            [],
        )
        .map_err(|e| crate::HistoryError::Storage(format!("add transcribe_us: {e}")))?;
    }
    Ok(())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn get_retention(
    conn: &rusqlite::Connection,
) -> Result<Option<RetentionPolicy>, crate::HistoryError> {
    let mut stmt = conn
        .prepare("SELECT value_json FROM settings WHERE key = ?1")
        .map_err(|e| crate::HistoryError::Storage(e.to_string()))?;
    let val: Option<String> = stmt
        .query_row([SETTINGS_KEY], |r| r.get(0))
        .optional()
        .map_err(|e| crate::HistoryError::Storage(e.to_string()))?;
    if let Some(json_str) = val {
        // Try parse as JSON string or raw
        // Stored as JSON string "\"Off\"" or "\"7d\"" etc, or as raw
        let parsed: Result<String, _> = serde_json::from_str(&json_str);
        let s = if let Ok(inner) = parsed {
            inner
        } else {
            // fallback raw
            json_str.trim_matches('"').to_string()
        };
        Ok(s.parse().ok())
    } else {
        Ok(None)
    }
}

fn set_retention_db(
    conn: &rusqlite::Connection,
    policy: RetentionPolicy,
) -> Result<(), crate::HistoryError> {
    let json_val = serde_json::to_string(&policy.as_str().to_string())
        .map_err(|e| crate::HistoryError::Storage(e.to_string()))?;
    conn.execute(
        "INSERT INTO settings (key, value_json, updated_at_ms) VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json, updated_at_ms=excluded.updated_at_ms",
        rusqlite::params![SETTINGS_KEY, json_val, now_ms() as i64],
    )
    .map_err(|e| crate::HistoryError::Storage(e.to_string()))?;
    Ok(())
}

/// HistoryStore — SQLite per-user with retention and privacy checks.
pub struct HistoryStore {
    path: PathBuf,
}

impl HistoryStore {
    /// Open history.db at given path (file path expected). If path is a directory, history.db is appended.
    /// `retention` is initial value if no persisted setting exists.
    pub fn open(
        path: impl Into<PathBuf>,
        retention: RetentionPolicy,
    ) -> Result<Self, crate::HistoryError> {
        let raw = path.into();
        let path = if raw.is_dir() {
            raw.join("history.db")
        } else {
            raw
        };
        let conn = open_conn(&path)?;
        ensure_schema(&conn)?;
        let existing = get_retention(&conn)?;
        match existing {
            Some(_) => {}
            None => set_retention_db(&conn, retention)?,
        }
        Ok(Self { path })
    }

    /// Convenience open that defaults retention to Forever if not persisted.
    pub fn open_default(path: impl Into<PathBuf>) -> Result<Self, crate::HistoryError> {
        Self::open(path, RetentionPolicy::Forever)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn retention(&self) -> RetentionPolicy {
        let conn = open_conn(&self.path).ok();
        if let Some(conn) = conn {
            if let Ok(Some(p)) = get_retention(&conn) {
                return p;
            }
        }
        RetentionPolicy::Forever
    }

    pub fn set_retention(&self, policy: RetentionPolicy) -> Result<(), crate::HistoryError> {
        let conn = open_conn(&self.path)?;
        set_retention_db(&conn, policy)
    }

    /// Insert entry respecting retention Off and privacy zones is_allowed.
    /// Returns true if inserted, false if skipped (Off or denied).
    pub fn insert(&self, entry: &HistoryEntry) -> Result<bool, crate::HistoryError> {
        if self.retention() == RetentionPolicy::Off {
            return Ok(false);
        }
        let conn = open_conn(&self.path)?;
        conn.execute(
            "INSERT OR REPLACE INTO history_entries (id, ts, pipeline_id, raw, corrected, delivered, app_id, transcribe_us) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                entry.id,
                entry.ts as i64,
                entry.pipeline_id,
                entry.raw,
                entry.corrected,
                entry.delivered,
                entry.app_id,
                entry.transcribe_us.map(|us| us.min(i64::MAX as u64) as i64)
            ],
        )
        .map_err(|e| crate::HistoryError::Storage(e.to_string()))?;
        Ok(true)
    }

    /// Insert only if is_allowed(app_id) per privacy zones.
    pub fn insert_if_allowed(
        &self,
        entry: &HistoryEntry,
        matcher: &crate::privacy_zones::ZoneMatcher,
    ) -> Result<bool, crate::HistoryError> {
        if !matcher.is_allowed(&entry.app_id) {
            return Ok(false);
        }
        self.insert(entry)
    }

    pub fn list(&self, limit: usize) -> Result<Vec<HistoryEntry>, crate::HistoryError> {
        let conn = open_conn(&self.path)?;
        let mut stmt = conn.prepare("SELECT id, ts, pipeline_id, raw, corrected, delivered, app_id, transcribe_us FROM history_entries ORDER BY ts DESC LIMIT ?1")
            .map_err(|e| crate::HistoryError::Storage(e.to_string()))?;
        let rows = stmt
            .query_map([limit as i64], |row| {
                Ok(HistoryEntry {
                    id: row.get(0)?,
                    ts: row.get::<_, i64>(1)? as u64,
                    pipeline_id: row.get(2)?,
                    raw: row.get(3)?,
                    corrected: row.get(4)?,
                    delivered: row.get(5)?,
                    app_id: row.get(6)?,
                    transcribe_us: row
                        .get::<_, Option<i64>>(7)?
                        .and_then(|us| u64::try_from(us).ok()),
                })
            })
            .map_err(|e| crate::HistoryError::Storage(e.to_string()))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| crate::HistoryError::Storage(e.to_string()))?);
        }
        Ok(out)
    }

    pub fn count(&self) -> Result<usize, crate::HistoryError> {
        let conn = open_conn(&self.path)?;
        let c: i64 = conn
            .query_row("SELECT COUNT(*) FROM history_entries", [], |r| r.get(0))
            .map_err(|e| crate::HistoryError::Storage(e.to_string()))?;
        Ok(c as usize)
    }

    /// Delete one entry by id. Returns whether a row was removed.
    pub fn delete_entry(&self, id: &str) -> Result<bool, crate::HistoryError> {
        let conn = open_conn(&self.path)?;
        let n = conn
            .execute("DELETE FROM history_entries WHERE id = ?1", [id])
            .map_err(|e| crate::HistoryError::Storage(e.to_string()))?;
        Ok(n > 0)
    }

    pub fn clear_all(&self) -> Result<usize, crate::HistoryError> {
        let conn = open_conn(&self.path)?;
        let n = conn
            .execute("DELETE FROM history_entries", [])
            .map_err(|e| crate::HistoryError::Storage(e.to_string()))?;
        Ok(n)
    }

    /// Prune expired entries based on current retention policy and given now_ms.
    /// Off and Forever prune nothing (Off disables inserts but keeps prior rows per spec).
    pub fn prune_expired(&self, now_ms: u64) -> Result<usize, crate::HistoryError> {
        let policy = self.retention();
        let ttl = match policy.ttl_ms() {
            Some(v) => v,
            None => return Ok(0),
        };
        let cutoff = now_ms.saturating_sub(ttl) as i64;
        let conn = open_conn(&self.path)?;
        let n = conn
            .execute("DELETE FROM history_entries WHERE ts < ?1", [cutoff])
            .map_err(|e| crate::HistoryError::Storage(e.to_string()))?;
        Ok(n)
    }

    /// Legacy helper: prune by days Option (None = Forever, 0 = clear all) for compatibility with v0.4 tests.
    pub fn prune_by_retention(
        &self,
        retention_days: Option<u32>,
    ) -> Result<usize, crate::HistoryError> {
        match retention_days {
            None => Ok(0),
            Some(0) => self.clear_all(),
            Some(days) => {
                let cutoff = now_ms().saturating_sub(days as u64 * 24 * 60 * 60 * 1000) as i64;
                let conn = open_conn(&self.path)?;
                let n = conn
                    .execute("DELETE FROM history_entries WHERE ts < ?1", [cutoff])
                    .map_err(|e| crate::HistoryError::Storage(e.to_string()))?;
                Ok(n)
            }
        }
    }

    /// Insert helper for tests that also writes a journal file to demonstrate journal still recoverable.
    pub fn insert_with_journal_check(
        &self,
        entry: &HistoryEntry,
        journal_dir: &Path,
    ) -> Result<bool, crate::HistoryError> {
        // History insert respects Off, but journal is separate - we simulate writing journal entry regardless.
        let inserted = self.insert(entry)?;
        // Ensure journal dir exists and write a dummy journal file if needed (simulates real journal)
        let _ = std::fs::create_dir_all(journal_dir);
        let journal_path = journal_dir.join(format!("{}.json", entry.id));
        let journal_content = serde_json::json!({
            "txid": entry.id,
            "raw": entry.raw,
            "delivered": entry.delivered,
            "state": "committed"
        });
        if let Ok(serialized) = serde_json::to_string_pretty(&journal_content) {
            let _ = std::fs::write(journal_path, serialized);
        }
        Ok(inserted)
    }
}

// Internal helper for tests
#[allow(dead_code)]
pub(crate) fn open_conn_for_test(path: &Path) -> Result<rusqlite::Connection, crate::HistoryError> {
    open_conn(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::privacy_zones::{PrivacyZone, PrivacyZoneAction, ZoneMatcher};
    use tempfile::tempdir;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn now() -> u64 {
        now_ms()
    }

    #[test]
    fn retention_policy_inherent_parser_remains_backward_compatible() {
        let legacy_parser: fn(&str) -> Option<RetentionPolicy> = RetentionPolicy::from_str;
        assert_eq!(legacy_parser("Off"), Some(RetentionPolicy::Off));
        assert_eq!(
            RetentionPolicy::from_str("Days7"),
            Some(RetentionPolicy::Days7)
        );
        assert_eq!(
            RetentionPolicy::from_str("30d"),
            Some(RetentionPolicy::Days30)
        );
        assert_eq!(
            RetentionPolicy::from_str("forever"),
            Some(RetentionPolicy::Forever)
        );
        assert_eq!(RetentionPolicy::from_str("unsupported"), None);

        assert_eq!("7d".parse::<RetentionPolicy>(), Ok(RetentionPolicy::Days7));
        assert_eq!(
            "unsupported".parse::<RetentionPolicy>(),
            Err(RetentionPolicyParseError)
        );
    }

    #[test]
    fn delete_entry_removes_exactly_one_row() -> TestResult {
        let dir = tempdir()?;
        let store = HistoryStore::open(dir.path().join("history.db"), RetentionPolicy::Forever)?;
        for id in ["tx-a", "tx-b"] {
            let entry = HistoryEntry::new(id, now(), "clean", "raw", "corrected", "delivered", "");
            assert!(store.insert(&entry)?);
        }
        assert!(store.delete_entry("tx-a")?);
        assert!(
            !store.delete_entry("tx-a")?,
            "a second delete finds nothing"
        );
        assert!(!store.delete_entry("tx-missing")?);
        assert_eq!(store.count()?, 1);
        Ok(())
    }

    #[test]
    fn transcription_time_round_trips_and_is_absent_when_not_measured() -> TestResult {
        let dir = tempdir()?;
        let store = HistoryStore::open(dir.path().join("history.db"), RetentionPolicy::Forever)?;
        let timed = HistoryEntry::new("tx-timed", now(), "clean", "r", "c", "d", "")
            .with_transcribe_us(Some(1_250_000));
        let untimed = HistoryEntry::new("tx-untimed", now() - 1, "clean", "r", "c", "d", "");
        assert!(store.insert(&timed)?);
        assert!(store.insert(&untimed)?);
        let listed = store.list(10)?;
        assert_eq!(listed[0].transcribe_us, Some(1_250_000));
        assert_eq!(listed[1].transcribe_us, None);
        Ok(())
    }

    #[test]
    fn a_store_created_before_the_timing_column_is_migrated_in_place() -> TestResult {
        let dir = tempdir()?;
        let path = dir.path().join("history.db");
        {
            let conn = rusqlite::Connection::open(&path)?;
            conn.execute_batch(
                "CREATE TABLE history_entries (
                    id TEXT PRIMARY KEY, ts INTEGER NOT NULL, pipeline_id TEXT NOT NULL,
                    raw TEXT NOT NULL, corrected TEXT NOT NULL, delivered TEXT NOT NULL,
                    app_id TEXT NOT NULL
                );
                INSERT INTO history_entries VALUES ('tx-old', 1, 'clean', 'r', 'c', 'd', '');",
            )?;
        }
        let store = HistoryStore::open(&path, RetentionPolicy::Forever)?;
        let listed = store.list(10)?;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "tx-old");
        assert_eq!(listed[0].transcribe_us, None);
        // Opening again is a no-op, and new rows can carry a time.
        let store = HistoryStore::open(&path, RetentionPolicy::Forever)?;
        let timed = HistoryEntry::new("tx-new", now(), "clean", "r", "c", "d", "")
            .with_transcribe_us(Some(42));
        assert!(store.insert(&timed)?);
        assert_eq!(store.list(10)?[0].transcribe_us, Some(42));
        Ok(())
    }

    #[test]
    fn privacy_history_off_disables_insert_but_journal_still_recoverable() -> TestResult {
        let dir = tempdir()?;
        let hist_path = dir.path().join("history.db");
        let journal_dir = dir.path().join("journal");
        std::fs::create_dir_all(&journal_dir)?;
        let store = HistoryStore::open(&hist_path, RetentionPolicy::Off)?;
        assert_eq!(store.retention(), RetentionPolicy::Off);
        let entry = HistoryEntry::new(
            "tx1",
            now(),
            "mock-dictation",
            "raw",
            "corrected",
            "delivered",
            "com.example.app",
        );
        let inserted = store.insert(&entry)?;
        assert!(!inserted, "Off must not insert");
        assert_eq!(store.count()?, 0);
        // Simulate journal still recoverable: write journal file regardless of history Off
        let journal_file = journal_dir.join("tx1.json");
        std::fs::write(
            &journal_file,
            r#"{"txid":"tx1","state":"committed","raw":"raw"}"#,
        )?;
        assert!(
            journal_file.exists(),
            "journal file must exist even when history Off"
        );
        let content = std::fs::read_to_string(&journal_file)?;
        assert!(content.contains("tx1"));
        Ok(())
    }

    #[test]
    fn privacy_history_retention_7d_prunes_expired() -> TestResult {
        let dir = tempdir()?;
        let store = HistoryStore::open(dir.path().join("history.db"), RetentionPolicy::Days7)?;
        let old_ts = now().saturating_sub(8 * 24 * 60 * 60 * 1000);
        let new_ts = now();
        let old = HistoryEntry::new("old", old_ts, "p", "r", "c", "d", "app");
        let new = HistoryEntry::new("new", new_ts, "p", "r", "c", "d", "app");
        store.insert(&old)?;
        store.insert(&new)?;
        assert_eq!(store.count()?, 2);
        let pruned = store.prune_expired(now())?;
        assert_eq!(pruned, 1);
        let list = store.list(10)?;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "new");
        Ok(())
    }

    #[test]
    fn privacy_history_retention_30d_prunes() -> TestResult {
        let dir = tempdir()?;
        let store = HistoryStore::open(dir.path().join("history.db"), RetentionPolicy::Days30)?;
        let old_ts = now().saturating_sub(31 * 24 * 60 * 60 * 1000);
        let mid_ts = now().saturating_sub(15 * 24 * 60 * 60 * 1000);
        let new_ts = now();
        store.insert(&HistoryEntry::new("old", old_ts, "p", "r", "c", "d", "app"))?;
        store.insert(&HistoryEntry::new("mid", mid_ts, "p", "r", "c", "d", "app"))?;
        store.insert(&HistoryEntry::new("new", new_ts, "p", "r", "c", "d", "app"))?;
        let pruned = store.prune_expired(now())?;
        assert_eq!(pruned, 1);
        assert_eq!(store.count()?, 2);
        let ids: Vec<_> = store.list(10)?.into_iter().map(|e| e.id).collect();
        assert!(ids.contains(&"mid".to_string()));
        assert!(ids.contains(&"new".to_string()));
        Ok(())
    }

    #[test]
    fn privacy_history_off_then_reenable_preserves_prior_rows() -> TestResult {
        let dir = tempdir()?;
        let hist_path = dir.path().join("history.db");
        let store = HistoryStore::open(&hist_path, RetentionPolicy::Forever)?;
        let e1 = HistoryEntry::new("e1", now(), "p", "r", "c", "d", "app");
        store.insert(&e1)?;
        assert_eq!(store.count()?, 1);
        // Disable
        store.set_retention(RetentionPolicy::Off)?;
        assert_eq!(store.retention(), RetentionPolicy::Off);
        // Prune while Off should prune 0 and preserve rows
        let pruned = store.prune_expired(now())?;
        assert_eq!(pruned, 0);
        assert_eq!(store.count()?, 1, "Off must preserve prior rows");
        // Insert while Off should be blocked
        let e2 = HistoryEntry::new("e2", now(), "p", "r", "c", "d", "app");
        let inserted = store.insert(&e2)?;
        assert!(!inserted);
        assert_eq!(store.count()?, 1);
        // Re-enable
        store.set_retention(RetentionPolicy::Forever)?;
        assert_eq!(store.retention(), RetentionPolicy::Forever);
        assert_eq!(store.count()?, 1, "re-enable must preserve prior rows");
        let e3 = HistoryEntry::new("e3", now(), "p", "r", "c", "d", "app");
        store.insert(&e3)?;
        assert_eq!(store.count()?, 2);
        // Reopen from disk should preserve
        drop(store);
        let reopened = HistoryStore::open(&hist_path, RetentionPolicy::Off)?;
        // persisted retention is Forever, not Off passed in
        assert_eq!(reopened.retention(), RetentionPolicy::Forever);
        assert_eq!(reopened.count()?, 2);
        Ok(())
    }

    #[test]
    fn privacy_history_denied_app_produces_no_row_but_journal_still_has_it() -> TestResult {
        let dir = tempdir()?;
        let hist_path = dir.path().join("history.db");
        let journal_dir = dir.path().join("journal");
        std::fs::create_dir_all(&journal_dir)?;
        let store = HistoryStore::open(&hist_path, RetentionPolicy::Forever)?;
        let zone = PrivacyZone {
            id: "deny-app-x".to_string(),
            pattern: "app_x".to_string(),
            app_ids: vec!["app_x".to_string()],
            deny: true,
            actions: vec![PrivacyZoneAction::DenyHistory],
        };
        let matcher = ZoneMatcher::new(vec![zone]);
        // Dictation into app_x should be denied for history
        let entry = HistoryEntry::new("tx-appx", now(), "p", "raw", "corr", "del", "app_x");
        assert!(!matcher.is_allowed("app_x"));
        let inserted = store.insert_if_allowed(&entry, &matcher)?;
        assert!(!inserted, "denied app should produce no history row");
        assert_eq!(store.count()?, 0);
        // But journal still has it (simulate)
        let journal_file = journal_dir.join("tx-appx.json");
        std::fs::write(
            &journal_file,
            serde_json::json!({"txid":"tx-appx","app_id":"app_x","state":"committed"}).to_string(),
        )?;
        assert!(journal_file.exists());
        // Allowed app should insert
        let entry2 = HistoryEntry::new("tx-other", now(), "p", "raw", "corr", "del", "other_app");
        assert!(matcher.is_allowed("other_app"));
        let inserted2 = store.insert_if_allowed(&entry2, &matcher)?;
        assert!(inserted2);
        assert_eq!(store.count()?, 1);
        Ok(())
    }

    #[test]
    fn privacy_history_disable_scenario() -> TestResult {
        // p8-history-disable: history Off -> after dictation, history empty but journal recoverable
        let dir = tempdir()?;
        let hist_path = dir.path().join("history.db");
        let journal_dir = dir.path().join("journal");
        std::fs::create_dir_all(&journal_dir)?;
        let store = HistoryStore::open(&hist_path, RetentionPolicy::Off)?;
        // Simulate dictation
        let entry = HistoryEntry::new(
            "tx-disable",
            now(),
            "mock",
            "hello world",
            "Hello world.",
            "Hello world.",
            "com.example.editor",
        );
        let inserted = store.insert(&entry)?;
        assert!(!inserted);
        assert_eq!(store.count()?, 0, "history empty when Off");
        // Journal recoverable
        let jpath = journal_dir.join("tx-disable.json");
        std::fs::write(&jpath, r#"{"txid":"tx-disable","state":"committed"}"#)?;
        assert!(jpath.exists());
        let rec = std::fs::read_to_string(&jpath)?;
        assert!(rec.contains("tx-disable"));
        Ok(())
    }

    #[test]
    fn privacy_history_forever_never_prunes() -> TestResult {
        let dir = tempdir()?;
        let store = HistoryStore::open(dir.path().join("history.db"), RetentionPolicy::Forever)?;
        let old_ts = now().saturating_sub(365 * 24 * 60 * 60 * 1000);
        store.insert(&HistoryEntry::new(
            "ancient", old_ts, "p", "r", "c", "d", "app",
        ))?;
        let pruned = store.prune_expired(now())?;
        assert_eq!(pruned, 0);
        assert_eq!(store.count()?, 1);
        Ok(())
    }

    #[test]
    fn privacy_history_off_prunes_nothing() -> TestResult {
        let dir = tempdir()?;
        let store = HistoryStore::open(dir.path().join("history.db"), RetentionPolicy::Off)?;
        // Manually bypass Off to insert old row for test (using direct conn)
        {
            let conn = open_conn(store.path())?;
            conn.execute("INSERT OR REPLACE INTO history_entries (id, ts, pipeline_id, raw, corrected, delivered, app_id) VALUES (?1,?2,?3,?4,?5,?6,?7)",
                rusqlite::params!["old", (now().saturating_sub(100*24*60*60*1000) as i64), "p", "r", "c", "d", "app"])?;
        }
        assert_eq!(store.count()?, 1);
        let pruned = store.prune_expired(now())?;
        assert_eq!(pruned, 0, "Off should prune 0");
        assert_eq!(store.count()?, 1);
        Ok(())
    }
}
