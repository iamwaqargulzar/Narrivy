//! Engine composition, pipeline policy and recovery-journal helpers.

#![forbid(unsafe_code)]
#![allow(unused_imports)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use narrivy_core::{Journal, JournalEntryState, RecoveryDeliveryMode};
use narrivy_ipc::protocol::{CapabilityCheckResultDto, CaptureEndDto, RecoveryEntryDetailDto};
use narrivy_journal_api::RecoveryJournal;
use narrivy_journal_unix::{RetainedUnixJournal, UnixJournal};
use narrivy_pipeline::{
    CapabilitySlot, PipelineDefinition, PipelineEdge, PipelineNode, PipelineRegistry, ResourceRef,
};
use narrivy_platform_linux::recovery::{PipeWireRecorderStatus, WtypeSubmitterStatus};

use super::capture::AudioSourceKind;

#[cfg(test)]
fn private_tempdir() -> std::io::Result<tempfile::TempDir> {
    let dir = tempfile::tempdir()?;
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
    Ok(dir)
}

// ---------------------------------------------------------------------------
// Pipeline / correction policy constants
// ---------------------------------------------------------------------------

/// The one pipeline this build serves (mock ASR -> deterministic corrector
/// -> null delivery).
pub const DEFAULT_PIPELINE_ID: &str = "mock-dictation";

/// Historical Recovery Phase 2A **composition lane** identity. This value is
/// selected by `NARRIVY_RECOVERY_MODE` and chooses the owned Linux microphone
/// capture plus Inbox-primary delivery path. It is not a selectable or
/// advertised pipeline id: the built-in dictation modes ([`CorrectionMode`])
/// are the pipeline identities that execute over this lane.
pub const LINUX_MIC_INBOX_PIPELINE_ID: &str = "linux-mic-inbox";

/// Historical Recovery Phase 2B **composition lane** identity. Unlike the
/// Phase 2A Inbox-primary lane, its wtype attempt is explicitly unverified and
/// Inbox remains the durable terminal recovery path. Like Phase 2A it is
/// composition configuration, not a pipeline identity.
pub const LINUX_MIC_WTYPE_INBOX_PIPELINE_ID: &str = "linux-mic-wtype-inbox";

/// Production target-aware routed **composition lane** identity. It composes
/// the owned Linux microphone capture and installed resident ASR of the strict
/// lanes with the approved target-aware delivery router
/// ([`narrivy_platform_linux::delivery::RoutedThenInboxDelivery`]): only
/// qualified compatible backends may be attempted, and exactly one durable
/// Inbox recovery write under the session transaction identity is mandatory
/// for every outcome that is not a verified insertion.
///
/// Like the other two lane ids this is process composition configuration
/// selected by `NARRIVY_RECOVERY_MODE`; it is never a selectable, advertised,
/// journaled or evented pipeline identity. The canonical built-in modes
/// ([`CorrectionMode`]) remain the only pipeline identities over it.
pub const LINUX_MIC_ROUTED_INBOX_LANE_ID: &str = "linux-mic-routed-inbox";

/// P7 built-in Raw mode: verbatim engine text with whitespace collapse ONLY
/// (no punctuation, capitalization, or ITN changes beyond the collapse).
pub const RAW_PIPELINE_ID: &str = "raw";

/// P7 built-in Clean mode: full deterministic corrector stage (same output
/// as the historical `correct::correct` path).
pub const CLEAN_PIPELINE_ID: &str = "clean";

/// Optional env override for the pipeline used when StartSession carries no
/// explicit `pipeline_id`. Must name a known pipeline; anything else is a
/// hard startup-adjacent request error (never a silent lane switch).
pub const DEFAULT_PIPELINE_ENV: &str = "NARRIVY_DEFAULT_PIPELINE";

/// Fixed engine fixture transcribed for every session.
pub const MOCK_TRANSCRIPT: &str = "hello world from mock asr";

/// Upper bound of the controlled `NARRIVY_CAPTURE_SECONDS` developer probe.
///
/// Ordinary dictation is Stop-driven and composes the documented portable
/// maximum ([`narrivy_types::MAX_CAPTURE_SECONDS`]). This environment hook is a
/// controlled short-window override used by deterministic fixture probes; it
/// can only shorten one session and can never extend capture past the
/// documented production bound.
pub const DEVELOPER_CAPTURE_SECONDS_MAX: f32 = 30.0;

/// Validate and apply the controlled `NARRIVY_CAPTURE_SECONDS` developer
/// override. A probe may only shorten one session inside
/// `[1, DEVELOPER_CAPTURE_SECONDS_MAX]`; the production window stays the
/// documented portable maximum and neither extreme nor non-finite input is
/// silently accepted.
fn apply_developer_capture_seconds(window: &mut f32, raw: &str) -> Result<(), String> {
    let parsed = raw
        .parse::<f32>()
        .map_err(|_| "NARRIVY_CAPTURE_SECONDS must be a number".to_owned())?;
    if !parsed.is_finite() || !(1.0..=DEVELOPER_CAPTURE_SECONDS_MAX).contains(&parsed) {
        return Err(format!(
            "NARRIVY_CAPTURE_SECONDS is a controlled short-window developer override and must be between 1 and {DEVELOPER_CAPTURE_SECONDS_MAX}"
        ));
    }
    *window = parsed;
    Ok(())
}

// ---------------------------------------------------------------------------
// Correction strategy
// ---------------------------------------------------------------------------

/// Per-pipeline correction strategy (P7 FUN-021). The strategy — not the UI —
/// decides how the Transforming stage rewrites the raw transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorrectionStrategy {
    /// Raw mode: collapse repeated whitespace only. Engine text is passed
    /// through verbatim otherwise.
    CollapseOnly,
    /// Clean mode: full deterministic corrector (`narrivyd::correct::correct`).
    FullCorrect,
}

impl CorrectionStrategy {
    /// Resolve the deterministic correction for one historical **non-strict**
    /// pipeline id (mock dictation or a mock-composition mode alias). Strict
    /// microphone compositions resolve their canonical [`CorrectionMode`] into
    /// an [`AdmittedPipeline`] instead; their lane ids are composition
    /// configuration and never resolve to a selectable pipeline here.
    /// `None` (no explicit id) falls back to [`default_pipeline_id`].
    /// Returns `None` for unknown ids so callers reject them explicitly.
    pub fn resolve(pipeline_id: Option<&str>) -> Option<Self> {
        let id = match pipeline_id {
            Some(id) => id,
            None => return Self::resolve(Some(&default_pipeline_id())),
        };
        match id {
            RAW_PIPELINE_ID => Some(Self::CollapseOnly),
            DEFAULT_PIPELINE_ID | CLEAN_PIPELINE_ID => Some(Self::FullCorrect),
            _ => None,
        }
    }

    pub fn applies(&self, raw: &str) -> String {
        match self {
            Self::CollapseOnly => collapse_only(raw),
            Self::FullCorrect => narrivyd::correct::correct(raw),
        }
    }
}

/// Canonical built-in dictation modes (FUN-021). Modes ARE pipelines: the mode
/// id is the pipeline identity recorded in the recovery journal, published on
/// session and partial events, and advertised over IPC. Raw and Clean execute
/// over the same owned capture -> ASR -> privacy -> durable finalization ->
/// delivery path; they differ only in the deterministic correction stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorrectionMode {
    /// Verbatim engine text with whitespace collapse only.
    Raw,
    /// Full deterministic corrector (`narrivyd::correct::correct`).
    Clean,
}

impl CorrectionMode {
    /// Advertisement order for one strict composition. The first entry is the
    /// mode an id-less request resolves to, so a fresh client selecting the
    /// first enabled advertised entry starts the daemon's default mode.
    pub const ALL: [Self; 2] = [Self::Clean, Self::Raw];

    #[must_use]
    pub const fn pipeline_id(self) -> &'static str {
        match self {
            Self::Raw => RAW_PIPELINE_ID,
            Self::Clean => CLEAN_PIPELINE_ID,
        }
    }

    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Raw => "Raw dictation",
            Self::Clean => "Clean dictation",
        }
    }

    #[must_use]
    pub const fn strategy(self) -> CorrectionStrategy {
        match self {
            Self::Raw => CorrectionStrategy::CollapseOnly,
            Self::Clean => CorrectionStrategy::FullCorrect,
        }
    }

    /// Exact canonical mode id, or `None` for anything else (including the
    /// historical lane ids and the legacy mock lane id).
    #[must_use]
    pub fn from_pipeline_id(id: &str) -> Option<Self> {
        match id {
            RAW_PIPELINE_ID => Some(Self::Raw),
            CLEAN_PIPELINE_ID => Some(Self::Clean),
            _ => None,
        }
    }

    /// Mode for a request that carries no explicit pipeline id: the
    /// `NARRIVY_DEFAULT_PIPELINE` hook when it names a canonical mode,
    /// otherwise the behavior-preserving Clean mode that the strict lane
    /// applied before modes were selectable.
    #[must_use]
    pub fn default_for_request() -> Self {
        Self::from_pipeline_id(&default_pipeline_id()).unwrap_or(Self::Clean)
    }
}

/// Where one admitted pipeline executes. This selects orchestration timing
/// only; it never carries OS policy into a portable contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineLane {
    /// Historical in-process mock composition with no owned live capture.
    Mock,
    /// Owned Linux microphone composition. Its `Listening` transition is
    /// deferred until the capture adapter reports a complete first frame.
    StrictCapture,
}

/// The single pipeline admission decision for one session.
///
/// `select_pipeline` resolves this exactly once from the process composition
/// plus the requested id. Admission, journaling, session and partial events,
/// batch orchestration and the owned install/resident worker then all consume
/// this same value, so a mode can never be admitted with one correction
/// strategy and executed with another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmittedPipeline {
    id: String,
    correction: CorrectionStrategy,
    lane: PipelineLane,
}

impl AdmittedPipeline {
    /// One canonical built-in mode over the owned microphone composition.
    #[must_use]
    pub fn strict_capture(mode: CorrectionMode) -> Self {
        Self {
            id: mode.pipeline_id().to_owned(),
            correction: mode.strategy(),
            lane: PipelineLane::StrictCapture,
        }
    }

    /// One historical non-strict pipeline id (mock dictation or a
    /// mock-composition mode alias) with its resolved correction strategy.
    #[must_use]
    pub fn mock_lane(id: impl Into<String>, correction: CorrectionStrategy) -> Self {
        Self {
            id: id.into(),
            correction,
            lane: PipelineLane::Mock,
        }
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub fn correction(&self) -> CorrectionStrategy {
        self.correction
    }

    #[must_use]
    pub fn lane(&self) -> PipelineLane {
        self.lane
    }

    #[must_use]
    pub fn defers_listening_until_capture_ready(&self) -> bool {
        self.lane == PipelineLane::StrictCapture
    }
}

/// Default pipeline id from an explicit override value (pure; testable).
pub fn default_pipeline_id_from(env_value: Option<&str>) -> String {
    match env_value {
        Some(v) if CorrectionStrategy::resolve(Some(v)).is_some() => v.to_owned(),
        _ => DEFAULT_PIPELINE_ID.to_owned(),
    }
}

/// Default pipeline id: `NARRIVY_DEFAULT_PIPELINE` when it names a known
/// pipeline, else the historical mock-dictation default.
pub fn default_pipeline_id() -> String {
    default_pipeline_id_from(std::env::var(DEFAULT_PIPELINE_ENV).ok().as_deref())
}

/// Raw-mode deterministic formatting: whitespace collapse ONLY. No
/// punctuation insertion, no capitalization, no ITN. Honest definition of
/// "verbatim corrected-for-spacing" (FUN-027 deterministic lane).
pub fn collapse_only(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ---------------------------------------------------------------------------
// P2 real-path configuration (env hooks)
// ---------------------------------------------------------------------------

/// Selected ASR engine (`NARRIVY_ASR_ENGINE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsrEngineKind {
    /// Deterministic fixture engine (P1 contract; the default).
    Mock,
    /// Real local ASR through the pinned transcribe.cpp sidecar.
    TranscribeCpp,
}

/// Delivery backend (`NARRIVY_DELIVERY_BACKEND`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryBackendKind {
    /// P1 null sink: text observed via sink + recovery journal only.
    NullSink,
    /// Recovery 2A: atomic Inbox persistence only. No focused-app, clipboard,
    /// wtype, or uinput route is considered.
    InboxOnly,
    /// Recovery 2B: one unverified stdin-only wtype attempt followed by
    /// mandatory durable Inbox recovery; no legacy router is involved.
    WtypeThenInbox,
    /// Production target-aware routed delivery: the approved bounded ladder of
    /// qualified compatible backends with exactly one mandatory durable Inbox
    /// recovery write for every outcome that is not a verified insertion. Only
    /// the strict routed composition lane selects this backend.
    RoutedThenInbox,
}

/// One strict owned-capture **composition lane**. Lanes are process
/// configuration (`NARRIVY_RECOVERY_MODE`); they select capture plus delivery
/// composition and are never pipeline identities. The selectable modes over
/// every lane are the canonical built-in modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrictRecoveryLane {
    /// Phase 2A Inbox-primary composition (unchanged meaning).
    InboxOnly,
    /// Phase 2B wtype-then-Inbox composition (unchanged meaning).
    WtypeThenInbox,
    /// Production target-aware routed composition with mandatory durable
    /// Inbox recovery.
    TargetRouted,
}

impl StrictRecoveryLane {
    /// The `NARRIVY_RECOVERY_MODE` value this lane is selected by. Readback
    /// only: the lane id is never advertised as a pipeline.
    #[must_use]
    pub const fn recovery_mode_id(self) -> &'static str {
        match self {
            Self::InboxOnly => LINUX_MIC_INBOX_PIPELINE_ID,
            Self::WtypeThenInbox => LINUX_MIC_WTYPE_INBOX_PIPELINE_ID,
            Self::TargetRouted => LINUX_MIC_ROUTED_INBOX_LANE_ID,
        }
    }
}

/// Immutable per-process composition choices. Defaults reproduce the P1
/// mock-dictation contract exactly; `from_env` is called once in `main`.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Opt-in strict recovery composition lane selected by
    /// `NARRIVY_RECOVERY_MODE`: owned Linux microphone capture plus
    /// Inbox-primary delivery. The selectable pipelines over this lane are the
    /// canonical built-in modes (`raw`, `clean`); the lane id itself is never a
    /// pipeline.
    pub recovery_linux_mic_inbox: bool,
    /// Opt-in strict Recovery 2B composition lane. It never aliases Phase 2A
    /// and exposes the same canonical modes over wtype-then-Inbox recovery.
    pub recovery_linux_mic_wtype_inbox: bool,
    /// Opt-in strict target-aware routed composition lane
    /// (`NARRIVY_RECOVERY_MODE=linux-mic-routed-inbox`). It never aliases the
    /// Phase 2A/2B lanes and exposes the same canonical modes over the approved
    /// delivery ladder with mandatory durable Inbox recovery.
    pub recovery_linux_mic_routed_inbox: bool,
    /// Explicit authorization flag for the clipboard privacy boundary of the
    /// routed ladder (`NARRIVY_CLIPBOARD_AUTHORIZED=1`): clipboard paste reads
    /// the user's clipboard to put it back. With it, the ladder composes the
    /// clipboard-paste rung through the Wayland data-control protocol and the
    /// trusted wtype (D29). The legacy PATH-resolved
    /// `wl-copy`/`wl-paste`/`xclip` implementations are still never composed.
    /// The user's delivery method supersedes it once chosen (D37,
    /// `text_options::paste`), for each dictation and for the status tokens.
    pub clipboard_authorized: bool,
    pub asr_engine: AsrEngineKind,
    pub audio_source: AudioSourceKind,
    pub delivery_backend: DeliveryBackendKind,
    /// Force the delivery closure to fail (typed InboxFallback proof).
    pub force_delivery_failure: bool,
    /// Effective bounded live-capture window in seconds (pipewire source only).
    /// Ordinary dictation uses the documented production maximum and ends on
    /// Stop/endpoint/cancel; the `NARRIVY_CAPTURE_SECONDS` environment hook is a
    /// controlled developer probe that may only shorten this window.
    pub pipewire_capture_seconds: f32,
    /// GGUF model path override (transcribe-cpp engine only).
    pub transcribe_model_path: Option<PathBuf>,
    /// Exact controlled model identity required by the resident runtime.
    pub transcribe_model_sha256: Option<String>,
    pub transcribe_model_size: Option<u64>,
    /// Recovery 4B explicit resident-worker selection. False retains the
    /// versioned CLI sidecar as the rollback path.
    pub transcribe_resident: bool,
    /// Installed-runtime composition selected explicitly at startup. This
    /// uses a fixed root-owned worker manifest plus a registry model ID and
    /// rejects raw worker/model identity environment values.
    pub transcribe_installed: bool,
    pub transcribe_model_id: Option<String>,
    /// Inbox directory handed to the platform router (set by main after the
    /// runtime dir resolves; None keeps the router inbox-free).
    pub inbox_dir: Option<PathBuf>,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            asr_engine: AsrEngineKind::Mock,
            recovery_linux_mic_inbox: false,
            recovery_linux_mic_wtype_inbox: false,
            recovery_linux_mic_routed_inbox: false,
            clipboard_authorized: false,
            audio_source: AudioSourceKind::SineFixture,
            delivery_backend: DeliveryBackendKind::NullSink,
            force_delivery_failure: false,
            pipewire_capture_seconds: narrivy_types::MAX_CAPTURE_SECONDS as f32,
            transcribe_model_path: None,
            transcribe_model_sha256: None,
            transcribe_model_size: None,
            transcribe_resident: false,
            transcribe_installed: false,
            transcribe_model_id: None,
            inbox_dir: None,
        }
    }
}

impl EngineConfig {
    /// Pure defaults; used by tests and the legacy handler constructor so no
    /// ambient environment variable can silently change test behavior.
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads + validates the P2 env hooks. Unknown values are hard startup
    /// errors — the daemon never silently falls back to a different lane.
    pub fn from_env() -> Result<Self, String> {
        let mut cfg = Self::default();
        if let Some(v) = std::env::var_os("NARRIVY_RECOVERY_MODE") {
            match v.to_string_lossy().as_ref() {
                "linux-mic-inbox" => cfg.recovery_linux_mic_inbox = true,
                "linux-mic-wtype-inbox" => cfg.recovery_linux_mic_wtype_inbox = true,
                "linux-mic-routed-inbox" => cfg.recovery_linux_mic_routed_inbox = true,
                _ => return Err("NARRIVY_RECOVERY_MODE has an unsupported value".to_owned()),
            }
        }
        if let Some(v) = std::env::var_os("NARRIVY_ASR_ENGINE") {
            cfg.asr_engine = match v.to_string_lossy().as_ref() {
                "mock" => AsrEngineKind::Mock,
                "transcribe-cpp" => {
                    #[cfg(feature = "asr-transcribe-cpp")]
                    {
                        AsrEngineKind::TranscribeCpp
                    }
                    #[cfg(not(feature = "asr-transcribe-cpp"))]
                    {
                        return Err(
                            "NARRIVY_ASR_ENGINE=transcribe-cpp but this binary was built                              without the asr-transcribe-cpp cargo feature"
                                .to_owned(),
                        );
                    }
                }
                _ => {
                    return Err(
                        "NARRIVY_ASR_ENGINE has an invalid value (expected mock|transcribe-cpp)"
                            .to_owned(),
                    );
                }
            };
        } else {
            // A hand-started daemon must never return the mock engine's fake
            // transcripts (F-M13): the mock is chosen explicitly. The
            // installed unit and `scripts/dev-daemon.sh` set the real engine.
            return Err(
                "NARRIVY_ASR_ENGINE is not set (expected mock|transcribe-cpp); start Narrivy \
                 with `systemctl --user start narrivy-daemon`"
                    .to_owned(),
            );
        }
        if let Some(v) = std::env::var_os("NARRIVY_AUDIO_SOURCE") {
            cfg.audio_source = match v.to_string_lossy().as_ref() {
                "sine-fixture" => AudioSourceKind::SineFixture,
                "pipewire" => AudioSourceKind::PipeWire,
                other if other.starts_with("file:") => {
                    let path = PathBuf::from(&other[5..]);
                    if !path.is_file() {
                        return Err(
                            "NARRIVY_AUDIO_SOURCE names a file that does not exist".to_owned()
                        );
                    }
                    AudioSourceKind::FileWav(path)
                }
                _ => {
                    return Err("NARRIVY_AUDIO_SOURCE has an invalid value (expected sine-fixture|pipewire|file:<wav>)".to_owned());
                }
            };
        }
        #[cfg(not(feature = "asr-transcribe-cpp"))]
        if cfg.asr_engine == AsrEngineKind::TranscribeCpp {
            return Err(
                "NARRIVY_ASR_ENGINE=transcribe-cpp requires a daemon built with --features asr-transcribe-cpp"
                    .to_owned(),
            );
        }
        if let Some(v) = std::env::var_os("NARRIVY_DELIVERY_BACKEND") {
            cfg.delivery_backend = match v.to_string_lossy().as_ref() {
                "null" => DeliveryBackendKind::NullSink,
                "inbox" => DeliveryBackendKind::InboxOnly,
                "wtype-inbox" => DeliveryBackendKind::WtypeThenInbox,
                "routed-inbox" => DeliveryBackendKind::RoutedThenInbox,
                _ => {
                    return Err("NARRIVY_DELIVERY_BACKEND has an invalid value (expected null|inbox|wtype-inbox|routed-inbox)".to_owned());
                }
            };
        }
        if let Some(v) = std::env::var_os("NARRIVY_CLIPBOARD_AUTHORIZED") {
            cfg.clipboard_authorized = match v.to_string_lossy().as_ref() {
                "1" | "true" | "yes" => true,
                "0" | "false" | "" => false,
                _ => {
                    return Err("NARRIVY_CLIPBOARD_AUTHORIZED has an invalid value (expected 0|1|true|false)".to_owned());
                }
            };
        }
        if let Some(v) = std::env::var_os("NARRIVY_DELIVERY_FAIL") {
            cfg.force_delivery_failure = match v.to_string_lossy().as_ref() {
                "1" | "true" | "yes" => true,
                "0" | "false" | "" => false,
                _ => {
                    return Err(
                        "NARRIVY_DELIVERY_FAIL has an invalid value (expected 0|1|true|false)"
                            .to_owned(),
                    );
                }
            };
        }
        if let Some(v) = std::env::var_os("NARRIVY_CAPTURE_SECONDS") {
            let raw = v.to_string_lossy();
            apply_developer_capture_seconds(&mut cfg.pipewire_capture_seconds, &raw)?;
        }
        if let Some(v) = std::env::var_os("NARRIVY_TRANSCRIBE_MODEL") {
            cfg.transcribe_model_path = Some(PathBuf::from(v));
        }
        if let Some(v) = std::env::var_os("NARRIVY_TRANSCRIBE_MODEL_SHA256") {
            let digest = v
                .into_string()
                .map_err(|_| "NARRIVY_TRANSCRIBE_MODEL_SHA256 must be UTF-8 hex".to_owned())?;
            if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(
                    "NARRIVY_TRANSCRIBE_MODEL_SHA256 must be a 64-character SHA-256 hex digest"
                        .to_owned(),
                );
            }
            cfg.transcribe_model_sha256 = Some(digest);
        }
        if let Some(v) = std::env::var_os("NARRIVY_TRANSCRIBE_MODEL_SIZE") {
            let size = v
                .to_string_lossy()
                .parse::<u64>()
                .map_err(|_| "NARRIVY_TRANSCRIBE_MODEL_SIZE must be an integer".to_owned())?;
            if size == 0 {
                return Err("NARRIVY_TRANSCRIBE_MODEL_SIZE must be positive".to_owned());
            }
            cfg.transcribe_model_size = Some(size);
        }
        if let Some(v) = std::env::var_os("NARRIVY_TRANSCRIBE_RUNTIME") {
            match v.to_string_lossy().as_ref() {
                "sidecar" => cfg.transcribe_resident = false,
                "resident" => {
                    #[cfg(feature = "asr-transcribe-resident")]
                    {
                        cfg.transcribe_resident = true;
                    }
                    #[cfg(not(feature = "asr-transcribe-resident"))]
                    {
                        return Err(
                            "NARRIVY_TRANSCRIBE_RUNTIME=resident requires a daemon built with --features asr-transcribe-resident"
                                .to_owned(),
                        );
                    }
                }
                "installed" => {
                    #[cfg(feature = "asr-transcribe-resident")]
                    {
                        cfg.transcribe_resident = true;
                        cfg.transcribe_installed = true;
                    }
                    #[cfg(not(feature = "asr-transcribe-resident"))]
                    {
                        return Err(
                            "NARRIVY_TRANSCRIBE_RUNTIME=installed requires a daemon built with --features asr-transcribe-resident"
                                .to_owned(),
                        );
                    }
                }
                _ => {
                    return Err("NARRIVY_TRANSCRIBE_RUNTIME has an invalid value (expected sidecar|resident|installed)".to_owned());
                }
            }
        }
        if let Some(v) = std::env::var_os("NARRIVY_TRANSCRIBE_MODEL_ID") {
            let value = v
                .into_string()
                .map_err(|_| "NARRIVY_TRANSCRIBE_MODEL_ID must be UTF-8".to_owned())?;
            if value.is_empty()
                || value.len() > 128
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
            {
                return Err("NARRIVY_TRANSCRIBE_MODEL_ID is invalid".to_owned());
            }
            cfg.transcribe_model_id = Some(value);
        }
        if cfg.transcribe_installed
            && (cfg.transcribe_model_path.is_some()
                || cfg.transcribe_model_sha256.is_some()
                || cfg.transcribe_model_size.is_some()
                || cfg.transcribe_model_id.is_none()
                || installed_worker_identity_is_ambient(|name| std::env::var_os(name).is_some()))
        {
            return Err("installed resident ASR requires only NARRIVY_TRANSCRIBE_MODEL_ID; raw worker or model path, digest, and size are forbidden".to_owned());
        }
        if cfg.is_strict_recovery()
            && !strict_default_pipeline_is_valid(
                std::env::var(DEFAULT_PIPELINE_ENV).ok().as_deref(),
            )
        {
            return Err(
                "a strict recovery composition requires NARRIVY_DEFAULT_PIPELINE to name the built-in raw or clean mode"
                    .to_owned(),
            );
        }
        match cfg.validate_strict_recovery_configuration() {
            Ok(()) => {}
            Err("strict_recovery_modes_conflict") => {
                return Err(
                    "only one strict recovery mode may be selected for a daemon process".to_owned(),
                );
            }
            Err("linux_mic_inbox_configuration_invalid") => {
                return Err("linux-mic-inbox requires PipeWire, transcribe.cpp, an explicit GGUF model, Inbox-only delivery, and no forced delivery failure".to_owned());
            }
            Err("linux_mic_wtype_inbox_configuration_invalid") => {
                return Err("linux-mic-wtype-inbox requires PipeWire, transcribe.cpp, an explicit GGUF model, wtype-Inbox recovery delivery, and no forced delivery failure".to_owned());
            }
            Err("linux_mic_routed_inbox_configuration_invalid") => {
                return Err("linux-mic-routed-inbox requires PipeWire, transcribe.cpp, an explicit GGUF model, routed-Inbox delivery, and no forced delivery failure".to_owned());
            }
            Err("routed_inbox_requires_routed_lane") => {
                return Err("NARRIVY_DELIVERY_BACKEND=routed-inbox requires NARRIVY_RECOVERY_MODE=linux-mic-routed-inbox".to_owned());
            }
            Err("non_strict_pipewire_disabled") => {
                return Err(
                    "NARRIVY_AUDIO_SOURCE=pipewire requires an owned strict NARRIVY_RECOVERY_MODE"
                        .to_owned(),
                );
            }
            Err("installed_resident_configuration_invalid") => {
                return Err("installed resident ASR requires transcribe.cpp, one registered model ID, and no raw worker or model identity".to_owned());
            }
            Err(_) => return Err("strict recovery configuration is invalid".to_owned()),
        }
        Ok(cfg)
    }

    /// Validate only static composition choices. This runs both at startup
    /// and at readiness time so an in-process caller cannot construct a
    /// strict lane that silently substitutes a mock, fixture, null sink, or
    /// the broad platform router.
    fn validate_strict_recovery_configuration(&self) -> Result<(), &'static str> {
        if self.transcribe_installed
            && (self.asr_engine != AsrEngineKind::TranscribeCpp
                || !self.transcribe_resident
                || self.transcribe_model_id.is_none()
                || self.transcribe_model_path.is_some()
                || self.transcribe_model_sha256.is_some()
                || self.transcribe_model_size.is_some())
        {
            return Err("installed_resident_configuration_invalid");
        }
        if self.recovery_linux_mic_inbox && self.recovery_linux_mic_wtype_inbox {
            return Err("strict_recovery_modes_conflict");
        }
        if self.recovery_linux_mic_routed_inbox
            && (self.recovery_linux_mic_inbox || self.recovery_linux_mic_wtype_inbox)
        {
            return Err("strict_recovery_modes_conflict");
        }
        if self.audio_source == AudioSourceKind::PipeWire && !self.is_strict_recovery() {
            return Err("non_strict_pipewire_disabled");
        }
        if self.recovery_linux_mic_inbox
            && (self.asr_engine != AsrEngineKind::TranscribeCpp
                || self.audio_source != AudioSourceKind::PipeWire
                || self.delivery_backend != DeliveryBackendKind::InboxOnly
                || self.force_delivery_failure
                || (self.transcribe_model_path.is_none()
                    && !(self.transcribe_installed && self.transcribe_model_id.is_some())))
        {
            return Err("linux_mic_inbox_configuration_invalid");
        }
        if self.recovery_linux_mic_wtype_inbox
            && (self.asr_engine != AsrEngineKind::TranscribeCpp
                || self.audio_source != AudioSourceKind::PipeWire
                || self.delivery_backend != DeliveryBackendKind::WtypeThenInbox
                || self.force_delivery_failure
                || (self.transcribe_model_path.is_none()
                    && !(self.transcribe_installed && self.transcribe_model_id.is_some())))
        {
            return Err("linux_mic_wtype_inbox_configuration_invalid");
        }
        if self.recovery_linux_mic_routed_inbox
            && (self.asr_engine != AsrEngineKind::TranscribeCpp
                || self.audio_source != AudioSourceKind::PipeWire
                || self.delivery_backend != DeliveryBackendKind::RoutedThenInbox
                || self.force_delivery_failure
                || (self.transcribe_model_path.is_none()
                    && !(self.transcribe_installed && self.transcribe_model_id.is_some())))
        {
            return Err("linux_mic_routed_inbox_configuration_invalid");
        }
        // A process that selects the routed backend without the routed lane has
        // no composition that can ever deliver: reject it instead of
        // advertising a mechanism the daemon cannot execute.
        if self.delivery_backend == DeliveryBackendKind::RoutedThenInbox
            && !self.recovery_linux_mic_routed_inbox
        {
            return Err("routed_inbox_requires_routed_lane");
        }
        Ok(())
    }

    /// The one strict composition lane this process selected, if any. A lane
    /// conflict or no lane at all resolves to `None`; use
    /// [`Self::is_strict_recovery`] to distinguish the conflict state from an
    /// ordinary non-strict process.
    #[must_use]
    pub fn strict_recovery_lane(&self) -> Option<StrictRecoveryLane> {
        match (
            self.recovery_linux_mic_inbox,
            self.recovery_linux_mic_wtype_inbox,
            self.recovery_linux_mic_routed_inbox,
        ) {
            (true, false, false) => Some(StrictRecoveryLane::InboxOnly),
            (false, true, false) => Some(StrictRecoveryLane::WtypeThenInbox),
            (false, false, true) => Some(StrictRecoveryLane::TargetRouted),
            _ => None,
        }
    }

    /// True when any strict composition lane is configured, including the
    /// invalid multi-lane conflict state (which always fails validation).
    #[must_use]
    pub fn is_strict_recovery(&self) -> bool {
        self.recovery_linux_mic_inbox
            || self.recovery_linux_mic_wtype_inbox
            || self.recovery_linux_mic_routed_inbox
    }

    /// The delivery mode identity one strict lane publishes to the portable
    /// recovery tail. The lane ids and mode ids are distinct namespaces: this
    /// value is a delivery-mode identity, never a pipeline identity.
    #[must_use]
    pub fn strict_recovery_delivery_mode(&self) -> Option<RecoveryDeliveryMode> {
        match self.strict_recovery_lane()? {
            StrictRecoveryLane::InboxOnly => Some(RecoveryDeliveryMode::InboxPrimary),
            StrictRecoveryLane::WtypeThenInbox => {
                Some(RecoveryDeliveryMode::WtypeThenInboxRecovery)
            }
            StrictRecoveryLane::TargetRouted => {
                Some(RecoveryDeliveryMode::TargetRoutedThenInboxRecovery)
            }
        }
    }

    /// Content-free readiness check used by status and by StartSession. This
    /// is deliberately a hard reject in strict recovery mode, never a fallback
    /// selector.
    pub fn recovery_static_readiness(&self) -> Result<(), &'static str> {
        self.validate_strict_recovery_configuration()?;
        if !self.is_strict_recovery() {
            return Ok(());
        }
        // A resident process has already bound, sealed, hashed and loaded its
        // model before IPC starts. Reopening the original pathname here would
        // reintroduce path drift and could contradict the retained runtime.
        if !self.transcribe_resident {
            self.model_readiness()?;
            self.sidecar_readiness()?;
        }
        if self.inbox_dir.is_none() {
            return Err("inbox_unavailable");
        }
        pipewire_helper_readiness(&narrivy_platform_linux::recovery::pipewire_recorder_status())?;
        // The Phase 2B lane's single direct rung is wtype, so its helper is a
        // hard readiness requirement. The routed lane deliberately does not
        // require any direct backend: its qualified-backend ladder is evaluated
        // per attempt and an absent/unqualified backend is a truthful bounded
        // Inbox fallback, never a readiness failure and never a support claim.
        if self.recovery_linux_mic_wtype_inbox {
            wtype_helper_readiness(&narrivy_platform_linux::recovery::wtype_submitter_status())?;
        }
        Ok(())
    }

    fn model_readiness(&self) -> Result<(), &'static str> {
        let Some(path) = self.transcribe_model_path.as_deref() else {
            return Err("model_missing");
        };
        #[cfg(feature = "asr-transcribe-cpp")]
        match narrivy_asr_transcribe::validate_gguf_model(path) {
            Ok(()) => Ok(()),
            Err(err) => match err.code {
                "model_missing" => Err("model_missing"),
                "model_unsupported" => Err("model_unsupported"),
                _ => Err("model_invalid"),
            },
        }
        #[cfg(not(feature = "asr-transcribe-cpp"))]
        {
            let _ = path;
            Err("asr_runtime_unavailable")
        }
    }

    fn sidecar_readiness(&self) -> Result<(), &'static str> {
        let configured = std::env::var_os("TRANSCRIBE_CPP_BIN").map(PathBuf::from);
        Self::sidecar_readiness_at(configured.as_deref())
    }

    fn sidecar_readiness_at(path: Option<&Path>) -> Result<(), &'static str> {
        let Some(path) = path else {
            return Err("sidecar_missing");
        };
        let Ok(meta) = std::fs::metadata(path) else {
            return Err("sidecar_missing");
        };
        if !meta.is_file() {
            return Err("sidecar_missing");
        }
        if meta.permissions().mode() & 0o111 == 0 {
            return Err("sidecar_nonexecutable");
        }
        Ok(())
    }
}

fn installed_worker_identity_is_ambient(mut is_present: impl FnMut(&str) -> bool) -> bool {
    [
        "NARRIVY_TRANSCRIBE_WORKER",
        "NARRIVY_TRANSCRIBE_WORKER_SHA256",
        "NARRIVY_TRANSCRIBE_WORKER_SIZE",
    ]
    .into_iter()
    .any(&mut is_present)
}

/// A strict composition's configured default must name a canonical built-in
/// mode. An absent value is valid (the behavior-preserving Clean default);
/// historical lane ids and the legacy mock lane are not strict defaults.
fn strict_default_pipeline_is_valid(env_value: Option<&str>) -> bool {
    env_value.is_none_or(|value| CorrectionMode::from_pipeline_id(value).is_some())
}

fn pipewire_helper_readiness(status: &PipeWireRecorderStatus) -> Result<(), &'static str> {
    match status {
        PipeWireRecorderStatus::Available(_) => Ok(()),
        PipeWireRecorderStatus::Missing => Err("pipewire_recorder_missing"),
        PipeWireRecorderStatus::NonExecutable => Err("pipewire_recorder_nonexecutable"),
        PipeWireRecorderStatus::Untrusted => Err("pipewire_recorder_untrusted"),
        PipeWireRecorderStatus::IdentityChanged => Err("pipewire_recorder_identity_changed"),
        PipeWireRecorderStatus::UnsupportedPlatform => {
            Err("pipewire_recorder_isolation_unavailable")
        }
    }
}

fn wtype_helper_readiness(status: &WtypeSubmitterStatus) -> Result<(), &'static str> {
    match status {
        WtypeSubmitterStatus::Available(_) => Ok(()),
        WtypeSubmitterStatus::Missing => Err("wtype_missing"),
        WtypeSubmitterStatus::NonExecutable => Err("wtype_nonexecutable"),
        WtypeSubmitterStatus::Untrusted => Err("wtype_untrusted"),
        WtypeSubmitterStatus::IdentityChanged => Err("wtype_identity_changed"),
        WtypeSubmitterStatus::UnsupportedPlatform => Err("wtype_isolation_unavailable"),
    }
}

fn pipewire_helper_capability(status: &PipeWireRecorderStatus) -> &'static str {
    match status {
        PipeWireRecorderStatus::Available(_) => "ready:system-owned-revalidated",
        PipeWireRecorderStatus::Missing => "unavailable:pipewire_recorder_missing",
        PipeWireRecorderStatus::NonExecutable => "unavailable:pipewire_recorder_nonexecutable",
        PipeWireRecorderStatus::Untrusted => "unavailable:pipewire_recorder_untrusted",
        PipeWireRecorderStatus::IdentityChanged => "unavailable:pipewire_recorder_identity_changed",
        PipeWireRecorderStatus::UnsupportedPlatform => {
            "unavailable:pipewire_recorder_isolation_unavailable"
        }
    }
}

fn wtype_helper_capability(status: &WtypeSubmitterStatus) -> &'static str {
    match status {
        WtypeSubmitterStatus::Available(_) => "ready:system-owned-revalidated",
        WtypeSubmitterStatus::Missing => "unavailable:wtype_missing",
        WtypeSubmitterStatus::NonExecutable => "unavailable:wtype_nonexecutable",
        WtypeSubmitterStatus::Untrusted => "unavailable:wtype_untrusted",
        WtypeSubmitterStatus::IdentityChanged => "unavailable:wtype_identity_changed",
        WtypeSubmitterStatus::UnsupportedPlatform => "unavailable:wtype_isolation_unavailable",
    }
}

/// Content-free tokens for the target-aware routed lane.
///
/// Every token names one composed mechanism and the evidence currently known
/// for it. None of them claims verified insertion (no rung can verify), reveals
/// target identity, or carries clipboard/transcript content. The routed lane
/// therefore stays truthfully distinguishable from the Phase 2A Inbox-only lane
/// and the Phase 2B single-wtype lane.
fn append_routed_delivery_capabilities(caps: &mut Vec<String>, config: &EngineConfig) {
    caps.push("delivery.route=target-aware".to_owned());
    caps.push("delivery.verified=unavailable".to_owned());
    caps.push("delivery.inbox-recovery=enabled".to_owned());
    // The installed Start path has no trustworthy pre-activation target
    // binding: this is the explicit typed-unavailable input the routed
    // composition consumes, and every target-requiring rung is skipped while it
    // holds. Advertising it keeps the card honest about what can actually run.
    caps.push(
        "delivery.target-binding=unavailable:pre_activation_target_binding_absent".to_owned(),
    );
    let wtype_status = narrivy_platform_linux::recovery::wtype_submitter_status();
    caps.push(format!(
        "delivery.wtype-helper={}",
        wtype_helper_capability(&wtype_status)
    ));
    caps.push("delivery.wtype-submission=unverified".to_owned());
    // The portal/EIS rung dispatches only through an already-active grant the
    // user requested; an unverified frame is never reported as qualified
    // insertion.
    caps.push("delivery.backend.portal-eis=unverified:permission-gated".to_owned());
    // The legacy PATH-resolved `wl-copy`/`wl-paste`/`xclip` paths are never
    // composed. With the explicit authorization, clipboard paste runs through
    // the Wayland data-control protocol and the trusted wtype (D29); it is
    // unverified (the paste is sent, not observed in the target). Copy-only
    // still has no accepted helper.
    let (clipboard, paste) = if config.clipboard_authorized {
        (
            "unverified:wayland-data-control",
            "unverified:wayland-data-control",
        )
    } else {
        (
            "unavailable:clipboard_not_authorized",
            "unavailable:clipboard_not_authorized",
        )
    };
    let copy = if config.clipboard_authorized {
        "unavailable:trusted_clipboard_helper_not_accepted"
    } else {
        "unavailable:clipboard_not_authorized"
    };
    caps.push(format!("delivery.backend.clipboard={clipboard}"));
    caps.push(format!("delivery.clipboard-restore={clipboard}"));
    caps.push(format!("delivery.rung.clipboard-paste={paste}"));
    caps.push(format!("delivery.rung.clipboard-copy={copy}"));
}

/// Truthful capability strings for GetStatus. The DEFAULT config reports
/// exactly the P1 set ("mock.asr", "null.delivery"); every non-default lane
/// advertises itself instead of masquerading as another engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(feature = "asr-transcribe-resident"), allow(dead_code))]
pub(crate) enum ResidentArtifactBinding {
    ControlledDigestBound,
    InstalledRuntimeManifestBound,
    RegistryObservedDigestBound,
    InstalledModelMissing,
    PackageBindingUnavailable,
    StoredBindingUnavailable,
}

impl ResidentArtifactBinding {
    fn worker_ready(self) -> bool {
        matches!(
            self,
            Self::ControlledDigestBound | Self::InstalledRuntimeManifestBound
        )
    }

    fn model_ready(self) -> bool {
        matches!(
            self,
            Self::ControlledDigestBound | Self::RegistryObservedDigestBound
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(feature = "asr-transcribe-resident"), allow(dead_code))]
pub(crate) enum ResidentLifecycleCapability {
    Ready,
    Active,
    Warming,
    RestartRequired,
    FailedClosed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(feature = "asr-transcribe-resident"), allow(dead_code))]
pub(crate) struct ResidentCapabilitySnapshot {
    pub lifecycle: ResidentLifecycleCapability,
    pub artifact_binding: ResidentArtifactBinding,
    pub model_binding: ResidentArtifactBinding,
}

#[cfg(any(test, not(feature = "asr-transcribe-resident")))]
pub(crate) fn capability_strings(
    config: &EngineConfig,
    compute: Option<&super::compute::ComputePlan>,
) -> Vec<String> {
    capability_strings_with_resident(config, None, compute)
}

/// Advertise only what the compute planner actually resolved. A plan that
/// could not be built is stated as unavailable rather than omitted, so a
/// client can distinguish "no accelerator" from "the daemon never looked".
pub(crate) fn append_compute_capabilities(
    caps: &mut Vec<String>,
    compute: Option<&super::compute::ComputePlan>,
) {
    match compute {
        Some(plan) => plan.append_capabilities(caps),
        None => caps.push(super::compute::DEVICE_UNAVAILABLE_CAPABILITY.to_owned()),
    }
}

/// Render static composition plus the content-free artifact identity retained
/// by the daemon-owned resident engine. The caller must pass only a stored
/// engine snapshot; this function performs no artifact path I/O.
pub(crate) fn capability_strings_with_resident(
    config: &EngineConfig,
    resident: Option<ResidentCapabilitySnapshot>,
    compute: Option<&super::compute::ComputePlan>,
) -> Vec<String> {
    let mut caps = Vec::new();
    #[allow(clippy::vec_init_then_push)] // cfg arms read clearer as pushes
    {
        if config.is_strict_recovery() {
            let static_ready = config.recovery_static_readiness();
            let pipewire_status = narrivy_platform_linux::recovery::pipewire_recorder_status();
            let capture = pipewire_helper_capability(&pipewire_status);
            caps.push(format!("capture.pipewire-helper={capture}"));
            caps.push("capture.pipewire-signal=unverified".to_owned());
            caps.push(format!(
                "capture.window_seconds={}",
                config.pipewire_capture_seconds
            ));
            // The documented production bound is reported independently of the
            // effective window so a client can render both the active limit and
            // the fact that a controlled developer override shortened it.
            caps.push(format!(
                "capture.max_seconds={}",
                narrivy_types::MAX_CAPTURE_SECONDS
            ));
            let model = if config.transcribe_resident {
                match resident.map(|snapshot| snapshot.model_binding) {
                    Some(
                        ResidentArtifactBinding::ControlledDigestBound
                        | ResidentArtifactBinding::RegistryObservedDigestBound,
                    ) => "available",
                    Some(ResidentArtifactBinding::InstalledRuntimeManifestBound) => {
                        "resident_model_binding_mismatched"
                    }
                    Some(ResidentArtifactBinding::InstalledModelMissing) => {
                        "installed_model_missing"
                    }
                    Some(ResidentArtifactBinding::PackageBindingUnavailable) => {
                        "resident_model_package_binding_unavailable"
                    }
                    Some(ResidentArtifactBinding::StoredBindingUnavailable) => {
                        "resident_model_binding_unavailable"
                    }
                    None => "resident_not_initialized",
                }
            } else {
                config
                    .model_readiness()
                    .map(|_| "available")
                    .unwrap_or_else(|reason| reason)
            };
            caps.push(format!(
                "model.gguf={}",
                if model == "available" {
                    model.to_owned()
                } else {
                    format!("unavailable:{model}")
                }
            ));
            let runtime = if config.transcribe_resident {
                "resident-worker"
            } else {
                "sidecar"
            };
            let runtime_readiness = if config.transcribe_resident {
                match resident {
                    Some(snapshot) if !snapshot.model_binding.model_ready() => Err(match snapshot
                        .model_binding
                    {
                        ResidentArtifactBinding::PackageBindingUnavailable => {
                            "resident_model_package_binding_unavailable"
                        }
                        ResidentArtifactBinding::StoredBindingUnavailable => {
                            "resident_model_binding_unavailable"
                        }
                        ResidentArtifactBinding::InstalledModelMissing => "installed_model_missing",
                        _ => "resident_model_binding_mismatched",
                    }),
                    Some(snapshot) if !snapshot.artifact_binding.worker_ready() => {
                        Err(match snapshot.artifact_binding {
                            ResidentArtifactBinding::PackageBindingUnavailable => {
                                "resident_worker_package_binding_unavailable"
                            }
                            ResidentArtifactBinding::StoredBindingUnavailable => {
                                "resident_worker_binding_unavailable"
                            }
                            _ => "resident_worker_binding_mismatched",
                        })
                    }
                    Some(ResidentCapabilitySnapshot {
                        lifecycle: ResidentLifecycleCapability::Ready,
                        ..
                    }) => Ok(()),
                    Some(ResidentCapabilitySnapshot {
                        lifecycle: ResidentLifecycleCapability::Active,
                        ..
                    }) => Err("resident_worker_active"),
                    Some(ResidentCapabilitySnapshot {
                        lifecycle: ResidentLifecycleCapability::Warming,
                        ..
                    }) => Err("resident_worker_warming"),
                    Some(ResidentCapabilitySnapshot {
                        lifecycle: ResidentLifecycleCapability::RestartRequired,
                        ..
                    }) => Err("resident_worker_restart_required"),
                    Some(ResidentCapabilitySnapshot {
                        lifecycle: ResidentLifecycleCapability::FailedClosed,
                        ..
                    }) => Err("resident_worker_failed_closed"),
                    None => Err("resident_not_initialized"),
                }
            } else {
                config.sidecar_readiness()
            };
            let asr = if model != "available" {
                format!("unavailable:{model}")
            } else {
                runtime_readiness
                    .map(|()| "available".to_owned())
                    .unwrap_or_else(|reason| format!("unavailable:{reason}"))
            };
            caps.push(format!("asr.transcribe-cpp.cpu={asr}"));
            caps.push(format!("asr.transcribe-cpp.runtime={runtime}"));
            if config.transcribe_resident {
                caps.push(format!(
                    "asr.transcribe-cpp.worker-artifact={}",
                    match resident.map(|snapshot| snapshot.artifact_binding) {
                        Some(ResidentArtifactBinding::ControlledDigestBound) => {
                            "ready:controlled-digest-bound"
                        }
                        Some(ResidentArtifactBinding::InstalledRuntimeManifestBound) => {
                            "ready:installed-runtime-manifest-bound"
                        }
                        Some(ResidentArtifactBinding::RegistryObservedDigestBound) => {
                            "unavailable:mismatched-model-binding"
                        }
                        Some(ResidentArtifactBinding::InstalledModelMissing) => {
                            "unavailable:model-not-started"
                        }
                        Some(ResidentArtifactBinding::PackageBindingUnavailable) => {
                            "unavailable:package-binding-unavailable"
                        }
                        Some(ResidentArtifactBinding::StoredBindingUnavailable) => {
                            "unavailable:stored-binding-unavailable"
                        }
                        None => "unavailable:resident-not-initialized",
                    }
                ));
                caps.push(
                    "asr.transcribe-cpp.worker-package-binding=unavailable:not-composed".to_owned(),
                );
                caps.push(format!(
                    "asr.transcribe-cpp.model-artifact={}",
                    match resident.map(|snapshot| snapshot.model_binding) {
                        Some(ResidentArtifactBinding::ControlledDigestBound) => {
                            "ready:controlled-digest-bound"
                        }
                        Some(ResidentArtifactBinding::RegistryObservedDigestBound) => {
                            "ready:registry-observed-immutable-digest-bound"
                        }
                        Some(ResidentArtifactBinding::InstalledRuntimeManifestBound) => {
                            "unavailable:mismatched-worker-binding"
                        }
                        Some(ResidentArtifactBinding::InstalledModelMissing) => {
                            "unavailable:installed-model-missing"
                        }
                        Some(ResidentArtifactBinding::PackageBindingUnavailable) => {
                            "unavailable:package-binding-unavailable"
                        }
                        Some(ResidentArtifactBinding::StoredBindingUnavailable) => {
                            "unavailable:stored-binding-unavailable"
                        }
                        None => "unavailable:resident-not-initialized",
                    }
                ));
                caps.push(
                    "asr.transcribe-cpp.model-package-binding=unavailable:not-composed".to_owned(),
                );
                caps.push(
                    "asr.transcribe-cpp.release-provenance=unavailable:not-composed".to_owned(),
                );
            }
            match config.strict_recovery_lane() {
                Some(StrictRecoveryLane::WtypeThenInbox) => {
                    let wtype_status = narrivy_platform_linux::recovery::wtype_submitter_status();
                    let wtype = wtype_helper_capability(&wtype_status);
                    caps.push(format!("delivery.wtype-helper={wtype}"));
                    caps.push("delivery.wtype-submission=unverified".to_owned());
                    caps.push("delivery.verified=unavailable".to_owned());
                    caps.push("delivery.inbox-recovery=enabled".to_owned());
                }
                Some(StrictRecoveryLane::TargetRouted) => {
                    append_routed_delivery_capabilities(&mut caps, config);
                }
                Some(StrictRecoveryLane::InboxOnly) => {
                    caps.push("delivery.inbox-only=available".to_owned());
                }
                None => {
                    caps.push("delivery.lane=invalid:strict_recovery_modes_conflict".to_owned());
                }
            }
            caps.push("policy.local-only=enabled".to_owned());
            caps.push("cancellation.shared=available".to_owned());
            caps.push("recovery.behavioral_qualification=unqualified".to_owned());
            // Advertising follows the same resolved mode policy that the
            // selection rule enforces: both canonical modes are listed exactly
            // when the strict composition is statically and live ready. The
            // composition lane id is configuration and is not advertised as a
            // pipeline.
            let mode_enabled = static_ready.and(runtime_readiness).is_ok();
            for mode in CorrectionMode::ALL {
                caps.push(format!(
                    "pipeline.{}={}",
                    mode.pipeline_id(),
                    if mode_enabled {
                        "enabled"
                    } else {
                        "unavailable"
                    }
                ));
            }
            caps.push(format!(
                "recovery.static-readiness={}",
                static_ready.map(|_| "ready").unwrap_or_else(|r| r)
            ));
            // The production path reports the compute plan too, so a client
            // can show which device class serves recognition.
            append_compute_capabilities(&mut caps, compute);
            return caps;
        }
        if config.audio_source == AudioSourceKind::PipeWire {
            caps.push("capture.pipewire=disabled:owned_strict_mode_required".to_owned());
        }
        caps.push(
            match config.asr_engine {
                AsrEngineKind::Mock => "mock.asr",
                #[cfg(feature = "asr-transcribe-cpp")]
                AsrEngineKind::TranscribeCpp => "asr.transcribe-cpp",
                #[cfg(not(feature = "asr-transcribe-cpp"))]
                AsrEngineKind::TranscribeCpp => "asr.transcribe-cpp-unavailable",
            }
            .to_owned(),
        );
        caps.push(
            match config.delivery_backend {
                DeliveryBackendKind::NullSink => "null.delivery",
                DeliveryBackendKind::InboxOnly => "inbox.delivery",
                DeliveryBackendKind::WtypeThenInbox => "wtype.inbox-recovery.delivery",
                DeliveryBackendKind::RoutedThenInbox => "routed.inbox-recovery.delivery",
            }
            .to_owned(),
        );
    }
    append_compute_capabilities(&mut caps, compute);
    caps
}

// ---------------------------------------------------------------------------
// Activation capability facts (atomic toggle / portal probe / listener)
// ---------------------------------------------------------------------------

/// Daemon-composed, content-free activation facts reported through status.
///
/// The three facts are deliberately separate: `atomic_command` says this
/// daemon serves the atomic `ToggleSession` request; `portal_interface` is the
/// observed result of the direct
/// `org.freedesktop.portal.GlobalShortcuts` probe (`None` until the daemon has
/// actually probed); `portal_listener` stays false until a real owned portal
/// listener is composed. No fact implies another, and none of them names a
/// desktop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActivationFacts {
    pub atomic_command: bool,
    pub portal_interface: Option<narrivy_platform_linux::trigger::PortalInterfaceProbe>,
    pub portal_listener: bool,
}

impl Default for ActivationFacts {
    fn default() -> Self {
        Self {
            // This build serves the atomic toggle request; the client may
            // still fail to reach the daemon, which is a separate fact.
            atomic_command: true,
            // Never guessed: an unprobed interface is unavailable.
            portal_interface: None,
            // No owned portal listener exists in this composition.
            portal_listener: false,
        }
    }
}

impl ActivationFacts {
    /// Truthful closed token for the portal interface fact.
    fn portal_interface_token(&self) -> &'static str {
        use narrivy_platform_linux::trigger::PortalInterfaceProbe;
        match self.portal_interface {
            Some(PortalInterfaceProbe::Present { .. }) => "available",
            Some(PortalInterfaceProbe::Absent) => "unavailable:interface_absent",
            Some(PortalInterfaceProbe::RuntimeUnavailable) => "unavailable:runtime_unavailable",
            Some(PortalInterfaceProbe::TimedOut) => "unavailable:timed_out",
            None => "unavailable:not_probed",
        }
    }
}

/// Append the three separate activation capability facts.
///
/// Clients must treat them independently: `activation.atomic-command=
/// available` means the daemon accepts one atomic toggle request;
/// `activation.portal-global-shortcuts=available` means the exact portal
/// interface answered a bounded probe; `activation.portal-listener=ready`
/// means a real daemon-owned listener is composed. The listener fact is
/// `unavailable:not_composed` until that is true.
pub(crate) fn append_activation_capabilities(
    capabilities: &mut Vec<String>,
    facts: ActivationFacts,
) {
    capabilities.push(format!(
        "activation.atomic-command={}",
        if facts.atomic_command {
            "available"
        } else {
            "unavailable:not_composed"
        }
    ));
    capabilities.push(format!(
        "activation.portal-global-shortcuts={}",
        facts.portal_interface_token()
    ));
    capabilities.push(format!(
        "activation.portal-listener={}",
        if facts.portal_listener {
            "ready"
        } else {
            "unavailable:not_composed"
        }
    ));
}

// ---------------------------------------------------------------------------
// Transaction-scoped recovery result
// ---------------------------------------------------------------------------

/// Map one typed portable capture end onto its closed wire tag. Both sources of
/// the reason (batch and streaming finalization) share this mapping.
fn capture_end_dto(end: narrivy_types::CaptureEnd) -> CaptureEndDto {
    match end {
        narrivy_types::CaptureEnd::FinalizationRequested => CaptureEndDto::FinalizationRequested,
        narrivy_types::CaptureEnd::EndpointDetected => CaptureEndDto::EndpointDetected,
        narrivy_types::CaptureEnd::SafetyLimit => CaptureEndDto::SafetyLimit,
        narrivy_types::CaptureEnd::SourceEnded => CaptureEndDto::SourceEnded,
    }
}

/// Map one exact short-lived recovery entry onto its explicit-request payload.
pub(crate) fn recovery_entry_detail(
    journal: &Journal,
    txid: &str,
) -> Result<RecoveryEntryDetailDto, narrivy_ipc::IpcError> {
    match journal.read(txid) {
        Ok(Some(entry)) => Ok(RecoveryEntryDetailDto {
            txid: entry.txid.clone(),
            state: match entry.state {
                JournalEntryState::Pending => "pending",
                JournalEntryState::Committed => "committed",
                JournalEntryState::Delivered => "delivered",
                JournalEntryState::InboxRecovered => "inbox_recovered",
                JournalEntryState::Cancelled => "cancelled",
                JournalEntryState::FailedRecoverable => "failed_recoverable",
                JournalEntryState::FailedTerminal => "failed_terminal",
            }
            .to_owned(),
            raw: entry.variants.raw.clone(),
            corrected: entry.variants.corrected.clone(),
            delivered: entry.variants.delivered.clone(),
            pipeline_id: entry.pipeline_id.clone(),
            capture_end: entry.capture_end.map(capture_end_dto),
        }),
        Ok(None) => Err(narrivy_ipc::IpcError::new(
            narrivy_ipc::IpcErrorCode::UnknownCommand,
            "unknown recovery transaction",
        )),
        Err(err) => Err(narrivy_ipc::IpcError::new(
            narrivy_ipc::IpcErrorCode::BadRequest,
            format!("invalid txid: {err}"),
        )),
    }
}

/// Closed set of supported check ids: mic | delivery | shortcuts |
/// theme-store. Each probe reports REAL system state truthfully and
/// content-free.
pub fn run_capability_check(id: &str) -> CapabilityCheckResultDto {
    let theme_override = std::env::var_os("NARRIVY_THEME_FILE").map(std::path::PathBuf::from);
    run_capability_check_with(id, theme_override.as_deref())
}

/// [`run_capability_check`] with an injected theme-file location (test and
/// harness lanes; production reads the env hook exactly once here).
pub fn run_capability_check_with(
    id: &str,
    theme_file: Option<&std::path::Path>,
) -> CapabilityCheckResultDto {
    const SUPPORTED: [&str; 4] = ["mic", "delivery", "shortcuts", "theme-store"];
    if !SUPPORTED.contains(&id) {
        return CapabilityCheckResultDto {
            id: id.to_owned(),
            available: false,
            detail: format!("unknown id; supported: {}", SUPPORTED.join(", ")),
        };
    }
    let (available, detail) = match id {
        "mic" => match narrivy_platform_linux::recovery::pipewire_recorder_status() {
            narrivy_platform_linux::recovery::PipeWireRecorderStatus::Available(path) => (
                false,
                format!(
                    "trusted pw-record helper at {}; microphone signal and routing unverified",
                    path.display()
                ),
            ),
            narrivy_platform_linux::recovery::PipeWireRecorderStatus::Missing => {
                (false, "trusted pw-record helper missing".to_owned())
            }
            narrivy_platform_linux::recovery::PipeWireRecorderStatus::NonExecutable => {
                (false, "trusted pw-record helper nonexecutable".to_owned())
            }
            narrivy_platform_linux::recovery::PipeWireRecorderStatus::Untrusted => {
                (false, "pw-record helper untrusted".to_owned())
            }
            narrivy_platform_linux::recovery::PipeWireRecorderStatus::IdentityChanged => {
                (false, "pw-record helper identity changed".to_owned())
            }
            narrivy_platform_linux::recovery::PipeWireRecorderStatus::UnsupportedPlatform => {
                (false, "pw-record helper isolation unavailable".to_owned())
            }
        },
        "delivery" => {
            portal_delivery_check(narrivy_platform_linux::portal_eis::classify_portal_eis(
                &narrivy_platform_linux::portal_eis::SystemRemoteDesktopProbe::default(),
            ))
        }
        "shortcuts" => {
            let route = narrivy_platform_linux::trigger::probe_portal_globalshortcuts();
            (
                route.is_available(),
                route
                    .available_note()
                    .or_else(|| route.unavailable_reason())
                    .unwrap_or_default()
                    .to_owned(),
            )
        }
        "theme-store" => match probe_theme_store_writability(theme_file) {
            Ok(true) => (true, "config dir writable".to_owned()),
            Ok(false) => (false, "config dir missing or not writable".to_owned()),
            Err(reason) => (false, reason),
        },
        _ => (false, "unreachable".to_owned()),
    };
    CapabilityCheckResultDto {
        id: id.to_owned(),
        available,
        detail,
    }
}

/// Translate portal eligibility into the existing diagnostics check shape.
/// `available` remains false until an external-target backend is permissioned,
/// connected, behaviorally qualified, and composed into the daemon.
fn portal_delivery_check(
    state: narrivy_platform_linux::portal_eis::PortalEisEligibility,
) -> (bool, String) {
    use narrivy_platform_linux::portal_eis::{PortalEisEligibility, PortalProbeFailure};

    let detail = match state {
        PortalEisEligibility::ReadyToRequestPermission { version } => {
            format!(
                "portal RemoteDesktop v{version} keyboard permission may be requested; external delivery unqualified"
            )
        }
        PortalEisEligibility::InterfaceAbsent => {
            "portal RemoteDesktop unavailable; external delivery unqualified".to_owned()
        }
        PortalEisEligibility::VersionTooOld { version } => {
            format!("portal RemoteDesktop v{version} lacks EIS; external delivery unqualified")
        }
        PortalEisEligibility::KeyboardUnavailable { version } => {
            format!(
                "portal RemoteDesktop v{version} has no keyboard device; external delivery unqualified"
            )
        }
        PortalEisEligibility::ProbeFailed(PortalProbeFailure::RuntimeUnavailable) => {
            "portal probe runtime unavailable; external delivery unqualified".to_owned()
        }
        PortalEisEligibility::ProbeFailed(PortalProbeFailure::TimedOut) => {
            "portal probe timed out; external delivery unqualified".to_owned()
        }
        PortalEisEligibility::ProbeFailed(PortalProbeFailure::QueryFailed) => {
            "portal probe failed; external delivery unqualified".to_owned()
        }
    };
    (false, detail)
}

/// Trusted system `pw-record` launch alias, if available.
pub fn probe_pw_record() -> Option<std::path::PathBuf> {
    match narrivy_platform_linux::recovery::pipewire_recorder_status() {
        narrivy_platform_linux::recovery::PipeWireRecorderStatus::Available(path) => Some(path),
        _ => None,
    }
}

/// Writability probe for the shell's theme storage directory.
pub fn probe_theme_store_writability(theme_file: Option<&std::path::Path>) -> Result<bool, String> {
    let target_dir: std::path::PathBuf = match theme_file {
        Some(path) => {
            let parent = path
                .parent()
                .ok_or("theme file path has no parent")?
                .to_path_buf();
            std::fs::create_dir_all(&parent)
                .map_err(|e| format!("create theme dir failed: {e}"))?;
            parent
        }
        _ => {
            let base = match std::env::var_os("XDG_CONFIG_HOME") {
                Some(v) if !v.is_empty() => std::path::PathBuf::from(v),
                _ => p3_home_dir().join(".config"),
            };
            base.join("narrivy")
        }
    };
    if !target_dir.is_dir() {
        return Ok(false);
    }
    let probe = target_dir.join(format!(".narrivyd-writable-probe-{}", std::process::id()));
    let outcome = std::fs::write(&probe, b"probe");
    let cleaned = std::fs::remove_file(&probe).is_ok();
    Ok(outcome.is_ok() && cleaned)
}

/// $HOME without extra dependencies (ungated; P3 probes use this).
fn p3_home_dir() -> std::path::PathBuf {
    std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
}

// ---------------------------------------------------------------------------
// P7 built-in pipelines (FUN-021)
// ---------------------------------------------------------------------------

/// Runtime-state directory holding saved pipeline definitions.
pub fn pipelines_dir(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join("pipelines")
}

fn node(id: &str, slot: CapabilitySlot, module_name: &str) -> PipelineNode {
    PipelineNode {
        id: id.to_owned(),
        slot,
        module: ResourceRef::narrivy_module(module_name, "1"),
        enabled: true,
        optional: false,
        device_preference: Vec::new(),
    }
}

fn edge(from: &str, to: &str) -> PipelineEdge {
    PipelineEdge {
        from_node: from.to_owned(),
        to_node: to.to_owned(),
    }
}

/// The two built-in mode definitions (FUN-021: modes ARE pipelines).
pub fn builtin_pipeline_defs() -> Vec<PipelineDefinition> {
    let mut raw = PipelineDefinition::new(
        RAW_PIPELINE_ID,
        "Raw dictation",
        vec![
            node("capture", CapabilitySlot::AudioCapture, "capture-audio"),
            node("vad", CapabilitySlot::SpeechVad, "speech-vad"),
            node("asr", CapabilitySlot::SpeechAsr, "asr-engine"),
            node(
                "render",
                CapabilitySlot::TextPunctuation,
                "text-collapse-whitespace",
            ),
            node("deliver", CapabilitySlot::DeliveryDirect, "delivery-sink"),
        ],
    );
    raw.edges = vec![
        edge("capture", "vad"),
        edge("vad", "asr"),
        edge("asr", "render"),
        edge("render", "deliver"),
    ];

    let mut clean = PipelineDefinition::new(
        CLEAN_PIPELINE_ID,
        "Clean dictation",
        vec![
            node("capture", CapabilitySlot::AudioCapture, "capture-audio"),
            node("vad", CapabilitySlot::SpeechVad, "speech-vad"),
            node("asr", CapabilitySlot::SpeechAsr, "asr-engine"),
            node(
                "correct",
                CapabilitySlot::TextPunctuation,
                "text-deterministic-corrector",
            ),
            node("itn", CapabilitySlot::TextItn, "text-itn"),
            node(
                "guard",
                CapabilitySlot::TextSemanticGuard,
                "text-semantic-guard",
            ),
            node("deliver", CapabilitySlot::DeliveryDirect, "delivery-sink"),
        ],
    );
    clean.edges = vec![
        edge("capture", "vad"),
        edge("vad", "asr"),
        edge("asr", "correct"),
        edge("correct", "itn"),
        edge("itn", "guard"),
        edge("guard", "deliver"),
    ];

    vec![raw, clean]
}

/// Register missing built-ins into the on-disk registry and persist the
/// store. Idempotent: existing definitions (possibly user-edited versions)
/// are preserved; only absent ids are inserted. Returns how many were added.
pub fn seed_builtin_pipelines(dir: &Path) -> Result<usize, String> {
    let mut registry = if dir.is_dir() {
        PipelineRegistry::load_from_dir(dir)
            .map_err(|e| format!("registry load failed: {e}"))?
            .registry
    } else {
        PipelineRegistry::new()
    };
    let mut inserted = 0usize;
    for def in builtin_pipeline_defs() {
        if registry.get(&def.id).is_none() {
            registry
                .insert(def)
                .map_err(|e| format!("builtin insert failed: {e}"))?;
            inserted += 1;
        }
    }
    registry
        .save_to_dir(dir)
        .map_err(|e| format!("registry save failed: {e}"))?;
    Ok(inserted)
}

/// Open the secure Unix recovery journal below Narrivy's durable state root.
pub fn open_journal(state_dir: &Path) -> Result<UnixJournal, String> {
    let journal_dir = state_dir.join("journal");
    UnixJournal::open(journal_dir).map_err(|e| format!("open journal failed: {e}"))
}

/// Production startup boundary: open secure durable recovery storage and
/// enforce TTL/count policy before any IPC request can be accepted.
pub fn open_startup_journal(state_dir: &Path) -> Result<RetainedUnixJournal, String> {
    let journal_dir = state_dir.join("journal");
    RetainedUnixJournal::open(journal_dir)
        .map_err(|error| format!("open retained journal failed: {error}"))
}

#[cfg(test)]
mod p2_config_tests {
    use super::super::capture::sine_fixture_samples;
    use super::super::compute::DEVICE_UNAVAILABLE_CAPABILITY;
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// The three activation facts are separate, content-free tokens: the
    /// atomic command, the observed portal interface, and the live listener.
    /// No fact may imply another (an available interface is not a listener).
    #[test]
    fn activation_capabilities_are_separate_and_content_free() {
        use narrivy_platform_linux::trigger::PortalInterfaceProbe;

        fn tokens(facts: ActivationFacts) -> Vec<String> {
            let mut caps = Vec::new();
            append_activation_capabilities(&mut caps, facts);
            caps
        }

        let unprobed = tokens(ActivationFacts::default());
        assert_eq!(
            unprobed,
            vec![
                "activation.atomic-command=available".to_owned(),
                "activation.portal-global-shortcuts=unavailable:not_probed".to_owned(),
                "activation.portal-listener=unavailable:not_composed".to_owned(),
            ]
        );

        for (probe, expected) in [
            (
                PortalInterfaceProbe::Present { version: 1 },
                "activation.portal-global-shortcuts=available",
            ),
            (
                PortalInterfaceProbe::Absent,
                "activation.portal-global-shortcuts=unavailable:interface_absent",
            ),
            (
                PortalInterfaceProbe::RuntimeUnavailable,
                "activation.portal-global-shortcuts=unavailable:runtime_unavailable",
            ),
            (
                PortalInterfaceProbe::TimedOut,
                "activation.portal-global-shortcuts=unavailable:timed_out",
            ),
        ] {
            let caps = tokens(ActivationFacts {
                atomic_command: true,
                portal_interface: Some(probe),
                portal_listener: false,
            });
            assert!(
                caps.iter().any(|cap| cap == expected),
                "missing {expected} in {caps:?}"
            );
            assert!(
                caps.iter()
                    .any(|cap| cap == "activation.portal-listener=unavailable:not_composed"),
                "a probed interface must never claim a live listener: {caps:?}"
            );
        }

        // A composed listener and an unavailable command are still reported
        // independently of the interface probe.
        let composed = tokens(ActivationFacts {
            atomic_command: false,
            portal_interface: None,
            portal_listener: true,
        });
        assert!(
            composed
                .iter()
                .any(|cap| cap == "activation.atomic-command=unavailable:not_composed")
        );
        assert!(
            composed
                .iter()
                .any(|cap| cap == "activation.portal-listener=ready")
        );
    }

    /// Both canonical modes carry the same strict-composition availability;
    /// the lane identity is configuration and is never a pipeline token. The
    /// routed-lane capability regression uses it in every feature build.
    fn assert_mode_tokens(caps: &[String], expected: &str) {
        for mode in CorrectionMode::ALL {
            assert!(
                caps.iter()
                    .any(|cap| cap == &format!("pipeline.{}={expected}", mode.pipeline_id())),
                "missing pipeline.{}={expected} in {caps:?}",
                mode.pipeline_id()
            );
        }
        assert!(
            caps.iter().all(|cap| {
                !cap.contains("pipeline.linux-mic-inbox")
                    && !cap.contains("pipeline.linux-mic-wtype-inbox")
                    && !cap.contains("pipeline.linux-mic-routed-inbox")
            }),
            "lane ids must not be advertised as pipelines: {caps:?}"
        );
    }

    #[test]
    fn defaults_reproduce_p1_contract() {
        let cfg = EngineConfig::new();
        assert_eq!(cfg.asr_engine, AsrEngineKind::Mock);
        assert_eq!(cfg.audio_source, AudioSourceKind::SineFixture);
        assert_eq!(cfg.delivery_backend, DeliveryBackendKind::NullSink);
        assert!(!cfg.force_delivery_failure);
        assert_eq!(
            capability_strings(&cfg, None),
            vec![
                "mock.asr".to_owned(),
                "null.delivery".to_owned(),
                // An uncomposed compute plan is stated, not omitted, so a client
                // can tell "no accelerator" from "the daemon never looked".
                DEVICE_UNAVAILABLE_CAPABILITY.to_owned(),
            ]
        );
    }

    #[test]
    fn strict_default_pipeline_must_name_a_canonical_mode() {
        assert!(strict_default_pipeline_is_valid(None));
        assert!(strict_default_pipeline_is_valid(Some(RAW_PIPELINE_ID)));
        assert!(strict_default_pipeline_is_valid(Some(CLEAN_PIPELINE_ID)));
        for invalid in [
            DEFAULT_PIPELINE_ID,
            LINUX_MIC_INBOX_PIPELINE_ID,
            LINUX_MIC_WTYPE_INBOX_PIPELINE_ID,
            "bogus",
            "",
        ] {
            assert!(
                !strict_default_pipeline_is_valid(Some(invalid)),
                "{invalid:?} must not be a strict default"
            );
        }
    }

    #[test]
    fn production_capture_window_is_the_documented_maximum() {
        let cfg = EngineConfig::new();
        assert_eq!(
            cfg.pipewire_capture_seconds,
            narrivy_types::MAX_CAPTURE_SECONDS as f32
        );
        let strict = EngineConfig {
            recovery_linux_mic_inbox: true,
            ..EngineConfig::new()
        };
        let caps = capability_strings(&strict, None);
        assert!(
            caps.iter().any(|cap| cap == "capture.window_seconds=300"),
            "effective production window must be reported: {caps:?}"
        );
        assert!(
            caps.iter().any(|cap| cap == "capture.max_seconds=300"),
            "documented bound must be reported independently: {caps:?}"
        );
        // A shortened controlled probe reports both the active window and the
        // unchanged production maximum.
        let probe = EngineConfig {
            recovery_linux_mic_inbox: true,
            pipewire_capture_seconds: 3.0,
            ..EngineConfig::new()
        };
        let caps = capability_strings(&probe, None);
        assert!(
            caps.iter().any(|cap| cap == "capture.window_seconds=3"),
            "{caps:?}"
        );
        assert!(
            caps.iter().any(|cap| cap == "capture.max_seconds=300"),
            "{caps:?}"
        );
    }

    #[test]
    fn developer_capture_override_can_only_shorten_a_bounded_probe() {
        let mut window = narrivy_types::MAX_CAPTURE_SECONDS as f32;
        assert_eq!(apply_developer_capture_seconds(&mut window, "1"), Ok(()));
        assert_eq!(window, 1.0);
        assert_eq!(apply_developer_capture_seconds(&mut window, "30"), Ok(()));
        assert_eq!(window, 30.0);
        for rejected in ["0", "0.5", "30.5", "300", "nan", "inf", "-3", "abc", ""] {
            let mut window = narrivy_types::MAX_CAPTURE_SECONDS as f32;
            assert!(
                apply_developer_capture_seconds(&mut window, rejected).is_err(),
                "{rejected:?} must be rejected"
            );
            assert_eq!(
                window,
                narrivy_types::MAX_CAPTURE_SECONDS as f32,
                "{rejected:?} must not mutate the window"
            );
        }
    }

    #[test]
    fn sine_fixture_is_deterministic_and_bounded() {
        let a = sine_fixture_samples();
        let b = sine_fixture_samples();
        assert_eq!(a.len(), 16_000);
        assert_eq!(a, b);
        assert!(a.iter().all(|s| s.abs() <= 0.25));
    }

    #[test]
    fn capabilities_track_lane_selection() {
        let cfg = EngineConfig {
            delivery_backend: DeliveryBackendKind::InboxOnly,
            ..EngineConfig::new()
        };
        assert_eq!(
            capability_strings(&cfg, None),
            vec![
                "mock.asr".to_owned(),
                "inbox.delivery".to_owned(),
                DEVICE_UNAVAILABLE_CAPABILITY.to_owned(),
            ]
        );
    }

    /// The packaged user unit selects the routed lane with the installed
    /// runtime (F-C1). That exact combination must pass startup validation,
    /// or the installed daemon would refuse to start.
    #[cfg(feature = "asr-transcribe-resident")]
    #[test]
    fn packaged_unit_routed_installed_composition_is_accepted() {
        let unit = include_str!("../../../../packaging/deb/narrivy-daemon.service");
        let lines: Vec<&str> = unit.lines().collect();
        for line in [
            "Environment=NARRIVY_RECOVERY_MODE=linux-mic-routed-inbox",
            "Environment=NARRIVY_DELIVERY_BACKEND=routed-inbox",
            "Environment=NARRIVY_ASR_ENGINE=transcribe-cpp",
            "Environment=NARRIVY_AUDIO_SOURCE=pipewire",
            "Environment=NARRIVY_DELIVERY_FAIL=0",
            "Environment=NARRIVY_TRANSCRIBE_RUNTIME=installed",
            "Environment=NARRIVY_TRANSCRIBE_MODEL_ID=whisper-tiny.en-Q8_0",
            "UnsetEnvironment=NARRIVY_TRANSCRIBE_MODEL",
        ] {
            assert!(lines.contains(&line), "packaged unit lacks {line}");
        }
        let packaged = EngineConfig {
            recovery_linux_mic_routed_inbox: true,
            asr_engine: AsrEngineKind::TranscribeCpp,
            audio_source: AudioSourceKind::PipeWire,
            delivery_backend: DeliveryBackendKind::RoutedThenInbox,
            transcribe_resident: true,
            transcribe_installed: true,
            transcribe_model_id: Some("whisper-tiny.en-Q8_0".into()),
            ..EngineConfig::new()
        };
        assert_eq!(packaged.validate_strict_recovery_configuration(), Ok(()));
        assert_eq!(
            packaged.strict_recovery_lane(),
            Some(StrictRecoveryLane::TargetRouted)
        );
    }

    #[cfg(feature = "asr-transcribe-resident")]
    #[test]
    fn installed_strict_configuration_requires_model_id_and_forbids_raw_identity() {
        let installed = EngineConfig {
            recovery_linux_mic_inbox: true,
            asr_engine: AsrEngineKind::TranscribeCpp,
            audio_source: AudioSourceKind::PipeWire,
            delivery_backend: DeliveryBackendKind::InboxOnly,
            transcribe_resident: true,
            transcribe_installed: true,
            transcribe_model_id: Some("whisper-tiny.en-Q8_0".into()),
            ..EngineConfig::new()
        };
        assert_eq!(installed.validate_strict_recovery_configuration(), Ok(()));

        let missing_id = EngineConfig {
            transcribe_model_id: None,
            ..installed.clone()
        };
        assert_eq!(
            missing_id.validate_strict_recovery_configuration(),
            Err("installed_resident_configuration_invalid")
        );
        let raw_path = EngineConfig {
            transcribe_model_path: Some(PathBuf::from("/tmp/model.gguf")),
            ..installed.clone()
        };
        assert_eq!(
            raw_path.validate_strict_recovery_configuration(),
            Err("installed_resident_configuration_invalid")
        );

        let wrong_engine = EngineConfig {
            asr_engine: AsrEngineKind::Mock,
            ..installed
        };
        assert_eq!(
            wrong_engine.validate_strict_recovery_configuration(),
            Err("installed_resident_configuration_invalid")
        );

        for raw_name in [
            "NARRIVY_TRANSCRIBE_WORKER",
            "NARRIVY_TRANSCRIBE_WORKER_SHA256",
            "NARRIVY_TRANSCRIBE_WORKER_SIZE",
        ] {
            assert!(installed_worker_identity_is_ambient(|name| name == raw_name));
        }
        assert!(!installed_worker_identity_is_ambient(|_| false));
    }

    #[cfg(feature = "asr-transcribe-resident")]
    #[test]
    fn resident_capabilities_use_supplied_stored_binding_without_claiming_package_provenance()
    -> TestResult {
        let dir = private_tempdir()?;
        let model = dir.path().join("model.gguf");
        std::fs::write(&model, b"GGUF\x03\0\0\0")?;
        let cfg = EngineConfig {
            recovery_linux_mic_inbox: true,
            asr_engine: AsrEngineKind::TranscribeCpp,
            audio_source: AudioSourceKind::PipeWire,
            delivery_backend: DeliveryBackendKind::InboxOnly,
            transcribe_model_path: Some(model),
            transcribe_resident: true,
            inbox_dir: Some(PathBuf::from("/controlled/inbox")),
            ..EngineConfig::new()
        };
        std::fs::remove_file(
            cfg.transcribe_model_path
                .as_ref()
                .ok_or("missing model path")?,
        )?;

        let controlled = capability_strings_with_resident(
            &cfg,
            Some(ResidentCapabilitySnapshot {
                lifecycle: ResidentLifecycleCapability::Ready,
                artifact_binding: ResidentArtifactBinding::ControlledDigestBound,
                model_binding: ResidentArtifactBinding::ControlledDigestBound,
            }),
            None,
        );
        assert!(controlled.iter().any(|cap| {
            cap == "asr.transcribe-cpp.worker-artifact=ready:controlled-digest-bound"
        }));
        assert!(controlled.iter().any(|cap| {
            cap == "asr.transcribe-cpp.worker-package-binding=unavailable:not-composed"
        }));
        assert!(controlled.iter().any(|cap| {
            cap == "asr.transcribe-cpp.model-artifact=ready:controlled-digest-bound"
        }));
        assert!(controlled.iter().any(|cap| {
            cap == "asr.transcribe-cpp.model-package-binding=unavailable:not-composed"
        }));
        assert!(controlled.iter().any(|cap| {
            cap == "asr.transcribe-cpp.release-provenance=unavailable:not-composed"
        }));
        assert!(
            controlled
                .iter()
                .any(|cap| cap == "asr.transcribe-cpp.cpu=available")
        );

        let installed = capability_strings_with_resident(
            &cfg,
            Some(ResidentCapabilitySnapshot {
                lifecycle: ResidentLifecycleCapability::Ready,
                artifact_binding: ResidentArtifactBinding::InstalledRuntimeManifestBound,
                model_binding: ResidentArtifactBinding::RegistryObservedDigestBound,
            }),
            None,
        );
        assert!(installed.iter().any(|cap| {
            cap == "asr.transcribe-cpp.worker-artifact=ready:installed-runtime-manifest-bound"
        }));
        assert!(installed.iter().any(|cap| {
            cap == "asr.transcribe-cpp.model-artifact=ready:registry-observed-immutable-digest-bound"
        }));
        assert!(installed.iter().any(|cap| {
            cap == "asr.transcribe-cpp.worker-package-binding=unavailable:not-composed"
        }));
        assert!(installed.iter().any(|cap| {
            cap == "asr.transcribe-cpp.model-package-binding=unavailable:not-composed"
        }));
        assert!(installed.iter().any(|cap| {
            cap == "asr.transcribe-cpp.release-provenance=unavailable:not-composed"
        }));
        assert!(
            installed
                .iter()
                .any(|cap| cap == "asr.transcribe-cpp.cpu=available")
        );

        let degraded = capability_strings_with_resident(
            &cfg,
            Some(ResidentCapabilitySnapshot {
                lifecycle: ResidentLifecycleCapability::FailedClosed,
                artifact_binding: ResidentArtifactBinding::InstalledRuntimeManifestBound,
                model_binding: ResidentArtifactBinding::InstalledModelMissing,
            }),
            None,
        );
        assert!(
            degraded
                .iter()
                .any(|cap| cap == "model.gguf=unavailable:installed_model_missing")
        );
        assert!(
            degraded
                .iter()
                .any(|cap| { cap == "asr.transcribe-cpp.cpu=unavailable:installed_model_missing" })
        );
        assert!(degraded.iter().any(|cap| {
            cap == "asr.transcribe-cpp.worker-artifact=ready:installed-runtime-manifest-bound"
        }));
        assert!(degraded.iter().any(|cap| {
            cap == "asr.transcribe-cpp.model-artifact=unavailable:installed-model-missing"
        }));
        assert_mode_tokens(&degraded, "unavailable");

        let missing = capability_strings_with_resident(&cfg, None, None);
        assert!(missing.iter().any(|cap| {
            cap == "asr.transcribe-cpp.worker-artifact=unavailable:resident-not-initialized"
        }));
        assert!(!missing.iter().any(|cap| cap.contains("ready:controlled")));
        assert!(missing.iter().any(|cap| {
            cap == "asr.transcribe-cpp.release-provenance=unavailable:not-composed"
        }));

        for lifecycle in [
            ResidentLifecycleCapability::Active,
            ResidentLifecycleCapability::Warming,
            ResidentLifecycleCapability::RestartRequired,
            ResidentLifecycleCapability::FailedClosed,
        ] {
            let unavailable = capability_strings_with_resident(
                &cfg,
                Some(ResidentCapabilitySnapshot {
                    lifecycle,
                    artifact_binding: ResidentArtifactBinding::ControlledDigestBound,
                    model_binding: ResidentArtifactBinding::ControlledDigestBound,
                }),
                None,
            );
            assert!(unavailable.iter().any(|cap| cap
                == &format!(
                    "asr.transcribe-cpp.cpu=unavailable:{}",
                    match lifecycle {
                        ResidentLifecycleCapability::Active => "resident_worker_active",
                        ResidentLifecycleCapability::Warming => "resident_worker_warming",
                        ResidentLifecycleCapability::RestartRequired => {
                            "resident_worker_restart_required"
                        }
                        ResidentLifecycleCapability::FailedClosed => {
                            "resident_worker_failed_closed"
                        }
                        ResidentLifecycleCapability::Ready => unreachable!(),
                    }
                )));
            assert_mode_tokens(&unavailable, "unavailable");
        }

        let unavailable_model = capability_strings_with_resident(
            &cfg,
            Some(ResidentCapabilitySnapshot {
                lifecycle: ResidentLifecycleCapability::Ready,
                artifact_binding: ResidentArtifactBinding::ControlledDigestBound,
                model_binding: ResidentArtifactBinding::StoredBindingUnavailable,
            }),
            None,
        );
        assert!(unavailable_model.iter().any(|cap| {
            cap == "asr.transcribe-cpp.cpu=unavailable:resident_model_binding_unavailable"
        }));
        Ok(())
    }

    #[test]
    fn strict_recovery_reports_a_missing_model_without_selecting_a_fallback() {
        let cfg = EngineConfig {
            recovery_linux_mic_inbox: true,
            asr_engine: AsrEngineKind::TranscribeCpp,
            audio_source: AudioSourceKind::PipeWire,
            delivery_backend: DeliveryBackendKind::InboxOnly,
            transcribe_model_path: Some(PathBuf::from("/definitely/missing/model.gguf")),
            inbox_dir: Some(PathBuf::from("/tmp/inbox")),
            ..EngineConfig::new()
        };
        #[cfg(feature = "asr-transcribe-cpp")]
        assert_eq!(cfg.recovery_static_readiness(), Err("model_missing"));
        #[cfg(not(feature = "asr-transcribe-cpp"))]
        assert_eq!(
            cfg.recovery_static_readiness(),
            Err("asr_runtime_unavailable")
        );
        let caps = capability_strings(&cfg, None);
        assert!(
            caps.iter()
                .any(|cap| cap == "model.gguf=unavailable:model_missing"
                    || cap == "model.gguf=unavailable:asr_runtime_unavailable")
        );
        assert!(
            !caps
                .iter()
                .any(|cap| cap.contains("mock") || cap.contains("null"))
        );
    }

    #[test]
    fn strict_helper_statuses_keep_typed_readiness_and_capability_reasons() {
        let pipewire_cases = [
            (
                PipeWireRecorderStatus::Available(PathBuf::from("/usr/bin/pw-record")),
                Ok(()),
                "ready:system-owned-revalidated",
            ),
            (
                PipeWireRecorderStatus::Missing,
                Err("pipewire_recorder_missing"),
                "unavailable:pipewire_recorder_missing",
            ),
            (
                PipeWireRecorderStatus::NonExecutable,
                Err("pipewire_recorder_nonexecutable"),
                "unavailable:pipewire_recorder_nonexecutable",
            ),
            (
                PipeWireRecorderStatus::Untrusted,
                Err("pipewire_recorder_untrusted"),
                "unavailable:pipewire_recorder_untrusted",
            ),
            (
                PipeWireRecorderStatus::IdentityChanged,
                Err("pipewire_recorder_identity_changed"),
                "unavailable:pipewire_recorder_identity_changed",
            ),
            (
                PipeWireRecorderStatus::UnsupportedPlatform,
                Err("pipewire_recorder_isolation_unavailable"),
                "unavailable:pipewire_recorder_isolation_unavailable",
            ),
        ];
        for (status, readiness, capability) in pipewire_cases {
            assert_eq!(pipewire_helper_readiness(&status), readiness);
            assert_eq!(pipewire_helper_capability(&status), capability);
        }

        let wtype_cases = [
            (
                WtypeSubmitterStatus::Available(PathBuf::from("/usr/bin/wtype")),
                Ok(()),
                "ready:system-owned-revalidated",
            ),
            (
                WtypeSubmitterStatus::Missing,
                Err("wtype_missing"),
                "unavailable:wtype_missing",
            ),
            (
                WtypeSubmitterStatus::NonExecutable,
                Err("wtype_nonexecutable"),
                "unavailable:wtype_nonexecutable",
            ),
            (
                WtypeSubmitterStatus::Untrusted,
                Err("wtype_untrusted"),
                "unavailable:wtype_untrusted",
            ),
            (
                WtypeSubmitterStatus::IdentityChanged,
                Err("wtype_identity_changed"),
                "unavailable:wtype_identity_changed",
            ),
            (
                WtypeSubmitterStatus::UnsupportedPlatform,
                Err("wtype_isolation_unavailable"),
                "unavailable:wtype_isolation_unavailable",
            ),
        ];
        for (status, readiness, capability) in wtype_cases {
            assert_eq!(wtype_helper_readiness(&status), readiness);
            assert_eq!(wtype_helper_capability(&status), capability);
        }
    }

    #[test]
    fn strict_capabilities_report_the_compute_plan() {
        let caps = capability_strings(&strict_wtype_config(), None);
        assert_eq!(
            caps.iter()
                .filter(|cap| cap.starts_with("compute.device="))
                .collect::<Vec<_>>(),
            vec![DEVICE_UNAVAILABLE_CAPABILITY]
        );
    }

    #[test]
    fn strict_wtype_capabilities_do_not_claim_signal_or_target_retention() {
        let caps = capability_strings(&strict_wtype_config(), None);
        assert!(
            caps.iter()
                .any(|cap| cap.starts_with("capture.pipewire-helper="))
        );
        assert!(
            caps.iter()
                .any(|cap| cap == "capture.pipewire-signal=unverified")
        );
        assert!(
            caps.iter()
                .any(|cap| cap.starts_with("delivery.wtype-helper="))
        );
        assert!(
            caps.iter()
                .any(|cap| cap == "delivery.wtype-submission=unverified")
        );
        assert!(
            caps.iter()
                .any(|cap| cap == "delivery.verified=unavailable")
        );
        assert_eq!(
            caps.iter()
                .filter(|cap| cap.as_str() == "delivery.inbox-recovery=enabled")
                .count(),
            1
        );
        assert!(
            caps.iter()
                .any(|cap| cap.starts_with("recovery.static-readiness="))
        );
        assert!(!caps.iter().any(|cap| {
            cap == "capture.pipewire=available"
                || cap == "delivery.wtype=verified"
                || cap.starts_with("recovery.ready=")
        }));
    }

    fn strict_wtype_config() -> EngineConfig {
        EngineConfig {
            recovery_linux_mic_wtype_inbox: true,
            asr_engine: AsrEngineKind::TranscribeCpp,
            audio_source: AudioSourceKind::PipeWire,
            delivery_backend: DeliveryBackendKind::WtypeThenInbox,
            transcribe_model_path: Some(PathBuf::from("/controlled/model.gguf")),
            inbox_dir: Some(PathBuf::from("/controlled/inbox")),
            ..EngineConfig::new()
        }
    }

    #[test]
    fn strict_wtype_lane_rejects_every_substitute_composition() {
        let valid = strict_wtype_config();
        assert_eq!(valid.validate_strict_recovery_configuration(), Ok(()));

        let invalid = [
            EngineConfig {
                asr_engine: AsrEngineKind::Mock,
                ..valid.clone()
            },
            EngineConfig {
                audio_source: AudioSourceKind::SineFixture,
                ..valid.clone()
            },
            EngineConfig {
                audio_source: AudioSourceKind::FileWav(PathBuf::from("/controlled/audio.wav")),
                ..valid.clone()
            },
            EngineConfig {
                delivery_backend: DeliveryBackendKind::NullSink,
                ..valid.clone()
            },
            EngineConfig {
                delivery_backend: DeliveryBackendKind::InboxOnly,
                ..valid.clone()
            },
            EngineConfig {
                force_delivery_failure: true,
                ..valid.clone()
            },
            EngineConfig {
                transcribe_model_path: None,
                ..valid.clone()
            },
        ];
        for cfg in invalid {
            assert_eq!(
                cfg.validate_strict_recovery_configuration(),
                Err("linux_mic_wtype_inbox_configuration_invalid")
            );
            assert_eq!(
                cfg.recovery_static_readiness(),
                Err("linux_mic_wtype_inbox_configuration_invalid")
            );
        }
    }

    #[test]
    fn strict_recovery_modes_cannot_be_combined_or_masquerade_as_each_other() {
        let mut cfg = strict_wtype_config();
        cfg.recovery_linux_mic_inbox = true;
        assert_eq!(
            cfg.validate_strict_recovery_configuration(),
            Err("strict_recovery_modes_conflict")
        );
        assert_eq!(
            cfg.recovery_static_readiness(),
            Err("strict_recovery_modes_conflict")
        );

        let phase_2a = EngineConfig {
            recovery_linux_mic_inbox: true,
            asr_engine: AsrEngineKind::TranscribeCpp,
            audio_source: AudioSourceKind::PipeWire,
            delivery_backend: DeliveryBackendKind::InboxOnly,
            transcribe_model_path: Some(PathBuf::from("/controlled/model.gguf")),
            ..EngineConfig::new()
        };
        assert_eq!(phase_2a.validate_strict_recovery_configuration(), Ok(()));
    }

    #[test]
    fn non_strict_pipewire_requires_an_owned_recovery_composition() {
        let non_strict = EngineConfig {
            audio_source: AudioSourceKind::PipeWire,
            ..EngineConfig::new()
        };
        assert_eq!(
            non_strict.validate_strict_recovery_configuration(),
            Err("non_strict_pipewire_disabled")
        );
        assert_eq!(
            non_strict.recovery_static_readiness(),
            Err("non_strict_pipewire_disabled")
        );
        assert!(
            capability_strings(&non_strict, None)
                .iter()
                .any(|cap| cap == "capture.pipewire=disabled:owned_strict_mode_required")
        );

        let strict_inbox = EngineConfig {
            recovery_linux_mic_inbox: true,
            asr_engine: AsrEngineKind::TranscribeCpp,
            audio_source: AudioSourceKind::PipeWire,
            delivery_backend: DeliveryBackendKind::InboxOnly,
            transcribe_model_path: Some(PathBuf::from("/controlled/model.gguf")),
            ..EngineConfig::new()
        };
        assert_eq!(
            strict_inbox.validate_strict_recovery_configuration(),
            Ok(())
        );

        let strict_wtype = EngineConfig {
            recovery_linux_mic_wtype_inbox: true,
            asr_engine: AsrEngineKind::TranscribeCpp,
            audio_source: AudioSourceKind::PipeWire,
            delivery_backend: DeliveryBackendKind::WtypeThenInbox,
            transcribe_model_path: Some(PathBuf::from("/controlled/model.gguf")),
            ..EngineConfig::new()
        };
        assert_eq!(
            strict_wtype.validate_strict_recovery_configuration(),
            Ok(())
        );
    }

    /// The strict routed composition is exactly as substitutable-rejecting as
    /// the Phase 2A/2B lanes: it requires the owned capture, the real local
    /// engine, an explicit model and its own delivery backend, and it never
    /// accepts the null sink, the broad platform router, or another lane.
    fn strict_routed_config() -> EngineConfig {
        EngineConfig {
            recovery_linux_mic_routed_inbox: true,
            asr_engine: AsrEngineKind::TranscribeCpp,
            audio_source: AudioSourceKind::PipeWire,
            delivery_backend: DeliveryBackendKind::RoutedThenInbox,
            transcribe_model_path: Some(PathBuf::from("/controlled/model.gguf")),
            inbox_dir: Some(PathBuf::from("/controlled/inbox")),
            ..EngineConfig::new()
        }
    }

    #[test]
    fn strict_routed_lane_rejects_every_substitute_composition() {
        let valid = strict_routed_config();
        assert_eq!(valid.validate_strict_recovery_configuration(), Ok(()));
        assert_eq!(
            valid.strict_recovery_lane(),
            Some(StrictRecoveryLane::TargetRouted)
        );
        assert_eq!(
            valid.strict_recovery_delivery_mode(),
            Some(RecoveryDeliveryMode::TargetRoutedThenInboxRecovery)
        );

        let invalid = [
            EngineConfig {
                asr_engine: AsrEngineKind::Mock,
                ..valid.clone()
            },
            EngineConfig {
                audio_source: AudioSourceKind::SineFixture,
                ..valid.clone()
            },
            EngineConfig {
                delivery_backend: DeliveryBackendKind::NullSink,
                ..valid.clone()
            },
            EngineConfig {
                delivery_backend: DeliveryBackendKind::InboxOnly,
                ..valid.clone()
            },
            EngineConfig {
                delivery_backend: DeliveryBackendKind::WtypeThenInbox,
                ..valid.clone()
            },
            EngineConfig {
                force_delivery_failure: true,
                ..valid.clone()
            },
            EngineConfig {
                transcribe_model_path: None,
                ..valid.clone()
            },
        ];
        for cfg in invalid {
            assert_eq!(
                cfg.validate_strict_recovery_configuration(),
                Err("linux_mic_routed_inbox_configuration_invalid")
            );
            assert_eq!(
                cfg.recovery_static_readiness(),
                Err("linux_mic_routed_inbox_configuration_invalid")
            );
        }
    }

    #[test]
    fn strict_routed_lane_conflicts_with_the_other_strict_lanes() {
        let mut with_phase_2a = strict_routed_config();
        with_phase_2a.recovery_linux_mic_inbox = true;
        assert_eq!(
            with_phase_2a.validate_strict_recovery_configuration(),
            Err("strict_recovery_modes_conflict")
        );
        assert_eq!(with_phase_2a.strict_recovery_lane(), None);
        assert_eq!(with_phase_2a.strict_recovery_delivery_mode(), None);

        let mut with_phase_2b = strict_routed_config();
        with_phase_2b.recovery_linux_mic_wtype_inbox = true;
        assert_eq!(
            with_phase_2b.validate_strict_recovery_configuration(),
            Err("strict_recovery_modes_conflict")
        );
    }

    /// Advertising follows the same resolved policy the admission rule
    /// enforces: the routed lane publishes only mechanisms and evidence it can
    /// observe, never verified insertion, never a lane id as a pipeline, and it
    /// reports the missing admission target binding and the unaccepted clipboard
    /// helpers instead of promising a rung that cannot run.
    #[test]
    fn strict_routed_capabilities_are_capability_gated_and_truthful() {
        let caps = capability_strings(&strict_routed_config(), None);
        for expected in [
            "delivery.route=target-aware",
            "delivery.verified=unavailable",
            "delivery.inbox-recovery=enabled",
            "delivery.target-binding=unavailable:pre_activation_target_binding_absent",
            "delivery.backend.portal-eis=unverified:permission-gated",
            "delivery.backend.clipboard=unavailable:clipboard_not_authorized",
            "delivery.clipboard-restore=unavailable:clipboard_not_authorized",
            "delivery.rung.clipboard-paste=unavailable:clipboard_not_authorized",
            "delivery.rung.clipboard-copy=unavailable:clipboard_not_authorized",
            "delivery.wtype-submission=unverified",
        ] {
            assert!(
                caps.iter().any(|cap| cap == expected),
                "missing {expected} in {caps:?}"
            );
        }
        assert_mode_tokens(&caps, "unavailable");
        assert!(
            !caps
                .iter()
                .any(|cap| cap.contains("linux-mic-routed-inbox")),
            "the composition lane id must never be advertised: {caps:?}"
        );
        assert!(
            !caps.iter().any(|cap| {
                cap == "delivery.verified=available" || cap == "delivery.verified=verified"
            }),
            "an unverified ladder must never advertise verified delivery: {caps:?}"
        );

        // Authorization enables clipboard paste through the data-control
        // protocol, reported as unverified (never a verified insert); copy-only
        // still has no accepted helper.
        let authorized = EngineConfig {
            clipboard_authorized: true,
            ..strict_routed_config()
        };
        let caps = capability_strings(&authorized, None);
        assert!(
            caps.iter()
                .any(|cap| { cap == "delivery.backend.clipboard=unverified:wayland-data-control" })
        );
        assert!(
            caps.iter().any(|cap| {
                cap == "delivery.rung.clipboard-paste=unverified:wayland-data-control"
            })
        );
        assert!(caps.iter().any(|cap| {
            cap == "delivery.rung.clipboard-copy=unavailable:trusted_clipboard_helper_not_accepted"
        }));
        assert!(
            !caps
                .iter()
                .any(|cap| cap.contains("ownership-aware:authorized")),
            "no clipboard mechanism may be advertised as available: {caps:?}"
        );
    }

    /// The persisted Phase 2A/2B lanes keep their exact old meaning: adding the
    /// routed lane must not rewrite Inbox-only or wtype-then-Inbox operation.
    #[test]
    fn existing_strict_lanes_keep_their_original_meaning() {
        let phase_2a = EngineConfig {
            recovery_linux_mic_inbox: true,
            asr_engine: AsrEngineKind::TranscribeCpp,
            audio_source: AudioSourceKind::PipeWire,
            delivery_backend: DeliveryBackendKind::InboxOnly,
            transcribe_model_path: Some(PathBuf::from("/controlled/model.gguf")),
            inbox_dir: Some(PathBuf::from("/controlled/inbox")),
            ..EngineConfig::new()
        };
        assert_eq!(
            phase_2a.strict_recovery_lane(),
            Some(StrictRecoveryLane::InboxOnly)
        );
        assert_eq!(
            phase_2a.strict_recovery_delivery_mode(),
            Some(RecoveryDeliveryMode::InboxPrimary)
        );
        assert!(
            capability_strings(&phase_2a, None)
                .iter()
                .any(|cap| cap == "delivery.inbox-only=available")
        );

        let phase_2b = strict_wtype_config();
        assert_eq!(
            phase_2b.strict_recovery_lane(),
            Some(StrictRecoveryLane::WtypeThenInbox)
        );
        assert_eq!(
            phase_2b.strict_recovery_delivery_mode(),
            Some(RecoveryDeliveryMode::WtypeThenInboxRecovery)
        );
        assert!(
            capability_strings(&phase_2b, None)
                .iter()
                .all(|cap| cap != "delivery.route=target-aware"),
            "the Phase 2B lane must not advertise the routed mechanism"
        );
    }

    /// Lane ids are composition configuration in one namespace; canonical
    /// modes are pipeline identities in another. The routed lane id resolves to
    /// neither a selectable mode nor a correction strategy.
    #[test]
    fn routed_lane_id_is_never_a_pipeline_identity() {
        assert_eq!(
            CorrectionMode::from_pipeline_id(LINUX_MIC_ROUTED_INBOX_LANE_ID),
            None
        );
        assert_eq!(
            CorrectionStrategy::resolve(Some(LINUX_MIC_ROUTED_INBOX_LANE_ID)),
            None
        );
        for lane in [
            StrictRecoveryLane::InboxOnly,
            StrictRecoveryLane::WtypeThenInbox,
            StrictRecoveryLane::TargetRouted,
        ] {
            assert_eq!(
                CorrectionMode::from_pipeline_id(lane.recovery_mode_id()),
                None
            );
        }
        assert_eq!(
            StrictRecoveryLane::TargetRouted.recovery_mode_id(),
            LINUX_MIC_ROUTED_INBOX_LANE_ID
        );
    }

    #[test]
    fn strict_routed_delivery_backend_requires_its_own_lane() {
        // The routed backend is composed only by the routed lane; a non-strict
        // process that selects it must fail closed at startup instead of
        // advertising a mechanism it can never execute.
        let non_strict = EngineConfig {
            delivery_backend: DeliveryBackendKind::RoutedThenInbox,
            ..EngineConfig::new()
        };
        assert_eq!(non_strict.strict_recovery_lane(), None);
        assert_eq!(non_strict.strict_recovery_delivery_mode(), None);
        assert_eq!(
            non_strict.validate_strict_recovery_configuration(),
            Err("routed_inbox_requires_routed_lane")
        );
        assert_eq!(
            non_strict.recovery_static_readiness(),
            Err("routed_inbox_requires_routed_lane")
        );
    }

    #[test]
    fn sidecar_readiness_distinguishes_missing_and_nonexecutable() -> TestResult {
        assert_eq!(
            EngineConfig::sidecar_readiness_at(None),
            Err("sidecar_missing")
        );
        let dir = private_tempdir()?;
        let missing = dir.path().join("missing-sidecar");
        assert_eq!(
            EngineConfig::sidecar_readiness_at(Some(&missing)),
            Err("sidecar_missing")
        );
        let sidecar = dir.path().join("sidecar");
        std::fs::write(&sidecar, b"placeholder")?;
        std::fs::set_permissions(&sidecar, std::fs::Permissions::from_mode(0o600))?;
        assert_eq!(
            EngineConfig::sidecar_readiness_at(Some(&sidecar)),
            Err("sidecar_nonexecutable")
        );
        Ok(())
    }

    #[cfg(feature = "asr-transcribe-cpp")]
    #[test]
    fn strict_recovery_model_validation_has_no_filename_identity_bypass() -> TestResult {
        let dir = private_tempdir()?;
        for (name, bytes, expected) in [
            ("arbitrary-name", b"not-a-gguf".as_slice(), "model_invalid"),
            (
                "anything.gguf",
                b"GGUF\x04\0\0\0".as_slice(),
                "model_unsupported",
            ),
        ] {
            let model = dir.path().join(name);
            std::fs::write(&model, bytes)?;
            let cfg = EngineConfig {
                recovery_linux_mic_inbox: true,
                asr_engine: AsrEngineKind::TranscribeCpp,
                audio_source: AudioSourceKind::PipeWire,
                delivery_backend: DeliveryBackendKind::InboxOnly,
                transcribe_model_path: Some(model),
                inbox_dir: Some(dir.path().join("inbox")),
                ..EngineConfig::new()
            };
            assert_eq!(cfg.model_readiness(), Err(expected));
            assert!(
                capability_strings(&cfg, None)
                    .iter()
                    .any(|cap| cap == &format!("model.gguf=unavailable:{expected}"))
            );
            assert_mode_tokens(&capability_strings(&cfg, None), "unavailable");
        }
        Ok(())
    }
}

#[cfg(test)]
mod p3_extension_tests {
    use super::*;
    use narrivy_platform_linux::portal_eis::{PortalEisEligibility, PortalProbeFailure};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn required<T>(value: Option<T>, message: &'static str) -> Result<T, std::io::Error> {
        value.ok_or_else(|| std::io::Error::other(message))
    }

    #[test]
    fn portal_delivery_diagnostics_never_promote_eligibility_to_availability() {
        for state in [
            PortalEisEligibility::ReadyToRequestPermission { version: 2 },
            PortalEisEligibility::InterfaceAbsent,
            PortalEisEligibility::VersionTooOld { version: 1 },
            PortalEisEligibility::KeyboardUnavailable { version: 2 },
            PortalEisEligibility::ProbeFailed(PortalProbeFailure::TimedOut),
        ] {
            let (available, detail) = portal_delivery_check(state);
            assert!(
                !available,
                "eligibility must not become delivery availability"
            );
            assert!(detail.contains("external delivery unqualified"));
        }
    }

    #[test]
    fn insecure_journal_directory_fails_startup_without_path_or_payload_disclosure() -> TestResult {
        let dir = private_tempdir()?;
        let journal_dir = dir.path().join("journal");
        std::fs::create_dir(&journal_dir)?;
        std::fs::set_permissions(&journal_dir, std::fs::Permissions::from_mode(0o750))?;
        let error = match open_journal(dir.path()) {
            Ok(_) => return Err("insecure journal directory was accepted".into()),
            Err(error) => error,
        };
        assert!(error.contains("unsafe journal storage state"));
        assert!(!error.contains(&journal_dir.display().to_string()));
        assert!(!error.contains("SECRET"));
        assert_eq!(
            std::fs::metadata(&journal_dir)?.permissions().mode() & 0o777,
            0o750
        );
        Ok(())
    }

    #[test]
    fn startup_journal_prunes_expired_and_caps_before_use() -> TestResult {
        let dir = private_tempdir()?;
        let journal = open_journal(dir.path())?;
        let now = u64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_millis(),
        )?;
        journal.write(&narrivy_core::JournalEntry::new_pending("expired", 0))?;
        for index in 0..=narrivy_core::MAX_JOURNAL_ENTRIES {
            journal.write(&narrivy_core::JournalEntry::new_pending(
                format!("fresh-{index:03}"),
                now.saturating_sub(1_000).saturating_add(index as u64),
            ))?;
        }
        drop(journal);

        let journal = open_startup_journal(dir.path())?;
        assert!(journal.read("expired")?.is_none());
        assert_eq!(journal.entries()?.len(), narrivy_core::MAX_JOURNAL_ENTRIES);
        Ok(())
    }

    #[test]
    fn capability_check_closed_set_rejects_unknown_ids() {
        let result = run_capability_check("definitely-not-a-check");
        assert!(!result.available);
        assert!(result.detail.contains("unknown id"));
        assert!(result.detail.contains("mic"));
    }

    #[test]
    fn capability_check_mic_never_promotes_helper_presence_to_signal() {
        let result = run_capability_check("mic");
        assert!(!result.available);
        if result.detail.contains("trusted pw-record helper") {
            assert!(
                result
                    .detail
                    .contains("microphone signal and routing unverified")
            );
        } else {
            assert!(
                [
                    "missing",
                    "nonexecutable",
                    "untrusted",
                    "identity changed",
                    "isolation unavailable",
                ]
                .iter()
                .any(|reason| result.detail.contains(reason))
            );
        }
    }

    #[test]
    fn theme_store_check_uses_injected_test_lane_and_never_touches_home() -> TestResult {
        let dir = private_tempdir()?;
        let theme_file = dir.path().join("config/theme.json");
        let result = run_capability_check_with("theme-store", Some(&theme_file));
        assert!(
            result.available,
            "theme-store check should pass on a fresh tempdir: {:?}",
            result.detail
        );
        let theme_dir = required(
            theme_file.parent(),
            "theme file must have a parent directory",
        )?;
        assert!(theme_dir.is_dir());

        let residue: Vec<_> = std::fs::read_dir(theme_dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains("writable-probe"))
            .collect();
        assert!(residue.is_empty(), "probe file leaked: {residue:?}");
        Ok(())
    }

    #[test]
    fn pw_record_probe_reports_path_or_none() {
        if let Some(path) = probe_pw_record() {
            assert!(path.ends_with("pw-record"));
        }
    }
}
