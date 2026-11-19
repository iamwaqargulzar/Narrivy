//! Portable recovery-journal records, retention policy, and storage port.
//!
//! Persistent filesystem mechanics belong to outward adapters. This crate
//! owns the stable record shape and domain decisions needed by core recovery.

#![forbid(unsafe_code)]

use narrivy_types::{CaptureEnd, DeliveryIntent, DeliveryReport, TranscriptVariants};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub const JOURNAL_TTL_MS: u64 = 15 * 60 * 1000;
pub const MAX_JOURNAL_ENTRIES: usize = 100;
pub const MAX_JOURNAL_ENTRY_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalEntryState {
    Pending,
    Committed,
    Delivered,
    InboxRecovered,
    Cancelled,
    FailedRecoverable,
    FailedTerminal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JournalEntry {
    pub txid: String,
    pub state: JournalEntryState,
    #[serde(default)]
    pub pipeline_id: Option<String>,
    pub variants: TranscriptVariants,
    #[serde(default)]
    pub delivery_intent: Option<DeliveryIntent>,
    #[serde(default)]
    pub delivery_report: Option<DeliveryReport>,
    #[serde(default)]
    pub failure_code: Option<String>,
    /// Typed, content-free reason microphone capture stopped accepting audio
    /// for this transaction. Retained durably so a client can tell a graceful
    /// Stop from the explicit documented maximum without re-deriving OS state.
    #[serde(default)]
    pub capture_end: Option<CaptureEnd>,
    pub created_unix_ms: u64,
    pub updated_unix_ms: u64,
}

impl JournalEntry {
    pub fn new_pending(txid: impl Into<String>, now_unix_ms: u64) -> Self {
        Self {
            txid: txid.into(),
            state: JournalEntryState::Pending,
            pipeline_id: None,
            variants: TranscriptVariants::default(),
            delivery_intent: None,
            delivery_report: None,
            failure_code: None,
            capture_end: None,
            created_unix_ms: now_unix_ms,
            updated_unix_ms: now_unix_ms,
        }
    }

    pub fn is_recoverable_at(&self, now_unix_ms: u64) -> bool {
        matches!(
            self.state,
            JournalEntryState::Pending
                | JournalEntryState::Committed
                | JournalEntryState::FailedRecoverable
        ) && self.created_unix_ms <= now_unix_ms
            && now_unix_ms - self.created_unix_ms < JOURNAL_TTL_MS
    }
}

/// Deterministic transaction IDs to remove under the recovery TTL and count
/// policy. The caller owns mutation/locking; keeping selection portable avoids
/// policy drift between in-memory and filesystem adapters.
pub fn retention_victims(entries: &[JournalEntry], now_unix_ms: u64) -> Vec<String> {
    retention_victims_preserving(entries, now_unix_ms, None)
}

/// Variant used by atomic admission: a fresh, non-expired record is removed
/// only when no other survivor can satisfy the cap. TTL still applies to it.
pub fn retention_victims_preserving(
    entries: &[JournalEntry],
    now_unix_ms: u64,
    preserve_txid: Option<&str>,
) -> Vec<String> {
    let mut ordered = entries.to_vec();
    ordered.sort_by(|left, right| {
        left.created_unix_ms
            .cmp(&right.created_unix_ms)
            .then_with(|| left.txid.cmp(&right.txid))
    });

    let mut victims: Vec<String> = ordered
        .iter()
        .filter(|entry| {
            entry.created_unix_ms > now_unix_ms
                || now_unix_ms - entry.created_unix_ms >= JOURNAL_TTL_MS
        })
        .map(|entry| entry.txid.clone())
        .collect();

    let mut survivors: Vec<_> = ordered
        .into_iter()
        .filter(|entry| {
            entry.created_unix_ms <= now_unix_ms
                && now_unix_ms - entry.created_unix_ms < JOURNAL_TTL_MS
        })
        .collect();
    let excess = survivors.len().saturating_sub(MAX_JOURNAL_ENTRIES);
    if excess == 0 {
        return victims;
    }

    survivors.sort_by_key(|entry| {
        let recoverable = matches!(
            entry.state,
            JournalEntryState::Pending
                | JournalEntryState::Committed
                | JournalEntryState::FailedRecoverable
        );
        (
            preserve_txid.is_some_and(|txid| entry.txid == txid),
            recoverable,
            entry.created_unix_ms,
            entry.txid.clone(),
        )
    });
    victims.extend(survivors.into_iter().take(excess).map(|entry| entry.txid));
    victims
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JournalError {
    #[error("invalid journal transaction id")]
    InvalidTransactionId,
    #[error("journal entry is too large")]
    EntryTooLarge,
    #[error("journal transaction id already exists")]
    DuplicateTransactionId,
    #[error("unsafe journal storage state: {0}")]
    UnsafeStorage(&'static str),
    #[error("journal storage operation failed: {0}")]
    Storage(String),
}

/// Validate `[A-Za-z0-9._-]{1,64}`, excluding `.` and `..`.
pub fn valid_txid(txid: &str) -> Result<(), JournalError> {
    let ok_len = !txid.is_empty() && txid.len() <= 64;
    let ok_chars = txid
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if ok_len && ok_chars && txid != "." && txid != ".." {
        Ok(())
    } else {
        Err(JournalError::InvalidTransactionId)
    }
}

/// Storage boundary for the short-lived recovery journal.
///
/// Implementations must return entries in any order; the portable default
/// methods establish stable timestamp order and retention semantics.
pub trait RecoveryJournal: Send + Sync {
    fn write(&self, entry: &JournalEntry) -> Result<(), JournalError>;
    /// Persist a newly admitted transaction without replacing an existing
    /// record. Filesystem adapters must implement this atomically.
    fn write_new(&self, entry: &JournalEntry) -> Result<(), JournalError> {
        if self.read(&entry.txid)?.is_some() {
            return Err(JournalError::DuplicateTransactionId);
        }
        self.write(entry)
    }
    fn read(&self, txid: &str) -> Result<Option<JournalEntry>, JournalError>;
    fn entries(&self) -> Result<Vec<JournalEntry>, JournalError>;
    fn remove(&self, txid: &str) -> Result<bool, JournalError>;

    fn mark_state(
        &self,
        txid: &str,
        state: JournalEntryState,
    ) -> Result<JournalEntry, JournalError> {
        let mut entry = self
            .read(txid)?
            .ok_or_else(|| JournalError::Storage("unknown transaction id".to_owned()))?;
        entry.state = state;
        entry.updated_unix_ms = unix_now_ms();
        self.write(&entry)?;
        Ok(entry)
    }

    fn recoverable_entries(&self) -> Result<Vec<JournalEntry>, JournalError> {
        self.recoverable_entries_at(unix_now_ms())
    }

    fn recoverable_entries_at(&self, now_unix_ms: u64) -> Result<Vec<JournalEntry>, JournalError> {
        let mut entries: Vec<_> = self
            .entries()?
            .into_iter()
            .filter(|entry| entry.is_recoverable_at(now_unix_ms))
            .collect();
        entries.sort_by_key(|entry| entry.created_unix_ms);
        Ok(entries)
    }

    fn enforce_limits(&self) -> Result<usize, JournalError> {
        self.enforce_limits_at(unix_now_ms())
    }

    fn enforce_limits_at(&self, now_unix_ms: u64) -> Result<usize, JournalError> {
        let entries = self.entries()?;
        let mut removed = 0;
        for txid in retention_victims(&entries, now_unix_ms) {
            if self.remove(&txid)? {
                removed += 1;
            }
        }
        Ok(removed)
    }
}

impl<T> RecoveryJournal for Arc<T>
where
    T: RecoveryJournal + ?Sized,
{
    fn write(&self, entry: &JournalEntry) -> Result<(), JournalError> {
        (**self).write(entry)
    }

    fn write_new(&self, entry: &JournalEntry) -> Result<(), JournalError> {
        (**self).write_new(entry)
    }

    fn read(&self, txid: &str) -> Result<Option<JournalEntry>, JournalError> {
        (**self).read(txid)
    }

    fn entries(&self) -> Result<Vec<JournalEntry>, JournalError> {
        (**self).entries()
    }

    fn remove(&self, txid: &str) -> Result<bool, JournalError> {
        (**self).remove(txid)
    }

    fn mark_state(
        &self,
        txid: &str,
        state: JournalEntryState,
    ) -> Result<JournalEntry, JournalError> {
        (**self).mark_state(txid, state)
    }

    fn recoverable_entries(&self) -> Result<Vec<JournalEntry>, JournalError> {
        (**self).recoverable_entries()
    }

    fn recoverable_entries_at(&self, now_unix_ms: u64) -> Result<Vec<JournalEntry>, JournalError> {
        (**self).recoverable_entries_at(now_unix_ms)
    }

    fn enforce_limits(&self) -> Result<usize, JournalError> {
        (**self).enforce_limits()
    }

    fn enforce_limits_at(&self, now_unix_ms: u64) -> Result<usize, JournalError> {
        (**self).enforce_limits_at(now_unix_ms)
    }
}

/// Compatibility name for the trait object consumed by orchestration.
pub type Journal = dyn RecoveryJournal;

fn unix_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    #[derive(Default)]
    struct MemoryJournal(Mutex<BTreeMap<String, JournalEntry>>);

    struct OverrideProbe(MemoryJournal);

    impl RecoveryJournal for OverrideProbe {
        fn write(&self, entry: &JournalEntry) -> Result<(), JournalError> {
            self.0.write(entry)
        }

        fn read(&self, txid: &str) -> Result<Option<JournalEntry>, JournalError> {
            self.0.read(txid)
        }

        fn entries(&self) -> Result<Vec<JournalEntry>, JournalError> {
            self.0.entries()
        }

        fn remove(&self, txid: &str) -> Result<bool, JournalError> {
            self.0.remove(txid)
        }

        fn enforce_limits_at(&self, _now_unix_ms: u64) -> Result<usize, JournalError> {
            Ok(42)
        }
    }

    impl RecoveryJournal for MemoryJournal {
        fn write(&self, entry: &JournalEntry) -> Result<(), JournalError> {
            valid_txid(&entry.txid)?;
            self.0
                .lock()
                .map_err(|_| JournalError::Storage("memory lock poisoned".to_owned()))?
                .insert(entry.txid.clone(), entry.clone());
            Ok(())
        }

        fn read(&self, txid: &str) -> Result<Option<JournalEntry>, JournalError> {
            valid_txid(txid)?;
            Ok(self
                .0
                .lock()
                .map_err(|_| JournalError::Storage("memory lock poisoned".to_owned()))?
                .get(txid)
                .cloned())
        }

        fn entries(&self) -> Result<Vec<JournalEntry>, JournalError> {
            Ok(self
                .0
                .lock()
                .map_err(|_| JournalError::Storage("memory lock poisoned".to_owned()))?
                .values()
                .cloned()
                .collect())
        }

        fn remove(&self, txid: &str) -> Result<bool, JournalError> {
            valid_txid(txid)?;
            Ok(self
                .0
                .lock()
                .map_err(|_| JournalError::Storage("memory lock poisoned".to_owned()))?
                .remove(txid)
                .is_some())
        }
    }

    #[test]
    fn persistent_record_shape_and_state_tags_remain_stable()
    -> Result<(), Box<dyn std::error::Error>> {
        let golden = serde_json::json!({
            "txid": "tx-golden",
            "state": "inbox_recovered",
            "pipeline_id": "linux-mic-wtype-inbox",
            "variants": {
                "raw": "raw fixture",
                "corrected": "corrected fixture",
                "delivered": "delivered fixture",
                "attempts": [{
                    "backend": "wtype-stdin",
                    "outcome": "SubmittedUnverified",
                    "fallback_reason": {"BackendError": {"code": "unverified"}},
                    "at_unix_ms": 41
                }]
            },
            "delivery_intent": {
                "transaction_id": "tx-golden",
                "backend": "wtype-stdin"
            },
            "delivery_report": {
                "transaction_id": "tx-golden",
                "backend": "wtype-stdin",
                "attempts": [{
                    "backend": "wtype-stdin",
                    "outcome": "SubmittedUnverified",
                    "fallback_reason": null,
                    "at_unix_ms": 42
                }, {
                    "backend": "inbox",
                    "outcome": "InboxFallback",
                    "fallback_reason": null,
                    "at_unix_ms": 43
                }],
                "outcome": "InboxFallback",
                "failure_reason": null
            },
            "failure_code": null,
            "capture_end": null,
            "created_unix_ms": 10,
            "updated_unix_ms": 43
        });
        let entry = serde_json::from_value::<JournalEntry>(golden.clone())?;
        assert_eq!(serde_json::to_value(entry)?, golden);
        // The typed capture end is durable and content-free: an entry that
        // recorded the explicit maximum must round-trip as that exact reason.
        let mut limited = JournalEntry::new_pending("tx-limited", 11);
        limited.capture_end = Some(narrivy_types::CaptureEnd::SafetyLimit);
        let value = serde_json::to_value(&limited)?;
        assert_eq!(value["capture_end"], serde_json::json!("safety_limit"));
        assert_eq!(
            serde_json::from_value::<JournalEntry>(value)?.capture_end,
            Some(narrivy_types::CaptureEnd::SafetyLimit)
        );
        for (state, tag) in [
            (JournalEntryState::Pending, "pending"),
            (JournalEntryState::Committed, "committed"),
            (JournalEntryState::Delivered, "delivered"),
            (JournalEntryState::InboxRecovered, "inbox_recovered"),
            (JournalEntryState::Cancelled, "cancelled"),
            (JournalEntryState::FailedRecoverable, "failed_recoverable"),
            (JournalEntryState::FailedTerminal, "failed_terminal"),
        ] {
            assert_eq!(serde_json::to_value(state)?, tag);
        }
        Ok(())
    }

    #[test]
    fn arc_forwarding_preserves_implementation_overrides() -> Result<(), JournalError> {
        let journal = Arc::new(OverrideProbe(MemoryJournal::default()));
        assert_eq!(journal.enforce_limits_at(1)?, 42);
        Ok(())
    }

    #[test]
    fn default_new_write_rejects_duplicate_without_replacing() -> Result<(), JournalError> {
        let journal = MemoryJournal::default();
        let original = JournalEntry::new_pending("tx-unique", 1);
        journal.write_new(&original)?;
        let replacement = JournalEntry::new_pending("tx-unique", 2);
        assert_eq!(
            journal.write_new(&replacement),
            Err(JournalError::DuplicateTransactionId)
        );
        assert_eq!(journal.read("tx-unique")?, Some(original));
        Ok(())
    }

    #[test]
    fn retention_and_recoverability_are_portable_domain_policy() -> Result<(), JournalError> {
        let journal = MemoryJournal::default();
        let fresh = JournalEntry::new_pending("fresh", 100);
        journal.write(&fresh)?;
        let mut old = JournalEntry::new_pending("old", 0);
        old.state = JournalEntryState::Delivered;
        journal.write(&old)?;
        assert_eq!(journal.recoverable_entries_at(200)?, vec![fresh]);
        assert_eq!(journal.enforce_limits_at(JOURNAL_TTL_MS + 1)?, 1);
        assert!(journal.read("old")?.is_none());
        Ok(())
    }

    #[test]
    fn retention_expires_every_state_and_rejects_future_timestamps() {
        let now = JOURNAL_TTL_MS + 10;
        let mut entries = Vec::new();
        for (index, state) in [
            JournalEntryState::Pending,
            JournalEntryState::Committed,
            JournalEntryState::Delivered,
            JournalEntryState::InboxRecovered,
            JournalEntryState::Cancelled,
            JournalEntryState::FailedRecoverable,
            JournalEntryState::FailedTerminal,
        ]
        .into_iter()
        .enumerate()
        {
            let mut entry = JournalEntry::new_pending(format!("expired-{index}"), 10);
            entry.state = state;
            entries.push(entry);
        }
        entries.push(JournalEntry::new_pending("future", now + 1));
        assert_eq!(retention_victims(&entries, now).len(), 8);
        assert!(!JournalEntry::new_pending("future-check", now + 1).is_recoverable_at(now));
    }

    #[test]
    fn retention_cap_prefers_terminal_then_oldest_recoverable() {
        let now = 10_000;
        let mut entries = (0..MAX_JOURNAL_ENTRIES)
            .map(|index| JournalEntry::new_pending(format!("recoverable-{index:03}"), index as u64))
            .collect::<Vec<_>>();
        let mut terminal = JournalEntry::new_pending("terminal-old", 0);
        terminal.state = JournalEntryState::Delivered;
        entries.push(terminal);
        assert_eq!(retention_victims(&entries, now), ["terminal-old"]);

        entries.retain(|entry| entry.txid != "terminal-old");
        entries.push(JournalEntry::new_pending("recoverable-extra", 0));
        assert_eq!(retention_victims(&entries, now), ["recoverable-000"]);
    }

    #[test]
    fn exact_ttl_is_expired_and_equal_timestamps_use_txid_order() {
        let now = JOURNAL_TTL_MS + 50;
        let at_boundary = JournalEntry::new_pending("boundary", 50);
        assert!(!at_boundary.is_recoverable_at(now));

        let entries = (0..=MAX_JOURNAL_ENTRIES)
            .rev()
            .map(|index| JournalEntry::new_pending(format!("same-{index:03}"), now))
            .collect::<Vec<_>>();
        assert_eq!(retention_victims(&entries, now), ["same-000"]);
    }

    #[test]
    fn atomic_admission_can_preserve_the_fresh_record_under_equal_timestamps() {
        let now = 50_000;
        let mut entries: Vec<_> = (0..MAX_JOURNAL_ENTRIES)
            .map(|index| JournalEntry::new_pending(format!("z-{index:03}"), now))
            .collect();
        entries.push(JournalEntry::new_pending("a-new", now));
        assert_eq!(
            retention_victims_preserving(&entries, now, Some("a-new")),
            ["z-000"]
        );
    }

    #[test]
    fn transaction_ids_fail_closed() {
        for invalid in ["", ".", "..", "../escape", "a/b", &"x".repeat(65)] {
            assert_eq!(valid_txid(invalid), Err(JournalError::InvalidTransactionId));
        }
        assert!(valid_txid("ok-TX_9.x").is_ok());
    }
}
