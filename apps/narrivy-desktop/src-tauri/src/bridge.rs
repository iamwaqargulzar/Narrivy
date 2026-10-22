//! Framework-neutral desktop client operations over `narrivy_ipc_unix::IpcClient`.
//!
//! Contract (apps/narrivy-desktop/CONTRACT.md): this shell is a CLIENT of
//! narrivyd (P1 IPC). It owns no domain policy. Every command maps 1:1 onto
//! a narrivy-ipc exchange against `$XDG_RUNTIME_DIR/narrivy/narrivyd.sock`
//! (override via `NARRIVY_SOCKET` for qualification lanes).
//!
//! Degraded-state rule: when the daemon is absent or errors, commands return
//! a typed [`BridgeError`] (never panic); the frontend renders the degraded
//! state from that value.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use narrivy_ipc::{
    Command, IpcError, IpcErrorCode, PipelineSummaryDto, RECOGNITION_SETTINGS_PROTOCOL_MINOR,
    RecognitionSettingsDto, ResponsePayload, SHORTCUT_BINDINGS_PROTOCOL_MINOR, ShortcutBindingDto,
    ShortcutSetDto, ShortcutsDto, StatusDto,
};
use serde::{Deserialize, Serialize};

/// Env var holding an explicit daemon socket path (qualification lanes).
pub const NARRIVY_SOCKET_ENV: &str = "NARRIVY_SOCKET";

/// Bounded default request budget; long operations stay bounded per AGENTS.md.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(4);
/// Starting a dictation may first load the pipeline's own model, which the
/// daemon bounds at about a minute and a half.
pub const START_TIMEOUT: Duration = Duration::from_secs(100);

/// Status snapshot surfaced to the frontend (wire shape verbatim from
/// narrivy-ipc; the frontend maps it onto capability-driven states).
pub type StatusSnapshot = StatusDto;

/// Resolve the daemon endpoint: `NARRIVY_SOCKET` wins, else the canonical
/// `$XDG_RUNTIME_DIR/narrivy/narrivyd.sock` shared with every other client.
pub fn socket_path() -> PathBuf {
    match std::env::var_os(NARRIVY_SOCKET_ENV) {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => narrivy_ipc_unix::default_socket_path(),
    }
}

/// Typed, serializable error surfaced to the frontend. `code` is the stable
/// narrivy-ipc wire tag (`daemon_unavailable`, `timeout`, ...), so the UI can
/// branch on capabilities/error classes without any OS-name knowledge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeError {
    pub code: String,
    pub message: String,
}

impl BridgeError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_owned(),
            message: message.into(),
        }
    }

    pub fn daemon_unavailable(message: impl Into<String>) -> Self {
        Self::new("daemon_unavailable", message)
    }
}

impl From<IpcError> for BridgeError {
    fn from(err: IpcError) -> Self {
        Self {
            code: err.code.wire_tag().to_owned(),
            message: err.message,
        }
    }
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for BridgeError {}

pub(crate) type CmdResult<T> = Result<T, BridgeError>;

fn connect(socket: &Path) -> Result<IpcClientRole, BridgeError> {
    IpcClientRole::connect_as(socket, "desktop").map_err(BridgeError::from)
}

/// Thin wrapper so command bodies read linearly; reconnection is explicit.
struct IpcClientRole {
    inner: narrivy_ipc_unix::IpcClient,
}

impl IpcClientRole {
    fn connect_as(socket: &Path, role: &str) -> Result<Self, IpcError> {
        Ok(Self {
            inner: narrivy_ipc_unix::IpcClient::connect_as(socket, role)?,
        })
    }

    fn request(&mut self, command: &Command) -> Result<ResponsePayload, BridgeError> {
        let timeout = match command {
            Command::StartSession { .. } | Command::ToggleSession { .. } => START_TIMEOUT,
            _ => REQUEST_TIMEOUT,
        };
        match self.inner.request(command, timeout) {
            Ok(payload) => Ok(payload),
            Err(err) if err.code == IpcErrorCode::DaemonUnavailable => Err(BridgeError::from(err)),
            Err(err) => Err(BridgeError::from(err)),
        }
    }

    fn protocol_minor(&self) -> u16 {
        self.inner.negotiated_minor()
    }
}

// ---------------------------------------------------------------------------
// Typed bridge DTOs
// ---------------------------------------------------------------------------

/// One optional-History row served by GetHistory. Content-safe fields only;
/// recovery-journal rows must never populate this type.
pub type JournalEntryDto = narrivy_ipc::protocol::HistoryEntryDto;

/// Full detail for ONE journal entry returned only on explicit request.
pub type HistoryEntryDetailDto = narrivy_ipc::protocol::HistoryEntryDetailDto;
/// Exact short-lived result for one recovery transaction.
pub type RecoveryEntryDetailDto = narrivy_ipc::protocol::RecoveryEntryDetailDto;
pub type ModelProvisionStatusDto = narrivy_ipc::ModelProvisionStatusDto;

const BUNDLED_MODEL_SOURCE: &str = "/usr/share/narrivy/models/whisper-tiny.en-Q8_0.gguf";

/// Result of ONE isolated capability check (`mic|delivery|shortcuts|theme-store`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityCheckResult {
    pub id: String,
    pub pass: bool,
    pub detail: String,
}

// ---------------------------------------------------------------------------
// Desktop client operations (CONTRACT.md surface)
// ---------------------------------------------------------------------------

/// Core status probe shared by the command and the setup-hook marker.
pub fn status_snapshot(socket: &Path) -> Result<StatusSnapshot, BridgeError> {
    let mut client = connect(socket)?;
    match client.request(&Command::GetStatus)? {
        ResponsePayload::Status(status) => Ok(status),
        ResponsePayload::Rejected { code, detail } => {
            Err(BridgeError::from(IpcError::new(code, detail)))
        }
        _ => Err(BridgeError::new(
            "internal",
            "unexpected payload for get_status",
        )),
    }
}

/// systemd user unit started on demand when the daemon socket is absent.
pub const DAEMON_SERVICE: &str = "narrivy-daemon.service";
/// Bound for starting the daemon on demand, including the readiness wait.
pub const DAEMON_START_TIMEOUT: Duration = Duration::from_secs(3);
const SYSTEMCTL: &str = "/usr/bin/systemctl";

/// Status probe that starts the installed per-user daemon once when it is not
/// running. Qualification lanes with an explicit `NARRIVY_SOCKET` keep full
/// ownership of their daemon process, so no start is attempted for them.
/// Blocking: call it from a worker thread, never the UI thread.
pub fn daemon_status_with_start() -> CmdResult<StatusSnapshot> {
    let initial = match daemon_status() {
        Ok(status) => return Ok(status),
        Err(error) => error,
    };
    if initial.code != "daemon_unavailable" || std::env::var_os(NARRIVY_SOCKET_ENV).is_some() {
        return Err(initial);
    }
    let deadline = Instant::now() + DAEMON_START_TIMEOUT;
    run_systemctl(&["--user", "daemon-reload"], deadline, &initial)?;
    run_systemctl(&["--user", "start", DAEMON_SERVICE], deadline, &initial)?;
    loop {
        match daemon_status() {
            Ok(status) => return Ok(status),
            Err(error) if error.code == "daemon_unavailable" && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => return Err(error),
        }
    }
}

fn run_systemctl(
    arguments: &[&str],
    deadline: Instant,
    unavailable: &BridgeError,
) -> CmdResult<()> {
    let mut child = std::process::Command::new(SYSTEMCTL)
        .args(arguments)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|_| unavailable.clone())?;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) | Err(_) => return Err(unavailable.clone()),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(BridgeError::daemon_unavailable(
                    "daemon service start timed out",
                ));
            }
        }
    }
}

pub fn daemon_status() -> CmdResult<StatusSnapshot> {
    status_snapshot(&socket_path())
}

pub fn daemon_start(
    pipeline_id: Option<String>,
    recognition: Option<RecognitionSettingsDto>,
) -> CmdResult<()> {
    let mut client = connect(&socket_path())?;
    if recognition.is_some() && client.protocol_minor() < RECOGNITION_SETTINGS_PROTOCOL_MINOR {
        return Err(BridgeError::new(
            "incompatible_protocol",
            "the daemon does not support recognition settings; update Narrivy",
        ));
    }
    // The UI lane always activates through the shell ("ui"); trigger routing
    // stays a daemon/platform concern (ADR-0005: no platform logic here).
    match client.request(&Command::StartSession {
        pipeline_id,
        activation: "ui".to_owned(),
        recognition,
    })? {
        ResponsePayload::Accepted => Ok(()),
        ResponsePayload::Rejected { code, detail } => {
            Err(BridgeError::from(IpcError::new(code, detail)))
        }
        _ => Err(BridgeError::new(
            "internal",
            "unexpected payload for start_session",
        )),
    }
}

pub fn daemon_stop() -> CmdResult<()> {
    stop_or_cancel(Command::StopSession)
}

pub fn daemon_cancel() -> CmdResult<()> {
    stop_or_cancel(Command::CancelSession)
}

fn stop_or_cancel(command: Command) -> CmdResult<()> {
    let mut client = connect(&socket_path())?;
    match client.request(&command)? {
        ResponsePayload::Accepted => Ok(()),
        ResponsePayload::Rejected { code, detail } => {
            Err(BridgeError::from(IpcError::new(code, detail)))
        }
        _ => Err(BridgeError::new(
            "internal",
            "unexpected payload for stop/cancel",
        )),
    }
}

pub fn daemon_list_pipelines() -> CmdResult<Vec<PipelineSummaryDto>> {
    let mut client = connect(&socket_path())?;
    match client.request(&Command::ListPipelines)? {
        ResponsePayload::Pipelines(pipelines) => Ok(pipelines),
        ResponsePayload::Rejected { code, detail } => {
            Err(BridgeError::from(IpcError::new(code, detail)))
        }
        _ => Err(BridgeError::new(
            "internal",
            "unexpected payload for list_pipelines",
        )),
    }
}

pub fn model_install_bundled() -> CmdResult<ModelProvisionStatusDto> {
    let mut client = connect(&socket_path())?;
    match client.request(&Command::StartOfflineModelProvision {
        source_path: BUNDLED_MODEL_SOURCE.to_owned(),
    })? {
        ResponsePayload::OfflineModelProvision(status) => Ok(status),
        ResponsePayload::Rejected { code, detail } => {
            Err(BridgeError::from(IpcError::new(code, detail)))
        }
        _ => Err(BridgeError::new(
            "internal",
            "unexpected payload for bundled model install",
        )),
    }
}

pub fn model_install_cancel(operation_id: String) -> CmdResult<ModelProvisionStatusDto> {
    let mut client = connect(&socket_path())?;
    match client.request(&Command::CancelOfflineModelProvision { operation_id })? {
        ResponsePayload::OfflineModelProvision(status) => Ok(status),
        ResponsePayload::Rejected { code, detail } => {
            Err(BridgeError::from(IpcError::new(code, detail)))
        }
        _ => Err(BridgeError::new(
            "internal",
            "unexpected payload for bundled model cancellation",
        )),
    }
}

/// Optional History listing. The daemon rejects this while the independent
/// History store is disabled/not composed; it never returns recovery rows.
pub fn daemon_history() -> CmdResult<Vec<JournalEntryDto>> {
    let mut client = connect(&socket_path())?;
    match client.request(&Command::GetHistory)? {
        ResponsePayload::History(rows) => Ok(rows),
        ResponsePayload::Rejected { code, detail } => {
            Err(BridgeError::from(IpcError::new(code, detail)))
        }
        _ => Err(BridgeError::new(
            "internal",
            "unexpected payload for get_history",
        )),
    }
}

/// Full optional-History entry, only when retained History is composed and an
/// explicit user action selects one row.
pub fn daemon_history_entry(txid: String) -> CmdResult<HistoryEntryDetailDto> {
    validate_txid(&txid)?;
    let mut client = connect(&socket_path())?;
    match client.request(&Command::GetHistoryEntry { txid })? {
        ResponsePayload::HistoryEntry(detail) => Ok(detail),
        ResponsePayload::Rejected {
            code,
            detail: err_detail,
        } => Err(BridgeError::from(IpcError::new(code, err_detail))),
        _ => Err(BridgeError::new(
            "internal",
            "unexpected payload for get_history_entry",
        )),
    }
}

/// Delete one optional-History entry on explicit user request. Requires a
/// daemon at protocol minor 10 or newer; older daemons are refused here so the
/// request is never silently ignored.
pub fn daemon_history_delete(txid: String) -> CmdResult<()> {
    validate_txid(&txid)?;
    let mut client = connect(&socket_path())?;
    if client.protocol_minor() < narrivy_ipc::DELETE_HISTORY_ENTRY_PROTOCOL_MINOR {
        return Err(BridgeError::new(
            "incompatible_protocol",
            "the daemon cannot delete History entries; update Narrivy",
        ));
    }
    match client.request(&Command::DeleteHistoryEntry { txid })? {
        ResponsePayload::Accepted => Ok(()),
        ResponsePayload::Rejected {
            code,
            detail: err_detail,
        } => Err(BridgeError::from(IpcError::new(code, err_detail))),
        _ => Err(BridgeError::new(
            "internal",
            "unexpected payload for delete_history_entry",
        )),
    }
}

/// The speech models the daemon knows about and the state of each on this
/// computer. Requires a daemon at protocol minor 13 or newer.
pub fn daemon_models() -> CmdResult<Vec<narrivy_ipc::ModelDto>> {
    let mut client = connect(&socket_path())?;
    if client.protocol_minor() < narrivy_ipc::LIST_MODELS_PROTOCOL_MINOR {
        return Err(BridgeError::new(
            "incompatible_protocol",
            "the daemon cannot list models; update Narrivy",
        ));
    }
    match client.request(&Command::ListModels)? {
        ResponsePayload::Models(models) => Ok(models),
        ResponsePayload::Rejected {
            code,
            detail: err_detail,
        } => Err(BridgeError::from(IpcError::new(code, err_detail))),
        _ => Err(BridgeError::new(
            "internal",
            "unexpected payload for list_models",
        )),
    }
}

/// Ask the daemon to load a downloaded model in place of the one in use.
/// Returns once loading has started. Requires protocol minor 14 or newer.
pub fn daemon_model_select(id: String) -> CmdResult<()> {
    let mut client = connect(&socket_path())?;
    if client.protocol_minor() < narrivy_ipc::SELECT_MODEL_PROTOCOL_MINOR {
        return Err(BridgeError::new(
            "incompatible_protocol",
            "the daemon cannot switch models; update Narrivy",
        ));
    }
    match client.request(&Command::SelectModel { id })? {
        ResponsePayload::Accepted => Ok(()),
        ResponsePayload::Rejected {
            code,
            detail: err_detail,
        } => Err(BridgeError::from(IpcError::new(code, err_detail))),
        _ => Err(BridgeError::new(
            "internal",
            "unexpected payload for select_model",
        )),
    }
}

/// Ask the daemon to download a published model, or to stop that download.
/// Returns once the request is accepted. Requires protocol minor 16 or newer.
pub fn daemon_model_download(id: String, stop: bool) -> CmdResult<()> {
    let mut client = connect(&socket_path())?;
    if client.protocol_minor() < narrivy_ipc::DOWNLOAD_MODEL_PROTOCOL_MINOR {
        return Err(BridgeError::new(
            "incompatible_protocol",
            "the daemon cannot download models; update Narrivy",
        ));
    }
    let command = if stop {
        Command::CancelModelDownload { id }
    } else {
        Command::DownloadModel { id }
    };
    match client.request(&command)? {
        ResponsePayload::Accepted => Ok(()),
        ResponsePayload::Rejected {
            code,
            detail: err_detail,
        } => Err(BridgeError::from(IpcError::new(code, err_detail))),
        _ => Err(BridgeError::new(
            "internal",
            "unexpected payload for a model download",
        )),
    }
}

/// What the Store offers and whether each item fits this computer. Requires
/// protocol minor 17 or newer.
pub fn daemon_store_list() -> CmdResult<Vec<narrivy_ipc::StoreItemDto>> {
    let mut client = connect(&socket_path())?;
    if client.protocol_minor() < narrivy_ipc::STORE_PROTOCOL_MINOR {
        return Err(BridgeError::new(
            "incompatible_protocol",
            "the daemon has no Store; update Narrivy",
        ));
    }
    match client.request(&Command::ListStore)? {
        ResponsePayload::Store(items) => Ok(items),
        ResponsePayload::Rejected {
            code,
            detail: err_detail,
        } => Err(BridgeError::from(IpcError::new(code, err_detail))),
        _ => Err(BridgeError::new(
            "internal",
            "unexpected payload for list_store",
        )),
    }
}

/// Ask the AI assistant (protocol minor 19). The reply streams as
/// `assistant_delta` events and ends with `assistant_done`, all carrying
/// `turn_id`.
pub fn daemon_assistant_send(
    turn_id: String,
    messages: Vec<narrivy_ipc::AssistantMessageDto>,
    chat_id: Option<String>,
    speak: bool,
    pictures: Vec<String>,
) -> CmdResult<()> {
    let library = chat_id.is_some() || speak || !pictures.is_empty();
    if library {
        library_client()?;
    }
    assistant_request(&Command::AssistantSend {
        turn_id,
        messages,
        chat_id,
        speak,
        pictures,
    })
}

/// A client for the assistant library and voice mode (protocol minor 44).
fn library_client() -> CmdResult<IpcClientRole> {
    let client = connect(&socket_path())?;
    if client.protocol_minor() < narrivy_ipc::ASSISTANT_LIBRARY_PROTOCOL_MINOR {
        return Err(BridgeError::new(
            "incompatible_protocol",
            "the daemon has no assistant library or voice mode; update Narrivy",
        ));
    }
    Ok(client)
}

/// Read or change the assistant's library (protocol minor 44); JSON back.
pub fn daemon_assistant_library(
    op: String,
    kind: String,
    id: Option<String>,
    value: Option<String>,
) -> CmdResult<String> {
    let mut client = library_client()?;
    match client.request(&Command::AssistantLibrary {
        op,
        kind,
        id,
        value,
    })? {
        ResponsePayload::AssistantLibrary(json) => Ok(json),
        ResponsePayload::Rejected {
            code,
            detail: err_detail,
        } => Err(BridgeError::from(IpcError::new(code, err_detail))),
        _ => Err(BridgeError::new(
            "internal",
            "unexpected payload for assistant_library",
        )),
    }
}

/// Allow or refuse a tool call the assistant asked for (protocol minor 46).
pub fn daemon_assistant_tool_decision(call_id: String, allow: bool, always: bool) -> CmdResult<()> {
    let mut client = connect(&socket_path())?;
    if client.protocol_minor() < narrivy_ipc::ASSISTANT_TOOLS_PROTOCOL_MINOR {
        return Err(BridgeError::new(
            "incompatible_protocol",
            "the daemon has no assistant tools; update Narrivy",
        ));
    }
    match client.request(&Command::AssistantToolDecision {
        call_id,
        allow,
        always,
    })? {
        ResponsePayload::Accepted => Ok(()),
        ResponsePayload::Rejected {
            code,
            detail: err_detail,
        } => Err(BridgeError::from(IpcError::new(code, err_detail))),
        _ => Err(BridgeError::new("internal", "unexpected payload for tool decision")),
    }
}

/// Start, stop or change voice mode (protocol minor 44).
pub fn daemon_voice_mode(
    action: String,
    settings: Option<narrivy_ipc::VoiceSettingsDto>,
    down: Option<bool>,
) -> CmdResult<()> {
    let mut client = library_client()?;
    match client.request(&Command::VoiceMode {
        action,
        settings,
        down,
    })? {
        ResponsePayload::Accepted => Ok(()),
        ResponsePayload::Rejected {
            code,
            detail: err_detail,
        } => Err(BridgeError::from(IpcError::new(code, err_detail))),
        _ => Err(BridgeError::new("internal", "unexpected payload for voice_mode")),
    }
}

/// Put the `back`-th most recent History text on the clipboard (0 is the
/// last dictation, 1 the one before), for pasting by hand when a paste was
/// missed. The daemon copies it; the text never reaches this process.
pub fn daemon_copy_recent(back: usize) -> CmdResult<()> {
    let mut rows = daemon_history()?;
    rows.retain(|row| row.has_delivered_text);
    rows.sort_by_key(|row| std::cmp::Reverse(row.updated_unix_ms));
    let txid = rows
        .into_iter()
        .nth(back)
        .map(|row| row.txid)
        .ok_or_else(|| BridgeError::new("not_found", "no such dictation in History"))?;
    let mut client = connect(&socket_path())?;
    if client.protocol_minor() < narrivy_ipc::COPY_HISTORY_ENTRY_PROTOCOL_MINOR {
        return Err(BridgeError::new(
            "incompatible_protocol",
            "the daemon cannot copy from History; update Narrivy",
        ));
    }
    match client.request(&Command::CopyHistoryEntry { txid })? {
        ResponsePayload::Accepted => Ok(()),
        ResponsePayload::Rejected {
            code,
            detail: err_detail,
        } => Err(BridgeError::from(IpcError::new(code, err_detail))),
        _ => Err(BridgeError::new(
            "internal",
            "unexpected payload for copy_history_entry",
        )),
    }
}

/// Free the assistant model's memory now (protocol minor 22).
pub fn daemon_assistant_unload() -> CmdResult<()> {
    let mut client = connect(&socket_path())?;
    if client.protocol_minor() < narrivy_ipc::ASSISTANT_UNLOAD_PROTOCOL_MINOR {
        return Err(BridgeError::new(
            "incompatible_protocol",
            "the daemon cannot unload the assistant; update Narrivy",
        ));
    }
    match client.request(&Command::AssistantUnload)? {
        ResponsePayload::Accepted => Ok(()),
        ResponsePayload::Rejected {
            code,
            detail: err_detail,
        } => Err(BridgeError::from(IpcError::new(code, err_detail))),
        _ => Err(BridgeError::new(
            "internal",
            "unexpected payload for assistant_unload",
        )),
    }
}

/// Stop the assistant's reply to `turn_id` (protocol minor 19).
pub fn daemon_assistant_cancel(turn_id: String) -> CmdResult<()> {
    assistant_request(&Command::AssistantCancel { turn_id })
}

/// The language models on this computer and their jobs (protocol minor 20).
pub fn daemon_language_models() -> CmdResult<Vec<narrivy_ipc::LanguageModelDto>> {
    let mut client = connect(&socket_path())?;
    if client.protocol_minor() < narrivy_ipc::LANGUAGE_MODELS_PROTOCOL_MINOR {
        return Err(BridgeError::new(
            "incompatible_protocol",
            "the daemon has no language models; update Narrivy",
        ));
    }
    match client.request(&Command::ListLanguageModels)? {
        ResponsePayload::LanguageModels(models) => Ok(models),
        ResponsePayload::Rejected {
            code,
            detail: err_detail,
        } => Err(BridgeError::from(IpcError::new(code, err_detail))),
        _ => Err(BridgeError::new(
            "internal",
            "unexpected payload for list_language_models",
        )),
    }
}

/// Use a language model for `role` (`clean_up`, `assistant`), or none.
pub fn daemon_language_model_select(role: String, id: Option<String>) -> CmdResult<()> {
    let mut client = connect(&socket_path())?;
    if client.protocol_minor() < narrivy_ipc::LANGUAGE_MODELS_PROTOCOL_MINOR {
        return Err(BridgeError::new(
            "incompatible_protocol",
            "the daemon has no language models; update Narrivy",
        ));
    }
    match client.request(&Command::SelectLanguageModel { role, id })? {
        ResponsePayload::Accepted => Ok(()),
        ResponsePayload::Rejected {
            code,
            detail: err_detail,
        } => Err(BridgeError::from(IpcError::new(code, err_detail))),
        _ => Err(BridgeError::new(
            "internal",
            "unexpected payload for select_language_model",
        )),
    }
}

/// Send a meeting command (protocol minor 25) and return the answer.
fn meeting_request(command: &Command) -> CmdResult<ResponsePayload> {
    let mut client = connect(&socket_path())?;
    if client.protocol_minor() < narrivy_ipc::MEETINGS_PROTOCOL_MINOR {
        return Err(BridgeError::new(
            "incompatible_protocol",
            "the daemon cannot record meetings; update Narrivy",
        ));
    }
    match client.request(command)? {
        ResponsePayload::Rejected {
            code,
            detail: err_detail,
        } => Err(BridgeError::from(IpcError::new(code, err_detail))),
        payload => Ok(payload),
    }
}

/// Send an audio-options command (protocol minor 27).
fn audio_options_request(command: &Command) -> CmdResult<narrivy_ipc::AudioOptionsDto> {
    let mut client = connect(&socket_path())?;
    if client.protocol_minor() < narrivy_ipc::AUDIO_OPTIONS_PROTOCOL_MINOR {
        return Err(BridgeError::new(
            "incompatible_protocol",
            "the daemon has no audio options; update Narrivy",
        ));
    }
    match client.request(command)? {
        ResponsePayload::AudioOptions(options) => Ok(options),
        ResponsePayload::Rejected {
            code,
            detail: err_detail,
        } => Err(BridgeError::from(IpcError::new(code, err_detail))),
        _ => Err(unexpected("audio_options")),
    }
}

/// The audio options.
pub fn daemon_audio_options() -> CmdResult<narrivy_ipc::AudioOptionsDto> {
    audio_options_request(&Command::GetAudioOptions)
}

/// Turn removing the computer's sound from the microphone on or off.
pub fn daemon_set_echo_cancel(on: bool) -> CmdResult<narrivy_ipc::AudioOptionsDto> {
    audio_options_request(&Command::SetAudioOptions {
        echo_cancel: Some(on),
        only_my_voice: None,
        noise_suppression: None,
        microphone: None,
        sounds: None,
        voice_strictness: None,
    })
}

/// Keep only the user's voice (once trained), or not.
pub fn daemon_set_only_my_voice(on: bool) -> CmdResult<narrivy_ipc::AudioOptionsDto> {
    voice_request(&Command::SetAudioOptions {
        echo_cancel: None,
        only_my_voice: Some(on),
        noise_suppression: None,
        microphone: None,
        sounds: None,
        voice_strictness: None,
    })
}

/// How much background noise to remove: `off`, `standard` or `strong`.
pub fn daemon_set_noise_suppression(level: String) -> CmdResult<narrivy_ipc::AudioOptionsDto> {
    let client = connect(&socket_path())?;
    if client.protocol_minor() < narrivy_ipc::NOISE_SUPPRESSION_PROTOCOL_MINOR {
        return Err(BridgeError::new(
            "incompatible_protocol",
            "the daemon has no noise suppression levels; update Narrivy",
        ));
    }
    drop(client);
    audio_options_request(&Command::SetAudioOptions {
        echo_cancel: None,
        only_my_voice: None,
        noise_suppression: Some(level),
        microphone: None,
        sounds: None,
        voice_strictness: None,
    })
}

/// Send a vocabulary command (protocol minor 30).
fn vocabulary_request(command: &Command) -> CmdResult<narrivy_ipc::VocabularyDto> {
    let mut client = connect(&socket_path())?;
    if client.protocol_minor() < narrivy_ipc::VOCABULARY_PROTOCOL_MINOR {
        return Err(BridgeError::new(
            "incompatible_protocol",
            "the daemon has no vocabulary; update Narrivy",
        ));
    }
    match client.request(command)? {
        ResponsePayload::Vocabulary(vocabulary) => Ok(vocabulary),
        ResponsePayload::Rejected {
            code,
            detail: err_detail,
        } => Err(BridgeError::from(IpcError::new(code, err_detail))),
        _ => Err(unexpected("vocabulary")),
    }
}

/// The user's vocabulary.
pub fn daemon_vocabulary() -> CmdResult<narrivy_ipc::VocabularyDto> {
    vocabulary_request(&Command::GetVocabulary)
}

/// Replace the user's vocabulary; the answer is it as saved.
pub fn daemon_set_vocabulary(
    vocabulary: narrivy_ipc::VocabularyDto,
) -> CmdResult<narrivy_ipc::VocabularyDto> {
    vocabulary_request(&Command::SetVocabulary { vocabulary })
}

/// Choose the microphone by name; `""` follows the system default.
pub fn daemon_set_microphone(name: String) -> CmdResult<narrivy_ipc::AudioOptionsDto> {
    let client = connect(&socket_path())?;
    if client.protocol_minor() < narrivy_ipc::MICROPHONE_PROTOCOL_MINOR {
        return Err(BridgeError::new(
            "incompatible_protocol",
            "the daemon cannot choose a microphone; update Narrivy",
        ));
    }
    drop(client);
    audio_options_request(&Command::SetAudioOptions {
        echo_cancel: None,
        only_my_voice: None,
        noise_suppression: None,
        microphone: Some(name),
        sounds: None,
        voice_strictness: None,
    })
}

/// Send a press-and-hold command (protocol minor 32).
fn hold_request(command: &Command) -> CmdResult<narrivy_ipc::HoldKeyDto> {
    let mut client = connect(&socket_path())?;
    if client.protocol_minor() < narrivy_ipc::HOLD_KEY_PROTOCOL_MINOR {
        return Err(BridgeError::new(
            "incompatible_protocol",
            "the daemon has no press and hold; update Narrivy",
        ));
    }
    match client.request(command)? {
        ResponsePayload::HoldKey(hold) => Ok(hold),
        ResponsePayload::Rejected {
            code,
            detail: err_detail,
        } => Err(BridgeError::from(IpcError::new(code, err_detail))),
        _ => Err(unexpected("hold_key")),
    }
}

/// Press and hold: the chosen key and whether it works.
pub fn daemon_hold_key() -> CmdResult<narrivy_ipc::HoldKeyDto> {
    hold_request(&Command::GetHoldKey)
}

/// Choose the key to hold, or none.
pub fn daemon_set_hold_key(key: Option<u16>) -> CmdResult<narrivy_ipc::HoldKeyDto> {
    hold_request(&Command::SetHoldKey { key })
}

/// Start and stop sounds on or off.
pub fn daemon_set_sounds(on: bool) -> CmdResult<narrivy_ipc::AudioOptionsDto> {
    let client = connect(&socket_path())?;
    if client.protocol_minor() < narrivy_ipc::SOUNDS_PROTOCOL_MINOR {
        return Err(BridgeError::new(
            "incompatible_protocol",
            "the daemon has no start and stop sounds; update Narrivy",
        ));
    }
    drop(client);
    audio_options_request(&Command::SetAudioOptions {
        echo_cancel: None,
        only_my_voice: None,
        noise_suppression: None,
        microphone: None,
        sounds: Some(on),
        voice_strictness: None,
    })
}

/// A History settings command (protocol minor 34).
fn history_settings_request(command: &Command) -> CmdResult<()> {
    let mut client = connect(&socket_path())?;
    if client.protocol_minor() < narrivy_ipc::HISTORY_SETTINGS_PROTOCOL_MINOR {
        return Err(BridgeError::new(
            "incompatible_protocol",
            "the daemon cannot change History settings; update Narrivy",
        ));
    }
    match client.request(command)? {
        ResponsePayload::Accepted => Ok(()),
        ResponsePayload::Rejected {
            code,
            detail: err_detail,
        } => Err(BridgeError::from(IpcError::new(code, err_detail))),
        _ => Err(unexpected("history_settings")),
    }
}

/// How long History is kept: `off`, `7d`, `30d` or `forever`.
pub fn daemon_set_history_retention(retention: String) -> CmdResult<()> {
    history_settings_request(&Command::SetHistoryRetention { retention })
}

/// Remove every History entry.
pub fn daemon_clear_history() -> CmdResult<()> {
    history_settings_request(&Command::ClearHistory)
}

/// Text options (protocol minor 35).
fn text_options_request(command: &Command) -> CmdResult<narrivy_ipc::TextOptionsDto> {
    let mut client = connect(&socket_path())?;
    if client.protocol_minor() < narrivy_ipc::TEXT_OPTIONS_PROTOCOL_MINOR {
        return Err(BridgeError::new(
            "incompatible_protocol",
            "the daemon has no text options; update Narrivy",
        ));
    }
    match client.request(command)? {
        ResponsePayload::TextOptions(options) => Ok(options),
        ResponsePayload::Rejected {
            code,
            detail: err_detail,
        } => Err(BridgeError::from(IpcError::new(code, err_detail))),
        _ => Err(unexpected("text_options")),
    }
}

pub fn daemon_text_options() -> CmdResult<narrivy_ipc::TextOptionsDto> {
    text_options_request(&Command::GetTextOptions)
}

pub fn daemon_set_numbers_as_digits(on: bool) -> CmdResult<narrivy_ipc::TextOptionsDto> {
    text_options_request(&Command::SetTextOptions {
        numbers_as_digits: Some(on),
        delivery_method: None,
        cleanup_idle_minutes: None,
        spoken_breaks: None,
    })
}

/// Unload the clean-up model after `minutes` idle; 0 never.
pub fn daemon_set_cleanup_idle(minutes: u32) -> CmdResult<narrivy_ipc::TextOptionsDto> {
    text_options_request(&Command::SetTextOptions {
        numbers_as_digits: None,
        delivery_method: None,
        cleanup_idle_minutes: Some(minutes),
        spoken_breaks: None,
    })
}

/// "new line" / "new paragraph" as line breaks on Clean.
pub fn daemon_set_spoken_breaks(on: bool) -> CmdResult<narrivy_ipc::TextOptionsDto> {
    text_options_request(&Command::SetTextOptions {
        numbers_as_digits: None,
        delivery_method: None,
        cleanup_idle_minutes: None,
        spoken_breaks: Some(on),
    })
}

/// Paste (clipboard, then put back) or type.
pub fn daemon_set_delivery_method(method: String) -> CmdResult<narrivy_ipc::TextOptionsDto> {
    text_options_request(&Command::SetTextOptions {
        numbers_as_digits: None,
        delivery_method: Some(method),
        cleanup_idle_minutes: None,
        spoken_breaks: None,
    })
}

/// Give a pipeline its own Voice settings; `None` follows the default.
pub fn daemon_set_pipeline_voice(
    pipeline_id: String,
    echo_cancel: Option<bool>,
    only_my_voice: Option<bool>,
) -> CmdResult<narrivy_ipc::AudioOptionsDto> {
    let client = connect(&socket_path())?;
    if client.protocol_minor() < narrivy_ipc::PIPELINE_VOICE_PROTOCOL_MINOR {
        return Err(BridgeError::new(
            "incompatible_protocol",
            "the daemon has no per-pipeline Voice settings; update Narrivy",
        ));
    }
    drop(client);
    audio_options_request(&Command::SetPipelineVoice {
        pipeline_id,
        echo_cancel,
        only_my_voice,
    })
}

/// Import an audio or video file as a meeting (protocol minor 37).
pub fn daemon_meeting_import(path: String) -> CmdResult<narrivy_ipc::MeetingDto> {
    let mut client = connect(&socket_path())?;
    if client.protocol_minor() < narrivy_ipc::MEETING_IMPORT_PROTOCOL_MINOR {
        return Err(BridgeError::new(
            "incompatible_protocol",
            "the daemon cannot import recordings; update Narrivy",
        ));
    }
    match client.request(&Command::ImportMeeting { path, title: None })? {
        ResponsePayload::Meeting(meeting) => Ok(meeting),
        ResponsePayload::Rejected {
            code,
            detail: err_detail,
        } => Err(BridgeError::from(IpcError::new(code, err_detail))),
        _ => Err(unexpected("import_meeting")),
    }
}

/// A meeting-options command (protocol minor 39).
fn meeting_options_request(command: &Command) -> CmdResult<narrivy_ipc::MeetingOptionsDto> {
    let mut client = connect(&socket_path())?;
    if client.protocol_minor() < narrivy_ipc::MEETING_OPTIONS_PROTOCOL_MINOR {
        return Err(BridgeError::new(
            "incompatible_protocol",
            "the daemon has no meeting options; update Narrivy",
        ));
    }
    match client.request(command)? {
        ResponsePayload::MeetingOptions(options) => Ok(options),
        ResponsePayload::Rejected {
            code,
            detail: err_detail,
        } => Err(BridgeError::from(IpcError::new(code, err_detail))),
        _ => Err(unexpected("meeting_options")),
    }
}

pub fn daemon_meeting_options() -> CmdResult<narrivy_ipc::MeetingOptionsDto> {
    meeting_options_request(&Command::GetMeetingOptions)
}

pub fn daemon_set_auto_notes(on: bool) -> CmdResult<narrivy_ipc::MeetingOptionsDto> {
    meeting_options_request(&Command::SetMeetingOptions {
        auto_notes: Some(on),
        microphone: None,
        speaker: None,
        call_audio: None,
        room_audio: None,
        screen: None,
        process_on_stop: None,
    })
}

/// Keep pictures of the screen in meetings (protocol minor 45).
pub fn daemon_set_meeting_screen(on: bool) -> CmdResult<narrivy_ipc::MeetingOptionsDto> {
    let client = connect(&socket_path())?;
    if client.protocol_minor() < narrivy_ipc::MEETING_SCREEN_PROTOCOL_MINOR {
        return Err(BridgeError::new(
            "incompatible_protocol",
            "the daemon cannot keep the screen in meetings; update Narrivy",
        ));
    }
    drop(client);
    meeting_options_request(&Command::SetMeetingOptions {
        auto_notes: None,
        microphone: None,
        speaker: None,
        call_audio: None,
        room_audio: None,
        screen: Some(on),
        process_on_stop: None,
    })
}

/// Go over a recording as soon as it stops, or keep it as heard until
/// asked (protocol minor 52).
pub fn daemon_set_process_on_stop(on: bool) -> CmdResult<narrivy_ipc::MeetingOptionsDto> {
    let client = connect(&socket_path())?;
    if client.protocol_minor() < narrivy_ipc::MEETING_QUICK_SAVE_PROTOCOL_MINOR {
        return Err(BridgeError::new(
            "incompatible_protocol",
            "the daemon always goes over a recording when it stops; update Narrivy",
        ));
    }
    drop(client);
    meeting_options_request(&Command::SetMeetingOptions {
        auto_notes: None,
        microphone: None,
        speaker: None,
        call_audio: None,
        room_audio: None,
        screen: None,
        process_on_stop: Some(on),
    })
}

/// A meeting's screen picture, small, as base64 PNG (protocol minor 45).
pub fn daemon_meeting_slide(id: String, file: String) -> CmdResult<String> {
    let mut client = connect(&socket_path())?;
    if client.protocol_minor() < narrivy_ipc::MEETING_SCREEN_PROTOCOL_MINOR {
        return Err(BridgeError::new(
            "incompatible_protocol",
            "the daemon has no meeting screens; update Narrivy",
        ));
    }
    match client.request(&Command::GetMeetingSlide { id, file })? {
        ResponsePayload::MeetingSlide(png) => Ok(png),
        ResponsePayload::Rejected {
            code,
            detail: err_detail,
        } => Err(BridgeError::from(IpcError::new(code, err_detail))),
        _ => Err(BridgeError::new("internal", "unexpected payload for meeting_slide")),
    }
}

/// The meeting's microphone and output; `""` goes back to the default.
pub fn daemon_set_meeting_devices(
    microphone: Option<String>,
    speaker: Option<String>,
    call_audio: Option<bool>,
    room_audio: Option<bool>,
) -> CmdResult<narrivy_ipc::MeetingOptionsDto> {
    let client = connect(&socket_path())?;
    if client.protocol_minor() < narrivy_ipc::MEETING_DEVICES_PROTOCOL_MINOR {
        return Err(BridgeError::new(
            "incompatible_protocol",
            "the daemon cannot choose meeting devices; update Narrivy",
        ));
    }
    drop(client);
    meeting_options_request(&Command::SetMeetingOptions {
        auto_notes: None,
        microphone,
        speaker,
        call_audio,
        room_audio,
        screen: None,
        process_on_stop: None,
    })
}

/// How strictly "Only my voice" keeps only the user.
pub fn daemon_set_voice_strictness(level: String) -> CmdResult<narrivy_ipc::AudioOptionsDto> {
    let client = connect(&socket_path())?;
    if client.protocol_minor() < narrivy_ipc::VOICE_STRICTNESS_PROTOCOL_MINOR {
        return Err(BridgeError::new(
            "incompatible_protocol",
            "the daemon has no voice strictness; update Narrivy",
        ));
    }
    drop(client);
    audio_options_request(&Command::SetAudioOptions {
        echo_cancel: None,
        only_my_voice: None,
        noise_suppression: None,
        microphone: None,
        sounds: None,
        voice_strictness: Some(level),
    })
}

/// Start recording the user's voice for training.
pub fn daemon_train_voice() -> CmdResult<narrivy_ipc::AudioOptionsDto> {
    voice_request(&Command::TrainVoice)
}

/// Forget the trained voice.
pub fn daemon_forget_voice() -> CmdResult<narrivy_ipc::AudioOptionsDto> {
    voice_request(&Command::ForgetVoice)
}

/// A voice-filter command (protocol minor 28).
fn voice_request(command: &Command) -> CmdResult<narrivy_ipc::AudioOptionsDto> {
    let client = connect(&socket_path())?;
    if client.protocol_minor() < narrivy_ipc::VOICE_PROTOCOL_MINOR {
        return Err(BridgeError::new(
            "incompatible_protocol",
            "the daemon cannot learn your voice; update Narrivy",
        ));
    }
    drop(client);
    audio_options_request(command)
}

fn unexpected(what: &str) -> BridgeError {
    BridgeError::new("internal", format!("unexpected payload for {what}"))
}

/// Start recording a meeting.
pub fn daemon_meeting_start(title: Option<String>, watch: bool) -> CmdResult<narrivy_ipc::MeetingDto> {
    if watch {
        let client = connect(&socket_path())?;
        if client.protocol_minor() < narrivy_ipc::ASSISTANT_AGENT_PROTOCOL_MINOR {
            return Err(BridgeError::new(
                "incompatible_protocol",
                "the daemon cannot watch with you; update Narrivy",
            ));
        }
    }
    let start = Command::MeetingStart {
        title,
        watch,
        share: false,
        microphone: None,
        computer_sound: None,
    };
    match meeting_request(&start)? {
        ResponsePayload::Meeting(meeting) => Ok(meeting),
        _ => Err(unexpected("meeting_start")),
    }
}

/// Share live with the open chat (`share`) or record a session: the screen
/// (`screen`) and/or the sound, the microphone and the computer's sound as
/// chosen for this one (protocol minor 49).
pub fn daemon_screen_start(
    title: Option<String>,
    screen: bool,
    share: bool,
    microphone: bool,
    computer_sound: bool,
) -> CmdResult<narrivy_ipc::MeetingDto> {
    let client = connect(&socket_path())?;
    if client.protocol_minor() < narrivy_ipc::SCREEN_SESSIONS_PROTOCOL_MINOR {
        return Err(BridgeError::new(
            "incompatible_protocol",
            "the daemon cannot share or record the screen; update Narrivy",
        ));
    }
    drop(client);
    let start = Command::MeetingStart {
        title,
        watch: screen,
        share,
        microphone: Some(microphone),
        computer_sound: Some(computer_sound),
    };
    match meeting_request(&start)? {
        ResponsePayload::Meeting(meeting) => Ok(meeting),
        _ => Err(unexpected("meeting_start")),
    }
}

/// Stop the meeting being recorded; transcribing starts.
pub fn daemon_meeting_stop() -> CmdResult<()> {
    match meeting_request(&Command::MeetingStop)? {
        ResponsePayload::Accepted => Ok(()),
        _ => Err(unexpected("meeting_stop")),
    }
}

/// Pause, resume or stop a meeting's processing ("stop" while recording
/// stops the recording; protocol minor 48).
pub fn daemon_meeting_control(id: String, action: String) -> CmdResult<()> {
    let client = connect(&socket_path())?;
    if client.protocol_minor() < narrivy_ipc::MEETING_CONTROL_PROTOCOL_MINOR {
        return Err(BridgeError::new(
            "incompatible_protocol",
            "the daemon cannot pause or stop a meeting's processing; update Narrivy",
        ));
    }
    drop(client);
    match meeting_request(&Command::MeetingControl { id, action })? {
        ResponsePayload::Accepted => Ok(()),
        _ => Err(unexpected("meeting_control")),
    }
}

/// Switch the screen or the sound of the meeting being recorded on or off
/// (protocol minor 50).
pub fn daemon_meeting_sources(
    id: String,
    screen: Option<bool>,
    sound: Option<bool>,
    microphone: Option<bool>,
) -> CmdResult<narrivy_ipc::MeetingDto> {
    let client = connect(&socket_path())?;
    if client.protocol_minor() < narrivy_ipc::MEETING_SOURCES_PROTOCOL_MINOR {
        return Err(BridgeError::new(
            "incompatible_protocol",
            "the daemon cannot switch the screen or the sound during a recording; update Narrivy",
        ));
    }
    if microphone.is_some() && client.protocol_minor() < narrivy_ipc::MEETING_MICROPHONE_PROTOCOL_MINOR {
        return Err(BridgeError::new(
            "incompatible_protocol",
            "the daemon cannot switch the microphone apart during a recording; update Narrivy",
        ));
    }
    drop(client);
    match meeting_request(&Command::MeetingSources {
        id,
        screen,
        sound,
        microphone,
    })? {
        ResponsePayload::Meeting(meeting) => Ok(meeting),
        _ => Err(unexpected("meeting_sources")),
    }
}

/// Every meeting, newest first.
pub fn daemon_meetings() -> CmdResult<Vec<narrivy_ipc::MeetingDto>> {
    match meeting_request(&Command::ListMeetings)? {
        ResponsePayload::Meetings(meetings) => Ok(meetings),
        _ => Err(unexpected("list_meetings")),
    }
}

/// One meeting with its transcript.
pub fn daemon_meeting(id: String) -> CmdResult<narrivy_ipc::MeetingDetailDto> {
    match meeting_request(&Command::GetMeeting { id })? {
        ResponsePayload::MeetingDetail(detail) => Ok(detail),
        _ => Err(unexpected("get_meeting")),
    }
}

/// Transcribe a meeting again with another model or speaker counts.
pub fn daemon_meeting_process(
    id: String,
    model: Option<String>,
    room_speakers: Option<u16>,
    call_speakers: Option<u16>,
) -> CmdResult<()> {
    match meeting_request(&Command::ProcessMeeting {
        id,
        model,
        room_speakers,
        call_speakers,
    })? {
        ResponsePayload::Accepted => Ok(()),
        _ => Err(unexpected("process_meeting")),
    }
}

/// Name a speaker of a meeting.
pub fn daemon_meeting_rename(id: String, speaker: String, name: String) -> CmdResult<()> {
    match meeting_request(&Command::RenameMeetingSpeaker { id, speaker, name })? {
        ResponsePayload::Accepted => Ok(()),
        _ => Err(unexpected("rename_meeting_speaker")),
    }
}

/// Write notes for a meeting with the assistant model (protocol minor 26);
/// they stream as assistant events under `turn_id` and are saved.
pub fn daemon_meeting_summarize(
    id: String,
    turn_id: String,
    kind: String,
    instruction: Option<String>,
) -> CmdResult<()> {
    let mut client = connect(&socket_path())?;
    if client.protocol_minor() < narrivy_ipc::MEETING_NOTES_PROTOCOL_MINOR {
        return Err(BridgeError::new(
            "incompatible_protocol",
            "the daemon cannot write meeting notes; update Narrivy",
        ));
    }
    match client.request(&Command::SummarizeMeeting {
        id,
        turn_id,
        kind,
        instruction,
    })? {
        ResponsePayload::Accepted => Ok(()),
        ResponsePayload::Rejected {
            code,
            detail: err_detail,
        } => Err(BridgeError::from(IpcError::new(code, err_detail))),
        _ => Err(unexpected("summarize_meeting")),
    }
}

/// Delete a meeting's recording and transcript.
pub fn daemon_meeting_delete(id: String) -> CmdResult<()> {
    match meeting_request(&Command::DeleteMeeting { id })? {
        ResponsePayload::Accepted => Ok(()),
        _ => Err(unexpected("delete_meeting")),
    }
}

fn assistant_request(command: &Command) -> CmdResult<()> {
    let mut client = connect(&socket_path())?;
    if client.protocol_minor() < narrivy_ipc::ASSISTANT_PROTOCOL_MINOR {
        return Err(BridgeError::new(
            "incompatible_protocol",
            "the daemon has no assistant; update Narrivy",
        ));
    }
    match client.request(command)? {
        ResponsePayload::Accepted => Ok(()),
        ResponsePayload::Rejected {
            code,
            detail: err_detail,
        } => Err(BridgeError::from(IpcError::new(code, err_detail))),
        _ => Err(BridgeError::new(
            "internal",
            "unexpected payload for the assistant",
        )),
    }
}

/// Remove an installed Store item. Requires protocol minor 17 or newer.
pub fn daemon_store_remove(id: String) -> CmdResult<()> {
    let mut client = connect(&socket_path())?;
    if client.protocol_minor() < narrivy_ipc::STORE_PROTOCOL_MINOR {
        return Err(BridgeError::new(
            "incompatible_protocol",
            "the daemon has no Store; update Narrivy",
        ));
    }
    match client.request(&Command::RemoveStoreItem { id })? {
        ResponsePayload::Accepted => Ok(()),
        ResponsePayload::Rejected {
            code,
            detail: err_detail,
        } => Err(BridgeError::from(IpcError::new(code, err_detail))),
        _ => Err(BridgeError::new(
            "internal",
            "unexpected payload for remove_store_item",
        )),
    }
}

/// Set the model and device one pipeline recognises with. Requires protocol
/// minor 15 or newer.
pub fn daemon_pipeline_engine_set(
    pipeline_id: String,
    model_id: Option<String>,
    compute: String,
) -> CmdResult<()> {
    let mut client = connect(&socket_path())?;
    if client.protocol_minor() < narrivy_ipc::PIPELINE_ENGINE_PROTOCOL_MINOR {
        return Err(BridgeError::new(
            "incompatible_protocol",
            "the daemon cannot set a pipeline's model; update Narrivy",
        ));
    }
    match client.request(&Command::SetPipelineEngine {
        pipeline_id,
        model_id,
        compute,
    })? {
        ResponsePayload::Accepted => Ok(()),
        ResponsePayload::Rejected {
            code,
            detail: err_detail,
        } => Err(BridgeError::from(IpcError::new(code, err_detail))),
        _ => Err(BridgeError::new(
            "internal",
            "unexpected payload for set_pipeline_engine",
        )),
    }
}

/// Connect for a shortcut command; an older daemon is refused here so the
/// request is never silently ignored.
fn connect_for_shortcuts() -> CmdResult<IpcClientRole> {
    let client = connect(&socket_path())?;
    if client.protocol_minor() < SHORTCUT_BINDINGS_PROTOCOL_MINOR {
        return Err(BridgeError::new(
            "incompatible_protocol",
            "the daemon cannot manage shortcuts; update Narrivy",
        ));
    }
    Ok(client)
}

/// The shortcuts Narrivy manages in the desktop's settings, and whether it
/// can manage them at all.
pub fn daemon_shortcuts_list() -> CmdResult<ShortcutsDto> {
    let mut client = connect_for_shortcuts()?;
    match client.request(&Command::ListShortcuts)? {
        ResponsePayload::Shortcuts(shortcuts) => Ok(shortcuts),
        ResponsePayload::Rejected {
            code,
            detail: err_detail,
        } => Err(BridgeError::from(IpcError::new(code, err_detail))),
        _ => Err(BridgeError::new(
            "internal",
            "unexpected payload for list_shortcuts",
        )),
    }
}

/// Bind a key combination to a pipeline. A combination already in use comes
/// back as a `conflict` outcome unless `replace` is set.
pub fn daemon_shortcut_set(
    binding: ShortcutBindingDto,
    replace: bool,
) -> CmdResult<ShortcutSetDto> {
    let mut client = connect_for_shortcuts()?;
    match client.request(&Command::SetShortcut { binding, replace })? {
        ResponsePayload::ShortcutSet(result) => Ok(result),
        ResponsePayload::Rejected {
            code,
            detail: err_detail,
        } => Err(BridgeError::from(IpcError::new(code, err_detail))),
        _ => Err(BridgeError::new(
            "internal",
            "unexpected payload for set_shortcut",
        )),
    }
}

/// Remove one shortcut Narrivy created.
pub fn daemon_shortcut_remove(modifiers: Vec<String>, key: String) -> CmdResult<()> {
    let mut client = connect_for_shortcuts()?;
    match client.request(&Command::RemoveShortcut { modifiers, key })? {
        ResponsePayload::Accepted => Ok(()),
        ResponsePayload::Rejected {
            code,
            detail: err_detail,
        } => Err(BridgeError::from(IpcError::new(code, err_detail))),
        _ => Err(BridgeError::new(
            "internal",
            "unexpected payload for remove_shortcut",
        )),
    }
}

/// Fetch the exact short-lived recovery result named by a session event.
/// This command never lists recovery records and is not optional History.
pub fn daemon_recovery_entry(txid: String) -> CmdResult<RecoveryEntryDetailDto> {
    validate_txid(&txid)?;
    let mut client = connect(&socket_path())?;
    match client.request(&Command::GetRecoveryEntry { txid })? {
        ResponsePayload::RecoveryEntry(detail) => Ok(detail),
        ResponsePayload::Rejected {
            code,
            detail: err_detail,
        } => Err(BridgeError::from(IpcError::new(code, err_detail))),
        _ => Err(BridgeError::new(
            "internal",
            "unexpected payload for get_recovery_entry",
        )),
    }
}

fn validate_txid(txid: &str) -> CmdResult<()> {
    if !(1..=64).contains(&txid.len())
        || !txid
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        || txid == "."
        || txid == ".."
    {
        return Err(BridgeError::new("bad_request", "invalid txid shape"));
    }
    Ok(())
}

/// Core capability check shared by the command and the setup-hook marker.
pub fn run_capability_check(
    socket: &Path,
    id: String,
) -> Result<CapabilityCheckResult, BridgeError> {
    if id.len() > 32 || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return Err(BridgeError::new(
            "bad_request",
            "invalid capability check id",
        ));
    }
    let mut client = connect(socket)?;
    match client.request(&Command::RunCapabilityCheck {
        check_id: id.clone(),
    })? {
        ResponsePayload::CapabilityCheck(dto) => Ok(CapabilityCheckResult {
            id: dto.id,
            // Frontend vocabulary is pass/fail; wire vocabulary is available.
            pass: dto.available,
            detail: dto.detail,
        }),
        ResponsePayload::Rejected { code, detail } => {
            Err(BridgeError::from(IpcError::new(code, detail)))
        }
        _ => Err(BridgeError::new(
            "internal",
            "unexpected payload for run_capability_check",
        )),
    }
}

/// Run exactly one isolated capability check by id:
/// `mic | delivery | shortcuts | theme-store` (closed set server-side).
pub fn daemon_capability_check(id: String) -> CmdResult<CapabilityCheckResult> {
    run_capability_check(&socket_path(), id)
}

// ---------------------------------------------------------------------------
// P7 FUN-033: versioned pipeline export/import (UX-011).
//
// The desktop shell is a client: it never invents pipeline policy. Export
// writes the daemon's saved-pipeline registry (real narrivy-pipeline types)
// as ONE versioned JSON file. Import validates the file, refuses any
// secret-like key BY CONSTRUCTION, previews required dependencies, and only
// then merges into the registry — `dry_run` reports without writing.
// ---------------------------------------------------------------------------

/// Envelope for an exported pipelines file. `schema_version` bumps when the
/// shape changes; importers reject newer versions explicitly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelinesExportFile {
    pub schema_version: u32,
    /// Stable kind marker: "narrivy-pipelines-export".
    pub kind: String,
    pub exported_at_unix_ms: u64,
    pub pipelines: Vec<narrivy_pipeline::PipelineDefinition>,
}

pub const PIPELINES_EXPORT_SCHEMA_VERSION: u32 = 1;
const PIPELINES_EXPORT_KIND: &str = "narrivy-pipelines-export";

/// Keys that must NEVER travel inside a pipeline export/import. Registry
/// entries cannot contain them by construction; this scan is defense in
/// depth so a tampered/hand-authored file is refused, not merged.
pub const FORBIDDEN_IMPORT_KEYS: &[&str] = &[
    "secret",
    "token",
    "api_key",
    "apikey",
    "password",
    "credential",
    "authorization",
    "url",
    "qualification-cache",
];

#[derive(Debug, Clone, Serialize)]
pub struct PipelinesExportReport {
    pub count: usize,
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PipelinesImportReport {
    pub dry_run: bool,
    pub imported: usize,
    pub skipped_duplicates: usize,
    /// Required dependencies of the imported set (namespace:name@version_req),
    /// surfaced to the UI BEFORE anything is applied.
    pub required_dependencies: Vec<String>,
}

/// Resolve the saved-pipelines directory used by export/import:
/// `NARRIVY_PIPELINES_DIR` wins, else `$XDG_RUNTIME_DIR/narrivy/pipelines`.
pub fn resolve_pipelines_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("NARRIVY_PIPELINES_DIR").filter(|v| !v.is_empty()) {
        return PathBuf::from(dir);
    }
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(base) => PathBuf::from(base).join("narrivy").join("pipelines"),
        None => PathBuf::from("/tmp").join("narrivy").join("pipelines"),
    }
}

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Recursive key scan over a parsed JSON value; returns offending keys.
fn find_forbidden_keys(value: &serde_json::Value) -> Vec<String> {
    let mut hits = Vec::new();
    match value {
        serde_json::Value::Object(map) => {
            for (key, inner) in map {
                let lower = key.to_ascii_lowercase();
                if FORBIDDEN_IMPORT_KEYS.iter().any(|bad| lower == *bad) {
                    hits.push(key.clone());
                }
                hits.extend(find_forbidden_keys(inner));
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                hits.extend(find_forbidden_keys(item));
            }
        }
        _ => {}
    }
    hits
}

/// Export the saved pipelines from `registry_dir` into `out_path`.
pub fn pipelines_export_to(
    registry_dir: &Path,
    out_path: &Path,
) -> Result<PipelinesExportReport, BridgeError> {
    let report =
        narrivy_pipeline::PipelineRegistry::load_from_dir(registry_dir).map_err(|err| {
            BridgeError::new(
                "bad_request",
                format!("pipeline registry unreadable: {err}"),
            )
        })?;
    let defs: Vec<_> = report.registry.definitions();
    let file = PipelinesExportFile {
        schema_version: PIPELINES_EXPORT_SCHEMA_VERSION,
        kind: PIPELINES_EXPORT_KIND.to_owned(),
        exported_at_unix_ms: now_unix_ms(),
        pipelines: defs,
    };
    // Defense in depth: what we write must itself pass the forbidden-key scan.
    let json = serde_json::to_string_pretty(&file)
        .map_err(|err| BridgeError::new("internal", format!("export serialize failed: {err}")))?;
    let parsed: serde_json::Value = serde_json::from_str(&json)
        .map_err(|err| BridgeError::new("internal", format!("export re-parse failed: {err}")))?;
    let hits = find_forbidden_keys(&parsed);
    if !hits.is_empty() {
        return Err(BridgeError::new(
            "bad_request",
            format!("export refused: forbidden keys present: {hits:?}"),
        ));
    }
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| {
            BridgeError::new("internal", format!("export dir create failed: {err}"))
        })?;
    }
    std::fs::write(out_path, format!("{json}\n"))
        .map_err(|err| BridgeError::new("internal", format!("export write failed: {err}")))?;
    Ok(PipelinesExportReport {
        count: file.pipelines.len(),
        path: out_path.to_string_lossy().into_owned(),
    })
}

/// Import pipelines from `in_path` into `registry_dir`. When `dry_run` is
/// true nothing is written and the required-dependency preview is returned.
pub fn pipelines_import_from(
    registry_dir: &Path,
    in_path: &Path,
    dry_run: bool,
) -> Result<PipelinesImportReport, BridgeError> {
    let text = std::fs::read_to_string(in_path)
        .map_err(|err| BridgeError::new("bad_request", format!("import read failed: {err}")))?;
    let parsed: serde_json::Value = serde_json::from_str(&text)
        .map_err(|err| BridgeError::new("bad_request", format!("import parse failed: {err}")))?;

    // Refuse secrets / credential-like keys BEFORE typed parsing.
    let hits = find_forbidden_keys(&parsed);
    if !hits.is_empty() {
        return Err(BridgeError::new(
            "bad_request",
            format!("import refused: forbidden keys present: {hits:?}"),
        ));
    }

    let file: PipelinesExportFile = serde_json::from_value(parsed).map_err(|err| {
        // Closed enums (slots, kinds) make unknown values fail HERE — a
        // tampered file with an unknown slot is rejected by construction.
        BridgeError::new("bad_request", format!("import validation failed: {err}"))
    })?;
    if file.schema_version > PIPELINES_EXPORT_SCHEMA_VERSION {
        return Err(BridgeError::new(
            "bad_request",
            format!(
                "import refused: schema_version {} > supported {}",
                file.schema_version, PIPELINES_EXPORT_SCHEMA_VERSION
            ),
        ));
    }
    if file.kind != PIPELINES_EXPORT_KIND {
        return Err(BridgeError::new(
            "bad_request",
            format!("import refused: unexpected kind {:?}", file.kind),
        ));
    }

    // Required-dependencies preview: every module ref the imported set needs.
    let mut deps: Vec<String> = Vec::new();
    for def in &file.pipelines {
        for node in &def.nodes {
            let dep = format!(
                "{}:{}@{}",
                node.module.namespace, node.module.name, node.module.version_req
            );
            if !deps.contains(&dep) {
                deps.push(dep);
            }
        }
    }

    if dry_run {
        return Ok(PipelinesImportReport {
            dry_run: true,
            imported: 0,
            skipped_duplicates: 0,
            required_dependencies: deps,
        });
    }

    let mut report = narrivy_pipeline::PipelineRegistry::load_from_dir(registry_dir)
        .unwrap_or_else(|_| narrivy_pipeline::LoadReport {
            registry: narrivy_pipeline::PipelineRegistry::new(),
            skipped: Vec::new(),
            migrated: Vec::new(),
        });
    let mut imported = 0usize;
    let mut duplicates = 0usize;
    for def in file.pipelines {
        if report.registry.get(&def.id).is_some() {
            duplicates += 1;
            continue;
        }
        report.registry.insert(def).map_err(|err| {
            BridgeError::new("bad_request", format!("import insert failed: {err}"))
        })?;
        imported += 1;
    }
    report
        .registry
        .save_to_dir(registry_dir)
        .map_err(|err| BridgeError::new("internal", format!("import save failed: {err}")))?;
    Ok(PipelinesImportReport {
        dry_run: false,
        imported,
        skipped_duplicates: duplicates,
        required_dependencies: deps,
    })
}

/// Export saved pipelines to `path` (versioned JSON).
pub fn pipelines_export(path: String) -> CmdResult<PipelinesExportReport> {
    pipelines_export_to(&resolve_pipelines_dir(), &PathBuf::from(path))
}

/// Import pipelines from `path`; `dry_run` returns the
/// dependency preview without writing.
pub fn pipelines_import(path: String, dry_run: bool) -> CmdResult<PipelinesImportReport> {
    pipelines_import_from(&resolve_pipelines_dir(), &PathBuf::from(path), dry_run)
}

#[cfg(test)]
mod p7_pipelines_io_tests {
    use super::*;
    use narrivy_pipeline::{CapabilitySlot, PipelineDefinition, PipelineNode, ResourceRef};

    #[test]
    fn reusable_bridge_core_has_no_tauri_api_dependency() {
        let forbidden = ["tauri", "::"].concat();
        assert!(!include_str!("bridge.rs").contains(&forbidden));
    }

    fn sample_def(id: &str) -> PipelineDefinition {
        let mut def = PipelineDefinition::new(
            id,
            "Sample pipeline",
            vec![PipelineNode {
                id: "asr".to_owned(),
                slot: CapabilitySlot::SpeechAsr,
                module: ResourceRef::narrivy_module("asr-engine", "1"),
                enabled: true,
                optional: false,
                device_preference: Vec::new(),
            }],
        );
        def.manual_only = true;
        def
    }

    fn seed_registry(dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let mut registry = narrivy_pipeline::PipelineRegistry::new();
        registry.insert(sample_def("alpha"))?;
        registry.insert(sample_def("beta"))?;
        registry.save_to_dir(dir)?;
        Ok(())
    }

    #[test]
    fn export_import_round_trip_preserves_definitions() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let registry_dir = tmp.path().join("pipelines");
        seed_registry(&registry_dir)?;
        let export_path = tmp.path().join("out").join("pack.json");

        let exported = pipelines_export_to(&registry_dir, &export_path)?;
        assert_eq!(exported.count, 2);

        // Import into a FRESH registry dir; round trip must preserve both.
        let target_dir = tmp.path().join("target-pipelines");
        let report = pipelines_import_from(&target_dir, &export_path, false)?;
        assert_eq!(report.imported, 2);
        assert_eq!(report.skipped_duplicates, 0);

        let reloaded = narrivy_pipeline::PipelineRegistry::load_from_dir(&target_dir)?;
        assert!(reloaded.registry.get("alpha").is_some());
        assert!(reloaded.registry.get("beta").is_some());
        let Some(alpha) = reloaded.registry.get("alpha") else {
            return Err("round-tripped alpha pipeline is missing".into());
        };
        assert_eq!(alpha.name, "Sample pipeline");
        Ok(())
    }

    #[test]
    fn dry_run_previews_dependencies_without_writing() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let registry_dir = tmp.path().join("pipelines");
        seed_registry(&registry_dir)?;
        let export_path = tmp.path().join("pack.json");
        pipelines_export_to(&registry_dir, &export_path)?;

        let fresh_dir = tmp.path().join("fresh");
        let report = pipelines_import_from(&fresh_dir, &export_path, true)?;
        assert!(report.dry_run);
        assert_eq!(report.imported, 0, "dry run writes nothing");
        assert!(
            !fresh_dir.exists(),
            "dry run must not create the registry dir"
        );
        assert!(
            report
                .required_dependencies
                .iter()
                .all(|d| d.starts_with("narrivy:"))
        );
        Ok(())
    }

    #[test]
    fn import_refuses_forbidden_secret_like_keys() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let mut file = serde_json::to_value(PipelinesExportFile {
            schema_version: PIPELINES_EXPORT_SCHEMA_VERSION,
            kind: PIPELINES_EXPORT_KIND.to_owned(),
            exported_at_unix_ms: 1,
            pipelines: vec![sample_def("gamma")],
        })?;
        // Tamper: a credential-shaped key inside the first pipeline entry.
        file["pipelines"][0]["api_key"] = serde_json::json!("sk-LEAK");
        let path = tmp.path().join("tainted.json");
        std::fs::write(&path, serde_json::to_string(&file)?)?;

        let target = tmp.path().join("target");
        let err = match pipelines_import_from(&target, &path, false) {
            Err(err) => err,
            Ok(_) => return Err("secret-like import was accepted".into()),
        };
        assert_eq!(err.code, "bad_request");
        assert!(err.message.contains("forbidden keys"), "{err}");
        assert!(err.message.contains("api_key"), "{err}");
        // Nothing was written.
        assert!(!target.join("gamma.json").exists());
        Ok(())
    }

    #[test]
    fn import_rejects_unknown_slot_on_tampered_file() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let text =
            r#"{"schema_version":1,"kind":"narrivy-pipelines-export","exported_at_unix_ms":1,
                "pipelines":[{"id":"tampered","version":1,"name":"Tampered",
                "nodes":[{"id":"n1","slot":"text.nonexistent","module":{"namespace":"narrivy",
                "kind":"module","name":"x","version_req":"1"}}]}]}"#
                .to_owned();
        let path = tmp.path().join("tampered.json");
        std::fs::write(&path, text)?;

        let target = tmp.path().join("target");
        let err = match pipelines_import_from(&target, &path, false) {
            Err(err) => err,
            Ok(_) => return Err("unknown-slot import was accepted".into()),
        };
        assert_eq!(err.code, "bad_request");
        assert!(err.message.contains("validation failed"), "{err}");
        assert!(!target.is_dir(), "rejected import must not write state");
        Ok(())
    }

    #[test]
    fn import_rejects_newer_schema_and_wrong_kind() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let export_path = tmp.path().join("pack.json");
        let registry_dir = tmp.path().join("pipelines");
        seed_registry(&registry_dir)?;
        pipelines_export_to(&registry_dir, &export_path)?;

        // Newer schema version.
        let mut value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&export_path)?)?;
        value["schema_version"] = serde_json::json!(PIPELINES_EXPORT_SCHEMA_VERSION + 1);
        let newer = tmp.path().join("newer.json");
        std::fs::write(&newer, serde_json::to_string(&value)?)?;
        let newer_error = match pipelines_import_from(&tmp.path().join("a"), &newer, false) {
            Err(err) => err,
            Ok(_) => return Err("newer-schema import was accepted".into()),
        };
        assert_eq!(newer_error.code, "bad_request");

        // Wrong kind marker.
        value["schema_version"] = serde_json::json!(1);
        value["kind"] = serde_json::json!("something-else");
        let wrong_kind = tmp.path().join("wrong-kind.json");
        std::fs::write(&wrong_kind, serde_json::to_string(&value)?)?;
        let kind_error = match pipelines_import_from(&tmp.path().join("b"), &wrong_kind, false) {
            Err(err) => err,
            Ok(_) => return Err("wrong-kind import was accepted".into()),
        };
        assert_eq!(kind_error.code, "bad_request");
        Ok(())
    }

    #[test]
    fn export_of_real_registry_contains_no_secret_like_keys()
    -> Result<(), Box<dyn std::error::Error>> {
        // Secrets-exclusion by construction: the serialized envelope of a real
        // registry never carries any forbidden key.
        let tmp = tempfile::tempdir()?;
        let registry_dir = tmp.path().join("pipelines");
        seed_registry(&registry_dir)?;
        let export_path = tmp.path().join("pack.json");
        pipelines_export_to(&registry_dir, &export_path)?;
        let parsed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&export_path)?)?;
        assert!(find_forbidden_keys(&parsed).is_empty());
        Ok(())
    }
}
