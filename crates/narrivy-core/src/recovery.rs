//! Portable recovery ordering for strict Phase 2A/2B and controlled Phase 2C.
//!
//! This module deliberately contains no Linux process invocation, model path,
//! or delivery backend. Batch and streaming acquisition differ, but both
//! enforce the same final-transcript tail: policy -> correction -> durable
//! intent -> policy -> delivery -> terminal journal state. The streaming lane
//! authorizes raw audio before opening either capture or ASR.

use crate::{
    CoreError, EndpointDecision, Journal, JournalEntry, JournalEntryState, JournalError,
    StreamingEndpointing, unix_now_ms,
};
use narrivy_asr_api::{AsrEngine, AsrEvent, AsrRequest, AsrStream, OneShotAsrPort};
use narrivy_platform_api::{
    AudioCapturePort, CaptureStreamEvent, DeliveryPort, StreamingAudioCapturePort,
    StreamingAudioCaptureSession,
};
use narrivy_policy::{DataClass, ProcessingPolicy};
use narrivy_types::{
    DeliveryIntent, DeliveryOutcome, DeliveryReport, ProcessingBoundary, SessionCancellation,
    SessionControl, SessionId, SessionState, TransactionId,
};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const STREAM_CLEANUP_BOUND: Duration = Duration::from_secs(2);
/// Defensive content bound for one volatile partial hypothesis. Partials are
/// never persisted, but they still cross an in-process observer boundary.
pub const MAX_VOLATILE_PARTIAL_BYTES: usize = 16_384;

fn observe_timing(observer: Option<&dyn RecoveryTimingObserver>, stage: RecoveryTimingStage) {
    if let Some(observer) = observer {
        observer.observe(stage);
    }
}

/// Fixed content-free boundaries for controlled streaming diagnostics. These
/// stages never carry transaction identity, audio, transcript text, wall-clock
/// time, or persistence semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RecoveryTimingStage {
    CaptureStart,
    CaptureReady,
    AsrOpened,
    FirstAudioAccepted,
    FirstPartialAccepted,
    CaptureEnded,
    RawFinalAccepted,
    CorrectedDurablyCommitted,
    DeliveryReturned,
    TerminalJournalCommitted,
}

impl RecoveryTimingStage {
    pub const ALL: [Self; 10] = [
        Self::CaptureStart,
        Self::CaptureReady,
        Self::AsrOpened,
        Self::FirstAudioAccepted,
        Self::FirstPartialAccepted,
        Self::CaptureEnded,
        Self::RawFinalAccepted,
        Self::CorrectedDurablyCommitted,
        Self::DeliveryReturned,
        Self::TerminalJournalCommitted,
    ];

    const fn index(self) -> usize {
        self as usize
    }

    /// Stable content-free name used by IPC diagnostics and benchmarks.
    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::CaptureStart => "capture_start",
            Self::CaptureReady => "capture_ready",
            Self::AsrOpened => "asr_opened",
            Self::FirstAudioAccepted => "first_audio_accepted",
            Self::FirstPartialAccepted => "first_partial_accepted",
            Self::CaptureEnded => "capture_ended",
            Self::RawFinalAccepted => "raw_final_accepted",
            Self::CorrectedDurablyCommitted => "corrected_durably_committed",
            Self::DeliveryReturned => "delivery_returned",
            Self::TerminalJournalCommitted => "terminal_journal_committed",
        }
    }
}

/// Optional infallible timing sink. Production composition may expose its
/// content-free offsets for local diagnostics and controlled benchmarks.
pub trait RecoveryTimingObserver: Send + Sync {
    fn observe(&self, stage: RecoveryTimingStage);
}

/// Allocation-free monotonic trace for one controlled transaction. The first
/// observation of each fixed stage wins. A zero atomic means absent; stored
/// offsets are encoded as `microseconds + 1` so an actual zero is representable.
pub struct RecoveryTimingTrace {
    origin: Instant,
    offsets_us_plus_one: [AtomicU64; RecoveryTimingStage::ALL.len()],
}

/// Content-free result of reconciling durable committed transactions after a
/// daemon restart. Incomplete capture records stay in the journal for exact
/// recovery reads; only final corrected text is copied to the Inbox.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StartupRecoveryReport {
    pub recovered: usize,
    pub incomplete: usize,
    pub delivery_failed: usize,
}

/// Recover committed-but-unterminated results to the local Inbox.
///
/// Startup recovery deliberately never retries an external delivery backend:
/// a process may have crashed after insertion but before its terminal journal
/// write. The caller supplies an Inbox-only adapter, making replay durable,
/// bounded by journal retention, and idempotent under the transaction id.
pub fn recover_unsent_to_inbox(
    journal: &Journal,
    inbox: &mut dyn DeliveryPort,
) -> Result<StartupRecoveryReport, CoreError> {
    let mut summary = StartupRecoveryReport::default();
    for mut entry in journal.recoverable_entries()? {
        if !matches!(
            entry.state,
            JournalEntryState::Committed | JournalEntryState::FailedRecoverable
        ) {
            summary.incomplete += 1;
            continue;
        }
        let Some(corrected) = entry.variants.corrected.as_deref() else {
            summary.incomplete += 1;
            continue;
        };
        if corrected.is_empty()
            || entry
                .delivery_intent
                .as_ref()
                .is_none_or(|intent| intent.transaction_id.as_str() != entry.txid)
        {
            summary.incomplete += 1;
            continue;
        }

        let intent = DeliveryIntent {
            transaction_id: TransactionId::new(entry.txid.clone()),
            backend: "inbox".to_owned(),
        };
        let report = match inbox.deliver(&intent, corrected, &SessionCancellation::default()) {
            Ok(report)
                if report.transaction_id == intent.transaction_id
                    && report.backend == "inbox"
                    && report.outcome == DeliveryOutcome::Delivered =>
            {
                report
            }
            Ok(_) | Err(_) => {
                summary.delivery_failed += 1;
                continue;
            }
        };

        entry.variants.delivered = Some(corrected.to_owned());
        entry.variants.attempts = report.attempts.clone();
        entry.delivery_report = Some(report);
        entry.failure_code = None;
        entry.state = JournalEntryState::InboxRecovered;
        entry.updated_unix_ms = unix_now_ms();
        journal.write(&entry)?;
        summary.recovered += 1;
    }
    Ok(summary)
}

impl RecoveryTimingTrace {
    #[must_use]
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
            offsets_us_plus_one: [const { AtomicU64::new(0) }; RecoveryTimingStage::ALL.len()],
        }
    }

    #[must_use]
    pub fn offset_us(&self, stage: RecoveryTimingStage) -> Option<u64> {
        self.offsets_us_plus_one[stage.index()]
            .load(Ordering::Acquire)
            .checked_sub(1)
    }

    #[must_use]
    pub fn observed_count(&self) -> usize {
        RecoveryTimingStage::ALL
            .into_iter()
            .filter(|stage| self.offset_us(*stage).is_some())
            .count()
    }

    /// Snapshot all observed monotonic offsets without transaction content.
    #[must_use]
    pub fn snapshot_us(&self) -> BTreeMap<String, u64> {
        RecoveryTimingStage::ALL
            .into_iter()
            .filter_map(|stage| {
                self.offset_us(stage)
                    .map(|offset| (stage.wire_name().to_owned(), offset))
            })
            .collect()
    }
}

impl Default for RecoveryTimingTrace {
    fn default() -> Self {
        Self::new()
    }
}

impl RecoveryTimingObserver for RecoveryTimingTrace {
    fn observe(&self, stage: RecoveryTimingStage) {
        let elapsed_us = u64::try_from(self.origin.elapsed().as_micros()).unwrap_or(u64::MAX - 1);
        let encoded = elapsed_us.saturating_add(1);
        let _ = self.offsets_us_plus_one[stage.index()].compare_exchange(
            0,
            encoded,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }
}

/// One validated, non-final ASR hypothesis. Identity belongs to the caller's
/// transaction and is attached by the daemon at its IPC boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolatilePartial {
    pub seq: u64,
    pub text: String,
}

/// Deterministic, portable correction boundary.
pub trait CorrectionPort: Send {
    fn correct(
        &mut self,
        raw: &str,
        cancellation: &SessionCancellation,
    ) -> Result<String, CoreError>;
}

/// Exact terminal outcome persisted by the recovery slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryTerminal {
    DeliveredInbox(DeliveryReport),
    InboxRecovered(DeliveryReport),
    /// A routed direct rung handed the whole payload to its helper, which
    /// accepted it, but insertion cannot be verified. No Inbox copy is kept
    /// (F-M4); the journal records the report's exact attempts.
    SubmittedUnverified(DeliveryReport),
    Cancelled,
    FailedRecoverable {
        code: String,
    },
    FailedTerminal {
        code: String,
    },
}

/// Audio a microphone must have captured before it disappeared for the
/// dictation to be recognized and kept (D6, F-H2): half a second.
pub const SOURCE_ENDED_MIN_SAMPLES: u64 = narrivy_types::CAPTURE_SAMPLE_RATE_HZ as u64 / 2;

/// Failure code of a dictation whose microphone disappeared mid-way. With at
/// least [`SOURCE_ENDED_MIN_SAMPLES`] its text is kept in the journal (and
/// History) without delivery; below that nothing is kept.
pub const CAPTURE_SOURCE_ENDED: &str = "capture_source_ended";

/// The portable delivery truth contract selected by a strict composition.
/// Linux-specific process invocation remains behind [`DeliveryPort`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryDeliveryMode {
    /// Phase 2A: Inbox is the configured primary destination and its durable
    /// write is the adapter's verified delivery result.
    InboxPrimary,
    /// Phase 2B: a direct submission is unverified and Inbox is mandatory
    /// recovery. It can never become [`DeliveryOutcome::Delivered`].
    WtypeThenInboxRecovery,
    /// Phase 2C controlled contract: one portal/EIS text-frame attempt is
    /// unverified and Inbox is mandatory recovery. No production composition
    /// selects this mode in the current increment.
    PortalEisThenInboxRecovery,
    /// Target-aware routed delivery: a bounded ladder of *qualified*
    /// compatible backends is attempted in priority order; the first backend
    /// that can VERIFY insertion is the terminal `Delivered` fact, and any
    /// other outcome is followed by exactly one mandatory durable Inbox
    /// recovery write under the same transaction identity. The adapter reports
    /// which rungs ran and why an unqualified rung was skipped; a session can
    /// never claim verified insertion merely because a helper was spawned.
    TargetRoutedThenInboxRecovery,
}

impl RecoveryDeliveryMode {
    fn intent_backend(self) -> &'static str {
        match self {
            Self::InboxPrimary => "inbox",
            Self::WtypeThenInboxRecovery => "wtype-then-inbox",
            Self::PortalEisThenInboxRecovery => "portal-eis-then-inbox",
            Self::TargetRoutedThenInboxRecovery => "routed-then-inbox",
        }
    }

    fn recovery_attempt_backend(self) -> Option<&'static str> {
        match self {
            Self::InboxPrimary => None,
            Self::WtypeThenInboxRecovery => Some("wtype-stdin"),
            Self::PortalEisThenInboxRecovery => Some("portal-eis-text"),
            Self::TargetRoutedThenInboxRecovery => None,
        }
    }

    /// True for the bounded multi-rung routed composition. Its report shape is
    /// validated by [`is_target_routed_delivery_report`] instead of the single
    /// direct-plus-Inbox shape of the Phase 2B/2C modes.
    fn is_target_routed(self) -> bool {
        matches!(self, Self::TargetRoutedThenInboxRecovery)
    }
}

/// Typed port composition for one portable recovery session.
pub struct RecoverySlice<'a> {
    pub journal: &'a Journal,
    pub capture: &'a mut dyn AudioCapturePort,
    pub asr: &'a mut dyn OneShotAsrPort,
    pub policy: &'a dyn ProcessingPolicy,
    pub corrector: &'a mut dyn CorrectionPort,
    pub delivery: &'a mut dyn DeliveryPort,
    pub delivery_mode: RecoveryDeliveryMode,
}

/// A live preview while a whole-recording engine records (F3): the slice
/// hands it the audio so far as it arrives, and ends it before the final
/// recognition starts, so the preview never holds the engine then.
pub trait LivePreviewPort {
    /// Every sample captured so far. Called often; must return quickly.
    fn audio(&mut self, samples: &[f32]);
    /// Recording ended: stop previewing and wait for any preview decode in
    /// flight to return.
    fn finish(&mut self);
}

/// Portable live capture -> streaming ASR composition for one bounded recovery
/// transaction. The daemon does not select this path until its concrete model,
/// capture adapter, and lifecycle behavior have been qualified together.
pub struct StreamingRecoverySlice<'a> {
    pub journal: &'a Journal,
    pub capture: &'a mut dyn StreamingAudioCapturePort,
    pub asr: &'a mut dyn AsrEngine,
    pub policy: &'a dyn ProcessingPolicy,
    pub corrector: &'a mut dyn CorrectionPort,
    pub delivery: &'a mut dyn DeliveryPort,
    pub delivery_mode: RecoveryDeliveryMode,
    /// Independently replaceable VAD + endpoint modules. `None` retains the
    /// bounded safety-window behavior for models/pipelines without endpointing.
    pub endpointing: Option<StreamingEndpointing<'a>>,
    /// Optional volatile observer. It must not persist, log, correct, or
    /// deliver the hypothesis. `None` is the final-only presentation path.
    pub partial_observer: Option<&'a mut dyn FnMut(VolatilePartial) -> Result<(), &'static str>>,
}

impl RecoverySlice<'_> {
    /// Execute a session with the supplied opaque transaction identity.
    ///
    /// All stage failures are journaled before being returned as a terminal
    /// outcome. Only journal I/O failures escape as [`CoreError`].
    pub fn run(
        &mut self,
        session: SessionId,
        txid: TransactionId,
        pipeline_id: String,
        control: &SessionControl,
        observe: &mut dyn FnMut(SessionState) -> Result<(), CoreError>,
    ) -> Result<RecoveryTerminal, CoreError> {
        self.run_inner(
            session,
            txid,
            pipeline_id,
            control,
            observe,
            None,
            &AsrRequest::default(),
        )
    }

    /// Execute with content-free monotonic stage diagnostics.
    pub fn run_with_timing(
        &mut self,
        session: SessionId,
        txid: TransactionId,
        pipeline_id: String,
        control: &SessionControl,
        observe: &mut dyn FnMut(SessionState) -> Result<(), CoreError>,
        timing: &dyn RecoveryTimingObserver,
    ) -> Result<RecoveryTerminal, CoreError> {
        self.run_inner(
            session,
            txid,
            pipeline_id,
            control,
            observe,
            Some(timing),
            &AsrRequest::default(),
        )
    }

    /// Execute a session with an explicit recognition request.
    pub fn run_with_request(
        &mut self,
        session: SessionId,
        txid: TransactionId,
        pipeline_id: String,
        control: &SessionControl,
        observe: &mut dyn FnMut(SessionState) -> Result<(), CoreError>,
        request: &AsrRequest,
    ) -> Result<RecoveryTerminal, CoreError> {
        self.run_inner(session, txid, pipeline_id, control, observe, None, request)
    }

    /// Execute with an explicit recognition request and content-free timing.
    #[allow(clippy::too_many_arguments)]
    pub fn run_with_request_and_timing(
        &mut self,
        session: SessionId,
        txid: TransactionId,
        pipeline_id: String,
        control: &SessionControl,
        observe: &mut dyn FnMut(SessionState) -> Result<(), CoreError>,
        request: &AsrRequest,
        timing: &dyn RecoveryTimingObserver,
    ) -> Result<RecoveryTerminal, CoreError> {
        self.run_inner(
            session,
            txid,
            pipeline_id,
            control,
            observe,
            Some(timing),
            request,
        )
    }

    /// [`Self::run_with_request_and_timing`] with a live preview while
    /// recording.
    #[allow(clippy::too_many_arguments)]
    pub fn run_with_preview(
        &mut self,
        session: SessionId,
        txid: TransactionId,
        pipeline_id: String,
        control: &SessionControl,
        observe: &mut dyn FnMut(SessionState) -> Result<(), CoreError>,
        request: &AsrRequest,
        timing: &dyn RecoveryTimingObserver,
        preview: &mut dyn LivePreviewPort,
    ) -> Result<RecoveryTerminal, CoreError> {
        self.run_previewed(
            session,
            txid,
            pipeline_id,
            control,
            observe,
            Some(timing),
            request,
            Some(preview),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn run_inner(
        &mut self,
        session: SessionId,
        txid: TransactionId,
        pipeline_id: String,
        control: &SessionControl,
        observe: &mut dyn FnMut(SessionState) -> Result<(), CoreError>,
        timing: Option<&dyn RecoveryTimingObserver>,
        request: &AsrRequest,
    ) -> Result<RecoveryTerminal, CoreError> {
        self.run_previewed(
            session,
            txid,
            pipeline_id,
            control,
            observe,
            timing,
            request,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn run_previewed(
        &mut self,
        session: SessionId,
        txid: TransactionId,
        pipeline_id: String,
        control: &SessionControl,
        observe: &mut dyn FnMut(SessionState) -> Result<(), CoreError>,
        timing: Option<&dyn RecoveryTimingObserver>,
        request: &AsrRequest,
        mut preview: Option<&mut dyn LivePreviewPort>,
    ) -> Result<RecoveryTerminal, CoreError> {
        let cancellation = control.cancellation();
        let mut entry = self
            .journal
            .read(txid.as_str())?
            .unwrap_or_else(|| JournalEntry::new_pending(txid.as_str(), unix_now_ms()));
        entry.pipeline_id = Some(pipeline_id);
        self.journal.write(&entry)?;

        if cancellation.is_cancelled() {
            return self.cancel(entry, observe);
        }

        observe_timing(timing, RecoveryTimingStage::CaptureStart);
        let mut capture = match self.capture.start() {
            Ok(capture) => capture,
            Err(err) if cancellation.is_cancelled() && err.code == "cancelled" => {
                return self.cancel(entry, observe);
            }
            Err(err) => return self.fail_terminal(entry, err.code, observe),
        };
        if let Err(err) = capture.wait_ready(control) {
            if cancellation.is_cancelled() && err.code == "cancelled" {
                return self.cancel(entry, observe);
            }
            return self.fail_terminal(entry, err.code, observe);
        }
        observe_timing(timing, RecoveryTimingStage::CaptureReady);
        // Readiness requires a complete frame from the selected source;
        // spawning a recorder process or accepting IPC is insufficient.
        self.observe_before_side_effect(&mut entry, SessionState::Listening, observe)?;
        let collected = match preview.as_deref_mut() {
            Some(preview) => {
                capture.collect_observed(control, &mut |samples| preview.audio(samples))
            }
            None => capture.collect(control),
        };
        // The preview ends before recognition, on every path.
        if let Some(preview) = preview {
            preview.finish();
        }
        let captured = match collected {
            Ok(captured) => captured,
            Err(err) if cancellation.is_cancelled() && err.code == "cancelled" => {
                return self.cancel(entry, observe);
            }
            Err(err) => return self.fail_terminal(entry, err.code, observe),
        };
        observe_timing(timing, RecoveryTimingStage::CaptureEnded);

        if cancellation.is_cancelled() {
            return self.cancel(entry, observe);
        }
        // The typed end reason is durable transaction truth, not a transient
        // adapter detail: record it before ASR, correction or delivery can run
        // so a later failure still tells the user how recording actually ended.
        entry.capture_end = Some(captured.end);
        entry.updated_unix_ms = unix_now_ms();
        self.journal.write(&entry)?;
        // Portable defense in depth: the capture adapter owns its window, but a
        // misbehaving port must never push an unbounded frame count into ASR.
        if captured.audio.samples.len() > narrivy_types::MAX_CAPTURE_SAMPLES {
            return self.fail_terminal(entry, "capture_output_overflow", observe);
        }
        // A recorder that stopped before its bound (the microphone was
        // unplugged, PipeWire restarted) is durable typed truth, not a
        // successful capture. Too little audio fails closed before ASR; with
        // enough, what was said is recognized and kept, never delivered:
        // the user may no longer be in the window it was meant for (D6).
        let source_ended = captured.end == narrivy_platform_api::CaptureEnd::SourceEnded;
        if source_ended && (captured.audio.samples.len() as u64) < SOURCE_ENDED_MIN_SAMPLES {
            return self.fail_terminal(entry, CAPTURE_SOURCE_ENDED, observe);
        }
        self.observe_before_side_effect(&mut entry, SessionState::FinalizingAudio, observe)?;
        let audio = captured.audio;
        if self
            .policy
            .allow_data(DataClass::AudioRaw, ProcessingBoundary::Local)
            .is_err()
        {
            return self.fail_terminal(entry, "policy_denied_audio", observe);
        }

        self.observe_before_side_effect(&mut entry, SessionState::Transcribing, observe)?;
        observe_timing(timing, RecoveryTimingStage::AsrOpened);
        observe_timing(timing, RecoveryTimingStage::FirstAudioAccepted);
        let transcript = match self
            .asr
            .recognize_with(session, &audio, cancellation, request)
        {
            Ok(transcript) => transcript,
            Err(err) if cancellation.is_cancelled() && err.code == "cancelled" => {
                return self.cancel(entry, observe);
            }
            Err(err) => return self.fail_terminal(entry, err.code, observe),
        };
        if transcript.text.trim().is_empty() {
            return self.fail_terminal(entry, "empty_transcript", observe);
        }
        observe_timing(timing, RecoveryTimingStage::RawFinalAccepted);
        let mut tail = RecoveryTail {
            journal: self.journal,
            policy: self.policy,
            corrector: self.corrector,
            delivery: self.delivery,
            delivery_mode: self.delivery_mode,
        };
        if source_ended {
            return tail.keep_undelivered(entry, transcript.text, cancellation, observe);
        }
        tail.complete(entry, transcript.text, txid, cancellation, observe, timing)
    }

    fn cancel(
        &self,
        mut entry: JournalEntry,
        observe: &mut dyn FnMut(SessionState) -> Result<(), CoreError>,
    ) -> Result<RecoveryTerminal, CoreError> {
        entry.state = JournalEntryState::Cancelled;
        entry.failure_code = Some("cancelled".to_owned());
        entry.updated_unix_ms = unix_now_ms();
        self.journal.write(&entry)?;
        observe(SessionState::Cancelled)?;
        observe(SessionState::Idle)?;
        Ok(RecoveryTerminal::Cancelled)
    }

    fn fail_terminal(
        &self,
        mut entry: JournalEntry,
        code: &str,
        observe: &mut dyn FnMut(SessionState) -> Result<(), CoreError>,
    ) -> Result<RecoveryTerminal, CoreError> {
        entry.state = JournalEntryState::FailedTerminal;
        entry.failure_code = Some(code.to_owned());
        entry.updated_unix_ms = unix_now_ms();
        self.journal.write(&entry)?;
        observe(SessionState::Failed)?;
        observe(SessionState::Idle)?;
        Ok(RecoveryTerminal::FailedTerminal {
            code: code.to_owned(),
        })
    }

    /// Publish a stage before allowing its side effect. A rejected lifecycle
    /// transition must stop orchestration and leave a durable truthful row;
    /// it can never be ignored while ASR or delivery continues.
    fn observe_before_side_effect(
        &self,
        entry: &mut JournalEntry,
        state: SessionState,
        observe: &mut dyn FnMut(SessionState) -> Result<(), CoreError>,
    ) -> Result<(), CoreError> {
        if let Err(error) = observe(state) {
            entry.state = if entry.state == JournalEntryState::Committed {
                JournalEntryState::FailedRecoverable
            } else {
                JournalEntryState::FailedTerminal
            };
            entry.failure_code = Some("state_observation_failed".to_owned());
            entry.updated_unix_ms = unix_now_ms();
            self.journal.write(entry)?;
            return Err(error);
        }
        Ok(())
    }
}

impl StreamingRecoverySlice<'_> {
    fn partial_observer(
        &mut self,
    ) -> Option<&mut (dyn FnMut(VolatilePartial) -> Result<(), &'static str> + '_)> {
        self.partial_observer
            .as_mut()
            .map(|observer| &mut **observer as &mut _)
    }

    /// Execute the controlled live-capture path. Capture blocks are fed to the
    /// ASR stream as they arrive; partial hypotheses are validation-only and
    /// are never persisted or passed to correction/delivery.
    pub fn run(
        &mut self,
        session: SessionId,
        txid: TransactionId,
        pipeline_id: String,
        control: &SessionControl,
        observe: &mut dyn FnMut(SessionState) -> Result<(), CoreError>,
    ) -> Result<RecoveryTerminal, CoreError> {
        self.run_inner(
            session,
            txid,
            pipeline_id,
            control,
            observe,
            None,
            &AsrRequest::default(),
        )
    }

    /// Execute with an ephemeral, content-free monotonic observer. Callers may
    /// expose its offsets for local diagnostics; a reviewed procedure remains
    /// responsible for turning those observations into qualification evidence.
    pub fn run_with_timing(
        &mut self,
        session: SessionId,
        txid: TransactionId,
        pipeline_id: String,
        control: &SessionControl,
        observe: &mut dyn FnMut(SessionState) -> Result<(), CoreError>,
        timing: &dyn RecoveryTimingObserver,
    ) -> Result<RecoveryTerminal, CoreError> {
        self.run_inner(
            session,
            txid,
            pipeline_id,
            control,
            observe,
            Some(timing),
            &AsrRequest::default(),
        )
    }

    /// Execute streaming recovery with an explicit recognition request and
    /// content-free timing.
    #[allow(clippy::too_many_arguments)]
    pub fn run_with_request_and_timing(
        &mut self,
        session: SessionId,
        txid: TransactionId,
        pipeline_id: String,
        control: &SessionControl,
        observe: &mut dyn FnMut(SessionState) -> Result<(), CoreError>,
        request: &AsrRequest,
        timing: &dyn RecoveryTimingObserver,
    ) -> Result<RecoveryTerminal, CoreError> {
        self.run_inner(
            session,
            txid,
            pipeline_id,
            control,
            observe,
            Some(timing),
            request,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn run_inner(
        &mut self,
        session: SessionId,
        txid: TransactionId,
        pipeline_id: String,
        control: &SessionControl,
        observe: &mut dyn FnMut(SessionState) -> Result<(), CoreError>,
        timing: Option<&dyn RecoveryTimingObserver>,
        request: &AsrRequest,
    ) -> Result<RecoveryTerminal, CoreError> {
        let cancellation = control.cancellation();
        let mut entry = self
            .journal
            .read(txid.as_str())?
            .unwrap_or_else(|| JournalEntry::new_pending(txid.as_str(), unix_now_ms()));
        entry.pipeline_id = Some(pipeline_id);
        self.journal.write(&entry)?;

        if cancellation.is_cancelled() {
            return self.tail().cancel(entry, observe);
        }
        // Raw audio must be permitted before either capture or ASR receives
        // data. This is deliberately earlier than the legacy batch lane.
        if self
            .policy
            .allow_data(DataClass::AudioRaw, ProcessingBoundary::Local)
            .is_err()
        {
            return self
                .tail()
                .fail_terminal(entry, "policy_denied_audio", observe);
        }
        if !self.asr.capabilities().streaming {
            return self
                .tail()
                .fail_terminal(entry, "streaming_model_unsupported", observe);
        }

        // Start capture before opening the model stream. Native model admission
        // can take long enough to clip speech that begins immediately after
        // StartSession. `wait_ready` retains the first complete chunk, while
        // the concrete bounded queue continues accepting contiguous audio.
        observe_timing(timing, RecoveryTimingStage::CaptureStart);
        let mut capture = match self.capture.start_stream(control) {
            Ok(capture) => capture,
            Err(err) => {
                if cancellation.is_cancelled() && err.code == "cancelled" {
                    return self.tail().cancel(entry, observe);
                }
                return self.tail().fail_terminal(entry, err.code, observe);
            }
        };

        if let Err(err) = capture.wait_ready(control) {
            let cancelled = cancellation.is_cancelled() && err.code == "cancelled";
            let cleanup = cleanup_capture(&mut *capture);
            if let Some(code) = cleanup {
                return self.tail().fail_terminal(entry, code, observe);
            }
            if cancelled {
                return self.tail().cancel(entry, observe);
            }
            return self.tail().fail_terminal(entry, err.code, observe);
        }
        observe_timing(timing, RecoveryTimingStage::CaptureReady);

        let capture_info = capture.info().clone();
        if capture_info.sample_rate_hz == 0 || capture_info.channels == 0 {
            let cleanup = cleanup_capture(&mut *capture);
            return self.tail().fail_terminal(
                entry,
                cleanup.unwrap_or("capture_format_invalid"),
                observe,
            );
        }

        // Listening means real source flow is active. It does not claim that
        // resident model admission has completed.
        if let Err(error) =
            self.tail()
                .observe_before_side_effect(&mut entry, SessionState::Listening, observe)
        {
            if let Some(code) = cleanup_capture(&mut *capture) {
                entry.state = JournalEntryState::FailedTerminal;
                entry.failure_code = Some(code.to_owned());
                entry.updated_unix_ms = unix_now_ms();
                self.journal.write(&entry)?;
                return Err(CoreError::Platform(narrivy_platform_api::PlatformError {
                    code,
                    message: "stream cleanup failed after lifecycle observation rejection"
                        .to_owned(),
                }));
            }
            return Err(error);
        }

        if cancellation.is_cancelled() {
            let cleanup = cleanup_capture(&mut *capture);
            if let Some(code) = cleanup {
                return self.tail().fail_terminal(entry, code, observe);
            }
            return self.tail().cancel(entry, observe);
        }

        let mut asr = match self.asr.open_stream_with(session, cancellation, request) {
            Ok(stream) => stream,
            Err(err) => {
                let cancelled = cancellation.is_cancelled() && err.code == "cancelled";
                let cleanup = cleanup_capture(&mut *capture);
                if let Some(code) = cleanup {
                    return self.tail().fail_terminal(entry, code, observe);
                }
                if cancelled {
                    return self.tail().cancel(entry, observe);
                }
                return self.tail().fail_terminal(entry, err.code, observe);
            }
        };
        observe_timing(timing, RecoveryTimingStage::AsrOpened);
        let mut tracker = AsrEventTracker::new(session);
        if let Err(code) = tracker.drain(
            &mut *asr,
            false,
            self.policy,
            self.partial_observer(),
            timing,
        ) {
            let cleanup = cleanup_active(&mut *capture, &mut *asr);
            return self
                .tail()
                .fail_terminal(entry, cleanup.unwrap_or(code), observe);
        }
        let mut expected_capture_sequence = 0_u64;
        let mut total_samples = 0_u64;
        let mut source_ended = false;
        let mut first_frame_after_start_us = None;
        let mut endpoint_requested = false;
        let mut first_audio_accepted = false;
        loop {
            if cancellation.is_cancelled() {
                return self.terminate_active(
                    entry,
                    &mut *capture,
                    &mut *asr,
                    "cancelled",
                    true,
                    observe,
                );
            }
            let event = match capture.next_event(control) {
                Ok(event) => event,
                Err(err) => {
                    return self.terminate_active(
                        entry,
                        &mut *capture,
                        &mut *asr,
                        err.code,
                        cancellation.is_cancelled() && err.code == "cancelled",
                        observe,
                    );
                }
            };
            match event {
                CaptureStreamEvent::Audio(chunk) => {
                    if chunk.sequence != expected_capture_sequence {
                        return self.terminate_active(
                            entry,
                            &mut *capture,
                            &mut *asr,
                            "capture_sequence_invalid",
                            false,
                            observe,
                        );
                    }
                    if chunk.audio.sample_rate_hz != capture_info.sample_rate_hz
                        || chunk.audio.channels != capture_info.channels
                        || chunk.audio.samples.is_empty()
                    {
                        return self.terminate_active(
                            entry,
                            &mut *capture,
                            &mut *asr,
                            "capture_format_invalid",
                            false,
                            observe,
                        );
                    }
                    if expected_capture_sequence == 0 {
                        first_frame_after_start_us = chunk.first_frame_after_start_us;
                    } else if chunk.first_frame_after_start_us.is_some() {
                        return self.terminate_active(
                            entry,
                            &mut *capture,
                            &mut *asr,
                            "capture_measurement_invalid",
                            false,
                            observe,
                        );
                    }
                    expected_capture_sequence = match expected_capture_sequence.checked_add(1) {
                        Some(next) => next,
                        None => {
                            return self.terminate_active(
                                entry,
                                &mut *capture,
                                &mut *asr,
                                "capture_sequence_overflow",
                                false,
                                observe,
                            );
                        }
                    };
                    total_samples = match total_samples
                        .checked_add(u64::try_from(chunk.audio.samples.len()).unwrap_or(u64::MAX))
                    {
                        Some(total) => total,
                        None => {
                            return self.terminate_active(
                                entry,
                                &mut *capture,
                                &mut *asr,
                                "capture_sample_count_overflow",
                                false,
                                observe,
                            );
                        }
                    };
                    // The adapter owns the capture window; this portable bound
                    // is defense in depth so a misbehaving producer can never
                    // grow one transaction without a finite frame budget.
                    if total_samples > narrivy_types::MAX_CAPTURE_SAMPLES as u64 {
                        return self.terminate_active(
                            entry,
                            &mut *capture,
                            &mut *asr,
                            "capture_sample_count_overflow",
                            false,
                            observe,
                        );
                    }
                    let endpoint_decision = if endpoint_requested {
                        EndpointDecision::Continue
                    } else if let Some(endpointing) = self.endpointing.as_mut() {
                        match endpointing.observe(&chunk.audio) {
                            Ok(decision) => decision,
                            Err(err) => {
                                return self.terminate_active(
                                    entry,
                                    &mut *capture,
                                    &mut *asr,
                                    err.code,
                                    false,
                                    observe,
                                );
                            }
                        }
                    } else {
                        EndpointDecision::Continue
                    };
                    if let Err(err) = asr.push_audio(&chunk.audio) {
                        let cancelled = cancellation.is_cancelled() && err.code == "cancelled";
                        return self.terminate_active(
                            entry,
                            &mut *capture,
                            &mut *asr,
                            err.code,
                            cancelled,
                            observe,
                        );
                    }
                    if !first_audio_accepted {
                        first_audio_accepted = true;
                        observe_timing(timing, RecoveryTimingStage::FirstAudioAccepted);
                    }
                    if let Err(code) = tracker.drain(
                        &mut *asr,
                        false,
                        self.policy,
                        self.partial_observer(),
                        timing,
                    ) {
                        let cancelled = cancellation.is_cancelled() && code == "cancelled";
                        return self.terminate_active(
                            entry,
                            &mut *capture,
                            &mut *asr,
                            code,
                            cancelled,
                            observe,
                        );
                    }
                    if endpoint_decision == EndpointDecision::Finalize {
                        // This is a graceful stop request, not cancellation.
                        // Capture may still emit already-buffered chunks; they
                        // remain ordered and are fed to ASR without asking the
                        // one-shot endpoint policy for a second decision.
                        endpoint_requested = control.request_endpoint_finalization();
                    }
                }
                CaptureStreamEvent::End(summary) => {
                    if endpoint_requested
                        && summary.end != narrivy_platform_api::CaptureEnd::EndpointDetected
                    {
                        return self.terminate_active(
                            entry,
                            &mut *capture,
                            &mut *asr,
                            "endpoint_finalization_not_observed",
                            false,
                            observe,
                        );
                    }
                    if summary.total_samples != total_samples
                        || summary.first_frame_after_start_us != first_frame_after_start_us
                        || total_samples == 0
                    {
                        let code = if total_samples == 0 {
                            "empty_capture_stream"
                        } else if summary.first_frame_after_start_us != first_frame_after_start_us {
                            "capture_measurement_invalid"
                        } else {
                            "capture_summary_mismatch"
                        };
                        return self.terminate_active(
                            entry,
                            &mut *capture,
                            &mut *asr,
                            code,
                            false,
                            observe,
                        );
                    }
                    observe_timing(timing, RecoveryTimingStage::CaptureEnded);
                    // Durable typed end reason for both the graceful Stop lane
                    // and the explicit documented maximum, so the client can
                    // render how capture ended without re-deriving OS state.
                    entry.capture_end = Some(summary.end);
                    entry.updated_unix_ms = unix_now_ms();
                    self.journal.write(&entry)?;
                    // A short EOF is durable typed truth, not a successful
                    // stream. Too little audio: cancel/finish the already-fed
                    // ASR stream and abort capture through the owned cleanup
                    // path, and nothing commits. With enough, the stream is
                    // finished like a Stop and its text kept undelivered (D6).
                    if summary.end == narrivy_platform_api::CaptureEnd::SourceEnded {
                        if total_samples < SOURCE_ENDED_MIN_SAMPLES {
                            return self.terminate_active(
                                entry,
                                &mut *capture,
                                &mut *asr,
                                CAPTURE_SOURCE_ENDED,
                                false,
                                observe,
                            );
                        }
                        source_ended = true;
                    }
                    break;
                }
            }
        }

        if cancellation.is_cancelled() {
            return self.terminate_active(
                entry,
                &mut *capture,
                &mut *asr,
                "cancelled",
                true,
                observe,
            );
        }
        if let Err(error) = self.tail().observe_before_side_effect(
            &mut entry,
            SessionState::FinalizingAudio,
            observe,
        ) {
            return self.observation_failed(entry, &mut *capture, &mut *asr, error);
        }
        if let Err(error) =
            self.tail()
                .observe_before_side_effect(&mut entry, SessionState::Transcribing, observe)
        {
            return self.observation_failed(entry, &mut *capture, &mut *asr, error);
        }
        if let Err(err) = asr.finalize() {
            return self.terminate_active(
                entry,
                &mut *capture,
                &mut *asr,
                err.code,
                cancellation.is_cancelled() && err.code == "cancelled",
                observe,
            );
        }
        if let Err(code) = tracker.drain(
            &mut *asr,
            true,
            self.policy,
            self.partial_observer(),
            timing,
        ) {
            return self.terminate_active(
                entry,
                &mut *capture,
                &mut *asr,
                code,
                cancellation.is_cancelled() && code == "cancelled",
                observe,
            );
        }
        let finish_deadline = cleanup_deadline();
        if let Err(err) = asr.finish(remaining(finish_deadline)) {
            let original = err.code;
            let cleanup = cleanup_asr_until(&mut *asr, finish_deadline);
            return self
                .tail()
                .fail_terminal(entry, cleanup.unwrap_or(original), observe);
        }
        if let Err(code) = tracker.drain(
            &mut *asr,
            true,
            self.policy,
            self.partial_observer(),
            timing,
        ) {
            return self.tail().fail_terminal(entry, code, observe);
        }
        let Some(transcript) = tracker.final_transcript else {
            return self
                .tail()
                .fail_terminal(entry, "asr_final_missing", observe);
        };
        if transcript.trim().is_empty() {
            return self
                .tail()
                .fail_terminal(entry, "empty_transcript", observe);
        }
        observe_timing(timing, RecoveryTimingStage::RawFinalAccepted);

        if source_ended {
            return self
                .tail()
                .keep_undelivered(entry, transcript, cancellation, observe);
        }
        self.tail()
            .complete(entry, transcript, txid, cancellation, observe, timing)
    }

    fn tail(&mut self) -> RecoveryTail<'_> {
        RecoveryTail {
            journal: self.journal,
            policy: self.policy,
            corrector: self.corrector,
            delivery: self.delivery,
            delivery_mode: self.delivery_mode,
        }
    }

    fn terminate_active(
        &mut self,
        entry: JournalEntry,
        capture: &mut dyn StreamingAudioCaptureSession,
        asr: &mut dyn AsrStream,
        original_code: &'static str,
        cancelled_before_cleanup: bool,
        observe: &mut dyn FnMut(SessionState) -> Result<(), CoreError>,
    ) -> Result<RecoveryTerminal, CoreError> {
        if let Some(cleanup_code) = cleanup_active(capture, asr) {
            return self.tail().fail_terminal(entry, cleanup_code, observe);
        }
        if cancelled_before_cleanup {
            self.tail().cancel(entry, observe)
        } else {
            self.tail().fail_terminal(entry, original_code, observe)
        }
    }

    fn observation_failed(
        &mut self,
        mut entry: JournalEntry,
        capture: &mut dyn StreamingAudioCaptureSession,
        asr: &mut dyn AsrStream,
        original: CoreError,
    ) -> Result<RecoveryTerminal, CoreError> {
        let Some(code) = cleanup_active(capture, asr) else {
            return Err(original);
        };
        entry.state = JournalEntryState::FailedTerminal;
        entry.failure_code = Some(code.to_owned());
        entry.updated_unix_ms = unix_now_ms();
        self.journal.write(&entry)?;
        Err(CoreError::Platform(narrivy_platform_api::PlatformError {
            code,
            message: "stream cleanup failed after lifecycle observation rejection".to_owned(),
        }))
    }
}

struct AsrEventTracker {
    session: SessionId,
    started: bool,
    last_sequence: Option<u64>,
    final_transcript: Option<String>,
}

impl AsrEventTracker {
    fn new(session: SessionId) -> Self {
        Self {
            session,
            started: false,
            last_sequence: None,
            final_transcript: None,
        }
    }

    fn drain(
        &mut self,
        stream: &mut dyn AsrStream,
        final_allowed: bool,
        policy: &dyn ProcessingPolicy,
        mut partial_observer: Option<&mut dyn FnMut(VolatilePartial) -> Result<(), &'static str>>,
        timing: Option<&dyn RecoveryTimingObserver>,
    ) -> Result<(), &'static str> {
        while let Some(event) = stream.poll_event() {
            match event {
                AsrEvent::Started { session } => {
                    if self.started || session != self.session || self.last_sequence.is_some() {
                        return Err("asr_event_order_invalid");
                    }
                    self.started = true;
                }
                AsrEvent::Partial { seq, transcript } => {
                    if !self.started
                        || transcript.is_final
                        || self.final_transcript.is_some()
                        || self.last_sequence.is_some_and(|last| seq <= last)
                    {
                        return Err("asr_event_order_invalid");
                    }
                    if transcript.text.is_empty()
                        || transcript.text.len() > MAX_VOLATILE_PARTIAL_BYTES
                    {
                        return Err("asr_partial_invalid");
                    }
                    if policy
                        .allow_data(DataClass::TranscriptRaw, ProcessingBoundary::Local)
                        .is_err()
                    {
                        return Err("policy_denied_transcript_partial");
                    }
                    self.last_sequence = Some(seq);
                    if let Some(observer) = partial_observer.as_deref_mut() {
                        observer(VolatilePartial {
                            seq,
                            text: transcript.text,
                        })?;
                    }
                    // Validation and the optional presentation boundary must
                    // both accept the hypothesis before timing can call it an
                    // accepted partial. Rejection cannot manufacture latency.
                    observe_timing(timing, RecoveryTimingStage::FirstPartialAccepted);
                }
                AsrEvent::Final {
                    seq, transcript, ..
                } => {
                    if !final_allowed
                        || !self.started
                        || !transcript.is_final
                        || self.final_transcript.is_some()
                        || self.last_sequence.is_some_and(|last| seq <= last)
                    {
                        return Err("asr_event_order_invalid");
                    }
                    self.last_sequence = Some(seq);
                    self.final_transcript = Some(transcript.text);
                }
                AsrEvent::Error(error) => return Err(error.code),
            }
        }
        Ok(())
    }
}

fn cleanup_deadline() -> Instant {
    Instant::now()
        .checked_add(STREAM_CLEANUP_BOUND)
        .unwrap_or_else(Instant::now)
}

fn remaining(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}

fn cleanup_asr_until(asr: &mut dyn AsrStream, deadline: Instant) -> Option<&'static str> {
    asr.cancel();
    asr.finish(remaining(deadline))
        .err()
        .map(|error| error.code)
}

fn cleanup_capture(capture: &mut dyn StreamingAudioCaptureSession) -> Option<&'static str> {
    capture
        .abort(cleanup_deadline())
        .err()
        .map(|error| error.code)
}

fn cleanup_active(
    capture: &mut dyn StreamingAudioCaptureSession,
    asr: &mut dyn AsrStream,
) -> Option<&'static str> {
    let deadline = cleanup_deadline();
    // Signal ASR immediately so capture cleanup and the engine's cooperative
    // stop overlap. Reaping remains parent-owned and shares one deadline.
    asr.cancel();
    let capture_error = capture.abort(deadline).err().map(|error| error.code);
    let asr_error = asr
        .finish(remaining(deadline))
        .err()
        .map(|error| error.code);
    capture_error.or(asr_error)
}

struct RecoveryTail<'a> {
    journal: &'a Journal,
    policy: &'a dyn ProcessingPolicy,
    corrector: &'a mut dyn CorrectionPort,
    delivery: &'a mut dyn DeliveryPort,
    delivery_mode: RecoveryDeliveryMode,
}

impl RecoveryTail<'_> {
    /// Journal the raw text and correct it: the part of [`Self::complete`]
    /// before the commit. `Err` carries the terminal that ended it.
    fn transform(
        &mut self,
        mut entry: JournalEntry,
        raw: String,
        cancellation: &SessionCancellation,
        observe: &mut dyn FnMut(SessionState) -> Result<(), CoreError>,
    ) -> Result<Result<(JournalEntry, String), RecoveryTerminal>, CoreError> {
        // Raw transcript persistence is itself a privacy boundary. Decide
        // before placing the raw representation in the recovery journal.
        if self
            .policy
            .allow_data(DataClass::TranscriptRaw, ProcessingBoundary::Local)
            .is_err()
        {
            return self
                .fail_terminal(entry, "policy_denied_transcript_raw", observe)
                .map(Err);
        }
        entry.variants.raw = Some(raw);
        entry.updated_unix_ms = unix_now_ms();
        self.journal.write(&entry)?;

        if cancellation.is_cancelled() {
            return self.cancel(entry, observe).map(Err);
        }
        self.observe_before_side_effect(&mut entry, SessionState::Transforming, observe)?;
        let corrected = match self.corrector.correct(
            entry.variants.raw.as_deref().unwrap_or_default(),
            cancellation,
        ) {
            Ok(text) => text,
            Err(CoreError::Cancelled) if cancellation.is_cancelled() => {
                return self.cancel(entry, observe).map(Err);
            }
            Err(_) => {
                return self
                    .fail_terminal(entry, "correction_failed", observe)
                    .map(Err);
            }
        };
        entry.variants.corrected = Some(corrected.clone());
        Ok(Ok((entry, corrected)))
    }

    /// The microphone disappeared mid-dictation (D6, F-H2): keep the text in
    /// the journal, from which History retains it, and deliver nothing. The
    /// terminal is recoverable with [`CAPTURE_SOURCE_ENDED`]; no delivery
    /// intent is written, so startup replay leaves it alone.
    fn keep_undelivered(
        &mut self,
        entry: JournalEntry,
        raw: String,
        cancellation: &SessionCancellation,
        observe: &mut dyn FnMut(SessionState) -> Result<(), CoreError>,
    ) -> Result<RecoveryTerminal, CoreError> {
        let (entry, _) = match self.transform(entry, raw, cancellation, observe)? {
            Ok(transformed) => transformed,
            Err(terminal) => return Ok(terminal),
        };
        self.fail_recoverable(entry, CAPTURE_SOURCE_ENDED, observe)
    }

    fn complete(
        &mut self,
        entry: JournalEntry,
        raw: String,
        txid: TransactionId,
        cancellation: &SessionCancellation,
        observe: &mut dyn FnMut(SessionState) -> Result<(), CoreError>,
        timing: Option<&dyn RecoveryTimingObserver>,
    ) -> Result<RecoveryTerminal, CoreError> {
        let (mut entry, corrected) = match self.transform(entry, raw, cancellation, observe)? {
            Ok(transformed) => transformed,
            Err(terminal) => return Ok(terminal),
        };

        entry.state = JournalEntryState::Committed;
        entry.delivery_intent = Some(DeliveryIntent {
            transaction_id: txid.clone(),
            backend: self.delivery_mode.intent_backend().to_owned(),
        });
        entry.updated_unix_ms = unix_now_ms();
        self.journal.write(&entry)?;
        observe_timing(timing, RecoveryTimingStage::CorrectedDurablyCommitted);

        if cancellation.is_cancelled() {
            return self.cancel(entry, observe);
        }
        if self
            .policy
            .allow_data(DataClass::TranscriptCorrected, ProcessingBoundary::Local)
            .is_err()
        {
            return self.fail_recoverable(entry, "policy_denied_delivery", observe);
        }

        self.observe_before_side_effect(&mut entry, SessionState::Delivering, observe)?;
        let intent = entry.delivery_intent.clone().ok_or_else(|| {
            CoreError::Journal(JournalError::Storage(
                "delivery intent disappeared before delivery".to_owned(),
            ))
        })?;
        let delivery_result = self.delivery.deliver(&intent, &corrected, cancellation);
        observe_timing(timing, RecoveryTimingStage::DeliveryReturned);
        let report = match delivery_result {
            Ok(report) => report,
            Err(err) if cancellation.is_cancelled() && err.code == "cancelled" => {
                return self.cancel(entry, observe);
            }
            Err(err) => return self.fail_recoverable(entry, err.code, observe),
        };
        if report.transaction_id != txid || report.backend != self.delivery_mode.intent_backend() {
            return self.fail_recoverable(entry, "delivery_identity_mismatch", observe);
        }

        if self.delivery_mode.is_target_routed() {
            // The adapter's observed attempts are persisted before their shape
            // is judged, exactly like the single-rung recovery modes: a
            // crashed/uncertain delivery must stay truthfully recoverable.
            entry.variants.attempts = report.attempts.clone();
            entry.delivery_report = Some(report.clone());
            entry.updated_unix_ms = unix_now_ms();
            if self.journal.write(&entry).is_err() {
                observe(SessionState::Failed)?;
                observe(SessionState::Idle)?;
                return Ok(RecoveryTerminal::FailedRecoverable {
                    code: "journal_write_after_delivery_uncertain".to_owned(),
                });
            }
            if !is_target_routed_delivery_report(self.delivery_mode, &report) {
                return self.fail_recoverable(
                    entry,
                    "delivery_outcome_not_routed_recovery",
                    observe,
                );
            }
            // Verified insertion, an accepted-but-unverified submission and
            // durable Inbox recovery are distinct terminal facts. All preserve
            // the exact corrected text that was handed to the delivery
            // adapter; none can be reached from another's report shape. A
            // submission is a finished delivery (it must never be replayed into
            // the Inbox at startup), and its journaled attempts keep the fact
            // that it was not verified.
            let outcome = report.outcome.clone();
            entry.variants.delivered = Some(corrected);
            entry.failure_code = None;
            entry.state = if outcome == DeliveryOutcome::InboxFallback {
                JournalEntryState::InboxRecovered
            } else {
                JournalEntryState::Delivered
            };
            entry.updated_unix_ms = unix_now_ms();
            if self.journal.write(&entry).is_err() {
                observe(SessionState::Failed)?;
                observe(SessionState::Idle)?;
                return Ok(RecoveryTerminal::FailedRecoverable {
                    code: "journal_write_after_delivery_uncertain".to_owned(),
                });
            }
            observe_timing(timing, RecoveryTimingStage::TerminalJournalCommitted);
            observe(SessionState::Committed)?;
            observe(SessionState::Idle)?;
            return Ok(match outcome {
                DeliveryOutcome::Delivered => RecoveryTerminal::DeliveredInbox(report),
                DeliveryOutcome::SubmittedUnverified => {
                    RecoveryTerminal::SubmittedUnverified(report)
                }
                _ => RecoveryTerminal::InboxRecovered(report),
            });
        }

        if self.delivery_mode.recovery_attempt_backend().is_some() {
            entry.variants.attempts = report.attempts.clone();
            entry.delivery_report = Some(report.clone());
            entry.updated_unix_ms = unix_now_ms();
            if self.journal.write(&entry).is_err() {
                observe(SessionState::Failed)?;
                observe(SessionState::Idle)?;
                return Ok(RecoveryTerminal::FailedRecoverable {
                    code: "journal_write_after_delivery_uncertain".to_owned(),
                });
            }
            if !is_direct_inbox_recovery_report(self.delivery_mode, &report) {
                return self.fail_recoverable(
                    entry,
                    "delivery_outcome_not_inbox_recovery",
                    observe,
                );
            }
            entry.variants.delivered = Some(corrected);
            entry.failure_code = None;
            entry.state = JournalEntryState::InboxRecovered;
            entry.updated_unix_ms = unix_now_ms();
            if self.journal.write(&entry).is_err() {
                observe(SessionState::Failed)?;
                observe(SessionState::Idle)?;
                return Ok(RecoveryTerminal::FailedRecoverable {
                    code: "journal_write_after_delivery_uncertain".to_owned(),
                });
            }
            observe_timing(timing, RecoveryTimingStage::TerminalJournalCommitted);
            observe(SessionState::Committed)?;
            observe(SessionState::Idle)?;
            return Ok(RecoveryTerminal::InboxRecovered(report));
        }

        if report.outcome != DeliveryOutcome::Delivered {
            return self.fail_recoverable(entry, "delivery_outcome_not_inbox", observe);
        }
        entry.variants.delivered = Some(corrected);
        entry.variants.attempts = report.attempts.clone();
        entry.delivery_report = Some(report.clone());
        entry.failure_code = None;
        entry.state = JournalEntryState::Delivered;
        entry.updated_unix_ms = unix_now_ms();
        if self.journal.write(&entry).is_err() {
            observe(SessionState::Failed)?;
            observe(SessionState::Idle)?;
            return Ok(RecoveryTerminal::FailedRecoverable {
                code: "journal_write_after_delivery_uncertain".to_owned(),
            });
        }
        observe_timing(timing, RecoveryTimingStage::TerminalJournalCommitted);
        observe(SessionState::Committed)?;
        observe(SessionState::Idle)?;
        Ok(RecoveryTerminal::DeliveredInbox(report))
    }

    fn cancel(
        &self,
        mut entry: JournalEntry,
        observe: &mut dyn FnMut(SessionState) -> Result<(), CoreError>,
    ) -> Result<RecoveryTerminal, CoreError> {
        entry.state = JournalEntryState::Cancelled;
        entry.failure_code = Some("cancelled".to_owned());
        entry.updated_unix_ms = unix_now_ms();
        self.journal.write(&entry)?;
        observe(SessionState::Cancelled)?;
        observe(SessionState::Idle)?;
        Ok(RecoveryTerminal::Cancelled)
    }

    fn fail_terminal(
        &self,
        mut entry: JournalEntry,
        code: &str,
        observe: &mut dyn FnMut(SessionState) -> Result<(), CoreError>,
    ) -> Result<RecoveryTerminal, CoreError> {
        entry.state = JournalEntryState::FailedTerminal;
        entry.failure_code = Some(code.to_owned());
        entry.updated_unix_ms = unix_now_ms();
        self.journal.write(&entry)?;
        observe(SessionState::Failed)?;
        observe(SessionState::Idle)?;
        Ok(RecoveryTerminal::FailedTerminal {
            code: code.to_owned(),
        })
    }

    fn fail_recoverable(
        &self,
        mut entry: JournalEntry,
        code: &str,
        observe: &mut dyn FnMut(SessionState) -> Result<(), CoreError>,
    ) -> Result<RecoveryTerminal, CoreError> {
        entry.state = JournalEntryState::FailedRecoverable;
        entry.failure_code = Some(code.to_owned());
        entry.updated_unix_ms = unix_now_ms();
        self.journal.write(&entry)?;
        observe(SessionState::Failed)?;
        observe(SessionState::Idle)?;
        Ok(RecoveryTerminal::FailedRecoverable {
            code: code.to_owned(),
        })
    }

    fn observe_before_side_effect(
        &self,
        entry: &mut JournalEntry,
        state: SessionState,
        observe: &mut dyn FnMut(SessionState) -> Result<(), CoreError>,
    ) -> Result<(), CoreError> {
        if let Err(error) = observe(state) {
            entry.state = if entry.state == JournalEntryState::Committed {
                JournalEntryState::FailedRecoverable
            } else {
                JournalEntryState::FailedTerminal
            };
            entry.failure_code = Some("state_observation_failed".to_owned());
            entry.updated_unix_ms = unix_now_ms();
            self.journal.write(entry)?;
            return Err(error);
        }
        Ok(())
    }
}

fn is_direct_inbox_recovery_report(mode: RecoveryDeliveryMode, report: &DeliveryReport) -> bool {
    let Some(direct_backend) = mode.recovery_attempt_backend() else {
        return false;
    };
    let [direct, inbox] = report.attempts.as_slice() else {
        return false;
    };
    report.outcome == DeliveryOutcome::InboxFallback
        && report.failure_reason.is_none()
        && report.backend == mode.intent_backend()
        && direct.backend == direct_backend
        && matches!(
            (&direct.outcome, &direct.fallback_reason),
            (DeliveryOutcome::SubmittedUnverified, None) | (DeliveryOutcome::Failed, Some(_))
        )
        && inbox.backend == "inbox"
        && inbox.outcome == DeliveryOutcome::InboxFallback
        && inbox.fallback_reason.is_none()
}

/// Validate one routed-ladder report against the exact contract the portable
/// tail can accept.
///
/// Accepted shapes:
/// - verified commit: at least one direct attempt, every attempt naming a
///   non-empty backend, no Inbox rung anywhere, and the last attempt
///   `Delivered` with no fallback reason (a direct backend that observed a
///   durable target result);
/// - unverified submission (F-M4): the same shape, with the report and its
///   last attempt `SubmittedUnverified` with no fallback reason and every
///   earlier attempt a bounded `Failed` (a helper accepted the whole payload;
///   no Inbox copy is kept);
/// - recovery: zero or more non-Inbox direct attempts, each `SubmittedUnverified`,
///   `CopiedOnly` or bounded `Failed`, followed by exactly one final durable
///   `inbox` attempt with `InboxFallback` and no failure reason.
///
/// `inbox` is the durable recovery rung only: it can never be a verified
/// external commit, so a `Delivered` report may not end on it. A verified
/// commit likewise carries no failure/fallback reason, and an attempt that
/// does not name the backend it observed is malformed.
///
/// `Delivered` is never accepted on an intermediate rung: the first verified
/// commit must end the ladder, so a stale/replayed report can never describe
/// two insertions of one transaction.
fn is_target_routed_delivery_report(mode: RecoveryDeliveryMode, report: &DeliveryReport) -> bool {
    if !mode.is_target_routed()
        || report.backend != mode.intent_backend()
        || report.failure_reason.is_some()
    {
        return false;
    }
    let Some((last, direct)) = report.attempts.split_last() else {
        return false;
    };
    if report
        .attempts
        .iter()
        .any(|attempt| attempt.backend.is_empty())
        || direct.iter().any(|attempt| attempt.backend == "inbox")
        || direct.iter().any(|attempt| {
            !matches!(
                attempt.outcome,
                DeliveryOutcome::SubmittedUnverified
                    | DeliveryOutcome::CopiedOnly
                    | DeliveryOutcome::Failed
            )
        })
    {
        return false;
    }
    match report.outcome {
        DeliveryOutcome::Delivered => {
            last.backend != "inbox"
                && last.outcome == DeliveryOutcome::Delivered
                && last.fallback_reason.is_none()
        }
        // Only the submitting rung may have dispatched: every earlier rung
        // must be a bounded failure, or the report would describe a second
        // insertion of one transaction.
        DeliveryOutcome::SubmittedUnverified => {
            last.backend != "inbox"
                && last.outcome == DeliveryOutcome::SubmittedUnverified
                && last.fallback_reason.is_none()
                && direct
                    .iter()
                    .all(|attempt| attempt.outcome == DeliveryOutcome::Failed)
        }
        DeliveryOutcome::InboxFallback => {
            last.backend == "inbox"
                && last.outcome == DeliveryOutcome::InboxFallback
                && last.fallback_reason.is_none()
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{RecoveryJournal, TestJournal};
    use crate::{
        DictationEndpointer, EndpointError, NativeEnergyVad, SpeechActivity, VoiceActivityDetector,
    };
    use narrivy_asr_api::{AsrCapabilities, AsrError, AsrTag, SpokenLanguage};
    use narrivy_platform_api::{
        AudioCaptureSession, CaptureEnd, CaptureStreamChunk, CaptureStreamInfo,
        CaptureStreamSummary, CapturedAudio, PlatformError,
    };
    use narrivy_policy::{DataClass, PolicyDocument, PolicyEngine, PolicyError};
    use narrivy_types::{AudioChunk, DeliveryAttempt, FallbackReason, Transcript};
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn required<T>(value: Option<T>, message: &'static str) -> Result<T, std::io::Error> {
        value.ok_or_else(|| std::io::Error::other(message))
    }

    #[derive(Default)]
    struct StartupInbox {
        delivered: Vec<String>,
        fail_txid: Option<String>,
    }

    impl DeliveryPort for StartupInbox {
        fn deliver(
            &mut self,
            intent: &DeliveryIntent,
            text: &str,
            _: &SessionCancellation,
        ) -> Result<DeliveryReport, PlatformError> {
            if self.fail_txid.as_deref() == Some(intent.transaction_id.as_str()) {
                return Err(PlatformError {
                    code: "controlled_inbox_failure",
                    message: "controlled Inbox failure".to_owned(),
                });
            }
            self.delivered.push(text.to_owned());
            Ok(DeliveryReport {
                transaction_id: intent.transaction_id.clone(),
                backend: "inbox".to_owned(),
                attempts: vec![DeliveryAttempt {
                    backend: "inbox".to_owned(),
                    outcome: DeliveryOutcome::Delivered,
                    fallback_reason: None,
                    at_unix_ms: 1,
                }],
                outcome: DeliveryOutcome::Delivered,
                failure_reason: None,
            })
        }
    }

    fn recovery_entry(txid: &str, state: JournalEntryState) -> JournalEntry {
        let transaction_id = TransactionId::new(txid);
        let mut entry = JournalEntry::new_pending(txid, unix_now_ms());
        entry.state = state;
        entry.variants.raw = Some("raw".to_owned());
        entry.variants.corrected = Some(format!("corrected-{txid}"));
        entry.delivery_intent = Some(DeliveryIntent {
            transaction_id,
            backend: "routed-then-inbox".to_owned(),
        });
        entry
    }

    #[test]
    fn startup_recovery_moves_only_complete_unsent_results_to_inbox_once() -> TestResult {
        let journal = TestJournal::new();
        for entry in [
            recovery_entry("tx-committed", JournalEntryState::Committed),
            recovery_entry("tx-failed", JournalEntryState::FailedRecoverable),
            recovery_entry("tx-pending", JournalEntryState::Pending),
            recovery_entry("tx-delivered", JournalEntryState::Delivered),
        ] {
            journal.write(&entry)?;
        }
        let mut malformed = recovery_entry("tx-malformed", JournalEntryState::Committed);
        malformed.delivery_intent = None;
        journal.write(&malformed)?;

        let mut inbox = StartupInbox::default();
        assert_eq!(
            recover_unsent_to_inbox(&journal, &mut inbox)?,
            StartupRecoveryReport {
                recovered: 2,
                incomplete: 2,
                delivery_failed: 0,
            }
        );
        assert_eq!(inbox.delivered.len(), 2);
        for txid in ["tx-committed", "tx-failed"] {
            let entry = required(journal.read(txid)?, "missing recovered entry")?;
            assert_eq!(entry.state, JournalEntryState::InboxRecovered);
            assert_eq!(entry.variants.delivered, entry.variants.corrected);
            assert_eq!(
                required(entry.delivery_report, "missing recovery report")?.backend,
                "inbox"
            );
        }
        assert_eq!(
            recover_unsent_to_inbox(&journal, &mut inbox)?,
            StartupRecoveryReport {
                recovered: 0,
                incomplete: 2,
                delivery_failed: 0,
            }
        );
        assert_eq!(inbox.delivered.len(), 2, "terminal entries were replayed");
        Ok(())
    }

    #[test]
    fn startup_recovery_failure_keeps_the_result_recoverable() -> TestResult {
        let journal = TestJournal::new();
        journal.write(&recovery_entry(
            "tx-retryable",
            JournalEntryState::FailedRecoverable,
        ))?;
        let mut inbox = StartupInbox {
            fail_txid: Some("tx-retryable".to_owned()),
            ..StartupInbox::default()
        };

        assert_eq!(
            recover_unsent_to_inbox(&journal, &mut inbox)?,
            StartupRecoveryReport {
                recovered: 0,
                incomplete: 0,
                delivery_failed: 1,
            }
        );
        assert_eq!(
            required(journal.read("tx-retryable")?, "missing retry entry")?.state,
            JournalEntryState::FailedRecoverable
        );
        Ok(())
    }

    struct Capture;
    impl AudioCapturePort for Capture {
        fn start(&mut self) -> Result<Box<dyn AudioCaptureSession>, PlatformError> {
            Ok(Box::new(ControlledCaptureSession {
                info: CaptureStreamInfo {
                    source_selection: "controlled-source".to_owned(),
                    sample_rate_hz: 16_000,
                    channels: 1,
                },
            }))
        }
    }
    struct ControlledCaptureSession {
        info: CaptureStreamInfo,
    }
    impl AudioCaptureSession for ControlledCaptureSession {
        fn info(&self) -> &CaptureStreamInfo {
            &self.info
        }

        fn wait_ready(&mut self, control: &SessionControl) -> Result<(), PlatformError> {
            if control.is_cancelled() {
                return Err(PlatformError {
                    code: "cancelled",
                    message: "controlled capture cancelled".to_owned(),
                });
            }
            Ok(())
        }

        fn collect(&mut self, control: &SessionControl) -> Result<CapturedAudio, PlatformError> {
            if control.is_cancelled() {
                return Err(PlatformError {
                    code: "cancelled",
                    message: "controlled capture cancelled".to_owned(),
                });
            }
            Ok(CapturedAudio {
                audio: AudioChunk {
                    sample_rate_hz: 16_000,
                    channels: 1,
                    samples: vec![0.0],
                },
                end: CaptureEnd::FinalizationRequested,
                first_frame_after_start_us: Some(1),
                capture_duration_us: 2,
            })
        }
    }
    struct Asr;
    impl OneShotAsrPort for Asr {
        fn recognize(
            &mut self,
            _: SessionId,
            _: &AudioChunk,
            _: &SessionCancellation,
        ) -> Result<Transcript, AsrError> {
            Ok(Transcript {
                text: "controlled raw".to_owned(),
                is_final: true,
            })
        }
    }
    struct Correct;
    impl CorrectionPort for Correct {
        fn correct(&mut self, raw: &str, _: &SessionCancellation) -> Result<String, CoreError> {
            Ok(format!("{raw}."))
        }
    }
    struct FailingCorrect;
    impl CorrectionPort for FailingCorrect {
        fn correct(&mut self, _: &str, _: &SessionCancellation) -> Result<String, CoreError> {
            Err(CoreError::Store("controlled correction failure".to_owned()))
        }
    }
    struct Inbox;
    impl DeliveryPort for Inbox {
        fn deliver(
            &mut self,
            intent: &DeliveryIntent,
            _: &str,
            _: &SessionCancellation,
        ) -> Result<DeliveryReport, PlatformError> {
            Ok(DeliveryReport {
                transaction_id: intent.transaction_id.clone(),
                backend: "inbox".to_owned(),
                attempts: vec![DeliveryAttempt {
                    backend: "inbox".to_owned(),
                    outcome: DeliveryOutcome::Delivered,
                    fallback_reason: None,
                    at_unix_ms: 1,
                }],
                outcome: DeliveryOutcome::Delivered,
                failure_reason: None,
            })
        }
    }

    #[test]
    fn controlled_success_persists_intent_before_observed_inbox_delivery() -> TestResult {
        let journal = TestJournal::new();
        let mut capture = Capture;
        let mut asr = Asr;
        let mut correct = Correct;
        let mut inbox = Inbox;
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let timing = RecoveryTimingTrace::new();
        let mut slice = RecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut inbox,
            delivery_mode: RecoveryDeliveryMode::InboxPrimary,
        };
        let mut states = Vec::new();
        let terminal = slice.run_with_timing(
            SessionId(7),
            TransactionId::new("tx-controlled"),
            "linux-mic-inbox".to_owned(),
            &SessionControl::new(),
            &mut |s| {
                states.push(s);
                Ok(())
            },
            &timing,
        )?;
        assert!(matches!(terminal, RecoveryTerminal::DeliveredInbox(_)));
        let entry = required(
            journal.read("tx-controlled")?,
            "missing controlled journal entry",
        )?;
        assert_eq!(entry.state, JournalEntryState::Delivered);
        assert_eq!(
            required(entry.delivery_intent, "missing controlled delivery intent")?.backend,
            "inbox"
        );
        assert!(states.contains(&SessionState::Delivering));
        assert_eq!(
            timing.snapshot_us().len(),
            RecoveryTimingStage::ALL.len() - 1,
            "batch ASR must report every stage except a streaming partial"
        );
        assert_eq!(
            timing.offset_us(RecoveryTimingStage::FirstPartialAccepted),
            None
        );
        assert!(
            states
                .windows(2)
                .any(|pair| { pair == [SessionState::Listening, SessionState::FinalizingAudio] }),
            "capture readiness must precede finalization"
        );
        Ok(())
    }

    #[test]
    fn a_live_preview_ends_before_recognition_on_the_batch_path() -> TestResult {
        #[derive(Default)]
        struct Preview {
            finished: usize,
        }
        impl LivePreviewPort for Preview {
            fn audio(&mut self, _samples: &[f32]) {}
            fn finish(&mut self) {
                self.finished += 1;
            }
        }
        let journal = TestJournal::new();
        let mut capture = Capture;
        let mut asr = Asr;
        let mut correct = Correct;
        let mut inbox = Inbox;
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let timing = RecoveryTimingTrace::new();
        let mut preview = Preview::default();
        let mut slice = RecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut inbox,
            delivery_mode: RecoveryDeliveryMode::InboxPrimary,
        };
        let terminal = slice.run_with_preview(
            SessionId(8),
            TransactionId::new("tx-preview"),
            "linux-mic-inbox".to_owned(),
            &SessionControl::new(),
            &mut |_| Ok(()),
            &AsrRequest::default(),
            &timing,
            &mut preview,
        )?;
        assert!(matches!(terminal, RecoveryTerminal::DeliveredInbox(_)));
        assert_eq!(preview.finished, 1, "the preview stops once, before ASR");
        Ok(())
    }

    #[test]
    fn batch_explicit_language_request_reaches_recognize_with() -> TestResult {
        struct LanguageRequestSpy(Arc<AtomicBool>);
        impl OneShotAsrPort for LanguageRequestSpy {
            fn recognize(
                &mut self,
                _: SessionId,
                _: &AudioChunk,
                _: &SessionCancellation,
            ) -> Result<Transcript, AsrError> {
                Err(AsrError::new(
                    "unexpected_recognize",
                    "non-default request must reach recognize_with",
                ))
            }

            fn recognize_with(
                &mut self,
                _: SessionId,
                _: &AudioChunk,
                _: &SessionCancellation,
                request: &AsrRequest,
            ) -> Result<Transcript, AsrError> {
                let matched = !request.is_default()
                    && matches!(
                        &request.spoken_language,
                        SpokenLanguage::Explicit(tag) if tag.as_str() == "en"
                    );
                self.0.store(matched, Ordering::SeqCst);
                Ok(Transcript {
                    text: "controlled raw".to_owned(),
                    is_final: true,
                })
            }
        }

        let journal = TestJournal::new();
        let mut capture = Capture;
        let saw_explicit_en = Arc::new(AtomicBool::new(false));
        let mut asr = LanguageRequestSpy(Arc::clone(&saw_explicit_en));
        let mut correct = Correct;
        let mut inbox = Inbox;
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let mut slice = RecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut inbox,
            delivery_mode: RecoveryDeliveryMode::InboxPrimary,
        };
        let request = AsrRequest {
            spoken_language: SpokenLanguage::Explicit(
                AsrTag::parse("en")
                    .map_err(|_| std::io::Error::other("controlled language tag is invalid"))?,
            ),
            ..AsrRequest::default()
        };
        let terminal = slice.run_with_request(
            SessionId(803),
            TransactionId::new("tx-explicit-asr-request"),
            "linux-mic-inbox".to_owned(),
            &SessionControl::new(),
            &mut |_| Ok(()),
            &request,
        )?;
        assert!(matches!(terminal, RecoveryTerminal::DeliveredInbox(_)));
        assert!(
            saw_explicit_en.load(Ordering::SeqCst),
            "explicit language request did not reach recognize_with"
        );
        Ok(())
    }

    #[test]
    fn batch_long_capture_reaches_asr_without_clipping_and_records_end() -> TestResult {
        struct LongCapture {
            samples: usize,
        }
        struct LongCaptureSession {
            info: CaptureStreamInfo,
            samples: usize,
        }
        impl AudioCapturePort for LongCapture {
            fn start(&mut self) -> Result<Box<dyn AudioCaptureSession>, PlatformError> {
                Ok(Box::new(LongCaptureSession {
                    info: CaptureStreamInfo {
                        source_selection: "controlled-long-source".to_owned(),
                        sample_rate_hz: 16_000,
                        channels: 1,
                    },
                    samples: self.samples,
                }))
            }
        }
        impl AudioCaptureSession for LongCaptureSession {
            fn info(&self) -> &CaptureStreamInfo {
                &self.info
            }

            fn wait_ready(&mut self, _: &SessionControl) -> Result<(), PlatformError> {
                Ok(())
            }

            fn collect(&mut self, _: &SessionControl) -> Result<CapturedAudio, PlatformError> {
                Ok(CapturedAudio {
                    audio: AudioChunk {
                        sample_rate_hz: 16_000,
                        channels: 1,
                        samples: vec![0.25; self.samples],
                    },
                    end: CaptureEnd::SafetyLimit,
                    first_frame_after_start_us: Some(1),
                    capture_duration_us: 40_000_000,
                })
            }
        }
        struct CountingAsr(Arc<Mutex<usize>>);
        impl OneShotAsrPort for CountingAsr {
            fn recognize(
                &mut self,
                _: SessionId,
                audio: &AudioChunk,
                _: &SessionCancellation,
            ) -> Result<Transcript, AsrError> {
                *self
                    .0
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = audio.samples.len();
                Ok(Transcript {
                    text: "controlled long raw".to_owned(),
                    is_final: true,
                })
            }
        }

        let journal = TestJournal::new();
        // Forty seconds at 16 kHz: beyond the historical thirty-second bound
        // and delivered without any speech-length sleep.
        let expected_samples = 640_000_usize;
        let mut capture = LongCapture {
            samples: expected_samples,
        };
        let seen = Arc::new(Mutex::new(0_usize));
        let mut asr = CountingAsr(Arc::clone(&seen));
        let mut correct = Correct;
        let mut inbox = Inbox;
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let mut slice = RecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut inbox,
            delivery_mode: RecoveryDeliveryMode::InboxPrimary,
        };
        let terminal = slice.run(
            SessionId(801),
            TransactionId::new("tx-long-batch"),
            "linux-mic-inbox".to_owned(),
            &SessionControl::new(),
            &mut |_| Ok(()),
        )?;
        assert!(matches!(terminal, RecoveryTerminal::DeliveredInbox(_)));
        assert_eq!(
            *seen.lock().unwrap_or_else(|poisoned| poisoned.into_inner()),
            expected_samples,
            "ASR must receive every captured sample exactly once"
        );
        let entry = required(
            journal.read("tx-long-batch")?,
            "missing long capture journal entry",
        )?;
        assert_eq!(entry.capture_end, Some(CaptureEnd::SafetyLimit));
        Ok(())
    }

    #[test]
    fn misbehaving_capture_over_the_portable_bound_fails_closed_before_asr() -> TestResult {
        struct OverBoundCapture(usize);
        struct OverBoundSession(usize);
        impl AudioCapturePort for OverBoundCapture {
            fn start(&mut self) -> Result<Box<dyn AudioCaptureSession>, PlatformError> {
                Ok(Box::new(OverBoundSession(self.0)))
            }
        }
        impl AudioCaptureSession for OverBoundSession {
            fn info(&self) -> &CaptureStreamInfo {
                static INFO: std::sync::OnceLock<CaptureStreamInfo> = std::sync::OnceLock::new();
                INFO.get_or_init(|| CaptureStreamInfo {
                    source_selection: "controlled-over-bound-source".to_owned(),
                    sample_rate_hz: 16_000,
                    channels: 1,
                })
            }

            fn wait_ready(&mut self, _: &SessionControl) -> Result<(), PlatformError> {
                Ok(())
            }

            fn collect(&mut self, _: &SessionControl) -> Result<CapturedAudio, PlatformError> {
                Ok(CapturedAudio {
                    audio: AudioChunk {
                        sample_rate_hz: 16_000,
                        channels: 1,
                        samples: vec![0.0; self.0],
                    },
                    end: CaptureEnd::SafetyLimit,
                    first_frame_after_start_us: Some(1),
                    capture_duration_us: 1,
                })
            }
        }
        struct ForbiddenAsr;
        impl OneShotAsrPort for ForbiddenAsr {
            fn recognize(
                &mut self,
                _: SessionId,
                _: &AudioChunk,
                _: &SessionCancellation,
            ) -> Result<Transcript, AsrError> {
                Err(AsrError::new(
                    "unexpected_recognize",
                    "over-bound audio must never reach ASR",
                ))
            }
        }

        let journal = TestJournal::new();
        let mut capture = OverBoundCapture(narrivy_types::MAX_CAPTURE_SAMPLES + 1);
        let mut asr = ForbiddenAsr;
        let mut correct = Correct;
        let mut inbox = Inbox;
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let mut slice = RecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut inbox,
            delivery_mode: RecoveryDeliveryMode::InboxPrimary,
        };
        let terminal = slice.run(
            SessionId(802),
            TransactionId::new("tx-over-bound"),
            "linux-mic-inbox".to_owned(),
            &SessionControl::new(),
            &mut |_| Ok(()),
        )?;
        assert_eq!(
            terminal,
            RecoveryTerminal::FailedTerminal {
                code: "capture_output_overflow".to_owned()
            }
        );
        let entry = required(
            journal.read("tx-over-bound")?,
            "missing over-bound journal entry",
        )?;
        assert_eq!(entry.state, JournalEntryState::FailedTerminal);
        // The end reason is still durable truth even though ASR was refused.
        assert_eq!(entry.capture_end, Some(CaptureEnd::SafetyLimit));
        Ok(())
    }

    #[test]
    fn batch_source_ended_journals_reason_and_fails_before_any_tail_side_effect() -> TestResult {
        struct EndedCapture;
        struct EndedCaptureSession;
        impl AudioCapturePort for EndedCapture {
            fn start(&mut self) -> Result<Box<dyn AudioCaptureSession>, PlatformError> {
                Ok(Box::new(EndedCaptureSession))
            }
        }
        impl AudioCaptureSession for EndedCaptureSession {
            fn info(&self) -> &CaptureStreamInfo {
                static INFO: std::sync::OnceLock<CaptureStreamInfo> = std::sync::OnceLock::new();
                INFO.get_or_init(|| CaptureStreamInfo {
                    source_selection: "controlled-ended-source".to_owned(),
                    sample_rate_hz: 16_000,
                    channels: 1,
                })
            }

            fn wait_ready(&mut self, _: &SessionControl) -> Result<(), PlatformError> {
                Ok(())
            }

            fn collect(&mut self, _: &SessionControl) -> Result<CapturedAudio, PlatformError> {
                Ok(CapturedAudio {
                    audio: AudioChunk {
                        sample_rate_hz: 16_000,
                        channels: 1,
                        samples: vec![0.25; 4],
                    },
                    end: CaptureEnd::SourceEnded,
                    first_frame_after_start_us: Some(1),
                    capture_duration_us: 2,
                })
            }
        }
        struct CountingAsr(Arc<AtomicU64>);
        impl OneShotAsrPort for CountingAsr {
            fn recognize(
                &mut self,
                _: SessionId,
                _: &AudioChunk,
                _: &SessionCancellation,
            ) -> Result<Transcript, AsrError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(Transcript {
                    text: "partial audio must never commit".to_owned(),
                    is_final: true,
                })
            }
        }
        struct CountingCorrect(Arc<AtomicU64>);
        impl CorrectionPort for CountingCorrect {
            fn correct(&mut self, raw: &str, _: &SessionCancellation) -> Result<String, CoreError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(raw.to_owned())
            }
        }
        struct CountingDelivery(Arc<AtomicU64>);
        impl DeliveryPort for CountingDelivery {
            fn deliver(
                &mut self,
                intent: &DeliveryIntent,
                _: &str,
                _: &SessionCancellation,
            ) -> Result<DeliveryReport, PlatformError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(DeliveryReport {
                    transaction_id: intent.transaction_id.clone(),
                    backend: "inbox".to_owned(),
                    attempts: Vec::new(),
                    outcome: DeliveryOutcome::Delivered,
                    failure_reason: None,
                })
            }
        }

        let journal = TestJournal::new();
        let mut capture = EndedCapture;
        let asr_calls = Arc::new(AtomicU64::new(0));
        let mut asr = CountingAsr(Arc::clone(&asr_calls));
        let correct_calls = Arc::new(AtomicU64::new(0));
        let mut correct = CountingCorrect(Arc::clone(&correct_calls));
        let delivery_calls = Arc::new(AtomicU64::new(0));
        let mut inbox = CountingDelivery(Arc::clone(&delivery_calls));
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let mut slice = RecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut inbox,
            delivery_mode: RecoveryDeliveryMode::InboxPrimary,
        };
        let mut states = Vec::new();
        let terminal = slice.run(
            SessionId(803),
            TransactionId::new("tx-source-ended-batch"),
            "linux-mic-inbox".to_owned(),
            &SessionControl::new(),
            &mut |state| {
                states.push(state);
                Ok(())
            },
        )?;
        assert_eq!(
            terminal,
            RecoveryTerminal::FailedTerminal {
                code: "capture_source_ended".to_owned()
            }
        );
        let entry = required(
            journal.read("tx-source-ended-batch")?,
            "missing source-ended journal entry",
        )?;
        assert_eq!(entry.state, JournalEntryState::FailedTerminal);
        assert_eq!(entry.failure_code.as_deref(), Some("capture_source_ended"));
        // The reason is durable truth even though no partial audio was used.
        assert_eq!(entry.capture_end, Some(CaptureEnd::SourceEnded));
        assert_eq!(asr_calls.load(Ordering::SeqCst), 0);
        assert_eq!(correct_calls.load(Ordering::SeqCst), 0);
        assert_eq!(delivery_calls.load(Ordering::SeqCst), 0);
        assert!(entry.variants.raw.is_none());
        assert!(entry.variants.corrected.is_none());
        assert!(entry.variants.delivered.is_none());
        assert!(entry.delivery_intent.is_none());
        assert!(
            !states.contains(&SessionState::FinalizingAudio)
                && !states.contains(&SessionState::Transcribing)
                && !states.contains(&SessionState::Delivering),
            "source-ended capture must not claim an ASR/correction/delivery stage"
        );
        Ok(())
    }

    /// D6/F-H2, batch path: with half a second captured before the source
    /// ended, the audio is recognized and corrected, the text is journaled,
    /// and nothing is delivered.
    #[test]
    fn batch_source_ended_after_enough_audio_keeps_the_text_undelivered() -> TestResult {
        struct EndedCapture;
        struct EndedSession(CaptureStreamInfo);
        impl AudioCapturePort for EndedCapture {
            fn start(&mut self) -> Result<Box<dyn AudioCaptureSession>, PlatformError> {
                Ok(Box::new(EndedSession(CaptureStreamInfo {
                    source_selection: "controlled-ended-source".to_owned(),
                    sample_rate_hz: 16_000,
                    channels: 1,
                })))
            }
        }
        impl AudioCaptureSession for EndedSession {
            fn info(&self) -> &CaptureStreamInfo {
                &self.0
            }
            fn wait_ready(&mut self, _: &SessionControl) -> Result<(), PlatformError> {
                Ok(())
            }
            fn collect(&mut self, _: &SessionControl) -> Result<CapturedAudio, PlatformError> {
                Ok(CapturedAudio {
                    audio: AudioChunk {
                        sample_rate_hz: 16_000,
                        channels: 1,
                        samples: vec![0.25; SOURCE_ENDED_MIN_SAMPLES as usize],
                    },
                    end: CaptureEnd::SourceEnded,
                    first_frame_after_start_us: Some(1),
                    capture_duration_us: 500_000,
                })
            }
        }
        let journal = TestJournal::new();
        let mut capture = EndedCapture;
        let mut asr = Asr;
        let mut correct = Correct;
        let delivered = Arc::new(AtomicU64::new(0));
        let mut delivery = ForbiddenDelivery(Arc::clone(&delivered));
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let mut slice = RecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut delivery,
            delivery_mode: RecoveryDeliveryMode::InboxPrimary,
        };
        let mut states = Vec::new();
        let terminal = slice.run(
            SessionId(804),
            TransactionId::new("tx-source-ended-kept"),
            "linux-mic-inbox".to_owned(),
            &SessionControl::new(),
            &mut |state| {
                states.push(state);
                Ok(())
            },
        )?;
        assert_eq!(
            terminal,
            RecoveryTerminal::FailedRecoverable {
                code: CAPTURE_SOURCE_ENDED.to_owned()
            }
        );
        let entry = required(
            journal.read("tx-source-ended-kept")?,
            "missing kept source-ended journal entry",
        )?;
        assert_eq!(entry.state, JournalEntryState::FailedRecoverable);
        assert_eq!(entry.capture_end, Some(CaptureEnd::SourceEnded));
        assert_eq!(entry.variants.raw.as_deref(), Some("controlled raw"));
        assert_eq!(entry.variants.corrected.as_deref(), Some("controlled raw."));
        assert!(entry.variants.delivered.is_none());
        assert!(entry.delivery_intent.is_none());
        assert_eq!(delivered.load(Ordering::SeqCst), 0);
        assert!(!states.contains(&SessionState::Delivering));
        assert_eq!(states.last(), Some(&SessionState::Idle));
        Ok(())
    }

    #[test]
    fn finalization_is_not_reinterpreted_as_cancellation() -> TestResult {
        let journal = TestJournal::new();
        let mut capture = Capture;
        let mut asr = Asr;
        let mut correct = Correct;
        let mut inbox = Inbox;
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let control = SessionControl::new();
        control.request_finalization();
        let mut slice = RecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut inbox,
            delivery_mode: RecoveryDeliveryMode::InboxPrimary,
        };

        assert!(matches!(
            slice.run(
                SessionId(701),
                TransactionId::new("tx-finalize-not-cancel"),
                "linux-mic-inbox".to_owned(),
                &control,
                &mut |_| Ok(()),
            )?,
            RecoveryTerminal::DeliveredInbox(_)
        ));
        assert!(!control.is_cancelled());
        Ok(())
    }

    struct FailingReadyCapture;
    impl AudioCapturePort for FailingReadyCapture {
        fn start(&mut self) -> Result<Box<dyn AudioCaptureSession>, PlatformError> {
            Ok(Box::new(FailingReadySession {
                info: CaptureStreamInfo {
                    source_selection: "controlled-unavailable-source".to_owned(),
                    sample_rate_hz: 16_000,
                    channels: 1,
                },
            }))
        }
    }

    struct FailingReadySession {
        info: CaptureStreamInfo,
    }

    impl AudioCaptureSession for FailingReadySession {
        fn info(&self) -> &CaptureStreamInfo {
            &self.info
        }

        fn wait_ready(&mut self, _: &SessionControl) -> Result<(), PlatformError> {
            Err(PlatformError {
                code: "capture_source_unavailable",
                message: "controlled readiness failure".to_owned(),
            })
        }

        fn collect(&mut self, _: &SessionControl) -> Result<CapturedAudio, PlatformError> {
            Err(PlatformError {
                code: "controlled_collect_called_after_readiness_failure",
                message: "collect must not run when readiness fails".to_owned(),
            })
        }
    }

    #[test]
    fn readiness_failure_never_claims_listening() -> TestResult {
        let journal = TestJournal::new();
        let mut capture = FailingReadyCapture;
        let mut asr = Asr;
        let mut correct = Correct;
        let mut inbox = Inbox;
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let mut states = Vec::new();
        let mut slice = RecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut inbox,
            delivery_mode: RecoveryDeliveryMode::InboxPrimary,
        };

        assert_eq!(
            slice.run(
                SessionId(702),
                TransactionId::new("tx-ready-failed"),
                "linux-mic-inbox".to_owned(),
                &SessionControl::new(),
                &mut |state| {
                    states.push(state);
                    Ok(())
                },
            )?,
            RecoveryTerminal::FailedTerminal {
                code: "capture_source_unavailable".to_owned()
            }
        );
        assert!(!states.contains(&SessionState::Listening));
        Ok(())
    }

    struct CleanupFailureCapture;
    impl AudioCapturePort for CleanupFailureCapture {
        fn start(&mut self) -> Result<Box<dyn AudioCaptureSession>, PlatformError> {
            Ok(Box::new(CleanupFailureSession {
                info: CaptureStreamInfo {
                    source_selection: "controlled-cleanup-failure".to_owned(),
                    sample_rate_hz: 16_000,
                    channels: 1,
                },
            }))
        }
    }

    struct CleanupFailureSession {
        info: CaptureStreamInfo,
    }

    impl AudioCaptureSession for CleanupFailureSession {
        fn info(&self) -> &CaptureStreamInfo {
            &self.info
        }

        fn wait_ready(&mut self, _: &SessionControl) -> Result<(), PlatformError> {
            Ok(())
        }

        fn collect(&mut self, control: &SessionControl) -> Result<CapturedAudio, PlatformError> {
            control.cancel();
            Err(PlatformError {
                code: "capture_termination_timeout",
                message: "controlled owned-child cleanup failure".to_owned(),
            })
        }
    }

    #[test]
    fn cancellation_never_masks_owned_resource_cleanup_failure() -> TestResult {
        let journal = TestJournal::new();
        let mut capture = CleanupFailureCapture;
        let mut asr = Asr;
        let mut correct = Correct;
        let mut inbox = Inbox;
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let control = SessionControl::new();
        let mut slice = RecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut inbox,
            delivery_mode: RecoveryDeliveryMode::InboxPrimary,
        };

        assert_eq!(
            slice.run(
                SessionId(703),
                TransactionId::new("tx-cleanup-failed"),
                "linux-mic-inbox".to_owned(),
                &control,
                &mut |_| Ok(()),
            )?,
            RecoveryTerminal::FailedTerminal {
                code: "capture_termination_timeout".to_owned()
            }
        );
        assert!(control.is_cancelled());
        Ok(())
    }

    #[test]
    fn controlled_cancellation_never_calls_delivery() -> TestResult {
        let journal = TestJournal::new();
        let mut capture = Capture;
        let mut asr = Asr;
        let mut correct = Correct;
        let mut inbox = Inbox;
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let control = SessionControl::new();
        control.cancel();
        let mut slice = RecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut inbox,
            delivery_mode: RecoveryDeliveryMode::InboxPrimary,
        };
        let terminal = slice.run(
            SessionId(8),
            TransactionId::new("tx-cancelled"),
            "linux-mic-inbox".to_owned(),
            &control,
            &mut |_| Ok(()),
        )?;
        assert_eq!(terminal, RecoveryTerminal::Cancelled);
        assert_eq!(
            required(
                journal.read("tx-cancelled")?,
                "missing cancelled journal entry"
            )?
            .state,
            JournalEntryState::Cancelled
        );
        Ok(())
    }

    struct DenyAudio;
    impl ProcessingPolicy for DenyAudio {
        fn allow_data(
            &self,
            class: DataClass,
            boundary: ProcessingBoundary,
        ) -> Result<(), PolicyError> {
            Err(PolicyError::Denied { class, boundary })
        }
    }
    struct AsrSpy(Arc<AtomicBool>);
    impl OneShotAsrPort for AsrSpy {
        fn recognize(
            &mut self,
            _: SessionId,
            _: &AudioChunk,
            _: &SessionCancellation,
        ) -> Result<Transcript, AsrError> {
            self.0.store(true, Ordering::SeqCst);
            Ok(Transcript {
                text: "controlled".to_owned(),
                is_final: true,
            })
        }
    }

    #[test]
    fn controlled_policy_denial_prevents_asr_from_receiving_audio() -> TestResult {
        let journal = TestJournal::new();
        let mut capture = Capture;
        let asr_called = Arc::new(AtomicBool::new(false));
        let mut asr = AsrSpy(asr_called.clone());
        let mut correct = Correct;
        let mut inbox = Inbox;
        let policy = DenyAudio;
        let mut slice = RecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut inbox,
            delivery_mode: RecoveryDeliveryMode::InboxPrimary,
        };
        let terminal = slice.run(
            SessionId(9),
            TransactionId::new("tx-policy"),
            "linux-mic-inbox".to_owned(),
            &SessionControl::new(),
            &mut |_| Ok(()),
        )?;
        assert_eq!(
            terminal,
            RecoveryTerminal::FailedTerminal {
                code: "policy_denied_audio".to_owned()
            }
        );
        assert!(
            !asr_called.load(Ordering::SeqCst),
            "denied audio reached the ASR port"
        );
        Ok(())
    }

    #[test]
    fn rejected_listening_transition_stops_before_asr_and_is_durable() -> TestResult {
        let journal = TestJournal::new();
        let mut capture = Capture;
        let asr_called = Arc::new(AtomicBool::new(false));
        let mut asr = AsrSpy(asr_called.clone());
        let mut correct = Correct;
        let mut inbox = Inbox;
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let mut slice = RecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut inbox,
            delivery_mode: RecoveryDeliveryMode::InboxPrimary,
        };

        let error = match slice.run(
            SessionId(904),
            TransactionId::new("tx-state-rejected-before-asr"),
            "linux-mic-inbox".to_owned(),
            &SessionControl::new(),
            &mut |state| {
                if state == SessionState::Listening {
                    Err(CoreError::Worker(
                        "controlled state transition rejection".to_owned(),
                    ))
                } else {
                    Ok(())
                }
            },
        ) {
            Err(error) => error,
            Ok(_) => {
                return Err(std::io::Error::other(
                    "rejected state transition did not stop orchestration",
                )
                .into());
            }
        };
        assert!(matches!(error, CoreError::Worker(_)));
        assert!(!asr_called.load(Ordering::SeqCst));
        let entry = required(
            journal.read("tx-state-rejected-before-asr")?,
            "missing state-rejection journal entry",
        )?;
        assert_eq!(entry.state, JournalEntryState::FailedTerminal);
        assert_eq!(
            entry.failure_code.as_deref(),
            Some("state_observation_failed")
        );
        assert!(entry.delivery_intent.is_none());
        Ok(())
    }

    struct DenyRawTranscript;
    impl ProcessingPolicy for DenyRawTranscript {
        fn allow_data(
            &self,
            class: DataClass,
            boundary: ProcessingBoundary,
        ) -> Result<(), PolicyError> {
            if class == DataClass::TranscriptRaw {
                Err(PolicyError::Denied { class, boundary })
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn raw_transcript_policy_is_checked_before_journal_persistence() -> TestResult {
        let journal = TestJournal::new();
        let mut capture = Capture;
        let mut asr = Asr;
        let mut correct = Correct;
        let mut inbox = Inbox;
        let policy = DenyRawTranscript;
        let mut slice = RecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut inbox,
            delivery_mode: RecoveryDeliveryMode::InboxPrimary,
        };
        let terminal = slice.run(
            SessionId(91),
            TransactionId::new("tx-policy-raw"),
            "linux-mic-inbox".to_owned(),
            &SessionControl::new(),
            &mut |_| Ok(()),
        )?;
        assert_eq!(
            terminal,
            RecoveryTerminal::FailedTerminal {
                code: "policy_denied_transcript_raw".to_owned()
            }
        );
        let entry = required(
            journal.read("tx-policy-raw")?,
            "missing raw-policy journal entry",
        )?;
        assert!(
            entry.variants.raw.is_none(),
            "denied raw text was persisted"
        );
        assert!(entry.variants.corrected.is_none());
        Ok(())
    }

    struct DeliveryTriggersJournalFailure<'a>(&'a TestJournal);
    impl DeliveryPort for DeliveryTriggersJournalFailure<'_> {
        fn deliver(
            &mut self,
            intent: &DeliveryIntent,
            _: &str,
            _: &SessionCancellation,
        ) -> Result<DeliveryReport, PlatformError> {
            self.0.fail_next_write_for_test();
            Ok(DeliveryReport {
                transaction_id: intent.transaction_id.clone(),
                backend: "inbox".to_owned(),
                attempts: vec![DeliveryAttempt {
                    backend: "inbox".to_owned(),
                    outcome: DeliveryOutcome::Delivered,
                    fallback_reason: None,
                    at_unix_ms: 1,
                }],
                outcome: DeliveryOutcome::Delivered,
                failure_reason: None,
            })
        }
    }

    #[test]
    fn journal_failure_after_primary_delivery_is_recoverable_uncertainty() -> TestResult {
        let journal = TestJournal::new();
        let mut capture = Capture;
        let mut asr = Asr;
        let mut correct = Correct;
        let mut delivery = DeliveryTriggersJournalFailure(&journal);
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let mut slice = RecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut delivery,
            delivery_mode: RecoveryDeliveryMode::InboxPrimary,
        };
        let terminal = slice.run(
            SessionId(92),
            TransactionId::new("tx-journal-uncertain"),
            "linux-mic-inbox".to_owned(),
            &SessionControl::new(),
            &mut |_| Ok(()),
        )?;
        assert_eq!(
            terminal,
            RecoveryTerminal::FailedRecoverable {
                code: "journal_write_after_delivery_uncertain".to_owned()
            }
        );
        let entry = required(
            journal.read("tx-journal-uncertain")?,
            "missing uncertain journal entry",
        )?;
        assert_eq!(entry.state, JournalEntryState::Committed);
        assert!(entry.delivery_intent.is_some());
        assert!(entry.delivery_report.is_none());
        Ok(())
    }

    struct CancellingCorrect;
    impl CorrectionPort for CancellingCorrect {
        fn correct(
            &mut self,
            raw: &str,
            cancellation: &SessionCancellation,
        ) -> Result<String, CoreError> {
            cancellation.cancel();
            Ok(raw.to_owned())
        }
    }
    struct DeliverySpy(Arc<AtomicBool>);
    impl DeliveryPort for DeliverySpy {
        fn deliver(
            &mut self,
            _: &DeliveryIntent,
            _: &str,
            _: &SessionCancellation,
        ) -> Result<DeliveryReport, PlatformError> {
            self.0.store(true, Ordering::SeqCst);
            Err(PlatformError {
                code: "test_bug",
                message: "delivery must not run".to_owned(),
            })
        }
    }

    #[test]
    fn controlled_cancellation_before_delivery_persists_intent_but_writes_no_delivery() -> TestResult
    {
        let journal = TestJournal::new();
        let mut capture = Capture;
        let mut asr = Asr;
        let mut correct = CancellingCorrect;
        let called = Arc::new(AtomicBool::new(false));
        let mut delivery = DeliverySpy(called.clone());
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let control = SessionControl::new();
        let mut slice = RecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut delivery,
            delivery_mode: RecoveryDeliveryMode::InboxPrimary,
        };
        let terminal = slice.run(
            SessionId(10),
            TransactionId::new("tx-before-delivery"),
            "linux-mic-inbox".to_owned(),
            &control,
            &mut |_| Ok(()),
        )?;
        assert_eq!(terminal, RecoveryTerminal::Cancelled);
        assert!(!called.load(Ordering::SeqCst));
        let entry = required(
            journal.read("tx-before-delivery")?,
            "missing pre-delivery cancellation journal entry",
        )?;
        assert!(entry.delivery_intent.is_some());
        assert!(entry.delivery_report.is_none());
        Ok(())
    }

    #[test]
    fn rejected_delivering_transition_preserves_committed_text_without_delivery() -> TestResult {
        let journal = TestJournal::new();
        let mut capture = Capture;
        let mut asr = Asr;
        let mut correct = Correct;
        let called = Arc::new(AtomicBool::new(false));
        let mut delivery = DeliverySpy(called.clone());
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let mut slice = RecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut delivery,
            delivery_mode: RecoveryDeliveryMode::InboxPrimary,
        };

        let error = match slice.run(
            SessionId(905),
            TransactionId::new("tx-state-rejected-before-delivery"),
            "linux-mic-inbox".to_owned(),
            &SessionControl::new(),
            &mut |state| {
                if state == SessionState::Delivering {
                    Err(CoreError::Worker(
                        "controlled state transition rejection".to_owned(),
                    ))
                } else {
                    Ok(())
                }
            },
        ) {
            Err(error) => error,
            Ok(_) => {
                return Err(std::io::Error::other(
                    "rejected delivery transition did not stop orchestration",
                )
                .into());
            }
        };
        assert!(matches!(error, CoreError::Worker(_)));
        assert!(!called.load(Ordering::SeqCst));
        let entry = required(
            journal.read("tx-state-rejected-before-delivery")?,
            "missing delivery-state-rejection journal entry",
        )?;
        assert_eq!(entry.state, JournalEntryState::FailedRecoverable);
        assert_eq!(
            entry.failure_code.as_deref(),
            Some("state_observation_failed")
        );
        assert!(entry.variants.corrected.is_some());
        assert!(entry.delivery_intent.is_some());
        assert!(entry.delivery_report.is_none());
        Ok(())
    }

    struct MissingModel;
    impl OneShotAsrPort for MissingModel {
        fn recognize(
            &mut self,
            _: SessionId,
            _: &AudioChunk,
            _: &SessionCancellation,
        ) -> Result<Transcript, AsrError> {
            Err(AsrError::new(
                "model_unavailable",
                "controlled missing model",
            ))
        }
    }

    #[test]
    fn controlled_asr_initialization_failure_is_a_terminal_journal_state() -> TestResult {
        let journal = TestJournal::new();
        let mut capture = Capture;
        let mut asr = MissingModel;
        let mut correct = Correct;
        let mut inbox = Inbox;
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let mut slice = RecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut inbox,
            delivery_mode: RecoveryDeliveryMode::InboxPrimary,
        };
        let terminal = slice.run(
            SessionId(11),
            TransactionId::new("tx-model-missing"),
            "linux-mic-inbox".to_owned(),
            &SessionControl::new(),
            &mut |_| Ok(()),
        )?;
        assert_eq!(
            terminal,
            RecoveryTerminal::FailedTerminal {
                code: "model_unavailable".to_owned()
            }
        );
        let entry = required(
            journal.read("tx-model-missing")?,
            "missing model-failure journal entry",
        )?;
        assert_eq!(entry.state, JournalEntryState::FailedTerminal);
        assert_eq!(entry.failure_code.as_deref(), Some("model_unavailable"));
        Ok(())
    }

    struct EmptyAsr;
    impl OneShotAsrPort for EmptyAsr {
        fn recognize(
            &mut self,
            _: SessionId,
            _: &AudioChunk,
            _: &SessionCancellation,
        ) -> Result<Transcript, AsrError> {
            Ok(Transcript {
                text: "  \n".to_owned(),
                is_final: true,
            })
        }
    }

    #[test]
    fn controlled_empty_asr_result_never_reaches_correction_or_delivery() -> TestResult {
        let journal = TestJournal::new();
        let mut capture = Capture;
        let mut asr = EmptyAsr;
        let mut correct = Correct;
        let mut inbox = Inbox;
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let mut slice = RecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut inbox,
            delivery_mode: RecoveryDeliveryMode::InboxPrimary,
        };
        let terminal = slice.run(
            SessionId(12),
            TransactionId::new("tx-empty"),
            "linux-mic-inbox".to_owned(),
            &SessionControl::new(),
            &mut |_| Ok(()),
        )?;
        assert_eq!(
            terminal,
            RecoveryTerminal::FailedTerminal {
                code: "empty_transcript".to_owned()
            }
        );
        let entry = required(journal.read("tx-empty")?, "missing empty-ASR journal entry")?;
        assert_eq!(entry.state, JournalEntryState::FailedTerminal);
        assert!(entry.delivery_intent.is_none());
        Ok(())
    }

    struct WtypeInboxReport {
        report: DeliveryReport,
        cancel_after_dispatch: bool,
    }

    impl DeliveryPort for WtypeInboxReport {
        fn deliver(
            &mut self,
            _: &DeliveryIntent,
            _: &str,
            cancellation: &SessionCancellation,
        ) -> Result<DeliveryReport, PlatformError> {
            if self.cancel_after_dispatch {
                cancellation.cancel();
            }
            Ok(self.report.clone())
        }
    }

    struct IntentCommittedBeforeControlledDirectAttempt<'a> {
        journal: &'a Journal,
    }

    impl DeliveryPort for IntentCommittedBeforeControlledDirectAttempt<'_> {
        fn deliver(
            &mut self,
            intent: &DeliveryIntent,
            text: &str,
            _: &SessionCancellation,
        ) -> Result<DeliveryReport, PlatformError> {
            let entry = self
                .journal
                .read(intent.transaction_id.as_str())
                .map_err(|_| PlatformError {
                    code: "controlled_journal_read_failed",
                    message: String::new(),
                })?
                .ok_or_else(|| PlatformError {
                    code: "controlled_intent_missing",
                    message: String::new(),
                })?;
            assert_eq!(entry.state, JournalEntryState::Committed);
            assert_eq!(entry.variants.corrected.as_deref(), Some(text));
            assert_eq!(entry.delivery_intent.as_ref(), Some(intent));
            assert!(entry.delivery_report.is_none());
            Ok(wtype_inbox_report(
                intent.transaction_id.clone(),
                DeliveryOutcome::SubmittedUnverified,
            ))
        }
    }

    struct WtypeInboxReportThenFinalJournalFailure<'a> {
        journal: &'a TestJournal,
        report: DeliveryReport,
    }

    impl DeliveryPort for WtypeInboxReportThenFinalJournalFailure<'_> {
        fn deliver(
            &mut self,
            _: &DeliveryIntent,
            _: &str,
            _: &SessionCancellation,
        ) -> Result<DeliveryReport, PlatformError> {
            // The next core write stores the actual report. Fail only the
            // terminal transition after that report is durable.
            self.journal.fail_after_successful_writes_for_test(1);
            Ok(self.report.clone())
        }
    }

    fn wtype_inbox_report(txid: TransactionId, direct: DeliveryOutcome) -> DeliveryReport {
        let direct_fallback =
            (direct == DeliveryOutcome::Failed).then(|| FallbackReason::BackendError {
                code: "WTYPE_CONTROLLED_FAILURE".to_owned(),
            });
        DeliveryReport {
            transaction_id: txid,
            backend: "wtype-then-inbox".to_owned(),
            attempts: vec![
                DeliveryAttempt {
                    backend: "wtype-stdin".to_owned(),
                    outcome: direct,
                    fallback_reason: direct_fallback,
                    at_unix_ms: 1,
                },
                DeliveryAttempt {
                    backend: "inbox".to_owned(),
                    outcome: DeliveryOutcome::InboxFallback,
                    fallback_reason: None,
                    at_unix_ms: 2,
                },
            ],
            outcome: DeliveryOutcome::InboxFallback,
            failure_reason: None,
        }
    }

    fn portal_eis_inbox_report(txid: TransactionId, direct: DeliveryOutcome) -> DeliveryReport {
        let direct_fallback =
            (direct == DeliveryOutcome::Failed).then(|| FallbackReason::BackendError {
                code: "PORTAL_EIS_CONTROLLED_FAILURE".to_owned(),
            });
        DeliveryReport {
            transaction_id: txid,
            backend: "portal-eis-then-inbox".to_owned(),
            attempts: vec![
                DeliveryAttempt {
                    backend: "portal-eis-text".to_owned(),
                    outcome: direct,
                    fallback_reason: direct_fallback,
                    at_unix_ms: 1,
                },
                DeliveryAttempt {
                    backend: "inbox".to_owned(),
                    outcome: DeliveryOutcome::InboxFallback,
                    fallback_reason: None,
                    at_unix_ms: 2,
                },
            ],
            outcome: DeliveryOutcome::InboxFallback,
            failure_reason: None,
        }
    }

    #[test]
    fn only_exact_wtype_inbox_recovery_reports_can_reach_inbox_recovered() -> TestResult {
        let valid_txid = TransactionId::new("tx-wtype-direct-failed");
        let valid = wtype_inbox_report(valid_txid, DeliveryOutcome::Failed);
        assert!(is_direct_inbox_recovery_report(
            RecoveryDeliveryMode::WtypeThenInboxRecovery,
            &valid
        ));

        let malformed = [
            "wrong-attempt-count",
            "extra-attempt",
            "wrong-direct-backend",
            "submitted-with-fallback",
            "failed-without-fallback",
            "wrong-direct-outcome",
            "wrong-inbox-backend",
            "wrong-inbox-outcome",
            "inbox-with-fallback",
            "wrong-final-outcome",
            "final-failure-reason",
            "wrong-report-backend",
        ];

        for (index, kind) in malformed.into_iter().enumerate() {
            let journal = TestJournal::new();
            let txid = TransactionId::new(format!("tx-wtype-malformed-{index}"));
            let mut report = wtype_inbox_report(txid.clone(), DeliveryOutcome::SubmittedUnverified);
            match kind {
                "wrong-attempt-count" => {
                    report.attempts.pop();
                }
                "extra-attempt" => report.attempts.push(DeliveryAttempt {
                    backend: "inbox".to_owned(),
                    outcome: DeliveryOutcome::InboxFallback,
                    fallback_reason: None,
                    at_unix_ms: 3,
                }),
                "wrong-direct-backend" => report.attempts[0].backend = "wtype".to_owned(),
                "submitted-with-fallback" => {
                    report.attempts[0].fallback_reason = Some(FallbackReason::BackendError {
                        code: "UNEXPECTED".to_owned(),
                    });
                }
                "failed-without-fallback" => {
                    report.attempts[0].outcome = DeliveryOutcome::Failed;
                }
                "wrong-direct-outcome" => {
                    report.attempts[0].outcome = DeliveryOutcome::Delivered;
                }
                "wrong-inbox-backend" => report.attempts[1].backend = "clipboard".to_owned(),
                "wrong-inbox-outcome" => {
                    report.attempts[1].outcome = DeliveryOutcome::Delivered;
                }
                "inbox-with-fallback" => {
                    report.attempts[1].fallback_reason = Some(FallbackReason::BackendError {
                        code: "UNEXPECTED".to_owned(),
                    });
                }
                "wrong-final-outcome" => report.outcome = DeliveryOutcome::Delivered,
                "final-failure-reason" => {
                    report.failure_reason = Some(FallbackReason::BackendError {
                        code: "UNEXPECTED".to_owned(),
                    });
                }
                "wrong-report-backend" => report.backend = "inbox".to_owned(),
                unexpected => {
                    return Err(std::io::Error::other(format!(
                        "unexpected malformed report matrix case: {unexpected}"
                    ))
                    .into());
                }
            }
            assert!(
                !is_direct_inbox_recovery_report(
                    RecoveryDeliveryMode::WtypeThenInboxRecovery,
                    &report
                ),
                "{kind} must not qualify as Inbox recovery"
            );

            let mut capture = Capture;
            let mut asr = Asr;
            let mut correct = Correct;
            let mut delivery = WtypeInboxReport {
                report,
                cancel_after_dispatch: false,
            };
            let policy = PolicyEngine::from_document(PolicyDocument::default());
            let mut slice = RecoverySlice {
                journal: &journal,
                capture: &mut capture,
                asr: &mut asr,
                policy: &policy,
                corrector: &mut correct,
                delivery: &mut delivery,
                delivery_mode: RecoveryDeliveryMode::WtypeThenInboxRecovery,
            };
            let terminal = slice.run(
                SessionId(100 + index as u128),
                txid.clone(),
                "linux-mic-wtype-inbox".to_owned(),
                &SessionControl::new(),
                &mut |_| Ok(()),
            )?;
            assert!(
                matches!(
                    terminal,
                    RecoveryTerminal::FailedRecoverable { ref code }
                        if code == "delivery_outcome_not_inbox_recovery"
                            || code == "delivery_identity_mismatch"
                ),
                "{kind} must not reach InboxRecovered: {terminal:?}"
            );
            assert_eq!(
                required(
                    journal.read(txid.as_str())?,
                    "missing malformed-report journal entry"
                )?
                .state,
                JournalEntryState::FailedRecoverable,
                "{kind} must leave a truthful failed journal state"
            );
        }
        Ok(())
    }

    /// One controlled routed-ladder report, exactly as the Linux adapter would
    /// return it. The core tail must judge the observed attempts, never a
    /// caller-supplied success flag.
    struct RoutedReport {
        report: DeliveryReport,
    }

    impl DeliveryPort for RoutedReport {
        fn deliver(
            &mut self,
            _: &DeliveryIntent,
            _: &str,
            _: &SessionCancellation,
        ) -> Result<DeliveryReport, PlatformError> {
            Ok(self.report.clone())
        }
    }

    fn routed_attempt(backend: &str, outcome: DeliveryOutcome) -> DeliveryAttempt {
        let fallback_reason = match outcome {
            DeliveryOutcome::Failed => Some(FallbackReason::BackendError {
                code: "CONTROLLED_BACKEND_FAILURE".to_owned(),
            }),
            _ => None,
        };
        DeliveryAttempt {
            backend: backend.to_owned(),
            outcome,
            fallback_reason,
            at_unix_ms: 1,
        }
    }

    fn routed_inbox_attempt() -> DeliveryAttempt {
        DeliveryAttempt {
            backend: "inbox".to_owned(),
            outcome: DeliveryOutcome::InboxFallback,
            fallback_reason: None,
            at_unix_ms: 2,
        }
    }

    fn routed_report(
        txid: &TransactionId,
        attempts: Vec<DeliveryAttempt>,
        outcome: DeliveryOutcome,
    ) -> DeliveryReport {
        DeliveryReport {
            transaction_id: txid.clone(),
            backend: "routed-then-inbox".to_owned(),
            attempts,
            outcome,
            failure_reason: None,
        }
    }

    #[test]
    fn routed_unverified_submission_is_a_finished_delivery_without_inbox() -> TestResult {
        // F-M4: a helper accepted the whole payload. The transaction is
        // finished (never replayed to the Inbox at startup) and the journal
        // keeps the fact that it was not verified.
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let run = |txid: &TransactionId, attempts: Vec<DeliveryAttempt>, journal: &TestJournal| {
            let mut capture = Capture;
            let mut asr = Asr;
            let mut correct = Correct;
            let mut delivery = RoutedReport {
                report: routed_report(txid, attempts, DeliveryOutcome::SubmittedUnverified),
            };
            RecoverySlice {
                journal,
                capture: &mut capture,
                asr: &mut asr,
                policy: &policy,
                corrector: &mut correct,
                delivery: &mut delivery,
                delivery_mode: RecoveryDeliveryMode::TargetRoutedThenInboxRecovery,
            }
            .run(
                SessionId(23),
                txid.clone(),
                "clean".to_owned(),
                &SessionControl::new(),
                &mut |_| Ok(()),
            )
        };

        let journal = TestJournal::new();
        let txid = TransactionId::new("tx-routed-submitted");
        let terminal = run(
            &txid,
            vec![
                routed_attempt("portal-eis-text", DeliveryOutcome::Failed),
                routed_attempt(
                    "wtype-virtual-keyboard",
                    DeliveryOutcome::SubmittedUnverified,
                ),
            ],
            &journal,
        )?;
        assert!(matches!(terminal, RecoveryTerminal::SubmittedUnverified(_)));
        let entry = required(journal.read(txid.as_str())?, "missing submitted entry")?;
        assert_eq!(entry.state, JournalEntryState::Delivered);
        assert_eq!(
            entry.variants.attempts.last().map(|a| &a.outcome),
            Some(&DeliveryOutcome::SubmittedUnverified)
        );
        let mut startup = StartupInbox::default();
        let summary = recover_unsent_to_inbox(&journal, &mut startup)?;
        assert_eq!(summary.recovered, 0);
        assert!(startup.delivered.is_empty());

        // Two dispatching rungs would describe a double insertion: rejected
        // and kept recoverable.
        let journal = TestJournal::new();
        let txid = TransactionId::new("tx-routed-double");
        let terminal = run(
            &txid,
            vec![
                routed_attempt("clipboard-copy:wl-copy", DeliveryOutcome::CopiedOnly),
                routed_attempt(
                    "wtype-virtual-keyboard",
                    DeliveryOutcome::SubmittedUnverified,
                ),
            ],
            &journal,
        )?;
        assert!(matches!(
            terminal,
            RecoveryTerminal::FailedRecoverable { .. }
        ));
        Ok(())
    }

    #[test]
    fn routed_reports_distinguish_verified_commit_from_inbox_recovery() -> TestResult {
        // Verified commit: the last rung observed a durable target result and
        // the ladder ended there. No Inbox write is claimed or required.
        let journal = TestJournal::new();
        let verified_txid = TransactionId::new("tx-routed-verified");
        let mut capture = Capture;
        let mut asr = Asr;
        let mut correct = Correct;
        let mut delivery = RoutedReport {
            report: routed_report(
                &verified_txid,
                vec![routed_attempt(
                    "portal-eis-text",
                    DeliveryOutcome::Delivered,
                )],
                DeliveryOutcome::Delivered,
            ),
        };
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let verified_terminal = RecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut delivery,
            delivery_mode: RecoveryDeliveryMode::TargetRoutedThenInboxRecovery,
        }
        .run(
            SessionId(21),
            verified_txid.clone(),
            "clean".to_owned(),
            &SessionControl::new(),
            &mut |_| Ok(()),
        )?;
        assert!(matches!(
            verified_terminal,
            RecoveryTerminal::DeliveredInbox(_)
        ));
        let verified_entry = required(
            journal.read(verified_txid.as_str())?,
            "missing verified routed journal entry",
        )?;
        assert_eq!(verified_entry.state, JournalEntryState::Delivered);
        assert_eq!(verified_entry.pipeline_id.as_deref(), Some("clean"));
        assert_eq!(
            verified_entry
                .delivery_intent
                .as_ref()
                .map(|intent| intent.backend.as_str()),
            Some("routed-then-inbox"),
            "the portable delivery intent must name the routed mode, not a lane id"
        );
        assert_eq!(
            verified_entry.variants.delivered.as_deref(),
            Some("controlled raw.")
        );

        // Unverified submission plus bounded failures: the adapter ran the
        // ladder and then one durable Inbox recovery write under the same
        // transaction. The terminal fact is recovery, never a delivery claim.
        let recovered_txid = TransactionId::new("tx-routed-recovered");
        let journal = TestJournal::new();
        let mut capture = Capture;
        let mut asr = Asr;
        let mut correct = Correct;
        let mut delivery = RoutedReport {
            report: routed_report(
                &recovered_txid,
                vec![
                    routed_attempt("portal-eis-text", DeliveryOutcome::Failed),
                    routed_attempt(
                        "wtype-virtual-keyboard",
                        DeliveryOutcome::SubmittedUnverified,
                    ),
                    routed_attempt("clipboard-copy:wl-copy", DeliveryOutcome::CopiedOnly),
                    routed_inbox_attempt(),
                ],
                DeliveryOutcome::InboxFallback,
            ),
        };
        let recovered_terminal = RecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut delivery,
            delivery_mode: RecoveryDeliveryMode::TargetRoutedThenInboxRecovery,
        }
        .run(
            SessionId(22),
            recovered_txid.clone(),
            "raw".to_owned(),
            &SessionControl::new(),
            &mut |_| Ok(()),
        )?;
        assert!(matches!(
            recovered_terminal,
            RecoveryTerminal::InboxRecovered(_)
        ));
        let recovered_entry = required(
            journal.read(recovered_txid.as_str())?,
            "missing routed recovery journal entry",
        )?;
        assert_eq!(recovered_entry.state, JournalEntryState::InboxRecovered);
        assert_eq!(
            recovered_entry
                .delivery_report
                .as_ref()
                .map(|report| report.attempts.len()),
            Some(4),
            "observed attempts must be preserved for recovery"
        );
        assert_eq!(
            recovered_entry.variants.delivered.as_deref(),
            Some("controlled raw.")
        );
        Ok(())
    }

    /// Every report shape the routed ladder must never report (or a
    /// stale/forged replay must never smuggle in) is rejected by the real
    /// portable tail, and the transaction stays truthfully recoverable.
    #[test]
    fn routed_report_shape_rejects_double_commit_and_untruthful_verified_commit() -> TestResult {
        let malformed = [
            "intermediate-verified-commit-then-inbox",
            "inbox-outcome-without-final-inbox-attempt",
            "verified-outcome-without-final-verified-attempt",
            "wrong-report-backend",
            "final-failure-reason",
            "copied-only-after-verified-commit",
            "verified-commit-on-inbox-rung",
            "inbox-only-verified-commit",
            "verified-attempt-with-fallback-reason",
            "empty-verified-backend",
            "empty-direct-backend",
        ];
        for (index, kind) in malformed.into_iter().enumerate() {
            let journal = TestJournal::new();
            let txid = TransactionId::new(format!("tx-routed-malformed-{index}"));
            let mut report = routed_report(
                &txid,
                vec![
                    routed_attempt("portal-eis-text", DeliveryOutcome::SubmittedUnverified),
                    routed_inbox_attempt(),
                ],
                DeliveryOutcome::InboxFallback,
            );
            match kind {
                "intermediate-verified-commit-then-inbox" => {
                    report.attempts.insert(
                        0,
                        routed_attempt("portal-eis-text", DeliveryOutcome::Delivered),
                    );
                }
                "inbox-outcome-without-final-inbox-attempt" => {
                    report.attempts = vec![routed_attempt(
                        "wtype-virtual-keyboard",
                        DeliveryOutcome::SubmittedUnverified,
                    )];
                }
                "verified-outcome-without-final-verified-attempt" => {
                    report.outcome = DeliveryOutcome::Delivered;
                }
                "wrong-report-backend" => report.backend = "wtype-then-inbox".to_owned(),
                "final-failure-reason" => {
                    report.failure_reason = Some(FallbackReason::BackendError {
                        code: "CONTROLLED".to_owned(),
                    });
                }
                "copied-only-after-verified-commit" => {
                    report.attempts.push(routed_attempt(
                        "clipboard-copy:wl-copy",
                        DeliveryOutcome::CopiedOnly,
                    ));
                    report.outcome = DeliveryOutcome::Delivered;
                }
                "verified-commit-on-inbox-rung" => {
                    // The durable recovery rung is relabelled as the verified
                    // external commit: Inbox can only ever be recovery.
                    report.attempts[1] = routed_attempt("inbox", DeliveryOutcome::Delivered);
                    report.outcome = DeliveryOutcome::Delivered;
                }
                "inbox-only-verified-commit" => {
                    report.attempts = vec![routed_attempt("inbox", DeliveryOutcome::Delivered)];
                    report.outcome = DeliveryOutcome::Delivered;
                }
                "verified-attempt-with-fallback-reason" => {
                    report.attempts = vec![routed_attempt(
                        "portal-eis-text",
                        DeliveryOutcome::Delivered,
                    )];
                    report.attempts[0].fallback_reason = Some(FallbackReason::BackendError {
                        code: "CONTROLLED".to_owned(),
                    });
                    report.outcome = DeliveryOutcome::Delivered;
                }
                "empty-verified-backend" => {
                    report.attempts = vec![routed_attempt("", DeliveryOutcome::Delivered)];
                    report.outcome = DeliveryOutcome::Delivered;
                }
                "empty-direct-backend" => {
                    report.attempts[0].backend = String::new();
                }
                unexpected => {
                    return Err(std::io::Error::other(format!(
                        "unexpected routed malformed matrix case: {unexpected}"
                    ))
                    .into());
                }
            }
            assert!(
                !is_target_routed_delivery_report(
                    RecoveryDeliveryMode::TargetRoutedThenInboxRecovery,
                    &report
                ),
                "{kind} must not qualify as routed recovery"
            );
            let observed_attempts = report.attempts.len();

            let mut capture = Capture;
            let mut asr = Asr;
            let mut correct = Correct;
            let mut delivery = RoutedReport { report };
            let policy = PolicyEngine::from_document(PolicyDocument::default());
            let terminal = RecoverySlice {
                journal: &journal,
                capture: &mut capture,
                asr: &mut asr,
                policy: &policy,
                corrector: &mut correct,
                delivery: &mut delivery,
                delivery_mode: RecoveryDeliveryMode::TargetRoutedThenInboxRecovery,
            }
            .run(
                SessionId(200 + index as u128),
                txid.clone(),
                "clean".to_owned(),
                &SessionControl::new(),
                &mut |_| Ok(()),
            )?;
            assert!(
                matches!(
                    terminal,
                    RecoveryTerminal::FailedRecoverable { ref code }
                        if code == "delivery_outcome_not_routed_recovery"
                            || code == "delivery_identity_mismatch"
                ),
                "{kind} must not reach a terminal delivery fact: {terminal:?}"
            );
            let entry = required(
                journal.read(txid.as_str())?,
                "missing routed malformed journal entry",
            )?;
            assert_eq!(
                entry.state,
                JournalEntryState::FailedRecoverable,
                "{kind} must leave a truthful failed journal state"
            );
            assert!(
                entry.failure_code.is_some(),
                "{kind} must record the rejection instead of a delivery fact"
            );
            assert!(
                entry.variants.delivered.is_none(),
                "{kind} must never persist the corrected text as delivered"
            );
            if kind != "wrong-report-backend" {
                // The adapter's observed attempts stay durable even when the
                // report shape is rejected, so the transaction remains
                // truthfully recoverable.
                assert_eq!(
                    entry
                        .delivery_report
                        .as_ref()
                        .map(|report| report.attempts.len()),
                    Some(observed_attempts),
                    "{kind} must preserve the adapter's observed attempts for recovery"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn controlled_wtype_inbox_report_is_persisted_as_inbox_recovery_not_delivered() -> TestResult {
        let journal = TestJournal::new();
        let txid = TransactionId::new("tx-wtype-recovered");
        let mut capture = Capture;
        let mut asr = Asr;
        let mut correct = Correct;
        let mut delivery = WtypeInboxReport {
            report: wtype_inbox_report(txid.clone(), DeliveryOutcome::SubmittedUnverified),
            cancel_after_dispatch: false,
        };
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let mut slice = RecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut delivery,
            delivery_mode: RecoveryDeliveryMode::WtypeThenInboxRecovery,
        };
        let terminal = slice.run(
            SessionId(13),
            txid.clone(),
            "linux-mic-wtype-inbox".to_owned(),
            &SessionControl::new(),
            &mut |_| Ok(()),
        )?;
        assert!(matches!(terminal, RecoveryTerminal::InboxRecovered(_)));
        let entry = required(
            journal.read(txid.as_str())?,
            "missing wtype recovery journal entry",
        )?;
        assert_eq!(entry.state, JournalEntryState::InboxRecovered);
        assert_eq!(
            required(
                entry.delivery_intent.as_ref(),
                "missing wtype delivery intent",
            )?
            .backend,
            "wtype-then-inbox"
        );
        assert_eq!(
            required(
                entry.delivery_report.as_ref(),
                "missing wtype delivery report",
            )?
            .attempts
            .len(),
            2
        );
        assert_ne!(entry.state, JournalEntryState::Delivered);
        Ok(())
    }

    #[test]
    fn controlled_portal_eis_report_uses_exact_backend_contract_and_inbox_recovery() -> TestResult {
        let journal = TestJournal::new();
        let txid = TransactionId::new("tx-portal-eis-recovered");
        let report = portal_eis_inbox_report(txid.clone(), DeliveryOutcome::SubmittedUnverified);
        assert!(is_direct_inbox_recovery_report(
            RecoveryDeliveryMode::PortalEisThenInboxRecovery,
            &report
        ));

        let mut wrong_direct_backend = report.clone();
        wrong_direct_backend.attempts[0].backend = "wtype-stdin".to_owned();
        assert!(!is_direct_inbox_recovery_report(
            RecoveryDeliveryMode::PortalEisThenInboxRecovery,
            &wrong_direct_backend
        ));

        let mut capture = Capture;
        let mut asr = Asr;
        let mut correct = Correct;
        let mut delivery = WtypeInboxReport {
            report,
            cancel_after_dispatch: false,
        };
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let mut slice = RecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut delivery,
            delivery_mode: RecoveryDeliveryMode::PortalEisThenInboxRecovery,
        };
        let terminal = slice.run(
            SessionId(18),
            txid.clone(),
            "controlled-portal-eis-inbox".to_owned(),
            &SessionControl::new(),
            &mut |_| Ok(()),
        )?;
        assert!(matches!(terminal, RecoveryTerminal::InboxRecovered(_)));
        let entry = required(
            journal.read(txid.as_str())?,
            "missing portal recovery journal entry",
        )?;
        assert_eq!(entry.state, JournalEntryState::InboxRecovered);
        assert_eq!(
            required(
                entry.delivery_intent.as_ref(),
                "missing portal delivery intent",
            )?
            .backend,
            "portal-eis-then-inbox"
        );
        assert_eq!(
            required(
                entry.delivery_report.as_ref(),
                "missing portal delivery report",
            )?
            .attempts
            .len(),
            2
        );
        assert_ne!(entry.state, JournalEntryState::Delivered);
        Ok(())
    }

    #[test]
    fn controlled_wtype_attempt_observes_durable_corrected_text_and_intent_first() -> TestResult {
        let journal = TestJournal::new();
        let txid = TransactionId::new("tx-wtype-intent-first");
        let mut capture = Capture;
        let mut asr = Asr;
        let mut correct = Correct;
        let mut delivery = IntentCommittedBeforeControlledDirectAttempt { journal: &journal };
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let mut slice = RecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut delivery,
            delivery_mode: RecoveryDeliveryMode::WtypeThenInboxRecovery,
        };

        assert!(matches!(
            slice.run(
                SessionId(16),
                txid.clone(),
                "linux-mic-wtype-inbox".to_owned(),
                &SessionControl::new(),
                &mut |_| Ok(()),
            )?,
            RecoveryTerminal::InboxRecovered(_)
        ));
        Ok(())
    }

    #[test]
    fn controlled_final_journal_failure_after_inbox_recovery_keeps_actual_report_recoverable()
    -> TestResult {
        let journal = TestJournal::new();
        let txid = TransactionId::new("tx-wtype-final-journal-failure");
        let mut capture = Capture;
        let mut asr = Asr;
        let mut correct = Correct;
        let mut delivery = WtypeInboxReportThenFinalJournalFailure {
            journal: &journal,
            report: wtype_inbox_report(txid.clone(), DeliveryOutcome::SubmittedUnverified),
        };
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let mut slice = RecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut delivery,
            delivery_mode: RecoveryDeliveryMode::WtypeThenInboxRecovery,
        };

        assert_eq!(
            slice.run(
                SessionId(17),
                txid.clone(),
                "linux-mic-wtype-inbox".to_owned(),
                &SessionControl::new(),
                &mut |_| Ok(()),
            )?,
            RecoveryTerminal::FailedRecoverable {
                code: "journal_write_after_delivery_uncertain".to_owned()
            }
        );
        let entry = required(
            journal.read(txid.as_str())?,
            "missing final-journal-failure entry",
        )?;
        assert_eq!(entry.state, JournalEntryState::Committed);
        assert_eq!(
            required(
                entry.delivery_report.as_ref(),
                "missing durable delivery report",
            )?
            .attempts
            .len(),
            2
        );
        assert!(entry.variants.delivered.is_none());
        Ok(())
    }

    #[test]
    fn controlled_inbox_failure_retains_the_actual_direct_attempt_report() -> TestResult {
        let journal = TestJournal::new();
        let txid = TransactionId::new("tx-wtype-inbox-failed");
        let mut capture = Capture;
        let mut asr = Asr;
        let mut correct = Correct;
        let report = DeliveryReport {
            transaction_id: txid.clone(),
            backend: "wtype-then-inbox".to_owned(),
            attempts: vec![DeliveryAttempt {
                backend: "wtype-stdin".to_owned(),
                outcome: DeliveryOutcome::SubmittedUnverified,
                fallback_reason: None,
                at_unix_ms: 1,
            }],
            outcome: DeliveryOutcome::Failed,
            failure_reason: Some(FallbackReason::BackendError {
                code: "INBOX_WRITE_FAILED".to_owned(),
            }),
        };
        let mut delivery = WtypeInboxReport {
            report,
            cancel_after_dispatch: false,
        };
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let mut slice = RecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut delivery,
            delivery_mode: RecoveryDeliveryMode::WtypeThenInboxRecovery,
        };
        assert_eq!(
            slice.run(
                SessionId(14),
                txid.clone(),
                "linux-mic-wtype-inbox".to_owned(),
                &SessionControl::new(),
                &mut |_| Ok(()),
            )?,
            RecoveryTerminal::FailedRecoverable {
                code: "delivery_outcome_not_inbox_recovery".to_owned()
            }
        );
        let entry = required(
            journal.read(txid.as_str())?,
            "missing Inbox-failure journal entry",
        )?;
        assert_eq!(entry.state, JournalEntryState::FailedRecoverable);
        assert_eq!(
            required(entry.delivery_report, "missing direct-attempt report")?
                .attempts
                .len(),
            1
        );
        Ok(())
    }

    #[test]
    fn controlled_late_cancellation_after_wtype_dispatch_is_not_clean_cancelled() -> TestResult {
        let journal = TestJournal::new();
        let txid = TransactionId::new("tx-wtype-late-cancel");
        let mut capture = Capture;
        let mut asr = Asr;
        let mut correct = Correct;
        let mut delivery = WtypeInboxReport {
            report: wtype_inbox_report(txid.clone(), DeliveryOutcome::SubmittedUnverified),
            cancel_after_dispatch: true,
        };
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let control = SessionControl::new();
        let mut slice = RecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut delivery,
            delivery_mode: RecoveryDeliveryMode::WtypeThenInboxRecovery,
        };
        let terminal = slice.run(
            SessionId(15),
            txid.clone(),
            "linux-mic-wtype-inbox".to_owned(),
            &control,
            &mut |_| Ok(()),
        )?;
        assert!(matches!(terminal, RecoveryTerminal::InboxRecovered(_)));
        assert_ne!(terminal, RecoveryTerminal::Cancelled);
        assert_eq!(
            required(
                journal.read(txid.as_str())?,
                "missing late-cancellation journal entry"
            )?
            .state,
            JournalEntryState::InboxRecovered
        );
        Ok(())
    }

    #[derive(Clone, Copy)]
    enum StreamingCaptureBehavior {
        Success,
        ReadinessFails,
        EndpointFixture,
        FailAfterFirst,
        CancelAfterFirst,
        ReportCancelledWithoutToken,
        AbortFailsAfterFirst,
        SummaryMismatch,
        ShortEof,
        /// Ends like `ShortEof` after 8192 samples (over half a second).
        LongEof,
    }

    struct ControlledStreamingCapture {
        behavior: StreamingCaptureBehavior,
        started: Arc<AtomicBool>,
        aborted: Arc<AtomicBool>,
        order: Option<Arc<Mutex<Vec<&'static str>>>>,
    }

    impl StreamingAudioCapturePort for ControlledStreamingCapture {
        fn start_stream(
            &mut self,
            control: &SessionControl,
        ) -> Result<Box<dyn StreamingAudioCaptureSession>, PlatformError> {
            if let Some(order) = &self.order {
                order
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push("capture_start");
            }
            self.started.store(true, Ordering::SeqCst);
            Ok(Box::new(ControlledStreamingCaptureSession {
                behavior: self.behavior,
                info: CaptureStreamInfo {
                    source_selection: "controlled-stream-source".to_owned(),
                    sample_rate_hz: 16_000,
                    channels: 1,
                },
                next: 0,
                pending: None,
                control: control.clone(),
                aborted: Arc::clone(&self.aborted),
                order: self.order.clone(),
            }))
        }
    }

    struct ControlledStreamingCaptureSession {
        behavior: StreamingCaptureBehavior,
        info: CaptureStreamInfo,
        next: usize,
        pending: Option<CaptureStreamChunk>,
        control: SessionControl,
        aborted: Arc<AtomicBool>,
        order: Option<Arc<Mutex<Vec<&'static str>>>>,
    }

    impl StreamingAudioCaptureSession for ControlledStreamingCaptureSession {
        fn info(&self) -> &CaptureStreamInfo {
            &self.info
        }

        fn wait_ready(&mut self, control: &SessionControl) -> Result<(), PlatformError> {
            if control.is_cancelled() {
                return Err(PlatformError {
                    code: "cancelled",
                    message: "controlled stream cancelled".to_owned(),
                });
            }
            if let Some(order) = &self.order {
                order
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push("capture_ready");
            }
            if matches!(self.behavior, StreamingCaptureBehavior::ReadinessFails) {
                return Err(PlatformError {
                    code: "capture_readiness_failed",
                    message: "controlled readiness failure".to_owned(),
                });
            }
            self.pending = Some(CaptureStreamChunk {
                sequence: 0,
                audio: AudioChunk {
                    sample_rate_hz: 16_000,
                    channels: 1,
                    samples: if matches!(self.behavior, StreamingCaptureBehavior::EndpointFixture) {
                        vec![0.05; 1_280]
                    } else if matches!(self.behavior, StreamingCaptureBehavior::LongEof) {
                        vec![0.25; 4_096]
                    } else {
                        vec![0.25; 4]
                    },
                },
                first_frame_after_start_us: Some(10),
            });
            self.next = 1;
            Ok(())
        }

        fn next_event(
            &mut self,
            control: &SessionControl,
        ) -> Result<CaptureStreamEvent, PlatformError> {
            if matches!(self.behavior, StreamingCaptureBehavior::EndpointFixture)
                && control.is_finalization_requested()
            {
                return Ok(CaptureStreamEvent::End(CaptureStreamSummary {
                    end: CaptureEnd::EndpointDetected,
                    total_samples: u64::try_from(self.next)
                        .unwrap_or(u64::MAX)
                        .saturating_mul(1_280),
                    first_frame_after_start_us: Some(10),
                    capture_duration_us: 800,
                }));
            }
            if let Some(pending) = self.pending.take() {
                return Ok(CaptureStreamEvent::Audio(pending));
            }
            let next = self.next;
            self.next += 1;
            if matches!(self.behavior, StreamingCaptureBehavior::FailAfterFirst) && next == 1 {
                return Err(PlatformError {
                    code: "capture_backpressure",
                    message: "controlled bounded queue overflow".to_owned(),
                });
            }
            if matches!(self.behavior, StreamingCaptureBehavior::CancelAfterFirst) && next == 1 {
                self.control.cancel();
                return Err(PlatformError {
                    code: "cancelled",
                    message: "controlled stream cancellation".to_owned(),
                });
            }
            if matches!(
                self.behavior,
                StreamingCaptureBehavior::ReportCancelledWithoutToken
            ) && next == 1
            {
                return Err(PlatformError {
                    code: "cancelled",
                    message: "component reported cancellation without caller token".to_owned(),
                });
            }
            if matches!(
                self.behavior,
                StreamingCaptureBehavior::AbortFailsAfterFirst
            ) && next == 1
            {
                return Err(PlatformError {
                    code: "capture_source_failed",
                    message: "controlled capture source failure".to_owned(),
                });
            }
            // 3 speech chunks (240ms) then 27 silent chunks (2.16s). The silent
            // run must exceed the endpoint hang-over, so lengthening that
            // policy cannot silently stop this fixture from exercising the
            // endpoint at all.
            if matches!(self.behavior, StreamingCaptureBehavior::EndpointFixture) && next < 30 {
                return Ok(CaptureStreamEvent::Audio(CaptureStreamChunk {
                    sequence: u64::try_from(next).unwrap_or(u64::MAX),
                    audio: AudioChunk {
                        sample_rate_hz: 16_000,
                        channels: 1,
                        samples: vec![if next < 3 { 0.05 } else { 0.0 }; 1_280],
                    },
                    first_frame_after_start_us: (next == 0).then_some(10),
                }));
            }
            if next < 2 {
                let len = if matches!(self.behavior, StreamingCaptureBehavior::LongEof) {
                    4_096
                } else {
                    4
                };
                return Ok(CaptureStreamEvent::Audio(CaptureStreamChunk {
                    sequence: u64::try_from(next).unwrap_or(u64::MAX),
                    audio: AudioChunk {
                        sample_rate_hz: 16_000,
                        channels: 1,
                        samples: vec![0.25; len],
                    },
                    first_frame_after_start_us: (next == 0).then_some(10),
                }));
            }
            Ok(CaptureStreamEvent::End(CaptureStreamSummary {
                end: if matches!(
                    self.behavior,
                    StreamingCaptureBehavior::ShortEof | StreamingCaptureBehavior::LongEof
                ) {
                    CaptureEnd::SourceEnded
                } else {
                    CaptureEnd::SafetyLimit
                },
                total_samples: match self.behavior {
                    StreamingCaptureBehavior::SummaryMismatch => 7,
                    StreamingCaptureBehavior::LongEof => 8_192,
                    // The fixture ran to its end without an endpoint: every
                    // 80 ms chunk it produced is accounted for.
                    StreamingCaptureBehavior::EndpointFixture => u64::try_from(next)
                        .unwrap_or(u64::MAX)
                        .saturating_mul(1_280),
                    _ => 8,
                },
                first_frame_after_start_us: Some(10),
                capture_duration_us: 500,
            }))
        }

        fn abort(&mut self, _: Instant) -> Result<(), PlatformError> {
            self.aborted.store(true, Ordering::SeqCst);
            if matches!(
                self.behavior,
                StreamingCaptureBehavior::AbortFailsAfterFirst
            ) {
                return Err(PlatformError {
                    code: "capture_cleanup_failed",
                    message: "controlled capture cleanup failure".to_owned(),
                });
            }
            Ok(())
        }
    }

    struct ControlledStreamingEngine {
        streaming: bool,
        push_error: Option<&'static str>,
        final_before_finalize: bool,
        opened: Arc<AtomicBool>,
        cancelled: Arc<AtomicBool>,
        finished: Arc<AtomicBool>,
        pushed_chunks: Arc<AtomicU64>,
        order: Option<Arc<Mutex<Vec<&'static str>>>>,
        open_error: Option<&'static str>,
        cancel_on_open: bool,
    }

    impl AsrEngine for ControlledStreamingEngine {
        fn capabilities(&self) -> AsrCapabilities {
            AsrCapabilities {
                streaming: self.streaming,
                language_detection: false,
                translation: false,
                confidence: false,
                hotword_biasing: false,
                timestamps: false,
                ..AsrCapabilities::default()
            }
        }

        fn open_stream(
            &mut self,
            session: SessionId,
            cancellation: &SessionCancellation,
        ) -> Result<Box<dyn AsrStream>, AsrError> {
            if let Some(order) = &self.order {
                order
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push("asr_open");
            }
            self.opened.store(true, Ordering::SeqCst);
            if self.cancel_on_open {
                cancellation.cancel();
                return Err(AsrError::new(
                    "cancelled",
                    "controlled admission cancellation",
                ));
            }
            if let Some(code) = self.open_error {
                return Err(AsrError::new(code, "controlled admission failure"));
            }
            let mut pending = VecDeque::from([AsrEvent::Started { session }]);
            if self.final_before_finalize {
                pending.push_back(AsrEvent::Final {
                    seq: 1,
                    transcript: Transcript {
                        text: "premature final".to_owned(),
                        is_final: true,
                    },
                    confidence: None,
                    first_audio_offset_ms: None,
                });
            }
            Ok(Box::new(ControlledStreamingAsr {
                session,
                cancellation: cancellation.clone(),
                pending,
                push_error: self.push_error,
                next_sequence: 0,
                finalized: false,
                cancelled: Arc::clone(&self.cancelled),
                finished: Arc::clone(&self.finished),
                pushed_chunks: Arc::clone(&self.pushed_chunks),
            }))
        }
    }

    struct ControlledStreamingAsr {
        session: SessionId,
        cancellation: SessionCancellation,
        pending: VecDeque<AsrEvent>,
        push_error: Option<&'static str>,
        next_sequence: u64,
        finalized: bool,
        cancelled: Arc<AtomicBool>,
        finished: Arc<AtomicBool>,
        pushed_chunks: Arc<AtomicU64>,
    }

    impl AsrStream for ControlledStreamingAsr {
        fn session(&self) -> SessionId {
            self.session
        }

        fn cancellation(&self) -> SessionCancellation {
            self.cancellation.clone()
        }

        fn push_audio(&mut self, _: &AudioChunk) -> Result<(), AsrError> {
            if let Some(code) = self.push_error {
                return Err(AsrError::new(code, "controlled push failure"));
            }
            self.pushed_chunks.fetch_add(1, Ordering::SeqCst);
            self.next_sequence += 1;
            self.pending.push_back(AsrEvent::Partial {
                seq: self.next_sequence,
                transcript: Transcript {
                    text: "volatile partial must not persist".to_owned(),
                    is_final: false,
                },
            });
            Ok(())
        }

        fn poll_event(&mut self) -> Option<AsrEvent> {
            self.pending.pop_front()
        }

        fn finalize(&mut self) -> Result<(), AsrError> {
            self.next_sequence += 1;
            self.pending.push_back(AsrEvent::Final {
                seq: self.next_sequence,
                transcript: Transcript {
                    text: "controlled streaming final".to_owned(),
                    is_final: true,
                },
                confidence: None,
                first_audio_offset_ms: None,
            });
            self.finalized = true;
            Ok(())
        }

        fn cancel(&self) {
            self.cancelled.store(true, Ordering::SeqCst);
            self.cancellation.cancel();
        }

        fn finish(&mut self, _: Duration) -> Result<(), AsrError> {
            self.finished.store(true, Ordering::SeqCst);
            if self.finalized || self.cancellation.is_cancelled() {
                Ok(())
            } else {
                Err(AsrError::new(
                    "stream_not_terminal",
                    "controlled stream was not terminated",
                ))
            }
        }
    }

    fn streaming_engine(
        push_error: Option<&'static str>,
        final_before_finalize: bool,
    ) -> (
        ControlledStreamingEngine,
        Arc<AtomicBool>,
        Arc<AtomicBool>,
        Arc<AtomicBool>,
    ) {
        let (engine, opened, cancelled, finished, _) =
            streaming_engine_with_counter(push_error, final_before_finalize);
        (engine, opened, cancelled, finished)
    }

    fn streaming_engine_with_counter(
        push_error: Option<&'static str>,
        final_before_finalize: bool,
    ) -> (
        ControlledStreamingEngine,
        Arc<AtomicBool>,
        Arc<AtomicBool>,
        Arc<AtomicBool>,
        Arc<AtomicU64>,
    ) {
        let opened = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let pushed_chunks = Arc::new(AtomicU64::new(0));
        (
            ControlledStreamingEngine {
                streaming: true,
                push_error,
                final_before_finalize,
                opened: Arc::clone(&opened),
                cancelled: Arc::clone(&cancelled),
                finished: Arc::clone(&finished),
                pushed_chunks: Arc::clone(&pushed_chunks),
                order: None,
                open_error: None,
                cancel_on_open: false,
            },
            opened,
            cancelled,
            finished,
            pushed_chunks,
        )
    }

    fn streaming_capture(
        behavior: StreamingCaptureBehavior,
    ) -> (ControlledStreamingCapture, Arc<AtomicBool>, Arc<AtomicBool>) {
        let started = Arc::new(AtomicBool::new(false));
        let aborted = Arc::new(AtomicBool::new(false));
        (
            ControlledStreamingCapture {
                behavior,
                started: Arc::clone(&started),
                aborted: Arc::clone(&aborted),
                order: None,
            },
            started,
            aborted,
        )
    }

    struct FailingVad;

    impl VoiceActivityDetector for FailingVad {
        fn classify(&mut self, _: &AudioChunk) -> Result<&[SpeechActivity], EndpointError> {
            Err(EndpointError {
                code: "controlled_vad_failed",
            })
        }

        fn window_frames(&self, sample_rate_hz: u32) -> Result<u64, EndpointError> {
            Ok(u64::from(sample_rate_hz))
        }
    }

    #[test]
    fn controlled_streaming_success_uses_only_validated_final_for_shared_tail() -> TestResult {
        let journal = TestJournal::new();
        let (mut capture, _, aborted) = streaming_capture(StreamingCaptureBehavior::Success);
        let (mut asr, _, cancelled, finished, pushed_chunks) =
            streaming_engine_with_counter(None, false);
        let mut correct = Correct;
        let mut inbox = Inbox;
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let mut slice = StreamingRecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut inbox,
            delivery_mode: RecoveryDeliveryMode::InboxPrimary,
            endpointing: None,

            partial_observer: None,
        };
        let terminal = slice.run(
            SessionId(81),
            TransactionId::new("tx-stream-success"),
            "controlled-streaming-inbox".to_owned(),
            &SessionControl::new(),
            &mut |_| Ok(()),
        )?;
        assert!(matches!(terminal, RecoveryTerminal::DeliveredInbox(_)));
        let entry = required(
            journal.read("tx-stream-success")?,
            "missing streaming success journal entry",
        )?;
        assert_eq!(
            entry.variants.raw.as_deref(),
            Some("controlled streaming final")
        );
        // The streaming branch must retain the typed end reason durably; the
        // controlled fixture terminates at the explicit safety bound.
        assert_eq!(entry.capture_end, Some(CaptureEnd::SafetyLimit));
        assert!(!aborted.load(Ordering::SeqCst));
        assert!(!cancelled.load(Ordering::SeqCst));
        assert!(finished.load(Ordering::SeqCst));
        assert_eq!(pushed_chunks.load(Ordering::SeqCst), 2);
        Ok(())
    }

    /// Counts delivery calls; a kept-undelivered dictation must make none.
    struct ForbiddenDelivery(Arc<AtomicU64>);
    impl DeliveryPort for ForbiddenDelivery {
        fn deliver(
            &mut self,
            _: &DeliveryIntent,
            _: &str,
            _: &SessionCancellation,
        ) -> Result<DeliveryReport, PlatformError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(PlatformError {
                code: "delivery_forbidden",
                message: "a kept-undelivered dictation must not deliver".to_owned(),
            })
        }
    }

    /// D6/F-H2: a microphone lost after more than half a second keeps what
    /// was said: the stream is finished like a Stop, the text is corrected
    /// and journaled, and nothing is delivered.
    #[test]
    fn controlled_streaming_source_ended_keeps_the_text_undelivered() -> TestResult {
        let journal = TestJournal::new();
        let (mut capture, _, aborted) = streaming_capture(StreamingCaptureBehavior::LongEof);
        let (mut asr, opened, cancelled, finished, pushed_chunks) =
            streaming_engine_with_counter(None, false);
        let mut correct = Correct;
        let delivered = Arc::new(AtomicU64::new(0));
        let mut delivery = ForbiddenDelivery(Arc::clone(&delivered));
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let mut slice = StreamingRecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut delivery,
            delivery_mode: RecoveryDeliveryMode::InboxPrimary,
            endpointing: None,
            partial_observer: None,
        };
        let terminal = slice.run(
            SessionId(83),
            TransactionId::new("tx-stream-source-ended-kept"),
            "controlled-streaming-inbox".to_owned(),
            &SessionControl::new(),
            &mut |_| Ok(()),
        )?;
        assert_eq!(
            terminal,
            RecoveryTerminal::FailedRecoverable {
                code: CAPTURE_SOURCE_ENDED.to_owned()
            }
        );
        let entry = required(
            journal.read("tx-stream-source-ended-kept")?,
            "missing kept source-ended journal entry",
        )?;
        assert_eq!(entry.state, JournalEntryState::FailedRecoverable);
        assert_eq!(entry.capture_end, Some(CaptureEnd::SourceEnded));
        assert_eq!(
            entry.variants.raw.as_deref(),
            Some("controlled streaming final")
        );
        assert!(entry.variants.corrected.is_some());
        assert!(entry.variants.delivered.is_none());
        // No delivery intent: startup replay never sends it anywhere.
        assert!(entry.delivery_intent.is_none());
        assert_eq!(delivered.load(Ordering::SeqCst), 0);
        assert!(opened.load(Ordering::SeqCst));
        assert!(finished.load(Ordering::SeqCst));
        assert!(!cancelled.load(Ordering::SeqCst));
        assert!(!aborted.load(Ordering::SeqCst));
        assert_eq!(pushed_chunks.load(Ordering::SeqCst), 2);
        Ok(())
    }

    #[test]
    fn controlled_streaming_source_ended_fails_terminal_without_committing_final_text() -> TestResult
    {
        let journal = TestJournal::new();
        let (mut capture, _, aborted) = streaming_capture(StreamingCaptureBehavior::ShortEof);
        let (mut asr, opened, cancelled, finished, pushed_chunks) =
            streaming_engine_with_counter(None, false);
        let mut correct = Correct;
        let mut inbox = Inbox;
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let mut slice = StreamingRecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut inbox,
            delivery_mode: RecoveryDeliveryMode::InboxPrimary,
            endpointing: None,
            partial_observer: None,
        };
        let terminal = slice.run(
            SessionId(82),
            TransactionId::new("tx-stream-source-ended"),
            "controlled-streaming-inbox".to_owned(),
            &SessionControl::new(),
            &mut |_| Ok(()),
        )?;
        assert_eq!(
            terminal,
            RecoveryTerminal::FailedTerminal {
                code: "capture_source_ended".to_owned()
            }
        );
        let entry = required(
            journal.read("tx-stream-source-ended")?,
            "missing streaming source-ended journal entry",
        )?;
        assert_eq!(entry.state, JournalEntryState::FailedTerminal);
        assert_eq!(entry.failure_code.as_deref(), Some("capture_source_ended"));
        assert_eq!(entry.capture_end, Some(CaptureEnd::SourceEnded));
        // Already-fed ASR state was cancelled and finished through the owned
        // cleanup path, capture was aborted, and no final variant committed.
        assert!(opened.load(Ordering::SeqCst));
        assert!(cancelled.load(Ordering::SeqCst));
        assert!(finished.load(Ordering::SeqCst));
        assert!(aborted.load(Ordering::SeqCst));
        assert_eq!(pushed_chunks.load(Ordering::SeqCst), 2);
        assert!(entry.variants.raw.is_none());
        assert!(entry.variants.corrected.is_none());
        assert!(entry.variants.delivered.is_none());
        assert!(entry.delivery_intent.is_none());
        assert!(entry.delivery_report.is_none());
        Ok(())
    }

    #[test]
    fn controlled_monotonic_timing_records_fixed_success_boundaries_once() -> TestResult {
        let journal = TestJournal::new();
        let (mut capture, _, _) = streaming_capture(StreamingCaptureBehavior::Success);
        let (mut asr, _, _, _) = streaming_engine(None, false);
        let mut correct = Correct;
        let mut inbox = Inbox;
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let timing = RecoveryTimingTrace::new();
        let mut slice = StreamingRecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut inbox,
            delivery_mode: RecoveryDeliveryMode::InboxPrimary,
            endpointing: None,
            partial_observer: None,
        };

        assert!(matches!(
            slice.run_with_timing(
                SessionId(181),
                TransactionId::new("tx-stream-timing-success"),
                "controlled-streaming-inbox".to_owned(),
                &SessionControl::new(),
                &mut |_| Ok(()),
                &timing,
            )?,
            RecoveryTerminal::DeliveredInbox(_)
        ));
        assert_eq!(timing.observed_count(), RecoveryTimingStage::ALL.len());
        let offsets = RecoveryTimingStage::ALL
            .into_iter()
            .map(|stage| required(timing.offset_us(stage), "missing timing stage"))
            .collect::<Result<Vec<_>, _>>()?;
        assert!(offsets.windows(2).all(|pair| pair[0] <= pair[1]));

        let first = required(
            timing.offset_us(RecoveryTimingStage::FirstPartialAccepted),
            "missing first partial timing",
        )?;
        std::thread::sleep(Duration::from_millis(2));
        timing.observe(RecoveryTimingStage::FirstPartialAccepted);
        assert_eq!(
            timing.offset_us(RecoveryTimingStage::FirstPartialAccepted),
            Some(first)
        );
        Ok(())
    }

    #[test]
    fn failed_correction_never_claims_corrected_commit_or_delivery_timing() -> TestResult {
        let journal = TestJournal::new();
        let (mut capture, _, _) = streaming_capture(StreamingCaptureBehavior::Success);
        let (mut asr, _, _, _) = streaming_engine(None, false);
        let mut correct = FailingCorrect;
        let mut inbox = Inbox;
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let timing = RecoveryTimingTrace::new();
        let mut slice = StreamingRecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut inbox,
            delivery_mode: RecoveryDeliveryMode::InboxPrimary,
            endpointing: None,
            partial_observer: None,
        };

        assert_eq!(
            slice.run_with_timing(
                SessionId(183),
                TransactionId::new("tx-stream-timing-correction-failure"),
                "controlled-streaming-inbox".to_owned(),
                &SessionControl::new(),
                &mut |_| Ok(()),
                &timing,
            )?,
            RecoveryTerminal::FailedTerminal {
                code: "correction_failed".to_owned()
            }
        );
        assert!(
            timing
                .offset_us(RecoveryTimingStage::RawFinalAccepted)
                .is_some()
        );
        for stage in [
            RecoveryTimingStage::CorrectedDurablyCommitted,
            RecoveryTimingStage::DeliveryReturned,
            RecoveryTimingStage::TerminalJournalCommitted,
        ] {
            assert_eq!(timing.offset_us(stage), None, "fabricated {stage:?}");
        }
        Ok(())
    }

    #[test]
    fn rejected_partial_and_failed_session_do_not_manufacture_later_timings() -> TestResult {
        let journal = TestJournal::new();
        let (mut capture, _, _) = streaming_capture(StreamingCaptureBehavior::Success);
        let (mut asr, _, cancelled, finished) = streaming_engine(None, false);
        let mut correct = Correct;
        let mut inbox = Inbox;
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let timing = RecoveryTimingTrace::new();
        let mut reject = |_: VolatilePartial| Err("partial_observer_failed");
        let mut slice = StreamingRecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut inbox,
            delivery_mode: RecoveryDeliveryMode::InboxPrimary,
            endpointing: None,
            partial_observer: Some(&mut reject),
        };

        assert_eq!(
            slice.run_with_timing(
                SessionId(182),
                TransactionId::new("tx-stream-timing-rejected"),
                "controlled-streaming-inbox".to_owned(),
                &SessionControl::new(),
                &mut |_| Ok(()),
                &timing,
            )?,
            RecoveryTerminal::FailedTerminal {
                code: "partial_observer_failed".to_owned()
            }
        );
        for stage in [
            RecoveryTimingStage::FirstPartialAccepted,
            RecoveryTimingStage::CaptureEnded,
            RecoveryTimingStage::RawFinalAccepted,
            RecoveryTimingStage::CorrectedDurablyCommitted,
            RecoveryTimingStage::DeliveryReturned,
            RecoveryTimingStage::TerminalJournalCommitted,
        ] {
            assert_eq!(timing.offset_us(stage), None, "fabricated {stage:?}");
        }
        assert!(
            timing
                .offset_us(RecoveryTimingStage::FirstAudioAccepted)
                .is_some()
        );
        assert!(cancelled.load(Ordering::SeqCst));
        assert!(finished.load(Ordering::SeqCst));
        Ok(())
    }

    #[test]
    fn controlled_capture_is_ready_and_listening_before_asr_admission() -> TestResult {
        let journal = TestJournal::new();
        let order = Arc::new(Mutex::new(Vec::new()));
        let (mut capture, _, _) = streaming_capture(StreamingCaptureBehavior::Success);
        capture.order = Some(Arc::clone(&order));
        let (mut asr, _, _, _, pushed_chunks) = streaming_engine_with_counter(None, false);
        asr.order = Some(Arc::clone(&order));
        let mut correct = Correct;
        let mut inbox = Inbox;
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let order_for_observer = Arc::clone(&order);
        let mut slice = StreamingRecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut inbox,
            delivery_mode: RecoveryDeliveryMode::InboxPrimary,
            endpointing: None,
            partial_observer: None,
        };
        assert!(matches!(
            slice.run(
                SessionId(94),
                TransactionId::new("tx-stream-capture-first"),
                "controlled-streaming-inbox".to_owned(),
                &SessionControl::new(),
                &mut |state| {
                    if state == SessionState::Listening {
                        order_for_observer
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .push("listening");
                    }
                    Ok(())
                },
            )?,
            RecoveryTerminal::DeliveredInbox(_)
        ));
        assert_eq!(
            *order
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            ["capture_start", "capture_ready", "listening", "asr_open"]
        );
        assert_eq!(pushed_chunks.load(Ordering::SeqCst), 2);
        Ok(())
    }

    #[test]
    fn controlled_capture_readiness_failure_opens_no_asr_owner() -> TestResult {
        let journal = TestJournal::new();
        let (mut capture, started, aborted) =
            streaming_capture(StreamingCaptureBehavior::ReadinessFails);
        let (mut asr, opened, cancelled, finished) = streaming_engine(None, false);
        let mut correct = Correct;
        let mut inbox = Inbox;
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let mut slice = StreamingRecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut inbox,
            delivery_mode: RecoveryDeliveryMode::InboxPrimary,
            endpointing: None,
            partial_observer: None,
        };

        assert_eq!(
            slice.run(
                SessionId(95),
                TransactionId::new("tx-stream-readiness-failure"),
                "controlled-streaming-inbox".to_owned(),
                &SessionControl::new(),
                &mut |_| Ok(()),
            )?,
            RecoveryTerminal::FailedTerminal {
                code: "capture_readiness_failed".to_owned(),
            }
        );
        assert!(started.load(Ordering::SeqCst));
        assert!(aborted.load(Ordering::SeqCst));
        assert!(!opened.load(Ordering::SeqCst));
        assert!(!cancelled.load(Ordering::SeqCst));
        assert!(!finished.load(Ordering::SeqCst));
        Ok(())
    }

    #[test]
    fn controlled_asr_admission_failure_aborts_capture_without_asr_owner() -> TestResult {
        let journal = TestJournal::new();
        let (mut capture, _, aborted) = streaming_capture(StreamingCaptureBehavior::Success);
        let (mut asr, opened, cancelled, finished) = streaming_engine(None, false);
        asr.open_error = Some("asr_admission_failed");
        let mut correct = Correct;
        let mut inbox = Inbox;
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let mut slice = StreamingRecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut inbox,
            delivery_mode: RecoveryDeliveryMode::InboxPrimary,
            endpointing: None,
            partial_observer: None,
        };

        assert_eq!(
            slice.run(
                SessionId(96),
                TransactionId::new("tx-stream-asr-admission-failure"),
                "controlled-streaming-inbox".to_owned(),
                &SessionControl::new(),
                &mut |_| Ok(()),
            )?,
            RecoveryTerminal::FailedTerminal {
                code: "asr_admission_failed".to_owned(),
            }
        );
        assert!(opened.load(Ordering::SeqCst));
        assert!(aborted.load(Ordering::SeqCst));
        assert!(!cancelled.load(Ordering::SeqCst));
        assert!(!finished.load(Ordering::SeqCst));
        Ok(())
    }

    #[test]
    fn controlled_capture_cleanup_failure_precedes_asr_admission_failure() -> TestResult {
        let journal = TestJournal::new();
        let txid = TransactionId::new("tx-stream-admission-cleanup-failure");
        let (mut capture, _, aborted) =
            streaming_capture(StreamingCaptureBehavior::AbortFailsAfterFirst);
        let (mut asr, opened, cancelled, finished) = streaming_engine(None, false);
        asr.open_error = Some("asr_admission_failed");
        let mut correct = Correct;
        let mut inbox = Inbox;
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let mut slice = StreamingRecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut inbox,
            delivery_mode: RecoveryDeliveryMode::InboxPrimary,
            endpointing: None,
            partial_observer: None,
        };

        assert_eq!(
            slice.run(
                SessionId(98),
                txid.clone(),
                "controlled-streaming-inbox".to_owned(),
                &SessionControl::new(),
                &mut |_| Ok(()),
            )?,
            RecoveryTerminal::FailedTerminal {
                code: "capture_cleanup_failed".to_owned(),
            }
        );
        assert_eq!(
            required(journal.read(txid.as_str())?, "missing cleanup journal")?
                .failure_code
                .as_deref(),
            Some("capture_cleanup_failed")
        );
        assert!(opened.load(Ordering::SeqCst));
        assert!(aborted.load(Ordering::SeqCst));
        assert!(!cancelled.load(Ordering::SeqCst));
        assert!(!finished.load(Ordering::SeqCst));
        Ok(())
    }

    #[test]
    fn controlled_asr_admission_cancellation_aborts_capture_and_is_typed() -> TestResult {
        let journal = TestJournal::new();
        let (mut capture, _, aborted) = streaming_capture(StreamingCaptureBehavior::Success);
        let (mut asr, opened, cancelled, finished) = streaming_engine(None, false);
        asr.cancel_on_open = true;
        let control = SessionControl::new();
        let mut correct = Correct;
        let mut inbox = Inbox;
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let mut slice = StreamingRecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut inbox,
            delivery_mode: RecoveryDeliveryMode::InboxPrimary,
            endpointing: None,
            partial_observer: None,
        };

        assert_eq!(
            slice.run(
                SessionId(97),
                TransactionId::new("tx-stream-asr-admission-cancelled"),
                "controlled-streaming-inbox".to_owned(),
                &control,
                &mut |_| Ok(()),
            )?,
            RecoveryTerminal::Cancelled
        );
        assert!(control.is_cancelled());
        assert!(opened.load(Ordering::SeqCst));
        assert!(aborted.load(Ordering::SeqCst));
        assert!(!cancelled.load(Ordering::SeqCst));
        assert!(!finished.load(Ordering::SeqCst));
        Ok(())
    }

    #[test]
    fn controlled_listening_observer_cancellation_opens_no_asr_owner() -> TestResult {
        let journal = TestJournal::new();
        let (mut capture, _, aborted) = streaming_capture(StreamingCaptureBehavior::Success);
        let (mut asr, opened, cancelled, finished) = streaming_engine(None, false);
        let control = SessionControl::new();
        let observer_control = control.clone();
        let mut correct = Correct;
        let mut inbox = Inbox;
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let mut slice = StreamingRecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut inbox,
            delivery_mode: RecoveryDeliveryMode::InboxPrimary,
            endpointing: None,
            partial_observer: None,
        };

        assert_eq!(
            slice.run(
                SessionId(99),
                TransactionId::new("tx-stream-listening-cancelled"),
                "controlled-streaming-inbox".to_owned(),
                &control,
                &mut |state| {
                    if state == SessionState::Listening {
                        observer_control.cancel();
                    }
                    Ok(())
                },
            )?,
            RecoveryTerminal::Cancelled
        );
        assert!(aborted.load(Ordering::SeqCst));
        assert!(!opened.load(Ordering::SeqCst));
        assert!(!cancelled.load(Ordering::SeqCst));
        assert!(!finished.load(Ordering::SeqCst));
        Ok(())
    }

    #[test]
    fn controlled_partials_are_volatile_ordered_and_never_enter_the_journal() -> TestResult {
        let journal = TestJournal::new();
        let txid = TransactionId::new("tx-stream-partials");
        let (mut capture, _, _) = streaming_capture(StreamingCaptureBehavior::Success);
        let (mut asr, _, _, _) = streaming_engine(None, false);
        let mut correct = Correct;
        let mut inbox = Inbox;
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let mut observed = Vec::new();
        let mut journal_was_clean = true;
        let mut observe_partial = |partial: VolatilePartial| {
            observed.push(partial);
            let entry = journal.read(txid.as_str()).ok().flatten();
            journal_was_clean &= entry.is_some_and(|entry| {
                entry.variants.raw.is_none()
                    && entry.variants.corrected.is_none()
                    && entry.variants.delivered.is_none()
                    && entry.delivery_intent.is_none()
                    && entry.delivery_report.is_none()
            });
            Ok(())
        };
        let mut slice = StreamingRecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut inbox,
            delivery_mode: RecoveryDeliveryMode::InboxPrimary,
            endpointing: None,
            partial_observer: Some(&mut observe_partial),
        };

        assert!(matches!(
            slice.run(
                SessionId(91),
                txid.clone(),
                "controlled-streaming-inbox".to_owned(),
                &SessionControl::new(),
                &mut |_| Ok(()),
            )?,
            RecoveryTerminal::DeliveredInbox(_)
        ));
        assert!(journal_was_clean);
        assert!(!observed.is_empty());
        assert!(observed.windows(2).all(|pair| pair[0].seq < pair[1].seq));
        assert!(
            observed
                .iter()
                .all(|partial| partial.text == "volatile partial must not persist")
        );
        Ok(())
    }

    #[test]
    fn controlled_partial_observer_failure_cleans_both_stream_owners() -> TestResult {
        let journal = TestJournal::new();
        let (mut capture, _, aborted) = streaming_capture(StreamingCaptureBehavior::Success);
        let (mut asr, _, cancelled, finished) = streaming_engine(None, false);
        let mut correct = Correct;
        let mut inbox = Inbox;
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let mut reject = |_: VolatilePartial| Err("partial_observer_failed");
        let mut slice = StreamingRecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut inbox,
            delivery_mode: RecoveryDeliveryMode::InboxPrimary,
            endpointing: None,
            partial_observer: Some(&mut reject),
        };

        assert_eq!(
            slice.run(
                SessionId(92),
                TransactionId::new("tx-stream-partial-rejected"),
                "controlled-streaming-inbox".to_owned(),
                &SessionControl::new(),
                &mut |_| Ok(()),
            )?,
            RecoveryTerminal::FailedTerminal {
                code: "partial_observer_failed".to_owned()
            }
        );
        assert!(aborted.load(Ordering::SeqCst));
        assert!(cancelled.load(Ordering::SeqCst));
        assert!(finished.load(Ordering::SeqCst));
        Ok(())
    }

    #[test]
    fn controlled_partial_validation_rejects_empty_and_oversized_hypotheses() {
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        for text in [String::new(), "x".repeat(MAX_VOLATILE_PARTIAL_BYTES + 1)] {
            let session = SessionId(93);
            let control = SessionControl::new();
            let mut stream = ControlledStreamingAsr {
                session,
                cancellation: control.cancellation().clone(),
                pending: VecDeque::from([
                    AsrEvent::Started { session },
                    AsrEvent::Partial {
                        seq: 1,
                        transcript: Transcript {
                            text,
                            is_final: false,
                        },
                    },
                ]),
                push_error: None,
                next_sequence: 1,
                finalized: false,
                cancelled: Arc::new(AtomicBool::new(false)),
                finished: Arc::new(AtomicBool::new(false)),
                pushed_chunks: Arc::new(AtomicU64::new(0)),
            };
            assert_eq!(
                AsrEventTracker::new(session).drain(&mut stream, false, &policy, None, None),
                Err("asr_partial_invalid")
            );
        }
    }

    #[test]
    fn controlled_endpoint_gracefully_ends_stream_before_safety_limit() -> TestResult {
        let journal = TestJournal::new();
        let (mut capture, _, aborted) =
            streaming_capture(StreamingCaptureBehavior::EndpointFixture);
        let (mut asr, _, cancelled, finished, pushed_chunks) =
            streaming_engine_with_counter(None, false);
        let mut correct = Correct;
        let mut inbox = Inbox;
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let control = SessionControl::new();
        let mut vad = NativeEnergyVad::default();
        let mut endpointer = DictationEndpointer::default();
        let mut slice = StreamingRecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut inbox,
            delivery_mode: RecoveryDeliveryMode::InboxPrimary,
            endpointing: Some(StreamingEndpointing {
                vad: &mut vad,
                endpointer: &mut endpointer,
            }),

            partial_observer: None,
        };
        assert!(matches!(
            slice.run(
                SessionId(89),
                TransactionId::new("tx-stream-endpoint"),
                "controlled-streaming-inbox".to_owned(),
                &control,
                &mut |_| Ok(()),
            )?,
            RecoveryTerminal::DeliveredInbox(_)
        ));
        assert!(control.is_finalization_requested());
        assert!(!control.is_cancelled());
        assert!(!aborted.load(Ordering::SeqCst));
        assert!(!cancelled.load(Ordering::SeqCst));
        assert!(finished.load(Ordering::SeqCst));
        // Every chunk the endpoint has observed reaches the ASR: endpointing
        // decides when to stop capturing, it never trims or withholds audio.
        // Three speech chunks, then as many silence chunks as the hang-over
        // needs, and no more.
        let hangover_ms =
            u64::from(crate::endpoint::DictationEndpointConfig::default().trailing_silence_ms);
        let hangover_frames = hangover_ms * 16_000 / 1_000;
        let chunk_frames = 1_280_u64;
        let silence_chunks = hangover_frames.div_ceil(chunk_frames);
        assert_eq!(
            pushed_chunks.load(Ordering::SeqCst),
            3 + silence_chunks,
            "endpoint must stop after the full hang-over, not a truncated one"
        );
        Ok(())
    }

    /// T2.6: the daemon composes no endpointer, so a long silence is just more
    /// audio. The session keeps listening and ends only when capture does.
    #[test]
    fn stream_without_endpointing_keeps_listening_through_silence() -> TestResult {
        let journal = TestJournal::new();
        let (mut capture, _, aborted) =
            streaming_capture(StreamingCaptureBehavior::EndpointFixture);
        let (mut asr, _, cancelled, finished, pushed_chunks) =
            streaming_engine_with_counter(None, false);
        let mut correct = Correct;
        let mut inbox = Inbox;
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let control = SessionControl::new();
        let mut slice = StreamingRecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut inbox,
            delivery_mode: RecoveryDeliveryMode::InboxPrimary,
            endpointing: None,
            partial_observer: None,
        };
        assert!(matches!(
            slice.run(
                SessionId(90),
                TransactionId::new("tx-stream-no-endpoint"),
                "controlled-streaming-inbox".to_owned(),
                &control,
                &mut |_| Ok(()),
            )?,
            RecoveryTerminal::DeliveredInbox(_)
        ));
        // Three speech chunks then 27 silent ones (2.16 s, longer than the
        // endpoint hang-over): nothing asked the session to stop, and all 30
        // chunks reached the ASR.
        assert!(!control.is_finalization_requested());
        assert!(!control.is_cancelled());
        assert!(!aborted.load(Ordering::SeqCst));
        assert!(!cancelled.load(Ordering::SeqCst));
        assert!(finished.load(Ordering::SeqCst));
        assert_eq!(pushed_chunks.load(Ordering::SeqCst), 30);
        Ok(())
    }

    #[test]
    fn controlled_vad_failure_cleans_both_stream_owners() -> TestResult {
        let journal = TestJournal::new();
        let (mut capture, _, aborted) = streaming_capture(StreamingCaptureBehavior::Success);
        let (mut asr, _, cancelled, finished) = streaming_engine(None, false);
        let mut correct = Correct;
        let mut inbox = Inbox;
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let mut vad = FailingVad;
        let mut endpointer = DictationEndpointer::default();
        let mut slice = StreamingRecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut inbox,
            delivery_mode: RecoveryDeliveryMode::InboxPrimary,
            endpointing: Some(StreamingEndpointing {
                vad: &mut vad,
                endpointer: &mut endpointer,
            }),

            partial_observer: None,
        };
        assert_eq!(
            slice.run(
                SessionId(90),
                TransactionId::new("tx-stream-vad-failure"),
                "controlled-streaming-inbox".to_owned(),
                &SessionControl::new(),
                &mut |_| Ok(()),
            )?,
            RecoveryTerminal::FailedTerminal {
                code: "controlled_vad_failed".to_owned()
            }
        );
        assert!(aborted.load(Ordering::SeqCst));
        assert!(cancelled.load(Ordering::SeqCst));
        assert!(finished.load(Ordering::SeqCst));
        Ok(())
    }

    #[test]
    fn controlled_capture_failure_cancels_and_finishes_asr_without_delivery() -> TestResult {
        let journal = TestJournal::new();
        let (mut capture, _, aborted) = streaming_capture(StreamingCaptureBehavior::FailAfterFirst);
        let (mut asr, _, cancelled, finished) = streaming_engine(None, false);
        let mut correct = Correct;
        let delivery_called = Arc::new(AtomicBool::new(false));
        let mut delivery = DeliverySpy(Arc::clone(&delivery_called));
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let mut slice = StreamingRecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut delivery,
            delivery_mode: RecoveryDeliveryMode::InboxPrimary,
            endpointing: None,

            partial_observer: None,
        };
        assert_eq!(
            slice.run(
                SessionId(82),
                TransactionId::new("tx-stream-capture-fail"),
                "controlled-streaming-inbox".to_owned(),
                &SessionControl::new(),
                &mut |_| Ok(()),
            )?,
            RecoveryTerminal::FailedTerminal {
                code: "capture_backpressure".to_owned()
            }
        );
        assert!(aborted.load(Ordering::SeqCst));
        assert!(cancelled.load(Ordering::SeqCst));
        assert!(finished.load(Ordering::SeqCst));
        assert!(!delivery_called.load(Ordering::SeqCst));
        Ok(())
    }

    #[test]
    fn controlled_streaming_cancellation_reaps_both_owners_and_never_delivers() -> TestResult {
        let journal = TestJournal::new();
        let (mut capture, _, aborted) =
            streaming_capture(StreamingCaptureBehavior::CancelAfterFirst);
        let (mut asr, _, cancelled, finished) = streaming_engine(None, false);
        let mut correct = Correct;
        let delivery_called = Arc::new(AtomicBool::new(false));
        let mut delivery = DeliverySpy(Arc::clone(&delivery_called));
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let mut slice = StreamingRecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut delivery,
            delivery_mode: RecoveryDeliveryMode::InboxPrimary,
            endpointing: None,

            partial_observer: None,
        };
        assert_eq!(
            slice.run(
                SessionId(86),
                TransactionId::new("tx-stream-cancelled"),
                "controlled-streaming-inbox".to_owned(),
                &SessionControl::new(),
                &mut |_| Ok(()),
            )?,
            RecoveryTerminal::Cancelled
        );
        assert!(aborted.load(Ordering::SeqCst));
        assert!(cancelled.load(Ordering::SeqCst));
        assert!(finished.load(Ordering::SeqCst));
        assert!(!delivery_called.load(Ordering::SeqCst));
        assert_eq!(
            required(
                journal.read("tx-stream-cancelled")?,
                "missing streaming cancellation journal entry",
            )?
            .state,
            JournalEntryState::Cancelled
        );
        Ok(())
    }

    #[test]
    fn controlled_streaming_component_cancel_code_without_token_is_a_failure() -> TestResult {
        let journal = TestJournal::new();
        let (mut capture, _, aborted) =
            streaming_capture(StreamingCaptureBehavior::ReportCancelledWithoutToken);
        let (mut asr, _, cancelled, finished) = streaming_engine(None, false);
        let mut correct = Correct;
        let delivery_called = Arc::new(AtomicBool::new(false));
        let mut delivery = DeliverySpy(Arc::clone(&delivery_called));
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let control = SessionControl::new();
        assert!(!control.is_cancelled());
        let mut slice = StreamingRecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut delivery,
            delivery_mode: RecoveryDeliveryMode::InboxPrimary,
            endpointing: None,

            partial_observer: None,
        };
        assert_eq!(
            slice.run(
                SessionId(87),
                TransactionId::new("tx-stream-component-cancel-code"),
                "controlled-streaming-inbox".to_owned(),
                &control,
                &mut |_| Ok(()),
            )?,
            RecoveryTerminal::FailedTerminal {
                code: "cancelled".to_owned()
            }
        );
        // Owner teardown signals the shared token only after the component
        // failure has already been classified from the caller's prior state.
        assert!(control.is_cancelled());
        assert!(aborted.load(Ordering::SeqCst));
        assert!(cancelled.load(Ordering::SeqCst));
        assert!(finished.load(Ordering::SeqCst));
        assert!(!delivery_called.load(Ordering::SeqCst));
        assert_eq!(
            required(
                journal.read("tx-stream-component-cancel-code")?,
                "missing component cancellation failure journal entry",
            )?
            .state,
            JournalEntryState::FailedTerminal
        );
        Ok(())
    }

    #[test]
    fn controlled_stream_cleanup_failure_masks_source_failure_but_attempts_asr_finish() -> TestResult
    {
        let journal = TestJournal::new();
        let (mut capture, _, aborted) =
            streaming_capture(StreamingCaptureBehavior::AbortFailsAfterFirst);
        let (mut asr, _, cancelled, finished) = streaming_engine(None, false);
        let mut correct = Correct;
        let mut inbox = Inbox;
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let mut slice = StreamingRecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut inbox,
            delivery_mode: RecoveryDeliveryMode::InboxPrimary,
            endpointing: None,

            partial_observer: None,
        };
        assert_eq!(
            slice.run(
                SessionId(87),
                TransactionId::new("tx-stream-cleanup-fail"),
                "controlled-streaming-inbox".to_owned(),
                &SessionControl::new(),
                &mut |_| Ok(()),
            )?,
            RecoveryTerminal::FailedTerminal {
                code: "capture_cleanup_failed".to_owned()
            }
        );
        assert!(aborted.load(Ordering::SeqCst));
        assert!(cancelled.load(Ordering::SeqCst));
        assert!(finished.load(Ordering::SeqCst));
        Ok(())
    }

    #[test]
    fn controlled_asr_failure_aborts_capture_and_preserves_original_code() -> TestResult {
        let journal = TestJournal::new();
        let (mut capture, _, aborted) = streaming_capture(StreamingCaptureBehavior::Success);
        let (mut asr, _, cancelled, finished) =
            streaming_engine(Some("controlled_asr_push_failed"), false);
        let mut correct = Correct;
        let mut inbox = Inbox;
        let policy = PolicyEngine::from_document(PolicyDocument::default());
        let mut slice = StreamingRecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &policy,
            corrector: &mut correct,
            delivery: &mut inbox,
            delivery_mode: RecoveryDeliveryMode::InboxPrimary,
            endpointing: None,

            partial_observer: None,
        };
        assert_eq!(
            slice.run(
                SessionId(83),
                TransactionId::new("tx-stream-asr-fail"),
                "controlled-streaming-inbox".to_owned(),
                &SessionControl::new(),
                &mut |_| Ok(()),
            )?,
            RecoveryTerminal::FailedTerminal {
                code: "controlled_asr_push_failed".to_owned()
            }
        );
        assert!(aborted.load(Ordering::SeqCst));
        assert!(cancelled.load(Ordering::SeqCst));
        assert!(finished.load(Ordering::SeqCst));
        Ok(())
    }

    #[test]
    fn controlled_stream_rejects_premature_final_and_mismatched_summary() -> TestResult {
        for (behavior, premature, expected) in [
            (
                StreamingCaptureBehavior::Success,
                true,
                "asr_event_order_invalid",
            ),
            (
                StreamingCaptureBehavior::SummaryMismatch,
                false,
                "capture_summary_mismatch",
            ),
        ] {
            let journal = TestJournal::new();
            let (mut capture, _, aborted) = streaming_capture(behavior);
            let (mut asr, _, cancelled, finished) = streaming_engine(None, premature);
            let mut correct = Correct;
            let mut inbox = Inbox;
            let policy = PolicyEngine::from_document(PolicyDocument::default());
            let mut slice = StreamingRecoverySlice {
                journal: &journal,
                capture: &mut capture,
                asr: &mut asr,
                policy: &policy,
                corrector: &mut correct,
                delivery: &mut inbox,
                delivery_mode: RecoveryDeliveryMode::InboxPrimary,
                endpointing: None,

                partial_observer: None,
            };
            let result = slice.run(
                SessionId(84),
                TransactionId::new(format!("tx-stream-invalid-{expected}")),
                "controlled-streaming-inbox".to_owned(),
                &SessionControl::new(),
                &mut |_| Ok(()),
            )?;
            assert_eq!(
                result,
                RecoveryTerminal::FailedTerminal {
                    code: expected.to_owned()
                }
            );
            assert!(cancelled.load(Ordering::SeqCst));
            assert!(finished.load(Ordering::SeqCst));
            if !premature {
                assert!(aborted.load(Ordering::SeqCst));
            }
        }
        Ok(())
    }

    #[test]
    fn controlled_streaming_audio_policy_denial_opens_no_owner() -> TestResult {
        let journal = TestJournal::new();
        let (mut capture, started, _) = streaming_capture(StreamingCaptureBehavior::Success);
        let (mut asr, opened, _, _) = streaming_engine(None, false);
        let mut correct = Correct;
        let mut inbox = Inbox;
        let mut slice = StreamingRecoverySlice {
            journal: &journal,
            capture: &mut capture,
            asr: &mut asr,
            policy: &DenyAudio,
            corrector: &mut correct,
            delivery: &mut inbox,
            delivery_mode: RecoveryDeliveryMode::InboxPrimary,
            endpointing: None,

            partial_observer: None,
        };
        assert_eq!(
            slice.run(
                SessionId(85),
                TransactionId::new("tx-stream-policy-denied"),
                "controlled-streaming-inbox".to_owned(),
                &SessionControl::new(),
                &mut |_| Ok(()),
            )?,
            RecoveryTerminal::FailedTerminal {
                code: "policy_denied_audio".to_owned()
            }
        );
        assert!(!started.load(Ordering::SeqCst));
        assert!(!opened.load(Ordering::SeqCst));
        Ok(())
    }

    #[test]
    fn controlled_streaming_state_rejection_cleans_owners_before_returning_error() -> TestResult {
        for rejected_state in [SessionState::Listening, SessionState::FinalizingAudio] {
            let journal = TestJournal::new();
            let (mut capture, _, aborted) = streaming_capture(StreamingCaptureBehavior::Success);
            let (mut asr, opened, cancelled, finished) = streaming_engine(None, false);
            let mut correct = Correct;
            let delivery_called = Arc::new(AtomicBool::new(false));
            let mut delivery = DeliverySpy(Arc::clone(&delivery_called));
            let policy = PolicyEngine::from_document(PolicyDocument::default());
            let suffix = if rejected_state == SessionState::Listening {
                "listening"
            } else {
                "finalizing"
            };
            let txid = TransactionId::new(format!("tx-stream-reject-{suffix}"));
            let mut slice = StreamingRecoverySlice {
                journal: &journal,
                capture: &mut capture,
                asr: &mut asr,
                policy: &policy,
                corrector: &mut correct,
                delivery: &mut delivery,
                delivery_mode: RecoveryDeliveryMode::InboxPrimary,
                endpointing: None,

                partial_observer: None,
            };
            let result = slice.run(
                SessionId(88),
                txid.clone(),
                "controlled-streaming-inbox".to_owned(),
                &SessionControl::new(),
                &mut |state| {
                    if state == rejected_state {
                        Err(CoreError::Worker("controlled state rejection".to_owned()))
                    } else {
                        Ok(())
                    }
                },
            );
            assert!(matches!(result, Err(CoreError::Worker(_))));
            assert!(aborted.load(Ordering::SeqCst));
            if rejected_state == SessionState::Listening {
                // Capture readiness is truthful before ASR admission. A
                // rejected Listening transition therefore owns capture only.
                assert!(!opened.load(Ordering::SeqCst));
                assert!(!cancelled.load(Ordering::SeqCst));
                assert!(!finished.load(Ordering::SeqCst));
            } else {
                assert!(opened.load(Ordering::SeqCst));
                assert!(cancelled.load(Ordering::SeqCst));
                assert!(finished.load(Ordering::SeqCst));
            }
            assert!(!delivery_called.load(Ordering::SeqCst));
            assert_eq!(
                required(
                    journal.read(txid.as_str())?,
                    "missing streaming state-rejection journal entry",
                )?
                .failure_code
                .as_deref(),
                Some("state_observation_failed")
            );
        }
        Ok(())
    }
}
