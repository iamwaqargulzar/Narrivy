//! Portable recovery-journal boundary.
//!
//! Record shape, retention, and the storage port live in
//! `narrivy-journal-api`. Concrete filesystem persistence is composed by the
//! service from an outward adapter.

pub use narrivy_journal_api::{
    JOURNAL_TTL_MS, Journal, JournalEntry, JournalEntryState, JournalError, MAX_JOURNAL_ENTRIES,
    RecoveryJournal, valid_txid,
};

#[cfg(test)]
pub(crate) use test_support::TestJournal;

#[cfg(test)]
mod test_support {
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use narrivy_journal_api::{JournalEntry, JournalError, RecoveryJournal, valid_txid};

    pub(crate) struct TestJournal {
        entries: Mutex<BTreeMap<String, JournalEntry>>,
        fail_next_write: AtomicBool,
        fail_after_successful_writes: AtomicUsize,
    }

    impl Default for TestJournal {
        fn default() -> Self {
            Self::new()
        }
    }

    impl TestJournal {
        pub(crate) fn new() -> Self {
            Self {
                entries: Mutex::new(BTreeMap::new()),
                fail_next_write: AtomicBool::new(false),
                fail_after_successful_writes: AtomicUsize::new(usize::MAX),
            }
        }

        pub(crate) fn fail_next_write_for_test(&self) {
            self.fail_next_write.store(true, Ordering::SeqCst);
        }

        pub(crate) fn fail_after_successful_writes_for_test(&self, writes: usize) {
            self.fail_after_successful_writes
                .store(writes, Ordering::SeqCst);
        }

        fn lock_entries(
            &self,
        ) -> Result<std::sync::MutexGuard<'_, BTreeMap<String, JournalEntry>>, JournalError>
        {
            self.entries
                .lock()
                .map_err(|_| JournalError::Storage("test journal lock poisoned".to_owned()))
        }
    }

    impl RecoveryJournal for TestJournal {
        fn write(&self, entry: &JournalEntry) -> Result<(), JournalError> {
            valid_txid(&entry.txid)?;
            if self.fail_next_write.swap(false, Ordering::SeqCst) {
                return Err(JournalError::Storage("controlled write failure".to_owned()));
            }
            let remaining = self.fail_after_successful_writes.load(Ordering::SeqCst);
            if remaining != usize::MAX {
                if remaining == 0 {
                    self.fail_after_successful_writes
                        .store(usize::MAX, Ordering::SeqCst);
                    return Err(JournalError::Storage("controlled write failure".to_owned()));
                }
                self.fail_after_successful_writes
                    .fetch_sub(1, Ordering::SeqCst);
            }
            self.lock_entries()?
                .insert(entry.txid.clone(), entry.clone());
            Ok(())
        }

        fn read(&self, txid: &str) -> Result<Option<JournalEntry>, JournalError> {
            valid_txid(txid)?;
            Ok(self.lock_entries()?.get(txid).cloned())
        }

        fn entries(&self) -> Result<Vec<JournalEntry>, JournalError> {
            Ok(self.lock_entries()?.values().cloned().collect())
        }

        fn remove(&self, txid: &str) -> Result<bool, JournalError> {
            valid_txid(txid)?;
            Ok(self.lock_entries()?.remove(txid).is_some())
        }
    }
}
