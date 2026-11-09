//! Portable domain types for Narrivy.
//!
//! P1 additions are serde-serializable so IPC/persistence layers can bind them
//! without duplicating domain semantics.

pub mod shutdown;

pub use shutdown::ShutdownBudget;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct SessionId(pub u128);

/// Canonical sample rate of the dictation capture contract.
///
/// The daemon, the Linux capture adapter and the resident ASR protocol all
/// account for microphone audio as mono f32 at this rate.
pub const CAPTURE_SAMPLE_RATE_HZ: u32 = 16_000;

/// Documented absolute maximum for one live dictation capture session.
///
/// Ordinary toggle/manual dictation is Stop-driven: capture ends when the user
/// requests Stop, the endpoint policy detects an endpoint, or the user cancels.
/// This bound is the safety maximum that keeps one session's frames, IPC
/// payload and resident-ASR staging finite. Meeting-length capture (FUN-100) is
/// a separate post-1.0 capability and must not raise this bound implicitly.
pub const MAX_CAPTURE_SECONDS: u32 = 300;

/// Derived frame/IPC bound for [`MAX_CAPTURE_SECONDS`] at
/// [`CAPTURE_SAMPLE_RATE_HZ`]. Mono f32 PCM is 4 bytes per sample, so the
/// whole-utterance staging bound for one session is
/// `MAX_CAPTURE_SAMPLES * size_of::<f32>()` bytes per full buffer copy.
pub const MAX_CAPTURE_SAMPLES: usize =
    CAPTURE_SAMPLE_RATE_HZ as usize * MAX_CAPTURE_SECONDS as usize;

/// Why capture stopped accepting audio. This never carries sample data.
///
/// Produced by the platform capture adapter, retained by portable recovery
/// finalization, journaled durably and rendered for the user. The set is
/// closed: a new variant is a contract change requiring review.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureEnd {
    /// The user requested Stop. Every sample buffered before the graceful stop
    /// remains part of the finalized utterance.
    FinalizationRequested,
    /// Independent endpointing/VAD ended the utterance.
    EndpointDetected,
    /// The documented bounded maximum was reached. This is an explicit,
    /// truthful termination at the bound, never a silent clip.
    SafetyLimit,
    /// The concrete source ended before the bound; callers fail closed.
    SourceEnded,
}

/// The single cooperative cancellation signal for a dictation transaction.
///
/// It intentionally lives in portable types so capture, ASR, correction, and
/// delivery share one signal instead of constructing disconnected flags.
#[derive(Debug, Clone, Default)]
pub struct SessionCancellation(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl SessionCancellation {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// Cooperative request to stop accepting microphone audio and finalize the
/// utterance that has already been captured.
///
/// This is deliberately distinct from [`SessionCancellation`]. A finalize
/// request preserves captured audio and allows the pipeline to proceed to ASR;
/// cancellation is terminal and forbids ASR/correction/delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SessionFinalizationReason {
    UserRequested = 1,
    EndpointDetected = 2,
}

#[derive(Debug, Clone, Default)]
pub struct SessionFinalization(std::sync::Arc<std::sync::atomic::AtomicU8>);

impl SessionFinalization {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn request(&self, reason: SessionFinalizationReason) -> SessionFinalizationReason {
        let requested = reason as u8;
        match self.0.compare_exchange(
            0,
            requested,
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
        ) {
            Ok(_) => reason,
            Err(existing) => decode_finalization_reason(existing).unwrap_or(reason),
        }
    }

    #[must_use]
    pub fn is_requested(&self) -> bool {
        self.reason().is_some()
    }

    #[must_use]
    pub fn reason(&self) -> Option<SessionFinalizationReason> {
        decode_finalization_reason(self.0.load(std::sync::atomic::Ordering::SeqCst))
    }
}

fn decode_finalization_reason(raw: u8) -> Option<SessionFinalizationReason> {
    match raw {
        value if value == SessionFinalizationReason::UserRequested as u8 => {
            Some(SessionFinalizationReason::UserRequested)
        }
        value if value == SessionFinalizationReason::EndpointDetected as u8 => {
            Some(SessionFinalizationReason::EndpointDetected)
        }
        _ => None,
    }
}

/// Shared control signals for one dictation transaction.
///
/// Capture observes both signals. Later stages observe only `cancellation`:
/// once capture has finalized, a Stop request has done its job and must not be
/// reinterpreted as cancellation. Callers must check cancellation before and
/// after observing finalization so cancellation wins a concurrent race.
#[derive(Debug, Clone, Default)]
pub struct SessionControl {
    cancellation: SessionCancellation,
    finalization: SessionFinalization,
}

impl SessionControl {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn cancellation(&self) -> &SessionCancellation {
        &self.cancellation
    }

    #[must_use]
    pub fn finalization(&self) -> &SessionFinalization {
        &self.finalization
    }

    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    pub fn request_finalization(&self) {
        self.finalization
            .request(SessionFinalizationReason::UserRequested);
    }

    /// Request graceful finalization from endpoint policy. Returns true only
    /// when endpointing owns the first graceful-stop reason; a concurrent user
    /// Stop remains authoritative and is never overwritten.
    pub fn request_endpoint_finalization(&self) -> bool {
        self.finalization
            .request(SessionFinalizationReason::EndpointDetected)
            == SessionFinalizationReason::EndpointDetected
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }

    #[must_use]
    pub fn is_finalization_requested(&self) -> bool {
        self.finalization.is_requested()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SessionState {
    Idle,
    Preparing,
    Listening,
    FinalizingAudio,
    Transcribing,
    Transforming,
    Delivering,
    Committed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ApplicationIdentity {
    pub app_id: String,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TargetContext {
    pub application: Option<ApplicationIdentity>,
    pub window_title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AudioChunk {
    pub sample_rate_hz: u32,
    pub channels: u16,
    pub samples: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Transcript {
    pub text: String,
    pub is_final: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Capability {
    GlobalShortcutPressRelease,
    ActiveApplication,
    SelectedText,
    SurroundingText,
    TextInsert,
    TextReplaceSelection,
    Overlay,
    MediaControl,
    SecretStore,
    SystemAudioCapture,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProcessingBoundary {
    Local,
    LocalNetwork,
    Remote,
}
use serde::{Deserialize, Serialize};

/// Stable wire/storage identifier for one dictation transaction.
///
/// Opaque string (prefix + monotonic counter + random suffix at the creator),
/// never parsed back into meaning by domain code.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TransactionId(pub String);

impl TransactionId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for TransactionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Monotonic per-session event sequence number.
///
/// Events with a sequence lower than the highest observed are stale and must
/// be rejected by consumers; ordering is total within one session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EventSeq(pub u64);

/// One session state transition observation with transaction identity and
/// ordering, as required by REL-001 (explicit, testable state machine) and the
/// IPC protocol contract ("clients can reject stale events").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionEvent {
    pub session_id: SessionId,
    pub transaction_id: Option<TransactionId>,
    pub seq: EventSeq,
    pub from: SessionState,
    pub to: SessionState,
    #[serde(default)]
    pub at_unix_ms: u64,
}

/// Which representation of the transcript a stored variant holds (FUN-005).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TranscriptVariantKind {
    /// Verbatim engine output before any deterministic correction.
    Raw,
    /// After deterministic correction/formatting (guaranteed baseline).
    Corrected,
    /// The exact bytes handed to the delivery backend.
    Delivered,
}

/// Typed outcome of one delivery attempt (FUN-005/ADR-0016 fallback ladder).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeliveryOutcome {
    /// The configured primary destination accepted the payload and the
    /// concrete adapter verified its durable result.
    Delivered,
    /// A direct helper accepted the payload, but the adapter cannot verify
    /// that the focused destination retained it. This is an attempt fact, not
    /// a terminal delivery claim.
    SubmittedUnverified,
    /// Clipboard-only honesty: text is on the clipboard, insertion not proven.
    CopiedOnly,
    /// Stored in the Inbox/recovery journal — always non-destructive.
    InboxFallback,
    /// Terminal failure after all bounded attempts; text stays recoverable.
    Failed,
}

/// Why a direct delivery fell back (bounded, typed, no user content).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FallbackReason {
    NoQualifiedBackend,
    TargetUnavailable,
    BackendError { code: String },
    PolicyDenied,
    Timeout,
}

/// Record of one bounded delivery attempt (FUN-005).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryAttempt {
    pub backend: String,
    pub outcome: DeliveryOutcome,
    pub fallback_reason: Option<FallbackReason>,
    #[serde(default)]
    pub at_unix_ms: u64,
}

/// The persisted, pre-delivery declaration for a risky delivery boundary.
/// The transaction id is repeated deliberately: journal, delivery adapter and
/// resulting Inbox artifact must all agree on one opaque identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryIntent {
    pub transaction_id: TransactionId,
    pub backend: String,
}

/// Report returned by the concrete delivery adapter. It records only facts
/// observed by that adapter; orchestration must never synthesize an attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryReport {
    pub transaction_id: TransactionId,
    pub backend: String,
    pub attempts: Vec<DeliveryAttempt>,
    pub outcome: DeliveryOutcome,
    pub failure_reason: Option<FallbackReason>,
}

/// Raw/corrected/delivered transcript representations preserved per committed
/// session (FUN-005). Recoverable until explicitly terminal.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptVariants {
    pub raw: Option<String>,
    pub corrected: Option<String>,
    pub delivered: Option<String>,
    pub attempts: Vec<DeliveryAttempt>,
}

impl TranscriptVariants {
    /// A transaction is recoverable while its final text exists and no
    /// terminal failure was recorded.
    pub fn is_recoverable(&self) -> bool {
        self.raw.is_some() || self.corrected.is_some() || self.delivered.is_some()
    }

    pub fn last_outcome(&self) -> Option<&DeliveryOutcome> {
        self.attempts.last().map(|a| &a.outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::{CaptureEnd, DeliveryOutcome, SessionControl, SessionFinalizationReason};

    #[test]
    fn capture_bound_is_exact_bounded_and_typed() {
        assert_eq!(super::CAPTURE_SAMPLE_RATE_HZ, 16_000);
        assert_eq!(super::MAX_CAPTURE_SECONDS, 300);
        assert_eq!(super::MAX_CAPTURE_SAMPLES, 16_000 * 300);
        let sample_bytes = super::MAX_CAPTURE_SAMPLES * size_of::<f32>();
        assert_eq!(sample_bytes, 19_200_000);
        // The four reasons are distinct so status/journal rendering cannot
        // collapse a graceful stop into the explicit maximum.
        assert_ne!(CaptureEnd::FinalizationRequested, CaptureEnd::SafetyLimit);
        assert_ne!(CaptureEnd::EndpointDetected, CaptureEnd::SafetyLimit);
        assert_ne!(CaptureEnd::SourceEnded, CaptureEnd::SafetyLimit);
    }

    #[test]
    fn unverified_submission_is_not_verified_delivery() {
        assert_ne!(
            DeliveryOutcome::SubmittedUnverified,
            DeliveryOutcome::Delivered
        );
    }

    #[test]
    fn stop_and_cancel_are_independent_shared_signals() {
        let control = SessionControl::new();
        let observer = control.clone();

        control.request_finalization();
        assert!(observer.is_finalization_requested());
        assert_eq!(
            observer.finalization().reason(),
            Some(SessionFinalizationReason::UserRequested)
        );
        assert!(!observer.is_cancelled());

        observer.cancel();
        assert!(control.is_cancelled());
        assert!(control.is_finalization_requested());
    }

    #[test]
    fn first_graceful_finalization_origin_is_preserved() {
        let endpoint_first = SessionControl::new();
        assert!(endpoint_first.request_endpoint_finalization());
        endpoint_first.request_finalization();
        assert_eq!(
            endpoint_first.finalization().reason(),
            Some(SessionFinalizationReason::EndpointDetected)
        );

        let user_first = SessionControl::new();
        user_first.request_finalization();
        assert!(!user_first.request_endpoint_finalization());
        assert_eq!(
            user_first.finalization().reason(),
            Some(SessionFinalizationReason::UserRequested)
        );
    }
}
