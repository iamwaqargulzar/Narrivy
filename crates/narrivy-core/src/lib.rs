//! Narrivy application core.
//!
//! Concrete platform and ASR implementations are injected; this crate owns the
//! portable domain machinery required by P1 (REL-001 session machine with
//! monotonic events, FUN-005 transcript store, REL-007 worker supervision,
//! ADR-0013 recovery journal + versioned persistence, and the typed module
//! registry). See `IMPLEMENTATION-NOTES.md` for the P1 API map.

#![forbid(unsafe_code)]

mod endpoint;
mod journal;
mod modules;
mod persistence;
mod recovery;
mod session;
mod transcript;
mod worker;

pub use endpoint::{
    DictationEndpointConfig, DictationEndpointer, EndpointDecision, EndpointError, EnergyVadConfig,
    NativeEnergyVad, SpeechActivity, SpeechEndpointer, StreamingEndpointing, VoiceActivityDetector,
};
pub use journal::{
    JOURNAL_TTL_MS, Journal, JournalEntry, JournalEntryState, JournalError, MAX_JOURNAL_ENTRIES,
    RecoveryJournal, valid_txid,
};
pub use modules::{KNOWN_SLOTS, ModuleDescriptor, ModuleRegistry};
pub use persistence::{ConfigStore, MIGRATIONS, Migration, Store, apply_migrations, sha256_hex};
pub use recovery::{
    CAPTURE_SOURCE_ENDED, CorrectionPort, LivePreviewPort, MAX_VOLATILE_PARTIAL_BYTES,
    RecoveryDeliveryMode, RecoverySlice, RecoveryTerminal, RecoveryTimingObserver,
    RecoveryTimingStage, RecoveryTimingTrace, SOURCE_ENDED_MIN_SAMPLES, StartupRecoveryReport,
    StreamingRecoverySlice, VolatilePartial, recover_unsent_to_inbox,
};
pub use session::{SessionMachine, is_valid_transition};
pub use transcript::{TransactionClass, TranscriptStore};
pub use worker::{CancelFlag, WorkerHandle, WorkerJob, WorkerStop, WorkerSupervisor};

use narrivy_asr_api::{AsrEngine, AsrError};
use narrivy_platform_api::{PlatformError, TargetProvider, TextDelivery};
use narrivy_types::{AudioChunk, SessionId, SessionState, TargetContext, Transcript};

/// Typed error surface for narrivy-core. No stringly-typed failures escape
/// this crate's public API.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("operation cancelled after owned resources were stopped")]
    Cancelled,
    #[error("invalid session transition from {from:?} to {to:?}")]
    InvalidTransition {
        from: SessionState,
        to: SessionState,
    },
    #[error("journal error: {0}")]
    Journal(#[from] JournalError),
    #[error("migration error: {0}")]
    Migration(String),
    #[error("store error: {0}")]
    Store(String),
    #[error("worker error: {0}")]
    Worker(String),
    #[error("module registry error: {0}")]
    Module(String),
    #[error("platform error: {0:?}")]
    Platform(PlatformError),
    #[error("asr error: {0:?}")]
    Asr(AsrError),
}

pub(crate) fn unix_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

pub(crate) fn store_err<E: std::fmt::Display>(context: &str, err: E) -> CoreError {
    CoreError::Store(format!("{context}: {err}"))
}

/// Minimal Phase-0 orchestration scaffold kept for injection compatibility;
/// P1 domain machinery lives in the dedicated modules above.
pub struct NarrivyCore<P, D, A> {
    target_provider: P,
    delivery: D,
    asr: A,
}

impl<P, D, A> NarrivyCore<P, D, A>
where
    P: TargetProvider,
    D: TextDelivery,
    A: AsrEngine,
{
    pub fn new(target_provider: P, delivery: D, asr: A) -> Self {
        Self {
            target_provider,
            delivery,
            asr,
        }
    }

    pub fn transcribe_and_deliver(
        &mut self,
        _session_id: SessionId,
        audio: &AudioChunk,
    ) -> Result<(TargetContext, Transcript), CoreError> {
        let target = self
            .target_provider
            .capture_target()
            .map_err(CoreError::Platform)?;
        let transcript = self.asr.transcribe(audio).map_err(CoreError::Asr)?;
        self.delivery
            .insert_text(&target, &transcript.text)
            .map_err(CoreError::Platform)?;
        Ok((target, transcript))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use narrivy_asr_mock::MockAsr;
    use narrivy_platform_null::NullPlatform;

    #[test]
    fn portable_end_to_end_mock_dictation() -> Result<(), CoreError> {
        let platform = NullPlatform::default();
        let observer = platform.clone();
        let asr = MockAsr::new("Hello from Narrivy");
        let mut core = NarrivyCore::new(platform.clone(), platform, asr);
        let audio = AudioChunk {
            sample_rate_hz: 16_000,
            channels: 1,
            samples: vec![],
        };
        let (_, transcript) = core.transcribe_and_deliver(SessionId(1), &audio)?;
        assert_eq!(transcript.text, "Hello from Narrivy");
        assert_eq!(observer.delivered(), vec!["Hello from Narrivy"]);
        Ok(())
    }

    #[test]
    fn journal_error_kind_survives_core_boundary() {
        let error = CoreError::from(JournalError::InvalidTransactionId);
        assert!(matches!(
            error,
            CoreError::Journal(JournalError::InvalidTransactionId)
        ));
    }
}
