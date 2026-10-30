//! Session orchestration: DaemonState, run_session, IPC wiring.

#![forbid(unsafe_code)]
#![allow(unused_imports)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
#[cfg(test)]
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use narrivy_asr_api::{
    AsrCapabilities, AsrEngine, AsrRequest, AsrTag, ContextBias, OneShotAsrPort, SpokenLanguage,
    WrittenOutput,
};
use narrivy_asr_mock::MockAsr;
#[cfg(feature = "asr-transcribe-cpp")]
use narrivy_asr_transcribe::TranscribeCppEngine;
#[cfg(feature = "asr-transcribe-resident")]
use narrivy_asr_transcribe::{
    ResidentWorkerEngine,
    resident::{ResidentModelArtifactStatus, ResidentReadiness, ResidentWorkerArtifactStatus},
};
use narrivy_core::{
    CancelFlag, CoreError, CorrectionPort, DictationEndpointer, Journal, JournalEntryState,
    ModuleDescriptor, ModuleRegistry, NativeEnergyVad, RecoveryDeliveryMode, RecoverySlice,
    RecoveryTerminal, RecoveryTimingTrace, SessionMachine, StreamingEndpointing,
    StreamingRecoverySlice, VolatilePartial, WorkerSupervisor, valid_txid,
};
use narrivy_ipc::{
    ActivationSourceDto, ActiveSessionDto, Command, Envelope, Event, InstalledModelLifecycleDto,
    IpcErrorCode, MessageKind, ModelProvisionPublicationDto, ModelProvisionStateDto,
    ModelProvisionStatusDto, PROTOCOL_MINOR, PipelineSummaryDto, PortalLifecycleDto,
    RecognitionSettingsDto, ResponsePayload, SpokenLanguageDto, StageTimingsUs, StatusDto,
    ToggleOutcomeDto, ToggleResultDto,
};
use narrivy_ipc_unix::{PeerCredentials, UnixIpcServer, current_uid};
#[cfg(test)]
use narrivy_journal_unix::UnixJournal;
use narrivy_platform_api::DeliveryPort;
use narrivy_platform_linux::delivery::{
    ClipboardPasteRung, DeliveryPermissionEvidence, DeliveryPermissionPort, DirectRung,
    PortalEisTextRung, RoutedThenInboxDelivery, SystemWtypeKeyboardRung, TargetBinding,
    TargetBindingSource, UinputPasteChord, UnavailablePreActivationTargetBinding, UnavailableRung,
    WaylandClipboardHandoff, WtypePasteChord,
};
#[cfg(feature = "asr-transcribe-resident")]
use narrivy_platform_linux::installed_runtime::{
    InstalledModelRuntimeBinding, InstalledRuntimeBinding, bind_installed_model_controlled,
};
#[cfg(feature = "asr-transcribe-resident")]
use narrivy_platform_linux::model_import::{OfflineModelImportError, import_bundled_model};
use narrivy_platform_linux::recovery::PortalEisTextSubmitter;
use narrivy_policy::{ModulePermission, PolicyDocument, PolicyEngine};
use narrivy_types::{
    AudioChunk, DeliveryAttempt, DeliveryOutcome, FallbackReason, SessionControl, SessionId,
    SessionState, TransactionId, TranscriptVariants,
};
use rand::{TryRng, rngs::SysRng};

use super::capture::{AudioSourceKind, SAMPLE_RATE_HZ, parse_wav_mono, sine_fixture_samples};
use super::compute::ComputePlan;
use super::diagnostics::{
    DaemonDiagnostics, SessionOutcome, append_diagnostics_capabilities, content_safe,
};
use super::ipc_policy::IpcCommandPolicy;
#[cfg(any(test, not(feature = "asr-transcribe-resident")))]
use super::policy::capability_strings;
use super::policy::{
    ActivationFacts, AdmittedPipeline, AsrEngineKind, CLEAN_PIPELINE_ID, CorrectionMode,
    CorrectionStrategy, DEFAULT_PIPELINE_ID, DeliveryBackendKind, EngineConfig,
    LINUX_MIC_INBOX_PIPELINE_ID, LINUX_MIC_ROUTED_INBOX_LANE_ID, LINUX_MIC_WTYPE_INBOX_PIPELINE_ID,
    PipelineLane, RAW_PIPELINE_ID, ResidentArtifactBinding, ResidentCapabilitySnapshot,
    ResidentLifecycleCapability, StrictRecoveryLane, append_activation_capabilities,
    capability_strings_with_resident, default_pipeline_id, recovery_entry_detail,
    run_capability_check,
};
use super::portal::{PortalActorFailure, PortalPermissionService, PortalPermissionServiceFailure};
#[cfg(feature = "asr-transcribe-resident")]
use super::provisioning::{
    CandidateCommit, ProvisionCoordinator, ProvisionOperationKey, ProvisionPhaseKind,
    ProvisionStartError, ProvisionTerminal, ProvisionTransitionError, TerminalCommit,
};
use super::retained_history::{
    HISTORY_LIST_LIMIT, HISTORY_NOT_COMPOSED_DETAIL, HISTORY_UNAVAILABLE_DETAIL,
    MAX_HISTORY_ENTRIES, RecordOutcome, RetainedHistory,
};
use narrivy_platform_linux::portal_eis::{PortalParentWindow, PortalSessionFailure};
use narrivyd::bus::EventBus;

#[cfg(test)]
fn private_tempdir() -> std::io::Result<tempfile::TempDir> {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir()?;
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
    Ok(dir)
}

// ---------------------------------------------------------------------------
// Shared constants
// ---------------------------------------------------------------------------

/// Exit code when another daemon instance already owns the endpoint.
pub const EXIT_ALREADY_RUNNING: u8 = 2;
pub const EXIT_FAILURE: u8 = 1;

/// Emitted only when the daemon really owns an open retained History store.
/// It is the gate clients use before offering `GetHistory`.
const HISTORY_ENABLED_CAPABILITY: &str = "history.optional=enabled";
/// Emitted when no store is composed, so History stays fail-closed.
const HISTORY_NOT_COMPOSED_CAPABILITY: &str = "history.optional=disabled:not_composed";

/// Defensive wake bound if a lifecycle owner fails to signal the event bus.
/// Normal event publication and shutdown use condition-variable wakeups.
pub const PUMP_WAKE_FALLBACK: Duration = Duration::from_millis(250);
/// One user-mediated portal permission request. This remains below the IPC
/// connection idle bound and is cancellable from a separate same-user client.
pub const PORTAL_PERMISSION_REQUEST_TIMEOUT: Duration = Duration::from_secs(25);
/// Explicit revoke is non-interactive and must complete promptly.
pub const PORTAL_REVOKE_TIMEOUT: Duration = Duration::from_secs(2);
const TRANSACTION_ID_RANDOM_BYTES: usize = 16;
const TRANSACTION_ID_ATTEMPTS: usize = 8;
#[cfg(feature = "asr-transcribe-resident")]
const MODEL_PROVISION_COOLDOWN: Duration = Duration::from_secs(30);
#[cfg(feature = "asr-transcribe-resident")]
const MODEL_PROVISION_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Verifying, loading and self-testing a chosen model may take this long.
#[cfg(feature = "asr-transcribe-resident")]
const MODEL_SELECTION_TIMEOUT: Duration = Duration::from_secs(90);
/// An OpenVINO model's first load on an NPU compiles it, which takes
/// minutes for the large models; later loads read the compiled cache.
#[cfg(feature = "asr-transcribe-resident")]
const OPENVINO_LOAD_TIMEOUT: Duration = Duration::from_secs(360);
/// The model being replaced is given this long to stop.
#[cfg(feature = "asr-transcribe-resident")]
const REPLACED_MODEL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

pub fn exit_code(code: u8) -> std::process::ExitCode {
    std::process::ExitCode::from(code)
}

/// Stable lowercase wire string for a session state (ipc.md "Events").
pub fn state_wire(state: SessionState) -> &'static str {
    match state {
        SessionState::Idle => "idle",
        SessionState::Preparing => "preparing",
        SessionState::Listening => "listening",
        SessionState::FinalizingAudio => "finalizing_audio",
        SessionState::Transcribing => "transcribing",
        SessionState::Transforming => "transforming",
        SessionState::Delivering => "delivering",
        SessionState::Committed => "committed",
        SessionState::Cancelled => "cancelled",
        SessionState::Failed => "failed",
    }
}

// ---------------------------------------------------------------------------
// Session orchestration (P1 mock vertical)
// ---------------------------------------------------------------------------

/// Daemon event published onto the bounded bus. Transcript partials are
/// volatile presentation data and receive a redacted `Debug` representation.
#[derive(Clone)]
pub enum DaemonEvent {
    /// A session transition paired with the pipeline selected for that
    /// session.  The event pump must forward this value verbatim: inferring a
    /// pipeline later would let an event from the recovery lane masquerade as
    /// the daemon's historical mock default.
    Session {
        event: narrivy_types::SessionEvent,
        pipeline_id: Option<String>,
        /// How the text was delivered; only on `Committed`
        /// ([`delivery_result`]).
        delivery: Option<&'static str>,
    },
    Delivered {
        transaction_id: String,
        outcome: DeliveryOutcome,
    },
    Partial {
        session_id: u128,
        transaction_id: String,
        pipeline_id: String,
        partial: VolatilePartial,
    },
    /// A content-free daemon fact changed (for example a model began or
    /// finished loading). Forwarded as the wire `CapabilityChanged` event.
    Capability {
        capability: &'static str,
        available: bool,
    },
    /// The microphone level while listening (0..=1000). Volatile: a newer
    /// one replaces a queued one.
    Level { session_id: u128, level: u16 },
    /// A piece of an assistant reply (D33). Never dropped for room before
    /// recognition partials.
    AssistantDelta { turn_id: String, text: String },
    /// An assistant reply ended.
    AssistantDone {
        turn_id: String,
        outcome: String,
        text: String,
        /// A writing action's reply that failed a check.
        warning: Option<String>,
    },
    /// An event of an extension (voice mode, the assistant library), ready
    /// for clients as it is.
    Extension(Event),
    /// A meeting changed state or progress (content-free).
    MeetingChanged {
        id: String,
        state: String,
        progress: u16,
        stage: Option<String>,
    },
}

/// How a committed transaction's text was delivered, from its journaled
/// attempts: `inserted` (verified), `pasted` (clipboard paste, unverified),
/// `typed` (keyboard, unverified), `copied` (on the clipboard only) or
/// `inbox` (kept in the Inbox only). Content-free.
#[cfg(feature = "asr-transcribe-cpp")]
fn delivery_result(journal: &Journal, txid: &TransactionId) -> Option<&'static str> {
    let entry = journal.read(txid.as_str()).ok()??;
    Some(delivery_result_of(&entry.variants.attempts))
}

#[cfg(any(feature = "asr-transcribe-cpp", test))]
fn delivery_result_of(attempts: &[DeliveryAttempt]) -> &'static str {
    let direct = attempts.iter().find(|attempt| {
        matches!(
            attempt.outcome,
            DeliveryOutcome::Delivered
                | DeliveryOutcome::SubmittedUnverified
                | DeliveryOutcome::CopiedOnly
        )
    });
    match direct {
        Some(attempt) if attempt.outcome == DeliveryOutcome::Delivered => "inserted",
        Some(attempt) if attempt.outcome == DeliveryOutcome::CopiedOnly => "copied",
        Some(attempt) if attempt.backend.starts_with("clipboard") => "pasted",
        Some(_) => "typed",
        None => "inbox",
    }
}

/// Stands in for the live preview in builds without the resident engine.
#[cfg(all(
    feature = "asr-transcribe-cpp",
    not(feature = "asr-transcribe-resident")
))]
struct NoPreview;

#[cfg(all(
    feature = "asr-transcribe-cpp",
    not(feature = "asr-transcribe-resident")
))]
impl narrivy_core::LivePreviewPort for NoPreview {
    fn audio(&mut self, _: &[f32]) {}
    fn finish(&mut self) {}
}

/// Clean-up previews are numbered above any partial from recognition, so a
/// client that drops older partials accepts them.
#[cfg(feature = "asr-transcribe-resident")]
const REFINE_PREVIEW_SEQ_BASE: u64 = narrivy_ipc::REFINE_PREVIEW_SEQ_BASE;

/// Published while the assistant's model loads (D33).
pub const ASSISTANT_LOADING_CAPABILITY: &str = "assistant.loading";

impl DaemonState {
    /// Publish the microphone level; a queued older level for the session is
    /// replaced, and levels make room first when the queue is full.
    pub fn publish_level(&self, session_id: u128, level: u16) {
        self.bus.publish_coalesced(
            DaemonEvent::Level { session_id, level },
            |queued| matches!(queued, DaemonEvent::Level { session_id: queued, .. } if *queued == session_id),
            |queued| matches!(queued, DaemonEvent::Level { .. } | DaemonEvent::Partial { .. }),
        );
    }

    /// Publish an assistant event. Recognition partials make room first.
    pub fn publish_assistant(&self, event: DaemonEvent) {
        self.bus.publish_priority(event, |queued| {
            matches!(queued, DaemonEvent::Partial { .. })
        });
    }

    pub fn publish_assistant_loading(&self, loading: bool) {
        self.publish_assistant(DaemonEvent::Capability {
            capability: ASSISTANT_LOADING_CAPABILITY,
            available: loading,
        });
    }
}

/// Published while the daemon loads a model because a dictation asked for a
/// pipeline whose model or device is not the loaded one.
pub const ENGINE_LOADING_CAPABILITY: &str = "engine.loading";

impl std::fmt::Debug for DaemonEvent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Session {
                event,
                pipeline_id,
                delivery,
            } => formatter
                .debug_struct("Session")
                .field("event", event)
                .field("pipeline_id", pipeline_id)
                .field("delivery", delivery)
                .finish(),
            Self::Delivered {
                transaction_id,
                outcome,
            } => formatter
                .debug_struct("Delivered")
                .field("transaction_id", transaction_id)
                .field("outcome", outcome)
                .finish(),
            Self::Partial {
                session_id,
                transaction_id,
                pipeline_id,
                partial,
            } => formatter
                .debug_struct("Partial")
                .field("session_id", session_id)
                .field("transaction_id", transaction_id)
                .field("pipeline_id", pipeline_id)
                .field("seq", &partial.seq)
                .field("text", &"<redacted>")
                .finish(),
            Self::Level { session_id, level } => formatter
                .debug_struct("Level")
                .field("session_id", session_id)
                .field("level", level)
                .finish(),
            Self::Extension(event) => formatter.debug_tuple("Extension").field(event).finish(),
            Self::MeetingChanged {
                id,
                state,
                progress,
                ..
            } => formatter
                .debug_struct("MeetingChanged")
                .field("id", id)
                .field("state", state)
                .field("progress", progress)
                .finish_non_exhaustive(),
            Self::AssistantDelta { turn_id, .. } => formatter
                .debug_struct("AssistantDelta")
                .field("turn_id", turn_id)
                .finish_non_exhaustive(),
            Self::AssistantDone {
                turn_id, outcome, ..
            } => formatter
                .debug_struct("AssistantDone")
                .field("turn_id", turn_id)
                .field("outcome", outcome)
                .finish_non_exhaustive(),
            Self::Capability {
                capability,
                available,
            } => formatter
                .debug_struct("Capability")
                .field("capability", capability)
                .field("available", available)
                .finish(),
        }
    }
}

/// Inputs one started session needs to run without further interaction.
pub struct SessionRequest {
    pub audio_samples: Vec<f32>,
    pub sample_rate_hz: u32,
}

/// Truthful result of ONE delivery attempt, produced by the composed delivery
/// closure. `run_session` records it verbatim in the journal so evidence never
/// upgrades an outcome the backend did not report.
#[derive(Debug, Clone)]
pub struct DeliveryReport {
    /// Backend label recorded in `DeliveryAttempt.backend` (e.g.
    /// "null-platform", "wtype-virtual-keyboard", "inbox").
    pub backend: String,
    pub outcome: DeliveryOutcome,
    pub fallback_reason: Option<FallbackReason>,
}

impl DeliveryReport {
    pub fn delivered(backend: &str) -> Self {
        Self {
            backend: backend.to_owned(),
            outcome: DeliveryOutcome::Delivered,
            fallback_reason: None,
        }
    }
}

/// The deterministic corrector is injected into core rather than duplicated
/// in daemon orchestration.
/// Receives the text being cleaned up (see `DaemonCorrector::with_preview`).
#[cfg(feature = "asr-transcribe-cpp")]
type RefinePreview = Box<dyn FnMut(&str) + Send>;

#[cfg(feature = "asr-transcribe-cpp")]
struct DaemonCorrector(CorrectionStrategy, Option<RefinePreview>);

#[cfg(feature = "asr-transcribe-cpp")]
impl DaemonCorrector {
    /// Show the text while it is being cleaned up: `preview` receives the
    /// recognised text first, then the cleaned text when the model's reply
    /// is accepted (the overlay replaces one with the other in place).
    #[cfg(feature = "asr-transcribe-resident")]
    fn with_preview(mut self, preview: impl FnMut(&str) + Send + 'static) -> Self {
        self.1 = Some(Box::new(preview));
        self
    }
}

#[cfg(feature = "asr-transcribe-cpp")]
impl CorrectionPort for DaemonCorrector {
    fn correct(&mut self, raw: &str, cancellation: &CancelFlag) -> Result<String, CoreError> {
        if cancellation.is_cancelled() {
            return Err(CoreError::Cancelled);
        }
        let deterministic = self.0.applies(raw);
        // Clean mode: the clean-up model, when one is configured and ready,
        // and only when its reply is a faithful clean-up (D31). Otherwise the
        // deterministic text, so dictation never waits on a model load.
        if self.0 == CorrectionStrategy::FullCorrect
            && let Some(refiner) = super::refine::shared()
        {
            let ready = matches!(refiner.state(), super::refine::RefineState::Ready(_));
            if ready && let Some(preview) = self.1.as_mut() {
                preview(raw.trim());
            }
            match refiner.clean(raw.trim(), super::refine::CLEAN_BOUND) {
                Ok(cleaned)
                    if !cleaned.trim().is_empty() && super::refine::faithful(raw, &cleaned) =>
                {
                    let cleaned = super::text_options::finish_clean(cleaned.trim().to_owned());
                    if let Some(preview) = self.1.as_mut() {
                        preview(&cleaned);
                    }
                    return Ok(cleaned);
                }
                Ok(_) => eprintln!("narrivyd: clean-up reply refused by the faithfulness check"),
                Err("refine_loading") => {}
                Err(code) => eprintln!("narrivyd: clean-up skipped ({code})"),
            }
        }
        let deterministic = if self.0 == CorrectionStrategy::FullCorrect {
            super::text_options::finish_clean(deterministic)
        } else {
            deterministic
        };
        // The overlay always ends on the exact text being delivered.
        if let Some(preview) = self.1.as_mut() {
            preview(deterministic.trim());
        }
        Ok(deterministic)
    }
}

/// The one composed correction port for the owned strict lane. Both the batch
/// `RecoverySlice` and the `StreamingRecoverySlice` in the owned worker are
/// handed this value, so the admitted mode's deterministic strategy is the
/// only correction that can run for a session.
#[cfg(feature = "asr-transcribe-cpp")]
fn strict_composition_corrector(admitted: &AdmittedPipeline) -> DaemonCorrector {
    DaemonCorrector(admitted.correction(), None)
}

/// Permission evidence for one strict routed session.
///
/// The daemon-owned portal service is the only authority for the attended
/// keyboard grant: the external insertion boundary opens only while a grant is
/// active with a connected transport, and the daemon never requests permission
/// from a delivery attempt. Clipboard reading/writing is a separate privacy
/// boundary authorized explicitly by the composition, never by an attempt; the
/// authorization is still reported as evidence even though no contained
/// clipboard implementation is composed yet (the rungs are declared
/// unavailable).
#[cfg(feature = "asr-transcribe-cpp")]
struct SessionDeliveryPermission {
    portal: Option<Arc<PortalPermissionService>>,
    clipboard_authorized: bool,
}

#[cfg(feature = "asr-transcribe-cpp")]
impl DeliveryPermissionPort for SessionDeliveryPermission {
    fn evidence(&mut self) -> DeliveryPermissionEvidence {
        let keyboard_insert = self.portal.as_ref().is_some_and(|service| {
            let status = service.snapshot();
            // `sender_available`/`delivery_qualified` stay false until native
            // behavior is qualified; an unverified frame is never promoted to
            // a verified insert, so the dispatch precondition is the exact
            // evidence gate here.
            status.lifecycle == PortalLifecycleDto::Active
                && status.permission_active
                && status.transport_connected
        });
        DeliveryPermissionEvidence {
            keyboard_insert,
            clipboard_restore: self.clipboard_authorized,
        }
    }
}

/// The daemon's one clipboard connection. It lives for the daemon's lifetime
/// because a Wayland selection is served by the client that set it, which
/// includes the user's value Narrivy puts back after a paste. Connecting is
/// retried on the next dictation when it failed (no session yet, for
/// example).
#[cfg(feature = "asr-transcribe-cpp")]
fn shared_wayland_selection()
-> Option<Arc<narrivy_platform_linux::wayland_selection::WaylandSelection>> {
    static SELECTION: std::sync::Mutex<
        Option<Arc<narrivy_platform_linux::wayland_selection::WaylandSelection>>,
    > = std::sync::Mutex::new(None);
    let mut slot = SELECTION.lock().ok()?;
    // A selection whose worker stopped (the compositor closed the
    // connection, for example) answers nothing: connect again.
    if let Some(reason) = slot.as_ref().and_then(|selection| selection.stopped()) {
        eprintln!("narrivyd: clipboard connection stopped ({reason}); reconnecting");
        *slot = None;
    }
    if slot.is_none() {
        *slot = match narrivy_platform_linux::wayland_selection::WaylandSelection::connect() {
            Ok(selection) => Some(Arc::new(selection)),
            Err(code) => {
                eprintln!("narrivyd: clipboard unavailable ({code})");
                None
            }
        };
    }
    slot.clone()
}

/// Put `text` on the clipboard and keep serving it until something else is
/// copied. Used to copy a History entry by hand.
#[cfg(feature = "asr-transcribe-cpp")]
fn copy_to_clipboard(text: &str) -> Result<(), &'static str> {
    shared_wayland_selection()
        .ok_or("clipboard_unavailable")?
        .write_text(text)
        .map(|_| ())
}

#[cfg(not(feature = "asr-transcribe-cpp"))]
fn copy_to_clipboard(_text: &str) -> Result<(), &'static str> {
    Err("clipboard_unavailable")
}

/// Compose the production target-aware routed delivery adapter for the strict
/// routed lane. The durable Inbox destination is established by the adapter
/// before any direct rung can receive payload, and the immutable admission-time
/// [`TargetBinding`] is consumed exactly as it was bound on the Start path.
#[cfg(feature = "asr-transcribe-cpp")]
fn compose_strict_routed_delivery(
    inbox_dir: PathBuf,
    binding: TargetBinding,
    portal_service: Option<Arc<PortalPermissionService>>,
    clipboard_authorized: bool,
    uinput_paste: bool,
) -> Result<RoutedThenInboxDelivery, narrivy_platform_api::PlatformError> {
    let permission: Box<dyn DeliveryPermissionPort> = Box::new(SessionDeliveryPermission {
        portal: portal_service.clone(),
        clipboard_authorized,
    });
    compose_strict_routed_delivery_with(
        inbox_dir,
        binding,
        portal_service.map(super::portal::PortalPermissionSubmitter::new),
        permission,
        clipboard_authorized,
        uinput_paste,
    )
}

/// The composition itself with the permission transport injected. Production
/// passes the daemon-owned portal submitter; controlled tests pass a scripted
/// submitter through this same path so the composed ladder can never diverge
/// between production and test.
#[cfg(feature = "asr-transcribe-cpp")]
fn compose_strict_routed_delivery_with<S>(
    inbox_dir: PathBuf,
    binding: TargetBinding,
    portal_submitter: Option<S>,
    permission: Box<dyn DeliveryPermissionPort>,
    clipboard_authorized: bool,
    uinput_paste: bool,
) -> Result<RoutedThenInboxDelivery, narrivy_platform_api::PlatformError>
where
    S: PortalEisTextSubmitter + 'static,
{
    let mut rungs: Vec<Box<dyn DirectRung>> = Vec::new();
    // Priority order: portal/EIS text frame (requires an attended grant AND a
    // target bound at admission), then the virtual-keyboard helper, then the
    // clipboard rungs. The clipboard rungs are reported unavailable and are
    // never composed from the legacy `StdCommandRunner`/PATH-resolved
    // `wl-copy`/`wl-paste`/`xclip`/paste-chord paths: no trusted-helper
    // implementation is accepted for them yet, so production declares them
    // unavailable and the adapter can never dispatch them. The adapter always
    // terminates with exactly one durable Inbox recovery write unless a rung
    // verified its own insertion.
    if let Some(submitter) = portal_submitter {
        rungs.push(Box::new(PortalEisTextRung::new(submitter)));
    }
    // Clipboard paste (D29): the clipboard through the Wayland data-control
    // protocol (no helper from PATH) and Shift+Insert through the trusted
    // wtype. It runs before typing, needs the explicit clipboard
    // authorization (it reads the user's clipboard to put it back), and
    // pastes into the focused window like the typing rung does. With the
    // opt-in `paste-keyboard` method the chord goes through Narrivy's uinput
    // keyboard instead, for compositors that drop wtype's modifiers.
    if clipboard_authorized {
        rungs.push(live_clipboard_paste_rung(uinput_paste));
    } else {
        rungs.push(Box::new(UnavailableRung::clipboard_paste(
            "clipboard_not_authorized",
        )));
    }
    rungs.push(live_keyboard_rung());
    // Copy-only stays unavailable: no trusted copy helper is accepted.
    let clipboard_reason = if clipboard_authorized {
        "trusted_clipboard_helper_not_accepted"
    } else {
        "clipboard_not_authorized"
    };
    rungs.push(Box::new(UnavailableRung::clipboard_copy(clipboard_reason)));
    RoutedThenInboxDelivery::new(inbox_dir, binding, permission, rungs)
}

/// The clipboard-paste rung on the user's real clipboard and keyboard.
#[cfg(all(feature = "asr-transcribe-cpp", not(test)))]
fn live_clipboard_paste_rung(uinput_paste: bool) -> Box<dyn DirectRung> {
    match shared_wayland_selection() {
        Some(selection) if uinput_paste => Box::new(ClipboardPasteRung::for_focused_window(
            WaylandClipboardHandoff::new(selection),
            UinputPasteChord::shift_insert(),
        )),
        Some(selection) => Box::new(ClipboardPasteRung::for_focused_window(
            WaylandClipboardHandoff::new(selection),
            WtypePasteChord::shift_insert(),
        )),
        None => Box::new(UnavailableRung::clipboard_paste(
            "wayland_data_control_unavailable",
        )),
    }
}

#[cfg(all(feature = "asr-transcribe-cpp", not(test)))]
fn live_keyboard_rung() -> Box<dyn DirectRung> {
    Box::new(SystemWtypeKeyboardRung::new())
}

/// Tests compose the same ladder, but never on the live desktop: run from a
/// desktop terminal, the real rungs would write the user's clipboard and type
/// into the focused window (F-H16). The stand-ins keep each rung's id and
/// position, and report themselves unavailable.
#[cfg(all(feature = "asr-transcribe-cpp", test))]
fn live_clipboard_paste_rung(_uinput_paste: bool) -> Box<dyn DirectRung> {
    Box::new(UnavailableRung::clipboard_paste(
        "live_input_disabled_in_tests",
    ))
}

#[cfg(all(feature = "asr-transcribe-cpp", test))]
fn live_keyboard_rung() -> Box<dyn DirectRung> {
    Box::new(UnavailableRung::new(
        "wtype-virtual-keyboard",
        "live_input_disabled_in_tests",
    ))
}

/// Bind the delivery target exactly once on the installed Start path, before
/// any capture/activation/focus side effect, and carry the immutable value into
/// the owned worker.
///
/// The current IPC `StartSession` command carries no trustworthy pre-activation
/// target identity — its `activation` string is a self-declared reason, never a
/// window identity — and no accepted resolver supplies one, so production
/// returns the explicit typed-unavailable binding
/// ([`narrivy_platform_linux::delivery::PRE_ACTIVATION_BINDING_ABSENT`]). Every
/// target-requiring rung is then skipped and the exact text is recovered
/// durably in the Inbox. Wiring a real pre-activation target provider into
/// Start is the remaining live-trigger integration blocker.
fn admission_target_binding() -> TargetBinding {
    UnavailablePreActivationTargetBinding.bind()
}

/// Content-derived target application for one retained History row, or `None`
/// when admission bound no trustworthy pre-activation target.
///
/// The value is a privacy input, not a diagnostic one: it is never logged or
/// rendered, and a `None` binding yields an absent application that simply
/// matches no privacy zone. It is never guessed from the activation string or
/// from a desktop name.
fn target_application_id(binding: &TargetBinding) -> Option<String> {
    match binding {
        TargetBinding::Bound { application, .. } if !application.is_empty() => {
            Some(application.clone())
        }
        _ => None,
    }
}

/// Mirror one finished owned worker into the optional retained History.
///
/// The recovery journal remains the only holder of in-flight text: this reads
/// the FINALIZED record and hands it to the retention/privacy-zone/policy
/// gate. It is deliberately after the session result, so a History failure
/// can never change the dictation outcome or the recovery guarantee. Only
/// content-free facts are reported.
/// How long transcription took: from the end of capture to the raw text.
/// `None` when either point was not observed (a cancelled or failed
/// transaction), so nothing is ever guessed.
#[cfg(feature = "asr-transcribe-cpp")]
fn transcribe_duration_us(timing: &RecoveryTimingTrace) -> Option<u64> {
    use narrivy_core::RecoveryTimingStage::{CaptureEnded, RawFinalAccepted};
    timing
        .offset_us(RawFinalAccepted)?
        .checked_sub(timing.offset_us(CaptureEnded)?)
}

/// Enforce the Inbox's bounded retention (F-M3): files older than
/// [`INBOX_RETENTION`](narrivy_platform_linux::delivery::INBOX_RETENTION) go,
/// then the oldest beyond
/// [`INBOX_MAX_FILES`](narrivy_platform_linux::delivery::INBOX_MAX_FILES).
/// History is the long-term copy of a dictation; the Inbox only keeps text
/// whose delivery failed or is uncertain.
pub(crate) fn prune_inbox(dir: PathBuf) {
    use narrivy_platform_linux::delivery::{INBOX_MAX_FILES, INBOX_RETENTION, InboxStore};
    let removed = InboxStore::new(dir).prune(
        INBOX_RETENTION,
        INBOX_MAX_FILES,
        std::time::SystemTime::now(),
    );
    if removed > 0 {
        eprintln!("narrivyd: removed {removed} expired recovered-text file(s)");
    }
}

fn mirror_result_into_history(
    history: &Option<Arc<RetainedHistory>>,
    journal: &Journal,
    txid: &TransactionId,
    app_id: Option<&str>,
    transcribe_us: Option<u64>,
) {
    let Some(service) = history else {
        return;
    };
    let result = match journal.read(txid.as_str()) {
        Ok(Some(result)) => result,
        // An absent final record simply has nothing to retain. A read
        // failure is content-free too: only the fact is reported.
        Ok(None) => return,
        Err(_) => {
            eprintln!("narrivyd: retained history skipped a result the journal could not return");
            return;
        }
    };
    match service.record_committed_timed(&result, app_id.unwrap_or_default(), transcribe_us) {
        Ok(RecordOutcome::Recorded | RecordOutcome::NotRetained) => {}
        // A global disable or a privacy zone is a deliberate privacy
        // decision, not a failure: it is reported by the capability set.
        Ok(
            RecordOutcome::DisabledByPolicy
            | RecordOutcome::DeniedByPolicy
            | RecordOutcome::DeniedByZone,
        ) => {}
        Err(_) => eprintln!("narrivyd: retained history write failed"),
    }
}

#[cfg(feature = "asr-transcribe-cpp")]
enum StrictRecoveryAsr {
    Batch(Box<dyn OneShotAsrPort>),
    #[cfg(feature = "asr-transcribe-resident")]
    Streaming(ResidentWorkerEngine),
}

/// One active session slot: machine + identity + cancellation.
pub struct SessionSlot {
    pub machine: SessionMachine,
    pub transaction_id: TransactionId,
    pub pipeline_id: Option<String>,
    /// P7: correction strategy bound to the selected pipeline at begin time.
    pub correction: CorrectionStrategy,
    pub control: SessionControl,
    pub timing: Arc<RecoveryTimingTrace>,
}

/// Daemon core state: session slots plus the bounded routing event bus.
pub struct DaemonState {
    pub bus: EventBus<DaemonEvent>,
    pub sessions: Mutex<HashMap<u128, SessionSlot>>,
    /// Daemon-owned module registry. Capability use is admitted through
    /// `invoke_module`, which checks declaration and host grant before the
    /// operation closure can observe any payload.
    module_registry: Mutex<ModuleRegistry>,
    next_session: AtomicU64,
    /// Serializes secure ID creation and no-clobber journal admission. This
    /// makes the session visible only after its durable identity exists.
    transaction_admission: Mutex<()>,
    /// Serializes one atomic activation decision with the start path itself.
    /// Two concurrent toggles therefore cannot both observe "no session":
    /// the first admits (and registers) the session before the second looks,
    /// so the second requests its graceful stop instead of opening another
    /// session. Every start path takes this same gate, so a toggle can never
    /// race an explicit Start either.
    toggle_admission: Mutex<()>,
    /// One fail-closed admission boundary for strict capture pipelines. The
    /// permit is acquired before any journal or capture side effect and is
    /// held by the supervised session worker until every terminal path drops.
    strict_session_admitted: AtomicBool,
    /// Content-free activation composition facts reported through status.
    activation_facts: Mutex<ActivationFacts>,
    /// Content-free monotonic offsets retained after the session slot is
    /// removed so performance qualification cannot race worker cleanup.
    last_session_timings: Mutex<Option<StageTimingsUs>>,
    #[cfg(feature = "asr-transcribe-resident")]
    resident_lifecycle: Mutex<ResidentProvisionCoordinator>,
    #[cfg(feature = "asr-transcribe-resident")]
    model_provisioning: OnceLock<ModelProvisioningAuthority>,
    /// Desktop shortcut management; absent until composed at startup.
    shortcuts: OnceLock<Arc<super::shortcuts::ShortcutService>>,
    /// Which installed model is in use and whether another is loading.
    model_selection: super::model_selection::ModelSelection,
    /// The one model download that may run, and how the last one ended.
    model_downloads: super::download::ModelDownloads,
    #[cfg(all(test, feature = "asr-transcribe-resident"))]
    provision_test_hooks: ProvisionTestHooks,
}

#[cfg(feature = "asr-transcribe-resident")]
struct ResidentCandidate {
    engine: ResidentEngineOwner,
    installed_identity: Option<ResidentInstalledIdentity>,
}

#[cfg(feature = "asr-transcribe-resident")]
#[derive(Clone)]
enum ResidentRuntimeAuthority {
    Installed(InstalledRuntimeBinding),
    #[cfg(test)]
    Controlled,
}

#[cfg(feature = "asr-transcribe-resident")]
#[derive(Clone)]
enum ResidentEngineOwner {
    Installed(ResidentWorkerEngine),
    #[cfg(test)]
    Controlled(ControlledResidentEngine),
}

#[cfg(feature = "asr-transcribe-resident")]
type ResidentProvisionCoordinator =
    ProvisionCoordinator<ResidentRuntimeAuthority, ResidentCandidate>;

#[cfg(feature = "asr-transcribe-resident")]
enum ImportedProvision {
    Installed {
        runtime: InstalledRuntimeBinding,
        observed: narrivy_model_manager::BundledInstalledModelBinding,
    },
    #[cfg(test)]
    Controlled(ControlledActivation),
}

#[cfg(feature = "asr-transcribe-resident")]
#[derive(Clone)]
struct ModelProvisioningAuthority {
    supervisor: Arc<WorkerSupervisor>,
    backend: ModelProvisioningBackend,
}

#[cfg(feature = "asr-transcribe-resident")]
#[derive(Clone)]
enum ModelProvisioningBackend {
    Installed {
        store: Arc<narrivy_model_manager::ModelStore>,
        registry: Arc<narrivy_model_manager::BundledModelRegistry>,
        model_id: String,
    },
    #[cfg(test)]
    Controlled(ControlledProvisionBackend),
}

#[cfg(all(test, feature = "asr-transcribe-resident"))]
type ControlledProvisionBackend = Arc<
    dyn Fn(
            ResidentRuntimeAuthority,
            &Path,
            &CancelFlag,
            Instant,
        ) -> Result<ImportedProvision, ProvisionTerminal<ResidentRuntimeAuthority>>
        + Send
        + Sync,
>;

#[cfg(all(test, feature = "asr-transcribe-resident"))]
struct ControlledActivation {
    resident_pid_path: PathBuf,
}

#[cfg(all(test, feature = "asr-transcribe-resident"))]
#[derive(Clone)]
struct ControlledResidentEngine {
    inner: Arc<ControlledResidentInner>,
}

#[cfg(all(test, feature = "asr-transcribe-resident"))]
struct ControlledResidentInner {
    child: Mutex<Option<std::process::Child>>,
    pid: u32,
}

#[cfg(feature = "asr-transcribe-resident")]
impl ResidentEngineOwner {
    fn installed_clone(&self) -> Option<ResidentWorkerEngine> {
        match self {
            Self::Installed(engine) => Some(engine.clone()),
            #[cfg(test)]
            Self::Controlled(_) => None,
        }
    }

    fn shutdown(&self, timeout: Duration) -> Result<(), narrivy_asr_api::AsrError> {
        match self {
            Self::Installed(engine) => engine.shutdown(timeout),
            #[cfg(test)]
            Self::Controlled(engine) => engine.shutdown(),
        }
    }

    fn into_installed(self) -> Option<ResidentWorkerEngine> {
        match self {
            Self::Installed(engine) => Some(engine),
            #[cfg(test)]
            Self::Controlled(engine) => {
                let _ = engine.shutdown();
                None
            }
        }
    }
}

#[cfg(all(test, feature = "asr-transcribe-resident"))]
impl ControlledResidentEngine {
    fn spawn(pid_path: &Path) -> Result<Self, narrivy_asr_api::AsrError> {
        let child = std::process::Command::new("/bin/sleep")
            .arg("60")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|_| {
                narrivy_asr_api::AsrError::new(
                    "controlled_resident_spawn_failed",
                    "controlled resident ownership fixture did not start",
                )
            })?;
        let pid = child.id();
        std::fs::write(pid_path, pid.to_string()).map_err(|_| {
            narrivy_asr_api::AsrError::new(
                "controlled_resident_pid_failed",
                "controlled resident ownership fixture could not record its PID",
            )
        })?;
        Ok(Self {
            inner: Arc::new(ControlledResidentInner {
                child: Mutex::new(Some(child)),
                pid,
            }),
        })
    }

    fn shutdown(&self) -> Result<(), narrivy_asr_api::AsrError> {
        let child = self
            .inner
            .child
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(mut child) = child {
            let _ = child.kill();
            child.wait().map_err(|_| {
                narrivy_asr_api::AsrError::new(
                    "controlled_resident_reap_failed",
                    "controlled resident ownership fixture was not reaped",
                )
            })?;
        }
        Ok(())
    }

    fn pid(&self) -> u32 {
        self.inner.pid
    }
}

#[cfg(all(test, feature = "asr-transcribe-resident"))]
impl Drop for ControlledResidentInner {
    fn drop(&mut self) {
        let child = self
            .child
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(mut child) = child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(all(test, feature = "asr-transcribe-resident"))]
#[derive(Clone, Copy)]
enum ProvisionTestCheckpoint {
    BeforeImport,
    BeforeCandidateCommit,
}

#[cfg(all(test, feature = "asr-transcribe-resident"))]
struct ProvisionTestPause {
    reached: SyncSender<()>,
    resume: Receiver<()>,
}

#[cfg(all(test, feature = "asr-transcribe-resident"))]
#[derive(Default)]
struct ProvisionTestHooks {
    before_import: Mutex<Option<ProvisionTestPause>>,
    before_candidate_commit: Mutex<Option<ProvisionTestPause>>,
    panic_before_import: AtomicBool,
}

#[cfg(all(test, feature = "asr-transcribe-resident"))]
struct ProvisionTestPauseControl {
    reached: Receiver<()>,
    resume: SyncSender<()>,
}

#[cfg(all(test, feature = "asr-transcribe-resident"))]
impl ProvisionTestPauseControl {
    fn wait_until_reached(&self) -> Result<(), std::sync::mpsc::RecvTimeoutError> {
        self.reached.recv_timeout(Duration::from_secs(2))
    }

    fn release(&self) -> Result<(), std::sync::mpsc::SendError<()>> {
        self.resume.send(())
    }
}

#[cfg(feature = "asr-transcribe-resident")]
struct ProvisionWorkerCleanup {
    state: Arc<DaemonState>,
    key: ProvisionOperationKey,
    publication_verified: bool,
    armed: bool,
}

#[cfg(feature = "asr-transcribe-resident")]
impl ProvisionWorkerCleanup {
    fn new(state: Arc<DaemonState>, key: ProvisionOperationKey) -> Self {
        Self {
            state,
            key,
            publication_verified: false,
            armed: true,
        }
    }

    fn mark_publication_verified(&mut self) {
        self.publication_verified = true;
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

#[cfg(feature = "asr-transcribe-resident")]
impl Drop for ProvisionWorkerCleanup {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let mut lifecycle = self
            .state
            .resident_lifecycle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.publication_verified {
            let _ = lifecycle.mark_activating(&self.key);
        }
        let _ = lifecycle.complete_terminal(&self.key, ProvisionTerminal::WorkerFailedUncertain);
    }
}

#[cfg(feature = "asr-transcribe-resident")]
fn generate_model_provision_operation_id() -> Result<String, ()> {
    let mut bytes = [0_u8; 16];
    SysRng.try_fill_bytes(&mut bytes).map_err(|_| ())?;
    let mut id = String::from("mp-");
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut id, "{byte:02x}").map_err(|_| ())?;
    }
    Ok(id)
}

#[cfg(feature = "asr-transcribe-resident")]
fn model_provision_start_rejection(error: ProvisionStartError) -> (IpcErrorCode, String) {
    match error {
        ProvisionStartError::Busy | ProvisionStartError::Cooldown { .. } => (
            IpcErrorCode::Busy,
            "offline model provisioning is not currently admissible".to_owned(),
        ),
        ProvisionStartError::Unconfigured
        | ProvisionStartError::AlreadyReady
        | ProvisionStartError::FailClosed
        | ProvisionStartError::Closing => (
            IpcErrorCode::DaemonUnavailable,
            "offline model provisioning is unavailable".to_owned(),
        ),
        ProvisionStartError::GenerationExhausted => (
            IpcErrorCode::Internal,
            "offline model provisioning identity is exhausted".to_owned(),
        ),
    }
}

#[cfg(feature = "asr-transcribe-resident")]
fn model_provision_transition_rejection(error: ProvisionTransitionError) -> (IpcErrorCode, String) {
    match error {
        ProvisionTransitionError::Closing => (
            IpcErrorCode::DaemonUnavailable,
            "offline model provisioning is shutting down".to_owned(),
        ),
        ProvisionTransitionError::StaleOperation
        | ProvisionTransitionError::CancellationRequested
        | ProvisionTransitionError::InvalidPhase => (
            IpcErrorCode::BadRequest,
            "offline model provisioning operation is not active".to_owned(),
        ),
        ProvisionTransitionError::RevisionExhausted => (
            IpcErrorCode::Internal,
            "offline model provisioning status revision is exhausted".to_owned(),
        ),
    }
}

#[cfg(feature = "asr-transcribe-resident")]
fn complete_model_provision(
    state: &DaemonState,
    key: &ProvisionOperationKey,
    terminal: ProvisionTerminal<ResidentRuntimeAuthority>,
) {
    let mut lifecycle = state
        .resident_lifecycle
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let TerminalCommit::Rejected { terminal, .. } = lifecycle.complete_terminal(key, terminal) {
        let cancellation_requested = lifecycle
            .status()
            .is_some_and(|status| status.state == ModelProvisionStateDto::CancelRequested);
        let fallback = if cancellation_requested {
            match terminal {
                ProvisionTerminal::CancelledBeforePublicationClean { runtime }
                | ProvisionTerminal::TimedOutBeforePublicationClean { runtime }
                | ProvisionTerminal::SourceRejectedBeforeStaging { runtime }
                | ProvisionTerminal::SourceRejectedAfterStagingClean { runtime }
                | ProvisionTerminal::ImportFailedClean { runtime }
                | ProvisionTerminal::WorkerFailedBeforePublicationClean { runtime } => {
                    ProvisionTerminal::CancelledBeforePublicationClean { runtime }
                }
                ProvisionTerminal::BindingFailed
                | ProvisionTerminal::ActivationFailed
                | ProvisionTerminal::CancelledAfterPublication
                | ProvisionTerminal::TimedOutAfterPublication => {
                    ProvisionTerminal::CancelledAfterPublication
                }
                ProvisionTerminal::ImportFailedClosed
                | ProvisionTerminal::ImportFailedClosedAfterStagingClean
                | ProvisionTerminal::CleanupFailed
                | ProvisionTerminal::PublishedUnconfirmed
                | ProvisionTerminal::WorkerFailedUncertain => {
                    ProvisionTerminal::WorkerFailedUncertain
                }
            }
        } else {
            ProvisionTerminal::WorkerFailedUncertain
        };
        let _ = lifecycle.complete_terminal(key, fallback);
    }
}

#[cfg(feature = "asr-transcribe-resident")]
fn import_failure_terminal<Runtime>(
    error: OfflineModelImportError,
    runtime: Runtime,
) -> ProvisionTerminal<Runtime> {
    match error {
        OfflineModelImportError::CancelledBeforePublication => {
            ProvisionTerminal::CancelledBeforePublicationClean { runtime }
        }
        OfflineModelImportError::TimedOutBeforePublication => {
            ProvisionTerminal::TimedOutBeforePublicationClean { runtime }
        }
        OfflineModelImportError::SourceUnavailable | OfflineModelImportError::SourceUnsafe => {
            ProvisionTerminal::SourceRejectedBeforeStaging { runtime }
        }
        OfflineModelImportError::SourceChanged | OfflineModelImportError::BytesMismatch => {
            ProvisionTerminal::SourceRejectedAfterStagingClean { runtime }
        }
        OfflineModelImportError::Collision
        | OfflineModelImportError::DurabilityFailedBeforePublication
        | OfflineModelImportError::VerificationFailedBeforePublication
        | OfflineModelImportError::Io => ProvisionTerminal::ImportFailedClean { runtime },
        OfflineModelImportError::UnknownModel | OfflineModelImportError::UnsafeCacheRoot => {
            ProvisionTerminal::ImportFailedClosed
        }
        OfflineModelImportError::ExistingInstallInvalid => {
            ProvisionTerminal::ImportFailedClosedAfterStagingClean
        }
        OfflineModelImportError::CleanupFailedBeforePublication => ProvisionTerminal::CleanupFailed,
        OfflineModelImportError::DurabilityFailedAfterPublication
        | OfflineModelImportError::VerificationFailedAfterPublication => {
            ProvisionTerminal::PublishedUnconfirmed
        }
    }
}

#[cfg(feature = "asr-transcribe-resident")]
fn import_for_model_provision(
    authority: &ModelProvisioningAuthority,
    runtime: ResidentRuntimeAuthority,
    source_path: &Path,
    cancellation: &CancelFlag,
    deadline: Instant,
) -> Result<ImportedProvision, ProvisionTerminal<ResidentRuntimeAuthority>> {
    match &authority.backend {
        ModelProvisioningBackend::Installed {
            store,
            registry,
            model_id,
        } => {
            #[cfg(not(test))]
            let ResidentRuntimeAuthority::Installed(runtime) = runtime;
            #[cfg(test)]
            let runtime = match runtime {
                ResidentRuntimeAuthority::Installed(runtime) => runtime,
                ResidentRuntimeAuthority::Controlled => {
                    return Err(ProvisionTerminal::WorkerFailedUncertain);
                }
            };
            match import_bundled_model(
                store,
                registry,
                model_id,
                source_path,
                cancellation,
                deadline,
            ) {
                Ok(observed) => Ok(ImportedProvision::Installed { runtime, observed }),
                Err(error) => Err(import_failure_terminal(
                    error,
                    ResidentRuntimeAuthority::Installed(runtime),
                )),
            }
        }
        #[cfg(test)]
        ModelProvisioningBackend::Controlled(backend) => {
            backend(runtime, source_path, cancellation, deadline)
        }
    }
}

#[cfg(feature = "asr-transcribe-resident")]
fn activate_imported_provision(
    imported: ImportedProvision,
    cancellation: &CancelFlag,
    deadline: Instant,
) -> Result<ResidentCandidate, ProvisionTerminal<ResidentRuntimeAuthority>> {
    match imported {
        ImportedProvision::Installed { runtime, observed } => {
            let model = match bind_installed_model_controlled(&observed, cancellation, deadline) {
                Ok(model) => model,
                Err(narrivy_process_linux::DataArtifactError::Cancelled) => {
                    return Err(ProvisionTerminal::CancelledAfterPublication);
                }
                Err(narrivy_process_linux::DataArtifactError::TimedOut) => {
                    return Err(ProvisionTerminal::TimedOutAfterPublication);
                }
                Err(_) => return Err(ProvisionTerminal::BindingFailed),
            };
            let identity = ResidentInstalledIdentity {
                _worker_sha256: runtime.worker().sha256(),
                worker_size_bytes: runtime.worker().size(),
                _model_id: model.id().to_owned(),
                _model_family: model.family().to_owned(),
                _model_source_revision: model.source_revision().to_owned(),
                _model_sha256: model.model().sha256(),
                model_size_bytes: model.model().size(),
            };
            let engine = match ResidentWorkerEngine::with_trust_anchored_bindings_until(
                runtime.into_worker(),
                model.into_model(),
                deadline,
                cancellation,
            ) {
                Ok(engine) => engine,
                Err(error) if error.code == "cancelled" => {
                    return Err(ProvisionTerminal::CancelledAfterPublication);
                }
                Err(error) if error.code == "resident_worker_startup_timeout" => {
                    return Err(ProvisionTerminal::TimedOutAfterPublication);
                }
                Err(_) => return Err(ProvisionTerminal::ActivationFailed),
            };
            Ok(ResidentCandidate {
                engine: ResidentEngineOwner::Installed(engine),
                installed_identity: Some(identity),
            })
        }
        #[cfg(test)]
        ImportedProvision::Controlled(activation) => {
            if cancellation.is_cancelled() {
                return Err(ProvisionTerminal::CancelledAfterPublication);
            }
            if Instant::now() >= deadline {
                return Err(ProvisionTerminal::TimedOutAfterPublication);
            }
            let engine = ControlledResidentEngine::spawn(&activation.resident_pid_path)
                .map_err(|_| ProvisionTerminal::ActivationFailed)?;
            Ok(ResidentCandidate {
                engine: ResidentEngineOwner::Controlled(engine),
                installed_identity: None,
            })
        }
    }
}

#[cfg(feature = "asr-transcribe-resident")]
fn run_offline_model_provision(
    state: Arc<DaemonState>,
    authority: ModelProvisioningAuthority,
    key: ProvisionOperationKey,
    runtime: ResidentRuntimeAuthority,
    source_path: PathBuf,
    cancellation: CancelFlag,
    deadline: Instant,
) {
    let mut cleanup = ProvisionWorkerCleanup::new(Arc::clone(&state), key.clone());

    #[cfg(test)]
    state.run_provision_test_checkpoint(ProvisionTestCheckpoint::BeforeImport);

    let imported = match import_for_model_provision(
        &authority,
        runtime,
        &source_path,
        &cancellation,
        deadline,
    ) {
        Ok(imported) => imported,
        Err(terminal) => {
            complete_model_provision(&state, &key, terminal);
            cleanup.disarm();
            return;
        }
    };
    cleanup.mark_publication_verified();

    let activation = {
        let mut lifecycle = state
            .resident_lifecycle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        lifecycle.mark_activating(&key)
    };
    if let Err(error) = activation {
        if error == ProvisionTransitionError::CancellationRequested {
            complete_model_provision(&state, &key, ProvisionTerminal::CancelledAfterPublication);
        } else {
            complete_model_provision(&state, &key, ProvisionTerminal::WorkerFailedUncertain);
        }
        cleanup.disarm();
        return;
    }

    let candidate = match activate_imported_provision(imported, &cancellation, deadline) {
        Ok(candidate) => candidate,
        Err(terminal) => {
            complete_model_provision(&state, &key, terminal);
            cleanup.disarm();
            return;
        }
    };
    #[cfg(test)]
    state.run_provision_test_checkpoint(ProvisionTestCheckpoint::BeforeCandidateCommit);
    let rejected = {
        let mut lifecycle = state
            .resident_lifecycle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match lifecycle.commit_ready(&key, candidate) {
            CandidateCommit::Committed => None,
            CandidateCommit::Rejected { candidate, .. } => Some(candidate),
        }
    };
    if let Some(candidate) = rejected {
        complete_model_provision(
            &state,
            &key,
            if cancellation.is_cancelled() {
                ProvisionTerminal::CancelledAfterPublication
            } else {
                ProvisionTerminal::WorkerFailedUncertain
            },
        );
        let _ = candidate
            .engine
            .shutdown(deadline.saturating_duration_since(Instant::now()));
    }
    cleanup.disarm();
}

#[cfg(feature = "asr-transcribe-resident")]
struct ResidentInstalledIdentity {
    _worker_sha256: [u8; 32],
    worker_size_bytes: u64,
    _model_id: String,
    _model_family: String,
    _model_source_revision: String,
    _model_sha256: [u8; 32],
    model_size_bytes: u64,
}

#[cfg(feature = "asr-transcribe-resident")]
impl std::fmt::Debug for ResidentInstalledIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ResidentInstalledIdentity")
            .field("worker_size_bytes", &self.worker_size_bytes)
            .field("model_size_bytes", &self.model_size_bytes)
            .field("binding", &"worker-manifest-plus-registry-model")
            .finish_non_exhaustive()
    }
}

impl Default for DaemonState {
    fn default() -> Self {
        Self {
            bus: EventBus::default(),
            sessions: Mutex::new(HashMap::new()),
            module_registry: Mutex::new(ModuleRegistry::new()),
            next_session: AtomicU64::new(1),
            transaction_admission: Mutex::new(()),
            toggle_admission: Mutex::new(()),
            strict_session_admitted: AtomicBool::new(false),
            activation_facts: Mutex::new(ActivationFacts::default()),
            last_session_timings: Mutex::new(None),
            #[cfg(feature = "asr-transcribe-resident")]
            resident_lifecycle: Mutex::new(ProvisionCoordinator::unconfigured()),
            #[cfg(feature = "asr-transcribe-resident")]
            model_provisioning: OnceLock::new(),
            shortcuts: OnceLock::new(),
            model_selection: super::model_selection::ModelSelection::default(),
            model_downloads: super::download::ModelDownloads::default(),
            #[cfg(all(test, feature = "asr-transcribe-resident"))]
            provision_test_hooks: ProvisionTestHooks::default(),
        }
    }
}

struct StrictSessionPermit {
    state: Arc<DaemonState>,
}

impl Drop for StrictSessionPermit {
    fn drop(&mut self) {
        self.state
            .strict_session_admitted
            .store(false, Ordering::Release);
    }
}

/// Records exactly one content-free terminal observation for one owned session
/// worker, whichever way that worker exits.
///
/// Every ordinary exit records its typed outcome through [`Self::observe`]:
/// the strict lane's recovery terminal, the mock lane's delivery outcome, a
/// failure, or a cancellation observed before the run. Any remaining exit — a
/// rejected capture source, a missing model, a panic — falls through to
/// [`Drop`], which records `abandoned` (never healthy, never a latency
/// sample) so a failed attempt cannot be silently absent from the reported
/// health. An uncomposed diagnostics surface observes nothing.
struct SessionDiagnosticsGuard {
    diagnostics: Option<Arc<DaemonDiagnostics>>,
    started: Instant,
    observed: bool,
}

impl SessionDiagnosticsGuard {
    fn new(diagnostics: Option<Arc<DaemonDiagnostics>>) -> Self {
        Self {
            diagnostics,
            started: Instant::now(),
            observed: false,
        }
    }

    /// Record the worker's typed terminal outcome and its end-to-end duration.
    fn observe(&mut self, outcome: SessionOutcome) {
        if let Some(diagnostics) = &self.diagnostics {
            diagnostics.observe_session(outcome, self.started.elapsed());
        }
        self.observed = true;
    }
}

impl Drop for SessionDiagnosticsGuard {
    fn drop(&mut self) {
        if self.observed {
            return;
        }
        if let Some(diagnostics) = &self.diagnostics {
            diagnostics.observe_session(SessionOutcome::Abandoned, self.started.elapsed());
        }
    }
}

fn unix_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// One deterministic atomic-toggle decision, taken while the caller holds the
/// toggle admission gate.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ToggleDecision {
    /// No session exists: run the exact strict readiness/admission/start path
    /// `StartSession` runs, still under the gate.
    Start,
    /// The live capture session was asked to finalize gracefully (once).
    FinalizationRequested {
        session_id: u128,
        transaction_id: String,
        pipeline_id: Option<String>,
    },
    /// A session already exists and is stopping/terminal: open no second one.
    AlreadyStopping {
        session_id: u128,
        transaction_id: String,
        pipeline_id: Option<String>,
    },
}

#[cfg(feature = "asr-transcribe-resident")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ResidentRuntimeSnapshot {
    lifecycle: ResidentReadiness,
    supports_streaming: bool,
    artifact_binding: ResidentArtifactBinding,
    model_binding: ResidentArtifactBinding,
    installed_identity_present: bool,
    installed_model_lifecycle: Option<ResidentInstalledLifecycle>,
}

#[cfg(feature = "asr-transcribe-resident")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResidentInstalledLifecycle {
    Awaiting,
    Provisioning,
    PublishedUnconfirmed,
    ActivationFailed,
    Ready,
}

#[cfg(feature = "asr-transcribe-resident")]
impl ResidentRuntimeSnapshot {
    fn capability_snapshot(self) -> ResidentCapabilitySnapshot {
        ResidentCapabilitySnapshot {
            lifecycle: match self.lifecycle {
                ResidentReadiness::Ready => ResidentLifecycleCapability::Ready,
                ResidentReadiness::Active => ResidentLifecycleCapability::Active,
                ResidentReadiness::Warming => ResidentLifecycleCapability::Warming,
                ResidentReadiness::RestartRequired => ResidentLifecycleCapability::RestartRequired,
                ResidentReadiness::FailedClosed => ResidentLifecycleCapability::FailedClosed,
            },
            artifact_binding: self.artifact_binding,
            model_binding: self.model_binding,
        }
    }

    fn bindings_ready(self) -> bool {
        matches!(
            (self.artifact_binding, self.model_binding),
            (
                ResidentArtifactBinding::ControlledDigestBound,
                ResidentArtifactBinding::ControlledDigestBound
            ) | (
                ResidentArtifactBinding::InstalledRuntimeManifestBound,
                ResidentArtifactBinding::RegistryObservedDigestBound
            )
        ) && (self.artifact_binding != ResidentArtifactBinding::InstalledRuntimeManifestBound
            || self.installed_identity_present)
    }

    fn installed_model_lifecycle(self) -> Option<Box<InstalledModelLifecycleDto>> {
        self.installed_model_lifecycle.map(|lifecycle| {
            Box::new(match lifecycle {
                ResidentInstalledLifecycle::Awaiting => InstalledModelLifecycleDto::AwaitingModel,
                ResidentInstalledLifecycle::Provisioning => {
                    InstalledModelLifecycleDto::Provisioning
                }
                ResidentInstalledLifecycle::PublishedUnconfirmed => {
                    InstalledModelLifecycleDto::PublishedUnconfirmed
                }
                ResidentInstalledLifecycle::ActivationFailed => {
                    InstalledModelLifecycleDto::ActivationFailed
                }
                ResidentInstalledLifecycle::Ready => InstalledModelLifecycleDto::Ready,
            })
        })
    }
}

#[cfg(feature = "asr-transcribe-resident")]
fn installed_unavailable_snapshot(
    model_binding: ResidentArtifactBinding,
    installed_model_lifecycle: ResidentInstalledLifecycle,
) -> ResidentRuntimeSnapshot {
    ResidentRuntimeSnapshot {
        lifecycle: ResidentReadiness::FailedClosed,
        supports_streaming: false,
        artifact_binding: ResidentArtifactBinding::InstalledRuntimeManifestBound,
        model_binding,
        installed_identity_present: false,
        installed_model_lifecycle: Some(installed_model_lifecycle),
    }
}

#[cfg(feature = "asr-transcribe-resident")]
fn resident_binding_attribution(
    worker: ResidentWorkerArtifactStatus,
    model: ResidentModelArtifactStatus,
    installed_identity_present: bool,
) -> (ResidentArtifactBinding, ResidentArtifactBinding) {
    let worker = match worker {
        ResidentWorkerArtifactStatus::ControlledDigestBound => {
            ResidentArtifactBinding::ControlledDigestBound
        }
        ResidentWorkerArtifactStatus::TrustAnchorDigestBound if installed_identity_present => {
            ResidentArtifactBinding::InstalledRuntimeManifestBound
        }
        ResidentWorkerArtifactStatus::PackageBindingUnavailable => {
            ResidentArtifactBinding::PackageBindingUnavailable
        }
        _ => ResidentArtifactBinding::StoredBindingUnavailable,
    };
    let model = match model {
        ResidentModelArtifactStatus::ControlledDigestBound => {
            ResidentArtifactBinding::ControlledDigestBound
        }
        ResidentModelArtifactStatus::TrustAnchorDigestBound if installed_identity_present => {
            ResidentArtifactBinding::RegistryObservedDigestBound
        }
        ResidentModelArtifactStatus::PackageBindingUnavailable => {
            ResidentArtifactBinding::PackageBindingUnavailable
        }
        ResidentModelArtifactStatus::StoredBindingUnavailable
        | ResidentModelArtifactStatus::TrustAnchorDigestBound => {
            ResidentArtifactBinding::StoredBindingUnavailable
        }
    };
    (worker, model)
}

#[cfg(feature = "asr-transcribe-resident")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResidentAdmissionDecision {
    Ready,
    Recover,
}

#[cfg(feature = "asr-transcribe-resident")]
fn resident_admission_decision(
    snapshot: Option<ResidentRuntimeSnapshot>,
) -> Result<ResidentAdmissionDecision, (IpcErrorCode, String)> {
    let Some(snapshot) = snapshot else {
        return Err((
            IpcErrorCode::DaemonUnavailable,
            "resident ASR is not initialized".to_owned(),
        ));
    };
    if !snapshot.bindings_ready() {
        let detail = match snapshot.model_binding {
            ResidentArtifactBinding::InstalledModelMissing => "selected installed model is missing",
            _ => "resident ASR artifact binding is unavailable",
        };
        return Err((IpcErrorCode::DaemonUnavailable, detail.to_owned()));
    }
    match snapshot.lifecycle {
        ResidentReadiness::Ready => Ok(ResidentAdmissionDecision::Ready),
        ResidentReadiness::Active => Err((IpcErrorCode::Busy, "resident ASR is active".to_owned())),
        ResidentReadiness::RestartRequired => Ok(ResidentAdmissionDecision::Recover),
        ResidentReadiness::Warming => Err((
            IpcErrorCode::DaemonUnavailable,
            "resident ASR is warming".to_owned(),
        )),
        ResidentReadiness::FailedClosed => Err((
            IpcErrorCode::DaemonUnavailable,
            "resident ASR failed closed".to_owned(),
        )),
    }
}

/// Load `model` for `compute` on a supervised worker that holds the session
/// permit, so no dictation can begin against a model that is being replaced
/// and shutdown can stop the load. With `wait`, returns the load's outcome;
/// without, returns as soon as the load has started.
#[cfg(feature = "asr-transcribe-resident")]
fn load_engine(
    state: &Arc<DaemonState>,
    supervisor: &Arc<WorkerSupervisor>,
    model: super::model_selection::SelectedModel,
    compute: super::model_selection::ComputeChoice,
    permit: StrictSessionPermit,
    wait: bool,
) -> Result<(), &'static str> {
    if !state.model_selection.begin(&model.id) {
        return Err("another_model_is_loading");
    }
    let (done, finished) = std::sync::mpsc::sync_channel::<Result<(), &'static str>>(1);
    let job_state = Arc::clone(state);
    let spawned =
        supervisor.spawn_with_cancellation("model-selection", CancelFlag::new(), move |cancel| {
            let _permit = permit;
            let outcome = job_state.replace_resident_model(&model, compute, &cancel);
            job_state
                .model_selection
                .finish(outcome.map(|()| (model, compute)));
            // Nobody may be waiting; the outcome is also in the selection.
            let _ = done.try_send(outcome);
        });
    if spawned.is_err() {
        state.model_selection.finish(Err("worker_unavailable"));
        return Err("worker_unavailable");
    }
    if !wait {
        return Ok(());
    }
    finished
        .recv_timeout(OPENVINO_LOAD_TIMEOUT + REPLACED_MODEL_SHUTDOWN_TIMEOUT)
        .unwrap_or(Err("model_load_timeout"))
}

/// Start loading the model `id` in place of the one in use, on the device
/// the loaded engine was asked for. Returns once the load has started.
#[cfg(feature = "asr-transcribe-resident")]
fn select_model(
    state: &Arc<DaemonState>,
    supervisor: &Arc<WorkerSupervisor>,
    config: &EngineConfig,
    id: &str,
) -> Result<(), (IpcErrorCode, &'static str)> {
    use super::model_selection::{Resolution, resolve};

    if id.is_empty() || id.len() > narrivy_ipc::MAX_MODEL_ID_BYTES {
        return Err((IpcErrorCode::BadRequest, "invalid model id"));
    }
    let current = state.model_selection.current();
    let target = match resolve(config, current.as_ref(), id) {
        Ok(Resolution::AlreadyActive) => return Ok(()),
        Ok(Resolution::Load(target)) => target,
        Err(error) => return Err((IpcErrorCode::BadRequest, error.detail())),
    };
    let Some(permit) = state.try_admit_strict() else {
        return Err((IpcErrorCode::Busy, "a dictation is in progress"));
    };
    let compute = state.model_selection.current_compute();
    load_engine(state, supervisor, target, compute, permit, false).map_err(|code| match code {
        "another_model_is_loading" => (IpcErrorCode::Busy, "another model is loading"),
        _ => (IpcErrorCode::Internal, "the model could not start loading"),
    })
}

#[cfg(not(feature = "asr-transcribe-resident"))]
fn select_model(
    _state: &Arc<DaemonState>,
    _supervisor: &Arc<WorkerSupervisor>,
    _config: &EngineConfig,
    _id: &str,
) -> Result<(), (IpcErrorCode, &'static str)> {
    Err((
        IpcErrorCode::BadRequest,
        super::model_selection::SelectError::NotAvailable.detail(),
    ))
}

/// Start downloading the catalog model `id` beside the loaded model. This
/// is the only path on which the daemon uses the network, and only a client
/// request reaches it. Returns once the download has started; a supervised
/// worker owns it, so shutdown stops it.
fn download_model(
    state: &Arc<DaemonState>,
    supervisor: &Arc<WorkerSupervisor>,
    config: &EngineConfig,
    id: &str,
) -> Result<(), (IpcErrorCode, &'static str)> {
    use super::model_selection::{SelectError, loaded_path, switchable};
    use narrivy_model_manager::{DownloadRefusal, DownloadTarget, model_catalog};

    let bad = |detail| (IpcErrorCode::BadRequest, detail);
    if id.is_empty() || id.len() > narrivy_ipc::MAX_MODEL_ID_BYTES {
        return Err(bad("invalid model id"));
    }
    if !switchable(config) {
        return Err(bad(SelectError::NotAvailable.detail()));
    }
    // Packs of several files: the OpenVINO runtime and its model packs.
    if let Some(models) = super::store::models_dir(config, state.model_selection()) {
        match super::store::plan_install(&models, id) {
            Ok(install) => return install_pack(state, supervisor, id, install),
            Err("unknown_item") => {}
            Err("not_published_yet") => {
                return Err(bad("this item is not published for download yet"));
            }
            Err(_) => return Err(bad("this item cannot be installed")),
        }
    }
    let model = model_catalog()
        .into_iter()
        .find(|model| model.id == id)
        .ok_or_else(|| bad(SelectError::UnknownModel.detail()))?;
    let target = DownloadTarget::for_model(&model).map_err(|refusal| {
        bad(match refusal {
            DownloadRefusal::NeedsEngine => SelectError::NeedsEngine.detail(),
            DownloadRefusal::NotSingleVerifiedFile => SelectError::NotVerifiable.detail(),
            DownloadRefusal::NotPublishedByNarrivy => {
                "this model is not published for download by Narrivy"
            }
        })
    })?;
    let current = state.model_selection.current();
    let directory = loaded_path(config, current.as_ref())
        .and_then(std::path::Path::parent)
        .map(std::path::Path::to_path_buf)
        .ok_or_else(|| bad(SelectError::NotAvailable.detail()))?;
    let (final_path, _) = super::download::destination(&directory, &target);
    if final_path.exists() {
        return Err(bad("this model is already downloaded"));
    }
    let Some((received, cancel)) = state.model_downloads.begin(id) else {
        return Err((IpcErrorCode::Busy, "another model is downloading"));
    };
    let job_state = Arc::clone(state);
    let spawned = supervisor.spawn_with_cancellation("model-download", cancel, move |cancel| {
        let outcome = super::download::run(&target, &directory, &received, &cancel);
        job_state.model_downloads.finish(outcome);
    });
    if spawned.is_err() {
        state.model_downloads.finish(Err("worker_unavailable"));
        return Err((IpcErrorCode::Internal, "the download could not start"));
    }
    Ok(())
}

/// Install a pack of several files under its own directory, verified file
/// by file, on a supervised worker so shutdown stops it.
fn install_pack(
    state: &Arc<DaemonState>,
    supervisor: &Arc<WorkerSupervisor>,
    id: &str,
    install: super::store::Install,
) -> Result<(), (IpcErrorCode, &'static str)> {
    let (final_dir, _) = super::download::pack_destination(&install.parent, &install.pack);
    if std::fs::symlink_metadata(&final_dir).is_ok() {
        return Err((IpcErrorCode::BadRequest, "this item is already installed"));
    }
    let Some((received, cancel)) = state.model_downloads.begin(id) else {
        return Err((IpcErrorCode::Busy, "another download is running"));
    };
    let job_state = Arc::clone(state);
    let spawned = supervisor.spawn_with_cancellation("store-install", cancel, move |cancel| {
        let outcome = super::download::run_pack(
            &install.pack,
            &install.parent,
            install.local.as_deref(),
            &received,
            &cancel,
        );
        job_state.model_downloads.finish(outcome);
    });
    if spawned.is_err() {
        state.model_downloads.finish(Err("worker_unavailable"));
        return Err((IpcErrorCode::Internal, "the install could not start"));
    }
    Ok(())
}

/// Where the loaded engine runs, as its worker attests.
#[cfg(feature = "asr-transcribe-resident")]
fn resident_device(state: &DaemonState) -> super::model_selection::LoadedDevice {
    state
        .resident_asr()
        .map_or(super::model_selection::LoadedDevice::Cpu, |engine| {
            engine.compute().into()
        })
}

/// Store what `pipeline_id` recognises with and, when nothing is dictating,
/// load it at once so the pipeline's next dictation starts without a wait.
/// While a dictation runs the choice is only stored; it is loaded when that
/// pipeline next starts.
#[cfg(feature = "asr-transcribe-resident")]
fn set_pipeline_engine(
    state: &Arc<DaemonState>,
    supervisor: &Arc<WorkerSupervisor>,
    config: &EngineConfig,
    pipeline_id: &str,
    model_id: Option<String>,
    compute: &str,
) -> Result<(), (IpcErrorCode, String)> {
    use super::model_selection::{ComputeChoice, PipelineEngine, Plan, plan, resolve};

    let bad = |detail: &str| (IpcErrorCode::BadRequest, detail.to_owned());
    let admitted = resolve_pipeline_selection(config, Some(pipeline_id))?;
    let compute = ComputeChoice::parse(compute)
        .ok_or_else(|| bad("the device must be auto, gpu, npu or cpu"))?;
    if let Some(id) = &model_id {
        if id.is_empty() || id.len() > narrivy_ipc::MAX_MODEL_ID_BYTES {
            return Err(bad("invalid model id"));
        }
        let current = state.model_selection.current();
        resolve(config, current.as_ref(), id).map_err(|error| bad(error.detail()))?;
    } else if compute != ComputeChoice::Auto && !super::model_selection::switchable(config) {
        return Err(bad(
            super::model_selection::SelectError::NotAvailable.detail()
        ));
    }
    let pipeline_id = admitted.id();
    state.model_selection.set_pipeline_engine(
        pipeline_id,
        PipelineEngine {
            model: model_id,
            compute,
        },
    );
    if let Plan::Load { model, compute } = plan(
        config,
        &state.model_selection,
        pipeline_id,
        resident_device(state),
    ) && let Some(permit) = state.try_admit_strict()
    {
        // A load that cannot start now is retried when the pipeline starts.
        let _ = load_engine(state, supervisor, model, compute, permit, false);
    }
    Ok(())
}

#[cfg(not(feature = "asr-transcribe-resident"))]
fn set_pipeline_engine(
    _state: &Arc<DaemonState>,
    _supervisor: &Arc<WorkerSupervisor>,
    _config: &EngineConfig,
    _pipeline_id: &str,
    _model_id: Option<String>,
    _compute: &str,
) -> Result<(), (IpcErrorCode, String)> {
    Err((
        IpcErrorCode::BadRequest,
        super::model_selection::SelectError::NotAvailable
            .detail()
            .to_owned(),
    ))
}

/// Before a dictation starts: if its pipeline wants another model or device
/// than the loaded one, load it now. Clients are told through the
/// `engine.loading` capability event, so the overlay can say a model is
/// loading before listening begins. A load that fails keeps the previous
/// model and the dictation proceeds with it: speech is never refused because
/// a preferred model could not load.
#[cfg(feature = "asr-transcribe-resident")]
fn ensure_pipeline_engine(
    state: &Arc<DaemonState>,
    supervisor: &Arc<WorkerSupervisor>,
    config: &EngineConfig,
    pipeline_id: &str,
) {
    use super::model_selection::{Plan, plan};

    let Plan::Load { model, compute } = plan(
        config,
        &state.model_selection,
        pipeline_id,
        resident_device(state),
    ) else {
        return;
    };
    // A dictation or another load holds the permit: the normal start path
    // reports that.
    let Some(permit) = state.try_admit_strict() else {
        return;
    };
    let announce = |available| {
        state.bus.publish_priority(
            DaemonEvent::Capability {
                capability: ENGINE_LOADING_CAPABILITY,
                available,
            },
            |queued| matches!(queued, DaemonEvent::Partial { .. }),
        );
    };
    announce(true);
    let _ = load_engine(state, supervisor, model, compute, permit, true);
    announce(false);
}

/// After start-up, load the default pipeline's model in the background when
/// it is not the one loaded (an OpenVINO model is never loaded during
/// start-up itself), so the first dictation does not wait for it. A
/// dictation started meanwhile is refused as busy, like during any load.
#[cfg(feature = "asr-transcribe-resident")]
pub fn warm_default_pipeline(
    state: &Arc<DaemonState>,
    supervisor: &Arc<WorkerSupervisor>,
    config: &EngineConfig,
) {
    use super::model_selection::{Plan, plan};

    let Plan::Load { model, compute } = plan(
        config,
        &state.model_selection,
        &super::policy::default_pipeline_id(),
        resident_device(state),
    ) else {
        return;
    };
    if let Some(permit) = state.try_admit_strict() {
        let _ = load_engine(state, supervisor, model, compute, permit, false);
    }
}

#[cfg(not(feature = "asr-transcribe-resident"))]
fn ensure_pipeline_engine(
    _state: &Arc<DaemonState>,
    _supervisor: &Arc<WorkerSupervisor>,
    _config: &EngineConfig,
    _pipeline_id: &str,
) {
}

/// Whether the live speech worker runs on a GPU (through Vulkan; the
/// OpenVINO worker never attests a GPU here).
#[cfg(feature = "asr-transcribe-resident")]
fn gpu_in_use(state: &Arc<DaemonState>) -> bool {
    use narrivy_asr_transcribe::resident_protocol::WorkerCompute;
    state.resident_asr().is_some_and(|engine| {
        engine.is_worker_ready()
            && matches!(
                engine.compute(),
                WorkerCompute::IntegratedGpu | WorkerCompute::DiscreteGpu
            )
    })
}

#[cfg(not(feature = "asr-transcribe-resident"))]
fn gpu_in_use(_state: &Arc<DaemonState>) -> bool {
    false
}

/// The device the live worker attested: `cpu`, `igpu` or `dgpu`. Nothing is
/// reported while no worker is ready, so a client never shows a stale device.
#[cfg(feature = "asr-transcribe-resident")]
fn resident_device_capability(
    worker_ready: bool,
    compute: narrivy_asr_transcribe::resident_protocol::WorkerCompute,
) -> Option<String> {
    worker_ready.then(|| format!("asr.transcribe-cpp.device={}", compute.as_token()))
}

#[cfg(feature = "asr-transcribe-resident")]
fn append_resident_streaming_capabilities(
    capabilities: &mut Vec<String>,
    config: &EngineConfig,
    resident: Option<ResidentRuntimeSnapshot>,
) {
    if !config.transcribe_resident {
        return;
    }
    capabilities.push(format!(
        "asr.transcribe-cpp.model-streaming={}",
        match resident {
            Some(ResidentRuntimeSnapshot {
                model_binding: ResidentArtifactBinding::InstalledModelMissing,
                ..
            }) => "unavailable:installed_model_missing",
            Some(ResidentRuntimeSnapshot {
                supports_streaming: true,
                ..
            }) => "supported",
            Some(ResidentRuntimeSnapshot {
                supports_streaming: false,
                ..
            }) => "unsupported",
            None => "unavailable:resident_not_initialized",
        }
    ));
    capabilities.push(format!(
        "asr.transcribe-cpp.worker={}",
        match resident {
            Some(ResidentRuntimeSnapshot {
                model_binding: ResidentArtifactBinding::InstalledModelMissing,
                ..
            }) => "not-started:installed-model-missing",
            Some(snapshot) if !snapshot.bindings_ready() => {
                "failed-closed:artifact-binding"
            }
            Some(ResidentRuntimeSnapshot {
                lifecycle: ResidentReadiness::Ready,
                ..
            }) => "ready",
            Some(ResidentRuntimeSnapshot {
                lifecycle: ResidentReadiness::Active,
                ..
            }) => "active",
            Some(ResidentRuntimeSnapshot {
                lifecycle: ResidentReadiness::Warming,
                ..
            }) => "warming",
            Some(ResidentRuntimeSnapshot {
                lifecycle: ResidentReadiness::RestartRequired,
                ..
            }) => "restart-required",
            Some(ResidentRuntimeSnapshot {
                lifecycle: ResidentReadiness::FailedClosed,
                ..
            }) => "failed-closed",
            None => "unavailable",
        }
    ));
    let strict_recovery = config.is_strict_recovery();
    let composition = match (strict_recovery, resident) {
        (
            true,
            Some(ResidentRuntimeSnapshot {
                model_binding: ResidentArtifactBinding::InstalledModelMissing,
                ..
            }),
        ) => "asr.transcribe-cpp.streaming=disabled:installed_model_missing",
        (true, Some(snapshot)) if !snapshot.bindings_ready() => {
            "asr.transcribe-cpp.streaming=disabled:artifact_binding"
        }
        (
            true,
            Some(ResidentRuntimeSnapshot {
                supports_streaming: true,
                lifecycle: ResidentReadiness::Ready,
                artifact_binding: ResidentArtifactBinding::InstalledRuntimeManifestBound,
                model_binding: ResidentArtifactBinding::RegistryObservedDigestBound,
                ..
            }),
        ) => "asr.transcribe-cpp.streaming=composed:installed_unqualified",
        (
            true,
            Some(ResidentRuntimeSnapshot {
                supports_streaming: true,
                lifecycle: ResidentReadiness::Ready,
                ..
            }),
        ) => "asr.transcribe-cpp.streaming=composed:controlled_unqualified",
        (
            true,
            Some(ResidentRuntimeSnapshot {
                lifecycle: ResidentReadiness::Active,
                ..
            }),
        ) => "asr.transcribe-cpp.streaming=disabled:active",
        (
            true,
            Some(ResidentRuntimeSnapshot {
                lifecycle: ResidentReadiness::Warming,
                ..
            }),
        ) => "asr.transcribe-cpp.streaming=disabled:warming",
        (
            true,
            Some(ResidentRuntimeSnapshot {
                lifecycle: ResidentReadiness::RestartRequired,
                ..
            }),
        ) => "asr.transcribe-cpp.streaming=disabled:restart_required",
        (
            true,
            Some(ResidentRuntimeSnapshot {
                lifecycle: ResidentReadiness::FailedClosed,
                ..
            }),
        ) => "asr.transcribe-cpp.streaming=disabled:failed_closed",
        (
            true,
            Some(ResidentRuntimeSnapshot {
                supports_streaming: false,
                lifecycle: ResidentReadiness::Ready,
                ..
            }),
        ) => "asr.transcribe-cpp.streaming=disabled:model_unsupported",
        (true, None) => "asr.transcribe-cpp.streaming=disabled:resident_not_initialized",
        (false, _) => "asr.transcribe-cpp.streaming=disabled:not_composed",
    };
    capabilities.push(composition.to_owned());
    // T2.6: a dictation ends on Stop (or the capture safety limit), never on
    // silence, so a ready streaming model composes no VAD or endpointer.
    const ENDPOINT_NOT_USED: &str = "disabled:stop_on_request";
    let endpoint_state = match (strict_recovery, resident) {
        (
            true,
            Some(ResidentRuntimeSnapshot {
                model_binding: ResidentArtifactBinding::InstalledModelMissing,
                ..
            }),
        ) => "disabled:installed_model_missing",
        (true, Some(snapshot)) if !snapshot.bindings_ready() => "disabled:artifact_binding",
        (
            true,
            Some(ResidentRuntimeSnapshot {
                supports_streaming: true,
                lifecycle: ResidentReadiness::Ready,
                artifact_binding: ResidentArtifactBinding::InstalledRuntimeManifestBound,
                model_binding: ResidentArtifactBinding::RegistryObservedDigestBound,
                ..
            }),
        ) => ENDPOINT_NOT_USED,
        (
            true,
            Some(ResidentRuntimeSnapshot {
                supports_streaming: true,
                lifecycle: ResidentReadiness::Ready,
                ..
            }),
        ) => ENDPOINT_NOT_USED,
        (
            true,
            Some(ResidentRuntimeSnapshot {
                lifecycle: ResidentReadiness::Active,
                ..
            }),
        ) => "disabled:active",
        (
            true,
            Some(ResidentRuntimeSnapshot {
                lifecycle: ResidentReadiness::Warming,
                ..
            }),
        ) => "disabled:warming",
        (
            true,
            Some(ResidentRuntimeSnapshot {
                lifecycle: ResidentReadiness::RestartRequired,
                ..
            }),
        ) => "disabled:restart_required",
        (
            true,
            Some(ResidentRuntimeSnapshot {
                lifecycle: ResidentReadiness::FailedClosed,
                ..
            }),
        ) => "disabled:failed_closed",
        (
            true,
            Some(ResidentRuntimeSnapshot {
                supports_streaming: false,
                lifecycle: ResidentReadiness::Ready,
                ..
            }),
        ) => "disabled:streaming_model_unsupported",
        (true, None) => "disabled:resident_not_initialized",
        (false, _) => "disabled:not_composed",
    };
    capabilities.push(format!("audio.native-energy-vad={endpoint_state}"));
    capabilities.push(format!("speech.endpoint={endpoint_state}"));
}

fn append_recognition_request_capabilities(
    capabilities: &mut Vec<String>,
    asr: Option<&AsrCapabilities>,
) {
    let Some(asr) = asr else {
        capabilities.push("asr.recognition-settings=unavailable".to_owned());
        return;
    };
    capabilities.push("asr.recognition-settings=available".to_owned());
    capabilities.push(format!(
        "asr.language-detection={}",
        if asr.language_detection {
            "available"
        } else {
            "unavailable"
        }
    ));
    capabilities.push(format!(
        "asr.context-bias={}",
        if asr.hotword_biasing {
            "available"
        } else {
            "unavailable"
        }
    ));
    capabilities.push(format!(
        "asr.translation={}",
        if asr.translation {
            "available"
        } else {
            "unavailable"
        }
    ));
    for (name, tags) in [
        ("languages", &asr.languages),
        ("translation-targets", &asr.translate_target_languages),
        ("scripts", &asr.scripts),
        ("locales", &asr.locales),
    ] {
        if !tags.is_empty() {
            capabilities.push(format!(
                "asr.{name}={}",
                tags.iter()
                    .map(AsrTag::as_str)
                    .collect::<Vec<_>>()
                    .join(",")
            ));
        }
    }
}

impl DaemonState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_module(&self, descriptor: ModuleDescriptor) -> Result<(), CoreError> {
        self.module_registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .register(descriptor)
    }

    pub fn set_module_permission_grants(
        &self,
        module_id: &str,
        granted: impl IntoIterator<Item = ModulePermission>,
    ) -> Result<(), CoreError> {
        self.module_registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .set_permission_grants(module_id, granted)
    }

    /// Authorize first, then invoke one module operation. The registry lock is
    /// released before module code runs so a module cannot block registry
    /// administration or re-enter a poisoned lock.
    pub fn invoke_module<T>(
        &self,
        module_id: &str,
        permission: ModulePermission,
        operation: impl FnOnce() -> T,
    ) -> Result<T, CoreError> {
        self.module_registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .authorize(module_id, permission)?;
        Ok(operation())
    }

    #[cfg(all(test, feature = "asr-transcribe-resident"))]
    fn pause_next_provision_checkpoint(
        &self,
        checkpoint: ProvisionTestCheckpoint,
    ) -> ProvisionTestPauseControl {
        let (reached_tx, reached_rx) = sync_channel(0);
        let (resume_tx, resume_rx) = sync_channel(0);
        let slot = match checkpoint {
            ProvisionTestCheckpoint::BeforeImport => &self.provision_test_hooks.before_import,
            ProvisionTestCheckpoint::BeforeCandidateCommit => {
                &self.provision_test_hooks.before_candidate_commit
            }
        };
        let mut slot = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        assert!(
            slot.is_none(),
            "provision checkpoint pause already installed"
        );
        *slot = Some(ProvisionTestPause {
            reached: reached_tx,
            resume: resume_rx,
        });
        ProvisionTestPauseControl {
            reached: reached_rx,
            resume: resume_tx,
        }
    }

    #[cfg(all(test, feature = "asr-transcribe-resident"))]
    fn panic_before_next_provision_import(&self) {
        self.provision_test_hooks
            .panic_before_import
            .store(true, Ordering::Release);
    }

    #[cfg(all(test, feature = "asr-transcribe-resident"))]
    #[allow(clippy::panic)]
    fn run_provision_test_checkpoint(&self, checkpoint: ProvisionTestCheckpoint) {
        if matches!(checkpoint, ProvisionTestCheckpoint::BeforeImport)
            && self
                .provision_test_hooks
                .panic_before_import
                .swap(false, Ordering::AcqRel)
        {
            panic!("controlled provisioning worker panic");
        }
        let slot = match checkpoint {
            ProvisionTestCheckpoint::BeforeImport => &self.provision_test_hooks.before_import,
            ProvisionTestCheckpoint::BeforeCandidateCommit => {
                &self.provision_test_hooks.before_candidate_commit
            }
        };
        let pause = slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(pause) = pause {
            let _ = pause.reached.send(());
            let _ = pause.resume.recv();
        }
    }

    fn try_admit_strict(self: &Arc<Self>) -> Option<StrictSessionPermit> {
        self.strict_session_admitted
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| StrictSessionPermit {
                state: Arc::clone(self),
            })
    }

    /// Load and handshake the one daemon-owned resident model. Production
    /// startup calls this before accepting IPC, so readiness cannot precede
    /// a successful strict-CPU native model load.
    #[cfg(feature = "asr-transcribe-resident")]
    pub fn initialize_resident_asr(
        &self,
        model: &Path,
        expected_sha256: &str,
        expected_size: u64,
    ) -> Result<(), narrivy_asr_api::AsrError> {
        let mut lifecycle = self
            .resident_lifecycle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if lifecycle.phase_kind() != ProvisionPhaseKind::Unconfigured {
            return Err(narrivy_asr_api::AsrError::new(
                "resident_already_initialized",
                "the daemon resident ASR model is already initialized",
            ));
        }
        let engine = ResidentWorkerEngine::with_model_binding(
            model.to_path_buf(),
            expected_sha256,
            expected_size,
        )?;
        *lifecycle = ProvisionCoordinator::ready(ResidentCandidate {
            engine: ResidentEngineOwner::Installed(engine),
            installed_identity: None,
        });
        Ok(())
    }

    /// Compose independently verified fixed-root worker and installed-model
    /// observations into one daemon-owned resident. This accepts no raw
    /// worker/model path and asserts no package or release provenance.
    #[cfg(feature = "asr-transcribe-resident")]
    pub fn initialize_installed_resident_asr(
        &self,
        runtime: InstalledRuntimeBinding,
        model: InstalledModelRuntimeBinding,
    ) -> Result<(), narrivy_asr_api::AsrError> {
        let mut lifecycle = self
            .resident_lifecycle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if lifecycle.phase_kind() != ProvisionPhaseKind::Unconfigured {
            return Err(narrivy_asr_api::AsrError::new(
                "resident_already_initialized",
                "the daemon resident ASR model is already initialized",
            ));
        }
        let identity = ResidentInstalledIdentity {
            _worker_sha256: runtime.worker().sha256(),
            worker_size_bytes: runtime.worker().size(),
            _model_id: model.id().to_owned(),
            _model_family: model.family().to_owned(),
            _model_source_revision: model.source_revision().to_owned(),
            _model_sha256: model.model().sha256(),
            model_size_bytes: model.model().size(),
        };
        let engine = ResidentWorkerEngine::with_trust_anchored_bindings(
            runtime.into_worker(),
            model.into_model(),
        )?;
        *lifecycle = ProvisionCoordinator::ready(ResidentCandidate {
            engine: ResidentEngineOwner::Installed(engine),
            installed_identity: Some(identity),
        });
        Ok(())
    }

    /// Retain the verified installed worker while awaiting an absent bundled
    /// model. No resident process is created here.
    #[cfg(feature = "asr-transcribe-resident")]
    pub fn initialize_awaiting_installed_model(
        &self,
        runtime: InstalledRuntimeBinding,
    ) -> Result<(), narrivy_asr_api::AsrError> {
        let mut lifecycle = self
            .resident_lifecycle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if lifecycle.phase_kind() != ProvisionPhaseKind::Unconfigured {
            return Err(narrivy_asr_api::AsrError::new(
                "resident_already_initialized",
                "the daemon resident ASR model is already initialized",
            ));
        }
        *lifecycle = ProvisionCoordinator::awaiting(ResidentRuntimeAuthority::Installed(runtime));
        Ok(())
    }

    /// Configure the immutable daemon-side authority for a future offline
    /// model provision. The client can supply only a local source path; model
    /// identity, destination, deadline, and worker ownership remain here.
    #[cfg(feature = "asr-transcribe-resident")]
    pub fn configure_offline_model_provisioning(
        &self,
        store: Arc<narrivy_model_manager::ModelStore>,
        registry: Arc<narrivy_model_manager::BundledModelRegistry>,
        model_id: String,
        supervisor: Arc<WorkerSupervisor>,
    ) -> Result<(), narrivy_asr_api::AsrError> {
        self.configure_offline_model_provisioning_inner(ModelProvisioningAuthority {
            supervisor,
            backend: ModelProvisioningBackend::Installed {
                store,
                registry,
                model_id,
            },
        })
    }

    #[cfg(all(test, feature = "asr-transcribe-resident"))]
    fn configure_offline_model_provisioning_for_test(
        &self,
        supervisor: Arc<WorkerSupervisor>,
        backend: ControlledProvisionBackend,
    ) -> Result<(), narrivy_asr_api::AsrError> {
        {
            let mut lifecycle = self
                .resident_lifecycle
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if lifecycle.phase_kind() != ProvisionPhaseKind::Unconfigured {
                return Err(narrivy_asr_api::AsrError::new(
                    "resident_already_initialized",
                    "the daemon resident ASR model is already initialized",
                ));
            }
            *lifecycle = ProvisionCoordinator::awaiting(ResidentRuntimeAuthority::Controlled);
        }
        self.configure_offline_model_provisioning_inner(ModelProvisioningAuthority {
            supervisor,
            backend: ModelProvisioningBackend::Controlled(backend),
        })
    }

    #[cfg(feature = "asr-transcribe-resident")]
    fn configure_offline_model_provisioning_inner(
        &self,
        authority: ModelProvisioningAuthority,
    ) -> Result<(), narrivy_asr_api::AsrError> {
        let lifecycle = self
            .resident_lifecycle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if lifecycle.phase_kind() != ProvisionPhaseKind::Awaiting {
            return Err(narrivy_asr_api::AsrError::new(
                "model_provisioning_lifecycle_unavailable",
                "offline model provisioning requires the awaiting-model lifecycle",
            ));
        }
        if self.model_provisioning.set(authority).is_err() {
            return Err(narrivy_asr_api::AsrError::new(
                "model_provisioning_already_configured",
                "offline model provisioning is already configured",
            ));
        }
        drop(lifecycle);
        Ok(())
    }

    #[cfg(feature = "asr-transcribe-resident")]
    fn start_offline_model_provision(
        self: &Arc<Self>,
        source_path: String,
    ) -> Result<ModelProvisionStatusDto, (IpcErrorCode, String)> {
        let authority = self.model_provisioning.get().cloned().ok_or_else(|| {
            (
                IpcErrorCode::DaemonUnavailable,
                "offline model provisioning is not composed".to_owned(),
            )
        })?;
        let source_path = PathBuf::from(source_path);
        if !source_path.is_absolute() {
            return Err((
                IpcErrorCode::BadRequest,
                "offline model source must be an absolute local path".to_owned(),
            ));
        }
        let operation_id = generate_model_provision_operation_id().map_err(|()| {
            (
                IpcErrorCode::Internal,
                "secure provisioning identity unavailable".to_owned(),
            )
        })?;
        let admitted_at = Instant::now();
        let deadline = admitted_at
            .checked_add(MODEL_PROVISION_TIMEOUT)
            .ok_or_else(|| {
                (
                    IpcErrorCode::Internal,
                    "offline model provisioning deadline is unavailable".to_owned(),
                )
            })?;

        let mut lifecycle = self
            .resident_lifecycle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let admission = lifecycle
            .admit_start(operation_id, admitted_at, MODEL_PROVISION_COOLDOWN)
            .map_err(model_provision_start_rejection)?;
        let key = admission.key.clone();
        let fallback_runtime = admission.runtime.clone();
        let worker_state = Arc::clone(self);
        let worker_authority = authority.clone();
        let worker_key = admission.key;
        let worker_runtime = admission.runtime;
        let worker_flag = admission.cancellation.clone();
        let spawn = authority.supervisor.spawn_with_cancellation(
            "offline-model-provision",
            worker_flag,
            move |cancellation| {
                run_offline_model_provision(
                    worker_state,
                    worker_authority,
                    worker_key,
                    worker_runtime,
                    source_path,
                    cancellation,
                    deadline,
                );
            },
        );
        if spawn.is_err() {
            let _ = lifecycle.complete_terminal(
                &key,
                ProvisionTerminal::WorkerFailedBeforePublicationClean {
                    runtime: fallback_runtime,
                },
            );
            return Err((
                IpcErrorCode::Internal,
                "offline model provisioning worker was not started".to_owned(),
            ));
        }
        lifecycle.status().ok_or_else(|| {
            (
                IpcErrorCode::Internal,
                "offline model provisioning status is unavailable".to_owned(),
            )
        })
    }

    #[cfg(feature = "asr-transcribe-resident")]
    fn cancel_offline_model_provision(
        &self,
        operation_id: &str,
    ) -> Result<ModelProvisionStatusDto, (IpcErrorCode, String)> {
        if self.model_provisioning.get().is_none() {
            return Err((
                IpcErrorCode::DaemonUnavailable,
                "offline model provisioning is not composed".to_owned(),
            ));
        }
        let mut lifecycle = self
            .resident_lifecycle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let key = lifecycle.active_key().ok_or_else(|| {
            (
                IpcErrorCode::BadRequest,
                "offline model provisioning operation is not active".to_owned(),
            )
        })?;
        if key.operation_id() != operation_id {
            return Err((
                IpcErrorCode::BadRequest,
                "offline model provisioning operation is not active".to_owned(),
            ));
        }
        lifecycle
            .request_cancel(&key)
            .map_err(model_provision_transition_rejection)
    }

    #[cfg(feature = "asr-transcribe-resident")]
    fn resident_asr(&self) -> Option<ResidentWorkerEngine> {
        self.resident_lifecycle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .ready_candidate()
            .and_then(|candidate| candidate.engine.installed_clone())
    }

    /// Snapshot the already-bound daemon resident without reopening its
    /// configured executable path. Status and request admission use this same
    /// source of runtime truth.
    #[cfg(feature = "asr-transcribe-resident")]
    fn resident_runtime_snapshot(&self) -> Option<ResidentRuntimeSnapshot> {
        self.resident_status_snapshot().0
    }

    #[cfg(feature = "asr-transcribe-resident")]
    fn resident_status_snapshot(
        &self,
    ) -> (
        Option<ResidentRuntimeSnapshot>,
        Option<ModelProvisionStatusDto>,
    ) {
        let lifecycle = self
            .resident_lifecycle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let snapshot = match lifecycle.phase_kind() {
            ProvisionPhaseKind::Ready => {
                let Some(candidate) = lifecycle.ready_candidate() else {
                    return (None, lifecycle.status());
                };
                let installed_identity = &candidate.installed_identity;
                let installed_identity_present = installed_identity.is_some();
                match &candidate.engine {
                    ResidentEngineOwner::Installed(engine) => {
                        let (artifact_binding, model_binding) = resident_binding_attribution(
                            engine.artifact_status(),
                            engine.model_artifact_status(),
                            installed_identity_present,
                        );
                        Some(ResidentRuntimeSnapshot {
                            lifecycle: engine.readiness(),
                            supports_streaming: engine.model_capabilities().supports_streaming,
                            artifact_binding,
                            model_binding,
                            installed_identity_present,
                            installed_model_lifecycle: installed_identity_present
                                .then_some(ResidentInstalledLifecycle::Ready),
                        })
                    }
                    #[cfg(test)]
                    ResidentEngineOwner::Controlled(_) => Some(ResidentRuntimeSnapshot {
                        lifecycle: ResidentReadiness::Ready,
                        supports_streaming: true,
                        artifact_binding: ResidentArtifactBinding::ControlledDigestBound,
                        model_binding: ResidentArtifactBinding::ControlledDigestBound,
                        installed_identity_present: false,
                        installed_model_lifecycle: None,
                    }),
                }
            }
            ProvisionPhaseKind::Awaiting => Some(installed_unavailable_snapshot(
                ResidentArtifactBinding::InstalledModelMissing,
                ResidentInstalledLifecycle::Awaiting,
            )),
            ProvisionPhaseKind::Provisioning => Some(installed_unavailable_snapshot(
                if lifecycle.status().is_some_and(|status| {
                    status.publication == ModelProvisionPublicationDto::VerifiedInstalled
                }) {
                    ResidentArtifactBinding::StoredBindingUnavailable
                } else {
                    ResidentArtifactBinding::InstalledModelMissing
                },
                ResidentInstalledLifecycle::Provisioning,
            )),
            ProvisionPhaseKind::PublishedUnconfirmed => Some(installed_unavailable_snapshot(
                ResidentArtifactBinding::StoredBindingUnavailable,
                ResidentInstalledLifecycle::PublishedUnconfirmed,
            )),
            ProvisionPhaseKind::ActivationFailed => Some(installed_unavailable_snapshot(
                ResidentArtifactBinding::StoredBindingUnavailable,
                ResidentInstalledLifecycle::ActivationFailed,
            )),
            ProvisionPhaseKind::Unconfigured | ProvisionPhaseKind::Closing => None,
        };
        (snapshot, lifecycle.status())
    }

    /// Remove the resident handle from daemon lookup before shutdown begins.
    /// Existing worker-owned clones remain governed by the engine's shared
    /// admission closure and bounded shutdown contract.
    #[cfg(feature = "asr-transcribe-resident")]
    pub fn take_resident_asr(&self) -> Option<ResidentWorkerEngine> {
        let mut lifecycle = self
            .resident_lifecycle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        lifecycle
            .close()
            .and_then(|candidate| candidate.engine.into_installed())
    }

    fn lock_sessions(&self) -> std::sync::MutexGuard<'_, HashMap<u128, SessionSlot>> {
        self.sessions.lock().unwrap_or_else(|p| p.into_inner())
    }

    #[cfg(feature = "asr-transcribe-cpp")]
    fn record_completed_timing(&self, timing: &RecoveryTimingTrace) {
        let snapshot = timing.snapshot_us();
        if !snapshot.is_empty() {
            *self
                .last_session_timings
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(StageTimingsUs(snapshot));
        }
    }

    fn last_session_timing(&self) -> Option<StageTimingsUs> {
        self.last_session_timings
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Begin a session, persist its Pending journal entry, and register it.
    /// Strict microphone lanes remain in `Preparing`; the portable recovery
    /// slice publishes `Listening` only after the capture adapter reports the
    /// selected source ready. Historical non-strict lanes retain their
    /// immediate `Listening` transition until they are migrated.
    ///
    /// The caller hands in the already resolved [`AdmittedPipeline`] so the
    /// correction strategy, journal identity and capture-ready timing all come
    /// from one admission decision rather than being re-derived here.
    pub fn begin_session(
        &self,
        journal: &Journal,
        admitted: &AdmittedPipeline,
    ) -> Result<(u128, TransactionId), CoreError> {
        let mut random = SysRng;
        self.begin_session_with_entropy(journal, admitted, |bytes| {
            random.try_fill_bytes(bytes).map_err(|_| {
                CoreError::Worker("secure transaction identity unavailable".to_owned())
            })
        })
    }

    fn begin_session_with_entropy<F>(
        &self,
        journal: &Journal,
        admitted: &AdmittedPipeline,
        mut fill_entropy: F,
    ) -> Result<(u128, TransactionId), CoreError>
    where
        F: FnMut(&mut [u8; TRANSACTION_ID_RANDOM_BYTES]) -> Result<(), CoreError>,
    {
        let _admission = self
            .transaction_admission
            .lock()
            .map_err(|_| CoreError::Worker("transaction admission lock poisoned".to_owned()))?;
        let sid = 1u128 << 32 | u128::from(self.next_session.fetch_add(1, Ordering::SeqCst));
        let pipeline_id = admitted.id().to_owned();
        let mut admitted_txid = None;
        for _ in 0..TRANSACTION_ID_ATTEMPTS {
            let mut random_bytes = [0u8; TRANSACTION_ID_RANDOM_BYTES];
            fill_entropy(&mut random_bytes)?;
            let txid = TransactionId::new(format!("tx-{:032x}", u128::from_be_bytes(random_bytes)));
            let mut entry = narrivy_core::JournalEntry::new_pending(txid.as_str(), unix_now_ms());
            entry.pipeline_id = Some(pipeline_id.clone());
            match journal.write_new(&entry) {
                Ok(()) => {
                    admitted_txid = Some(txid);
                    break;
                }
                Err(narrivy_core::JournalError::DuplicateTransactionId) => continue,
                Err(error) => return Err(error.into()),
            }
        }
        let Some(txid) = admitted_txid else {
            return Err(CoreError::Worker(
                "secure transaction identity collision budget exhausted".to_owned(),
            ));
        };
        let mut machine = SessionMachine::with_session(SessionId(sid));
        self.publish_session(
            &mut machine,
            SessionState::Preparing,
            Some(txid.clone()),
            Some(pipeline_id.clone()),
        )?;
        if !admitted.defers_listening_until_capture_ready() {
            self.publish_session(
                &mut machine,
                SessionState::Listening,
                Some(txid.clone()),
                Some(pipeline_id.clone()),
            )?;
        }

        self.lock_sessions().insert(
            sid,
            SessionSlot {
                machine,
                transaction_id: txid.clone(),
                pipeline_id: Some(pipeline_id),
                correction: admitted.correction(),
                control: SessionControl::default(),
                timing: Arc::new(RecoveryTimingTrace::new()),
            },
        );
        Ok((sid, txid))
    }

    /// Run the full mock pipeline for one started session against an
    /// injected engine + delivery closure.
    #[allow(clippy::too_many_arguments)]
    /// FinalizingAudio -> Transcribing -> Transforming -> Delivering ->
    /// Committed, with truthful cancel/fail paths and journal tracking.
    pub fn run_session<E, D>(
        &self,
        session_id: u128,
        journal: &Journal,
        mut engine: E,
        delivery: &D,
        request: SessionRequest,
        cancel: CancelFlag,
        variants: &mut TranscriptVariants,
    ) -> Result<DeliveryOutcome, CoreError>
    where
        E: AsrEngine,
        D: Fn(&str) -> Result<(), CoreError>,
    {
        let (mut machine, txid, correction, pipeline_id) = {
            let mut sessions = self.lock_sessions();
            let slot = sessions
                .get_mut(&session_id)
                .ok_or_else(|| CoreError::Worker(format!("unknown session {session_id}")))?;
            (
                slot.machine,
                slot.transaction_id.clone(),
                slot.correction,
                slot.pipeline_id.clone(),
            )
        };
        let tx_for_events = txid.clone();

        macro_rules! step {
            ($to:expr) => {{
                let event = machine.transition_with_event(
                    $to,
                    Some(tx_for_events.clone()),
                    unix_now_ms(),
                )?;
                self.reinsert_machine(session_id, machine);
                event
            }};
        }

        self.publish_session_event(step!(SessionState::FinalizingAudio), pipeline_id.clone());
        if cancel.is_cancelled() {
            return self.cancel_path(session_id, machine, &txid, journal);
        }
        self.publish_session_event(step!(SessionState::Transcribing), pipeline_id.clone());

        let mut stream = engine
            .open_stream(SessionId(session_id), &cancel)
            .map_err(CoreError::Asr)?;
        stream
            .push_audio(&AudioChunk {
                sample_rate_hz: request.sample_rate_hz,
                channels: 1,
                samples: request.audio_samples,
            })
            .map_err(CoreError::Asr)?;
        stream.finalize().map_err(CoreError::Asr)?;

        let mut final_text: Option<String> = None;
        let mut last_seq: u64 = 0;
        while let Some(event) = stream.poll_event() {
            match event {
                narrivy_asr_api::AsrEvent::Started { .. } => {}
                narrivy_asr_api::AsrEvent::Partial { seq, transcript } => {
                    if seq > last_seq {
                        last_seq = seq;
                        variants.raw = Some(transcript.text);
                    }
                }
                narrivy_asr_api::AsrEvent::Final {
                    seq, transcript, ..
                } => {
                    if seq > last_seq {
                        last_seq = seq;
                        final_text = Some(transcript.text);
                    }
                }
                narrivy_asr_api::AsrEvent::Error(error) => {
                    stream
                        .finish(Duration::from_secs(1))
                        .map_err(CoreError::Asr)?;
                    if cancel.is_cancelled() && error.code == "cancelled" {
                        return self.cancel_path(session_id, machine, &txid, journal);
                    }
                    return self.fail_path(session_id, machine, &txid, journal, error.code);
                }
            }
        }
        stream
            .finish(Duration::from_secs(2))
            .map_err(CoreError::Asr)?;
        let raw = final_text.ok_or_else(|| {
            CoreError::Asr(narrivy_asr_api::AsrError::new(
                "no_final",
                "engine produced no final",
            ))
        })?;
        variants.raw = Some(raw.clone());

        if cancel.is_cancelled() {
            return self.cancel_path(session_id, machine, &txid, journal);
        }
        self.publish_session_event(step!(SessionState::Transforming), pipeline_id.clone());
        let corrected = correction.applies(&raw);
        variants.corrected = Some(corrected.clone());

        // Commit before delivery, like the strict lanes: once the corrected
        // text is final it is durable with its delivery intent, so a crash
        // during delivery is replayed into the Inbox at the next start.
        Self::finalize_entry_with_intent(journal, &txid, variants)?;
        self.publish_session_event(step!(SessionState::Delivering), pipeline_id.clone());
        match delivery(&corrected) {
            Ok(()) => {
                variants.delivered = Some(corrected);
                variants.attempts.push(DeliveryAttempt {
                    backend: "null-platform".to_owned(),
                    outcome: DeliveryOutcome::Delivered,
                    fallback_reason: None,
                    at_unix_ms: unix_now_ms(),
                });
                Self::finalize_entry(journal, &txid, JournalEntryState::Delivered, Some(variants))?;
                self.publish_session_event(step!(SessionState::Committed), pipeline_id.clone());
                self.publish_session_event(step!(SessionState::Idle), pipeline_id.clone());
                self.reinsert_machine(session_id, machine);
                Ok(DeliveryOutcome::Delivered)
            }
            Err(_err) => {
                variants.attempts.push(DeliveryAttempt {
                    backend: "inbox".to_owned(),
                    outcome: DeliveryOutcome::InboxFallback,
                    fallback_reason: Some(FallbackReason::BackendError {
                        code: "delivery_failed".to_owned(),
                    }),
                    at_unix_ms: unix_now_ms(),
                });
                Self::finalize_entry(journal, &txid, JournalEntryState::Committed, Some(variants))?;
                self.publish_session_event(step!(SessionState::Committed), pipeline_id.clone());
                self.publish_session_event(step!(SessionState::Idle), pipeline_id.clone());
                self.reinsert_machine(session_id, machine);
                Ok(DeliveryOutcome::InboxFallback)
            }
        }
    }

    /// Journal the final corrected text as `Committed` with its delivery
    /// intent, before any delivery side effect.
    fn finalize_entry_with_intent(
        journal: &Journal,
        txid: &TransactionId,
        variants: &TranscriptVariants,
    ) -> Result<(), CoreError> {
        let mut entry = journal.read(txid.as_str())?.ok_or_else(|| {
            CoreError::Journal(narrivy_core::JournalError::Storage(
                "missing admitted transaction before delivery".to_owned(),
            ))
        })?;
        entry.state = JournalEntryState::Committed;
        entry.variants = variants.clone();
        entry.delivery_intent = Some(narrivy_types::DeliveryIntent {
            transaction_id: txid.clone(),
            backend: "null-platform".to_owned(),
        });
        entry.updated_unix_ms = unix_now_ms();
        journal.write(&entry)?;
        Ok(())
    }

    fn finalize_entry(
        journal: &Journal,
        txid: &TransactionId,
        state: JournalEntryState,
        variants: Option<&TranscriptVariants>,
    ) -> Result<(), CoreError> {
        let mut entry = journal.read(txid.as_str())?.ok_or_else(|| {
            CoreError::Journal(narrivy_core::JournalError::Storage(
                "missing admitted transaction during finalization".to_owned(),
            ))
        })?;
        entry.state = state;
        if let Some(variants) = variants {
            entry.variants = variants.clone();
        }
        entry.updated_unix_ms = unix_now_ms();
        journal.write(&entry)?;
        Ok(())
    }

    fn publish_session(
        &self,
        machine: &mut SessionMachine,
        to: SessionState,
        txid: Option<TransactionId>,
        pipeline_id: Option<String>,
    ) -> Result<(), CoreError> {
        let event = machine.transition_with_event(to, txid, unix_now_ms())?;
        self.publish_session_event(event, pipeline_id);
        Ok(())
    }

    fn publish_session_event(
        &self,
        event: narrivy_types::SessionEvent,
        pipeline_id: Option<String>,
    ) {
        self.publish_session_event_with(event, pipeline_id, None);
    }

    fn publish_session_event_with(
        &self,
        event: narrivy_types::SessionEvent,
        pipeline_id: Option<String>,
        delivery: Option<&'static str>,
    ) {
        // Start and stop sounds, when turned on.
        match (event.from, event.to) {
            (_, SessionState::Listening) => super::sounds::play(super::sounds::Cue::Start),
            (SessionState::Listening, SessionState::FinalizingAudio | SessionState::Cancelled) => {
                super::sounds::play(super::sounds::Cue::Stop);
            }
            _ => {}
        }
        self.bus.publish_priority(
            DaemonEvent::Session {
                event,
                pipeline_id,
                delivery,
            },
            |queued| matches!(queued, DaemonEvent::Partial { .. }),
        );
    }

    #[cfg(any(feature = "asr-transcribe-resident", test))]
    fn publish_partial(
        &self,
        session_id: u128,
        transaction_id: &TransactionId,
        pipeline_id: &str,
        partial: VolatilePartial,
    ) {
        let transaction_id = transaction_id.as_str().to_owned();
        self.bus.publish_coalesced(
            DaemonEvent::Partial {
                session_id,
                transaction_id: transaction_id.clone(),
                pipeline_id: pipeline_id.to_owned(),
                partial,
            },
            |queued| {
                matches!(
                    queued,
                    DaemonEvent::Partial {
                        session_id: queued_session,
                        transaction_id: queued_transaction,
                        ..
                    } if *queued_session == session_id && queued_transaction == &transaction_id
                )
            },
            |queued| matches!(queued, DaemonEvent::Partial { .. }),
        );
    }

    fn cancel_path(
        &self,
        session_id: u128,
        mut machine: SessionMachine,
        txid: &TransactionId,
        journal: &Journal,
    ) -> Result<DeliveryOutcome, CoreError> {
        Self::finalize_entry(journal, txid, JournalEntryState::Cancelled, None)?;
        let ev = machine.transition_with_event(
            SessionState::Cancelled,
            Some(txid.clone()),
            unix_now_ms(),
        )?;
        self.publish_session_event(ev, self.session_pipeline(session_id));
        let ev = machine.transition_with_event(SessionState::Idle, None, unix_now_ms())?;
        self.publish_session_event(ev, self.session_pipeline(session_id));
        self.reinsert_machine(session_id, machine);
        Ok(DeliveryOutcome::Failed)
    }

    fn fail_path(
        &self,
        session_id: u128,
        mut machine: SessionMachine,
        txid: &TransactionId,
        journal: &Journal,
        failure_code: &str,
    ) -> Result<DeliveryOutcome, CoreError> {
        let mut entry = journal.read(txid.as_str())?.ok_or_else(|| {
            CoreError::Journal(narrivy_core::JournalError::Storage(
                "missing admitted transaction during failure finalization".to_owned(),
            ))
        })?;
        entry.state = JournalEntryState::FailedTerminal;
        entry.failure_code = Some(failure_code.to_owned());
        entry.updated_unix_ms = unix_now_ms();
        journal.write(&entry)?;
        let ev = machine.transition_with_event(
            SessionState::Failed,
            Some(txid.clone()),
            unix_now_ms(),
        )?;
        self.publish_session_event(ev, self.session_pipeline(session_id));
        let ev = machine.transition_with_event(SessionState::Idle, None, unix_now_ms())?;
        self.publish_session_event(ev, self.session_pipeline(session_id));
        self.reinsert_machine(session_id, machine);
        Ok(DeliveryOutcome::Failed)
    }

    fn reinsert_machine(&self, session_id: u128, machine: SessionMachine) {
        if let Some(slot) = self.lock_sessions().get_mut(&session_id) {
            slot.machine = machine;
        }
    }

    fn session_pipeline(&self, session_id: u128) -> Option<String> {
        self.lock_sessions()
            .get(&session_id)
            .and_then(|slot| slot.pipeline_id.clone())
    }

    /// Request graceful capture finalization for every active session.
    /// This is deliberately independent of cancellation: accepted audio must
    /// continue through transcription, policy, correction, and delivery.
    fn request_active_finalization(&self) -> usize {
        let sessions = self.lock_sessions();
        for slot in sessions.values() {
            slot.control.request_finalization();
        }
        sessions.len()
    }

    /// Hold the atomic toggle/start admission gate for one decision.
    fn lock_toggle_admission(&self) -> std::sync::MutexGuard<'_, ()> {
        self.toggle_admission
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Decide what one atomic toggle must do, and (for a live capture session)
    /// request graceful finalization exactly once while the toggle gate is
    /// held. The caller must already own [`DaemonState::lock_toggle_admission`]
    /// and must perform the returned [`ToggleDecision::Start`] start path
    /// before releasing it, so concurrent toggles linearize on the session map.
    ///
    /// No path here cancels: a toggle either starts one session or asks the
    /// existing one to stop gracefully.
    /// Whether no session exists at all (a toggle now would start one).
    fn no_sessions(&self) -> bool {
        self.lock_sessions().is_empty()
    }

    fn toggle_decision(&self) -> ToggleDecision {
        let mut sessions = self.lock_sessions();
        let Some(slot) = sessions
            .values_mut()
            .min_by_key(|slot| slot.machine.session_id().0)
        else {
            return ToggleDecision::Start;
        };
        let session_id = slot.machine.session_id().0;
        let transaction_id = slot.transaction_id.as_str().to_owned();
        let pipeline_id = slot.pipeline_id.clone();
        // `Preparing` covers a strict capture acquisition that has not yet
        // published `Listening`; both are live capture and can be finalized
        // gracefully. Later stages are already stopping.
        let live_capture = matches!(
            slot.machine.state(),
            SessionState::Preparing | SessionState::Listening
        );
        if live_capture && !slot.control.finalization().is_requested() {
            slot.control.request_finalization();
            return ToggleDecision::FinalizationRequested {
                session_id,
                transaction_id,
                pipeline_id,
            };
        }
        ToggleDecision::AlreadyStopping {
            session_id,
            transaction_id,
            pipeline_id,
        }
    }

    /// Replace the content-free activation facts (startup composition and
    /// tests). Only the daemon-owned composition decides these.
    /// Compose desktop shortcut management once at startup. A second call is
    /// ignored: the service is immutable for the daemon's lifetime.
    pub fn model_selection(&self) -> &super::model_selection::ModelSelection {
        &self.model_selection
    }

    /// Load `model` and put it in place of the resident model. The new
    /// model is loaded first: on any failure the old one stays in use.
    #[cfg(feature = "asr-transcribe-resident")]
    fn replace_resident_model(
        &self,
        model: &super::model_selection::SelectedModel,
        compute: super::model_selection::ComputeChoice,
        cancellation: &CancelFlag,
    ) -> Result<(), &'static str> {
        let engine = match model.engine {
            super::model_selection::ModelEngine::TranscribeCpp => {
                ResidentWorkerEngine::with_model_binding_until(
                    model.path.clone(),
                    &model.sha256,
                    model.size,
                    compute.request(),
                    Instant::now() + MODEL_SELECTION_TIMEOUT,
                    cancellation,
                )
            }
            super::model_selection::ModelEngine::OpenVino => {
                let pack = model.path.parent().ok_or("model_unavailable")?;
                let models = super::model_selection::models_directory(&model.path)
                    .ok_or("model_unavailable")?;
                let leading = super::openvino::leading_args(&models, pack)
                    .ok_or("openvino_cache_unavailable")?;
                ResidentWorkerEngine::with_worker_binding_until(
                    super::openvino::worker()?,
                    leading,
                    model.path.clone(),
                    &model.sha256,
                    model.size,
                    compute.request(),
                    Instant::now() + OPENVINO_LOAD_TIMEOUT,
                    cancellation,
                )
            }
        }
        .map_err(|error| error.code)?;
        let previous = {
            let mut lifecycle = self
                .resident_lifecycle
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let previous = lifecycle
                .ready_candidate()
                .and_then(|candidate| candidate.engine.installed_clone());
            *lifecycle = ProvisionCoordinator::ready(ResidentCandidate {
                engine: ResidentEngineOwner::Installed(engine),
                installed_identity: None,
            });
            previous
        };
        if let Some(previous) = previous {
            // A worker that does not stop in time is still owned by its
            // engine, which keeps reaping it; the new model is already in use.
            let _ = previous.shutdown(REPLACED_MODEL_SHUTDOWN_TIMEOUT);
        }
        Ok(())
    }

    pub fn configure_shortcuts(&self, service: Arc<super::shortcuts::ShortcutService>) {
        let _ = self.shortcuts.set(service);
    }

    pub(crate) fn shortcuts(&self) -> Option<&Arc<super::shortcuts::ShortcutService>> {
        self.shortcuts.get()
    }

    pub fn configure_activation_facts(&self, facts: ActivationFacts) {
        *self
            .activation_facts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = facts;
    }

    /// Current activation facts; never inferred from the desktop name.
    pub(crate) fn activation_facts(&self) -> ActivationFacts {
        *self
            .activation_facts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Cancel every active session and all later pipeline stages.
    fn cancel_active_sessions(&self) -> usize {
        let sessions = self.lock_sessions();
        for slot in sessions.values() {
            slot.control.cancel();
        }
        sessions.len()
    }

    fn observe_recovery_state(
        &self,
        session_id: u128,
        txid: &TransactionId,
        to: SessionState,
        delivery: Option<&'static str>,
    ) -> Result<(), CoreError> {
        let (event, pipeline_id) = {
            let mut sessions = self.lock_sessions();
            let slot = sessions
                .get_mut(&session_id)
                .ok_or_else(|| CoreError::Worker(format!("unknown session {session_id}")))?;
            (
                slot.machine
                    .transition_with_event(to, Some(txid.clone()), unix_now_ms())?,
                slot.pipeline_id.clone(),
            )
        };
        self.publish_session_event_with(event, pipeline_id, delivery);
        Ok(())
    }

    fn recovery_initialization_failed(
        &self,
        session_id: u128,
        txid: &TransactionId,
        journal: &Journal,
        code: &str,
    ) {
        if self
            .try_recovery_initialization_failed(session_id, txid, journal, code)
            .is_err()
        {
            eprintln!("narrivyd: session failure terminalization failed");
        }
    }

    fn try_recovery_initialization_failed(
        &self,
        session_id: u128,
        txid: &TransactionId,
        journal: &Journal,
        code: &str,
    ) -> Result<(), CoreError> {
        let mut entry = journal.read(txid.as_str())?.ok_or_else(|| {
            CoreError::Journal(narrivy_core::JournalError::Storage(
                "missing admitted transaction during failure terminalization".to_owned(),
            ))
        })?;
        entry.state = JournalEntryState::FailedTerminal;
        entry.failure_code = Some(code.to_owned());
        entry.updated_unix_ms = unix_now_ms();
        journal.write(&entry)?;
        self.observe_recovery_state(session_id, txid, SessionState::Failed, None)?;
        self.observe_recovery_state(session_id, txid, SessionState::Idle, None)
    }

    /// A cancellation observed before the recovery slice has acquired capture
    /// or ASR resources.  It is still a real terminal state, not a silently
    /// removed pending entry.
    fn recovery_cancelled_before_run(
        &self,
        session_id: u128,
        txid: &TransactionId,
        journal: &Journal,
    ) {
        if self
            .try_recovery_cancelled(session_id, txid, journal)
            .is_err()
        {
            eprintln!("narrivyd: session cancellation terminalization failed");
        }
    }

    fn try_recovery_cancelled(
        &self,
        session_id: u128,
        txid: &TransactionId,
        journal: &Journal,
    ) -> Result<(), CoreError> {
        let mut entry = journal.read(txid.as_str())?.ok_or_else(|| {
            CoreError::Journal(narrivy_core::JournalError::Storage(
                "missing admitted transaction during cancellation terminalization".to_owned(),
            ))
        })?;
        entry.state = JournalEntryState::Cancelled;
        entry.failure_code = Some("cancelled".to_owned());
        entry.updated_unix_ms = unix_now_ms();
        journal.write(&entry)?;
        self.observe_recovery_state(session_id, txid, SessionState::Cancelled, None)?;
        self.observe_recovery_state(session_id, txid, SessionState::Idle, None)
    }

    /// `begin_session` has already persisted a Pending entry when spawning a
    /// worker fails.  Close it synchronously so restart cannot misread a
    /// never-started session as recoverable work.
    fn recovery_spawn_failed(&self, session_id: u128, txid: &TransactionId, journal: &Journal) {
        self.recovery_initialization_failed(session_id, txid, journal, "worker_spawn_failed");
        self.lock_sessions().remove(&session_id);
    }
}

// ---------------------------------------------------------------------------
// IPC wiring helpers
// ---------------------------------------------------------------------------

pub type DeliverySink = Arc<Mutex<Vec<String>>>;

/// Last-resort ownership guard for an admitted session worker.
///
/// Every ordinary exit should record its precise terminal reason before this
/// guard runs. If a return or panic would otherwise abandon a Pending record,
/// the guard fails it closed and always releases the in-memory session slot.
struct SessionWorkerCleanup {
    state: Arc<DaemonState>,
    journal: Arc<Journal>,
    session_id: u128,
    transaction_id: TransactionId,
    /// Dropped only after this guard's `Drop` has terminalized and removed
    /// the session, so strict admission cannot reopen early.
    _strict_permit: Option<Arc<StrictSessionPermit>>,
}

impl SessionWorkerCleanup {
    fn new(
        state: Arc<DaemonState>,
        journal: Arc<Journal>,
        session_id: u128,
        transaction_id: TransactionId,
        strict_permit: Option<Arc<StrictSessionPermit>>,
    ) -> Self {
        Self {
            state,
            journal,
            session_id,
            transaction_id,
            _strict_permit: strict_permit,
        }
    }
}

impl Drop for SessionWorkerCleanup {
    fn drop(&mut self) {
        let pending = self
            .journal
            .read(self.transaction_id.as_str())
            .ok()
            .flatten()
            .is_some_and(|entry| entry.state == JournalEntryState::Pending);
        if pending {
            let failure_code = if std::thread::panicking() {
                "worker_panicked"
            } else {
                "worker_exited_pending"
            };
            self.state.recovery_initialization_failed(
                self.session_id,
                &self.transaction_id,
                &self.journal,
                failure_code,
            );
        }
        self.state.lock_sessions().remove(&self.session_id);
    }
}

fn core_failure_code(error: &CoreError) -> &str {
    match error {
        CoreError::Cancelled => "cancelled",
        CoreError::InvalidTransition { .. } => "invalid_session_transition",
        CoreError::Journal(_) => "journal_failure",
        CoreError::Migration(_) => "migration_failure",
        CoreError::Store(_) => "store_failure",
        CoreError::Worker(_) => "worker_failure",
        CoreError::Module(_) => "module_failure",
        CoreError::Platform(error) => error.code,
        CoreError::Asr(error) => error.code,
    }
}

#[cfg(feature = "asr-transcribe-resident")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResidentRecoverySchedule {
    Spawned,
    AlreadyReady,
    AlreadyActive,
    AlreadyWarming,
    FailedClosed,
}

#[cfg(feature = "asr-transcribe-resident")]
fn resident_recovery_without_spawn(
    readiness: ResidentReadiness,
) -> Option<ResidentRecoverySchedule> {
    match readiness {
        ResidentReadiness::Ready => Some(ResidentRecoverySchedule::AlreadyReady),
        ResidentReadiness::Active => Some(ResidentRecoverySchedule::AlreadyActive),
        ResidentReadiness::Warming => Some(ResidentRecoverySchedule::AlreadyWarming),
        ResidentReadiness::FailedClosed => Some(ResidentRecoverySchedule::FailedClosed),
        ResidentReadiness::RestartRequired => None,
    }
}

#[cfg(feature = "asr-transcribe-resident")]
fn schedule_resident_recovery(
    engine: ResidentWorkerEngine,
    supervisor: &Arc<WorkerSupervisor>,
) -> Result<ResidentRecoverySchedule, CoreError> {
    if let Some(outcome) = resident_recovery_without_spawn(engine.readiness()) {
        return Ok(outcome);
    }
    supervisor
        .spawn("resident-asr-recovery", move |cancellation| {
            // Recovery is engine-deduplicated. The supervisor owns this task,
            // propagates shutdown cancellation, and joins it within the
            // daemon's normal bounded shutdown policy.
            let _ = engine.recover(&cancellation);
        })
        .map(|_| ResidentRecoverySchedule::Spawned)
}

#[cfg(feature = "asr-transcribe-resident")]
fn require_live_resident_ready(
    state: &Arc<DaemonState>,
    supervisor: &Arc<WorkerSupervisor>,
    config: &EngineConfig,
) -> Result<(), (IpcErrorCode, String)> {
    if !config.transcribe_resident {
        return Ok(());
    }
    match resident_admission_decision(state.resident_runtime_snapshot())? {
        ResidentAdmissionDecision::Ready => Ok(()),
        ResidentAdmissionDecision::Recover => {
            let scheduled = state
                .resident_asr()
                .ok_or_else(|| {
                    (
                        IpcErrorCode::DaemonUnavailable,
                        "resident ASR is not initialized".to_owned(),
                    )
                })
                .and_then(|engine| {
                    schedule_resident_recovery(engine, supervisor).map_err(|_| {
                        (
                            IpcErrorCode::DaemonUnavailable,
                            "resident ASR recovery could not be scheduled".to_owned(),
                        )
                    })
                });
            let detail = match scheduled {
                Ok(ResidentRecoverySchedule::Spawned) => "resident ASR recovery started",
                Ok(ResidentRecoverySchedule::AlreadyReady) => {
                    "resident ASR recovered before scheduling; retry request"
                }
                Ok(ResidentRecoverySchedule::AlreadyActive) => {
                    "resident ASR became active before scheduling"
                }
                Ok(ResidentRecoverySchedule::AlreadyWarming) => {
                    "resident ASR recovery is already active"
                }
                Ok(ResidentRecoverySchedule::FailedClosed) => {
                    "resident ASR failed closed before scheduling"
                }
                Err(_) => "resident ASR recovery could not be scheduled",
            };
            Err((IpcErrorCode::DaemonUnavailable, detail.to_owned()))
        }
    }
}

fn strict_pipeline_live_ready(
    #[cfg_attr(not(feature = "asr-transcribe-resident"), allow(unused_variables))]
    state: &DaemonState,
    config: &EngineConfig,
) -> bool {
    if !config.transcribe_resident {
        return true;
    }
    #[cfg(feature = "asr-transcribe-resident")]
    {
        state.resident_runtime_snapshot().is_some_and(|snapshot| {
            snapshot.lifecycle == ResidentReadiness::Ready && snapshot.bindings_ready()
        })
    }
    #[cfg(not(feature = "asr-transcribe-resident"))]
    {
        false
    }
}

fn admit_session_request(
    state: &Arc<DaemonState>,
    journal: &Journal,
    admitted: &AdmittedPipeline,
    config: &EngineConfig,
    supervisor: &Arc<WorkerSupervisor>,
) -> Result<(u128, TransactionId, Option<StrictSessionPermit>), (IpcErrorCode, String)> {
    // The composition lane, not the pipeline id, decides whether the strict
    // single-session permit and live resident readiness are required. The
    // resolved lane therefore cannot be redirected by a caller-supplied id.
    let strict = admitted.lane() == PipelineLane::StrictCapture;
    let permit = if strict {
        Some(state.try_admit_strict().ok_or_else(|| {
            let detail = if state.model_selection.loading_id().is_some() {
                "a model is loading; dictation starts once it is ready"
            } else {
                "a strict dictation session is already active"
            };
            (IpcErrorCode::Busy, detail.to_owned())
        })?)
    } else {
        None
    };
    if strict {
        require_live_resident_ready(state, supervisor, config)?;
    }
    let (session_id, transaction_id) = state.begin_session(journal, admitted).map_err(|error| {
        (
            IpcErrorCode::Internal,
            format!("begin_session failed: {error}"),
        )
    })?;
    Ok((session_id, transaction_id, permit))
}

#[cfg(not(feature = "asr-transcribe-resident"))]
fn require_live_resident_ready(
    _state: &Arc<DaemonState>,
    _supervisor: &Arc<WorkerSupervisor>,
    config: &EngineConfig,
) -> Result<(), (IpcErrorCode, String)> {
    if config.transcribe_resident {
        Err((
            IpcErrorCode::DaemonUnavailable,
            "resident ASR support is not compiled".to_owned(),
        ))
    } else {
        Ok(())
    }
}

/// Outcome of one start attempt shared by explicit `StartSession` and the
/// starting arm of `ToggleSession`. Only content-free identity travels here.
enum StartSessionOutcome {
    Accepted {
        session_id: u128,
        transaction_id: String,
        pipeline_id: String,
    },
    Rejected {
        code: IpcErrorCode,
        detail: String,
    },
}

fn recognition_request_from_dto(
    settings: Option<&RecognitionSettingsDto>,
) -> Result<AsrRequest, (IpcErrorCode, String)> {
    let Some(settings) = settings else {
        return Ok(AsrRequest::default());
    };
    let parse_tag = |tag: &narrivy_ipc::RecognitionTagDto| {
        AsrTag::parse(&tag.as_str().replace('_', "-")).map_err(|error| {
            (
                IpcErrorCode::BadRequest,
                format!("invalid recognition settings: {}", error.code),
            )
        })
    };
    let spoken_language = match &settings.spoken_language {
        SpokenLanguageDto::Unspecified => SpokenLanguage::Unspecified,
        SpokenLanguageDto::AutoDetect => SpokenLanguage::AutoDetect,
        SpokenLanguageDto::Explicit { tag } => SpokenLanguage::Explicit(parse_tag(tag)?),
    };
    let context_bias = settings
        .context_hints
        .as_ref()
        .map(|hints| ContextBias::try_new(hints.as_slice().iter().cloned()))
        .transpose()
        .map_err(|error| {
            (
                IpcErrorCode::BadRequest,
                format!("invalid recognition settings: {}", error.code),
            )
        })?;
    let mut request = AsrRequest {
        spoken_language,
        output: WrittenOutput {
            language: settings
                .output_language
                .as_ref()
                .map(parse_tag)
                .transpose()?,
            script: settings.output_script.as_ref().map(parse_tag).transpose()?,
        },
        translation_target: settings
            .translation_target
            .as_ref()
            .map(parse_tag)
            .transpose()?,
        locale: settings.locale.as_ref().map(parse_tag).transpose()?,
        context_bias,
    };
    written_language_as_spoken(&mut request);
    Ok(request)
}

/// A start that names no language (the plain toggle, the tray's Start,
/// press and hold) is dictated in the user's current dictation language.
/// One that names any language, or asks for detection or translation,
/// keeps its own.
fn with_dictation_language(mut request: AsrRequest, current: Option<String>) -> AsrRequest {
    let names_none = matches!(request.spoken_language, SpokenLanguage::Unspecified)
        && request.output.language.is_none()
        && request.translation_target.is_none();
    if names_none && let Some(tag) = current.and_then(|tag| AsrTag::parse(&tag).ok()) {
        request.spoken_language = SpokenLanguage::Explicit(tag);
    }
    request
}

/// "Write in Urdu" without a translation is the same request as "Urdu is
/// spoken": a speech model writes in the language it transcribes, and the
/// engines take only the spoken language. So a written language with no
/// spoken language (or the same one) becomes the spoken language. A written
/// language that differs from an explicit spoken one stays, and is refused
/// by the capability check as before.
fn written_language_as_spoken(request: &mut AsrRequest) {
    if request.translation_target.is_some() {
        return;
    }
    let Some(written) = request.output.language.clone() else {
        return;
    };
    match &request.spoken_language {
        SpokenLanguage::Unspecified | SpokenLanguage::AutoDetect => {
            request.spoken_language = SpokenLanguage::Explicit(written);
            request.output.language = None;
        }
        SpokenLanguage::Explicit(spoken) if written.matches(spoken) => {
            request.output.language = None;
        }
        SpokenLanguage::Explicit(_) => {}
    }
}

fn validate_recognition_capabilities(
    request: &AsrRequest,
    capabilities: Option<&AsrCapabilities>,
) -> Result<(), (IpcErrorCode, String)> {
    if request.is_default() {
        return Ok(());
    }
    let capabilities = capabilities.ok_or_else(|| {
        (
            IpcErrorCode::BadRequest,
            "recognition settings are unavailable".to_owned(),
        )
    })?;
    capabilities.validate(request).map_err(|error| {
        (
            IpcErrorCode::BadRequest,
            format!("unsupported recognition settings: {}", error.code),
        )
    })
}

fn validate_recognition_admission(
    request: &AsrRequest,
    state: &Arc<DaemonState>,
) -> Result<(), (IpcErrorCode, String)> {
    #[cfg(feature = "asr-transcribe-resident")]
    {
        let resident = state.resident_asr();
        validate_recognition_capabilities(
            request,
            resident.as_ref().map(AsrEngine::capabilities).as_ref(),
        )
    }
    #[cfg(not(feature = "asr-transcribe-resident"))]
    {
        let _ = state;
        validate_recognition_capabilities(request, None)
    }
}

/// Explicit `StartSession`: resolve the admitted mode through the pure
/// selection rule and run the one strict start path.
///
/// The caller holds the toggle admission gate, so a concurrent activation
/// cannot observe "no session" after this path has registered one.
#[allow(clippy::too_many_arguments)]
fn start_session(
    pipeline_id: Option<String>,
    recognition: AsrRequest,
    state: &Arc<DaemonState>,
    journal: &Arc<Journal>,
    history: &Option<Arc<RetainedHistory>>,
    sink: &DeliverySink,
    supervisor: &Arc<WorkerSupervisor>,
    worker_start_delay: Option<Duration>,
    engine_config: &Arc<EngineConfig>,
    portal_service: &Option<Arc<PortalPermissionService>>,
    diagnostics: &Option<Arc<DaemonDiagnostics>>,
) -> StartSessionOutcome {
    // Resolve the admitted pipeline exactly once. Admission,
    // journal identity, worker correction and event identity all
    // consume this same value.
    let admitted = match resolve_pipeline_selection(engine_config, pipeline_id.as_deref()) {
        Ok(admitted) => admitted,
        Err((code, detail)) => return StartSessionOutcome::Rejected { code, detail },
    };
    start_admitted_session(
        admitted,
        recognition,
        state,
        journal,
        history,
        sink,
        supervisor,
        worker_start_delay,
        engine_config,
        portal_service,
        diagnostics,
    )
}

/// The one strict start path for an already-selected admission: check
/// start-only readiness, bind the admission identity, acquire the single
/// strict permit, persist the pending journal entry and spawn the owned
/// worker.
///
/// `StartSession` resolves its admission immediately before calling this
/// function; the starting arm of `ToggleSession` passes the value its
/// fail-closed preflight already resolved, so a toggle start still resolves
/// the admitted identity exactly once. The caller holds the toggle admission
/// gate, so a concurrent activation cannot observe "no session" after this
/// path has registered one.
#[allow(clippy::too_many_arguments)]
fn start_admitted_session(
    admitted: AdmittedPipeline,
    recognition: AsrRequest,
    state: &Arc<DaemonState>,
    journal: &Arc<Journal>,
    history: &Option<Arc<RetainedHistory>>,
    sink: &DeliverySink,
    supervisor: &Arc<WorkerSupervisor>,
    worker_start_delay: Option<Duration>,
    engine_config: &Arc<EngineConfig>,
    portal_service: &Option<Arc<PortalPermissionService>>,
    diagnostics: &Option<Arc<DaemonDiagnostics>>,
) -> StartSessionOutcome {
    // The pipeline's own model and device, loaded only when they differ from
    // what is loaded now.
    if admitted.lane() == PipelineLane::StrictCapture {
        ensure_pipeline_engine(state, supervisor, engine_config, admitted.id());
    }
    // Start-only readiness: a toggle that stops an active recording never
    // reaches this path, so unavailable assets cannot block a valid stop.
    if let Err((code, detail)) = require_pipeline_start_readiness(engine_config, &admitted) {
        return StartSessionOutcome::Rejected { code, detail };
    }
    let recognition =
        with_dictation_language(recognition, super::text_options::dictation_language());
    if let Err((code, detail)) = validate_recognition_admission(&recognition, state) {
        return StartSessionOutcome::Rejected { code, detail };
    }
    // Bind the delivery target exactly once here, on the admission
    // path and before any capture/activation/focus side effect.
    // The immutable value is carried into the owned worker; the
    // adapter never queries the target again. The current Start
    // command supplies no trustworthy pre-activation identity, so
    // this is the explicit typed-unavailable input and every
    // target-requiring rung stays skipped.
    #[cfg_attr(not(feature = "asr-transcribe-cpp"), allow(unused_variables))]
    let job_target_binding = admission_target_binding();
    // The retained History row is scoped to the admission-time target
    // application. This is read once, here, before the binding is consumed
    // by the delivery rung, and it is never logged: the binding's own `Debug`
    // impl redacts it, and an absent binding simply matches no privacy zone.
    let job_app_id = target_application_id(&job_target_binding);
    let (session_id, txid, strict_permit) =
        match admit_session_request(state, journal, &admitted, engine_config, supervisor) {
            Ok(admission) => admission,
            Err((code, detail)) => return StartSessionOutcome::Rejected { code, detail },
        };
    let slot_runtime = state
        .lock_sessions()
        .get(&session_id)
        .map(|slot| (slot.control.clone(), Arc::clone(&slot.timing)));
    let Some((slot_control, slot_timing)) = slot_runtime else {
        state.recovery_initialization_failed(session_id, &txid, journal, "session_slot_missing");
        state.lock_sessions().remove(&session_id);
        return StartSessionOutcome::Rejected {
            code: IpcErrorCode::Internal,
            detail: format!("session {session_id} slot vanished"),
        };
    };
    let job_state = Arc::clone(state);
    #[cfg_attr(not(feature = "asr-transcribe-cpp"), allow(unused_variables))]
    let job_supervisor = Arc::clone(supervisor);
    let job_journal = Arc::clone(journal);
    // The content-free diagnostics surface is optional for the same reason the
    // retained History store is: a composition that leaves it out keeps every
    // session working and only loses the support-report facts.
    let job_diagnostics = diagnostics.clone();
    // The optional retained History store is owned by the same composition
    // root as the journal and is carried into the owned worker, which is the
    // only place a finalized result becomes a retained History row.
    let job_history = history.clone();
    let job_sink = Arc::clone(sink);
    let job_asr_engine = engine_config.asr_engine;
    let job_audio_source = engine_config.audio_source.clone();
    #[cfg_attr(not(feature = "asr-transcribe-cpp"), allow(unused_variables))]
    let job_capture_seconds = engine_config.pipewire_capture_seconds;
    #[cfg_attr(not(feature = "asr-transcribe-cpp"), allow(unused_variables))]
    let job_model_path = engine_config.transcribe_model_path.clone();
    let job_delivery_backend = engine_config.delivery_backend;
    let job_force_delivery_failure = engine_config.force_delivery_failure;
    let job_mock_delivery_hold = engine_config.mock_delivery_hold;
    let job_inbox_dir = engine_config.inbox_dir.clone();
    // One admitted lane value for the whole owned worker. The lane
    // selects capture/delivery composition only; pipeline identity
    // and correction come from `job_admitted`.
    #[cfg_attr(not(feature = "asr-transcribe-cpp"), allow(unused_variables))]
    let job_lane = engine_config.strict_recovery_lane();
    // The real delivery boundary is decided by the same vector that
    // admits the request, not by a second independent resolution.
    #[cfg_attr(not(feature = "asr-transcribe-cpp"), allow(unused_variables))]
    let job_delivery_mode = engine_config.strict_recovery_delivery_mode();
    #[cfg_attr(not(feature = "asr-transcribe-cpp"), allow(unused_variables))]
    // The user's delivery method, when chosen, else the composition's.
    let job_clipboard_authorized = super::text_options::paste(engine_config.clipboard_authorized);
    #[cfg_attr(not(feature = "asr-transcribe-cpp"), allow(unused_variables))]
    let job_uinput_paste = job_clipboard_authorized && super::text_options::paste_through_uinput();
    // The routed lane submits through the daemon-owned portal
    // service only; it can never request or fabricate a grant.
    #[cfg_attr(not(feature = "asr-transcribe-cpp"), allow(unused_variables))]
    let job_portal_service = portal_service.clone();
    #[cfg_attr(not(feature = "asr-transcribe-cpp"), allow(unused_variables))]
    let job_resident_mode = engine_config.transcribe_resident;
    let job_txid = txid.clone();
    #[cfg_attr(not(feature = "asr-transcribe-cpp"), allow(unused_variables))]
    let job_control = slot_control.clone();
    #[cfg_attr(not(feature = "asr-transcribe-cpp"), allow(unused_variables))]
    let job_timing = slot_timing;
    // The owned worker derives both its pipeline identity and its
    // deterministic correction from this one admitted value.
    #[cfg_attr(not(feature = "asr-transcribe-cpp"), allow(unused_variables))]
    let job_admitted = admitted.clone();
    #[cfg_attr(not(feature = "asr-transcribe-cpp"), allow(unused_variables))]
    let job_recognition = recognition;
    // The caller keeps one lease until the spawn result is fully
    // handled. A successfully started worker owns the clone; a
    // failed spawn therefore cannot reopen strict admission before
    // synchronous journal and slot cleanup completes.
    let strict_permit_owner = strict_permit.map(Arc::new);
    let worker_strict_permit = strict_permit_owner.clone();
    let spawned = supervisor.spawn_with_cancellation(
        format!("session-{session_id}"),
        slot_control.cancellation().clone(),
        move |supervisor_cancel| {
        let _worker_cleanup = SessionWorkerCleanup::new(
            job_state.clone(),
            job_journal.clone(),
            session_id,
            job_txid.clone(),
            worker_strict_permit,
        );
        // One content-free diagnostics observation per owned worker, on every
        // exit path. The guard records nothing itself; the two terminal paths
        // record their typed outcome, and any other exit (an early return, a
        // panic) is recorded as `abandoned` so a failed attempt is never
        // silently absent from the reported health.
        let mut worker_diagnostics =
            SessionDiagnosticsGuard::new(job_diagnostics.clone());
        if let Some(delay) = worker_start_delay {
            let deadline = std::time::Instant::now() + delay;
            while std::time::Instant::now() < deadline {
                if supervisor_cancel.is_cancelled() {
                    job_state.recovery_cancelled_before_run(session_id, &job_txid, &job_journal);
                    worker_diagnostics.observe(SessionOutcome::Cancelled);
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        if job_lane.is_some() {
            #[cfg(feature = "asr-transcribe-cpp")]
            {
                let mut capture = match narrivy_platform_linux::recovery::PipeWireCapture::new(
                    job_capture_seconds,
                ) {
                    // The microphone without the computer's own sound when
                    // that is on and running, else the chosen microphone.
                    Ok(capture) => match super::echo::capture_target_for(Some(job_admitted.id())) {
                        Some(source) => capture.with_target(source),
                        None => capture,
                    },
                    Err(err) => {
                        job_state.recovery_initialization_failed(session_id, &job_txid, &job_journal, err.code);
                        return;
                    }
                };
                // A model that cannot stream still shows text while the
                // user speaks: the live preview decodes the recent audio
                // with the same engine (F3).
                #[cfg(feature = "asr-transcribe-resident")]
                let mut preview_engine: Option<ResidentWorkerEngine> = None;
                let mut engine = if job_resident_mode {
                    #[cfg(feature = "asr-transcribe-resident")]
                    {
                        let Some(engine) = job_state.resident_asr() else {
                            job_state.recovery_initialization_failed(session_id, &job_txid, &job_journal, "resident_worker_unavailable");
                            return;
                        };
                        if engine.capabilities().streaming {
                            StrictRecoveryAsr::Streaming(engine)
                        } else {
                            preview_engine = Some(engine.clone());
                            // The voice filter decides what the model hears:
                            // no speech, no text; silence cut; once trained,
                            // only the user's voice.
                            {
                                let takes_words = engine.capabilities().hotword_biasing;
                                StrictRecoveryAsr::Batch(Box::new(
                                    super::voice::VoiceFiltered::new(Box::new(engine), takes_words)
                                        .for_pipeline(job_admitted.id())
                                        .with_plain_mic(),
                                ))
                            }
                        }
                    }
                    #[cfg(not(feature = "asr-transcribe-resident"))]
                    {
                        job_state.recovery_initialization_failed(session_id, &job_txid, &job_journal, "resident_runtime_unavailable");
                        return;
                    }
                } else {
                    // Only the sidecar engine takes a model path: the
                    // installed resident engine has its model bound at
                    // startup, and installed mode forbids the path (F-C6).
                    let Some(model) = job_model_path.clone() else {
                        job_state.recovery_initialization_failed(session_id, &job_txid, &job_journal, "model_unavailable");
                        return;
                    };
                    match TranscribeCppEngine::with_model_path(model) {
                        Ok(engine) => StrictRecoveryAsr::Batch(Box::new(
                            super::voice::VoiceFiltered::new(Box::new(engine), false)
                                .for_pipeline(job_admitted.id())
                                .with_plain_mic(),
                        )),
                        Err(err) => {
                            job_state.recovery_initialization_failed(session_id, &job_txid, &job_journal, err.code);
                            return;
                        }
                    }
                };
                let Some(inbox_dir) = job_inbox_dir.clone() else {
                    job_state.recovery_initialization_failed(session_id, &job_txid, &job_journal, "inbox_unavailable");
                    return;
                };
                // The portable delivery-mode identity is the one the
                // admitting composition resolved; it can never be
                // re-derived from a lane id string here.
                let Some(delivery_mode) = job_delivery_mode else {
                    job_state.recovery_initialization_failed(session_id, &job_txid, &job_journal, "strict_recovery_lane_missing");
                    return;
                };
                let mut delivery: Box<dyn DeliveryPort> = match job_lane {
                    Some(StrictRecoveryLane::TargetRouted) => {
                        match compose_strict_routed_delivery(
                            inbox_dir,
                            job_target_binding,
                            job_portal_service.clone(),
                            job_clipboard_authorized,
                            job_uinput_paste,
                        ) {
                            Ok(delivery) => Box::new(delivery),
                            Err(err) => {
                                job_state.recovery_initialization_failed(session_id, &job_txid, &job_journal, err.code);
                                return;
                            }
                        }
                    }
                    Some(StrictRecoveryLane::WtypeThenInbox) => {
                        match narrivy_platform_linux::recovery::WtypeThenInboxDelivery::new(inbox_dir) {
                            Ok(delivery) => Box::new(delivery),
                            Err(err) => {
                                job_state.recovery_initialization_failed(session_id, &job_txid, &job_journal, err.code);
                                return;
                            }
                        }
                    }
                    Some(StrictRecoveryLane::InboxOnly) => {
                        Box::new(narrivy_platform_linux::recovery::InboxOnlyDelivery::new(inbox_dir))
                    }
                    None => {
                        job_state.recovery_initialization_failed(session_id, &job_txid, &job_journal, "strict_recovery_lane_missing");
                        return;
                    }
                };
                let policy = PolicyEngine::from_document(PolicyDocument::default());
                // The correction strategy is the one resolved from
                // the admitted pipeline before this owned work was
                // launched; both batch and streaming slices consume
                // this same composed port.
                let mut corrector = strict_composition_corrector(&job_admitted);
                let job_pipeline = job_admitted.id().to_owned();
                // The overlay shows the recognised text while it is cleaned
                // up, then the cleaned text in its place (D29 "invisible
                // wait"). Volatile partials, numbered after any from
                // recognition.
                #[cfg(feature = "asr-transcribe-resident")]
                {
                    let state = job_state.clone();
                    let txid = job_txid.clone();
                    let pipeline = job_pipeline.clone();
                    let mut seq = REFINE_PREVIEW_SEQ_BASE;
                    corrector = corrector.with_preview(move |text| {
                        if text.is_empty() || text.len() > 16_384 {
                            return;
                        }
                        seq += 1;
                        state.publish_partial(
                            session_id,
                            &txid,
                            &pipeline,
                            VolatilePartial {
                                seq,
                                text: text.to_owned(),
                            },
                        );
                    });
                }
                let state_for_events = job_state.clone();
                let tx_for_events = job_txid.clone();
                let journal_for_events = &job_journal;
                let mut observe = |to| {
                    // The journal holds the attempts before the session
                    // commits, so the result can ride on that event.
                    let delivery = if to == SessionState::Committed {
                        delivery_result(journal_for_events, &tx_for_events)
                    } else {
                        None
                    };
                    state_for_events.observe_recovery_state(
                        session_id,
                        &tx_for_events,
                        to,
                        delivery,
                    )
                };
                #[cfg(feature = "asr-transcribe-resident")]
                let state_for_partials = job_state.clone();
                #[cfg(feature = "asr-transcribe-resident")]
                let tx_for_partials = job_txid.clone();
                #[cfg(feature = "asr-transcribe-resident")]
                let pipeline_for_partials = job_pipeline.clone();
                #[cfg(feature = "asr-transcribe-resident")]
                let mut observe_partial = |partial| {
                    state_for_partials.publish_partial(
                        session_id,
                        &tx_for_partials,
                        &pipeline_for_partials,
                        partial,
                    );
                    Ok(())
                };
                #[cfg(feature = "asr-transcribe-resident")]
                let mut live_preview = preview_engine.and_then(|engine| {
                    let state = job_state.clone();
                    let txid = job_txid.clone();
                    let pipeline = job_admitted.id().to_owned();
                    super::live_preview::LivePreview::start(
                        {
                            let takes_words = engine.capabilities().hotword_biasing;
                            Box::new(
                                super::voice::VoiceFiltered::new(Box::new(engine), takes_words)
                                    .for_pipeline(&pipeline),
                            )
                        },
                        SessionId(session_id),
                        job_recognition.clone(),
                        job_control.cancellation().clone(),
                        move |partial| state.publish_partial(session_id, &txid, &pipeline, partial),
                        {
                            let state = job_state.clone();
                            move |level| state.publish_level(session_id, level)
                        },
                    )
                });
                #[cfg(not(feature = "asr-transcribe-resident"))]
                let mut live_preview: Option<NoPreview> = None;
                let terminal = match &mut engine {
                    StrictRecoveryAsr::Batch(engine) => {
                        let mut slice = RecoverySlice {
                            journal: &job_journal,
                            capture: &mut capture,
                            asr: &mut **engine,
                            policy: &policy,
                            corrector: &mut corrector,
                            delivery: &mut *delivery,
                            delivery_mode,
                        };
                        match live_preview.as_mut() {
                            Some(preview) => slice.run_with_preview(
                                SessionId(session_id),
                                job_txid.clone(),
                                job_pipeline,
                                &job_control,
                                &mut observe,
                                &job_recognition,
                                &*job_timing,
                                preview,
                            ),
                            None => slice.run_with_request_and_timing(
                                SessionId(session_id),
                                job_txid.clone(),
                                job_pipeline,
                                &job_control,
                                &mut observe,
                                &job_recognition,
                                &*job_timing,
                            ),
                        }
                    }
                    #[cfg(feature = "asr-transcribe-resident")]
                    StrictRecoveryAsr::Streaming(engine) => {
                        // T2.6: dictation never stops on silence. The session
                        // ends on Stop, cancel or the capture safety limit.
                        StreamingRecoverySlice {
                            journal: &job_journal,
                            capture: &mut capture,
                            asr: engine,
                            policy: &policy,
                            corrector: &mut corrector,
                            delivery: &mut *delivery,
                            delivery_mode,
                            endpointing: None,
                            partial_observer: Some(&mut observe_partial),
                        }
                        .run_with_request_and_timing(
                            SessionId(session_id),
                            job_txid.clone(),
                            job_pipeline,
                            &job_control,
                            &mut observe,
                            &job_recognition,
                            &*job_timing,
                        )
                    }
                };
                // The recovery journal holds the only copy of the in-flight
                // text. Now that the transaction is final, mirror it into the
                // optional retained History behind its own privacy gate.
                mirror_result_into_history(
                    &job_history,
                    &job_journal,
                    &job_txid,
                    job_app_id.as_deref(),
                    transcribe_duration_us(&job_timing),
                );
                // Recovered text has a bounded retention (F-M3).
                if let Some(dir) = job_inbox_dir.clone() {
                    prune_inbox(dir);
                }
                job_state.record_completed_timing(&job_timing);
                // Content-free terminal fact for the support-report surface.
                worker_diagnostics.observe(match &terminal {
                    Ok(terminal) => SessionOutcome::from_terminal(terminal),
                    Err(_) => SessionOutcome::Failed,
                });
                #[cfg(feature = "asr-transcribe-resident")]
                if job_resident_mode {
                    if let Some(engine) = job_state.resident_asr() {
                        let _ = schedule_resident_recovery(engine, &job_supervisor);
                    }
                }
                return;
            }
            #[cfg(not(feature = "asr-transcribe-cpp"))]
            {
                job_state.recovery_initialization_failed(session_id, &job_txid, &job_journal, "runtime_unavailable");
                return;
            }
        }
        let mut variants = TranscriptVariants::default();

        let audio: (u32, Vec<f32>) = match job_audio_source {
            AudioSourceKind::SineFixture => (SAMPLE_RATE_HZ, sine_fixture_samples()),
            // This is unreachable through `select_pipeline`, but
            // remains a defensive fail-closed boundary for an
            // in-process caller that bypasses normal admission.
            // Live PipeWire capture is composed only by the owned,
            // fileless strict recovery transaction above.
            AudioSourceKind::PipeWire => {
                job_state.recovery_initialization_failed(
                    session_id,
                    &job_txid,
                    &job_journal,
                    "non_strict_pipewire_disabled",
                );
                return;
            }
            AudioSourceKind::FileWav(path) => match parse_wav_mono(&path) {
                Ok(pair) => pair,
                Err(reason) => {
                    eprintln!("narrivyd: fixture load failed: {}", content_safe(&reason));
                    job_state.recovery_initialization_failed(
                        session_id,
                        &job_txid,
                        &job_journal,
                        "fixture_load_failed",
                    );
                    return;
                }
            },
        };

        #[cfg(feature = "asr-transcribe-cpp")]
        let engine: Box<dyn narrivy_asr_api::AsrEngine> = match job_asr_engine {
            AsrEngineKind::Mock => Box::new(MockAsr::new(super::policy::MOCK_TRANSCRIPT)),
            AsrEngineKind::TranscribeCpp => {
                let Some(model) = job_model_path.clone() else {
                    eprintln!("narrivyd: transcribe-cpp requires an explicit configured GGUF model");
                    job_state.recovery_initialization_failed(
                        session_id,
                        &job_txid,
                        &job_journal,
                        "model_unavailable",
                    );
                    return;
                };
                match TranscribeCppEngine::with_model_path(model) {
                    Ok(e) => Box::new(e),
                    Err(err) => {
                        eprintln!(
                            "narrivyd: transcribe-cpp init failed: {} ({})",
                            content_safe(err.code),
                            content_safe(&err.message)
                        );
                        job_state.recovery_initialization_failed(
                            session_id,
                            &job_txid,
                            &job_journal,
                            err.code,
                        );
                        return;
                    }
                }
            }
        };
        #[cfg(not(feature = "asr-transcribe-cpp"))]
        let engine: Box<dyn narrivy_asr_api::AsrEngine> = match job_asr_engine {
            AsrEngineKind::Mock => Box::new(MockAsr::new(super::policy::MOCK_TRANSCRIPT)),
            AsrEngineKind::TranscribeCpp => {
                eprintln!("narrivyd: transcribe-cpp lane not compiled in");
                job_state.recovery_initialization_failed(
                    session_id,
                    &job_txid,
                    &job_journal,
                    "runtime_unavailable",
                );
                return;
            }
        };

        let force_fail = job_force_delivery_failure;
        let backend_kind = job_delivery_backend;
        let inbox_dir = job_inbox_dir.clone();
        let _ = &inbox_dir;
        let hold_cancel = supervisor_cancel.clone();
        let delivery = move |text: &str| -> Result<(), CoreError> {
            // Controlled crash window (F-M18): the transaction is already
            // journaled as committed; a kill here must not lose the text.
            if let Some(hold) = job_mock_delivery_hold {
                let deadline = std::time::Instant::now() + hold;
                while std::time::Instant::now() < deadline && !hold_cancel.is_cancelled() {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            if force_fail {
                return Err(CoreError::Platform(narrivy_platform_api::PlatformError {
                    code: "forced_failure",
                    message: "NARRIVY_DELIVERY_FAIL=1".to_owned(),
                }));
            }
            match backend_kind {
                DeliveryBackendKind::NullSink => {
                    job_sink
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .push(text.to_owned());
                    Ok(())
                }
                DeliveryBackendKind::InboxOnly => Err(CoreError::Platform(
                    narrivy_platform_api::PlatformError {
                        code: "inbox_only_requires_recovery_mode",
                        message: "Inbox-only delivery is unavailable outside strict recovery mode".to_owned(),
                    },
                )),
                DeliveryBackendKind::WtypeThenInbox => Err(CoreError::Platform(
                    narrivy_platform_api::PlatformError {
                        code: "wtype_inbox_requires_recovery_mode",
                        message: "wtype-Inbox recovery is unavailable outside strict recovery mode".to_owned(),
                    },
                )),
                DeliveryBackendKind::RoutedThenInbox => Err(CoreError::Platform(
                    narrivy_platform_api::PlatformError {
                        code: "routed_inbox_requires_recovery_mode",
                        message: "target-aware routed delivery is composed only by the strict routed recovery lane".to_owned(),
                    },
                )),
            }
        };
        let request = SessionRequest {
            audio_samples: audio.1,
            sample_rate_hz: audio.0,
        };
        match job_state.run_session(
            session_id,
            &job_journal,
            engine,
            &delivery,
            request,
            supervisor_cancel.clone(),
            &mut variants,
        ) {
            Ok(outcome) => {
                eprintln!(
                    "narrivyd: session {session_id} finished outcome={outcome:?}"
                );
                // Content-free terminal fact for the support-report surface.
                worker_diagnostics.observe(SessionOutcome::from_delivery(outcome));
            }
            Err(err) => {
                // The foreign `Display` of an engine/adapter error is
                // interpolated into an ordinary log sink, so it is routed
                // through the diagnostics redaction facility first.
                eprintln!("narrivyd: session {session_id} failed: {}", content_safe(&err.to_string()));
                let cancelled = matches!(err, CoreError::Cancelled)
                    || (supervisor_cancel.is_cancelled()
                        && core_failure_code(&err) == "cancelled");
                worker_diagnostics.observe(if cancelled {
                    SessionOutcome::Cancelled
                } else {
                    SessionOutcome::Failed
                });
                if cancelled {
                    job_state.recovery_cancelled_before_run(
                        session_id,
                        &job_txid,
                        &job_journal,
                    );
                } else {
                    job_state.recovery_initialization_failed(
                        session_id,
                        &job_txid,
                        &job_journal,
                        core_failure_code(&err),
                    );
                }
            }
        }
        // One mirror point for the whole owned worker. A cancelled or failed
        // transaction retains no transcript, so the gate reports it as
        // not-retained rather than writing a row.
        mirror_result_into_history(
            &job_history,
            &job_journal,
            &job_txid,
            job_app_id.as_deref(),
            None,
        );
        },
    );
    match spawned {
        Ok(_handle) => {
            drop(strict_permit_owner);
            StartSessionOutcome::Accepted {
                session_id,
                transaction_id: txid.as_str().to_owned(),
                pipeline_id: admitted.id().to_owned(),
            }
        }
        Err(_err) => {
            state.recovery_spawn_failed(session_id, &txid, journal);
            drop(strict_permit_owner);
            StartSessionOutcome::Rejected {
                code: IpcErrorCode::Internal,
                detail: "worker spawn failed".to_owned(),
            }
        }
    }
}

/// When `chat_id` is sharing a screen live: copies of what the screens show
/// now are added to `pictures`, and what is shared (with what was said so
/// far) is returned, to go beside the question.
fn live_share(
    state: &Arc<DaemonState>,
    chat_id: &str,
    pictures: &mut Vec<String>,
) -> Option<String> {
    let meetings = super::meetings::shared(state).ok()?;
    let (recording, title, screen) = meetings.recording_now()?;
    let chat = super::assistant_library::chat(chat_id)?;
    let sharing = chat.live.as_deref() == Some(recording.as_str());
    let (screens, said) = meetings.live_view(&recording, 6 * 1024)?;
    // The computer's sound also carries the assistant's own spoken answers;
    // those are not what the user shares.
    let answers: Vec<&str> = chat
        .turns
        .iter()
        .filter(|t| t.role == "assistant")
        .map(|t| t.text.as_str())
        .collect();
    let said = without_own_voice(&said, &answers);
    if !sharing {
        // Something else is being recorded: every chat can read what has
        // been said so far, but sees no screen.
        return Some(recording_instructions(&title, &said));
    }
    let mut seen = 0;
    for screen in &screens {
        if pictures.len() >= 4 {
            break;
        }
        if let Some(id) = super::assistant_library::keep_picture(screen) {
            pictures.push(id);
            seen += 1;
        }
    }
    Some(live_instructions(screen.then_some(seen > 0), &said))
}

/// Transcript lines (`[m:ss] Speaker: text`) that are mostly words of one of
/// the assistant's `answers` are its own voice heard back: dropped.
fn without_own_voice(said: &str, answers: &[&str]) -> String {
    fn words(text: &str) -> Vec<String> {
        text.split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() > 2)
            .map(str::to_lowercase)
            .collect()
    }
    let answers: Vec<std::collections::HashSet<String>> = answers
        .iter()
        .map(|a| words(a).into_iter().collect())
        .collect();
    said.lines()
        .filter(|line| {
            let text = line.split_once("]").map_or(*line, |(_, rest)| rest);
            let text = text.split_once(':').map_or(text, |(_, rest)| rest);
            let heard = words(text);
            if heard.len() < 5 {
                return true;
            }
            !answers.iter().any(|answer| {
                let common = heard.iter().filter(|w| answer.contains(*w)).count();
                common * 10 >= heard.len() * 7
            })
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// What was said so far, as the model is told it.
fn heard_so_far(said: &str) -> String {
    if said.trim().is_empty() {
        "Nothing has been transcribed yet (speech appears about 20 to 30 seconds after it is \
said); if asked what was said, say that nothing has come through yet."
            .to_owned()
    } else {
        format!(
            "Live transcript of what has been said so far, written by the speech model from the \
sound (Room: the microphone, Call or Person: the computer's sound; newest last; the last 20 to 30 \
seconds are not in it yet):\n{said}\n(end of the live transcript)"
        )
    }
}

/// What the model is told while a recording it does not see is going on.
fn recording_instructions(title: &str, said: &str) -> String {
    format!(
        "[The user is recording \"{title}\" right now. You can hear it through its live \
transcript below; use it to answer questions about what is being said, and never say you cannot \
hear or have no transcript.]\n{}",
        heard_so_far(said)
    )
}

/// What the model is told during a live share: whether a picture of the
/// screen comes with the question (`None`: only the sound is shared), and
/// what was said so far.
fn live_instructions(picture: Option<bool>, said: &str) -> String {
    let mut text = String::from(match picture {
        Some(_) => {
            "[The user is sharing their screen and its sound with you live, right now (a video, \
a meeting, a page). You can see the screen and you can hear the sound through the live transcript \
below; never say you cannot hear, have no audio or have no transcript. "
        }
        None => {
            "[The user is sharing what they are listening to with you live, right now (a podcast, \
a call, a talk). You hear it through the live transcript below; never say you cannot hear or have \
no transcript. You cannot see their screen. "
        }
    });
    if picture == Some(true) {
        text.push_str(
            "The picture with this question is what the screen shows at this moment; answer \
questions about \"this\", \"here\" or \"now\" from it. ",
        );
    } else if picture == Some(false) {
        text.push_str("No picture of the screen has arrived yet; say so if asked what is on it. ");
    }
    text.push_str("Do not search the web for questions about what is shared.]\n");
    text.push_str(&heard_so_far(said));
    text
}

fn reply(request_id: narrivy_ipc::RequestId, payload: &ResponsePayload) -> Option<Envelope> {
    Envelope::new(MessageKind::Response, request_id)
        .with_payload(payload)
        .ok()
}

/// Report the optional retained History composition as observed, never as a
/// constant.
///
/// `history.optional=enabled` is emitted only when the daemon really owns an
/// open store, so a client can trust it as the gate for `GetHistory` and
/// `GetHistoryEntry`. The retention and bound facts are separate,
/// content-free capabilities: a client that wants to explain "nothing is
/// being recorded right now" reads `history.retention=off` rather than
/// inferring it from an empty list.
fn append_history_capabilities(capabilities: &mut Vec<String>, history: Option<&RetainedHistory>) {
    let Some(service) = history else {
        capabilities.push(HISTORY_NOT_COMPOSED_CAPABILITY.to_owned());
        return;
    };
    capabilities.push(HISTORY_ENABLED_CAPABILITY.to_owned());
    capabilities.push(format!("history.retention={}", service.retention_wire()));
    capabilities.push(format!("history.max_entries={MAX_HISTORY_ENTRIES}"));
}

fn rejected(
    request_id: narrivy_ipc::RequestId,
    code: IpcErrorCode,
    detail: String,
) -> Option<Envelope> {
    reply(request_id, &ResponsePayload::Rejected { code, detail })
}

/// Build the request handler mapping ipc commands onto daemon actions.
///
/// `history` is the optional retained History store owned by the composition
/// root, exactly like the recovery `journal`. `None` is the fail-closed
/// composition: History commands are rejected with a typed error and Status
/// reports `history.optional=disabled:not_composed`.
///
/// The daemon-owned content-safe diagnostics surface is composed here as well;
/// pass [`build_handler_with_portal`] to leave it out deliberately.
#[allow(clippy::too_many_arguments)]
pub fn build_handler(
    state: Arc<DaemonState>,
    journal: Arc<Journal>,
    history: Option<Arc<RetainedHistory>>,
    sink: DeliverySink,
    supervisor: Arc<WorkerSupervisor>,
    worker_start_delay: Option<Duration>,
    engine_config: Arc<EngineConfig>,
    compute: Option<Arc<ComputePlan>>,
) -> impl Fn(PeerCredentials, Envelope) -> Option<Envelope> + Send + Sync + 'static {
    build_handler_with_portal(
        state,
        journal,
        history,
        sink,
        supervisor,
        worker_start_delay,
        engine_config,
        None,
        Some(Arc::new(DaemonDiagnostics::new())),
        compute,
    )
}

/// Build the handler with an explicitly supplied daemon-owned portal service.
/// `None` retains the fail-closed test/legacy behavior. Production startup
/// supplies the daemon-owned service after a read-only eligibility probe.
///
/// `diagnostics` is the content-free support-report surface. `None` is the
/// deliberate fail-closed composition: Status reports
/// `diagnostics.support-report=unavailable:not_composed` and no client may
/// offer a support report backed by daemon facts.
#[allow(clippy::too_many_arguments)]
pub fn build_handler_with_portal(
    state: Arc<DaemonState>,
    journal: Arc<Journal>,
    history: Option<Arc<RetainedHistory>>,
    sink: DeliverySink,
    supervisor: Arc<WorkerSupervisor>,
    worker_start_delay: Option<Duration>,
    engine_config: Arc<EngineConfig>,
    portal_service: Option<Arc<PortalPermissionService>>,
    diagnostics: Option<Arc<DaemonDiagnostics>>,
    compute: Option<Arc<ComputePlan>>,
) -> impl Fn(PeerCredentials, Envelope) -> Option<Envelope> + Send + Sync + 'static {
    // One policy instance is shared by every connection for this daemon
    // generation. Reconnects and self-declared role changes therefore cannot
    // reset command budgets.
    let command_policy = IpcCommandPolicy::new(current_uid());
    move |credentials: PeerCredentials, envelope: Envelope| {
        if !credentials.is_same_user(current_uid()) {
            return None;
        }
        let request_id = envelope.request_id;
        let envelope_kind = envelope.kind.clone();
        let command: Command = match envelope.parse_payload() {
            Ok(command) => command,
            Err(err) => return rejected(request_id, err.code, err.message),
        };
        if command.kind() != envelope_kind {
            return rejected(
                request_id,
                IpcErrorCode::BadRequest,
                "command does not match envelope kind".to_owned(),
            );
        }
        if let Err(denial) = command_policy.authorize(credentials, &command) {
            return rejected(request_id, denial.code, denial.detail.to_owned());
        }
        match command {
            Command::GetStatus => {
                let sessions = state.lock_sessions();
                let active_session = sessions
                    .values()
                    .min_by_key(|slot| slot.machine.session_id().0)
                    .map(|slot| {
                        let timings = slot.timing.snapshot_us();
                        ActiveSessionDto {
                            session_id: slot.machine.session_id().0,
                            transaction_id: Some(slot.transaction_id.as_str().to_owned()),
                            pipeline_id: slot.pipeline_id.clone(),
                            state: state_wire(slot.machine.state()).to_owned(),
                            stage_timings_us: (!timings.is_empty())
                                .then_some(StageTimingsUs(timings)),
                        }
                    });
                #[cfg(feature = "asr-transcribe-resident")]
                let (resident_snapshot, model_provisioning) = state.resident_status_snapshot();
                #[cfg(feature = "asr-transcribe-resident")]
                let installed_model =
                    resident_snapshot.and_then(ResidentRuntimeSnapshot::installed_model_lifecycle);
                #[cfg(not(feature = "asr-transcribe-resident"))]
                let installed_model = None;
                #[cfg(not(feature = "asr-transcribe-resident"))]
                let model_provisioning = None;
                // The clipboard tokens follow the user's delivery method, the
                // same rule the next dictation composes its ladder with (D37).
                let status_config = EngineConfig {
                    clipboard_authorized: super::text_options::paste(
                        engine_config.clipboard_authorized,
                    ),
                    ..(*engine_config).clone()
                };
                #[cfg(feature = "asr-transcribe-resident")]
                let mut capabilities = capability_strings_with_resident(
                    &status_config,
                    resident_snapshot.map(ResidentRuntimeSnapshot::capability_snapshot),
                    compute.as_deref(),
                );
                #[cfg(not(feature = "asr-transcribe-resident"))]
                let mut capabilities = capability_strings(&status_config, compute.as_deref());
                append_history_capabilities(&mut capabilities, history.as_deref());
                // The three activation facts are separate and content-free:
                // the atomic command, the observed portal interface, and the
                // live listener. The listener stays unavailable until a real
                // owned listener is composed.
                append_activation_capabilities(&mut capabilities, state.activation_facts());
                if let Some(shortcuts) = state.shortcuts() {
                    capabilities.push(shortcuts.capability());
                }
                // The content-free support-report surface is advertised as
                // observed: a client may offer a Diagnostics action only when
                // `diagnostics.support-report=available` is present, and it
                // renders that report from exactly these tokens.
                append_diagnostics_capabilities(&mut capabilities, diagnostics.as_deref());
                #[cfg(feature = "asr-transcribe-resident")]
                append_resident_streaming_capabilities(
                    &mut capabilities,
                    &engine_config,
                    resident_snapshot,
                );
                // Only a resident composition can load or download a model.
                if engine_config.transcribe_resident {
                    capabilities.push(state.model_selection().capability());
                    capabilities.push(state.model_downloads.capability());
                    // The OpenVINO add-on, when this daemon can run it.
                    if super::openvino::worker_configured()
                        && let Some(models) =
                            super::store::models_dir(&engine_config, state.model_selection())
                    {
                        capabilities.push(super::openvino::runtime_state(&models).capability());
                    }
                }
                #[cfg(feature = "asr-transcribe-resident")]
                let resident_engine = state.resident_asr();
                #[cfg(feature = "asr-transcribe-resident")]
                if let Some(engine) = &resident_engine {
                    capabilities.extend(resident_device_capability(
                        engine.is_worker_ready(),
                        engine.compute(),
                    ));
                }
                #[cfg(feature = "asr-transcribe-resident")]
                let recognition_capabilities = resident_engine.map(|engine| engine.capabilities());
                #[cfg(not(feature = "asr-transcribe-resident"))]
                let recognition_capabilities: Option<AsrCapabilities> = None;
                append_recognition_request_capabilities(
                    &mut capabilities,
                    recognition_capabilities.as_ref(),
                );
                reply(
                    request_id,
                    &ResponsePayload::Status(StatusDto {
                        daemon_build: env!("CARGO_PKG_VERSION").to_owned(),
                        protocol_minor: PROTOCOL_MINOR,
                        active_session,
                        last_session_timings_us: state.last_session_timing(),
                        capabilities,
                        installed_model,
                        model_provisioning: model_provisioning.map(Box::new),
                        portal_keyboard: portal_service.as_ref().map(|service| service.snapshot()),
                    }),
                )
            }
            Command::StartSession {
                pipeline_id,
                recognition,
                ..
            } => {
                let recognition = match recognition_request_from_dto(recognition.as_ref()) {
                    Ok(request) => request,
                    Err((code, detail)) => return rejected(request_id, code, detail),
                };
                // Explicit Start takes the same gate as Toggle, so a toggle can
                // never race an explicit start into two sessions.
                let _toggle_gate = state.lock_toggle_admission();
                match start_session(
                    pipeline_id,
                    recognition,
                    &state,
                    &journal,
                    &history,
                    &sink,
                    &supervisor,
                    worker_start_delay,
                    &engine_config,
                    &portal_service,
                    &diagnostics,
                ) {
                    StartSessionOutcome::Accepted { .. } => {
                        reply(request_id, &ResponsePayload::Accepted)
                    }
                    StartSessionOutcome::Rejected { code, detail } => {
                        rejected(request_id, code, detail)
                    }
                }
            }
            Command::ToggleSession {
                pipeline_id,
                activation,
                recognition,
            } => {
                let recognition = match recognition_request_from_dto(recognition.as_ref()) {
                    Ok(request) => request,
                    Err((code, detail)) => return rejected(request_id, code, detail),
                };
                // Fail closed before the toggle decision and before any
                // session/control mutation, whether idle, recording or already
                // stopping: the supplied pipeline must be selectable by this
                // composition and the activation source must be one of the
                // closed set of Narrivy-owned surfaces. This preflight uses the
                // pure selection rule and deliberately never consults start
                // readiness, so a valid toggle can still stop an active
                // recording when models or capture helpers become unavailable.
                let admitted = match validate_toggle_request(
                    &engine_config,
                    pipeline_id.as_deref(),
                    &activation,
                ) {
                    Ok(admitted) => admitted,
                    Err((code, detail)) => return rejected(request_id, code, detail),
                };
                // One gate covers both the decision and the start path, so two
                // concurrent toggles linearize: the first either admits one
                // session or requests one graceful stop, and the second
                // observes that result instead of opening a second session or
                // losing the stop. The start path is the exact same strict
                // readiness/admission/start path as `StartSession`, including
                // the single strict permit; nothing here ever cancels.
                //
                // A start may first have to load the pipeline's model, which
                // takes seconds. That load runs before the gate: a toggle
                // pressed again meanwhile is refused as busy (the load holds
                // the strict permit) instead of waiting at the gate and then
                // stopping the new session before it hears anything.
                if admitted.lane() == PipelineLane::StrictCapture && state.no_sessions() {
                    ensure_pipeline_engine(&state, &supervisor, &engine_config, admitted.id());
                }
                let _toggle_gate = state.lock_toggle_admission();
                match state.toggle_decision() {
                    ToggleDecision::Start => match start_admitted_session(
                        admitted,
                        recognition,
                        &state,
                        &journal,
                        &history,
                        &sink,
                        &supervisor,
                        worker_start_delay,
                        &engine_config,
                        &portal_service,
                        &diagnostics,
                    ) {
                        StartSessionOutcome::Accepted {
                            session_id,
                            transaction_id,
                            pipeline_id,
                        } => reply(
                            request_id,
                            &ResponsePayload::Toggle(ToggleResultDto {
                                outcome: ToggleOutcomeDto::Started,
                                activation,
                                session_id: Some(session_id),
                                transaction_id: Some(transaction_id),
                                pipeline_id: Some(pipeline_id),
                            }),
                        ),
                        StartSessionOutcome::Rejected { code, detail } => {
                            rejected(request_id, code, detail)
                        }
                    },
                    ToggleDecision::FinalizationRequested {
                        session_id,
                        transaction_id,
                        pipeline_id,
                    } => reply(
                        request_id,
                        &ResponsePayload::Toggle(ToggleResultDto {
                            outcome: ToggleOutcomeDto::FinalizationRequested,
                            activation,
                            session_id: Some(session_id),
                            transaction_id: Some(transaction_id),
                            pipeline_id,
                        }),
                    ),
                    // Any other existing session state is already stopping or
                    // terminal: report the typed deterministic outcome without
                    // opening a second session.
                    ToggleDecision::AlreadyStopping {
                        session_id,
                        transaction_id,
                        pipeline_id,
                    } => reply(
                        request_id,
                        &ResponsePayload::Toggle(ToggleResultDto {
                            outcome: ToggleOutcomeDto::AlreadyStopping,
                            activation,
                            session_id: Some(session_id),
                            transaction_id: Some(transaction_id),
                            pipeline_id,
                        }),
                    ),
                }
            }
            Command::StopSession => {
                let active = state.request_active_finalization();
                if active == 0 {
                    rejected(
                        request_id,
                        IpcErrorCode::UnknownCommand,
                        "no active session to stop".to_owned(),
                    )
                } else {
                    reply(request_id, &ResponsePayload::Accepted)
                }
            }
            Command::CancelSession => {
                let active = state.cancel_active_sessions();
                if active == 0 {
                    rejected(
                        request_id,
                        IpcErrorCode::UnknownCommand,
                        "no active session to cancel".to_owned(),
                    )
                } else {
                    reply(request_id, &ResponsePayload::Accepted)
                }
            }
            Command::Subscribe | Command::Unsubscribe => {
                reply(request_id, &ResponsePayload::Accepted)
            }
            Command::ListPipelines => reply(
                request_id,
                &ResponsePayload::Pipelines(advertised_pipelines(
                    &engine_config,
                    strict_pipeline_live_ready(&state, &engine_config),
                    Some(state.model_selection()),
                )),
            ),
            Command::GetHistory => {
                let Some(service) = history.as_deref() else {
                    return rejected(
                        request_id,
                        IpcErrorCode::BadRequest,
                        HISTORY_NOT_COMPOSED_DETAIL.to_owned(),
                    );
                };
                match service.list(HISTORY_LIST_LIMIT) {
                    Ok(entries) => reply(
                        request_id,
                        &ResponsePayload::History(
                            entries
                                .iter()
                                .map(super::retained_history::listing_row)
                                .collect(),
                        ),
                    ),
                    // A storage failure is a typed rejection, never a
                    // fabricated empty list that would look like success.
                    Err(_) => rejected(
                        request_id,
                        IpcErrorCode::Internal,
                        HISTORY_UNAVAILABLE_DETAIL.to_owned(),
                    ),
                }
            }
            Command::GetHistoryEntry { txid } => {
                let Some(service) = history.as_deref() else {
                    return rejected(
                        request_id,
                        IpcErrorCode::BadRequest,
                        HISTORY_NOT_COMPOSED_DETAIL.to_owned(),
                    );
                };
                if valid_txid(&txid).is_err() {
                    return rejected(
                        request_id,
                        IpcErrorCode::BadRequest,
                        "invalid txid".to_owned(),
                    );
                }
                match service.entry(&txid) {
                    Ok(Some(entry)) => reply(
                        request_id,
                        &ResponsePayload::HistoryEntry(super::retained_history::entry_detail(
                            &entry,
                        )),
                    ),
                    Ok(None) => rejected(
                        request_id,
                        IpcErrorCode::UnknownCommand,
                        "unknown history transaction".to_owned(),
                    ),
                    Err(_) => rejected(
                        request_id,
                        IpcErrorCode::Internal,
                        HISTORY_UNAVAILABLE_DETAIL.to_owned(),
                    ),
                }
            }
            Command::ListModels => reply(
                request_id,
                &ResponsePayload::Models(super::models::model_listing(
                    &engine_config,
                    state.model_selection(),
                    &state.model_downloads,
                )),
            ),
            Command::DownloadModel { id } => {
                match download_model(&state, &supervisor, &engine_config, &id) {
                    Ok(()) => reply(request_id, &ResponsePayload::Accepted),
                    Err((code, detail)) => rejected(request_id, code, detail.to_owned()),
                }
            }
            Command::ListStore => reply(
                request_id,
                &ResponsePayload::Store(super::store::listing(
                    &engine_config,
                    state.model_selection(),
                    &state.model_downloads,
                    gpu_in_use(&state),
                )),
            ),
            Command::RemoveStoreItem { id } => {
                let outcome = super::store::models_dir(&engine_config, state.model_selection())
                    .ok_or("not_available")
                    .and_then(|models| super::store::remove(&models, state.model_selection(), &id));
                match outcome {
                    Ok(()) => reply(request_id, &ResponsePayload::Accepted),
                    Err("in_use") => rejected(
                        request_id,
                        IpcErrorCode::Busy,
                        "it is in use; switch to another model first".to_owned(),
                    ),
                    Err(code) => rejected(request_id, IpcErrorCode::BadRequest, code.to_owned()),
                }
            }
            Command::AssistantSend {
                turn_id,
                messages,
                chat_id,
                speak,
                pictures,
            } => {
                let sent = super::assistant::shared(&state).and_then(|assistant| match chat_id {
                    Some(chat_id) => {
                        let question = messages
                            .iter()
                            .rev()
                            .find(|m| m.role == "user")
                            .map(|m| m.content.as_str())
                            .filter(|q| !q.trim().is_empty() || !pictures.is_empty())
                            .ok_or("assistant_question_missing")?;
                        // A chat sharing a screen live sees it now: the
                        // newest pictures go with the question, and what
                        // was said so far with the instructions.
                        let mut pictures = pictures;
                        let live = live_share(&state, &chat_id, &mut pictures);
                        // A question in a chat with tools, typed or spoken,
                        // runs in rounds, each call approved by the user.
                        if super::assistant_tools::chat_has_tools(&chat_id) {
                            super::assistant_tools::answer(
                                assistant,
                                Arc::clone(&state),
                                turn_id,
                                chat_id,
                                question.to_owned(),
                                pictures,
                                speak,
                                live,
                            )
                        } else {
                            assistant.send_in_chat(
                                turn_id,
                                chat_id,
                                question,
                                &pictures,
                                speak,
                                live.as_deref(),
                            )
                        }
                    }
                    None if speak || !pictures.is_empty() => Err("assistant_chat_missing"),
                    None => assistant.send(turn_id, &messages),
                });
                match sent {
                    Ok(()) => reply(request_id, &ResponsePayload::Accepted),
                    Err(code) => rejected(request_id, IpcErrorCode::BadRequest, code.to_owned()),
                }
            }
            Command::CheckWriting {
                text,
                dialect,
                rules,
                words,
            } => match super::writing::check(&text, dialect, rules, words) {
                Ok(findings) => reply(request_id, &ResponsePayload::WritingFindings(findings)),
                Err(code) => rejected(request_id, IpcErrorCode::BadRequest, code.to_owned()),
            },
            Command::MeetingStart {
                title,
                watch,
                share,
                microphone,
                computer_sound,
            } => {
                let start = super::meetings::Start {
                    watch,
                    share,
                    microphone,
                    computer_sound,
                };
                match super::meetings::shared(&state).and_then(|m| m.start_with(title, start)) {
                    Ok(meeting) => reply(request_id, &ResponsePayload::Meeting(meeting)),
                    Err(code) => rejected(request_id, IpcErrorCode::BadRequest, code.to_owned()),
                }
            }
            Command::GetMeetingOptions => {
                let meetings = super::meetings::shared(&state).ok();
                reply(
                    request_id,
                    &ResponsePayload::MeetingOptions(super::meetings::options_dto(
                        meetings.as_deref(),
                    )),
                )
            }
            Command::AssistantToolDecision {
                call_id,
                allow,
                always,
            } => match super::assistant_tools::decide(&call_id, allow, always) {
                Ok(()) => reply(request_id, &ResponsePayload::Accepted),
                Err(code) => rejected(request_id, IpcErrorCode::BadRequest, code.to_owned()),
            },
            Command::MeetingSources {
                id,
                screen,
                sound,
                microphone,
            } => {
                match super::meetings::shared(&state)
                    .and_then(|m| m.sources(&id, screen, sound, microphone))
                {
                    Ok(meeting) => reply(request_id, &ResponsePayload::Meeting(meeting)),
                    Err(code) => rejected(request_id, IpcErrorCode::BadRequest, code.to_owned()),
                }
            }
            Command::MeetingControl { id, action } => {
                match super::meetings::shared(&state).and_then(|m| m.control(&id, &action)) {
                    Ok(()) => reply(request_id, &ResponsePayload::Accepted),
                    Err(code) => rejected(request_id, IpcErrorCode::BadRequest, code.to_owned()),
                }
            }
            Command::GetMeetingSlide { id, file } => {
                match super::meetings::shared(&state).and_then(|m| m.slide(&id, &file)) {
                    Ok(png) => reply(request_id, &ResponsePayload::MeetingSlide(png)),
                    Err(code) => rejected(request_id, IpcErrorCode::BadRequest, code.to_owned()),
                }
            }
            Command::SetMeetingOptions {
                auto_notes,
                microphone,
                speaker,
                call_audio,
                room_audio,
                screen,
                process_on_stop,
            } => {
                let saved = auto_notes
                    .map_or(Ok(()), super::meetings::set_auto_notes)
                    .and_then(|()| screen.map_or(Ok(()), super::meetings::set_screen))
                    .and_then(|()| {
                        process_on_stop.map_or(Ok(()), super::meetings::set_process_on_stop)
                    })
                    .and_then(|()| {
                        if microphone.is_some()
                            || speaker.is_some()
                            || call_audio.is_some()
                            || room_audio.is_some()
                        {
                            super::meetings::set_devices(
                                microphone, speaker, call_audio, room_audio,
                            )
                        } else {
                            Ok(())
                        }
                    });
                match saved {
                    Ok(()) => {
                        let meetings = super::meetings::shared(&state).ok();
                        reply(
                            request_id,
                            &ResponsePayload::MeetingOptions(super::meetings::options_dto(
                                meetings.as_deref(),
                            )),
                        )
                    }
                    Err(code @ ("meeting_device_invalid" | "meeting_nothing_to_record")) => {
                        rejected(request_id, IpcErrorCode::BadRequest, code.to_owned())
                    }
                    Err(code) => rejected(request_id, IpcErrorCode::Internal, code.to_owned()),
                }
            }
            Command::ImportMeeting { path, title } => {
                match super::meetings::shared(&state).and_then(|m| m.import(&path, title)) {
                    Ok(meeting) => reply(request_id, &ResponsePayload::Meeting(meeting)),
                    Err(code) => rejected(request_id, IpcErrorCode::BadRequest, code.to_owned()),
                }
            }
            Command::MeetingStop => match super::meetings::shared(&state).and_then(|m| m.stop()) {
                Ok(()) => reply(request_id, &ResponsePayload::Accepted),
                Err(code) => rejected(request_id, IpcErrorCode::BadRequest, code.to_owned()),
            },
            Command::ListMeetings => match super::meetings::shared(&state) {
                Ok(meetings) => reply(request_id, &ResponsePayload::Meetings(meetings.list())),
                Err(code) => rejected(request_id, IpcErrorCode::Internal, code.to_owned()),
            },
            Command::GetMeeting { id } => {
                match super::meetings::shared(&state).and_then(|m| m.get(&id)) {
                    Ok(detail) => reply(request_id, &ResponsePayload::MeetingDetail(detail)),
                    Err(code) => rejected(request_id, IpcErrorCode::BadRequest, code.to_owned()),
                }
            }
            Command::ProcessMeeting {
                id,
                model,
                room_speakers,
                call_speakers,
            } => match super::meetings::shared(&state)
                .and_then(|m| m.process(&id, model, room_speakers, call_speakers))
            {
                Ok(()) => reply(request_id, &ResponsePayload::Accepted),
                Err(code) => rejected(request_id, IpcErrorCode::BadRequest, code.to_owned()),
            },
            Command::RenameMeetingSpeaker { id, speaker, name } => {
                match super::meetings::shared(&state).and_then(|m| m.rename(&id, &speaker, &name)) {
                    Ok(()) => reply(request_id, &ResponsePayload::Accepted),
                    Err(code) => rejected(request_id, IpcErrorCode::BadRequest, code.to_owned()),
                }
            }
            Command::SummarizeMeeting {
                id,
                turn_id,
                kind,
                instruction,
            } => match super::assistant::shared(&state).and_then(|assistant| {
                super::meetings::shared(&state)
                    .and_then(|m| m.summarize(assistant, &id, turn_id, &kind, instruction))
            }) {
                Ok(()) => reply(request_id, &ResponsePayload::Accepted),
                Err(code) => rejected(request_id, IpcErrorCode::BadRequest, code.to_owned()),
            },
            Command::GetAudioOptions => reply(
                request_id,
                &ResponsePayload::AudioOptions(super::echo::options()),
            ),
            Command::SetAudioOptions {
                echo_cancel,
                only_my_voice,
                noise_suppression,
                microphone,
                sounds,
                voice_strictness,
            } => {
                let outcome = echo_cancel
                    .map_or(Ok(()), super::echo::set_enabled)
                    .and_then(|()| only_my_voice.map_or(Ok(()), super::voice::set_only_my_voice))
                    .and_then(|()| {
                        noise_suppression
                            .as_deref()
                            .map_or(Ok(()), super::echo::set_noise)
                    })
                    .and_then(|()| {
                        microphone
                            .as_deref()
                            .map_or(Ok(()), |name| super::echo::set_microphone(Some(name)))
                    })
                    .and_then(|()| sounds.map_or(Ok(()), super::sounds::set_enabled))
                    .and_then(|()| {
                        voice_strictness
                            .as_deref()
                            .map_or(Ok(()), super::voice::set_strictness)
                    });
                match outcome {
                    Ok(()) => reply(
                        request_id,
                        &ResponsePayload::AudioOptions(super::echo::options()),
                    ),
                    Err(code) => rejected(request_id, IpcErrorCode::Internal, code.to_owned()),
                }
            }
            Command::GetHoldKey => reply(request_id, &ResponsePayload::HoldKey(super::hold::dto())),
            Command::SetPipelineVoice {
                pipeline_id,
                echo_cancel,
                only_my_voice,
            } => match super::echo::set_pipeline_voice(
                &pipeline_id,
                super::echo::PipelineVoice {
                    echo_cancel,
                    only_my_voice,
                },
            ) {
                Ok(()) => reply(
                    request_id,
                    &ResponsePayload::AudioOptions(super::echo::options()),
                ),
                Err(code) => rejected(request_id, IpcErrorCode::BadRequest, code.to_owned()),
            },
            Command::GetTextOptions => reply(
                request_id,
                &ResponsePayload::TextOptions(super::text_options::dto(
                    engine_config.clipboard_authorized,
                )),
            ),
            Command::SetTextOptions {
                numbers_as_digits,
                delivery_method,
                cleanup_idle_minutes,
                spoken_breaks,
                dictation_language,
                dictation_languages,
            } => {
                let outcome = numbers_as_digits
                    .map_or(Ok(()), super::text_options::set_numbers_as_digits)
                    .and_then(|()| {
                        delivery_method
                            .as_deref()
                            .map_or(Ok(()), super::text_options::set_delivery)
                    })
                    .and_then(|()| {
                        cleanup_idle_minutes
                            .map_or(Ok(()), super::text_options::set_cleanup_idle_minutes)
                    })
                    .and_then(|()| {
                        spoken_breaks.map_or(Ok(()), super::text_options::set_spoken_breaks)
                    })
                    .and_then(|()| {
                        dictation_languages
                            .as_deref()
                            .map_or(Ok(()), super::text_options::set_dictation_languages)
                    })
                    .and_then(|()| {
                        dictation_language
                            .as_deref()
                            .map_or(Ok(()), super::text_options::set_dictation_language)
                    });
                match outcome {
                    Ok(()) => reply(
                        request_id,
                        &ResponsePayload::TextOptions(super::text_options::dto(
                            engine_config.clipboard_authorized,
                        )),
                    ),
                    Err(code) => rejected(request_id, IpcErrorCode::BadRequest, code.to_owned()),
                }
            }
            Command::SetHoldKey { key } => match super::hold::set(key) {
                Ok(()) => reply(request_id, &ResponsePayload::HoldKey(super::hold::dto())),
                Err(code) => rejected(request_id, IpcErrorCode::BadRequest, code.to_owned()),
            },
            Command::GetVocabulary => reply(
                request_id,
                &ResponsePayload::Vocabulary(super::vocabulary::to_dto(&super::vocabulary::load())),
            ),
            Command::SetVocabulary { vocabulary } => {
                match super::vocabulary::save(super::vocabulary::from_dto(vocabulary)) {
                    Ok(saved) => reply(
                        request_id,
                        &ResponsePayload::Vocabulary(super::vocabulary::to_dto(&saved)),
                    ),
                    Err(code) => rejected(request_id, IpcErrorCode::BadRequest, code.to_owned()),
                }
            }
            Command::TrainVoice => match super::voice::train() {
                Ok(()) => reply(
                    request_id,
                    &ResponsePayload::AudioOptions(super::echo::options()),
                ),
                Err(code) => rejected(request_id, IpcErrorCode::BadRequest, code.to_owned()),
            },
            Command::ForgetVoice => match super::voice::forget() {
                Ok(()) => reply(
                    request_id,
                    &ResponsePayload::AudioOptions(super::echo::options()),
                ),
                Err(code) => rejected(request_id, IpcErrorCode::Internal, code.to_owned()),
            },
            Command::DeleteMeeting { id } => {
                match super::meetings::shared(&state).and_then(|m| m.delete(&id)) {
                    Ok(()) => reply(request_id, &ResponsePayload::Accepted),
                    Err(code) => rejected(request_id, IpcErrorCode::BadRequest, code.to_owned()),
                }
            }
            Command::ListWritingRules => match super::writing::rules() {
                Ok(rules) => reply(request_id, &ResponsePayload::WritingRules(rules)),
                Err(code) => rejected(request_id, IpcErrorCode::Internal, code.to_owned()),
            },
            Command::WritingAction {
                turn_id,
                action,
                text,
                instruction,
            } => {
                let outcome = super::writing::Action::parse(&action)
                    .ok_or("unknown_writing_action")
                    .and_then(|action| {
                        super::assistant::shared(&state).and_then(|assistant| {
                            assistant.send_writing(turn_id, action, text, instruction.as_deref())
                        })
                    });
                match outcome {
                    Ok(()) => reply(request_id, &ResponsePayload::Accepted),
                    Err(code) => rejected(request_id, IpcErrorCode::BadRequest, code.to_owned()),
                }
            }
            Command::AssistantLibrary {
                op,
                kind,
                id,
                value,
            } => {
                let answer = if kind == "voice" && op == "get" {
                    Ok(super::voice_mode::settings_json())
                } else if kind == "web" {
                    match op.as_str() {
                        "get" => Ok(super::assistant_builtin::web_settings_json()),
                        "put" => value
                            .as_deref()
                            .ok_or("assistant_library_bad_value")
                            .and_then(super::assistant_builtin::set_web_settings),
                        _ => Err("assistant_library_bad_request"),
                    }
                } else if kind == "model" {
                    // The assistant's model: its state, auto-unload, load now.
                    match op.as_str() {
                        "get" => Ok(super::assistant::model_json()),
                        // `autoUnload` and/or `folder` (a string, or null
                        // for the home folder).
                        "put" => value
                            .as_deref()
                            .and_then(|v| serde_json::from_str::<serde_json::Value>(v).ok())
                            .filter(|v| v.get("autoUnload").is_some() || v.get("folder").is_some())
                            .ok_or("assistant_library_bad_value")
                            .and_then(|v| {
                                if let Some(on) = v.get("autoUnload") {
                                    super::assistant::set_auto_unload(
                                        on.as_bool().ok_or("assistant_library_bad_value")?,
                                    )?;
                                }
                                if let Some(folder) = v.get("folder") {
                                    super::assistant::set_default_folder(folder.as_str())?;
                                }
                                Ok(())
                            })
                            .map(|()| super::assistant::model_json()),
                        "run" => super::assistant::shared(&state).map(|assistant| {
                            assistant.load();
                            super::assistant::model_json()
                        }),
                        _ => Err("assistant_library_bad_request"),
                    }
                } else {
                    super::assistant_library::handle(
                        &op,
                        &kind,
                        id.as_deref(),
                        value.as_deref(),
                        &state,
                    )
                };
                match answer {
                    Ok(json) => reply(request_id, &ResponsePayload::AssistantLibrary(json)),
                    Err(code) => rejected(request_id, IpcErrorCode::BadRequest, code.to_owned()),
                }
            }
            Command::VoiceMode {
                action,
                settings,
                down,
            } => match super::voice_mode::handle(&action, settings, down, &state) {
                Ok(()) => reply(request_id, &ResponsePayload::Accepted),
                Err(code) => rejected(request_id, IpcErrorCode::BadRequest, code.to_owned()),
            },
            Command::AssistantUnload => {
                if let Ok(assistant) = super::assistant::shared(&state) {
                    assistant.unload();
                }
                reply(request_id, &ResponsePayload::Accepted)
            }
            Command::ListLanguageModels => reply(
                request_id,
                &ResponsePayload::LanguageModels(super::language_models::list()),
            ),
            Command::SelectLanguageModel { role, id } => {
                let outcome = super::language_models::Role::parse(&role)
                    .ok_or("unknown_role")
                    .and_then(|parsed| {
                        let dir = super::language_models::select(parsed, id.as_deref())?;
                        match parsed {
                            super::language_models::Role::CleanUp => {
                                super::refine::reconfigure(dir);
                                Ok(())
                            }
                            super::language_models::Role::Assistant => {
                                super::assistant::reconfigure(dir, &state)
                            }
                        }
                    });
                match outcome {
                    Ok(()) => reply(request_id, &ResponsePayload::Accepted),
                    Err(code) => rejected(request_id, IpcErrorCode::BadRequest, code.to_owned()),
                }
            }
            Command::AssistantCancel { turn_id } => {
                if let Ok(assistant) = super::assistant::shared(&state) {
                    assistant.cancel(&turn_id);
                }
                reply(request_id, &ResponsePayload::Accepted)
            }
            Command::CancelModelDownload { id } => {
                if state.model_downloads.cancel(&id) {
                    reply(request_id, &ResponsePayload::Accepted)
                } else {
                    rejected(
                        request_id,
                        IpcErrorCode::BadRequest,
                        "that model is not downloading".to_owned(),
                    )
                }
            }
            Command::SelectModel { id } => {
                match select_model(&state, &supervisor, &engine_config, &id) {
                    Ok(()) => reply(request_id, &ResponsePayload::Accepted),
                    Err((code, detail)) => rejected(request_id, code, detail.to_owned()),
                }
            }
            Command::SetPipelineEngine {
                pipeline_id,
                model_id,
                compute,
            } => {
                // The same gate as Start and Toggle: a choice is never
                // applied halfway through another request's start.
                let _toggle_gate = state.lock_toggle_admission();
                match set_pipeline_engine(
                    &state,
                    &supervisor,
                    &engine_config,
                    &pipeline_id,
                    model_id,
                    &compute,
                ) {
                    Ok(()) => reply(request_id, &ResponsePayload::Accepted),
                    Err((code, detail)) => rejected(request_id, code, detail),
                }
            }
            Command::ListShortcuts => match state.shortcuts() {
                Some(service) => reply(request_id, &ResponsePayload::Shortcuts(service.list())),
                None => rejected(
                    request_id,
                    IpcErrorCode::BadRequest,
                    super::shortcuts::SHORTCUTS_NOT_COMPOSED_DETAIL.to_owned(),
                ),
            },
            Command::SetShortcut { binding, replace } => match state.shortcuts() {
                Some(service) => reply(
                    request_id,
                    &ResponsePayload::ShortcutSet(service.set(&binding, replace)),
                ),
                None => rejected(
                    request_id,
                    IpcErrorCode::BadRequest,
                    super::shortcuts::SHORTCUTS_NOT_COMPOSED_DETAIL.to_owned(),
                ),
            },
            Command::RemoveShortcut { modifiers, key } => {
                use super::shortcuts::RemoveError;
                let Some(service) = state.shortcuts() else {
                    return rejected(
                        request_id,
                        IpcErrorCode::BadRequest,
                        super::shortcuts::SHORTCUTS_NOT_COMPOSED_DETAIL.to_owned(),
                    );
                };
                match service.remove(&modifiers, &key) {
                    Ok(true) => reply(request_id, &ResponsePayload::Accepted),
                    Ok(false) => rejected(
                        request_id,
                        IpcErrorCode::UnknownCommand,
                        "no such Narrivy shortcut".to_owned(),
                    ),
                    Err(RemoveError::Invalid) => rejected(
                        request_id,
                        IpcErrorCode::BadRequest,
                        "invalid key combination".to_owned(),
                    ),
                    Err(RemoveError::Unsupported) => rejected(
                        request_id,
                        IpcErrorCode::BadRequest,
                        "shortcuts are not managed on this desktop".to_owned(),
                    ),
                    Err(RemoveError::Store) => rejected(
                        request_id,
                        IpcErrorCode::Internal,
                        "shortcut store unavailable".to_owned(),
                    ),
                }
            }
            Command::SetHistoryRetention { retention } => {
                let Some(service) = history.as_deref() else {
                    return rejected(
                        request_id,
                        IpcErrorCode::BadRequest,
                        HISTORY_NOT_COMPOSED_DETAIL.to_owned(),
                    );
                };
                let Ok(policy) = retention.parse::<narrivy_history::RetentionPolicy>() else {
                    return rejected(
                        request_id,
                        IpcErrorCode::BadRequest,
                        "history_retention_unknown".to_owned(),
                    );
                };
                match service.set_retention(policy) {
                    Ok(_) => {
                        if let Ok(dir) = narrivy_platform_linux::instance::state_dir() {
                            let _ = super::retained_history::save_retention_preference(
                                &dir,
                                service.retention_wire(),
                            );
                        }
                        reply(request_id, &ResponsePayload::Accepted)
                    }
                    Err(_) => rejected(
                        request_id,
                        IpcErrorCode::Internal,
                        HISTORY_UNAVAILABLE_DETAIL.to_owned(),
                    ),
                }
            }
            Command::ClearHistory => {
                let Some(service) = history.as_deref() else {
                    return rejected(
                        request_id,
                        IpcErrorCode::BadRequest,
                        HISTORY_NOT_COMPOSED_DETAIL.to_owned(),
                    );
                };
                match service.clear() {
                    Ok(_) => reply(request_id, &ResponsePayload::Accepted),
                    Err(_) => rejected(
                        request_id,
                        IpcErrorCode::Internal,
                        HISTORY_UNAVAILABLE_DETAIL.to_owned(),
                    ),
                }
            }
            Command::DeleteHistoryEntry { txid } => {
                let Some(service) = history.as_deref() else {
                    return rejected(
                        request_id,
                        IpcErrorCode::BadRequest,
                        HISTORY_NOT_COMPOSED_DETAIL.to_owned(),
                    );
                };
                if valid_txid(&txid).is_err() {
                    return rejected(
                        request_id,
                        IpcErrorCode::BadRequest,
                        "invalid txid".to_owned(),
                    );
                }
                match service.delete(&txid) {
                    Ok(true) => reply(request_id, &ResponsePayload::Accepted),
                    Ok(false) => rejected(
                        request_id,
                        IpcErrorCode::UnknownCommand,
                        "unknown history transaction".to_owned(),
                    ),
                    Err(_) => rejected(
                        request_id,
                        IpcErrorCode::Internal,
                        HISTORY_UNAVAILABLE_DETAIL.to_owned(),
                    ),
                }
            }
            Command::CopyHistoryEntry { txid } => {
                let Some(service) = history.as_deref() else {
                    return rejected(
                        request_id,
                        IpcErrorCode::BadRequest,
                        HISTORY_NOT_COMPOSED_DETAIL.to_owned(),
                    );
                };
                if valid_txid(&txid).is_err() {
                    return rejected(
                        request_id,
                        IpcErrorCode::BadRequest,
                        "invalid txid".to_owned(),
                    );
                }
                let detail = match service.entry(&txid) {
                    Ok(Some(entry)) => super::retained_history::entry_detail(&entry),
                    Ok(None) => {
                        return rejected(
                            request_id,
                            IpcErrorCode::UnknownCommand,
                            "unknown history transaction".to_owned(),
                        );
                    }
                    Err(_) => {
                        return rejected(
                            request_id,
                            IpcErrorCode::Internal,
                            HISTORY_UNAVAILABLE_DETAIL.to_owned(),
                        );
                    }
                };
                // What was delivered, else the best text there is.
                let Some(text) = detail
                    .delivered
                    .or(detail.corrected)
                    .or(detail.raw)
                    .filter(|text| !text.is_empty())
                else {
                    return rejected(
                        request_id,
                        IpcErrorCode::BadRequest,
                        "history entry has no text".to_owned(),
                    );
                };
                match copy_to_clipboard(&text) {
                    Ok(()) => reply(request_id, &ResponsePayload::Accepted),
                    Err(code) => rejected(request_id, IpcErrorCode::Internal, code.to_owned()),
                }
            }
            Command::GetRecoveryEntry { txid } => match recovery_entry_detail(&journal, &txid) {
                Ok(detail) => reply(request_id, &ResponsePayload::RecoveryEntry(detail)),
                Err(err) => rejected(request_id, err.code, err.message),
            },
            Command::RunCapabilityCheck { check_id } => {
                if check_id.len() > 32
                    || !check_id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                {
                    return rejected(
                        request_id,
                        IpcErrorCode::BadRequest,
                        "invalid capability check id shape".to_owned(),
                    );
                }
                reply(
                    request_id,
                    &ResponsePayload::CapabilityCheck(run_capability_check(&check_id)),
                )
            }
            Command::SelectPipeline { pipeline_id } => {
                match select_pipeline(&engine_config, Some(&pipeline_id)) {
                    Ok(admitted) if admitted.lane() == PipelineLane::StrictCapture => {
                        match require_live_resident_ready(&state, &supervisor, &engine_config) {
                            Ok(()) => reply(request_id, &ResponsePayload::Accepted),
                            Err((code, detail)) => rejected(request_id, code, detail),
                        }
                    }
                    Ok(_) => reply(request_id, &ResponsePayload::Accepted),
                    Err((code, detail)) => rejected(request_id, code, detail),
                }
            }
            Command::RequestPortalKeyboardPermission {
                operation_id,
                parent_window,
            } => {
                let Some(service) = portal_service.as_ref() else {
                    return rejected(
                        request_id,
                        IpcErrorCode::DaemonUnavailable,
                        "portal keyboard permission service is not composed".to_owned(),
                    );
                };
                let parent = match PortalParentWindow::parse(&parent_window) {
                    Ok(parent) => parent,
                    Err(_) => {
                        return rejected(
                            request_id,
                            IpcErrorCode::BadRequest,
                            "invalid portal parent window".to_owned(),
                        );
                    }
                };
                match service.request(
                    operation_id,
                    parent,
                    Instant::now() + PORTAL_PERMISSION_REQUEST_TIMEOUT,
                ) {
                    Ok(status) => reply(
                        request_id,
                        &ResponsePayload::PortalKeyboardPermission(status),
                    ),
                    Err(error) => portal_service_rejected(request_id, error),
                }
            }
            Command::CancelPortalKeyboardPermission { operation_id } => {
                let Some(service) = portal_service.as_ref() else {
                    return rejected(
                        request_id,
                        IpcErrorCode::DaemonUnavailable,
                        "portal keyboard permission service is not composed".to_owned(),
                    );
                };
                match service.cancel(&operation_id) {
                    Ok(status) => reply(
                        request_id,
                        &ResponsePayload::PortalKeyboardPermission(status),
                    ),
                    Err(error) => portal_service_rejected(request_id, error),
                }
            }
            Command::RevokePortalKeyboardPermission => {
                let Some(service) = portal_service.as_ref() else {
                    return rejected(
                        request_id,
                        IpcErrorCode::DaemonUnavailable,
                        "portal keyboard permission service is not composed".to_owned(),
                    );
                };
                match service.revoke(Instant::now() + PORTAL_REVOKE_TIMEOUT) {
                    Ok(status) => reply(
                        request_id,
                        &ResponsePayload::PortalKeyboardPermission(status),
                    ),
                    Err(error) => portal_service_rejected(request_id, error),
                }
            }
            Command::StartOfflineModelProvision { source_path } => {
                #[cfg(feature = "asr-transcribe-resident")]
                {
                    match state.start_offline_model_provision(source_path) {
                        Ok(status) => {
                            reply(request_id, &ResponsePayload::OfflineModelProvision(status))
                        }
                        Err((code, detail)) => rejected(request_id, code, detail),
                    }
                }
                #[cfg(not(feature = "asr-transcribe-resident"))]
                {
                    let _ = source_path;
                    rejected(
                        request_id,
                        IpcErrorCode::DaemonUnavailable,
                        "offline model provisioning is not composed".to_owned(),
                    )
                }
            }
            Command::CancelOfflineModelProvision { operation_id } => {
                #[cfg(feature = "asr-transcribe-resident")]
                {
                    match state.cancel_offline_model_provision(&operation_id) {
                        Ok(status) => {
                            reply(request_id, &ResponsePayload::OfflineModelProvision(status))
                        }
                        Err((code, detail)) => rejected(request_id, code, detail),
                    }
                }
                #[cfg(not(feature = "asr-transcribe-resident"))]
                {
                    let _ = operation_id;
                    rejected(
                        request_id,
                        IpcErrorCode::DaemonUnavailable,
                        "offline model provisioning is not composed".to_owned(),
                    )
                }
            }
        }
    }
}

fn portal_service_rejected(
    request_id: narrivy_ipc::RequestId,
    error: PortalPermissionServiceFailure,
) -> Option<Envelope> {
    let (code, detail) = match error {
        PortalPermissionServiceFailure::InvalidOperation => {
            (IpcErrorCode::BadRequest, "invalid portal operation")
        }
        PortalPermissionServiceFailure::Busy => (IpcErrorCode::Busy, "portal service busy"),
        PortalPermissionServiceFailure::AdmissionClosed => (
            IpcErrorCode::DaemonUnavailable,
            "portal service admission closed",
        ),
        PortalPermissionServiceFailure::Actor(PortalActorFailure::TimedOut)
        | PortalPermissionServiceFailure::Actor(PortalActorFailure::Lifecycle(
            narrivy_platform_linux::portal_eis::PortalSessionError {
                failure: PortalSessionFailure::TimedOut,
                ..
            },
        )) => (IpcErrorCode::Timeout, "portal operation timed out"),
        PortalPermissionServiceFailure::Actor(PortalActorFailure::Lifecycle(
            narrivy_platform_linux::portal_eis::PortalSessionError {
                failure: PortalSessionFailure::Cancelled,
                ..
            },
        )) => (IpcErrorCode::Cancelled, "portal operation cancelled"),
        PortalPermissionServiceFailure::Actor(PortalActorFailure::Lifecycle(
            narrivy_platform_linux::portal_eis::PortalSessionError {
                failure: PortalSessionFailure::PermissionDenied,
                ..
            },
        )) => (IpcErrorCode::PermissionDenied, "portal permission denied"),
        PortalPermissionServiceFailure::Actor(_) => {
            (IpcErrorCode::Internal, "portal operation failed")
        }
    };
    rejected(request_id, code, detail.to_owned())
}

/// Fail-closed preflight for one atomic toggle request. It runs before
/// `toggle_decision` and before any session/control mutation, in every state
/// (idle, recording or already stopping).
///
/// The pipeline rule is the pure syntax/catalog rule
/// ([`resolve_pipeline_selection`]) with start-only readiness deliberately
/// excluded: a valid toggle must still stop an active recording when models or
/// capture helpers become unavailable. The returned admission is the one value
/// the starting arm then admits, so a toggle start resolves its identity
/// exactly once.
///
/// The activation source is a closed command contract. The lossless decoder
/// preserves an unrecognized wire tag for forward compatibility, but execution
/// must fail closed, and the raw tag is never echoed into diagnostics.
fn validate_toggle_request(
    engine_config: &EngineConfig,
    pipeline_id: Option<&str>,
    activation: &ActivationSourceDto,
) -> Result<AdmittedPipeline, (IpcErrorCode, String)> {
    if matches!(activation, ActivationSourceDto::Unknown(_)) {
        return Err((
            IpcErrorCode::BadRequest,
            "activation source is not supported by this daemon".to_owned(),
        ));
    }
    resolve_pipeline_selection(engine_config, pipeline_id)
}

/// One fail-closed pipeline selection rule for both `StartSession` and
/// `SelectPipeline`.
///
/// A strict recovery composition admits exactly the two canonical built-in
/// modes (`raw`, `clean`) over its owned capture/ASR/finalization/delivery
/// path; its lane id remains composition configuration and is never selectable
/// or advertised. A non-strict composition admits the historical mock lanes
/// only. Unknown, mismatched and legacy lane ids reject before any journal,
/// capture or session side effect.
fn select_pipeline(
    engine_config: &EngineConfig,
    requested: Option<&str>,
) -> Result<AdmittedPipeline, (IpcErrorCode, String)> {
    let admitted = resolve_pipeline_selection(engine_config, requested)?;
    require_pipeline_start_readiness(engine_config, &admitted)?;
    Ok(admitted)
}

/// Start-only readiness for one already-validated admission.
///
/// This gate is deliberately separate from the pure syntax/catalog selection
/// rule: an explicit start or the starting arm of a toggle must fail closed
/// when the strict composition's assets are unavailable, while a toggle that
/// stops an active recording stays able to do so without consulting current
/// readiness.
fn require_pipeline_start_readiness(
    engine_config: &EngineConfig,
    admitted: &AdmittedPipeline,
) -> Result<(), (IpcErrorCode, String)> {
    if admitted.lane() == PipelineLane::StrictCapture
        && let Err(reason) = engine_config.recovery_static_readiness()
    {
        return Err((
            IpcErrorCode::DaemonUnavailable,
            format!("{} unavailable: {reason}", admitted.id()),
        ));
    }
    Ok(())
}

/// Pure selection resolution with no readiness, capture or journal side
/// effect. Kept separate so the exact identity/correction decision can be
/// regression-tested without a live model or microphone.
fn resolve_pipeline_selection(
    engine_config: &EngineConfig,
    requested: Option<&str>,
) -> Result<AdmittedPipeline, (IpcErrorCode, String)> {
    let strict = engine_config.is_strict_recovery();
    if strict {
        let mode = match requested {
            Some(id) => CorrectionMode::from_pipeline_id(id).ok_or_else(|| {
                (
                    IpcErrorCode::BadRequest,
                    format!(
                        "{id:?} is not selectable by the strict microphone composition; available modes: {RAW_PIPELINE_ID}, {CLEAN_PIPELINE_ID}"
                    ),
                )
            })?,
            None => CorrectionMode::default_for_request(),
        };
        return Ok(AdmittedPipeline::strict_capture(mode));
    }

    if matches!(
        requested,
        Some(
            LINUX_MIC_INBOX_PIPELINE_ID
                | LINUX_MIC_WTYPE_INBOX_PIPELINE_ID
                | LINUX_MIC_ROUTED_INBOX_LANE_ID
        )
    ) {
        return Err((
            IpcErrorCode::BadRequest,
            "the strict recovery lane ids are composition configuration; configure NARRIVY_RECOVERY_MODE and select a raw or clean mode"
                .to_owned(),
        ));
    }

    if engine_config.audio_source == AudioSourceKind::PipeWire {
        return Err((
            IpcErrorCode::DaemonUnavailable,
            "non-strict PipeWire capture is disabled; select a strict owned-capture recovery mode"
                .to_owned(),
        ));
    }

    let selected = requested.map_or_else(default_pipeline_id, ToOwned::to_owned);
    let Some(correction) = CorrectionStrategy::resolve(Some(&selected)) else {
        return Err((
            IpcErrorCode::BadRequest,
            format!(
                "unknown pipeline {selected:?}; available: {DEFAULT_PIPELINE_ID}, {RAW_PIPELINE_ID}, {CLEAN_PIPELINE_ID}"
            ),
        ));
    };
    Ok(AdmittedPipeline::mock_lane(selected, correction))
}

/// The only advertised pipelines are those the same configuration will accept
/// for StartSession. A strict composition advertises exactly its two real
/// built-in modes; the historical mock lanes (and its own lane id) are
/// intentionally absent rather than misleadingly enabled.
fn advertised_pipelines(
    engine_config: &EngineConfig,
    live_resident_ready: bool,
    selection: Option<&super::model_selection::ModelSelection>,
) -> Vec<PipelineSummaryDto> {
    if engine_config.is_strict_recovery() {
        let enabled = engine_config.recovery_static_readiness().is_ok() && live_resident_ready;
        // Only a resident composition has a model and device to choose.
        let selection = selection.filter(|_| engine_config.transcribe_resident);
        return CorrectionMode::ALL
            .into_iter()
            .map(|mode| {
                let engine =
                    selection.map(|selection| selection.pipeline_engine(mode.pipeline_id()));
                PipelineSummaryDto {
                    id: mode.pipeline_id().to_owned(),
                    name: mode.display_name().to_owned(),
                    version: 1,
                    enabled,
                    model_id: engine.as_ref().and_then(|engine| engine.model.clone()),
                    compute: engine.map(|engine| engine.compute.as_str().to_owned()),
                }
            })
            .collect();
    }
    let enabled = engine_config.audio_source != AudioSourceKind::PipeWire;
    vec![
        PipelineSummaryDto {
            id: DEFAULT_PIPELINE_ID.to_owned(),
            name: "Mock dictation pipeline".to_owned(),
            version: 1,
            enabled,
            model_id: None,
            compute: None,
        },
        PipelineSummaryDto {
            id: RAW_PIPELINE_ID.to_owned(),
            name: "Raw dictation".to_owned(),
            version: 1,
            enabled,
            model_id: None,
            compute: None,
        },
        PipelineSummaryDto {
            id: CLEAN_PIPELINE_ID.to_owned(),
            name: "Clean dictation".to_owned(),
            version: 1,
            enabled,
            model_id: None,
            compute: None,
        },
    ]
}

/// Drain the daemon bus and republish each event as a wire envelope to all
/// connected clients. Runs until `shutdown` is set.
pub fn spawn_event_pump(
    server: Arc<UnixIpcServer>,
    state: Arc<DaemonState>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        loop {
            let event = state
                .bus
                .try_next()
                .or_else(|| state.bus.wait_next(PUMP_WAKE_FALLBACK));
            let Some(event) = event else {
                if state.bus.is_closed() {
                    break;
                }
                continue;
            };
            // `serve` owns and closes IPC connections before lifecycle closes
            // this bus. If transport shutdown itself timed out, consume the
            // bounded pre-close queue without attempting more client writes.
            if state.bus.is_closed() && server.active_connection_count() > 0 {
                continue;
            }
            let wire = match event {
                DaemonEvent::Session {
                    event: session_event,
                    pipeline_id,
                    delivery,
                } => Event::SessionStateChanged {
                    session_id: session_event.session_id.0,
                    transaction_id: session_event
                        .transaction_id
                        .as_ref()
                        .map(|txid| txid.as_str().to_owned()),
                    pipeline_id,
                    state: state_wire(session_event.to).to_owned(),
                    seq: session_event.seq.0,
                    delivery: delivery.map(str::to_owned),
                },
                DaemonEvent::Delivered { .. } => continue,
                DaemonEvent::Partial {
                    session_id,
                    transaction_id,
                    pipeline_id,
                    partial,
                } => Event::TranscriptPartial {
                    session_id,
                    transaction_id,
                    pipeline_id,
                    seq: partial.seq,
                    text: partial.text,
                },
                DaemonEvent::Capability {
                    capability,
                    available,
                } => Event::CapabilityChanged {
                    capability: capability.to_owned(),
                    available,
                },
                DaemonEvent::AssistantDelta { turn_id, text } => {
                    Event::AssistantDelta { turn_id, text }
                }
                DaemonEvent::Level { session_id, level } => Event::AudioLevel { session_id, level },
                DaemonEvent::AssistantDone {
                    turn_id,
                    outcome,
                    text,
                    warning,
                } => Event::AssistantDone {
                    turn_id,
                    outcome,
                    text,
                    warning,
                },
                DaemonEvent::Extension(event) => event,
                DaemonEvent::MeetingChanged {
                    id,
                    state,
                    progress,
                    stage,
                } => Event::MeetingChanged {
                    id,
                    state,
                    progress,
                    stage,
                },
            };
            if let Err(err) = server.publish_event(&wire) {
                eprintln!("narrivyd: event publish failed: {err}");
            }
        }
    })
}

/// Close event-pump ownership without relying on periodic polling. Queued
/// events are drained before the pump observes the stop predicate.
pub fn stop_event_pump(state: &DaemonState) {
    state.bus.close();
}

#[cfg(test)]
mod module_permission_tests {
    use super::*;
    use narrivy_policy::ModulePermissions;
    use std::sync::atomic::AtomicUsize;

    fn remote_module() -> Result<ModuleDescriptor, narrivy_policy::PolicyError> {
        Ok(ModuleDescriptor {
            id: "controlled-remote-corrector".to_owned(),
            slot: "text.llm-postprocess".to_owned(),
            version: "1.0.0".to_owned(),
            provides: vec!["controlled-correction".to_owned()],
            healthy: true,
            detail: String::new(),
            permissions: ModulePermissions::new(
                [
                    ModulePermission::TranscriptRead,
                    ModulePermission::NetworkConnect,
                ],
                [ModulePermission::TranscriptRead],
            )?,
        })
    }

    #[test]
    fn daemon_enforces_module_permissions_before_invocation()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = DaemonState::new();
        state.register_module(remote_module()?)?;
        let calls = AtomicUsize::new(0);

        assert!(
            state
                .invoke_module(
                    "controlled-remote-corrector",
                    ModulePermission::NetworkConnect,
                    || calls.fetch_add(1, Ordering::Relaxed),
                )
                .is_err()
        );
        assert_eq!(calls.load(Ordering::Relaxed), 0);

        assert!(
            state
                .invoke_module(
                    "controlled-remote-corrector",
                    ModulePermission::AudioRead,
                    || calls.fetch_add(1, Ordering::Relaxed),
                )
                .is_err()
        );
        assert_eq!(calls.load(Ordering::Relaxed), 0);

        state.set_module_permission_grants(
            "controlled-remote-corrector",
            [
                ModulePermission::TranscriptRead,
                ModulePermission::NetworkConnect,
            ],
        )?;
        state.invoke_module(
            "controlled-remote-corrector",
            ModulePermission::NetworkConnect,
            || calls.fetch_add(1, Ordering::Relaxed),
        )?;
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod p7_modes_tests {
    use super::super::capture::SAMPLE_RATE_HZ;
    use super::super::policy::CorrectionStrategy;
    use super::super::policy::{
        CLEAN_PIPELINE_ID, DEFAULT_PIPELINE_ID, RAW_PIPELINE_ID, collapse_only,
        default_pipeline_id_from,
    };
    use super::*;
    use narrivy_asr_api::{AsrCapabilities, AsrError, AsrEvent, AsrStream, CancelToken};
    use narrivy_core::RecoveryJournal;
    use narrivy_ipc::{PortalEligibilityDto, PortalLifecycleDto};
    use narrivy_platform_api::{
        AudioCapturePort, AudioCaptureSession, CaptureEnd, CaptureStreamChunk, CaptureStreamEvent,
        CaptureStreamInfo, CaptureStreamSummary, CapturedAudio, PlatformError,
        StreamingAudioCapturePort, StreamingAudioCaptureSession,
    };
    use narrivy_platform_linux::portal_eis::{
        PortalCancellation, PortalDriverEvent, PortalEisTextPayload, PortalEisTextSubmission,
        PortalSessionDriver,
    };
    use narrivy_types::{
        DeliveryIntent, SessionCancellation, SessionFinalizationReason, Transcript,
    };
    use narrivyd::correct;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[cfg(feature = "asr-transcribe-cpp")]
    #[test]
    fn completed_session_timing_is_retained_for_status() {
        let state = DaemonState::default();
        let timing = RecoveryTimingTrace::new();
        narrivy_core::RecoveryTimingObserver::observe(
            &timing,
            narrivy_core::RecoveryTimingStage::CaptureStart,
        );

        state.record_completed_timing(&timing);

        let retained = state.last_session_timing().unwrap();
        assert!(retained.0.contains_key("capture_start"));
    }

    /// One strict composition configuration that is always statically
    /// unavailable in tests (no live installed model). Selection identity is
    /// still exercised: canonical modes resolve and then fail closed on
    /// readiness, while lane/mock ids reject as unknown modes.
    fn strict_test_config(inbox: &Path) -> EngineConfig {
        EngineConfig {
            recovery_linux_mic_inbox: true,
            asr_engine: AsrEngineKind::TranscribeCpp,
            audio_source: AudioSourceKind::PipeWire,
            delivery_backend: DeliveryBackendKind::InboxOnly,
            transcribe_model_path: Some(inbox.join("missing.gguf")),
            inbox_dir: Some(inbox.join("inbox")),
            ..EngineConfig::new()
        }
    }

    /// Resolve one admitted pipeline through the production selection rule.
    fn admitted_for(config: &EngineConfig, requested: Option<&str>) -> AdmittedPipeline {
        resolve_pipeline_selection(config, requested).unwrap()
    }

    /// Admit one historical non-strict pipeline id through the real rule.
    fn mock_admitted(id: &str) -> AdmittedPipeline {
        admitted_for(&EngineConfig::new(), Some(id))
    }

    /// Admit one canonical mode over the strict composition without claiming
    /// live readiness (readiness is checked by `select_pipeline` immediately
    /// before admission, not by the identity rule).
    fn strict_admitted(mode: CorrectionMode) -> AdmittedPipeline {
        let config = strict_test_config(Path::new("/controlled-transaction"));
        admitted_for(&config, Some(mode.pipeline_id()))
    }

    struct OpenFailAsr;

    impl AsrEngine for OpenFailAsr {
        fn capabilities(&self) -> AsrCapabilities {
            MockAsr::new("").capabilities()
        }

        fn open_stream(
            &mut self,
            _session: SessionId,
            _cancellation: &CancelToken,
        ) -> Result<Box<dyn AsrStream>, AsrError> {
            Err(AsrError::new(
                "controlled_open_failure",
                "controlled open failure",
            ))
        }
    }

    struct CancelEventAsr;

    struct CancelEventStream {
        session: SessionId,
        cancellation: CancelToken,
        events: Vec<AsrEvent>,
    }

    impl AsrEngine for CancelEventAsr {
        fn capabilities(&self) -> AsrCapabilities {
            MockAsr::new("").capabilities()
        }

        fn open_stream(
            &mut self,
            session: SessionId,
            cancellation: &CancelToken,
        ) -> Result<Box<dyn AsrStream>, AsrError> {
            Ok(Box::new(CancelEventStream {
                session,
                cancellation: cancellation.clone(),
                events: vec![AsrEvent::Started { session }],
            }))
        }
    }

    impl AsrStream for CancelEventStream {
        fn session(&self) -> SessionId {
            self.session
        }

        fn cancellation(&self) -> CancelToken {
            self.cancellation.clone()
        }

        fn push_audio(&mut self, _chunk: &AudioChunk) -> Result<(), AsrError> {
            Ok(())
        }

        fn poll_event(&mut self) -> Option<AsrEvent> {
            if self.events.is_empty() {
                None
            } else {
                Some(self.events.remove(0))
            }
        }

        fn finalize(&mut self) -> Result<(), AsrError> {
            self.cancellation.cancel();
            self.events.push(AsrEvent::Error(AsrError::new(
                "cancelled",
                "controlled cancellation",
            )));
            Ok(())
        }

        fn finish(&mut self, _timeout: Duration) -> Result<(), AsrError> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct HandlerPortalDriver {
        calls: Arc<Mutex<Vec<&'static str>>>,
    }

    impl PortalSessionDriver for HandlerPortalDriver {
        fn request_keyboard_grant(
            &mut self,
            _parent: &PortalParentWindow,
            _deadline: Instant,
            _cancellation: &dyn PortalCancellation,
        ) -> Result<(), PortalSessionFailure> {
            self.calls.lock().unwrap().push("request");
            Ok(())
        }

        fn connect_transport(
            &mut self,
            _deadline: Instant,
            _cancellation: &dyn PortalCancellation,
        ) -> Result<(), PortalSessionFailure> {
            self.calls.lock().unwrap().push("connect");
            Ok(())
        }

        fn close(&mut self, _deadline: Instant) -> Result<(), PortalSessionFailure> {
            self.calls.lock().unwrap().push("close");
            Ok(())
        }

        fn poll_event(&mut self) -> Result<Option<PortalDriverEvent>, PortalSessionFailure> {
            Ok(None)
        }

        fn submit_text(
            &mut self,
            _payload: PortalEisTextPayload,
            _deadline: Instant,
            _cancellation: &dyn PortalCancellation,
        ) -> PortalEisTextSubmission {
            self.calls.lock().unwrap().push("submit");
            PortalEisTextSubmission::SubmittedUnverified
        }

        fn abort_local(&mut self) {
            self.calls.lock().unwrap().push("abort");
        }
    }

    fn handler_envelope_response(config: EngineConfig, request: Envelope) -> ResponsePayload {
        let dir = private_tempdir().unwrap();
        let handler = build_handler(
            Arc::new(DaemonState::new()),
            Arc::new(UnixJournal::open(dir.path().join("journal")).unwrap()),
            None,
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(WorkerSupervisor::new()),
            None,
            Arc::new(config),
            None,
        );
        handler(
            PeerCredentials {
                uid: current_uid(),
                gid: 0,
                pid: None,
            },
            request,
        )
        .unwrap()
        .parse_payload()
        .unwrap()
    }

    fn handler_response(config: EngineConfig, command: Command) -> ResponsePayload {
        let request = Envelope::new(command.kind(), narrivy_ipc::RequestId(1))
            .with_payload(command)
            .unwrap();
        handler_envelope_response(config, request)
    }

    fn wait_for_session_cleanup(state: &DaemonState) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !state.lock_sessions().is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            state.lock_sessions().is_empty(),
            "session slot was not released within the bounded test window"
        );
    }

    #[cfg(feature = "asr-transcribe-resident")]
    struct ProvisionFixture {
        _root: tempfile::TempDir,
        state: Arc<DaemonState>,
        supervisor: Arc<WorkerSupervisor>,
        source_path: PathBuf,
        resident_pid_path: Option<PathBuf>,
        backend_calls: Arc<AtomicU64>,
    }

    #[cfg(feature = "asr-transcribe-resident")]
    fn provision_fixture(
        with_resident_candidate: bool,
    ) -> Result<ProvisionFixture, Box<dyn std::error::Error>> {
        let root = private_tempdir()?;
        let source_path = root.path().join("source.gguf");
        let resident_pid_path = with_resident_candidate.then(|| root.path().join("resident.pid"));
        let backend_calls = Arc::new(AtomicU64::new(0));
        let calls = Arc::clone(&backend_calls);
        let candidate_pid = resident_pid_path.clone();
        let backend: ControlledProvisionBackend =
            Arc::new(move |runtime, _source_path, cancellation, deadline| {
                calls.fetch_add(1, Ordering::SeqCst);
                if cancellation.is_cancelled() {
                    return Err(ProvisionTerminal::CancelledBeforePublicationClean { runtime });
                }
                if Instant::now() >= deadline {
                    return Err(ProvisionTerminal::TimedOutBeforePublicationClean { runtime });
                }
                match candidate_pid.clone() {
                    Some(resident_pid_path) => {
                        Ok(ImportedProvision::Controlled(ControlledActivation {
                            resident_pid_path,
                        }))
                    }
                    None => Err(ProvisionTerminal::ImportFailedClean { runtime }),
                }
            });

        let state = Arc::new(DaemonState::new());
        let supervisor = Arc::new(WorkerSupervisor::new());
        state
            .configure_offline_model_provisioning_for_test(Arc::clone(&supervisor), backend)
            .map_err(|error| std::io::Error::other(error.code))?;
        Ok(ProvisionFixture {
            _root: root,
            state,
            supervisor,
            source_path,
            resident_pid_path,
            backend_calls,
        })
    }

    #[cfg(feature = "asr-transcribe-resident")]
    fn wait_for_no_provision_workers(supervisor: &WorkerSupervisor) -> TestResult {
        let deadline = Instant::now() + Duration::from_secs(2);
        while supervisor.live_worker_count() != 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        if supervisor.live_worker_count() != 0 {
            return Err("provision worker was retained beyond the test deadline".into());
        }
        Ok(())
    }

    #[cfg(feature = "asr-transcribe-resident")]
    fn assert_pid_reaped(pid_path: &Path) -> TestResult {
        let pid: u32 = std::fs::read_to_string(pid_path)?.parse()?;
        if Path::new("/proc").join(pid.to_string()).exists() {
            return Err(format!("resident child {pid} was not reaped").into());
        }
        Ok(())
    }

    #[cfg(feature = "asr-transcribe-resident")]
    fn start_controlled_provision(
        fixture: &ProvisionFixture,
    ) -> Result<ModelProvisionStatusDto, (IpcErrorCode, String)> {
        fixture
            .state
            .start_offline_model_provision(fixture.source_path.to_string_lossy().into_owned())
    }

    #[cfg(feature = "asr-transcribe-resident")]
    #[test]
    fn provision_worker_spawn_rejection_restores_retryable_state_without_ownership() -> TestResult {
        let fixture = provision_fixture(false)?;
        fixture
            .supervisor
            .shutdown_all(Duration::from_millis(100))?;

        let result = start_controlled_provision(&fixture);
        assert!(matches!(result, Err((IpcErrorCode::Internal, _))));
        assert_eq!(fixture.backend_calls.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.supervisor.live_worker_count(), 0);
        let lifecycle = fixture
            .state
            .resident_lifecycle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(lifecycle.phase_kind(), ProvisionPhaseKind::Awaiting);
        let status = lifecycle.status().ok_or("missing spawn-rejection status")?;
        assert_eq!(status.state, ModelProvisionStateDto::Failed);
        assert_eq!(
            status.failure,
            Some(narrivy_ipc::ModelProvisionFailureDto::WorkerFailed)
        );
        drop(lifecycle);
        assert!(fixture.state.take_resident_asr().is_none());
        Ok(())
    }

    #[cfg(feature = "asr-transcribe-resident")]
    #[test]
    fn daemon_shutdown_during_import_cancels_and_reaps_provision_worker() -> TestResult {
        let fixture = provision_fixture(false)?;
        let pause = fixture
            .state
            .pause_next_provision_checkpoint(ProvisionTestCheckpoint::BeforeImport);
        start_controlled_provision(&fixture).map_err(|(_, detail)| detail)?;
        pause.wait_until_reached()?;

        assert!(fixture.state.take_resident_asr().is_none());
        pause.release()?;
        fixture.supervisor.shutdown_all(Duration::from_secs(1))?;

        assert_eq!(fixture.supervisor.live_worker_count(), 0);
        assert_eq!(fixture.backend_calls.load(Ordering::SeqCst), 1);
        let lifecycle = fixture
            .state
            .resident_lifecycle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(lifecycle.phase_kind(), ProvisionPhaseKind::Closing);
        let status = lifecycle.status().ok_or("missing shutdown status")?;
        assert_eq!(
            status.failure,
            Some(narrivy_ipc::ModelProvisionFailureDto::ShuttingDown)
        );
        Ok(())
    }

    #[cfg(feature = "asr-transcribe-resident")]
    #[test]
    fn daemon_shutdown_after_candidate_creation_reaps_rejected_resident() -> TestResult {
        let fixture = provision_fixture(true)?;
        let pid_path = fixture
            .resident_pid_path
            .as_deref()
            .ok_or("missing controlled resident PID path")?;
        let pause = fixture
            .state
            .pause_next_provision_checkpoint(ProvisionTestCheckpoint::BeforeCandidateCommit);
        start_controlled_provision(&fixture).map_err(|(_, detail)| detail)?;
        pause.wait_until_reached()?;
        let pid: u32 = std::fs::read_to_string(pid_path)?.parse()?;
        assert!(Path::new("/proc").join(pid.to_string()).exists());

        assert!(fixture.state.take_resident_asr().is_none());
        pause.release()?;
        fixture.supervisor.shutdown_all(Duration::from_secs(1))?;

        assert_pid_reaped(pid_path)?;
        assert_eq!(fixture.supervisor.live_worker_count(), 0);
        assert_eq!(fixture.backend_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            fixture
                .state
                .resident_lifecycle
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .phase_kind(),
            ProvisionPhaseKind::Closing
        );
        Ok(())
    }

    #[cfg(feature = "asr-transcribe-resident")]
    #[test]
    fn stale_provision_completion_is_rejected_and_reaps_candidate() -> TestResult {
        let fixture = provision_fixture(true)?;
        let pid_path = fixture
            .resident_pid_path
            .as_deref()
            .ok_or("missing controlled resident PID path")?;
        let pause = fixture
            .state
            .pause_next_provision_checkpoint(ProvisionTestCheckpoint::BeforeCandidateCommit);
        start_controlled_provision(&fixture).map_err(|(_, detail)| detail)?;
        pause.wait_until_reached()?;
        fixture
            .state
            .resident_lifecycle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .invalidate_active_generation_for_test();
        pause.release()?;
        wait_for_no_provision_workers(&fixture.supervisor)?;

        assert_pid_reaped(pid_path)?;
        assert_eq!(fixture.supervisor.live_worker_count(), 0);
        assert!(fixture.state.resident_asr().is_none());
        assert!(fixture.state.take_resident_asr().is_none());
        Ok(())
    }

    #[cfg(feature = "asr-transcribe-resident")]
    #[test]
    fn provisioning_worker_panic_terminal_guard_fails_closed_without_ownership() -> TestResult {
        let fixture = provision_fixture(false)?;
        fixture.state.panic_before_next_provision_import();
        start_controlled_provision(&fixture).map_err(|(_, detail)| detail)?;
        wait_for_no_provision_workers(&fixture.supervisor)?;

        assert_eq!(fixture.backend_calls.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.supervisor.live_worker_count(), 0);
        let lifecycle = fixture
            .state
            .resident_lifecycle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(lifecycle.phase_kind(), ProvisionPhaseKind::ActivationFailed);
        let status = lifecycle.status().ok_or("missing panic-guard status")?;
        assert_eq!(status.state, ModelProvisionStateDto::Failed);
        assert_eq!(
            status.cleanup,
            narrivy_ipc::ModelProvisionCleanupDto::Failed
        );
        assert_eq!(
            status.failure,
            Some(narrivy_ipc::ModelProvisionFailureDto::WorkerFailed)
        );
        drop(lifecycle);
        assert!(fixture.state.take_resident_asr().is_none());
        Ok(())
    }

    #[cfg(feature = "asr-transcribe-resident")]
    #[test]
    fn successful_provision_transfers_one_candidate_then_shutdown_reaps_it() -> TestResult {
        let fixture = provision_fixture(true)?;
        let pid_path = fixture
            .resident_pid_path
            .as_deref()
            .ok_or("missing controlled resident PID path")?;
        start_controlled_provision(&fixture).map_err(|(_, detail)| detail)?;
        wait_for_no_provision_workers(&fixture.supervisor)?;

        let controlled_pid = {
            let lifecycle = fixture
                .state
                .resident_lifecycle
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            assert_eq!(lifecycle.phase_kind(), ProvisionPhaseKind::Ready);
            let candidate = lifecycle
                .ready_candidate()
                .ok_or("missing ready candidate")?;
            let ResidentEngineOwner::Controlled(engine) = &candidate.engine else {
                return Err("unexpected production engine in controlled fixture".into());
            };
            engine.pid()
        };
        assert!(Path::new("/proc").join(controlled_pid.to_string()).exists());
        assert!(fixture.state.take_resident_asr().is_none());
        assert_pid_reaped(pid_path)?;
        assert_eq!(fixture.supervisor.live_worker_count(), 0);
        Ok(())
    }

    fn start_test_session(
        state: Arc<DaemonState>,
        journal: Arc<Journal>,
        supervisor: Arc<WorkerSupervisor>,
        config: EngineConfig,
    ) -> ResponsePayload {
        let handler = build_handler(
            state,
            journal,
            None,
            Arc::new(Mutex::new(Vec::new())),
            supervisor,
            None,
            Arc::new(config),
            None,
        );
        let command = Command::StartSession {
            pipeline_id: None,
            activation: "controlled-test".to_owned(),
            recognition: None,
        };
        let request = Envelope::new(command.kind(), narrivy_ipc::RequestId(1))
            .with_payload(command)
            .unwrap();
        handler(
            PeerCredentials {
                uid: current_uid(),
                gid: 0,
                pid: None,
            },
            request,
        )
        .unwrap()
        .parse_payload()
        .unwrap()
    }

    #[test]
    fn strict_session_permit_is_single_owner_and_releases_on_drop() {
        let state = Arc::new(DaemonState::new());
        let first = state.try_admit_strict();
        assert!(first.is_some(), "first strict admission must succeed");
        let Some(first) = first else {
            return;
        };
        assert!(
            state.try_admit_strict().is_none(),
            "a second strict session must be rejected before side effects"
        );
        drop(first);
        assert!(
            state.try_admit_strict().is_some(),
            "terminal permit drop must reopen strict admission"
        );
    }

    #[test]
    fn caller_permit_lease_stays_closed_through_spawn_failure_cleanup() -> TestResult {
        let dir = private_tempdir()?;
        let journal = UnixJournal::open(dir.path().join("journal"))?;
        let state = Arc::new(DaemonState::new());
        let permit = state
            .try_admit_strict()
            .ok_or("strict permit was unexpectedly unavailable")?;
        let caller_lease = Arc::new(permit);
        let dropped_job_lease = Arc::clone(&caller_lease);
        let (session_id, transaction_id) =
            state.begin_session(&journal, &strict_admitted(CorrectionMode::Clean))?;

        // `spawn_with_cancellation` owns and drops the unstarted closure on
        // rejection. Its lease disappears, but the caller's lease remains.
        drop(dropped_job_lease);
        assert!(state.try_admit_strict().is_none());
        state.recovery_spawn_failed(session_id, &transaction_id, &journal);
        assert!(state.lock_sessions().is_empty());
        let entry = journal
            .read(transaction_id.as_str())?
            .ok_or("missing spawn-failure journal record")?;
        assert_eq!(entry.state, JournalEntryState::FailedTerminal);
        assert_eq!(entry.failure_code.as_deref(), Some("worker_spawn_failed"));
        assert!(state.try_admit_strict().is_none());

        drop(caller_lease);
        assert!(state.try_admit_strict().is_some());
        Ok(())
    }

    #[test]
    fn worker_cleanup_terminalizes_pending_record_and_slot_during_unwind() -> TestResult {
        let dir = private_tempdir()?;
        let journal: Arc<Journal> = Arc::new(UnixJournal::open(dir.path().join("journal"))?);
        let state = Arc::new(DaemonState::new());
        let (session_id, transaction_id) =
            state.begin_session(&journal, &mock_admitted(DEFAULT_PIPELINE_ID))?;

        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe({
            let state = Arc::clone(&state);
            let journal = Arc::clone(&journal);
            let transaction_id = transaction_id.clone();
            move || {
                let _cleanup =
                    SessionWorkerCleanup::new(state, journal, session_id, transaction_id, None);
                std::panic::resume_unwind(Box::new("controlled worker panic"));
            }
        }));
        assert!(unwind.is_err());
        assert!(state.lock_sessions().is_empty());
        let entry = journal
            .read(transaction_id.as_str())?
            .ok_or("missing terminal journal record")?;
        assert_eq!(entry.state, JournalEntryState::FailedTerminal);
        assert_eq!(entry.failure_code.as_deref(), Some("worker_panicked"));
        Ok(())
    }

    #[test]
    fn non_strict_asr_error_event_terminalizes_journal_before_idle() -> TestResult {
        let dir = private_tempdir()?;
        let journal = UnixJournal::open(dir.path().join("journal"))?;
        let state = DaemonState::new();
        let (session_id, transaction_id) =
            state.begin_session(&journal, &mock_admitted(DEFAULT_PIPELINE_ID))?;
        let cancellation = CancelFlag::new();
        let mut variants = TranscriptVariants::default();

        let outcome = state.run_session(
            session_id,
            &journal,
            MockAsr::failing("controlled_asr_failure"),
            &|_text| Ok(()),
            SessionRequest {
                audio_samples: sine_fixture_samples(),
                sample_rate_hz: SAMPLE_RATE_HZ,
            },
            cancellation,
            &mut variants,
        )?;

        assert_eq!(outcome, DeliveryOutcome::Failed);
        let entry = journal
            .read(transaction_id.as_str())?
            .ok_or("missing failed ASR journal record")?;
        assert_eq!(entry.state, JournalEntryState::FailedTerminal);
        assert_eq!(
            entry.failure_code.as_deref(),
            Some("controlled_asr_failure")
        );
        assert_eq!(
            state
                .lock_sessions()
                .get(&session_id)
                .map(|slot| slot.machine.state()),
            Some(SessionState::Idle)
        );
        Ok(())
    }

    #[test]
    fn propagated_asr_error_preserves_monotonic_terminal_event_sequence() -> TestResult {
        let dir = private_tempdir()?;
        let journal = UnixJournal::open(dir.path().join("journal"))?;
        let state = DaemonState::new();
        let (session_id, transaction_id) =
            state.begin_session(&journal, &mock_admitted(DEFAULT_PIPELINE_ID))?;
        let mut variants = TranscriptVariants::default();

        let error = match state.run_session(
            session_id,
            &journal,
            OpenFailAsr,
            &|_text| Ok(()),
            SessionRequest {
                audio_samples: sine_fixture_samples(),
                sample_rate_hz: SAMPLE_RATE_HZ,
            },
            CancelFlag::new(),
            &mut variants,
        ) {
            Err(error) => error,
            Ok(_) => return Err("controlled ASR open failure unexpectedly succeeded".into()),
        };
        state.recovery_initialization_failed(
            session_id,
            &transaction_id,
            &journal,
            core_failure_code(&error),
        );

        let mut observed = Vec::new();
        while let Some(event) = state.bus.try_next() {
            if let DaemonEvent::Session { event, .. } = event {
                observed.push((event.seq.0, event.to));
            }
        }
        assert_eq!(
            observed,
            [
                (1, SessionState::Preparing),
                (2, SessionState::Listening),
                (3, SessionState::FinalizingAudio),
                (4, SessionState::Transcribing),
                (5, SessionState::Failed),
                (6, SessionState::Idle),
            ]
        );
        let entry = journal
            .read(transaction_id.as_str())?
            .ok_or("missing propagated failure journal record")?;
        assert_eq!(entry.state, JournalEntryState::FailedTerminal);
        assert_eq!(
            entry.failure_code.as_deref(),
            Some("controlled_open_failure")
        );
        Ok(())
    }

    #[test]
    fn typed_asr_cancellation_event_is_durably_cancelled() -> TestResult {
        let dir = private_tempdir()?;
        let journal = UnixJournal::open(dir.path().join("journal"))?;
        let state = DaemonState::new();
        let (session_id, transaction_id) =
            state.begin_session(&journal, &mock_admitted(DEFAULT_PIPELINE_ID))?;
        let mut variants = TranscriptVariants::default();

        let outcome = state.run_session(
            session_id,
            &journal,
            CancelEventAsr,
            &|_text| Ok(()),
            SessionRequest {
                audio_samples: sine_fixture_samples(),
                sample_rate_hz: SAMPLE_RATE_HZ,
            },
            CancelFlag::new(),
            &mut variants,
        )?;

        assert_eq!(outcome, DeliveryOutcome::Failed);
        let entry = journal
            .read(transaction_id.as_str())?
            .ok_or("missing cancellation journal record")?;
        assert_eq!(entry.state, JournalEntryState::Cancelled);
        assert!(state.bus.try_next().is_some());
        let mut terminal = Vec::new();
        while let Some(event) = state.bus.try_next() {
            if let DaemonEvent::Session { event, .. } = event {
                terminal.push(event.to);
            }
        }
        assert!(terminal.contains(&SessionState::Cancelled));
        assert_eq!(terminal.last(), Some(&SessionState::Idle));
        Ok(())
    }

    #[test]
    fn missing_journal_record_emits_no_false_terminal_events() -> TestResult {
        let dir = private_tempdir()?;
        let journal = UnixJournal::open(dir.path().join("journal"))?;
        let state = DaemonState::new();
        let (session_id, transaction_id) =
            state.begin_session(&journal, &mock_admitted(DEFAULT_PIPELINE_ID))?;
        while state.bus.try_next().is_some() {}
        assert!(journal.remove(transaction_id.as_str())?);

        state.recovery_initialization_failed(
            session_id,
            &transaction_id,
            &journal,
            "controlled_failure",
        );

        assert!(state.bus.try_next().is_none());
        assert!(journal.read(transaction_id.as_str())?.is_none());
        assert_eq!(
            state
                .lock_sessions()
                .get(&session_id)
                .map(|slot| slot.machine.state()),
            Some(SessionState::Listening)
        );
        Ok(())
    }

    #[test]
    fn finalization_rejects_missing_admitted_transaction() -> TestResult {
        let dir = private_tempdir()?;
        let journal = UnixJournal::open(dir.path().join("journal"))?;
        let missing = TransactionId::new("tx-missing-finalization");
        let error = match DaemonState::finalize_entry(
            &journal,
            &missing,
            JournalEntryState::Delivered,
            None,
        ) {
            Err(error) => error,
            Ok(()) => return Err("missing admitted transaction did not fail closed".into()),
        };
        assert!(matches!(
            error,
            CoreError::Journal(narrivy_core::JournalError::Storage(_))
        ));
        Ok(())
    }

    #[test]
    fn non_strict_fixture_failure_is_terminal_and_releases_slot() -> TestResult {
        let dir = private_tempdir()?;
        let journal: Arc<Journal> = Arc::new(UnixJournal::open(dir.path().join("journal"))?);
        let state = Arc::new(DaemonState::new());
        let supervisor = Arc::new(WorkerSupervisor::new());
        let config = EngineConfig {
            audio_source: AudioSourceKind::FileWav(dir.path().join("missing.wav")),
            ..EngineConfig::new()
        };

        assert!(matches!(
            start_test_session(
                Arc::clone(&state),
                Arc::clone(&journal),
                Arc::clone(&supervisor),
                config,
            ),
            ResponsePayload::Accepted
        ));
        wait_for_session_cleanup(&state);
        let entries = journal.entries()?;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].state, JournalEntryState::FailedTerminal);
        assert_eq!(
            entries[0].failure_code.as_deref(),
            Some("fixture_load_failed")
        );
        supervisor.shutdown_all(Duration::from_secs(1))?;
        assert_eq!(supervisor.live_worker_count(), 0);
        Ok(())
    }

    #[cfg(feature = "asr-transcribe-cpp")]
    #[test]
    fn non_strict_missing_model_is_terminal_and_releases_slot() -> TestResult {
        let dir = private_tempdir()?;
        let journal: Arc<Journal> = Arc::new(UnixJournal::open(dir.path().join("journal"))?);
        let state = Arc::new(DaemonState::new());
        let supervisor = Arc::new(WorkerSupervisor::new());
        let config = EngineConfig {
            asr_engine: AsrEngineKind::TranscribeCpp,
            audio_source: AudioSourceKind::SineFixture,
            transcribe_model_path: None,
            ..EngineConfig::new()
        };

        assert!(matches!(
            start_test_session(
                Arc::clone(&state),
                Arc::clone(&journal),
                Arc::clone(&supervisor),
                config,
            ),
            ResponsePayload::Accepted
        ));
        wait_for_session_cleanup(&state);
        let entries = journal.entries()?;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].state, JournalEntryState::FailedTerminal);
        assert_eq!(
            entries[0].failure_code.as_deref(),
            Some("model_unavailable")
        );
        supervisor.shutdown_all(Duration::from_secs(1))?;
        assert_eq!(supervisor.live_worker_count(), 0);
        Ok(())
    }

    #[test]
    fn concurrent_strict_admission_creates_exactly_one_session_and_journal_entry()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = private_tempdir()?;
        let journal = Arc::new(UnixJournal::open(dir.path().join("journal"))?);
        let state = Arc::new(DaemonState::new());
        let supervisor = Arc::new(WorkerSupervisor::new());
        let config = Arc::new(EngineConfig {
            recovery_linux_mic_inbox: true,
            ..EngineConfig::new()
        });
        let start = Arc::new(std::sync::Barrier::new(3));
        let release = Arc::new(std::sync::Barrier::new(3));
        let (result_tx, result_rx) = std::sync::mpsc::channel();
        let mut handles = Vec::new();

        for _ in 0..2 {
            let thread_state = Arc::clone(&state);
            let thread_journal = Arc::clone(&journal);
            let thread_supervisor = Arc::clone(&supervisor);
            let thread_config = Arc::clone(&config);
            let thread_start = Arc::clone(&start);
            let thread_release = Arc::clone(&release);
            let thread_result = result_tx.clone();
            handles.push(std::thread::spawn(move || {
                thread_start.wait();
                let admission = admit_session_request(
                    &thread_state,
                    thread_journal.as_ref(),
                    &strict_admitted(CorrectionMode::Clean),
                    &thread_config,
                    &thread_supervisor,
                );
                let summary = match &admission {
                    Ok(_) => Ok(()),
                    Err((code, _)) => Err(code.clone()),
                };
                let _ = thread_result.send(summary);
                thread_release.wait();
                drop(admission);
            }));
        }
        drop(result_tx);
        start.wait();
        let first = result_rx.recv_timeout(Duration::from_secs(1))?;
        let second = result_rx.recv_timeout(Duration::from_secs(1))?;
        assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
        assert!(
            matches!(first, Err(IpcErrorCode::Busy)) || matches!(second, Err(IpcErrorCode::Busy))
        );
        assert_eq!(state.lock_sessions().len(), 1);
        assert_eq!(journal.entries()?.len(), 1);
        release.wait();
        for handle in handles {
            assert!(handle.join().is_ok());
        }
        assert!(state.try_admit_strict().is_some());
        Ok(())
    }

    #[cfg(feature = "asr-transcribe-resident")]
    #[test]
    fn unavailable_resident_rejects_before_journal_or_session_side_effects()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = private_tempdir()?;
        let journal = UnixJournal::open(dir.path().join("journal"))?;
        let state = Arc::new(DaemonState::new());
        let supervisor = Arc::new(WorkerSupervisor::new());
        let config = EngineConfig {
            recovery_linux_mic_inbox: true,
            transcribe_resident: true,
            ..EngineConfig::new()
        };
        let rejection = admit_session_request(
            &state,
            &journal,
            &strict_admitted(CorrectionMode::Raw),
            &config,
            &supervisor,
        );
        assert!(matches!(
            rejection,
            Err((IpcErrorCode::DaemonUnavailable, _))
        ));
        assert!(state.lock_sessions().is_empty());
        assert!(journal.entries()?.is_empty());
        assert!(state.try_admit_strict().is_some());
        assert_eq!(supervisor.live_worker_count(), 0);
        Ok(())
    }

    #[cfg(feature = "asr-transcribe-resident")]
    #[test]
    fn installed_authority_is_attributed_only_at_the_newtype_composition_seam() {
        fn accepts_authoritative_initializer(
            _initializer: fn(
                &DaemonState,
                InstalledRuntimeBinding,
                InstalledModelRuntimeBinding,
            ) -> Result<(), narrivy_asr_api::AsrError>,
        ) {
        }
        accepts_authoritative_initializer(DaemonState::initialize_installed_resident_asr);

        let generic = resident_binding_attribution(
            ResidentWorkerArtifactStatus::TrustAnchorDigestBound,
            ResidentModelArtifactStatus::TrustAnchorDigestBound,
            false,
        );
        assert_eq!(
            generic,
            (
                ResidentArtifactBinding::StoredBindingUnavailable,
                ResidentArtifactBinding::StoredBindingUnavailable,
            )
        );
        let authoritative = resident_binding_attribution(
            ResidentWorkerArtifactStatus::TrustAnchorDigestBound,
            ResidentModelArtifactStatus::TrustAnchorDigestBound,
            true,
        );
        assert_eq!(
            authoritative,
            (
                ResidentArtifactBinding::InstalledRuntimeManifestBound,
                ResidentArtifactBinding::RegistryObservedDigestBound,
            )
        );

        let awaiting = ResidentRuntimeSnapshot {
            lifecycle: ResidentReadiness::FailedClosed,
            supports_streaming: false,
            artifact_binding: ResidentArtifactBinding::InstalledRuntimeManifestBound,
            model_binding: ResidentArtifactBinding::InstalledModelMissing,
            installed_identity_present: false,
            installed_model_lifecycle: Some(ResidentInstalledLifecycle::Awaiting),
        };
        assert_eq!(
            awaiting.installed_model_lifecycle(),
            Some(Box::new(InstalledModelLifecycleDto::AwaitingModel))
        );
        let ready = ResidentRuntimeSnapshot {
            lifecycle: ResidentReadiness::Ready,
            supports_streaming: true,
            artifact_binding: ResidentArtifactBinding::InstalledRuntimeManifestBound,
            model_binding: ResidentArtifactBinding::RegistryObservedDigestBound,
            installed_identity_present: true,
            installed_model_lifecycle: Some(ResidentInstalledLifecycle::Ready),
        };
        assert_eq!(
            ready.installed_model_lifecycle(),
            Some(Box::new(InstalledModelLifecycleDto::Ready))
        );
        assert_eq!(
            ResidentRuntimeSnapshot {
                lifecycle: ResidentReadiness::Ready,
                supports_streaming: true,
                artifact_binding: ResidentArtifactBinding::ControlledDigestBound,
                model_binding: ResidentArtifactBinding::ControlledDigestBound,
                installed_identity_present: false,
                installed_model_lifecycle: None,
            }
            .installed_model_lifecycle(),
            None
        );
    }

    #[cfg(feature = "asr-transcribe-resident")]
    #[test]
    fn resident_lifecycle_closes_atomically_and_rejects_reinitialization() {
        let state = DaemonState::new();
        assert!(state.take_resident_asr().is_none());
        let result =
            state.initialize_resident_asr(Path::new("/not-opened-after-close"), &"0".repeat(64), 1);
        assert!(matches!(
            result,
            Err(ref error) if error.code == "resident_already_initialized"
        ));
        assert!(state.resident_runtime_snapshot().is_none());
    }

    #[cfg(feature = "asr-transcribe-resident")]
    #[test]
    fn offline_import_failures_preserve_publication_and_cleanup_certainty() {
        assert!(matches!(
            import_failure_terminal(OfflineModelImportError::SourceUnavailable, ()),
            ProvisionTerminal::SourceRejectedBeforeStaging { runtime: () }
        ));
        assert!(matches!(
            import_failure_terminal(OfflineModelImportError::SourceChanged, ()),
            ProvisionTerminal::SourceRejectedAfterStagingClean { runtime: () }
        ));
        assert!(matches!(
            import_failure_terminal(OfflineModelImportError::Collision, ()),
            ProvisionTerminal::ImportFailedClean { runtime: () }
        ));
        for error in [
            OfflineModelImportError::UnknownModel,
            OfflineModelImportError::UnsafeCacheRoot,
        ] {
            assert!(matches!(
                import_failure_terminal(error, ()),
                ProvisionTerminal::ImportFailedClosed
            ));
        }
        assert!(matches!(
            import_failure_terminal(OfflineModelImportError::ExistingInstallInvalid, ()),
            ProvisionTerminal::ImportFailedClosedAfterStagingClean
        ));
        assert!(matches!(
            import_failure_terminal(OfflineModelImportError::CleanupFailedBeforePublication, ()),
            ProvisionTerminal::CleanupFailed
        ));
        assert!(matches!(
            import_failure_terminal(
                OfflineModelImportError::VerificationFailedAfterPublication,
                ()
            ),
            ProvisionTerminal::PublishedUnconfirmed
        ));
    }

    #[cfg(feature = "asr-transcribe-resident")]
    #[test]
    fn resident_streaming_capability_requires_strict_composition_and_reports_qualification() {
        let mut config = EngineConfig::new();
        config.transcribe_resident = true;
        let snapshot = |supports_streaming, lifecycle| ResidentRuntimeSnapshot {
            lifecycle,
            supports_streaming,
            artifact_binding: ResidentArtifactBinding::ControlledDigestBound,
            model_binding: ResidentArtifactBinding::ControlledDigestBound,
            installed_identity_present: false,
            installed_model_lifecycle: None,
        };

        for (resident, expected) in [
            (
                Some(snapshot(true, ResidentReadiness::Ready)),
                "asr.transcribe-cpp.model-streaming=supported",
            ),
            (
                Some(snapshot(false, ResidentReadiness::Ready)),
                "asr.transcribe-cpp.model-streaming=unsupported",
            ),
            (
                None,
                "asr.transcribe-cpp.model-streaming=unavailable:resident_not_initialized",
            ),
        ] {
            let mut capabilities = Vec::new();
            append_resident_streaming_capabilities(&mut capabilities, &config, resident);
            assert_eq!(capabilities[0], expected);
            assert_eq!(
                capabilities[2],
                "asr.transcribe-cpp.streaming=disabled:not_composed"
            );
            assert_eq!(
                capabilities[3],
                "audio.native-energy-vad=disabled:not_composed"
            );
            assert_eq!(capabilities[4], "speech.endpoint=disabled:not_composed");
        }

        config.recovery_linux_mic_inbox = true;
        for (resident, expected, endpoint_state) in [
            (
                Some(snapshot(true, ResidentReadiness::Ready)),
                "asr.transcribe-cpp.streaming=composed:controlled_unqualified",
                "disabled:stop_on_request",
            ),
            (
                Some(snapshot(false, ResidentReadiness::Ready)),
                "asr.transcribe-cpp.streaming=disabled:model_unsupported",
                "disabled:streaming_model_unsupported",
            ),
            (
                None,
                "asr.transcribe-cpp.streaming=disabled:resident_not_initialized",
                "disabled:resident_not_initialized",
            ),
        ] {
            let mut capabilities = Vec::new();
            append_resident_streaming_capabilities(&mut capabilities, &config, resident);
            assert_eq!(capabilities[2], expected);
            assert_eq!(
                capabilities[3],
                format!("audio.native-energy-vad={endpoint_state}")
            );
            assert_eq!(capabilities[4], format!("speech.endpoint={endpoint_state}"));
        }

        for (readiness, worker, composition) in [
            (ResidentReadiness::Active, "active", "disabled:active"),
            (ResidentReadiness::Warming, "warming", "disabled:warming"),
            (
                ResidentReadiness::RestartRequired,
                "restart-required",
                "disabled:restart_required",
            ),
            (
                ResidentReadiness::FailedClosed,
                "failed-closed",
                "disabled:failed_closed",
            ),
        ] {
            let mut capabilities = Vec::new();
            append_resident_streaming_capabilities(
                &mut capabilities,
                &config,
                Some(snapshot(true, readiness)),
            );
            assert_eq!(
                capabilities[1],
                format!("asr.transcribe-cpp.worker={worker}")
            );
            assert_eq!(
                capabilities[2],
                format!("asr.transcribe-cpp.streaming={composition}")
            );
        }

        let mut capabilities = Vec::new();
        append_resident_streaming_capabilities(
            &mut capabilities,
            &config,
            Some(ResidentRuntimeSnapshot {
                lifecycle: ResidentReadiness::Ready,
                supports_streaming: true,
                artifact_binding: ResidentArtifactBinding::StoredBindingUnavailable,
                model_binding: ResidentArtifactBinding::ControlledDigestBound,
                installed_identity_present: false,
                installed_model_lifecycle: None,
            }),
        );
        assert_eq!(
            capabilities[1],
            "asr.transcribe-cpp.worker=failed-closed:artifact-binding"
        );
        assert_eq!(
            capabilities[2],
            "asr.transcribe-cpp.streaming=disabled:artifact_binding"
        );
        assert_eq!(
            capabilities[3],
            "audio.native-energy-vad=disabled:artifact_binding"
        );
        assert_eq!(capabilities[4], "speech.endpoint=disabled:artifact_binding");
    }

    #[cfg(feature = "asr-transcribe-resident")]
    #[test]
    fn running_device_is_reported_only_for_a_ready_worker() {
        use narrivy_asr_transcribe::resident_protocol::WorkerCompute;
        assert_eq!(
            resident_device_capability(false, WorkerCompute::IntegratedGpu),
            None
        );
        for (compute, token) in [
            (WorkerCompute::Cpu, "asr.transcribe-cpp.device=cpu"),
            (
                WorkerCompute::IntegratedGpu,
                "asr.transcribe-cpp.device=igpu",
            ),
            (WorkerCompute::DiscreteGpu, "asr.transcribe-cpp.device=dgpu"),
        ] {
            assert_eq!(
                resident_device_capability(true, compute).as_deref(),
                Some(token)
            );
        }
    }

    #[cfg(feature = "asr-transcribe-resident")]
    #[test]
    fn resident_admission_maps_the_stored_snapshot_fail_closed() {
        let snapshot = |lifecycle, artifact_binding| ResidentRuntimeSnapshot {
            lifecycle,
            supports_streaming: true,
            artifact_binding,
            model_binding: ResidentArtifactBinding::ControlledDigestBound,
            installed_identity_present: false,
            installed_model_lifecycle: None,
        };
        assert_eq!(
            resident_admission_decision(Some(snapshot(
                ResidentReadiness::Ready,
                ResidentArtifactBinding::ControlledDigestBound,
            ))),
            Ok(ResidentAdmissionDecision::Ready)
        );
        assert_eq!(
            resident_admission_decision(Some(ResidentRuntimeSnapshot {
                lifecycle: ResidentReadiness::Ready,
                supports_streaming: true,
                artifact_binding: ResidentArtifactBinding::InstalledRuntimeManifestBound,
                model_binding: ResidentArtifactBinding::RegistryObservedDigestBound,
                installed_identity_present: true,
                installed_model_lifecycle: Some(ResidentInstalledLifecycle::Ready),
            })),
            Ok(ResidentAdmissionDecision::Ready)
        );
        assert!(matches!(
            resident_admission_decision(Some(ResidentRuntimeSnapshot {
                lifecycle: ResidentReadiness::Ready,
                supports_streaming: true,
                artifact_binding: ResidentArtifactBinding::InstalledRuntimeManifestBound,
                model_binding: ResidentArtifactBinding::RegistryObservedDigestBound,
                installed_identity_present: false,
                installed_model_lifecycle: None,
            })),
            Err((IpcErrorCode::DaemonUnavailable, _))
        ));
        assert_eq!(
            resident_admission_decision(Some(snapshot(
                ResidentReadiness::RestartRequired,
                ResidentArtifactBinding::ControlledDigestBound,
            ))),
            Ok(ResidentAdmissionDecision::Recover)
        );
        for lifecycle in [ResidentReadiness::Warming, ResidentReadiness::FailedClosed] {
            assert!(matches!(
                resident_admission_decision(Some(snapshot(
                    lifecycle,
                    ResidentArtifactBinding::ControlledDigestBound,
                ))),
                Err((IpcErrorCode::DaemonUnavailable, _))
            ));
        }
        assert!(matches!(
            resident_admission_decision(Some(snapshot(
                ResidentReadiness::Active,
                ResidentArtifactBinding::ControlledDigestBound,
            ))),
            Err((IpcErrorCode::Busy, _))
        ));
        for binding in [
            ResidentArtifactBinding::PackageBindingUnavailable,
            ResidentArtifactBinding::StoredBindingUnavailable,
        ] {
            assert!(matches!(
                resident_admission_decision(Some(snapshot(ResidentReadiness::Ready, binding))),
                Err((IpcErrorCode::DaemonUnavailable, _))
            ));
        }
        let unavailable_model = ResidentRuntimeSnapshot {
            lifecycle: ResidentReadiness::Ready,
            supports_streaming: true,
            artifact_binding: ResidentArtifactBinding::ControlledDigestBound,
            model_binding: ResidentArtifactBinding::StoredBindingUnavailable,
            installed_identity_present: false,
            installed_model_lifecycle: None,
        };
        assert!(matches!(
            resident_admission_decision(Some(unavailable_model)),
            Err((IpcErrorCode::DaemonUnavailable, _))
        ));
        assert_eq!(
            resident_admission_decision(Some(ResidentRuntimeSnapshot {
                lifecycle: ResidentReadiness::FailedClosed,
                supports_streaming: false,
                artifact_binding: ResidentArtifactBinding::InstalledRuntimeManifestBound,
                model_binding: ResidentArtifactBinding::InstalledModelMissing,
                installed_identity_present: false,
                installed_model_lifecycle: Some(ResidentInstalledLifecycle::Awaiting),
            })),
            Err((
                IpcErrorCode::DaemonUnavailable,
                "selected installed model is missing".to_owned(),
            ))
        );
        assert!(matches!(
            resident_admission_decision(None),
            Err((IpcErrorCode::DaemonUnavailable, _))
        ));
    }

    #[cfg(feature = "asr-transcribe-resident")]
    #[test]
    fn resident_recovery_schedule_never_claims_spawn_for_an_already_changed_state() {
        assert_eq!(
            resident_recovery_without_spawn(ResidentReadiness::Ready),
            Some(ResidentRecoverySchedule::AlreadyReady)
        );
        assert_eq!(
            resident_recovery_without_spawn(ResidentReadiness::Active),
            Some(ResidentRecoverySchedule::AlreadyActive)
        );
        assert_eq!(
            resident_recovery_without_spawn(ResidentReadiness::Warming),
            Some(ResidentRecoverySchedule::AlreadyWarming)
        );
        assert_eq!(
            resident_recovery_without_spawn(ResidentReadiness::FailedClosed),
            Some(ResidentRecoverySchedule::FailedClosed)
        );
        assert_eq!(
            resident_recovery_without_spawn(ResidentReadiness::RestartRequired),
            None
        );
    }

    fn select_response(config: EngineConfig, pipeline_id: &str) -> ResponsePayload {
        handler_response(
            config,
            Command::SelectPipeline {
                pipeline_id: pipeline_id.to_owned(),
            },
        )
    }

    fn unavailable_phase_2a_config(dir: &Path) -> EngineConfig {
        EngineConfig {
            recovery_linux_mic_inbox: true,
            asr_engine: AsrEngineKind::TranscribeCpp,
            audio_source: AudioSourceKind::PipeWire,
            delivery_backend: DeliveryBackendKind::InboxOnly,
            transcribe_model_path: Some(dir.join("missing.gguf")),
            inbox_dir: Some(dir.join("inbox")),
            ..EngineConfig::new()
        }
    }

    fn unavailable_phase_2b_config(dir: &Path) -> EngineConfig {
        EngineConfig {
            recovery_linux_mic_wtype_inbox: true,
            asr_engine: AsrEngineKind::TranscribeCpp,
            audio_source: AudioSourceKind::PipeWire,
            delivery_backend: DeliveryBackendKind::WtypeThenInbox,
            transcribe_model_path: Some(dir.join("missing.gguf")),
            inbox_dir: Some(dir.join("inbox")),
            ..EngineConfig::new()
        }
    }

    fn unavailable_routed_config(dir: &Path) -> EngineConfig {
        EngineConfig {
            recovery_linux_mic_routed_inbox: true,
            asr_engine: AsrEngineKind::TranscribeCpp,
            audio_source: AudioSourceKind::PipeWire,
            delivery_backend: DeliveryBackendKind::RoutedThenInbox,
            transcribe_model_path: Some(dir.join("missing.gguf")),
            inbox_dir: Some(dir.join("inbox")),
            ..EngineConfig::new()
        }
    }

    fn assert_rejected(response: ResponsePayload, expected: IpcErrorCode) {
        assert!(matches!(response, ResponsePayload::Rejected { code, .. } if code == expected));
    }

    #[test]
    fn mismatched_envelope_and_command_kinds_are_rejected_before_dispatch()
    -> Result<(), Box<dyn std::error::Error>> {
        const OPERATION_CANARY: &str = "OPERATION_CANARY_6ce64f64";
        const PARENT_CANARY: &str = "wayland:PARENT_CANARY_71385c93";

        let disguised_portal = Envelope::new(MessageKind::GetStatus, narrivy_ipc::RequestId(1))
            .with_payload(Command::RequestPortalKeyboardPermission {
                operation_id: OPERATION_CANARY.to_owned(),
                parent_window: PARENT_CANARY.to_owned(),
            })
            .unwrap();
        let existing_mismatch = Envelope::new(MessageKind::StartSession, narrivy_ipc::RequestId(2))
            .with_payload(Command::GetStatus)
            .unwrap();

        for request in [disguised_portal, existing_mismatch] {
            let response = handler_envelope_response(EngineConfig::new(), request);
            let ResponsePayload::Rejected { code, detail } = response else {
                return Err("mismatched envelope and command did not reject".into());
            };
            assert_eq!(code, IpcErrorCode::BadRequest);
            assert_eq!(detail, "command does not match envelope kind");
            assert!(!detail.contains(OPERATION_CANARY));
            assert!(!detail.contains(PARENT_CANARY));
        }
        Ok(())
    }

    #[test]
    fn portal_permission_commands_fail_closed_until_service_is_composed()
    -> Result<(), Box<dyn std::error::Error>> {
        let commands = [
            Command::RequestPortalKeyboardPermission {
                operation_id: "sensitive-operation-id".to_owned(),
                parent_window: "wayland:sensitive-parent-token".to_owned(),
            },
            Command::CancelPortalKeyboardPermission {
                operation_id: "sensitive-operation-id".to_owned(),
            },
            Command::RevokePortalKeyboardPermission,
        ];

        for command in commands {
            let response = handler_response(EngineConfig::new(), command);
            let ResponsePayload::Rejected { code, detail } = response else {
                return Err("uncomposed portal permission command did not reject".into());
            };
            assert_eq!(code, IpcErrorCode::DaemonUnavailable);
            assert_eq!(detail, "portal keyboard permission service is not composed");
            assert!(!detail.contains("sensitive-operation-id"));
            assert!(!detail.contains("sensitive-parent-token"));
        }
        Ok(())
    }

    #[test]
    fn offline_model_provision_commands_fail_closed_until_service_is_composed()
    -> Result<(), Box<dyn std::error::Error>> {
        let commands = [
            Command::StartOfflineModelProvision {
                source_path: "/private/source/model.gguf".to_owned(),
            },
            Command::CancelOfflineModelProvision {
                operation_id: "sensitive-provision-operation".to_owned(),
            },
        ];

        for command in commands {
            let response = handler_response(EngineConfig::new(), command);
            let ResponsePayload::Rejected { code, detail } = response else {
                return Err("uncomposed model provisioning command did not reject".into());
            };
            assert_eq!(code, IpcErrorCode::DaemonUnavailable);
            assert_eq!(detail, "offline model provisioning is not composed");
            assert!(!detail.contains("/private/source"));
            assert!(!detail.contains("sensitive-provision-operation"));
        }
        Ok(())
    }

    #[test]
    fn injected_portal_service_routes_authenticated_commands_without_capability_promotion()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = private_tempdir().unwrap();
        let supervisor = Arc::new(WorkerSupervisor::new());
        let calls = Arc::new(Mutex::new(Vec::new()));
        let portal_service = Arc::new(
            PortalPermissionService::spawn(
                &supervisor,
                HandlerPortalDriver {
                    calls: Arc::clone(&calls),
                },
                PortalEligibilityDto::ReadyToRequest,
            )
            .unwrap(),
        );
        let handler = build_handler_with_portal(
            Arc::new(DaemonState::new()),
            Arc::new(UnixJournal::open(dir.path().join("journal")).unwrap()),
            None,
            Arc::new(Mutex::new(Vec::new())),
            Arc::clone(&supervisor),
            None,
            Arc::new(EngineConfig::new()),
            Some(Arc::clone(&portal_service)),
            Some(Arc::new(DaemonDiagnostics::new())),
            None,
        );
        let credentials = PeerCredentials {
            uid: current_uid(),
            gid: 0,
            pid: None,
        };
        let unauthorized_request = Envelope::new(
            MessageKind::RequestPortalKeyboardPermission,
            narrivy_ipc::RequestId(6),
        )
        .with_payload(Command::RequestPortalKeyboardPermission {
            operation_id: "unauthorized-operation".to_owned(),
            parent_window: "wayland:unauthorized-parent".to_owned(),
        })
        .unwrap();
        assert!(
            handler(
                PeerCredentials {
                    uid: current_uid().saturating_add(1),
                    gid: 0,
                    pid: None,
                },
                unauthorized_request,
            )
            .is_none()
        );
        assert!(calls.lock().unwrap().is_empty());

        let send = |command: Command| {
            let request = Envelope::new(command.kind(), narrivy_ipc::RequestId(7))
                .with_payload(command)
                .unwrap();
            handler(credentials, request)
                .unwrap()
                .parse_payload::<ResponsePayload>()
                .unwrap()
        };

        let ResponsePayload::Status(initial) = send(Command::GetStatus) else {
            return Err("unexpected initial status response".into());
        };
        let initial_portal = initial.portal_keyboard.unwrap();
        assert_eq!(initial_portal.lifecycle, PortalLifecycleDto::Idle);
        assert!(!initial_portal.sender_available);
        assert!(!initial_portal.delivery_qualified);

        let ResponsePayload::Rejected { code, detail } =
            send(Command::RequestPortalKeyboardPermission {
                operation_id: "invalid-parent-operation".to_owned(),
                parent_window: "wayland:".to_owned(),
            })
        else {
            return Err("invalid parent did not reject".into());
        };
        assert_eq!(code, IpcErrorCode::BadRequest);
        assert_eq!(detail, "invalid portal parent window");
        assert!(!detail.contains("invalid-parent-operation"));
        assert!(calls.lock().unwrap().is_empty());

        let ResponsePayload::PortalKeyboardPermission(active) =
            send(Command::RequestPortalKeyboardPermission {
                operation_id: "controlled-operation".to_owned(),
                parent_window: "wayland:controlled-parent".to_owned(),
            })
        else {
            return Err("unexpected permission response".into());
        };
        assert_eq!(active.lifecycle, PortalLifecycleDto::Active);
        assert!(active.permission_active && active.transport_connected);
        assert!(!active.sender_available && !active.delivery_qualified);

        let ResponsePayload::PortalKeyboardPermission(idle) =
            send(Command::RevokePortalKeyboardPermission)
        else {
            return Err("unexpected revoke response".into());
        };
        assert_eq!(idle.lifecycle, PortalLifecycleDto::Idle);
        assert!(!idle.permission_active && !idle.transport_connected);
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            ["request", "connect", "close"]
        );
        drop(handler);
        portal_service.shutdown(Duration::from_secs(1)).unwrap();
        assert_eq!(supervisor.live_worker_count(), 0);
        Ok(())
    }

    #[test]
    fn select_pipeline_handler_uses_the_same_fail_closed_rule_as_session_start() {
        // A strict composition recognizes both canonical modes and then fails
        // closed on readiness; its own lane id, the other lane, and the legacy
        // mock lane reject as not selectable before any capture or journal
        // side effect.
        let phase_2a_dir = private_tempdir().unwrap();
        let phase_2a = unavailable_phase_2a_config(phase_2a_dir.path());
        for mode in [RAW_PIPELINE_ID, CLEAN_PIPELINE_ID] {
            assert_rejected(
                select_response(phase_2a.clone(), mode),
                IpcErrorCode::DaemonUnavailable,
            );
            assert_rejected(
                handler_response(
                    phase_2a.clone(),
                    Command::StartSession {
                        pipeline_id: Some(mode.to_owned()),
                        activation: "controlled".to_owned(),
                        recognition: None,
                    },
                ),
                IpcErrorCode::DaemonUnavailable,
            );
        }
        for pipeline in [
            LINUX_MIC_WTYPE_INBOX_PIPELINE_ID,
            LINUX_MIC_INBOX_PIPELINE_ID,
            LINUX_MIC_ROUTED_INBOX_LANE_ID,
            DEFAULT_PIPELINE_ID,
            "unknown-mode",
        ] {
            assert_rejected(
                select_response(phase_2a.clone(), pipeline),
                IpcErrorCode::BadRequest,
            );
        }

        let phase_2b_dir = private_tempdir().unwrap();
        let phase_2b = unavailable_phase_2b_config(phase_2b_dir.path());
        for mode in [RAW_PIPELINE_ID, CLEAN_PIPELINE_ID] {
            assert_rejected(
                select_response(phase_2b.clone(), mode),
                IpcErrorCode::DaemonUnavailable,
            );
            assert_rejected(
                handler_response(
                    phase_2b.clone(),
                    Command::StartSession {
                        pipeline_id: Some(mode.to_owned()),
                        activation: "controlled".to_owned(),
                        recognition: None,
                    },
                ),
                IpcErrorCode::DaemonUnavailable,
            );
        }
        for pipeline in [
            LINUX_MIC_INBOX_PIPELINE_ID,
            LINUX_MIC_WTYPE_INBOX_PIPELINE_ID,
            LINUX_MIC_ROUTED_INBOX_LANE_ID,
            DEFAULT_PIPELINE_ID,
            "unknown-mode",
        ] {
            assert_rejected(
                select_response(phase_2b.clone(), pipeline),
                IpcErrorCode::BadRequest,
            );
        }

        let routed_dir = private_tempdir().unwrap();
        let routed = unavailable_routed_config(routed_dir.path());
        for mode in [RAW_PIPELINE_ID, CLEAN_PIPELINE_ID] {
            assert_rejected(
                select_response(routed.clone(), mode),
                IpcErrorCode::DaemonUnavailable,
            );
            assert_rejected(
                handler_response(
                    routed.clone(),
                    Command::StartSession {
                        pipeline_id: Some(mode.to_owned()),
                        activation: "controlled".to_owned(),
                        recognition: None,
                    },
                ),
                IpcErrorCode::DaemonUnavailable,
            );
        }
        for pipeline in [
            LINUX_MIC_INBOX_PIPELINE_ID,
            LINUX_MIC_WTYPE_INBOX_PIPELINE_ID,
            LINUX_MIC_ROUTED_INBOX_LANE_ID,
            DEFAULT_PIPELINE_ID,
            "unknown-mode",
        ] {
            assert_rejected(
                select_response(routed.clone(), pipeline),
                IpcErrorCode::BadRequest,
            );
        }

        for pipeline in [DEFAULT_PIPELINE_ID, RAW_PIPELINE_ID, CLEAN_PIPELINE_ID] {
            assert_eq!(
                select_response(EngineConfig::new(), pipeline),
                ResponsePayload::Accepted
            );
        }
        for pipeline in [
            LINUX_MIC_INBOX_PIPELINE_ID,
            LINUX_MIC_WTYPE_INBOX_PIPELINE_ID,
            LINUX_MIC_ROUTED_INBOX_LANE_ID,
            "unknown-mode",
        ] {
            assert_rejected(
                select_response(EngineConfig::new(), pipeline),
                IpcErrorCode::BadRequest,
            );
        }
    }

    #[test]
    fn non_strict_pipewire_rejects_before_admission_or_worker_spawn() -> TestResult {
        let dir = private_tempdir()?;
        let journal: Arc<Journal> = Arc::new(UnixJournal::open(dir.path().join("journal"))?);
        let state = Arc::new(DaemonState::new());
        let supervisor = Arc::new(WorkerSupervisor::new());
        let sink: DeliverySink = Arc::new(Mutex::new(Vec::new()));
        let config = EngineConfig {
            audio_source: AudioSourceKind::PipeWire,
            ..EngineConfig::new()
        };
        assert!(
            advertised_pipelines(&config, true, None)
                .iter()
                .all(|pipeline| !pipeline.enabled)
        );

        let handler = build_handler(
            Arc::clone(&state),
            Arc::clone(&journal),
            None,
            Arc::clone(&sink),
            Arc::clone(&supervisor),
            None,
            Arc::new(config),
            None,
        );
        let call = |command: Command| -> Result<ResponsePayload, Box<dyn std::error::Error>> {
            let request =
                Envelope::new(command.kind(), narrivy_ipc::RequestId(1)).with_payload(command)?;
            let response = handler(
                PeerCredentials {
                    uid: current_uid(),
                    gid: 0,
                    pid: None,
                },
                request,
            )
            .ok_or("handler did not reply")?;
            Ok(response.parse_payload()?)
        };

        for command in [
            Command::StartSession {
                pipeline_id: None,
                activation: "controlled-test".to_owned(),
                recognition: None,
            },
            Command::SelectPipeline {
                pipeline_id: DEFAULT_PIPELINE_ID.to_owned(),
            },
        ] {
            assert!(matches!(
                call(command)?,
                ResponsePayload::Rejected {
                    code: IpcErrorCode::DaemonUnavailable,
                    ..
                }
            ));
        }

        assert!(journal.entries()?.is_empty());
        assert!(state.lock_sessions().is_empty());
        assert_eq!(supervisor.live_worker_count(), 0);
        assert!(sink.lock().unwrap_or_else(|p| p.into_inner()).is_empty());
        assert!(matches!(
            call(Command::StopSession)?,
            ResponsePayload::Rejected { .. }
        ));
        assert!(matches!(
            call(Command::CancelSession)?,
            ResponsePayload::Rejected { .. }
        ));
        Ok(())
    }

    #[test]
    fn collapse_only_is_whitespace_collapse_and_nothing_else() {
        assert_eq!(collapse_only("hello   world"), "hello world");
        assert_eq!(
            collapse_only("  keep lowercase, no period "),
            "keep lowercase, no period"
        );
        assert_eq!(collapse_only("one two Three"), "one two Three");
        assert_eq!(collapse_only(""), "");
    }

    #[test]
    fn recovery_events_and_advertisement_never_masquerade_as_mock() {
        let dir = private_tempdir().unwrap();
        let journal = UnixJournal::open(dir.path().join("journal")).unwrap();
        let state = DaemonState::new();
        state
            .begin_session(&journal, &strict_admitted(CorrectionMode::Clean))
            .unwrap();
        while let Some(event) = state.bus.try_next() {
            let DaemonEvent::Session { pipeline_id, .. } = event else {
                continue;
            };
            assert_eq!(pipeline_id.as_deref(), Some(CLEAN_PIPELINE_ID));
        }

        let strict = EngineConfig {
            recovery_linux_mic_inbox: true,
            asr_engine: AsrEngineKind::TranscribeCpp,
            audio_source: AudioSourceKind::PipeWire,
            delivery_backend: DeliveryBackendKind::InboxOnly,
            transcribe_model_path: Some(dir.path().join("missing.gguf")),
            inbox_dir: Some(dir.path().join("inbox")),
            ..EngineConfig::new()
        };
        let strict_advertised = advertised_pipelines(&strict, true, None);
        assert_eq!(
            strict_advertised
                .iter()
                .map(|pipeline| pipeline.id.as_str())
                .collect::<Vec<_>>(),
            [CLEAN_PIPELINE_ID, RAW_PIPELINE_ID],
            "a strict composition advertises exactly its two real modes"
        );
        assert!(strict_advertised.iter().all(|pipeline| !pipeline.enabled));

        let strict_wtype = EngineConfig {
            recovery_linux_mic_wtype_inbox: true,
            asr_engine: AsrEngineKind::TranscribeCpp,
            audio_source: AudioSourceKind::PipeWire,
            delivery_backend: DeliveryBackendKind::WtypeThenInbox,
            transcribe_model_path: Some(dir.path().join("missing-wtype.gguf")),
            inbox_dir: Some(dir.path().join("wtype-inbox")),
            ..EngineConfig::new()
        };
        let wtype_advertised = advertised_pipelines(&strict_wtype, true, None);
        assert_eq!(
            wtype_advertised
                .iter()
                .map(|pipeline| pipeline.id.as_str())
                .collect::<Vec<_>>(),
            [CLEAN_PIPELINE_ID, RAW_PIPELINE_ID]
        );
        assert!(wtype_advertised.iter().all(|pipeline| !pipeline.enabled));

        let routed = EngineConfig {
            recovery_linux_mic_routed_inbox: true,
            asr_engine: AsrEngineKind::TranscribeCpp,
            audio_source: AudioSourceKind::PipeWire,
            delivery_backend: DeliveryBackendKind::RoutedThenInbox,
            transcribe_model_path: Some(dir.path().join("missing-routed.gguf")),
            inbox_dir: Some(dir.path().join("routed-inbox")),
            ..EngineConfig::new()
        };
        let routed_advertised = advertised_pipelines(&routed, true, None);
        assert_eq!(
            routed_advertised
                .iter()
                .map(|pipeline| pipeline.id.as_str())
                .collect::<Vec<_>>(),
            [CLEAN_PIPELINE_ID, RAW_PIPELINE_ID],
            "the routed lane advertises the same two canonical modes"
        );
        assert!(routed_advertised.iter().all(|pipeline| !pipeline.enabled));

        let plain = advertised_pipelines(&EngineConfig::new(), true, None);
        assert_eq!(plain[0].id, DEFAULT_PIPELINE_ID);
        assert!(plain.iter().all(|pipeline| {
            pipeline.id != LINUX_MIC_INBOX_PIPELINE_ID
                && pipeline.id != LINUX_MIC_WTYPE_INBOX_PIPELINE_ID
                && pipeline.id != LINUX_MIC_ROUTED_INBOX_LANE_ID
        }));
    }

    #[test]
    fn volatile_partials_coalesce_in_order_and_debug_redacts_content() {
        let dir = private_tempdir().unwrap();
        let journal = UnixJournal::open(dir.path().join("journal")).unwrap();
        let state = DaemonState::new();
        let (session_id, txid) = state
            .begin_session(&journal, &strict_admitted(CorrectionMode::Clean))
            .unwrap();
        state.publish_partial(
            session_id,
            &txid,
            CLEAN_PIPELINE_ID,
            VolatilePartial {
                seq: 1,
                text: "superseded canary".to_owned(),
            },
        );
        state.publish_partial(
            session_id,
            &txid,
            CLEAN_PIPELINE_ID,
            VolatilePartial {
                seq: 2,
                text: "newest secret canary".to_owned(),
            },
        );
        state
            .observe_recovery_state(session_id, &txid, SessionState::Listening, None)
            .unwrap();

        assert!(matches!(
            state.bus.try_next(),
            Some(DaemonEvent::Session { .. })
        ));
        let partial = state.bus.try_next();
        assert!(partial.is_some(), "coalesced partial missing");
        let partial = partial.unwrap_or(DaemonEvent::Delivered {
            transaction_id: "missing-partial".to_owned(),
            outcome: DeliveryOutcome::Failed,
        });
        let debug = format!("{partial:?}");
        assert!(!debug.contains("newest secret canary"));
        assert!(debug.contains("<redacted>"));
        assert!(matches!(
            partial,
            DaemonEvent::Partial {
                session_id: queued_session,
                partial: VolatilePartial { seq: 2, .. },
                ..
            } if queued_session == session_id
        ));
        assert!(matches!(
            state.bus.try_next(),
            Some(DaemonEvent::Session { .. })
        ));
        assert!(state.bus.try_next().is_none());
        let entry = journal.read(txid.as_str()).unwrap().unwrap();
        assert!(entry.variants.raw.is_none());
        assert!(entry.variants.corrected.is_none());
        assert!(entry.variants.delivered.is_none());
    }

    #[test]
    fn strict_session_does_not_claim_listening_before_capture_readiness() {
        let dir = private_tempdir().unwrap();
        let journal = UnixJournal::open(dir.path().join("journal")).unwrap();
        let state = DaemonState::new();
        let (session_id, txid) = state
            .begin_session(&journal, &strict_admitted(CorrectionMode::Clean))
            .unwrap();
        assert_eq!(
            journal
                .read(txid.as_str())
                .unwrap()
                .and_then(|entry| entry.pipeline_id),
            Some(CLEAN_PIPELINE_ID.to_owned()),
            "the admitted mode identity must be durable from admission"
        );

        {
            let sessions = state.lock_sessions();
            let slot = sessions.get(&session_id).unwrap();
            assert_eq!(slot.machine.state(), SessionState::Preparing);
        }
        let mut observed = Vec::new();
        while let Some(event) = state.bus.try_next() {
            if let DaemonEvent::Session { event, .. } = event {
                observed.push(event.to);
            }
        }
        assert_eq!(observed, [SessionState::Preparing]);

        // This callback is what the recovery slice invokes only after the
        // capture session's explicit readiness boundary succeeds.
        state
            .observe_recovery_state(session_id, &txid, SessionState::Listening, None)
            .unwrap();
        let sessions = state.lock_sessions();
        let slot = sessions.get(&session_id).unwrap();
        assert_eq!(slot.machine.state(), SessionState::Listening);
    }

    #[test]
    fn stop_finalizes_capture_without_cancelling_but_cancel_cancels() {
        let dir = private_tempdir().unwrap();
        let journal = UnixJournal::open(dir.path().join("journal")).unwrap();
        let state = DaemonState::new();
        let (session_id, _) = state
            .begin_session(&journal, &strict_admitted(CorrectionMode::Clean))
            .unwrap();
        let control = state
            .lock_sessions()
            .get(&session_id)
            .unwrap()
            .control
            .clone();

        assert_eq!(state.request_active_finalization(), 1);
        assert!(control.is_finalization_requested());
        assert!(!control.is_cancelled());

        assert_eq!(state.cancel_active_sessions(), 1);
        assert!(control.is_cancelled());
        assert!(control.is_finalization_requested());
    }

    #[test]
    fn strategy_resolution_maps_known_ids_and_rejects_unknown() {
        assert_eq!(
            CorrectionStrategy::resolve(Some(RAW_PIPELINE_ID)),
            Some(CorrectionStrategy::CollapseOnly)
        );
        assert_eq!(
            CorrectionStrategy::resolve(Some(CLEAN_PIPELINE_ID)),
            Some(CorrectionStrategy::FullCorrect)
        );
        assert_eq!(
            CorrectionStrategy::resolve(Some(DEFAULT_PIPELINE_ID)),
            Some(CorrectionStrategy::FullCorrect)
        );
        assert_eq!(
            CorrectionStrategy::resolve(None),
            Some(CorrectionStrategy::FullCorrect)
        );
        assert_eq!(CorrectionStrategy::resolve(Some("turbo")), None);
        // Lane ids are composition configuration: they never resolve to a
        // selectable correction lane on their own.
        assert_eq!(
            CorrectionStrategy::resolve(Some(LINUX_MIC_INBOX_PIPELINE_ID)),
            None
        );
        assert_eq!(
            CorrectionStrategy::resolve(Some(LINUX_MIC_WTYPE_INBOX_PIPELINE_ID)),
            None
        );
    }

    #[test]
    fn canonical_modes_resolve_to_exact_strategies_and_lane_configuration() {
        assert_eq!(
            CorrectionMode::ALL,
            [CorrectionMode::Clean, CorrectionMode::Raw]
        );
        for mode in CorrectionMode::ALL {
            assert_eq!(
                CorrectionMode::from_pipeline_id(mode.pipeline_id()),
                Some(mode)
            );
        }
        assert_eq!(
            CorrectionMode::from_pipeline_id(RAW_PIPELINE_ID).map(CorrectionMode::strategy),
            Some(CorrectionStrategy::CollapseOnly)
        );
        assert_eq!(
            CorrectionMode::from_pipeline_id(CLEAN_PIPELINE_ID).map(CorrectionMode::strategy),
            Some(CorrectionStrategy::FullCorrect)
        );
        for not_a_mode in [
            DEFAULT_PIPELINE_ID,
            LINUX_MIC_INBOX_PIPELINE_ID,
            LINUX_MIC_WTYPE_INBOX_PIPELINE_ID,
            LINUX_MIC_ROUTED_INBOX_LANE_ID,
            "unknown-mode",
        ] {
            assert_eq!(CorrectionMode::from_pipeline_id(not_a_mode), None);
        }
    }

    #[test]
    fn strict_selection_resolves_one_admitted_mode_with_capture_ready_timing() {
        let dir = private_tempdir().unwrap();
        let strict = strict_test_config(dir.path());

        let default = admitted_for(&strict, None);
        assert_eq!(default.id(), CLEAN_PIPELINE_ID);
        assert_eq!(default.correction(), CorrectionStrategy::FullCorrect);
        assert_eq!(default.lane(), PipelineLane::StrictCapture);
        assert!(default.defers_listening_until_capture_ready());

        let raw = admitted_for(&strict, Some(RAW_PIPELINE_ID));
        assert_eq!(raw.id(), RAW_PIPELINE_ID);
        assert_eq!(raw.correction(), CorrectionStrategy::CollapseOnly);

        let clean = admitted_for(&strict, Some(CLEAN_PIPELINE_ID));
        assert_eq!(clean.id(), CLEAN_PIPELINE_ID);
        assert_eq!(clean.correction(), CorrectionStrategy::FullCorrect);

        for not_selectable in [
            DEFAULT_PIPELINE_ID,
            LINUX_MIC_INBOX_PIPELINE_ID,
            LINUX_MIC_WTYPE_INBOX_PIPELINE_ID,
            "unknown-mode",
        ] {
            assert!(matches!(
                resolve_pipeline_selection(&strict, Some(not_selectable)),
                Err((IpcErrorCode::BadRequest, _))
            ));
        }

        // The non-strict mock composition keeps its historical ids and never
        // claims the strict capture-ready timing.
        let mock = admitted_for(&EngineConfig::new(), Some(RAW_PIPELINE_ID));
        assert_eq!(mock.id(), RAW_PIPELINE_ID);
        assert_eq!(mock.correction(), CorrectionStrategy::CollapseOnly);
        assert_eq!(mock.lane(), PipelineLane::Mock);
        assert!(!mock.defers_listening_until_capture_ready());
    }

    #[test]
    fn default_pipeline_env_selects_known_pipeline_only() {
        assert_eq!(
            default_pipeline_id_from(Some(RAW_PIPELINE_ID)),
            RAW_PIPELINE_ID
        );
        assert_eq!(
            default_pipeline_id_from(Some(CLEAN_PIPELINE_ID)),
            CLEAN_PIPELINE_ID
        );
        assert_eq!(default_pipeline_id_from(Some("bogus")), DEFAULT_PIPELINE_ID);
        assert_eq!(default_pipeline_id_from(Some("")), DEFAULT_PIPELINE_ID);
        assert_eq!(default_pipeline_id_from(None), DEFAULT_PIPELINE_ID);
    }

    #[test]
    fn strategies_differ_on_identical_input() {
        let input = "hello   world from mock asr";
        let raw = CorrectionStrategy::CollapseOnly.applies(input);
        let clean = CorrectionStrategy::FullCorrect.applies(input);
        assert_eq!(raw, "hello world from mock asr");
        assert_ne!(raw, clean, "Raw and Clean must produce different output");
        assert_eq!(clean, correct::correct(input));
    }

    #[test]
    fn builtin_defs_validate_and_clean_has_semantic_guard_but_no_llm()
    -> Result<(), Box<dyn std::error::Error>> {
        for def in super::super::policy::builtin_pipeline_defs() {
            def.validate()?;
            let llm: Vec<_> = def
                .nodes
                .iter()
                .filter(|n| n.slot == narrivy_pipeline::CapabilitySlot::TextLlmPostprocess)
                .collect();
            assert!(
                llm.is_empty(),
                "pipeline {} must not carry text.llm-postprocess (FUN-027 lean path)",
                def.id
            );
        }
        let Some(clean) = super::super::policy::builtin_pipeline_defs()
            .into_iter()
            .find(|d| d.id == CLEAN_PIPELINE_ID)
        else {
            return Err("clean builtin is missing".into());
        };
        assert!(
            clean
                .nodes
                .iter()
                .any(|n| n.slot == narrivy_pipeline::CapabilitySlot::TextSemanticGuard),
            "clean chain must contain the text.semantic-guard slot"
        );
        Ok(())
    }

    #[test]
    fn seeding_persists_through_real_registry_and_is_idempotent() {
        let dir = private_tempdir().unwrap();
        let store = dir.path().join("pipelines");
        let inserted = super::super::policy::seed_builtin_pipelines(&store).unwrap();
        assert_eq!(inserted, 2);
        let report = narrivy_pipeline::PipelineRegistry::load_from_dir(&store).unwrap();
        assert!(report.skipped.is_empty(), "{:?}", report.skipped);
        assert!(report.registry.get(RAW_PIPELINE_ID).is_some());
        assert!(report.registry.get(CLEAN_PIPELINE_ID).is_some());
        assert_eq!(
            super::super::policy::seed_builtin_pipelines(&store).unwrap(),
            0
        );
        assert!(
            narrivy_pipeline::PipelineRegistry::load_from_dir(&store)
                .unwrap()
                .registry
                .get(RAW_PIPELINE_ID)
                .is_some()
        );
    }

    fn run_one_session(
        pipeline_id: &str,
    ) -> Result<narrivy_core::JournalEntry, Box<dyn std::error::Error>> {
        let dir = private_tempdir()?;
        let journal_path = dir.path().join("journal");
        let journal = UnixJournal::open(&journal_path)?;
        let state = DaemonState::new();
        let (sid, txid) = state.begin_session(&journal, &mock_admitted(pipeline_id))?;
        let Some(cancel) = state
            .lock_sessions()
            .get(&sid)
            .map(|session| session.control.cancellation().clone())
        else {
            return Err("new session was not registered".into());
        };
        let mut variants = TranscriptVariants::default();
        let outcome = state.run_session(
            sid,
            &journal,
            MockAsr::new(super::super::policy::MOCK_TRANSCRIPT),
            &|_text: &str| Ok(()),
            SessionRequest {
                audio_samples: vec![0.0; 16],
                sample_rate_hz: SAMPLE_RATE_HZ,
            },
            cancel,
            &mut variants,
        )?;
        assert_eq!(outcome, DeliveryOutcome::Delivered);
        let Some(entry) = journal.read(txid.as_str())? else {
            return Err("session journal entry was not persisted".into());
        };
        Ok(entry)
    }

    #[test]
    fn session_identity_retries_collision_without_replacing_existing_record() -> TestResult {
        let dir = private_tempdir()?;
        let journal = UnixJournal::open(dir.path().join("journal"))?;
        let colliding = [1u8; TRANSACTION_ID_RANDOM_BYTES];
        let unique = [2u8; TRANSACTION_ID_RANDOM_BYTES];
        let colliding_txid = format!("tx-{:032x}", u128::from_be_bytes(colliding));
        let existing = narrivy_core::JournalEntry::new_pending(&colliding_txid, 1);
        journal.write_new(&existing)?;
        let state = DaemonState::new();
        let mut calls = 0;
        let (_sid, admitted) = state.begin_session_with_entropy(
            &journal,
            &mock_admitted(DEFAULT_PIPELINE_ID),
            |bytes| {
                *bytes = if calls == 0 { colliding } else { unique };
                calls += 1;
                Ok(())
            },
        )?;
        assert_eq!(calls, 2);
        assert_ne!(admitted.as_str(), colliding_txid);
        assert_eq!(journal.read(&colliding_txid)?, Some(existing));
        assert!(journal.read(admitted.as_str())?.is_some());
        Ok(())
    }

    #[test]
    fn session_identity_failure_and_exhaustion_have_no_visible_session_side_effects() -> TestResult
    {
        let dir = private_tempdir()?;
        let journal = UnixJournal::open(dir.path().join("journal"))?;
        let state = DaemonState::new();
        assert!(
            state
                .begin_session_with_entropy(
                    &journal,
                    &mock_admitted(DEFAULT_PIPELINE_ID),
                    |_bytes| Err(CoreError::Worker("controlled entropy failure".to_owned())),
                )
                .is_err()
        );
        assert!(journal.entries()?.is_empty());
        assert!(state.bus.is_empty());
        assert!(state.lock_sessions().is_empty());

        let colliding = [3u8; TRANSACTION_ID_RANDOM_BYTES];
        let colliding_txid = format!("tx-{:032x}", u128::from_be_bytes(colliding));
        let existing = narrivy_core::JournalEntry::new_pending(&colliding_txid, 1);
        journal.write_new(&existing)?;
        assert!(
            state
                .begin_session_with_entropy(
                    &journal,
                    &mock_admitted(DEFAULT_PIPELINE_ID),
                    |bytes| {
                        *bytes = colliding;
                        Ok(())
                    },
                )
                .is_err()
        );
        assert_eq!(journal.entries()?, vec![existing]);
        assert!(state.bus.is_empty());
        assert!(state.lock_sessions().is_empty());
        Ok(())
    }

    #[test]
    fn raw_pipeline_session_delivers_collapsed_text_end_to_end()
    -> Result<(), Box<dyn std::error::Error>> {
        let entry = run_one_session(RAW_PIPELINE_ID)?;
        assert_eq!(entry.pipeline_id.as_deref(), Some(RAW_PIPELINE_ID));
        let variants = entry.variants;
        assert_eq!(
            variants.delivered.as_deref(),
            Some(collapse_only(super::super::policy::MOCK_TRANSCRIPT).as_str())
        );
        assert_eq!(variants.delivered, variants.corrected);
        Ok(())
    }

    #[test]
    fn clean_pipeline_session_applies_full_correction() -> Result<(), Box<dyn std::error::Error>> {
        let entry = run_one_session(CLEAN_PIPELINE_ID)?;
        assert_eq!(entry.pipeline_id.as_deref(), Some(CLEAN_PIPELINE_ID));
        assert_eq!(
            entry.variants.delivered.as_deref(),
            Some(correct::correct(super::super::policy::MOCK_TRANSCRIPT).as_str())
        );
        Ok(())
    }

    #[test]
    fn raw_remains_recoverable_when_clean_output_exists() -> Result<(), Box<dyn std::error::Error>>
    {
        let entry = run_one_session(CLEAN_PIPELINE_ID)?;
        let Some(raw) = entry.variants.raw.clone() else {
            return Err("raw variant was not persisted".into());
        };
        assert_eq!(raw, super::super::policy::MOCK_TRANSCRIPT);
        assert_ne!(Some(raw.as_str()), entry.variants.delivered.as_deref());
        assert!(entry.variants.is_recoverable());
        Ok(())
    }

    // ------------------------------------------------------------------
    // Atomic installed toggle (protocol minor 7)
    // ------------------------------------------------------------------

    /// Fixture for the atomic toggle: one observable daemon owns the state,
    /// journal, supervisor and handler so tests assert admission identity,
    /// session count and graceful-stop state together.
    struct ToggleHarness {
        _dir: tempfile::TempDir,
        state: Arc<DaemonState>,
        journal: Arc<Journal>,
        supervisor: Arc<WorkerSupervisor>,
        sink: DeliverySink,
        handler: Arc<dyn Fn(PeerCredentials, Envelope) -> Option<Envelope> + Send + Sync>,
    }

    impl ToggleHarness {
        fn start(
            config: EngineConfig,
            worker_start_delay: Option<Duration>,
        ) -> Result<Self, Box<dyn std::error::Error>> {
            let dir = private_tempdir()?;
            let state = Arc::new(DaemonState::new());
            let journal: Arc<Journal> = Arc::new(UnixJournal::open(dir.path().join("journal"))?);
            let supervisor = Arc::new(WorkerSupervisor::new());
            let sink: DeliverySink = Arc::new(Mutex::new(Vec::new()));
            let handler: Arc<dyn Fn(PeerCredentials, Envelope) -> Option<Envelope> + Send + Sync> =
                Arc::new(build_handler(
                    Arc::clone(&state),
                    Arc::clone(&journal),
                    None,
                    Arc::clone(&sink),
                    Arc::clone(&supervisor),
                    worker_start_delay,
                    Arc::new(config),
                    None,
                ));
            Ok(Self {
                _dir: dir,
                state,
                journal,
                supervisor,
                sink,
                handler,
            })
        }

        fn call(&self, command: Command) -> Result<ResponsePayload, Box<dyn std::error::Error>> {
            let request =
                Envelope::new(command.kind(), narrivy_ipc::RequestId(1)).with_payload(command)?;
            let response = (self.handler)(
                PeerCredentials {
                    uid: current_uid(),
                    gid: 0,
                    pid: None,
                },
                request,
            )
            .ok_or("handler did not reply")?;
            Ok(response.parse_payload()?)
        }

        fn toggle(
            &self,
            pipeline_id: Option<&str>,
        ) -> Result<ResponsePayload, Box<dyn std::error::Error>> {
            self.call(Command::ToggleSession {
                pipeline_id: pipeline_id.map(str::to_owned),
                activation: narrivy_ipc::ActivationSourceDto::InstalledCliCommand,
                recognition: None,
            })
        }

        fn start_session(
            &self,
            pipeline_id: Option<&str>,
        ) -> Result<ResponsePayload, Box<dyn std::error::Error>> {
            self.call(Command::StartSession {
                pipeline_id: pipeline_id.map(str::to_owned),
                activation: "controlled-test".to_owned(),
                recognition: None,
            })
        }

        /// The one session identity the daemon currently owns.
        fn only_session(&self) -> Result<(u128, TransactionId), Box<dyn std::error::Error>> {
            let sessions = self.state.lock_sessions();
            let mut ids: Vec<u128> = sessions.keys().copied().collect();
            assert_eq!(ids.len(), 1, "exactly one session must exist");
            let id = ids.pop().ok_or("no session")?;
            let txid = sessions
                .get(&id)
                .map(|slot| slot.transaction_id.clone())
                .ok_or("session slot missing")?;
            Ok((id, txid))
        }

        fn control_snapshot(
            &self,
            session_id: u128,
        ) -> Result<(bool, bool, Option<SessionFinalizationReason>), Box<dyn std::error::Error>>
        {
            let sessions = self.state.lock_sessions();
            let slot = sessions.get(&session_id).ok_or("session vanished")?;
            Ok((
                slot.control.is_cancelled(),
                slot.control.finalization().is_requested(),
                slot.control.finalization().reason(),
            ))
        }

        fn reap(&self) {
            let _ = self.supervisor.shutdown_all(Duration::from_secs(2));
            let deadline = Instant::now() + Duration::from_secs(2);
            while !self.state.lock_sessions().is_empty() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }

    impl Drop for ToggleHarness {
        fn drop(&mut self) {
            self.reap();
        }
    }

    #[test]
    fn idle_toggle_starts_exactly_one_admitted_raw_session() -> TestResult {
        // A 750ms worker start delay keeps the admitted session observable
        // while the assertions run; the drop path cancels and reaps it.
        let harness = ToggleHarness::start(EngineConfig::new(), Some(Duration::from_millis(750)))?;
        let response = harness.toggle(Some(RAW_PIPELINE_ID))?;
        let ResponsePayload::Toggle(result) = response else {
            return Err("toggle did not return a typed toggle payload".into());
        };
        assert_eq!(result.outcome, ToggleOutcomeDto::Started);
        assert_eq!(
            result.activation,
            narrivy_ipc::ActivationSourceDto::InstalledCliCommand
        );
        assert_eq!(result.pipeline_id.as_deref(), Some(RAW_PIPELINE_ID));
        let (session_id, txid) = harness.only_session()?;
        assert_eq!(result.session_id, Some(session_id));
        assert_eq!(result.transaction_id.as_deref(), Some(txid.as_str()));

        let entries = harness.journal.entries()?;
        assert_eq!(entries.len(), 1, "exactly one journal admission");
        assert_eq!(entries[0].pipeline_id.as_deref(), Some(RAW_PIPELINE_ID));
        assert_eq!(entries[0].state, JournalEntryState::Pending);
        assert_eq!(harness.supervisor.live_worker_count(), 1);

        let clean = harness.toggle(Some(CLEAN_PIPELINE_ID))?;
        let ResponsePayload::Toggle(second) = clean else {
            return Err("second toggle did not return a typed payload".into());
        };
        // The second toggle is a stop for the existing session, never a second
        // session (and the canonical mode of the running session is preserved).
        assert_eq!(second.outcome, ToggleOutcomeDto::FinalizationRequested);
        assert_eq!(second.pipeline_id.as_deref(), Some(RAW_PIPELINE_ID));
        assert_eq!(harness.state.lock_sessions().len(), 1);
        assert_eq!(harness.journal.entries()?.len(), 1);
        Ok(())
    }

    #[test]
    fn recording_toggle_requests_graceful_stop_once_and_never_cancels() -> TestResult {
        let harness = ToggleHarness::start(EngineConfig::new(), Some(Duration::from_millis(750)))?;
        assert_eq!(
            harness.start_session(Some(CLEAN_PIPELINE_ID))?,
            ResponsePayload::Accepted
        );
        let (session_id, txid) = harness.only_session()?;

        let response = harness.toggle(None)?;
        let ResponsePayload::Toggle(result) = response else {
            return Err("toggle did not return a typed toggle payload".into());
        };
        assert_eq!(result.outcome, ToggleOutcomeDto::FinalizationRequested);
        assert_eq!(result.session_id, Some(session_id));
        assert_eq!(result.transaction_id.as_deref(), Some(txid.as_str()));

        let (cancelled, finalization_requested, reason) = harness.control_snapshot(session_id)?;
        assert!(!cancelled, "toggle must never cancel accepted audio");
        assert!(finalization_requested);
        assert_eq!(
            reason,
            Some(SessionFinalizationReason::UserRequested),
            "the user toggle owns the graceful stop reason"
        );

        // Exactly once: a second toggle observes the already-requested stop
        // and opens no second session.
        let second = harness.toggle(None)?;
        let ResponsePayload::Toggle(second) = second else {
            return Err("second toggle did not return a typed payload".into());
        };
        assert_eq!(second.outcome, ToggleOutcomeDto::AlreadyStopping);
        assert_eq!(second.session_id, Some(session_id));
        let (cancelled, _, _) = harness.control_snapshot(session_id)?;
        assert!(!cancelled);
        assert_eq!(harness.state.lock_sessions().len(), 1);
        assert_eq!(harness.journal.entries()?.len(), 1);
        Ok(())
    }

    #[test]
    fn concurrent_double_toggle_never_creates_two_sessions_or_loses_the_stop() -> TestResult {
        let harness = Arc::new(ToggleHarness::start(
            EngineConfig::new(),
            Some(Duration::from_millis(1000)),
        )?);
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let mut threads = Vec::new();
        for _ in 0..2 {
            let harness = Arc::clone(&harness);
            let barrier = Arc::clone(&barrier);
            threads.push(std::thread::spawn(move || {
                barrier.wait();
                harness
                    .toggle(Some(CLEAN_PIPELINE_ID))
                    .map_err(|error| error.to_string())
            }));
        }
        barrier.wait();
        let mut outcomes = Vec::new();
        for thread in threads {
            let outcome = thread
                .join()
                .map_err(|_| "toggle thread panicked")?
                .map_err(Box::<dyn std::error::Error>::from)?;
            outcomes.push(outcome);
        }

        let mut started = 0usize;
        let mut stopping = 0usize;
        for outcome in &outcomes {
            let ResponsePayload::Toggle(result) = outcome else {
                return Err(format!("unexpected toggle response {outcome:?}").into());
            };
            match result.outcome {
                ToggleOutcomeDto::Started => started += 1,
                ToggleOutcomeDto::FinalizationRequested | ToggleOutcomeDto::AlreadyStopping => {
                    stopping += 1;
                }
                ToggleOutcomeDto::Unknown(ref tag) => {
                    return Err(format!("unexpected toggle outcome tag {tag:?}").into());
                }
            }
        }
        assert_eq!(started, 1, "exactly one concurrent toggle may start");
        assert_eq!(stopping, 1, "the concurrent toggle must not be lost");

        let (session_id, _) = harness.only_session()?;
        assert_eq!(harness.state.lock_sessions().len(), 1);
        assert_eq!(harness.journal.entries()?.len(), 1);
        let (cancelled, finalization_requested, reason) = harness.control_snapshot(session_id)?;
        assert!(!cancelled, "the stop must never become a cancel");
        assert!(finalization_requested);
        assert_eq!(reason, Some(SessionFinalizationReason::UserRequested));
        Ok(())
    }

    #[test]
    fn late_toggle_during_terminal_cleanup_is_typed_and_deterministic() -> TestResult {
        let harness = ToggleHarness::start(EngineConfig::new(), Some(Duration::from_millis(750)))?;
        assert_eq!(
            harness.start_session(Some(CLEAN_PIPELINE_ID))?,
            ResponsePayload::Accepted
        );
        let (session_id, txid) = harness.only_session()?;
        // The owned worker's later stages: no longer recording, still terminal
        // cleanup in progress.
        harness.state.observe_recovery_state(
            session_id,
            &txid,
            SessionState::FinalizingAudio,
            None,
        )?;
        harness.state.observe_recovery_state(
            session_id,
            &txid,
            SessionState::Transcribing,
            None,
        )?;

        for attempt in 0..2 {
            let response = harness.toggle(None)?;
            let ResponsePayload::Toggle(result) = response else {
                return Err("late toggle did not return a typed payload".into());
            };
            assert_eq!(
                result.outcome,
                ToggleOutcomeDto::AlreadyStopping,
                "late toggle attempt {attempt} must be deterministic"
            );
            assert_eq!(result.session_id, Some(session_id));
        }
        assert_eq!(harness.state.lock_sessions().len(), 1);
        assert_eq!(harness.journal.entries()?.len(), 1);
        let (cancelled, finalization_requested, _) = harness.control_snapshot(session_id)?;
        assert!(!cancelled);
        assert!(
            !finalization_requested,
            "a late toggle must not inject a stop into a non-recording session"
        );
        assert!(
            harness
                .sink
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .is_empty()
        );
        Ok(())
    }

    #[test]
    fn toggle_unsupported_or_malformed_pipeline_has_no_capture_side_effect() -> TestResult {
        let dir = private_tempdir()?;
        for (config, pipeline, expected) in [
            (
                strict_test_config(dir.path()),
                LINUX_MIC_INBOX_PIPELINE_ID,
                IpcErrorCode::BadRequest,
            ),
            (
                strict_test_config(dir.path()),
                "unknown-mode",
                IpcErrorCode::BadRequest,
            ),
            (
                strict_test_config(dir.path()),
                RAW_PIPELINE_ID,
                IpcErrorCode::DaemonUnavailable,
            ),
            (
                EngineConfig::new(),
                LINUX_MIC_ROUTED_INBOX_LANE_ID,
                IpcErrorCode::BadRequest,
            ),
        ] {
            let harness = ToggleHarness::start(config, Some(Duration::from_millis(500)))?;
            let response = harness.toggle(Some(pipeline))?;
            let ResponsePayload::Rejected { code, .. } = response else {
                return Err(format!("{pipeline:?} must be rejected, got {response:?}").into());
            };
            assert_eq!(
                code, expected,
                "{pipeline:?} must fail closed with {expected:?}"
            );
            assert!(
                harness.state.lock_sessions().is_empty(),
                "{pipeline:?} must not register a session"
            );
            assert!(
                harness.journal.entries()?.is_empty(),
                "{pipeline:?} must not write a journal entry"
            );
            assert_eq!(
                harness.supervisor.live_worker_count(),
                0,
                "{pipeline:?} must not spawn a capture worker"
            );
            assert!(
                harness
                    .sink
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .is_empty()
            );
        }
        Ok(())
    }

    #[test]
    fn toggle_rejects_invalid_input_before_stopping_an_active_session() -> TestResult {
        for invalid in [
            Command::ToggleSession {
                pipeline_id: Some("unknown-mode".to_owned()),
                activation: narrivy_ipc::ActivationSourceDto::InstalledCliCommand,
                recognition: None,
            },
            Command::ToggleSession {
                pipeline_id: None,
                activation: narrivy_ipc::ActivationSourceDto::Unknown("unknown-source".to_owned()),
                recognition: None,
            },
        ] {
            let harness = ToggleHarness::start(EngineConfig::new(), Some(Duration::from_secs(1)))?;
            assert_eq!(
                harness.start_session(Some(CLEAN_PIPELINE_ID))?,
                ResponsePayload::Accepted
            );
            let (session_id, _) = harness.only_session()?;
            let response = harness.call(invalid)?;
            assert!(
                matches!(
                    response,
                    ResponsePayload::Rejected {
                        code: IpcErrorCode::BadRequest,
                        ..
                    }
                ),
                "invalid toggle must reject before mutation: {response:?}"
            );
            let (cancelled, finalization_requested, _) = harness.control_snapshot(session_id)?;
            assert!(!cancelled);
            assert!(
                !finalization_requested,
                "invalid input must not stop an active recording"
            );
            assert_eq!(harness.state.lock_sessions().len(), 1);
        }
        Ok(())
    }

    #[test]
    fn a_start_without_a_language_takes_the_current_dictation_language() -> TestResult {
        let urdu = with_dictation_language(AsrRequest::default(), Some("ur".to_owned()));
        assert!(matches!(
            urdu.spoken_language,
            SpokenLanguage::Explicit(ref t) if t.as_str() == "ur"
        ));
        // Automatic leaves the request as it was.
        assert!(with_dictation_language(AsrRequest::default(), None).is_default());
        // A language shortcut keeps its own language.
        let english = AsrRequest {
            spoken_language: SpokenLanguage::Explicit(
                AsrTag::parse("en").map_err(|e| std::io::Error::other(e.code))?,
            ),
            ..AsrRequest::default()
        };
        let kept = with_dictation_language(english, Some("ur".to_owned()));
        assert!(matches!(
            kept.spoken_language,
            SpokenLanguage::Explicit(ref t) if t.as_str() == "en"
        ));
        // So does a shortcut that asks for detection.
        let detect = AsrRequest {
            spoken_language: SpokenLanguage::AutoDetect,
            ..AsrRequest::default()
        };
        assert!(matches!(
            with_dictation_language(detect, Some("ur".to_owned())).spoken_language,
            SpokenLanguage::AutoDetect
        ));
        Ok(())
    }

    #[test]
    fn a_written_language_alone_is_dictated_as_the_spoken_language() -> TestResult {
        let tag = |value| narrivy_ipc::RecognitionTagDto::parse(value);
        let written = |spoken, output| RecognitionSettingsDto {
            spoken_language: spoken,
            output_language: Some(output),
            ..RecognitionSettingsDto::default()
        };
        // "Write in Urdu" (Alt+B made with the old shortcut form).
        let request = recognition_request_from_dto(Some(&written(
            SpokenLanguageDto::Unspecified,
            tag("ur")?,
        )))
        .map_err(|(_, detail)| std::io::Error::other(detail))?;
        assert!(matches!(
            request.spoken_language,
            SpokenLanguage::Explicit(ref t) if t.as_str() == "ur"
        ));
        assert_eq!(request.output.language, None);
        // The same language twice is just that language.
        let request = recognition_request_from_dto(Some(&written(
            SpokenLanguageDto::Explicit { tag: tag("ur")? },
            tag("ur")?,
        )))
        .map_err(|(_, detail)| std::io::Error::other(detail))?;
        assert_eq!(request.output.language, None);
        // A different written language still asks for one, for the
        // capability check to refuse without translation.
        let request = recognition_request_from_dto(Some(&written(
            SpokenLanguageDto::Explicit { tag: tag("ur")? },
            tag("en")?,
        )))
        .map_err(|(_, detail)| std::io::Error::other(detail))?;
        assert_eq!(
            request.output.language.as_ref().map(AsrTag::as_str),
            Some("en")
        );
        Ok(())
    }

    #[test]
    fn recognition_wire_settings_map_to_distinct_domain_fields() -> TestResult {
        let tag = |value| narrivy_ipc::RecognitionTagDto::parse(value);
        let dto = RecognitionSettingsDto {
            spoken_language: SpokenLanguageDto::Explicit { tag: tag("EN_us")? },
            output_language: Some(tag("ur")?),
            output_script: Some(tag("Arab")?),
            translation_target: Some(tag("en")?),
            locale: Some(tag("ur-PK")?),
            context_hints: Some(narrivy_ipc::RecognitionContextHintsDto::try_new(vec![
                "Narrivy".into(),
            ])?),
        };
        let request = recognition_request_from_dto(Some(&dto))
            .map_err(|(_, detail)| std::io::Error::other(detail))?;
        assert!(matches!(
            request.spoken_language,
            SpokenLanguage::Explicit(ref tag) if tag.as_str() == "en-us"
        ));
        assert_eq!(
            request.output.language.as_ref().map(AsrTag::as_str),
            Some("ur")
        );
        assert_eq!(
            request.output.script.as_ref().map(AsrTag::as_str),
            Some("arab")
        );
        assert_eq!(
            request.translation_target.as_ref().map(AsrTag::as_str),
            Some("en")
        );
        assert_eq!(request.locale.as_ref().map(AsrTag::as_str), Some("ur-pk"));
        assert_eq!(
            request.context_bias.as_ref().map(ContextBias::hint_count),
            Some(1)
        );
        assert_eq!(
            recognition_request_from_dto(None)
                .map_err(|(_, detail)| std::io::Error::other(detail))?,
            AsrRequest::default()
        );
        Ok(())
    }

    #[test]
    fn recognition_settings_are_rejected_against_advertised_capabilities_before_admission()
    -> TestResult {
        let en = AsrTag::parse("en")
            .map_err(|_| std::io::Error::other("controlled language is invalid"))?;
        let ur = AsrTag::parse("ur")
            .map_err(|_| std::io::Error::other("controlled language is invalid"))?;
        let capabilities = AsrCapabilities {
            hotword_biasing: true,
            languages: vec![en.clone()],
            ..AsrCapabilities::default()
        };
        let requests = [
            AsrRequest {
                spoken_language: SpokenLanguage::AutoDetect,
                ..AsrRequest::default()
            },
            AsrRequest {
                spoken_language: SpokenLanguage::Explicit(ur.clone()),
                ..AsrRequest::default()
            },
            AsrRequest {
                output: WrittenOutput {
                    language: Some(ur.clone()),
                    ..WrittenOutput::default()
                },
                ..AsrRequest::default()
            },
            AsrRequest {
                output: WrittenOutput {
                    script: Some(
                        AsrTag::parse("Arab")
                            .map_err(|_| std::io::Error::other("controlled script is invalid"))?,
                    ),
                    ..WrittenOutput::default()
                },
                ..AsrRequest::default()
            },
            AsrRequest {
                translation_target: Some(en.clone()),
                ..AsrRequest::default()
            },
            AsrRequest {
                locale: Some(
                    AsrTag::parse("en-US")
                        .map_err(|_| std::io::Error::other("controlled locale is invalid"))?,
                ),
                ..AsrRequest::default()
            },
        ];
        for request in requests {
            let Err((code, detail)) =
                validate_recognition_capabilities(&request, Some(&capabilities))
            else {
                return Err("unsupported settings passed session admission".into());
            };
            assert_eq!(code, IpcErrorCode::BadRequest);
            assert!(detail.starts_with("unsupported recognition settings: "));
        }

        let supported = AsrRequest {
            spoken_language: SpokenLanguage::Explicit(en),
            context_bias: Some(
                ContextBias::try_new(["Narrivy"])
                    .map_err(|error| std::io::Error::other(error.code))?,
            ),
            ..AsrRequest::default()
        };
        validate_recognition_capabilities(&supported, Some(&capabilities))
            .map_err(|(_, detail)| std::io::Error::other(detail))?;
        assert!(
            validate_recognition_capabilities(&supported, None).is_err(),
            "non-default settings must fail when the daemon advertises no recognition capability"
        );
        assert!(
            validate_recognition_capabilities(&AsrRequest::default(), None).is_ok(),
            "the legacy default request remains available"
        );
        Ok(())
    }

    #[test]
    fn recognition_capability_strings_report_only_attested_options() -> TestResult {
        let mut reported = Vec::new();
        append_recognition_request_capabilities(
            &mut reported,
            Some(&AsrCapabilities {
                streaming: false,
                language_detection: true,
                translation: true,
                confidence: false,
                hotword_biasing: true,
                timestamps: false,
                languages: vec![
                    AsrTag::parse("en")
                        .map_err(|_| std::io::Error::other("controlled language is invalid"))?,
                    AsrTag::parse("ur")
                        .map_err(|_| std::io::Error::other("controlled language is invalid"))?,
                ],
                translate_target_languages: vec![AsrTag::parse("en").map_err(|_| {
                    std::io::Error::other("controlled translation target is invalid")
                })?],
                scripts: Vec::new(),
                locales: Vec::new(),
            }),
        );
        assert!(reported.contains(&"asr.recognition-settings=available".to_owned()));
        assert!(reported.contains(&"asr.language-detection=available".to_owned()));
        assert!(reported.contains(&"asr.context-bias=available".to_owned()));
        assert!(reported.contains(&"asr.translation=available".to_owned()));
        assert!(reported.contains(&"asr.languages=en,ur".to_owned()));
        assert!(reported.contains(&"asr.translation-targets=en".to_owned()));
        assert!(!reported.iter().any(|item| item.starts_with("asr.scripts=")));

        let mut unavailable = Vec::new();
        append_recognition_request_capabilities(&mut unavailable, None);
        assert_eq!(unavailable, ["asr.recognition-settings=unavailable"]);
        Ok(())
    }

    #[test]
    fn toggle_rejects_unknown_activation_source_before_any_mutation() -> TestResult {
        let unknown = || ActivationSourceDto::Unknown("future_surface".to_owned());

        // Idle: an unknown activation tag must fail closed and start nothing.
        let harness = ToggleHarness::start(EngineConfig::new(), Some(Duration::from_millis(500)))?;
        let response = harness.call(Command::ToggleSession {
            pipeline_id: Some(CLEAN_PIPELINE_ID.to_owned()),
            activation: unknown(),
            recognition: None,
        })?;
        let ResponsePayload::Rejected { code, detail } = response else {
            return Err(format!("unknown activation source must reject, got {response:?}").into());
        };
        assert_eq!(code, IpcErrorCode::BadRequest);
        assert!(
            !detail.contains("future_surface"),
            "the raw activation tag must never be echoed into diagnostics: {detail:?}"
        );
        assert!(harness.state.lock_sessions().is_empty());
        assert!(harness.journal.entries()?.is_empty());
        assert_eq!(harness.supervisor.live_worker_count(), 0);

        // Recording: the same request must not stop, cancel or otherwise
        // mutate the active session.
        let harness = ToggleHarness::start(EngineConfig::new(), Some(Duration::from_millis(1000)))?;
        assert_eq!(
            harness.start_session(Some(CLEAN_PIPELINE_ID))?,
            ResponsePayload::Accepted
        );
        let (session_id, _) = harness.only_session()?;
        let response = harness.call(Command::ToggleSession {
            pipeline_id: Some(CLEAN_PIPELINE_ID.to_owned()),
            activation: unknown(),
            recognition: None,
        })?;
        assert!(
            matches!(
                response,
                ResponsePayload::Rejected {
                    code: IpcErrorCode::BadRequest,
                    ..
                }
            ),
            "unknown activation source must reject before mutation: {response:?}"
        );
        let (cancelled, finalization_requested, _) = harness.control_snapshot(session_id)?;
        assert!(!cancelled, "invalid input must never cancel accepted audio");
        assert!(
            !finalization_requested,
            "invalid input must not stop an active recording"
        );
        assert_eq!(harness.state.lock_sessions().len(), 1);
        assert_eq!(harness.journal.entries()?.len(), 1);
        Ok(())
    }

    #[test]
    fn malformed_toggle_pipeline_rejects_before_already_stopping_mutation() -> TestResult {
        let harness = ToggleHarness::start(EngineConfig::new(), Some(Duration::from_millis(750)))?;
        assert_eq!(
            harness.start_session(Some(CLEAN_PIPELINE_ID))?,
            ResponsePayload::Accepted
        );
        let (session_id, txid) = harness.only_session()?;
        // The owned worker's later stages: no longer recording, terminal
        // cleanup in progress.
        harness.state.observe_recovery_state(
            session_id,
            &txid,
            SessionState::FinalizingAudio,
            None,
        )?;
        harness.state.observe_recovery_state(
            session_id,
            &txid,
            SessionState::Transcribing,
            None,
        )?;

        for pipeline in ["unknown-mode", LINUX_MIC_ROUTED_INBOX_LANE_ID] {
            let response = harness.toggle(Some(pipeline))?;
            assert!(
                matches!(
                    response,
                    ResponsePayload::Rejected {
                        code: IpcErrorCode::BadRequest,
                        ..
                    }
                ),
                "{pipeline:?} must reject before mutating an already-stopping session: {response:?}"
            );
            let (cancelled, finalization_requested, _) = harness.control_snapshot(session_id)?;
            assert!(!cancelled);
            assert!(
                !finalization_requested,
                "{pipeline:?} must not inject a stop into an already-stopping session"
            );
            assert_eq!(harness.state.lock_sessions().len(), 1);
            assert_eq!(harness.journal.entries()?.len(), 1);
        }
        Ok(())
    }

    #[test]
    fn valid_toggle_stop_survives_unavailable_start_readiness() -> TestResult {
        // This strict composition is valid but cannot start right now: a valid
        // toggle must still gracefully stop an active recording instead of
        // being gated on current model/helper readiness.
        let config = strict_test_config(Path::new("/controlled-toggle-readiness"));
        assert!(
            select_pipeline(&config, Some(CLEAN_PIPELINE_ID)).is_err(),
            "precondition: strict start readiness must be unavailable"
        );
        let harness = ToggleHarness::start(config.clone(), Some(Duration::from_millis(750)))?;
        // The same valid pipeline is still gated at start time: an idle toggle
        // fails closed without opening a session. Only the stop path below is
        // exempt from start readiness.
        let idle = harness.toggle(Some(CLEAN_PIPELINE_ID))?;
        assert!(
            matches!(
                idle,
                ResponsePayload::Rejected {
                    code: IpcErrorCode::DaemonUnavailable,
                    ..
                }
            ),
            "an idle start must still fail closed on unavailable readiness: {idle:?}"
        );
        assert!(harness.state.lock_sessions().is_empty());
        let admitted = admitted_for(&config, Some(CLEAN_PIPELINE_ID));
        let (session_id, txid, permit) = admit_session_request(
            &harness.state,
            harness.journal.as_ref(),
            &admitted,
            &config,
            &harness.supervisor,
        )
        .map_err(|(code, detail)| format!("controlled admission failed: {code:?} {detail}"))?;

        let response = harness.toggle(Some(CLEAN_PIPELINE_ID))?;
        let ResponsePayload::Toggle(result) = response else {
            return Err(format!("toggle did not return a typed payload: {response:?}").into());
        };
        assert_eq!(result.outcome, ToggleOutcomeDto::FinalizationRequested);
        assert_eq!(result.session_id, Some(session_id));
        assert_eq!(result.transaction_id.as_deref(), Some(txid.as_str()));
        assert_eq!(result.pipeline_id.as_deref(), Some(CLEAN_PIPELINE_ID));
        let (cancelled, finalization_requested, reason) = harness.control_snapshot(session_id)?;
        assert!(!cancelled, "a valid stop must never cancel accepted audio");
        assert!(finalization_requested);
        assert_eq!(reason, Some(SessionFinalizationReason::UserRequested));
        assert_eq!(harness.state.lock_sessions().len(), 1);
        assert_eq!(harness.journal.entries()?.len(), 1);

        // Release the directly admitted slot (this fixture has no owned worker
        // to retire it) so the harness drop path never waits on it.
        drop(permit);
        harness.state.lock_sessions().remove(&session_id);
        Ok(())
    }

    #[test]
    fn status_advertises_separate_activation_capability_facts() -> TestResult {
        use narrivy_platform_linux::trigger::PortalInterfaceProbe;

        let harness = ToggleHarness::start(EngineConfig::new(), None)?;
        let capability = |capabilities: &[String], token: &str| -> bool {
            capabilities.iter().any(|cap| cap == token)
        };

        let ResponsePayload::Status(initial) = harness.call(Command::GetStatus)? else {
            return Err("unexpected status payload".into());
        };
        assert!(capability(
            &initial.capabilities,
            "activation.atomic-command=available"
        ));
        assert!(capability(
            &initial.capabilities,
            "activation.portal-global-shortcuts=unavailable:not_probed"
        ));
        assert!(capability(
            &initial.capabilities,
            "activation.portal-listener=unavailable:not_composed"
        ));

        // A present interface is still only a probe result: no listener is
        // claimed until one is composed.
        harness.state.configure_activation_facts(ActivationFacts {
            atomic_command: true,
            portal_interface: Some(PortalInterfaceProbe::Present { version: 1 }),
            portal_listener: false,
        });
        let ResponsePayload::Status(present) = harness.call(Command::GetStatus)? else {
            return Err("unexpected status payload".into());
        };
        assert!(capability(
            &present.capabilities,
            "activation.portal-global-shortcuts=available"
        ));
        assert!(capability(
            &present.capabilities,
            "activation.portal-listener=unavailable:not_composed"
        ));

        // An absent interface is reported truthfully, never inferred away.
        harness.state.configure_activation_facts(ActivationFacts {
            atomic_command: true,
            portal_interface: Some(PortalInterfaceProbe::Absent),
            portal_listener: false,
        });
        let ResponsePayload::Status(absent) = harness.call(Command::GetStatus)? else {
            return Err("unexpected status payload".into());
        };
        assert!(capability(
            &absent.capabilities,
            "activation.portal-global-shortcuts=unavailable:interface_absent"
        ));
        assert!(capability(
            &absent.capabilities,
            "activation.atomic-command=available"
        ));
        Ok(())
    }

    #[cfg(feature = "asr-transcribe-cpp")]
    mod strict_composition_tests {
        use super::*;

        // ------------------------------------------------------------------
        // Controlled strict-composition fixtures.
        //
        // These ports drive the SAME portable slices the installed worker
        // composes (owned capture readiness -> policy -> ASR -> deterministic
        // correction -> durable finalization -> delivery). They are deterministic
        // correctness fixtures for Raw/Clean policy, never native microphone,
        // model or release qualification.
        // ------------------------------------------------------------------

        struct ControlledStrictBatchCapture;

        impl AudioCapturePort for ControlledStrictBatchCapture {
            fn start(&mut self) -> Result<Box<dyn AudioCaptureSession>, PlatformError> {
                Ok(Box::new(ControlledStrictBatchSession {
                    info: CaptureStreamInfo {
                        source_selection: "controlled-strict-batch".to_owned(),
                        sample_rate_hz: SAMPLE_RATE_HZ,
                        channels: 1,
                    },
                }))
            }
        }

        struct ControlledStrictBatchSession {
            info: CaptureStreamInfo,
        }

        impl AudioCaptureSession for ControlledStrictBatchSession {
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

            fn collect(
                &mut self,
                _control: &SessionControl,
            ) -> Result<CapturedAudio, PlatformError> {
                Ok(CapturedAudio {
                    audio: AudioChunk {
                        sample_rate_hz: SAMPLE_RATE_HZ,
                        channels: 1,
                        samples: vec![0.2; 16],
                    },
                    end: CaptureEnd::FinalizationRequested,
                    first_frame_after_start_us: Some(1),
                    capture_duration_us: 2,
                })
            }
        }

        struct ControlledTranscriptAsr {
            transcript: String,
        }

        impl OneShotAsrPort for ControlledTranscriptAsr {
            fn recognize(
                &mut self,
                _session: SessionId,
                _audio: &AudioChunk,
                _cancellation: &SessionCancellation,
            ) -> Result<Transcript, AsrError> {
                Ok(Transcript {
                    text: self.transcript.clone(),
                    is_final: true,
                })
            }
        }

        struct ControlledStrictStreamCapture;

        impl StreamingAudioCapturePort for ControlledStrictStreamCapture {
            fn start_stream(
                &mut self,
                _control: &SessionControl,
            ) -> Result<Box<dyn StreamingAudioCaptureSession>, PlatformError> {
                Ok(Box::new(ControlledStrictStreamSession {
                    info: CaptureStreamInfo {
                        source_selection: "controlled-strict-stream".to_owned(),
                        sample_rate_hz: SAMPLE_RATE_HZ,
                        channels: 1,
                    },
                    pending: None,
                }))
            }
        }

        struct ControlledStrictStreamSession {
            info: CaptureStreamInfo,
            pending: Option<CaptureStreamChunk>,
        }

        impl StreamingAudioCaptureSession for ControlledStrictStreamSession {
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
                self.pending = Some(CaptureStreamChunk {
                    sequence: 0,
                    audio: AudioChunk {
                        sample_rate_hz: SAMPLE_RATE_HZ,
                        channels: 1,
                        samples: vec![0.2; 16],
                    },
                    first_frame_after_start_us: Some(1),
                });
                Ok(())
            }

            fn next_event(
                &mut self,
                _control: &SessionControl,
            ) -> Result<CaptureStreamEvent, PlatformError> {
                if let Some(chunk) = self.pending.take() {
                    return Ok(CaptureStreamEvent::Audio(chunk));
                }
                Ok(CaptureStreamEvent::End(CaptureStreamSummary {
                    end: CaptureEnd::SafetyLimit,
                    total_samples: 16,
                    first_frame_after_start_us: Some(1),
                    capture_duration_us: 2,
                }))
            }

            fn abort(&mut self, _deadline: Instant) -> Result<(), PlatformError> {
                Ok(())
            }
        }

        struct ControlledStreamingTranscriptAsr {
            transcript: String,
        }

        impl AsrEngine for ControlledStreamingTranscriptAsr {
            fn capabilities(&self) -> AsrCapabilities {
                AsrCapabilities {
                    streaming: true,
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
                cancellation: &CancelToken,
            ) -> Result<Box<dyn AsrStream>, AsrError> {
                Ok(Box::new(ControlledStreamingTranscript {
                    session,
                    cancellation: cancellation.clone(),
                    transcript: self.transcript.clone(),
                    pending: vec![AsrEvent::Started { session }],
                    next_sequence: 1,
                    finalized: false,
                }))
            }
        }

        struct ControlledStreamingTranscript {
            session: SessionId,
            cancellation: CancelToken,
            transcript: String,
            pending: Vec<AsrEvent>,
            next_sequence: u64,
            finalized: bool,
        }

        impl AsrStream for ControlledStreamingTranscript {
            fn session(&self) -> SessionId {
                self.session
            }

            fn cancellation(&self) -> CancelToken {
                self.cancellation.clone()
            }

            fn push_audio(&mut self, _chunk: &AudioChunk) -> Result<(), AsrError> {
                self.pending.push(AsrEvent::Partial {
                    seq: self.next_sequence,
                    transcript: Transcript {
                        text: "controlled volatile partial".to_owned(),
                        is_final: false,
                    },
                });
                self.next_sequence += 1;
                Ok(())
            }

            fn poll_event(&mut self) -> Option<AsrEvent> {
                if self.pending.is_empty() {
                    None
                } else {
                    Some(self.pending.remove(0))
                }
            }

            fn finalize(&mut self) -> Result<(), AsrError> {
                self.pending.push(AsrEvent::Final {
                    seq: self.next_sequence,
                    transcript: Transcript {
                        text: self.transcript.clone(),
                        is_final: true,
                    },
                    confidence: None,
                    first_audio_offset_ms: None,
                });
                self.next_sequence += 1;
                self.finalized = true;
                Ok(())
            }

            fn cancel(&self) {
                self.cancellation.cancel();
            }

            fn finish(&mut self, _timeout: Duration) -> Result<(), AsrError> {
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

        struct ControlledInboxDelivery {
            delivered: Arc<Mutex<Vec<String>>>,
        }

        impl DeliveryPort for ControlledInboxDelivery {
            fn deliver(
                &mut self,
                intent: &DeliveryIntent,
                text: &str,
                _cancellation: &SessionCancellation,
            ) -> Result<narrivy_types::DeliveryReport, PlatformError> {
                self.delivered
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(text.to_owned());
                Ok(narrivy_types::DeliveryReport {
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

        fn expected_mode_output(mode: CorrectionMode, input: &str) -> String {
            match mode {
                CorrectionMode::Raw => collapse_only(input),
                CorrectionMode::Clean => correct::correct(input),
            }
        }

        // ------------------------------------------------------------------
        // Task 009: production routed-lane composition over the real adapter.
        //
        // These tests compose the REAL `RoutedThenInboxDelivery` and drive it
        // through the REAL portable `RecoverySlice`, exactly like the owned
        // strict worker does. Only the OS boundaries (target capture, permission
        // evidence, portal submission) are injected: no test may spawn a system
        // helper, type into a focused window, read a clipboard, or depend on
        // ambient session names.
        // ------------------------------------------------------------------

        use std::sync::atomic::AtomicUsize;

        use narrivy_platform_api::DeliveryPort;
        use narrivy_platform_linux::delivery::{
            DeliveryPermissionEvidence, DeliveryPermissionPort, DirectRung, PortalEisTextRung,
            RoutedThenInboxDelivery, RungOutcome, RungQualification, TargetBinding,
            TargetBindingSource, UnavailablePreActivationTargetBinding,
        };
        use narrivy_platform_linux::portal_eis::PortalEisTextSubmission;

        /// One strict routed composition configuration. It is statically
        /// unavailable in tests (no live installed model), but its lane,
        /// delivery backend and delivery-mode identity resolution are the
        /// production ones.
        fn routed_test_config(root: &Path) -> EngineConfig {
            EngineConfig {
                recovery_linux_mic_routed_inbox: true,
                asr_engine: AsrEngineKind::TranscribeCpp,
                audio_source: AudioSourceKind::PipeWire,
                delivery_backend: DeliveryBackendKind::RoutedThenInbox,
                transcribe_model_path: Some(root.join("missing.gguf")),
                inbox_dir: Some(root.join("inbox")),
                ..EngineConfig::new()
            }
        }

        fn controlled_target() -> TargetBinding {
            TargetBinding::Bound {
                application: "ControlledTarget".to_owned(),
                window: "0xcontrolled".to_owned(),
            }
        }

        struct FixedPermission(DeliveryPermissionEvidence);

        impl DeliveryPermissionPort for FixedPermission {
            fn evidence(&mut self) -> DeliveryPermissionEvidence {
                self.0
            }
        }

        struct MutablePermission(Arc<Mutex<DeliveryPermissionEvidence>>);

        impl DeliveryPermissionPort for MutablePermission {
            fn evidence(&mut self) -> DeliveryPermissionEvidence {
                *self
                    .0
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
            }
        }

        /// Controlled portal/EIS text transport. It records every submission
        /// attempt and returns one scripted fact; it never touches a real
        /// portal, socket or EIS sender.
        struct ScriptedPortalSubmitter {
            submissions: Arc<AtomicUsize>,
            result: PortalEisTextSubmission,
        }

        impl PortalEisTextSubmitter for ScriptedPortalSubmitter {
            fn submit(
                &mut self,
                _payload: narrivy_platform_linux::portal_eis::PortalEisTextPayload,
                _cancellation: &SessionCancellation,
                _deadline: Instant,
            ) -> PortalEisTextSubmission {
                self.submissions.fetch_add(1, Ordering::SeqCst);
                self.result
            }
        }

        /// One controlled rung that can verify its own durable result. No
        /// production rung may do this in the current increment.
        struct ScriptedCommittedRung {
            attempts: Arc<AtomicUsize>,
        }

        impl DirectRung for ScriptedCommittedRung {
            fn id(&self) -> &'static str {
                "controlled-verified-backend"
            }

            fn qualification(
                &mut self,
                _target: &TargetBinding,
                _permission: &DeliveryPermissionEvidence,
                _cancellation: &SessionCancellation,
            ) -> RungQualification {
                RungQualification::Qualified
            }

            fn attempt(&mut self, _text: &str, _cancellation: &SessionCancellation) -> RungOutcome {
                self.attempts.fetch_add(1, Ordering::SeqCst);
                RungOutcome::Committed
            }
        }

        fn platform<T>(result: Result<T, PlatformError>) -> Result<T, Box<dyn std::error::Error>> {
            result.map_err(|error| {
                Box::<dyn std::error::Error>::from(std::io::Error::other(format!(
                    "platform error {}",
                    error.code
                )))
            })
        }

        /// Drive the real production adapter through the real portable slice
        /// with one admitted canonical mode and the config-resolved delivery
        /// mode. This is the daemon's own composition path.
        fn run_routed_batch(
            journal: &UnixJournal,
            mode: CorrectionMode,
            input: &str,
            delivery: &mut dyn DeliveryPort,
            delivery_mode: RecoveryDeliveryMode,
            txid: &TransactionId,
        ) -> Result<RecoveryTerminal, Box<dyn std::error::Error>> {
            let admitted = strict_admitted(mode);
            let policy = PolicyEngine::from_document(PolicyDocument::default());
            let mut capture = ControlledStrictBatchCapture;
            let mut asr = ControlledTranscriptAsr {
                transcript: input.to_owned(),
            };
            let mut corrector = strict_composition_corrector(&admitted);
            let mut states = Vec::new();
            let terminal = RecoverySlice {
                journal,
                capture: &mut capture,
                asr: &mut asr,
                policy: &policy,
                corrector: &mut corrector,
                delivery,
                delivery_mode,
            }
            .run(
                SessionId(9009),
                txid.clone(),
                admitted.id().to_owned(),
                &SessionControl::new(),
                &mut |state| {
                    states.push(state);
                    Ok(())
                },
            )?;
            assert!(
                states
                    .windows(2)
                    .any(|pair| pair == [SessionState::Listening, SessionState::FinalizingAudio]),
                "capture readiness must precede finalization on the routed lane"
            );
            Ok(terminal)
        }

        fn attempt_pairs(report: &narrivy_types::DeliveryReport) -> Vec<(&str, DeliveryOutcome)> {
            report
                .attempts
                .iter()
                .map(|attempt| (attempt.backend.as_str(), attempt.outcome.clone()))
                .collect()
        }

        /// The production routed composition is capability-gated: the portal
        /// rung exists only when the daemon owns the permission service, the
        /// clipboard rungs are always REPORTED and never composed from a
        /// helper found on PATH, and composing the ladder never
        /// dispatches anything. The composition lane id is never a pipeline
        /// identity.
        #[test]
        fn strict_routed_composition_is_capability_gated() -> Result<(), Box<dyn std::error::Error>>
        {
            let dir = private_tempdir()?;
            let inbox = dir.path().join("inbox");
            let submissions = Arc::new(AtomicUsize::new(0));
            let denied = DeliveryPermissionEvidence::default();

            let without_portal = platform(compose_strict_routed_delivery_with(
                inbox.clone(),
                controlled_target(),
                None::<ScriptedPortalSubmitter>,
                Box::new(FixedPermission(denied)),
                false,
                false,
            ))?;
            assert_eq!(
                without_portal.rung_ids(),
                vec![
                    "clipboard-paste",
                    "wtype-virtual-keyboard",
                    "clipboard-copy"
                ]
            );

            let with_portal = platform(compose_strict_routed_delivery_with(
                inbox.clone(),
                controlled_target(),
                Some(ScriptedPortalSubmitter {
                    submissions: Arc::clone(&submissions),
                    result: PortalEisTextSubmission::SubmittedUnverified,
                }),
                Box::new(FixedPermission(denied)),
                false,
                false,
            ))?;
            assert_eq!(
                with_portal.rung_ids(),
                vec![
                    "portal-eis-text",
                    "clipboard-paste",
                    "wtype-virtual-keyboard",
                    "clipboard-copy"
                ]
            );

            // Clipboard paste runs ahead of typing (D29) in every
            // composition. Authorization composes the data-control rung
            // instead of the unavailable one; the legacy PATH-resolved
            // `wl-copy`/`wl-paste`/`xclip` implementations are never
            // composed.
            let authorized = platform(compose_strict_routed_delivery_with(
                inbox,
                controlled_target(),
                Some(ScriptedPortalSubmitter {
                    submissions: Arc::clone(&submissions),
                    result: PortalEisTextSubmission::SubmittedUnverified,
                }),
                Box::new(FixedPermission(DeliveryPermissionEvidence {
                    keyboard_insert: true,
                    clipboard_restore: true,
                })),
                true,
                false,
            ))?;
            assert_eq!(
                authorized.rung_ids(),
                vec![
                    "portal-eis-text",
                    "clipboard-paste",
                    "wtype-virtual-keyboard",
                    "clipboard-copy"
                ]
            );
            assert_eq!(
                submissions.load(Ordering::SeqCst),
                0,
                "composing the ladder must never dispatch"
            );

            // Identity separation: the lane id is never a selectable pipeline.
            let config = routed_test_config(Path::new("/controlled-routed"));
            assert_eq!(
                config.strict_recovery_lane(),
                Some(StrictRecoveryLane::TargetRouted)
            );
            assert_eq!(
                config.strict_recovery_delivery_mode(),
                Some(RecoveryDeliveryMode::TargetRoutedThenInboxRecovery)
            );
            assert!(
                resolve_pipeline_selection(&config, Some(LINUX_MIC_ROUTED_INBOX_LANE_ID)).is_err()
            );
            assert_eq!(
                resolve_pipeline_selection(&config, Some(CLEAN_PIPELINE_ID))
                    .map(|admitted| admitted.id().to_owned()),
                Ok(CLEAN_PIPELINE_ID.to_owned())
            );
            Ok(())
        }

        /// The installed Start path supplies the explicit typed-unavailable
        /// admission binding: no trustworthy pre-activation target identity
        /// exists yet, so every target-requiring rung is skipped even while the
        /// permission service reports an active grant, and the exact text is
        /// recovered durably in the Inbox.
        #[test]
        fn strict_routed_production_admission_binding_skips_target_rungs()
        -> Result<(), Box<dyn std::error::Error>> {
            let binding = UnavailablePreActivationTargetBinding.bind();
            assert_eq!(binding.diagnostic_class(), "unavailable");
            assert!(!binding.is_bound());
            assert_eq!(
                binding,
                TargetBinding::Unavailable(
                    narrivy_platform_linux::delivery::PRE_ACTIVATION_BINDING_ABSENT
                )
            );

            let dir = private_tempdir()?;
            let inbox = dir.path().join("inbox");
            let submissions = Arc::new(AtomicUsize::new(0));
            let mut delivery = platform(compose_strict_routed_delivery_with(
                inbox.clone(),
                binding,
                Some(ScriptedPortalSubmitter {
                    submissions: Arc::clone(&submissions),
                    result: PortalEisTextSubmission::SubmittedUnverified,
                }),
                Box::new(FixedPermission(DeliveryPermissionEvidence {
                    keyboard_insert: true,
                    clipboard_restore: true,
                })),
                true,
                false,
            ))?;
            let txid = TransactionId::new("tx-routed-typed-unavailable");
            let report = platform(delivery.deliver(
                &DeliveryIntent {
                    transaction_id: txid.clone(),
                    backend: "routed-then-inbox".to_owned(),
                },
                "typed unavailable target",
                &SessionCancellation::new(),
            ))?;
            assert_eq!(
                submissions.load(Ordering::SeqCst),
                0,
                "a permissioned target rung must never dispatch without a bound target"
            );
            assert_eq!(report.outcome, DeliveryOutcome::InboxFallback);
            assert_eq!(report.attempts[0].backend, "portal-eis-text");
            assert_eq!(
                report.attempts[0].fallback_reason,
                Some(FallbackReason::NoQualifiedBackend)
            );
            assert_eq!(
                std::fs::read_to_string(inbox.join(format!("{}.txt", txid.as_str())))?,
                "typed unavailable target"
            );
            Ok(())
        }

        /// One real composed routed session persists the exact admitted mode,
        /// transaction and delivery-mode identity; a permissioned dispatch the
        /// helper accepted but cannot verify ends as `SubmittedUnverified`,
        /// journals the exact corrected text and leaves no Inbox copy (F-M4).
        #[test]
        fn strict_routed_session_reaches_approved_router_with_exact_admitted_identity()
        -> Result<(), Box<dyn std::error::Error>> {
            // The portal text-frame transport accepts only bounded ASCII
            // printable payloads; Unicode/multiline content is covered by its
            // own bounded-failure test below.
            let input = "hello   routed   world  ";
            let mode = CorrectionMode::Clean;
            let config = routed_test_config(Path::new("/controlled-routed"));
            let delivery_mode = config
                .strict_recovery_delivery_mode()
                .ok_or("routed lane must resolve one delivery mode")?;

            let dir = private_tempdir()?;
            let journal = UnixJournal::open(dir.path().join("journal"))?;
            let inbox = dir.path().join("inbox");
            let submissions = Arc::new(AtomicUsize::new(0));
            let mut delivery = platform(RoutedThenInboxDelivery::new(
                inbox.clone(),
                controlled_target(),
                Box::new(FixedPermission(DeliveryPermissionEvidence {
                    keyboard_insert: true,
                    clipboard_restore: false,
                })),
                vec![Box::new(PortalEisTextRung::new(ScriptedPortalSubmitter {
                    submissions: Arc::clone(&submissions),
                    result: PortalEisTextSubmission::SubmittedUnverified,
                }))],
            ))?;

            let txid = TransactionId::new("tx-task009-routed-identity");
            let terminal =
                run_routed_batch(&journal, mode, input, &mut delivery, delivery_mode, &txid)?;
            let expected = expected_mode_output(mode, input);

            assert_eq!(submissions.load(Ordering::SeqCst), 1);
            // F-M4: an accepted but unverifiable submission is a finished
            // delivery with no plaintext Inbox copy.
            let RecoveryTerminal::SubmittedUnverified(report) = terminal else {
                return Err(
                    "an accepted unverifiable dispatch must terminate as SubmittedUnverified"
                        .into(),
                );
            };
            assert_eq!(report.backend, "routed-then-inbox");
            assert_eq!(report.transaction_id, txid);
            assert_eq!(report.outcome, DeliveryOutcome::SubmittedUnverified);
            assert_eq!(
                attempt_pairs(&report),
                vec![("portal-eis-text", DeliveryOutcome::SubmittedUnverified)]
            );

            let entry = journal
                .read(txid.as_str())?
                .ok_or("missing routed journal entry")?;
            assert_eq!(entry.pipeline_id.as_deref(), Some(CLEAN_PIPELINE_ID));
            assert_eq!(entry.state, JournalEntryState::Delivered);
            let intent = entry
                .delivery_intent
                .as_ref()
                .ok_or("routed session must persist its delivery intent")?;
            assert_eq!(intent.backend, "routed-then-inbox");
            assert_eq!(intent.transaction_id, txid);
            assert_eq!(entry.variants.corrected.as_deref(), Some(expected.as_str()));
            assert_eq!(entry.variants.delivered.as_deref(), Some(expected.as_str()));
            assert!(
                !inbox.join(format!("{}.txt", txid.as_str())).exists(),
                "a successful submission leaves no plaintext Inbox copy (F-M4)"
            );
            Ok(())
        }

        /// Unicode and multiline final text must never be mangled or silently
        /// dropped by a backend that cannot carry it: the portal rung records a
        /// bounded failure without dispatching, the portable report stays a
        /// recovery outcome, and the durable Inbox artifact keeps the exact
        /// corrected text.
        #[test]
        fn strict_routed_unicode_and_multiline_stay_unmangled_with_bounded_failure()
        -> Result<(), Box<dyn std::error::Error>> {
            let input = "  Grüße   aus  Zürich\nsecond   line\tmit  Tabs ";
            let mode = CorrectionMode::Clean;
            let dir = private_tempdir()?;
            let journal = UnixJournal::open(dir.path().join("journal"))?;
            let inbox = dir.path().join("inbox");
            let submissions = Arc::new(AtomicUsize::new(0));
            let mut delivery = platform(RoutedThenInboxDelivery::new(
                inbox.clone(),
                controlled_target(),
                Box::new(FixedPermission(DeliveryPermissionEvidence {
                    keyboard_insert: true,
                    clipboard_restore: false,
                })),
                vec![Box::new(PortalEisTextRung::new(ScriptedPortalSubmitter {
                    submissions: Arc::clone(&submissions),
                    result: PortalEisTextSubmission::SubmittedUnverified,
                }))],
            ))?;
            let config = routed_test_config(Path::new("/controlled-routed"));
            let txid = TransactionId::new("tx-task009-routed-unicode");
            let terminal = run_routed_batch(
                &journal,
                mode,
                input,
                &mut delivery,
                config
                    .strict_recovery_delivery_mode()
                    .ok_or("routed lane must resolve one delivery mode")?,
                &txid,
            )?;

            assert_eq!(
                submissions.load(Ordering::SeqCst),
                0,
                "a payload the transport cannot carry must never be dispatched"
            );
            let RecoveryTerminal::InboxRecovered(report) = terminal else {
                return Err("bounded payload failure must recover durably".into());
            };
            assert_eq!(
                attempt_pairs(&report),
                vec![
                    ("portal-eis-text", DeliveryOutcome::Failed),
                    ("inbox", DeliveryOutcome::InboxFallback),
                ]
            );
            let expected = expected_mode_output(mode, input);
            assert_eq!(
                std::fs::read_to_string(inbox.join(format!("{}.txt", txid.as_str())))?,
                expected,
                "Unicode/multiline final text must survive byte-exact"
            );
            let entry = journal
                .read(txid.as_str())?
                .ok_or("missing Unicode routed journal entry")?;
            assert_eq!(entry.variants.delivered.as_deref(), Some(expected.as_str()));
            Ok(())
        }

        /// An unqualified backend is never dispatched and never claims
        /// insertion: the session still terminates with the durable Inbox
        /// recovery and a content-free bounded reason.
        #[test]
        fn strict_routed_unqualified_backend_stays_fail_closed()
        -> Result<(), Box<dyn std::error::Error>> {
            let dir = private_tempdir()?;
            let journal = UnixJournal::open(dir.path().join("journal"))?;
            let inbox = dir.path().join("inbox");
            let submissions = Arc::new(AtomicUsize::new(0));
            let mut delivery = platform(RoutedThenInboxDelivery::new(
                inbox.clone(),
                controlled_target(),
                Box::new(FixedPermission(DeliveryPermissionEvidence::default())),
                vec![Box::new(PortalEisTextRung::new(ScriptedPortalSubmitter {
                    submissions: Arc::clone(&submissions),
                    result: PortalEisTextSubmission::SubmittedUnverified,
                }))],
            ))?;
            let config = routed_test_config(Path::new("/controlled-routed"));
            let txid = TransactionId::new("tx-task009-routed-unqualified");
            let terminal = run_routed_batch(
                &journal,
                CorrectionMode::Raw,
                "unqualified   backend   text",
                &mut delivery,
                config
                    .strict_recovery_delivery_mode()
                    .ok_or("routed lane must resolve one delivery mode")?,
                &txid,
            )?;

            assert_eq!(
                submissions.load(Ordering::SeqCst),
                0,
                "an unqualified rung must never be dispatched"
            );
            let RecoveryTerminal::InboxRecovered(report) = terminal else {
                return Err("a fail-closed ladder must still recover durably".into());
            };
            assert_eq!(
                attempt_pairs(&report),
                vec![
                    ("portal-eis-text", DeliveryOutcome::Failed),
                    ("inbox", DeliveryOutcome::InboxFallback),
                ]
            );
            assert_eq!(
                report.attempts[0].fallback_reason,
                Some(FallbackReason::BackendError {
                    code: "permission_inactive".to_owned(),
                })
            );
            let entry = journal
                .read(txid.as_str())?
                .ok_or("missing fail-closed journal entry")?;
            assert_eq!(entry.state, JournalEntryState::InboxRecovered);
            assert!(inbox.join(format!("{}.txt", txid.as_str())).exists());
            Ok(())
        }

        /// Granting then revoking the attended permission changes only the
        /// qualification fact: the already-dispatched transaction stays a
        /// finished unverified submission, and the revoked attempt falls back
        /// to Inbox without a second dispatch.
        #[test]
        fn strict_routed_permission_revoke_produces_durable_inbox_fallback()
        -> Result<(), Box<dyn std::error::Error>> {
            let dir = private_tempdir()?;
            let journal = UnixJournal::open(dir.path().join("journal"))?;
            let inbox = dir.path().join("inbox");
            let submissions = Arc::new(AtomicUsize::new(0));
            let permission = Arc::new(Mutex::new(DeliveryPermissionEvidence {
                keyboard_insert: true,
                clipboard_restore: false,
            }));
            let mut delivery = platform(RoutedThenInboxDelivery::new(
                inbox.clone(),
                controlled_target(),
                Box::new(MutablePermission(Arc::clone(&permission))),
                vec![Box::new(PortalEisTextRung::new(ScriptedPortalSubmitter {
                    submissions: Arc::clone(&submissions),
                    result: PortalEisTextSubmission::SubmittedUnverified,
                }))],
            ))?;
            let config = routed_test_config(Path::new("/controlled-routed"));
            let delivery_mode = config
                .strict_recovery_delivery_mode()
                .ok_or("routed lane must resolve one delivery mode")?;

            let granted_txid = TransactionId::new("tx-task009-routed-granted");
            let granted = run_routed_batch(
                &journal,
                CorrectionMode::Clean,
                "granted   routed   text",
                &mut delivery,
                delivery_mode,
                &granted_txid,
            )?;
            assert!(matches!(granted, RecoveryTerminal::SubmittedUnverified(_)));
            assert_eq!(submissions.load(Ordering::SeqCst), 1);

            // The user revokes the grant between sessions.
            *permission
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                DeliveryPermissionEvidence::default();

            let revoked_txid = TransactionId::new("tx-task009-routed-revoked");
            let revoked = run_routed_batch(
                &journal,
                CorrectionMode::Clean,
                "revoked   routed   text",
                &mut delivery,
                delivery_mode,
                &revoked_txid,
            )?;
            let RecoveryTerminal::InboxRecovered(report) = revoked else {
                return Err("a revoked grant must fall back to durable Inbox recovery".into());
            };
            assert_eq!(
                submissions.load(Ordering::SeqCst),
                1,
                "a revoked grant must not dispatch"
            );
            assert_eq!(
                report.attempts[0].fallback_reason,
                Some(FallbackReason::BackendError {
                    code: "permission_inactive".to_owned(),
                })
            );
            let entry = journal
                .read(revoked_txid.as_str())?
                .ok_or("missing revoked journal entry")?;
            assert_eq!(entry.state, JournalEntryState::InboxRecovered);
            assert!(
                inbox
                    .join(format!("{}.txt", revoked_txid.as_str()))
                    .exists()
            );
            Ok(())
        }

        /// Cancellation before any dispatch is a clean terminal cancellation
        /// with no report and no recovery artifact; a stale retry of an already
        /// dispatched transaction receives the same facts and never inserts or
        /// stores a second time.
        #[test]
        fn strict_routed_cancellation_and_stale_retry_never_double_commit()
        -> Result<(), Box<dyn std::error::Error>> {
            let dir = private_tempdir()?;
            let inbox = dir.path().join("inbox");
            let submissions = Arc::new(AtomicUsize::new(0));
            let mut delivery = platform(RoutedThenInboxDelivery::new(
                inbox.clone(),
                controlled_target(),
                Box::new(FixedPermission(DeliveryPermissionEvidence {
                    keyboard_insert: true,
                    clipboard_restore: false,
                })),
                vec![Box::new(PortalEisTextRung::new(ScriptedPortalSubmitter {
                    submissions: Arc::clone(&submissions),
                    result: PortalEisTextSubmission::SubmittedUnverified,
                }))],
            ))?;

            // Cancellation before the ladder starts: nothing left the process.
            let cancelled = SessionCancellation::new();
            cancelled.cancel();
            let cancelled_intent = DeliveryIntent {
                transaction_id: TransactionId::new("tx-task009-routed-cancelled"),
                backend: "routed-then-inbox".to_owned(),
            };
            let error = delivery.deliver(&cancelled_intent, "cancelled text", &cancelled);
            match error {
                Err(error) => assert_eq!(error.code, "cancelled"),
                Ok(_) => {
                    return Err(
                        "a pre-cancelled delivery must not produce a delivery report".into(),
                    );
                }
            }
            assert_eq!(submissions.load(Ordering::SeqCst), 0);
            assert!(!inbox.join("tx-task009-routed-cancelled.txt").exists());

            // One real dispatch, then a stale retry of the same transaction.
            let active = SessionCancellation::new();
            let intent = DeliveryIntent {
                transaction_id: TransactionId::new("tx-task009-routed-retry"),
                backend: "routed-then-inbox".to_owned(),
            };
            let first = platform(delivery.deliver(&intent, "durable text", &active))?;
            let retry = platform(delivery.deliver(&intent, "durable text", &active))?;
            assert_eq!(
                submissions.load(Ordering::SeqCst),
                1,
                "a stale retry must never dispatch a second insertion attempt"
            );
            assert_eq!(retry, first);
            assert_eq!(first.backend, "routed-then-inbox");
            assert_eq!(
                attempt_pairs(&first),
                vec![("portal-eis-text", DeliveryOutcome::SubmittedUnverified)]
            );
            assert!(
                !inbox.join("tx-task009-routed-retry.txt").exists(),
                "a successful submission leaves no plaintext Inbox copy (F-M4)"
            );
            Ok(())
        }

        /// A rung that can verify its own durable result is the only path to a
        /// verified commit: the portable terminal and journal state differ from
        /// the attempted/unverified Inbox-recovery path, and no Inbox artifact
        /// is written for the verified transaction.
        #[test]
        fn strict_routed_verified_commit_is_distinct_from_attempted_insertion()
        -> Result<(), Box<dyn std::error::Error>> {
            let dir = private_tempdir()?;
            let journal = UnixJournal::open(dir.path().join("journal"))?;
            let inbox = dir.path().join("inbox");
            let attempts = Arc::new(AtomicUsize::new(0));
            let mut delivery = platform(RoutedThenInboxDelivery::new(
                inbox.clone(),
                controlled_target(),
                Box::new(FixedPermission(DeliveryPermissionEvidence::default())),
                vec![Box::new(ScriptedCommittedRung {
                    attempts: Arc::clone(&attempts),
                })],
            ))?;
            let config = routed_test_config(Path::new("/controlled-routed"));
            let txid = TransactionId::new("tx-task009-routed-verified");
            let input = "verified   commit   text";
            let terminal = run_routed_batch(
                &journal,
                CorrectionMode::Raw,
                input,
                &mut delivery,
                config
                    .strict_recovery_delivery_mode()
                    .ok_or("routed lane must resolve one delivery mode")?,
                &txid,
            )?;

            assert_eq!(attempts.load(Ordering::SeqCst), 1);
            let RecoveryTerminal::DeliveredInbox(report) = terminal else {
                return Err("a verified rung must terminate as a verified commit".into());
            };
            assert_eq!(report.outcome, DeliveryOutcome::Delivered);
            assert_eq!(
                attempt_pairs(&report),
                vec![("controlled-verified-backend", DeliveryOutcome::Delivered)]
            );
            let entry = journal
                .read(txid.as_str())?
                .ok_or("missing verified journal entry")?;
            assert_eq!(entry.state, JournalEntryState::Delivered);
            assert_eq!(
                entry.variants.delivered.as_deref(),
                Some(expected_mode_output(CorrectionMode::Raw, input).as_str())
            );
            assert!(
                !inbox.join(format!("{}.txt", txid.as_str())).exists(),
                "a verified commit must not claim an Inbox recovery artifact"
            );
            Ok(())
        }

        /// Raw and Clean must apply exactly their admitted deterministic strategy
        /// over the same composed capture/ASR/finalization/delivery tail, in both
        /// the batch and streaming slices, including Unicode and multiline input.
        /// Journal, active-session and partial identities must all name the same
        /// admitted mode.
        #[test]
        fn strict_modes_apply_exact_correction_consistently_in_batch_and_streaming()
        -> Result<(), Box<dyn std::error::Error>> {
            let inputs = [
                "hello   world  from   the   strict   mic",
                "  Grüße   aus  Zürich\nsecond   line\tand  spacing  ",
                "多言語 テスト   です\n日本語  mixed    with English",
            ];
            // One hardcoded FullCorrect corrector cannot satisfy this test:
            // every fixture must distinguish Raw from Clean.
            for input in inputs {
                assert_ne!(
                    expected_mode_output(CorrectionMode::Raw, input),
                    expected_mode_output(CorrectionMode::Clean, input),
                    "fixture {input:?} must separate the two modes"
                );
            }
            for input in inputs {
                for mode in CorrectionMode::ALL {
                    let admitted = strict_admitted(mode);
                    assert_eq!(admitted.id(), mode.pipeline_id());
                    assert!(admitted.defers_listening_until_capture_ready());
                    let expected = expected_mode_output(mode, input);
                    let dir = private_tempdir()?;
                    let journal = UnixJournal::open(dir.path().join("journal"))?;
                    let policy = PolicyEngine::from_document(PolicyDocument::default());

                    // --- Batch slice -------------------------------------------
                    let batch_txid = TransactionId::new(format!("tx-batch-{}", mode.pipeline_id()));
                    let batch_delivered = Arc::new(Mutex::new(Vec::new()));
                    let mut capture = ControlledStrictBatchCapture;
                    let mut asr = ControlledTranscriptAsr {
                        transcript: input.to_owned(),
                    };
                    let mut corrector = strict_composition_corrector(&admitted);
                    let mut delivery = ControlledInboxDelivery {
                        delivered: Arc::clone(&batch_delivered),
                    };
                    let mut states = Vec::new();
                    let terminal = RecoverySlice {
                        journal: &journal,
                        capture: &mut capture,
                        asr: &mut asr,
                        policy: &policy,
                        corrector: &mut corrector,
                        delivery: &mut delivery,
                        delivery_mode: RecoveryDeliveryMode::InboxPrimary,
                    }
                    .run(
                        SessionId(11),
                        batch_txid.clone(),
                        admitted.id().to_owned(),
                        &SessionControl::new(),
                        &mut |state| {
                            states.push(state);
                            Ok(())
                        },
                    )?;
                    assert!(matches!(terminal, RecoveryTerminal::DeliveredInbox(_)));
                    assert!(
                        states
                            .windows(2)
                            .any(|pair| pair
                                == [SessionState::Listening, SessionState::FinalizingAudio]),
                        "capture readiness must precede finalization"
                    );
                    let batch_entry = journal
                        .read(batch_txid.as_str())?
                        .ok_or("missing batch journal entry")?;
                    assert_eq!(batch_entry.pipeline_id.as_deref(), Some(admitted.id()));
                    assert_eq!(batch_entry.state, JournalEntryState::Delivered);
                    assert_eq!(batch_entry.variants.raw.as_deref(), Some(input));
                    assert_eq!(
                        batch_entry.variants.corrected.as_deref(),
                        Some(expected.as_str())
                    );
                    assert_eq!(
                        batch_entry.variants.delivered.as_deref(),
                        Some(expected.as_str())
                    );
                    assert_eq!(
                        batch_delivered
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .as_slice(),
                        [expected.as_str()],
                        "{mode:?} delivered text must be the exact corrected output"
                    );

                    // --- Streaming slice over the same daemon admission ---------
                    let state = DaemonState::new();
                    let (session_id, stream_txid) = state.begin_session(&journal, &admitted)?;
                    assert_eq!(
                        journal
                            .read(stream_txid.as_str())?
                            .and_then(|entry| entry.pipeline_id),
                        Some(admitted.id().to_owned())
                    );
                    assert_eq!(
                        state
                            .lock_sessions()
                            .get(&session_id)
                            .and_then(|slot| slot.pipeline_id.clone()),
                        Some(admitted.id().to_owned())
                    );
                    {
                        let sessions = state.lock_sessions();
                        assert_eq!(
                            sessions.get(&session_id).map(|slot| slot.machine.state()),
                            Some(SessionState::Preparing),
                            "a strict session must not claim Listening before readiness"
                        );
                    }

                    let stream_delivered = Arc::new(Mutex::new(Vec::new()));
                    let mut capture = ControlledStrictStreamCapture;
                    let mut asr = ControlledStreamingTranscriptAsr {
                        transcript: input.to_owned(),
                    };
                    let mut corrector = strict_composition_corrector(&admitted);
                    let mut delivery = ControlledInboxDelivery {
                        delivered: Arc::clone(&stream_delivered),
                    };
                    let stream_terminal = StreamingRecoverySlice {
                        journal: &journal,
                        capture: &mut capture,
                        asr: &mut asr,
                        policy: &policy,
                        corrector: &mut corrector,
                        delivery: &mut delivery,
                        delivery_mode: RecoveryDeliveryMode::InboxPrimary,
                        endpointing: None,
                        partial_observer: Some(&mut |partial| {
                            state.publish_partial(session_id, &stream_txid, admitted.id(), partial);
                            Ok(())
                        }),
                    }
                    .run(
                        SessionId(session_id),
                        stream_txid.clone(),
                        admitted.id().to_owned(),
                        &SessionControl::new(),
                        &mut |to| state.observe_recovery_state(session_id, &stream_txid, to, None),
                    )?;
                    assert!(matches!(
                        stream_terminal,
                        RecoveryTerminal::DeliveredInbox(_)
                    ));
                    let stream_entry = journal
                        .read(stream_txid.as_str())?
                        .ok_or("missing streaming journal entry")?;
                    assert_eq!(stream_entry.pipeline_id.as_deref(), Some(admitted.id()));
                    assert_eq!(stream_entry.state, JournalEntryState::Delivered);
                    assert_eq!(
                        stream_entry.variants, batch_entry.variants,
                        "batch and streaming must persist identical variants for one mode"
                    );
                    assert_eq!(
                        stream_delivered
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .as_slice(),
                        [expected.as_str()]
                    );
                    assert_eq!(
                        state
                            .lock_sessions()
                            .get(&session_id)
                            .map(|slot| slot.machine.state()),
                        Some(SessionState::Idle)
                    );

                    // Partial events and the recovery-result read must carry the
                    // same admitted mode identity.
                    let mut partial_identity_seen = false;
                    while let Some(event) = state.bus.try_next() {
                        match event {
                            DaemonEvent::Partial { pipeline_id, .. } => {
                                assert_eq!(pipeline_id, admitted.id());
                                partial_identity_seen = true;
                            }
                            DaemonEvent::Session { pipeline_id, .. } => {
                                assert_eq!(pipeline_id.as_deref(), Some(admitted.id()));
                            }
                            DaemonEvent::Delivered { .. }
                            | DaemonEvent::Capability { .. }
                            | DaemonEvent::AssistantDelta { .. }
                            | DaemonEvent::AssistantDone { .. }
                            | DaemonEvent::MeetingChanged { .. }
                            | DaemonEvent::Extension(_)
                            | DaemonEvent::Level { .. } => {}
                        }
                    }
                    assert!(
                        partial_identity_seen,
                        "the streaming slice must publish an identified partial"
                    );
                    let detail = recovery_entry_detail(&journal, stream_txid.as_str())?;
                    assert_eq!(detail.pipeline_id.as_deref(), Some(admitted.id()));
                    assert_eq!(detail.raw.as_deref(), Some(input));
                    assert_eq!(detail.corrected.as_deref(), Some(expected.as_str()));
                }
            }
            Ok(())
        }

        /// A different mode can never reopen strict single-session admission.
        #[test]
        fn active_strict_session_rejects_other_modes_before_side_effects() -> TestResult {
            let dir = private_tempdir()?;
            let journal = UnixJournal::open(dir.path().join("journal"))?;
            let state = Arc::new(DaemonState::new());
            let supervisor = Arc::new(WorkerSupervisor::new());
            // `transcribe_resident: false` keeps the live-worker gate trivially
            // satisfied while preserving the strict lane identity, permit and
            // capture-ready timing under test.
            let config = EngineConfig {
                recovery_linux_mic_inbox: true,
                ..EngineConfig::new()
            };

            let first = admit_session_request(
                &state,
                &journal,
                &strict_admitted(CorrectionMode::Raw),
                &config,
                &supervisor,
            )
            .map_err(|(_, detail)| detail)?;
            let (first_session, first_txid, first_permit) = first;
            assert!(first_permit.is_some());
            assert_eq!(
                journal
                    .read(first_txid.as_str())?
                    .and_then(|entry| entry.pipeline_id),
                Some(RAW_PIPELINE_ID.to_owned())
            );

            let rejected = admit_session_request(
                &state,
                &journal,
                &strict_admitted(CorrectionMode::Clean),
                &config,
                &supervisor,
            );
            assert!(
                matches!(rejected, Err((IpcErrorCode::Busy, _))),
                "a different mode must not bypass strict single-session admission"
            );
            assert_eq!(state.lock_sessions().len(), 1);
            assert_eq!(journal.entries()?.len(), 1);
            assert_eq!(supervisor.live_worker_count(), 0);

            drop(first_permit);
            let admitted_after_release = admit_session_request(
                &state,
                &journal,
                &strict_admitted(CorrectionMode::Clean),
                &config,
                &supervisor,
            )
            .map_err(|(_, detail)| detail);
            assert!(
                admitted_after_release.is_ok(),
                "releasing the terminal permit must reopen admission"
            );
            assert_eq!(state.lock_sessions().len(), 2);
            assert_ne!(
                state
                    .lock_sessions()
                    .get(&first_session)
                    .and_then(|slot| slot.pipeline_id.clone()),
                None
            );
            Ok(())
        }
    }
}

#[cfg(test)]
mod delivery_result_tests {
    use super::*;

    fn attempt(backend: &str, outcome: DeliveryOutcome) -> DeliveryAttempt {
        DeliveryAttempt {
            backend: backend.to_owned(),
            outcome,
            fallback_reason: None,
            at_unix_ms: 0,
        }
    }

    #[test]
    fn the_result_names_the_rung_that_dispatched_the_text() {
        let skipped = attempt("portal-eis-text", DeliveryOutcome::Failed);
        let inbox = attempt("inbox", DeliveryOutcome::InboxFallback);
        let cases = [
            (
                vec![
                    skipped.clone(),
                    attempt("clipboard-paste", DeliveryOutcome::SubmittedUnverified),
                    inbox.clone(),
                ],
                "pasted",
            ),
            (
                vec![
                    attempt("clipboard-paste", DeliveryOutcome::Failed),
                    attempt(
                        "wtype-virtual-keyboard",
                        DeliveryOutcome::SubmittedUnverified,
                    ),
                    inbox.clone(),
                ],
                "typed",
            ),
            (
                vec![
                    attempt("clipboard-copy", DeliveryOutcome::CopiedOnly),
                    inbox.clone(),
                ],
                "copied",
            ),
            (
                vec![attempt("portal-eis-text", DeliveryOutcome::Delivered)],
                "inserted",
            ),
            (vec![skipped, inbox], "inbox"),
        ];
        for (attempts, expected) in cases {
            assert_eq!(delivery_result_of(&attempts), expected);
        }
    }
}

#[cfg(test)]
mod live_share_tests {
    use super::{live_instructions, recording_instructions, without_own_voice};

    #[test]
    fn a_live_share_tells_the_model_what_it_sees_and_heard() {
        let text = live_instructions(Some(true), "[0:05] Call 1: Welcome to the demo.");
        assert!(text.contains("what the screen shows at this moment"));
        assert!(text.contains("[0:05] Call 1: Welcome to the demo.\n(end of"));
        let other = recording_instructions("Standup", "[0:01] Room 1: Hello.");
        assert!(other.contains("recording \"Standup\""));
        assert!(other.contains("[0:01] Room 1: Hello."));
        let heard = live_instructions(None, "[0:02] Call 1: Welcome back to the show.");
        assert!(heard.contains("cannot see their screen"));
        assert!(heard.contains("never say you cannot hear"));
        assert!(!heard.contains("picture"));
        let early = live_instructions(Some(false), " ");
        assert!(early.contains("No picture of the screen has arrived yet"));
        assert!(early.contains("Nothing has been transcribed yet"));
    }

    #[test]
    fn the_assistants_own_voice_is_not_in_the_transcript() {
        let said = "[0:19] Person 1: can see a YouTube video page showing a video about the missile test.\n\
[0:48] Person 2: Today we look at the battery life of the new laptop.";
        let answers =
            ["I can see a YouTube video page showing a video about the missile being tested."];
        let kept = without_own_voice(said, &answers);
        assert_eq!(
            kept,
            "[0:48] Person 2: Today we look at the battery life of the new laptop."
        );
    }
}
