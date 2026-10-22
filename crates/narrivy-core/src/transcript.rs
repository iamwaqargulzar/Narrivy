//! Transcript variant store (FUN-005): raw / corrected / delivered
//! representations plus bounded delivery attempts, with truthful
//! recoverable-vs-terminal classification.

use crate::CoreError;
use narrivy_types::{DeliveryAttempt, DeliveryOutcome, TransactionId, TranscriptVariants};
use std::collections::BTreeMap;

/// Classification used by recovery/Inbox flows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionClass {
    /// Final text exists and has not been delivered/terminal-failed yet.
    Recoverable,
    /// Delivered successfully (last attempt Delivered).
    Delivered,
    /// Terminal failure recorded; text stays recoverable via explicit user action.
    TerminalFailed,
    /// Nothing stored yet.
    Empty,
}

#[derive(Debug, Default)]
pub struct TranscriptStore {
    by_tx: BTreeMap<String, TranscriptVariants>,
}

impl TranscriptStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn key(tx: &TransactionId) -> &str {
        tx.as_str()
    }

    pub fn set_raw(&mut self, tx: &TransactionId, raw: impl Into<String>) {
        self.by_tx.entry(Self::key(tx).to_string()).or_default().raw = Some(raw.into());
    }

    pub fn set_corrected(&mut self, tx: &TransactionId, corrected: impl Into<String>) {
        self.by_tx
            .entry(Self::key(tx).to_string())
            .or_default()
            .corrected = Some(corrected.into());
    }

    pub fn set_delivered(&mut self, tx: &TransactionId, delivered: impl Into<String>) {
        self.by_tx
            .entry(Self::key(tx).to_string())
            .or_default()
            .delivered = Some(delivered.into());
    }

    pub fn record_attempt(&mut self, tx: &TransactionId, attempt: DeliveryAttempt) {
        self.by_tx
            .entry(Self::key(tx).to_string())
            .or_default()
            .attempts
            .push(attempt);
    }

    pub fn variants(&self, tx: &TransactionId) -> Option<&TranscriptVariants> {
        self.by_tx.get(Self::key(tx))
    }

    /// Truthful classification per FUN-005: a transaction stays recoverable
    /// until a Delivered or terminal-Failed outcome exists.
    pub fn classify(&self, tx: &TransactionId) -> TransactionClass {
        let Some(v) = self.variants(tx) else {
            return TransactionClass::Empty;
        };
        let has_text = v.raw.is_some() || v.corrected.is_some() || v.delivered.is_some();
        match v.last_outcome() {
            Some(DeliveryOutcome::Delivered) => TransactionClass::Delivered,
            Some(DeliveryOutcome::Failed) if !has_text => TransactionClass::TerminalFailed,
            Some(DeliveryOutcome::Failed) => TransactionClass::TerminalFailed,
            _ if has_text => TransactionClass::Recoverable,
            _ => TransactionClass::Empty,
        }
    }

    /// Drop a transaction's memory (after delivery or explicit user action).
    pub fn forget(&mut self, tx: &TransactionId) -> Result<(), CoreError> {
        self.by_tx.remove(Self::key(tx));
        Ok(())
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.by_tx.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_tx.is_empty()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use narrivy_types::FallbackReason;

    #[test]
    fn variants_track_raw_corrected_delivered_and_classify() {
        let mut store = TranscriptStore::new();
        let tx = TransactionId::new("tx-1");
        assert_eq!(store.classify(&tx), TransactionClass::Empty);
        store.set_raw(&tx, "raw words");
        store.set_corrected(&tx, "Raw words.");
        assert_eq!(store.classify(&tx), TransactionClass::Recoverable);
        store.set_delivered(&tx, "Raw words.");
        store.record_attempt(
            &tx,
            DeliveryAttempt {
                backend: "wtype".to_string(),
                outcome: DeliveryOutcome::Delivered,
                fallback_reason: None,
                at_unix_ms: 1,
            },
        );
        assert_eq!(store.classify(&tx), TransactionClass::Delivered);
    }

    #[test]
    fn failed_attempt_with_preserved_text_is_terminal_but_recoverable_in_journal_terms() {
        let mut store = TranscriptStore::new();
        let tx = TransactionId::new("tx-2");
        store.set_raw(&tx, "keep me");
        store.record_attempt(
            &tx,
            DeliveryAttempt {
                backend: "wtype".to_string(),
                outcome: DeliveryOutcome::Failed,
                fallback_reason: Some(FallbackReason::TargetUnavailable),
                at_unix_ms: 2,
            },
        );
        // Store-level: terminal failure recorded...
        assert_eq!(store.classify(&tx), TransactionClass::TerminalFailed);
        // ...while the text remains preserved for explicit recovery.
        assert!(store.variants(&tx).unwrap().is_recoverable());
    }

    #[test]
    fn copy_only_is_not_delivered() {
        let mut store = TranscriptStore::new();
        let tx = TransactionId::new("tx-3");
        store.set_raw(&tx, "x");
        store.record_attempt(
            &tx,
            DeliveryAttempt {
                backend: "wl-copy".to_string(),
                outcome: DeliveryOutcome::CopiedOnly,
                fallback_reason: None,
                at_unix_ms: 3,
            },
        );
        assert_eq!(store.classify(&tx), TransactionClass::Recoverable);
    }
}
