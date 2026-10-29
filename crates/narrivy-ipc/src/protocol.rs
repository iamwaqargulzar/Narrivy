//! Wire protocol surface: handshake, bounded envelope, message kinds,
//! commands, response payloads and events.
//!
//! Salvage-adapted from `code/narrivy-v0.4/crates/narrivy-ipc/src/lib.rs`
//! (handshake negotiation, envelope bounds, request-id semantics) and
//! extended per `spec/protocols/ipc.md` for v1.3: first-class
//! `pipeline_id`/transaction identity, monotonic event sequencing, stable
//! wire tags with lossless unknown-value handling.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::{IpcError, IpcErrorCode};

/// Protocol major version. Major skew between client and daemon is a hard
/// reject (`IpcErrorCode::IncompatibleProtocol`).
pub const PROTOCOL_MAJOR: u16 = 1;

/// Protocol minor version. Minor skew is tolerated; the daemon advertises
/// its own minor in the handshake response.
pub const PROTOCOL_MINOR: u16 = 54;

/// First protocol minor that understands [`Event::TranscriptPartial`].
pub const TRANSCRIPT_PARTIAL_PROTOCOL_MINOR: u16 = 3;

/// First protocol minor that separates exact recovery-result reads from
/// optional user History.
pub const RECOVERY_ENTRY_PROTOCOL_MINOR: u16 = 4;

/// First protocol minor that understands daemon-owned offline model
/// provisioning commands and status.
pub const OFFLINE_MODEL_PROVISION_PROTOCOL_MINOR: u16 = 5;

/// First protocol minor whose recovery entry carries the typed, content-free
/// capture end reason. The field is additive and optional, so older peers keep
/// parsing the payload and simply do not render how capture ended.
pub const RECOVERY_ENTRY_CAPTURE_END_PROTOCOL_MINOR: u16 = 6;

/// First protocol minor that understands the additive atomic
/// `ToggleSession` request and its typed outcome payload. Older peers keep
/// using `StartSession`/`StopSession` unchanged; adding this request never
/// changes the meaning of the older commands.
pub const TOGGLE_SESSION_PROTOCOL_MINOR: u16 = 7;

/// Minor version that added the content-free last-session timing snapshot.
pub const SESSION_TIMINGS_PROTOCOL_MINOR: u16 = 8;

/// First protocol minor that carries bounded recognition settings on session
/// start/toggle commands. Clients must not send non-default settings to an
/// older daemon because older peers would ignore additive fields.
pub const RECOGNITION_SETTINGS_PROTOCOL_MINOR: u16 = 9;

/// First protocol minor that serves `DeleteHistoryEntry`. Clients must not
/// offer deletion against an older daemon, which would reject the command.
pub const DELETE_HISTORY_ENTRY_PROTOCOL_MINOR: u16 = 10;

/// First protocol minor that serves the desktop shortcut commands
/// (`ListShortcuts`, `SetShortcut`, `RemoveShortcut`).
pub const SHORTCUT_BINDINGS_PROTOCOL_MINOR: u16 = 11;

/// First protocol minor whose History listing rows carry the optional
/// transcription time.
pub const HISTORY_TIMING_PROTOCOL_MINOR: u16 = 12;

/// First protocol minor that serves `ListModels`.
pub const LIST_MODELS_PROTOCOL_MINOR: u16 = 13;

/// First protocol minor that serves `SelectModel`.
pub const SELECT_MODEL_PROTOCOL_MINOR: u16 = 14;

/// First protocol minor that serves `SetPipelineEngine` and whose pipeline
/// listing carries each pipeline's model and device.
pub const PIPELINE_ENGINE_PROTOCOL_MINOR: u16 = 15;

/// First protocol minor that serves `DownloadModel` and
/// `CancelModelDownload`.
pub const DOWNLOAD_MODEL_PROTOCOL_MINOR: u16 = 16;

/// First protocol minor that serves the Store (`ListStore`,
/// `RemoveStoreItem`) and installs runtime packs through `DownloadModel`.
pub const STORE_PROTOCOL_MINOR: u16 = 17;

/// First protocol minor whose terminal `committed` state event carries the
/// content-free delivery result (`inserted`, `pasted`, `typed`, `copied` or
/// `inbox`). The field is additive and optional.
pub const DELIVERY_RESULT_PROTOCOL_MINOR: u16 = 18;

/// First protocol minor that serves the AI assistant extension
/// (`AssistantSend`, `AssistantCancel`, and the `AssistantDelta` /
/// `AssistantDone` events).
pub const ASSISTANT_PROTOCOL_MINOR: u16 = 19;

/// First protocol minor that lists language models and selects the one used
/// for clean-up and for the assistant (`ListLanguageModels`,
/// `SelectLanguageModel`; D32).
pub const LANGUAGE_MODELS_PROTOCOL_MINOR: u16 = 20;

/// First protocol minor that sends the microphone level while listening
/// (`AudioLevel`), so the overlay's waves follow the voice.
pub const AUDIO_LEVEL_PROTOCOL_MINOR: u16 = 21;

/// First protocol minor that unloads the assistant's model on request
/// (`AssistantUnload`) and reports where it is loaded.
pub const ASSISTANT_UNLOAD_PROTOCOL_MINOR: u16 = 22;

/// First protocol minor that puts a History entry's text on the clipboard
/// (`CopyHistoryEntry`), so a missed paste can be pasted by hand.
pub const COPY_HISTORY_ENTRY_PROTOCOL_MINOR: u16 = 23;

/// First protocol minor that serves the browser writing assistant
/// (`CheckWriting`, `ListWritingRules`, `WritingAction`; D26).
pub const WRITING_PROTOCOL_MINOR: u16 = 24;

/// First protocol minor that records and transcribes meetings (F22):
/// `MeetingStart`, `MeetingStop`, `ListMeetings`, `GetMeeting`,
/// `ProcessMeeting`, `RenameMeetingSpeaker`, `DeleteMeeting` and the
/// `MeetingChanged` event.
pub const MEETINGS_PROTOCOL_MINOR: u16 = 25;

/// First protocol minor that writes meeting notes with the assistant model
/// (`SummarizeMeeting`) and reports saved notes and the full recording.
pub const MEETING_NOTES_PROTOCOL_MINOR: u16 = 26;

/// First protocol minor that reads and sets the audio options
/// (`GetAudioOptions`, `SetAudioOptions`): removing the computer's own sound
/// from the microphone.
pub const AUDIO_OPTIONS_PROTOCOL_MINOR: u16 = 27;

/// First protocol minor with the voice filter: training the user's voice
/// (`TrainVoice`, `ForgetVoice`), "Only my voice" in `SetAudioOptions`, and
/// the voice fields of `AudioOptions`.
pub const VOICE_PROTOCOL_MINOR: u16 = 28;

/// First protocol minor with noise suppression levels (`noise_suppression`
/// in `SetAudioOptions` and `AudioOptions`).
pub const NOISE_SUPPRESSION_PROTOCOL_MINOR: u16 = 29;

/// First protocol minor with the user's vocabulary (`GetVocabulary`,
/// `SetVocabulary`): words to recognise and replacements.
pub const VOCABULARY_PROTOCOL_MINOR: u16 = 30;

/// First protocol minor that lists the microphones and records the chosen
/// one (`microphone` in `SetAudioOptions` and `AudioOptions`).
pub const MICROPHONE_PROTOCOL_MINOR: u16 = 31;

/// First protocol minor with press and hold (`GetHoldKey`, `SetHoldKey`).
pub const HOLD_KEY_PROTOCOL_MINOR: u16 = 32;

/// First protocol minor with start and stop sounds (`sounds` in
/// `SetAudioOptions` and `AudioOptions`).
pub const SOUNDS_PROTOCOL_MINOR: u16 = 33;

/// First protocol minor that changes how long History is kept
/// (`SetHistoryRetention`) and clears it (`ClearHistory`).
pub const HISTORY_SETTINGS_PROTOCOL_MINOR: u16 = 34;

/// First protocol minor with text options (`GetTextOptions`,
/// `SetTextOptions`): numbers as digits on the Clean pipeline.
pub const TEXT_OPTIONS_PROTOCOL_MINOR: u16 = 35;

/// First protocol minor with per-pipeline Voice settings
/// (`SetPipelineVoice`, `AudioOptions.pipelines`).
pub const PIPELINE_VOICE_PROTOCOL_MINOR: u16 = 36;

/// First protocol minor that imports a recording as a meeting
/// (`ImportMeeting`).
pub const MEETING_IMPORT_PROTOCOL_MINOR: u16 = 37;

/// First protocol minor where the user chooses the delivery method
/// (`delivery_method` in `SetTextOptions` and `TextOptions`).
pub const DELIVERY_METHOD_PROTOCOL_MINOR: u16 = 38;

/// First protocol minor with meeting options (`GetMeetingOptions`,
/// `SetMeetingOptions`): notes written automatically.
pub const MEETING_OPTIONS_PROTOCOL_MINOR: u16 = 39;

/// First protocol minor that unloads the clean-up model when idle
/// (`cleanup_idle_minutes` in `SetTextOptions` and `TextOptions`).
pub const CLEANUP_IDLE_PROTOCOL_MINOR: u16 = 40;

/// First protocol minor with "Only my voice" strictness (`voice_strictness`
/// in `SetAudioOptions` and `AudioOptions`).
pub const VOICE_STRICTNESS_PROTOCOL_MINOR: u16 = 41;

/// First protocol minor with spoken line breaks (`spoken_breaks` in
/// `SetTextOptions` and `TextOptions`).
pub const SPOKEN_BREAKS_PROTOCOL_MINOR: u16 = 42;

/// First protocol minor where meetings have their own microphone and output
/// (`microphone`, `speaker`, `call_audio`, `room_audio` in `SetMeetingOptions`; the devices to choose
/// from and the ones in use in `MeetingOptions`).
pub const MEETING_DEVICES_PROTOCOL_MINOR: u16 = 43;

/// First protocol minor with the assistant extension's voice mode
/// (`VoiceMode`, the `Voice` event), its library of chats, memory, personas,
/// projects, skills and routines kept by the daemon (`AssistantLibrary`,
/// the `AssistantLibraryChanged` event), and questions sent into a kept
/// chat, spoken, or with pictures (`chat_id`, `speak`, `pictures` on
/// `AssistantSend`).
pub const ASSISTANT_LIBRARY_PROTOCOL_MINOR: u16 = 44;

/// First protocol minor where meetings can keep what is shown on screen
/// (`screen` in `SetMeetingOptions` and `MeetingOptions`, `slides` in
/// `MeetingDetail`, and `GetMeetingSlide`).
pub const MEETING_SCREEN_PROTOCOL_MINOR: u16 = 45;

/// First protocol minor where the assistant uses tools from MCP servers the
/// user added, each call approved first (`AssistantToolDecision`, the
/// `AssistantTool` event).
pub const ASSISTANT_TOOLS_PROTOCOL_MINOR: u16 = 46;

/// First protocol minor with "Watch with me" (`watch` on `MeetingStart` and
/// `Meeting`) and the assistant's built-in web and computer tools, whose
/// steps carry `detail` on the `AssistantTool` event.
pub const ASSISTANT_AGENT_PROTOCOL_MINOR: u16 = 47;

/// First protocol minor where meeting processing can be paused, resumed
/// and stopped (`MeetingControl`, `paused` on `MeetingDto`).
pub const MEETING_CONTROL_PROTOCOL_MINOR: u16 = 48;

/// First protocol minor with live screen sharing and screen recordings
/// (`share`, `microphone` and `computer_sound` on `MeetingStart`, `share`
/// on `Meeting`).
pub const SCREEN_SESSIONS_PROTOCOL_MINOR: u16 = 49;

/// First protocol minor where the screen and the sound of a meeting being
/// recorded can be switched on and off (`MeetingSources`, `screen_on` and
/// `sound_on` on `Meeting`).
pub const MEETING_SOURCES_PROTOCOL_MINOR: u16 = 50;

/// First protocol minor where the microphone and the computer's sound of a
/// meeting being recorded are switched apart (`microphone` on
/// `MeetingSources`, `mic_on` on `Meeting`; `sound` is then the computer's
/// sound only).
pub const MEETING_MICROPHONE_PROTOCOL_MINOR: u16 = 51;

/// First protocol minor where a stopped recording can be kept as it was
/// heard, gone over only when asked (`process_on_stop` in
/// `SetMeetingOptions` and `MeetingOptions`, `raw` on `Meeting`).
pub const MEETING_QUICK_SAVE_PROTOCOL_MINOR: u16 = 52;

/// First protocol minor where a meeting being recorded also carries the
/// words being said now, not yet final (a last turn with `partial`).
pub const MEETING_LIVE_PARTIAL_PROTOCOL_MINOR: u16 = 53;

/// First protocol minor with the dictation language (`dictation_language`
/// and `dictation_languages` in `SetTextOptions` and `TextOptions`).
pub const DICTATION_LANGUAGE_PROTOCOL_MINOR: u16 = 54;

/// Text one writing check or action carries at most, in bytes.
pub const MAX_WRITING_TEXT_BYTES: usize = 64 * 1024;
/// A custom writing instruction carries at most this many bytes.
pub const MAX_WRITING_INSTRUCTION_BYTES: usize = 1024;
/// A personal dictionary sent with a check holds at most this many words.
pub const MAX_WRITING_WORDS: usize = 5000;
/// A check carries at most this many rule switches.
pub const MAX_WRITING_RULES: usize = 1000;

/// An assistant conversation carries at most this many messages.
pub const MAX_ASSISTANT_MESSAGES: usize = 64;
/// One assistant message carries at most this many bytes.
pub const MAX_ASSISTANT_MESSAGE_BYTES: usize = 32 * 1024;
/// All messages of one request carry at most this many bytes together.
pub const MAX_ASSISTANT_REQUEST_BYTES: usize = 128 * 1024;

/// `TranscriptPartial` events numbered from here on carry the text being
/// cleaned up while the session is `transforming` (first as recognised,
/// then as cleaned), not recognition partials. A late recognition partial
/// can therefore never be shown as the cleaned text.
pub const REFINE_PREVIEW_SEQ_BASE: u64 = 1 << 40;

/// A model id carries at most this many bytes.
pub const MAX_MODEL_ID_BYTES: usize = 128;

/// A shortcut combination carries at most this many modifiers.
pub const MAX_SHORTCUT_MODIFIERS: usize = 4;
/// Upper bound on a shortcut's key name and on each modifier name.
pub const MAX_SHORTCUT_KEY_BYTES: usize = 16;

/// Build identity advertised by this build in handshakes (crate version).
pub const BUILD_IDENTITY: &str = env!("CARGO_PKG_VERSION");

/// Hard upper bound for one serialized IPC message in bytes (1 MiB).
pub const MAX_MESSAGE_BYTES: usize = 1_048_576;

/// Hard upper bound for any JSON collection (array/object) inside a payload.
pub const MAX_COLLECTION_ELEMENTS: usize = 10_000;

/// Maximum UTF-8 byte length for a caller-generated portal permission
/// operation identifier.
pub const MAX_PORTAL_OPERATION_ID_BYTES: usize = 64;

/// Maximum UTF-8 byte length for the opaque portal parent-window identifier.
/// This matches the platform adapter's defensive bound without interpreting
/// platform-specific window-system syntax in the portable wire crate.
pub const MAX_PORTAL_PARENT_WINDOW_BYTES: usize = 4_096;

/// Maximum UTF-8 byte length for a caller-selected local model source path.
///
/// This is a wire-shape bound only. The daemon remains responsible for
/// resolving and securely opening the path under its provisioning policy.
pub const MAX_MODEL_PROVISION_SOURCE_PATH_BYTES: usize = 4_096;

/// Maximum byte length for an opaque model-provisioning operation identifier.
pub const MAX_MODEL_PROVISION_OPERATION_ID_BYTES: usize = 64;

/// Maximum UTF-8 size of one volatile transcript hypothesis. This is far
/// below the frame bound so a streaming model cannot monopolize IPC memory.
pub const MAX_PARTIAL_TRANSCRIPT_BYTES: usize = 16_384;

/// Maximum UTF-8 byte length for a recognition language, script, or locale tag.
pub const MAX_RECOGNITION_TAG_BYTES: usize = 32;

/// Maximum context/bias hints carried by one recognition request.
pub const MAX_RECOGNITION_CONTEXT_HINTS: usize = 8;

/// Maximum UTF-8 byte length for one recognition context/bias hint.
pub const MAX_RECOGNITION_CONTEXT_HINT_BYTES: usize = 128;

/// Maximum nesting depth accepted inside a payload (stack-safety bound).
const MAX_JSON_DEPTH: usize = 64;

/// Serde helpers encoding `u128` identifiers as decimal STRINGS on the wire.
///
/// Rationale: serde_json cannot represent `u128` numbers at all without the
/// workspace-global `arbitrary_precision` feature (unacceptable here), and
/// raw large integers lose precision in JavaScript clients anyway. Decimal
/// strings are exact, stable and forward-compatible.
pub mod wire_u128 {
    use serde::{Deserialize, Deserializer, Serializer};

    /// Serialize one id as a decimal string.
    pub fn serialize<S: Serializer>(value: &u128, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&value.to_string())
    }

    /// Deserialize one id from a decimal string.
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u128, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse::<u128>()
            .map_err(|_| serde::de::Error::custom(format!("invalid u128 id: {text:?}")))
    }

    /// Serialize an optional id (`null` stays `null`).
    pub mod option {
        use serde::{Deserialize, Deserializer, Serializer};

        pub fn serialize<S: Serializer>(
            value: &Option<u128>,
            serializer: S,
        ) -> Result<S::Ok, S::Error> {
            match value {
                // Call the parent-module helper explicitly to avoid shadowing.
                Some(id) => super::serialize(id, serializer),
                None => serializer.serialize_none(),
            }
        }

        pub fn deserialize<'de, D: Deserializer<'de>>(
            deserializer: D,
        ) -> Result<Option<u128>, D::Error> {
            let text = Option::<String>::deserialize(deserializer)?;
            match text {
                None => Ok(None),
                Some(text) => text
                    .parse::<u128>()
                    .map(Some)
                    .map_err(|_| serde::de::Error::custom(format!("invalid u128 id: {text:?}"))),
            }
        }
    }
}

/// Correlation identity for one request/response pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RequestId(pub u64);

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Client hello advertising the supported protocol range, build identity
/// and client role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandshakeHello {
    pub protocol_major: u16,
    pub protocol_minor: u16,
    pub build_identity: String,
    pub client_role: String,
}

impl HandshakeHello {
    /// Hello for this build advertising [`PROTOCOL_MAJOR`] /
    /// [`PROTOCOL_MINOR`] with the given client role.
    pub fn new(client_role: impl Into<String>) -> Self {
        Self {
            protocol_major: PROTOCOL_MAJOR,
            protocol_minor: PROTOCOL_MINOR,
            build_identity: BUILD_IDENTITY.to_string(),
            client_role: client_role.into(),
        }
    }

    /// Bounded-shape validation applied before a hello goes on the wire.
    pub fn validate(&self) -> Result<(), IpcError> {
        if self.client_role.is_empty() || self.client_role.len() > 64 {
            return Err(IpcError::bad_request("client_role must be 1..=64 bytes"));
        }
        if self.build_identity.is_empty() || self.build_identity.len() > 256 {
            return Err(IpcError::bad_request(
                "build_identity must be 1..=256 bytes",
            ));
        }
        Ok(())
    }
}

/// Daemon reply to a handshake.
///
/// `Accepted.protocol_minor` is the daemon's own minor version; clients use
/// it to gate minor features explicitly (spec: "minor feature negotiation
/// is explicit").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HandshakeResponse {
    Accepted {
        protocol_minor: u16,
        daemon_build: String,
    },
    Rejected {
        code: IpcErrorCode,
        detail: String,
    },
}

/// Negotiate a client hello against this daemon build's protocol constants.
///
/// Major skew rejects with [`IpcErrorCode::IncompatibleProtocol`]; any
/// minor within the same major is accepted and the daemon's minor version
/// is returned so feature gating stays explicit.
pub fn negotiate_handshake(hello: &HandshakeHello) -> HandshakeResponse {
    if hello.protocol_major != PROTOCOL_MAJOR {
        return HandshakeResponse::Rejected {
            code: IpcErrorCode::IncompatibleProtocol,
            detail: format!(
                "protocol major skew: client {}, daemon {PROTOCOL_MAJOR}; update the mismatched side",
                hello.protocol_major
            ),
        };
    }
    HandshakeResponse::Accepted {
        protocol_minor: PROTOCOL_MINOR,
        daemon_build: BUILD_IDENTITY.to_string(),
    }
}

/// Message kind carried by every envelope.
///
/// Known kinds serialize as their fixed wire string (e.g.
/// `"request.get_status"`, `"response"`, `"event"`). Unknown strings from
/// newer peers deserialize into [`MessageKind::Other`] and serialize back
/// verbatim — forward-compatible passthrough instead of a decode failure.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MessageKind {
    GetStatus,
    StartSession,
    StopSession,
    /// Additive atomic activation request (protocol minor 7): start exactly
    /// one session or request graceful finalization of the one active session.
    ToggleSession,
    CancelSession,
    Subscribe,
    Unsubscribe,
    ListPipelines,
    SelectPipeline,
    GetHistory,
    GetHistoryEntry,
    DeleteHistoryEntry,
    ListShortcuts,
    SetShortcut,
    RemoveShortcut,
    ListModels,
    SelectModel,
    SetPipelineEngine,
    DownloadModel,
    CancelModelDownload,
    ListStore,
    RemoveStoreItem,
    AssistantSend,
    AssistantCancel,
    ListLanguageModels,
    SelectLanguageModel,
    AssistantUnload,
    CopyHistoryEntry,
    CheckWriting,
    ListWritingRules,
    WritingAction,
    MeetingStart,
    MeetingStop,
    ListMeetings,
    GetMeeting,
    ProcessMeeting,
    RenameMeetingSpeaker,
    DeleteMeeting,
    SummarizeMeeting,
    GetAudioOptions,
    SetAudioOptions,
    TrainVoice,
    ForgetVoice,
    GetVocabulary,
    SetVocabulary,
    GetHoldKey,
    SetHoldKey,
    SetHistoryRetention,
    ClearHistory,
    GetTextOptions,
    SetTextOptions,
    SetPipelineVoice,
    ImportMeeting,
    GetMeetingOptions,
    SetMeetingOptions,
    GetMeetingSlide,
    MeetingControl,
    MeetingSources,
    AssistantToolDecision,
    AssistantLibrary,
    VoiceMode,
    GetRecoveryEntry,
    RunCapabilityCheck,
    RequestPortalKeyboardPermission,
    CancelPortalKeyboardPermission,
    RevokePortalKeyboardPermission,
    StartOfflineModelProvision,
    CancelOfflineModelProvision,
    Response,
    Event,
    /// Unrecognized kind preserved verbatim (forward compatibility).
    Other(String),
}

impl MessageKind {
    fn known_wire_tag(&self) -> Option<&'static str> {
        match self {
            Self::GetStatus => Some("request.get_status"),
            Self::StartSession => Some("request.start_session"),
            Self::StopSession => Some("request.stop_session"),
            Self::ToggleSession => Some("request.toggle_session"),
            Self::CancelSession => Some("request.cancel_session"),
            Self::Subscribe => Some("request.subscribe"),
            Self::Unsubscribe => Some("request.unsubscribe"),
            Self::ListPipelines => Some("request.list_pipelines"),
            Self::SelectPipeline => Some("request.select_pipeline"),
            Self::GetHistory => Some("request.get_history"),
            Self::GetHistoryEntry => Some("request.get_history_entry"),
            Self::DeleteHistoryEntry => Some("request.delete_history_entry"),
            Self::ListShortcuts => Some("request.list_shortcuts"),
            Self::SetShortcut => Some("request.set_shortcut"),
            Self::RemoveShortcut => Some("request.remove_shortcut"),
            Self::ListModels => Some("request.list_models"),
            Self::SelectModel => Some("request.select_model"),
            Self::SetPipelineEngine => Some("request.set_pipeline_engine"),
            Self::DownloadModel => Some("request.download_model"),
            Self::CancelModelDownload => Some("request.cancel_model_download"),
            Self::ListStore => Some("request.list_store"),
            Self::RemoveStoreItem => Some("request.remove_store_item"),
            Self::AssistantSend => Some("request.assistant_send"),
            Self::AssistantCancel => Some("request.assistant_cancel"),
            Self::ListLanguageModels => Some("request.list_language_models"),
            Self::SelectLanguageModel => Some("request.select_language_model"),
            Self::AssistantUnload => Some("request.assistant_unload"),
            Self::CopyHistoryEntry => Some("request.copy_history_entry"),
            Self::CheckWriting => Some("request.check_writing"),
            Self::ListWritingRules => Some("request.list_writing_rules"),
            Self::WritingAction => Some("request.writing_action"),
            Self::MeetingStart => Some("request.meeting_start"),
            Self::MeetingStop => Some("request.meeting_stop"),
            Self::ListMeetings => Some("request.list_meetings"),
            Self::GetMeeting => Some("request.get_meeting"),
            Self::ProcessMeeting => Some("request.process_meeting"),
            Self::RenameMeetingSpeaker => Some("request.rename_meeting_speaker"),
            Self::DeleteMeeting => Some("request.delete_meeting"),
            Self::SummarizeMeeting => Some("request.summarize_meeting"),
            Self::GetAudioOptions => Some("request.get_audio_options"),
            Self::SetAudioOptions => Some("request.set_audio_options"),
            Self::TrainVoice => Some("request.train_voice"),
            Self::ForgetVoice => Some("request.forget_voice"),
            Self::GetVocabulary => Some("request.get_vocabulary"),
            Self::SetVocabulary => Some("request.set_vocabulary"),
            Self::GetHoldKey => Some("request.get_hold_key"),
            Self::SetHoldKey => Some("request.set_hold_key"),
            Self::SetHistoryRetention => Some("request.set_history_retention"),
            Self::ClearHistory => Some("request.clear_history"),
            Self::GetTextOptions => Some("request.get_text_options"),
            Self::SetTextOptions => Some("request.set_text_options"),
            Self::SetPipelineVoice => Some("request.set_pipeline_voice"),
            Self::ImportMeeting => Some("request.import_meeting"),
            Self::GetMeetingOptions => Some("request.get_meeting_options"),
            Self::SetMeetingOptions => Some("request.set_meeting_options"),
            Self::GetMeetingSlide => Some("request.get_meeting_slide"),
            Self::MeetingControl => Some("request.meeting_control"),
            Self::MeetingSources => Some("request.meeting_sources"),
            Self::AssistantToolDecision => Some("request.assistant_tool_decision"),
            Self::AssistantLibrary => Some("request.assistant_library"),
            Self::VoiceMode => Some("request.voice_mode"),
            Self::GetRecoveryEntry => Some("request.get_recovery_entry"),
            Self::RunCapabilityCheck => Some("request.run_capability_check"),
            Self::RequestPortalKeyboardPermission => {
                Some("request.request_portal_keyboard_permission")
            }
            Self::CancelPortalKeyboardPermission => {
                Some("request.cancel_portal_keyboard_permission")
            }
            Self::RevokePortalKeyboardPermission => {
                Some("request.revoke_portal_keyboard_permission")
            }
            Self::StartOfflineModelProvision => Some("request.start_offline_model_provision"),
            Self::CancelOfflineModelProvision => Some("request.cancel_offline_model_provision"),
            Self::Response => Some("response"),
            Self::Event => Some("event"),
            Self::Other(_) => None,
        }
    }

    /// Stable wire tag for this kind (`Other` yields its original string).
    pub fn wire_tag(&self) -> std::borrow::Cow<'static, str> {
        match self.known_wire_tag() {
            Some(tag) => std::borrow::Cow::Borrowed(tag),
            None => match self {
                Self::Other(raw) => std::borrow::Cow::Owned(raw.clone()),
                // Unreachable: every non-Other variant has a known tag.
                _ => std::borrow::Cow::Borrowed(""),
            },
        }
    }

    /// Decode a wire tag; unknown strings become [`MessageKind::Other`].
    pub fn from_wire_tag(tag: &str) -> Self {
        match tag {
            "request.get_status" => Self::GetStatus,
            "request.start_session" => Self::StartSession,
            "request.stop_session" => Self::StopSession,
            "request.toggle_session" => Self::ToggleSession,
            "request.cancel_session" => Self::CancelSession,
            "request.subscribe" => Self::Subscribe,
            "request.unsubscribe" => Self::Unsubscribe,
            "request.list_pipelines" => Self::ListPipelines,
            "request.select_pipeline" => Self::SelectPipeline,
            "request.get_history" => Self::GetHistory,
            "request.get_history_entry" => Self::GetHistoryEntry,
            "request.delete_history_entry" => Self::DeleteHistoryEntry,
            "request.list_shortcuts" => Self::ListShortcuts,
            "request.set_shortcut" => Self::SetShortcut,
            "request.remove_shortcut" => Self::RemoveShortcut,
            "request.list_models" => Self::ListModels,
            "request.select_model" => Self::SelectModel,
            "request.set_pipeline_engine" => Self::SetPipelineEngine,
            "request.download_model" => Self::DownloadModel,
            "request.cancel_model_download" => Self::CancelModelDownload,
            "request.list_store" => Self::ListStore,
            "request.remove_store_item" => Self::RemoveStoreItem,
            "request.assistant_send" => Self::AssistantSend,
            "request.assistant_cancel" => Self::AssistantCancel,
            "request.list_language_models" => Self::ListLanguageModels,
            "request.select_language_model" => Self::SelectLanguageModel,
            "request.assistant_unload" => Self::AssistantUnload,
            "request.copy_history_entry" => Self::CopyHistoryEntry,
            "request.check_writing" => Self::CheckWriting,
            "request.list_writing_rules" => Self::ListWritingRules,
            "request.writing_action" => Self::WritingAction,
            "request.meeting_start" => Self::MeetingStart,
            "request.meeting_stop" => Self::MeetingStop,
            "request.list_meetings" => Self::ListMeetings,
            "request.get_meeting" => Self::GetMeeting,
            "request.process_meeting" => Self::ProcessMeeting,
            "request.rename_meeting_speaker" => Self::RenameMeetingSpeaker,
            "request.delete_meeting" => Self::DeleteMeeting,
            "request.summarize_meeting" => Self::SummarizeMeeting,
            "request.get_audio_options" => Self::GetAudioOptions,
            "request.set_audio_options" => Self::SetAudioOptions,
            "request.train_voice" => Self::TrainVoice,
            "request.forget_voice" => Self::ForgetVoice,
            "request.get_vocabulary" => Self::GetVocabulary,
            "request.set_vocabulary" => Self::SetVocabulary,
            "request.get_hold_key" => Self::GetHoldKey,
            "request.set_hold_key" => Self::SetHoldKey,
            "request.set_history_retention" => Self::SetHistoryRetention,
            "request.clear_history" => Self::ClearHistory,
            "request.get_text_options" => Self::GetTextOptions,
            "request.set_text_options" => Self::SetTextOptions,
            "request.set_pipeline_voice" => Self::SetPipelineVoice,
            "request.import_meeting" => Self::ImportMeeting,
            "request.get_meeting_options" => Self::GetMeetingOptions,
            "request.set_meeting_options" => Self::SetMeetingOptions,
            "request.get_meeting_slide" => Self::GetMeetingSlide,
            "request.meeting_control" => Self::MeetingControl,
            "request.meeting_sources" => Self::MeetingSources,
            "request.assistant_tool_decision" => Self::AssistantToolDecision,
            "request.assistant_library" => Self::AssistantLibrary,
            "request.voice_mode" => Self::VoiceMode,
            "request.get_recovery_entry" => Self::GetRecoveryEntry,
            "request.run_capability_check" => Self::RunCapabilityCheck,
            "request.request_portal_keyboard_permission" => Self::RequestPortalKeyboardPermission,
            "request.cancel_portal_keyboard_permission" => Self::CancelPortalKeyboardPermission,
            "request.revoke_portal_keyboard_permission" => Self::RevokePortalKeyboardPermission,
            "request.start_offline_model_provision" => Self::StartOfflineModelProvision,
            "request.cancel_offline_model_provision" => Self::CancelOfflineModelProvision,
            "response" => Self::Response,
            "event" => Self::Event,
            _ => Self::Other(tag.to_owned()),
        }
    }

    /// True for the request kinds this daemon serves.
    pub fn is_request(&self) -> bool {
        matches!(
            self.known_wire_tag(),
            Some("request.get_status")
                | Some("request.start_session")
                | Some("request.stop_session")
                | Some("request.toggle_session")
                | Some("request.cancel_session")
                | Some("request.subscribe")
                | Some("request.unsubscribe")
                | Some("request.list_pipelines")
                | Some("request.select_pipeline")
                | Some("request.get_history")
                | Some("request.get_history_entry")
                | Some("request.delete_history_entry")
                | Some("request.list_shortcuts")
                | Some("request.set_shortcut")
                | Some("request.remove_shortcut")
                | Some("request.list_models")
                | Some("request.select_model")
                | Some("request.set_pipeline_engine")
                | Some("request.download_model")
                | Some("request.cancel_model_download")
                | Some("request.list_store")
                | Some("request.remove_store_item")
                | Some("request.assistant_send")
                | Some("request.assistant_cancel")
                | Some("request.list_language_models")
                | Some("request.select_language_model")
                | Some("request.assistant_unload")
                | Some("request.copy_history_entry")
                | Some("request.check_writing")
                | Some("request.list_writing_rules")
                | Some("request.writing_action")
                | Some("request.meeting_start")
                | Some("request.meeting_stop")
                | Some("request.list_meetings")
                | Some("request.get_meeting")
                | Some("request.process_meeting")
                | Some("request.rename_meeting_speaker")
                | Some("request.delete_meeting")
                | Some("request.summarize_meeting")
                | Some("request.get_audio_options")
                | Some("request.set_audio_options")
                | Some("request.train_voice")
                | Some("request.forget_voice")
                | Some("request.get_vocabulary")
                | Some("request.set_vocabulary")
                | Some("request.get_hold_key")
                | Some("request.set_hold_key")
                | Some("request.set_history_retention")
                | Some("request.clear_history")
                | Some("request.get_text_options")
                | Some("request.set_text_options")
                | Some("request.set_pipeline_voice")
                | Some("request.import_meeting")
                | Some("request.get_meeting_options")
                | Some("request.set_meeting_options")
                | Some("request.get_meeting_slide")
                | Some("request.meeting_control")
                | Some("request.meeting_sources")
                | Some("request.assistant_tool_decision")
                | Some("request.assistant_library")
                | Some("request.voice_mode")
                | Some("request.get_recovery_entry")
                | Some("request.run_capability_check")
                | Some("request.request_portal_keyboard_permission")
                | Some("request.cancel_portal_keyboard_permission")
                | Some("request.revoke_portal_keyboard_permission")
                | Some("request.start_offline_model_provision")
                | Some("request.cancel_offline_model_provision")
        )
    }
}

impl Serialize for MessageKind {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.wire_tag())
    }
}

impl<'de> Deserialize<'de> for MessageKind {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let tag = String::deserialize(deserializer)?;
        Ok(Self::from_wire_tag(&tag))
    }
}

/// Commands served by narrivyd. Serialized as an internally tagged enum:
/// `{"command": "start_session", "pipeline_id": null, "activation": "hotkey"}`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    GetStatus,
    StartSession {
        #[serde(default)]
        pipeline_id: Option<String>,
        activation: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        recognition: Option<RecognitionSettingsDto>,
    },
    StopSession,
    /// One atomic activation: start exactly one admitted Raw/Clean session
    /// when none exists, otherwise request graceful finalization of the active
    /// session. Deliberately carries no arbitrary policy string; the daemon
    /// decides atomically and reports a typed outcome.
    ToggleSession {
        #[serde(default)]
        pipeline_id: Option<String>,
        #[serde(default)]
        activation: ActivationSourceDto,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        recognition: Option<RecognitionSettingsDto>,
    },
    CancelSession,
    Subscribe,
    Unsubscribe,
    ListPipelines,
    SelectPipeline {
        pipeline_id: String,
    },
    /// Optional user History listing. A daemon without a composed History
    /// store rejects this command; it must never list the recovery journal.
    GetHistory,
    /// Full optional-History record on explicit request. A daemon without a
    /// composed History store rejects this command.
    GetHistoryEntry {
        txid: String,
    },
    /// Delete one optional-History record on explicit user request (protocol
    /// minor 10). Never touches the recovery journal. A daemon without a
    /// composed History store rejects this command.
    DeleteHistoryEntry {
        txid: String,
    },
    /// Keyboard shortcuts Narrivy manages in the desktop's own shortcut
    /// settings, and whether they can be managed at all (protocol minor 11).
    ListShortcuts,
    /// Bind a key combination to a pipeline (protocol minor 11). With
    /// `replace` false an existing use of the keys is reported, not changed.
    SetShortcut {
        binding: ShortcutBindingDto,
        #[serde(default)]
        replace: bool,
    },
    /// Remove one shortcut Narrivy created (protocol minor 11). Never removes
    /// a binding that belongs to something else.
    RemoveShortcut {
        modifiers: Vec<String>,
        key: String,
    },
    /// The speech models this daemon knows about and the state of each on
    /// this computer (protocol minor 13).
    ListModels,
    /// Load an installed model in place of the one in use (protocol minor
    /// 14). `Accepted` means loading has started; `ListModels` reports the
    /// model as `loading` and then `active`, and status reports
    /// `model.selection`.
    SelectModel {
        id: String,
    },
    /// Set the model and device one pipeline recognises with (protocol minor
    /// 15). `model_id` absent means "whatever model is loaded"; `compute` is
    /// `auto`, `gpu` or `cpu`. The choice is stored, and loaded at once when
    /// no dictation is running; otherwise it is loaded when that pipeline
    /// next starts.
    SetPipelineEngine {
        pipeline_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model_id: Option<String>,
        compute: String,
    },
    /// Download a model the daemon's catalog publishes (protocol minor 16).
    /// This is the daemon's only network use and happens only on this
    /// request. `Accepted` means the download has started; `ListModels`
    /// reports it as `downloading` with the bytes so far, and status
    /// reports `model.download`.
    DownloadModel {
        id: String,
    },
    /// Stop the running download of `id` (protocol minor 16). The partial
    /// file is kept, so a later `DownloadModel` resumes.
    CancelModelDownload {
        id: String,
    },
    /// What the Store offers and whether each item fits this computer
    /// (protocol minor 17). Items are installed with `DownloadModel`, which
    /// from this minor also accepts a runtime pack's id.
    ListStore,
    /// Remove an installed Store item (protocol minor 17). Refused while the
    /// item is in use.
    RemoveStoreItem {
        id: String,
    },
    /// Ask the AI assistant (protocol minor 19). Accepted at once; the reply
    /// streams as `AssistantDelta` events and ends with one `AssistantDone`,
    /// all carrying `turn_id`. The conversation is sent whole each time; the
    /// daemon keeps none of it.
    AssistantSend {
        #[serde(deserialize_with = "deserialize_turn_id")]
        turn_id: String,
        #[serde(deserialize_with = "deserialize_assistant_messages")]
        messages: Vec<AssistantMessageDto>,
        /// A chat the daemon keeps (minor 44): `messages` is then only the
        /// new question; the daemon adds the chat's history, persona,
        /// project, memory and skills, and saves the reply into the chat.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        chat_id: Option<String>,
        /// Spoken in voice mode (minor 44): a short, speakable reply, read
        /// aloud as it is written.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        speak: bool,
        /// Pictures for the question (minor 44): ids from
        /// `AssistantLibrary` `put` `picture`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pictures: Vec<String>,
    },
    /// Stop the reply to `turn_id` (protocol minor 19).
    AssistantCancel {
        #[serde(deserialize_with = "deserialize_turn_id")]
        turn_id: String,
    },
    /// The language models on this computer and what each is used for
    /// (protocol minor 20).
    ListLanguageModels,
    /// Use model `id` for `role` (`clean_up` or `assistant`), or nothing
    /// with no `id` (protocol minor 20).
    SelectLanguageModel {
        role: String,
        #[serde(default)]
        id: Option<String>,
    },
    /// Free the assistant model's memory now (protocol minor 22); the next
    /// question loads it again.
    AssistantUnload,
    /// The assistant's library (protocol minor 44). `op` is `list`, `get`,
    /// `put`, `delete`, `import` or `run`; `kind` is `chat`, `memory`,
    /// `persona`, `project`, `skill`, `routine` or `picture`; `value` is
    /// JSON (a picture: base64 PNG). Replies with `AssistantLibrary`.
    AssistantLibrary {
        op: String,
        kind: String,
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        value: Option<String>,
    },
    /// The assistant's voice mode (protocol minor 44): `action` is `start`,
    /// `stop`, `settings` (with `settings`) or `ptt` (push-to-talk key, with
    /// `down`). What it hears and does arrives as `Voice` events.
    VoiceMode {
        action: String,
        #[serde(default)]
        settings: Option<VoiceSettingsDto>,
        #[serde(default)]
        down: Option<bool>,
    },
    /// Put one History entry's text on the clipboard (protocol minor 23).
    /// The text stays inside the daemon; only the id crosses the socket.
    CopyHistoryEntry {
        txid: String,
    },
    /// Check text with the local rule checker (protocol minor 24). The text
    /// is not kept or logged.
    CheckWriting {
        #[serde(deserialize_with = "deserialize_writing_text")]
        text: String,
        /// `american` (default), `british`, `canadian`, `australian`, `indian`.
        #[serde(default)]
        dialect: Option<String>,
        /// Rule name to on/off, over the defaults.
        #[serde(default, deserialize_with = "deserialize_writing_rules")]
        rules: BTreeMap<String, bool>,
        /// Personal dictionary: never reported as misspelled.
        #[serde(default, deserialize_with = "deserialize_writing_words")]
        words: Vec<String>,
    },
    /// The rule checker's rules (protocol minor 24).
    ListWritingRules,
    /// Rewrite text with the assistant model (protocol minor 24): `action`
    /// is `fix`, `improve`, `rewrite`, `shorter`, `longer`, `formal`,
    /// `friendly`, `confident`, `bullets`, `summarise` or `custom` (with
    /// `instruction`). Accepted at once; the reply streams as
    /// `AssistantDelta` events and ends with `AssistantDone`, all carrying
    /// `turn_id`.
    WritingAction {
        #[serde(deserialize_with = "deserialize_turn_id")]
        turn_id: String,
        action: String,
        #[serde(deserialize_with = "deserialize_writing_text")]
        text: String,
        #[serde(default, deserialize_with = "deserialize_writing_instruction")]
        instruction: Option<String>,
    },
    /// Start recording a meeting (protocol minor 25): the microphone and the
    /// computer's sound, kept apart. Answers with the new meeting.
    MeetingStart {
        #[serde(default, deserialize_with = "deserialize_meeting_text")]
        title: Option<String>,
        /// "Watch with me" (minor 47): keep the screen whatever the
        /// meeting options say, and keep moving pictures more often.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        watch: bool,
        /// With `watch`, a live screen share (minor 49): the chat sharing it
        /// sees it while it lasts; once it stops it is kept as a session.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        share: bool,
        /// Record the microphone for this one (minor 49); unset follows the
        /// meeting options.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        microphone: Option<bool>,
        /// Record the computer's sound for this one (minor 49).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        computer_sound: Option<bool>,
    },
    /// Stop the meeting being recorded; transcribing starts at once
    /// (protocol minor 25).
    MeetingStop,
    /// Every meeting, newest first (protocol minor 25).
    ListMeetings,
    /// One meeting with its transcript (protocol minor 25).
    GetMeeting {
        #[serde(deserialize_with = "deserialize_meeting_id")]
        id: String,
    },
    /// Transcribe a meeting again (protocol minor 25): `model` is `turbo`
    /// (fast) or `large-v3` (more accurate, better Urdu); a speaker count
    /// fixes how many voices each source is split into.
    ProcessMeeting {
        #[serde(deserialize_with = "deserialize_meeting_id")]
        id: String,
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        room_speakers: Option<u16>,
        #[serde(default)]
        call_speakers: Option<u16>,
    },
    /// Name a speaker label such as `Room 1` (protocol minor 25); an empty
    /// name restores the label.
    RenameMeetingSpeaker {
        #[serde(deserialize_with = "deserialize_meeting_id")]
        id: String,
        speaker: String,
        #[serde(deserialize_with = "deserialize_meeting_name")]
        name: String,
    },
    /// Delete a meeting's recording and transcript (protocol minor 25).
    DeleteMeeting {
        #[serde(deserialize_with = "deserialize_meeting_id")]
        id: String,
    },
    /// Write notes for a meeting with the assistant model (protocol minor
    /// 26): `kind` is `notes` (summary, decisions, action items), `email`
    /// (a follow-up email) or `custom` (with `instruction`). Accepted at once;
    /// the result streams as `AssistantDelta` events and ends with
    /// `AssistantDone`, all carrying `turn_id`, and is saved with the meeting.
    SummarizeMeeting {
        #[serde(deserialize_with = "deserialize_meeting_id")]
        id: String,
        #[serde(deserialize_with = "deserialize_turn_id")]
        turn_id: String,
        kind: String,
        #[serde(default, deserialize_with = "deserialize_writing_instruction")]
        instruction: Option<String>,
    },
    /// The audio options (protocol minor 27).
    GetAudioOptions,
    /// Change the audio options (protocol minor 27; `only_my_voice` minor
    /// 28). A field left out stays as it is. Replies with the options.
    SetAudioOptions {
        #[serde(default)]
        echo_cancel: Option<bool>,
        #[serde(default)]
        only_my_voice: Option<bool>,
        /// `off`, `standard` or `strong` (minor 29).
        #[serde(default)]
        noise_suppression: Option<String>,
        /// A microphone's name from `AudioOptions.microphones`, or `""` for
        /// the system default (minor 31).
        #[serde(default)]
        microphone: Option<String>,
        /// Start and stop sounds on or off (minor 33).
        #[serde(default)]
        sounds: Option<bool>,
        /// `relaxed`, `normal` or `strict` (minor 41).
        #[serde(default)]
        voice_strictness: Option<String>,
    },
    /// Record the user reading for about 20 seconds and save their voice
    /// profile (protocol minor 28). Accepted at once; `AudioOptions`
    /// reports `voice_training` until it ends.
    TrainVoice,
    /// Forget the trained voice (protocol minor 28).
    ForgetVoice,
    /// The user's vocabulary (protocol minor 30).
    GetVocabulary,
    /// Replace the user's vocabulary (protocol minor 30); replies with it
    /// as saved (tidied).
    SetVocabulary {
        vocabulary: VocabularyDto,
    },
    /// Press and hold (protocol minor 32).
    GetHoldKey,
    /// Choose the key to press and hold, or `None` to turn it off (protocol
    /// minor 32). Replies `HoldKey`.
    SetHoldKey {
        #[serde(default)]
        key: Option<u16>,
    },
    /// How long History is kept: `off`, `7d`, `30d` or `forever` (protocol
    /// minor 34). Applies at once and is kept for the next start.
    SetHistoryRetention {
        retention: String,
    },
    /// Remove every History entry (protocol minor 34).
    ClearHistory,
    /// Text options (protocol minor 35).
    GetTextOptions,
    /// Change text options; a field left out stays (protocol minor 35).
    SetTextOptions {
        #[serde(default)]
        numbers_as_digits: Option<bool>,
        /// `paste` or `type` (minor 38).
        #[serde(default)]
        delivery_method: Option<String>,
        /// Unload the clean-up model after this many idle minutes; 0 never
        /// (minor 40).
        #[serde(default)]
        cleanup_idle_minutes: Option<u32>,
        /// "new line" / "new paragraph" as line breaks on Clean (minor 42).
        #[serde(default)]
        spoken_breaks: Option<bool>,
        /// The language a start without its own is dictated in: a tag, or
        /// `auto` (minor 54).
        #[serde(default)]
        dictation_language: Option<String>,
        /// The user's languages to choose from (minor 54).
        #[serde(default)]
        dictation_languages: Option<Vec<String>>,
    },
    /// Give one pipeline its own Voice settings; `null` follows the default
    /// (protocol minor 36). Replies `AudioOptions`.
    /// Transcribe an audio or video file as a meeting (protocol minor 37):
    /// `path` is absolute; the file is only read. Replies `Meeting`; the
    /// meeting is `processing` with stage `import`, then as usual.
    ImportMeeting {
        path: String,
        #[serde(default, deserialize_with = "deserialize_meeting_text")]
        title: Option<String>,
    },
    /// Meeting options (protocol minor 39).
    GetMeetingOptions,
    /// Change meeting options; a field left out stays (protocol minor 39).
    SetMeetingOptions {
        #[serde(default)]
        auto_notes: Option<bool>,
        /// A microphone's name, or `""` to follow the Microphone stage
        /// (minor 43).
        #[serde(default)]
        microphone: Option<String>,
        /// An output's name, or `""` for the system's default (minor 43).
        #[serde(default)]
        speaker: Option<String>,
        /// Record the call's sound (the output) as well (minor 43).
        #[serde(default)]
        call_audio: Option<bool>,
        /// Record the microphone as well (minor 43); not both off.
        #[serde(default)]
        room_audio: Option<bool>,
        /// Keep pictures of what is shown on screen (minor 45).
        #[serde(default)]
        screen: Option<bool>,
        /// Go over a recording as soon as it stops (minor 52).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        process_on_stop: Option<bool>,
    },
    /// Allow or refuse a tool call the assistant asked for (minor 46).
    /// `always` allows that tool for the rest of the chat.
    AssistantToolDecision {
        #[serde(deserialize_with = "deserialize_turn_id")]
        call_id: String,
        allow: bool,
        #[serde(default)]
        always: bool,
    },
    /// A meeting's screen picture as base64 PNG, small (minor 45).
    GetMeetingSlide {
        #[serde(deserialize_with = "deserialize_meeting_id")]
        id: String,
        file: String,
    },
    /// Pause, resume or stop what is being done with a meeting: recording,
    /// transcribing, describing its screens or writing its notes (minor 48).
    /// `action` is `pause`, `resume` or `stop`.
    MeetingControl {
        #[serde(deserialize_with = "deserialize_meeting_id")]
        id: String,
        action: String,
    },
    /// Switch the screen, the computer's sound or the microphone of the
    /// meeting being recorded on or off; unset leaves it (minor 50; the
    /// microphone apart from minor 51). Answers with the meeting.
    MeetingSources {
        #[serde(deserialize_with = "deserialize_meeting_id")]
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        screen: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sound: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        microphone: Option<bool>,
    },
    SetPipelineVoice {
        pipeline_id: String,
        #[serde(default)]
        echo_cancel: Option<bool>,
        #[serde(default)]
        only_my_voice: Option<bool>,
    },
    /// Exact short-lived recovery result for the transaction named by a
    /// session event. This is not a listing and is not optional History.
    GetRecoveryEntry {
        txid: String,
    },
    /// Run one isolated capability check by id
    /// (`mic` | `delivery` | `shortcuts` | `theme-store`).
    RunCapabilityCheck {
        check_id: String,
    },
    /// Ask the user/system portal for a keyboard-only permission session.
    RequestPortalKeyboardPermission {
        #[serde(
            serialize_with = "serialize_portal_operation_id",
            deserialize_with = "deserialize_portal_operation_id"
        )]
        operation_id: String,
        #[serde(
            serialize_with = "serialize_portal_parent_window",
            deserialize_with = "deserialize_portal_parent_window"
        )]
        parent_window: String,
    },
    /// Cancel exactly one in-flight portal permission request.
    CancelPortalKeyboardPermission {
        #[serde(
            serialize_with = "serialize_portal_operation_id",
            deserialize_with = "deserialize_portal_operation_id"
        )]
        operation_id: String,
    },
    /// Revoke the daemon-owned portal keyboard permission session, if any.
    RevokePortalKeyboardPermission,
    /// Start daemon-owned provisioning from one bounded local source path.
    /// Model identity, expected digest, destination and deadline remain
    /// daemon policy and are intentionally absent from the wire command.
    StartOfflineModelProvision {
        #[serde(
            serialize_with = "serialize_model_provision_source_path",
            deserialize_with = "deserialize_model_provision_source_path"
        )]
        source_path: String,
    },
    /// Cancel exactly one daemon-owned model-provisioning operation.
    CancelOfflineModelProvision {
        #[serde(
            serialize_with = "serialize_model_provision_operation_id",
            deserialize_with = "deserialize_model_provision_operation_id"
        )]
        operation_id: String,
    },
}

impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::GetStatus => f.write_str("GetStatus"),
            Self::StartSession {
                pipeline_id,
                activation,
                recognition,
            } => f
                .debug_struct("StartSession")
                .field("pipeline_id", pipeline_id)
                .field("activation", activation)
                .field("recognition", recognition)
                .finish(),
            Self::StopSession => f.write_str("StopSession"),
            Self::ToggleSession {
                pipeline_id,
                activation,
                recognition,
            } => f
                .debug_struct("ToggleSession")
                .field("pipeline_id", pipeline_id)
                .field("activation", activation)
                .field("recognition", recognition)
                .finish(),
            Self::CancelSession => f.write_str("CancelSession"),
            Self::Subscribe => f.write_str("Subscribe"),
            Self::Unsubscribe => f.write_str("Unsubscribe"),
            Self::ListPipelines => f.write_str("ListPipelines"),
            Self::SelectPipeline { pipeline_id } => f
                .debug_struct("SelectPipeline")
                .field("pipeline_id", pipeline_id)
                .finish(),
            Self::GetHistory => f.write_str("GetHistory"),
            Self::GetHistoryEntry { txid } => f
                .debug_struct("GetHistoryEntry")
                .field("txid", txid)
                .finish(),
            Self::DeleteHistoryEntry { txid } => f
                .debug_struct("DeleteHistoryEntry")
                .field("txid", txid)
                .finish(),
            Self::ListShortcuts => f.write_str("ListShortcuts"),
            Self::SetShortcut { binding, replace } => f
                .debug_struct("SetShortcut")
                .field("binding", binding)
                .field("replace", replace)
                .finish(),
            Self::RemoveShortcut { modifiers, key } => f
                .debug_struct("RemoveShortcut")
                .field("modifiers", modifiers)
                .field("key", key)
                .finish(),
            Self::ListModels => f.write_str("ListModels"),
            Self::SelectModel { id } => f.debug_struct("SelectModel").field("id", id).finish(),
            Self::SetPipelineEngine {
                pipeline_id,
                model_id,
                compute,
            } => f
                .debug_struct("SetPipelineEngine")
                .field("pipeline_id", pipeline_id)
                .field("model_id", model_id)
                .field("compute", compute)
                .finish(),
            Self::DownloadModel { id } => {
                f.debug_struct("DownloadModel").field("id", id).finish()
            }
            Self::CancelModelDownload { id } => f
                .debug_struct("CancelModelDownload")
                .field("id", id)
                .finish(),
            Self::ListStore => f.write_str("ListStore"),
            Self::AssistantSend {
                turn_id,
                messages,
                chat_id,
                speak,
                pictures,
            } => f
                .debug_struct("AssistantSend")
                .field("turn_id", turn_id)
                .field("messages", &messages.len())
                .field("chat_id", chat_id)
                .field("speak", speak)
                .field("pictures", &pictures.len())
                .finish(),
            Self::AssistantLibrary { op, kind, id, .. } => f
                .debug_struct("AssistantLibrary")
                .field("op", op)
                .field("kind", kind)
                .field("id", id)
                .field("value", &"<redacted>")
                .finish(),
            Self::VoiceMode {
                action,
                settings,
                down,
            } => f
                .debug_struct("VoiceMode")
                .field("action", action)
                .field("settings", settings)
                .field("down", down)
                .finish(),
            Self::AssistantCancel { turn_id } => f
                .debug_struct("AssistantCancel")
                .field("turn_id", turn_id)
                .finish(),
            Self::ListLanguageModels => f.write_str("ListLanguageModels"),
            Self::AssistantUnload => f.write_str("AssistantUnload"),
            Self::CopyHistoryEntry { txid } => f
                .debug_struct("CopyHistoryEntry")
                .field("txid", txid)
                .finish(),
            Self::CheckWriting { dialect, .. } => f
                .debug_struct("CheckWriting")
                .field("text", &"<redacted>")
                .field("dialect", dialect)
                .finish_non_exhaustive(),
            Self::ListWritingRules => f.write_str("ListWritingRules"),
            Self::MeetingStart { .. } => f.write_str("MeetingStart"),
            Self::MeetingStop => f.write_str("MeetingStop"),
            Self::ListMeetings => f.write_str("ListMeetings"),
            Self::GetMeeting { id } => f.debug_struct("GetMeeting").field("id", id).finish(),
            Self::ProcessMeeting { id, model, .. } => f
                .debug_struct("ProcessMeeting")
                .field("id", id)
                .field("model", model)
                .finish_non_exhaustive(),
            Self::RenameMeetingSpeaker { id, .. } => f
                .debug_struct("RenameMeetingSpeaker")
                .field("id", id)
                .finish_non_exhaustive(),
            Self::DeleteMeeting { id } => f.debug_struct("DeleteMeeting").field("id", id).finish(),
            Self::GetAudioOptions => f.write_str("GetAudioOptions"),
            Self::SetAudioOptions {
                echo_cancel,
                only_my_voice,
                noise_suppression,
                microphone,
                sounds,
                voice_strictness,
            } => f
                .debug_struct("SetAudioOptions")
                .field("voice_strictness", voice_strictness)
                .field("microphone", microphone)
                .field("sounds", sounds)
                .field("echo_cancel", echo_cancel)
                .field("only_my_voice", only_my_voice)
                .field("noise_suppression", noise_suppression)
                .finish(),
            Self::TrainVoice => f.write_str("TrainVoice"),
            Self::ForgetVoice => f.write_str("ForgetVoice"),
            Self::GetVocabulary => f.write_str("GetVocabulary"),
            Self::GetHoldKey => f.write_str("GetHoldKey"),
            Self::SetHistoryRetention { retention } => f
                .debug_struct("SetHistoryRetention")
                .field("retention", retention)
                .finish(),
            Self::ClearHistory => f.write_str("ClearHistory"),
            Self::GetTextOptions => f.write_str("GetTextOptions"),
            // A path can name the user's files: left out.
            Self::ImportMeeting { .. } => f.debug_struct("ImportMeeting").finish_non_exhaustive(),
            Self::GetMeetingOptions => f.write_str("GetMeetingOptions"),
            Self::SetMeetingOptions {
                auto_notes,
                microphone,
                speaker,
                call_audio,
                room_audio,
                screen,
                process_on_stop,
            } => f
                .debug_struct("SetMeetingOptions")
                .field("auto_notes", auto_notes)
                .field("microphone", microphone)
                .field("speaker", speaker)
                .field("call_audio", call_audio)
                .field("room_audio", room_audio)
                .field("screen", screen)
                .field("process_on_stop", process_on_stop)
                .finish(),
            Self::AssistantToolDecision {
                call_id,
                allow,
                always,
            } => f
                .debug_struct("AssistantToolDecision")
                .field("call_id", call_id)
                .field("allow", allow)
                .field("always", always)
                .finish(),
            Self::GetMeetingSlide { id, file } => f
                .debug_struct("GetMeetingSlide")
                .field("id", id)
                .field("file", file)
                .finish(),
            Self::MeetingControl { id, action } => f
                .debug_struct("MeetingControl")
                .field("id", id)
                .field("action", action)
                .finish(),
            Self::MeetingSources {
                id,
                screen,
                sound,
                microphone,
            } => f
                .debug_struct("MeetingSources")
                .field("id", id)
                .field("screen", screen)
                .field("sound", sound)
                .field("microphone", microphone)
                .finish(),
            Self::SetPipelineVoice {
                pipeline_id,
                echo_cancel,
                only_my_voice,
            } => f
                .debug_struct("SetPipelineVoice")
                .field("pipeline_id", pipeline_id)
                .field("echo_cancel", echo_cancel)
                .field("only_my_voice", only_my_voice)
                .finish(),
            Self::SetTextOptions {
                numbers_as_digits,
                delivery_method,
                cleanup_idle_minutes,
                spoken_breaks,
                dictation_language,
                dictation_languages,
            } => f
                .debug_struct("SetTextOptions")
                .field("spoken_breaks", spoken_breaks)
                .field("numbers_as_digits", numbers_as_digits)
                .field("delivery_method", delivery_method)
                .field("cleanup_idle_minutes", cleanup_idle_minutes)
                .field("dictation_language", dictation_language)
                .field("dictation_languages", dictation_languages)
                .finish(),
            Self::SetHoldKey { key } => f.debug_struct("SetHoldKey").field("key", key).finish(),
            // Words can be private (names): counts only.
            Self::SetVocabulary { vocabulary } => f
                .debug_struct("SetVocabulary")
                .field("words", &vocabulary.words.len())
                .field("replacements", &vocabulary.replacements.len())
                .finish(),
            Self::SummarizeMeeting {
                id, turn_id, kind, ..
            } => f
                .debug_struct("SummarizeMeeting")
                .field("id", id)
                .field("turn_id", turn_id)
                .field("kind", kind)
                .finish_non_exhaustive(),
            Self::WritingAction {
                turn_id, action, ..
            } => f
                .debug_struct("WritingAction")
                .field("turn_id", turn_id)
                .field("action", action)
                .field("text", &"<redacted>")
                .finish_non_exhaustive(),
            Self::SelectLanguageModel { role, id } => f
                .debug_struct("SelectLanguageModel")
                .field("role", role)
                .field("id", id)
                .finish(),
            Self::RemoveStoreItem { id } => {
                f.debug_struct("RemoveStoreItem").field("id", id).finish()
            }
            Self::GetRecoveryEntry { txid } => f
                .debug_struct("GetRecoveryEntry")
                .field("txid", txid)
                .finish(),
            Self::RunCapabilityCheck { check_id } => f
                .debug_struct("RunCapabilityCheck")
                .field("check_id", check_id)
                .finish(),
            Self::RequestPortalKeyboardPermission { .. } => f.write_str(
                "RequestPortalKeyboardPermission { operation_id: <redacted>, parent_window: <redacted> }",
            ),
            Self::CancelPortalKeyboardPermission { .. } => f.write_str(
                "CancelPortalKeyboardPermission { operation_id: <redacted> }",
            ),
            Self::RevokePortalKeyboardPermission => {
                f.write_str("RevokePortalKeyboardPermission")
            }
            Self::StartOfflineModelProvision { .. } => {
                f.write_str("StartOfflineModelProvision { source_path: <redacted> }")
            }
            Self::CancelOfflineModelProvision { .. } => {
                f.write_str("CancelOfflineModelProvision { operation_id: <redacted> }")
            }
        }
    }
}

impl Command {
    /// Wire message kind for this command.
    pub fn kind(&self) -> MessageKind {
        match self {
            Self::GetStatus => MessageKind::GetStatus,
            Self::StartSession { .. } => MessageKind::StartSession,
            Self::StopSession => MessageKind::StopSession,
            Self::ToggleSession { .. } => MessageKind::ToggleSession,
            Self::CancelSession => MessageKind::CancelSession,
            Self::Subscribe => MessageKind::Subscribe,
            Self::Unsubscribe => MessageKind::Unsubscribe,
            Self::ListPipelines => MessageKind::ListPipelines,
            Self::SelectPipeline { .. } => MessageKind::SelectPipeline,
            Self::GetHistory => MessageKind::GetHistory,
            Self::GetHistoryEntry { .. } => MessageKind::GetHistoryEntry,
            Self::DeleteHistoryEntry { .. } => MessageKind::DeleteHistoryEntry,
            Self::ListShortcuts => MessageKind::ListShortcuts,
            Self::SetShortcut { .. } => MessageKind::SetShortcut,
            Self::RemoveShortcut { .. } => MessageKind::RemoveShortcut,
            Self::ListModels => MessageKind::ListModels,
            Self::SelectModel { .. } => MessageKind::SelectModel,
            Self::SetPipelineEngine { .. } => MessageKind::SetPipelineEngine,
            Self::DownloadModel { .. } => MessageKind::DownloadModel,
            Self::CancelModelDownload { .. } => MessageKind::CancelModelDownload,
            Self::ListStore => MessageKind::ListStore,
            Self::RemoveStoreItem { .. } => MessageKind::RemoveStoreItem,
            Self::AssistantSend { .. } => MessageKind::AssistantSend,
            Self::AssistantCancel { .. } => MessageKind::AssistantCancel,
            Self::ListLanguageModels => MessageKind::ListLanguageModels,
            Self::SelectLanguageModel { .. } => MessageKind::SelectLanguageModel,
            Self::AssistantUnload => MessageKind::AssistantUnload,
            Self::CopyHistoryEntry { .. } => MessageKind::CopyHistoryEntry,
            Self::CheckWriting { .. } => MessageKind::CheckWriting,
            Self::ListWritingRules => MessageKind::ListWritingRules,
            Self::WritingAction { .. } => MessageKind::WritingAction,
            Self::MeetingStart { .. } => MessageKind::MeetingStart,
            Self::MeetingStop => MessageKind::MeetingStop,
            Self::ListMeetings => MessageKind::ListMeetings,
            Self::GetMeeting { .. } => MessageKind::GetMeeting,
            Self::ProcessMeeting { .. } => MessageKind::ProcessMeeting,
            Self::RenameMeetingSpeaker { .. } => MessageKind::RenameMeetingSpeaker,
            Self::DeleteMeeting { .. } => MessageKind::DeleteMeeting,
            Self::SummarizeMeeting { .. } => MessageKind::SummarizeMeeting,
            Self::GetAudioOptions => MessageKind::GetAudioOptions,
            Self::SetAudioOptions { .. } => MessageKind::SetAudioOptions,
            Self::TrainVoice => MessageKind::TrainVoice,
            Self::ForgetVoice => MessageKind::ForgetVoice,
            Self::GetVocabulary => MessageKind::GetVocabulary,
            Self::GetHoldKey => MessageKind::GetHoldKey,
            Self::SetHistoryRetention { .. } => MessageKind::SetHistoryRetention,
            Self::ClearHistory => MessageKind::ClearHistory,
            Self::GetTextOptions => MessageKind::GetTextOptions,
            Self::SetPipelineVoice { .. } => MessageKind::SetPipelineVoice,
            Self::ImportMeeting { .. } => MessageKind::ImportMeeting,
            Self::GetMeetingOptions => MessageKind::GetMeetingOptions,
            Self::SetMeetingOptions { .. } => MessageKind::SetMeetingOptions,
            Self::GetMeetingSlide { .. } => MessageKind::GetMeetingSlide,
            Self::MeetingControl { .. } => MessageKind::MeetingControl,
            Self::MeetingSources { .. } => MessageKind::MeetingSources,
            Self::AssistantToolDecision { .. } => MessageKind::AssistantToolDecision,
            Self::AssistantLibrary { .. } => MessageKind::AssistantLibrary,
            Self::VoiceMode { .. } => MessageKind::VoiceMode,
            Self::SetTextOptions { .. } => MessageKind::SetTextOptions,
            Self::SetHoldKey { .. } => MessageKind::SetHoldKey,
            Self::SetVocabulary { .. } => MessageKind::SetVocabulary,
            Self::GetRecoveryEntry { .. } => MessageKind::GetRecoveryEntry,
            Self::RunCapabilityCheck { .. } => MessageKind::RunCapabilityCheck,
            Self::RequestPortalKeyboardPermission { .. } => {
                MessageKind::RequestPortalKeyboardPermission
            }
            Self::CancelPortalKeyboardPermission { .. } => {
                MessageKind::CancelPortalKeyboardPermission
            }
            Self::RevokePortalKeyboardPermission => MessageKind::RevokePortalKeyboardPermission,
            Self::StartOfflineModelProvision { .. } => MessageKind::StartOfflineModelProvision,
            Self::CancelOfflineModelProvision { .. } => MessageKind::CancelOfflineModelProvision,
        }
    }
}

fn validate_model_provision_source_path(value: &str) -> Result<(), &'static str> {
    if value.is_empty() || value.len() > MAX_MODEL_PROVISION_SOURCE_PATH_BYTES {
        return Err("source_path must be 1..=4096 UTF-8 bytes");
    }
    if value.chars().any(char::is_control) {
        return Err("source_path must not contain control characters");
    }
    Ok(())
}

fn serialize_model_provision_source_path<S: Serializer>(
    value: &str,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    validate_model_provision_source_path(value).map_err(serde::ser::Error::custom)?;
    serializer.serialize_str(value)
}

fn deserialize_model_provision_source_path<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    validate_model_provision_source_path(&value).map_err(serde::de::Error::custom)?;
    Ok(value)
}

fn validate_model_provision_operation_id(value: &str) -> Result<(), &'static str> {
    if value.is_empty() || value.len() > MAX_MODEL_PROVISION_OPERATION_ID_BYTES {
        return Err("operation_id must be 1..=64 bytes");
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err("operation_id may contain only ASCII letters, digits, '-' and '_'");
    }
    Ok(())
}

fn serialize_model_provision_operation_id<S: Serializer>(
    value: &str,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    validate_model_provision_operation_id(value).map_err(serde::ser::Error::custom)?;
    serializer.serialize_str(value)
}

fn deserialize_model_provision_operation_id<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    validate_model_provision_operation_id(&value).map_err(serde::de::Error::custom)?;
    Ok(value)
}

fn validate_portal_operation_id(value: &str) -> Result<(), &'static str> {
    if value.is_empty() || value.len() > MAX_PORTAL_OPERATION_ID_BYTES {
        return Err("operation_id must be 1..=64 bytes");
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err("operation_id may contain only ASCII letters, digits, '-' and '_'");
    }
    Ok(())
}

fn serialize_portal_operation_id<S: Serializer>(
    value: &str,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    validate_portal_operation_id(value).map_err(serde::ser::Error::custom)?;
    serializer.serialize_str(value)
}

fn deserialize_portal_operation_id<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    validate_portal_operation_id(&value).map_err(serde::de::Error::custom)?;
    Ok(value)
}

mod optional_portal_operation_id {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(
        value: &Option<String>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(value) => super::serialize_portal_operation_id(value, serializer),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<String>, D::Error> {
        let value = Option::<String>::deserialize(deserializer)?;
        value
            .map(|value| {
                super::validate_portal_operation_id(&value)
                    .map(|()| value)
                    .map_err(serde::de::Error::custom)
            })
            .transpose()
    }
}

fn validate_portal_parent_window(value: &str) -> Result<(), &'static str> {
    if value.is_empty() || value.len() > MAX_PORTAL_PARENT_WINDOW_BYTES {
        return Err("parent_window must be 1..=4096 bytes");
    }
    if value.chars().any(char::is_control) {
        return Err("parent_window must not contain control characters");
    }
    Ok(())
}

fn serialize_portal_parent_window<S: Serializer>(
    value: &str,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    validate_portal_parent_window(value).map_err(serde::ser::Error::custom)?;
    serializer.serialize_str(value)
}

fn deserialize_portal_parent_window<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    validate_portal_parent_window(&value).map_err(serde::de::Error::custom)?;
    Ok(value)
}

fn validate_recognition_tag(value: &str) -> Result<(), &'static str> {
    if value.is_empty() || value.len() > MAX_RECOGNITION_TAG_BYTES {
        return Err("recognition tag must be 1..=32 bytes");
    }
    if !value
        .split(['-', '_'])
        .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_alphanumeric()))
    {
        return Err("recognition tag must contain bounded ASCII alphanumeric subtags");
    }
    Ok(())
}

/// Bounded wire tag for a language, script, or locale. Original casing is
/// preserved; the daemon converts it to the normalized domain representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecognitionTagDto(String);

impl RecognitionTagDto {
    pub fn parse(value: impl Into<String>) -> Result<Self, IpcError> {
        let value = value.into();
        validate_recognition_tag(&value).map_err(IpcError::bad_request)?;
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Serialize for RecognitionTagDto {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        validate_recognition_tag(&self.0).map_err(serde::ser::Error::custom)?;
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for RecognitionTagDto {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        validate_recognition_tag(&value).map_err(serde::de::Error::custom)?;
        Ok(Self(value))
    }
}

fn validate_recognition_context_hints(hints: &[String]) -> Result<(), &'static str> {
    if hints.is_empty() {
        return Err("recognition context hints must not be empty");
    }
    if hints.len() > MAX_RECOGNITION_CONTEXT_HINTS {
        return Err("too many recognition context hints");
    }
    if hints.iter().any(|hint| {
        hint.is_empty()
            || hint.len() > MAX_RECOGNITION_CONTEXT_HINT_BYTES
            || hint.as_bytes().contains(&0)
    }) {
        return Err("recognition context hint is empty, oversized, or contains NUL");
    }
    Ok(())
}

/// Bounded context/bias hints. Debug output exposes only the number of hints.
#[derive(Clone, PartialEq, Eq)]
pub struct RecognitionContextHintsDto(Vec<String>);

impl RecognitionContextHintsDto {
    pub fn try_new(hints: Vec<String>) -> Result<Self, IpcError> {
        validate_recognition_context_hints(&hints).map_err(IpcError::bad_request)?;
        Ok(Self(hints))
    }

    #[must_use]
    pub fn as_slice(&self) -> &[String] {
        &self.0
    }
}

impl std::fmt::Debug for RecognitionContextHintsDto {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RecognitionContextHintsDto")
            .field("hint_count", &self.0.len())
            .finish()
    }
}

impl Serialize for RecognitionContextHintsDto {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        validate_recognition_context_hints(&self.0).map_err(serde::ser::Error::custom)?;
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for RecognitionContextHintsDto {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let hints = Vec::<String>::deserialize(deserializer)?;
        validate_recognition_context_hints(&hints).map_err(serde::de::Error::custom)?;
        Ok(Self(hints))
    }
}

/// Spoken-language selection. `Unspecified` is the engine default and differs
/// from an explicit request for automatic language detection.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SpokenLanguageDto {
    #[default]
    Unspecified,
    AutoDetect,
    Explicit {
        tag: RecognitionTagDto,
    },
}

impl SpokenLanguageDto {
    fn is_unspecified(&self) -> bool {
        matches!(self, Self::Unspecified)
    }
}

/// Portable recognition request used at the IPC boundary. Each setting stays
/// distinct so clients cannot conflate speech recognition, translation, script,
/// locale formatting, or context bias.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RecognitionSettingsDto {
    #[serde(default, skip_serializing_if = "SpokenLanguageDto::is_unspecified")]
    pub spoken_language: SpokenLanguageDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_language: Option<RecognitionTagDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_script: Option<RecognitionTagDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translation_target: Option<RecognitionTagDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<RecognitionTagDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_hints: Option<RecognitionContextHintsDto>,
}

impl std::fmt::Debug for RecognitionSettingsDto {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RecognitionSettingsDto")
            .field("spoken_language", &self.spoken_language)
            .field("output_language", &self.output_language)
            .field("output_script", &self.output_script)
            .field("translation_target", &self.translation_target)
            .field("locale", &self.locale)
            .field("context_hints", &self.context_hints)
            .finish()
    }
}

/// Status/capability snapshot payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusDto {
    pub daemon_build: String,
    pub protocol_minor: u16,
    pub active_session: Option<ActiveSessionDto>,
    /// Monotonic offsets for the most recently completed local session.
    /// Keys are fixed stage names and values are microseconds from admission;
    /// no transcript, audio, target, or transaction identity is included.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_session_timings_us: Option<StageTimingsUs>,
    pub capabilities: Vec<String>,
    /// Typed lifecycle of the configured bundled installed model. Absent for
    /// older peers and daemon compositions that do not use installed models.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed_model: Option<Box<InstalledModelLifecycleDto>>,
    /// Keyboard portal permission/transport status. Absent on protocol-minor
    /// 0/1 peers and when this daemon has not composed the portal service.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub portal_keyboard: Option<PortalKeyboardStatusDto>,
    /// Current daemon-owned offline model provisioning operation. Absent for
    /// older peers and when no provisioning operation has been observed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_provisioning: Option<Box<ModelProvisionStatusDto>>,
}

macro_rules! lossless_wire_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $($variant:ident => $tag:literal),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub enum $name {
            $($variant,)+
            /// Unrecognized wire tag preserved verbatim for forward compatibility.
            Unknown(String),
        }

        impl $name {
            pub fn wire_tag(&self) -> &str {
                match self {
                    $(Self::$variant => $tag,)+
                    Self::Unknown(raw) => raw.as_str(),
                }
            }

            pub fn from_wire_tag(tag: &str) -> Self {
                match tag {
                    $($tag => Self::$variant,)+
                    other => Self::Unknown(other.to_owned()),
                }
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.wire_tag())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let tag = String::deserialize(deserializer)?;
                Ok(Self::from_wire_tag(&tag))
            }
        }
    };
}

lossless_wire_enum! {
    /// Daemon-owned lifecycle of the selected bundled installed model.
    pub enum InstalledModelLifecycleDto {
        AwaitingModel => "awaiting_model",
        Provisioning => "provisioning",
        PublishedUnconfirmed => "published_unconfirmed",
        ActivationFailed => "activation_failed",
        Ready => "ready",
    }
}

lossless_wire_enum! {
    /// Daemon-owned lifecycle of one offline model provisioning operation.
    pub enum ModelProvisionStateDto {
        Importing => "importing",
        CancelRequested => "cancel_requested",
        Activating => "activating",
        Ready => "ready",
        Cancelled => "cancelled",
        TimedOut => "timed_out",
        Failed => "failed",
    }
}

lossless_wire_enum! {
    /// Publication certainty for one provisioning operation.
    pub enum ModelProvisionPublicationDto {
        NotPublished => "not_published",
        VerifiedInstalled => "verified_installed",
        PublishedUnconfirmed => "published_unconfirmed",
    }
}

lossless_wire_enum! {
    /// Cleanup certainty for temporary provisioning resources.
    pub enum ModelProvisionCleanupDto {
        NotRequired => "not_required",
        Confirmed => "confirmed",
        Failed => "failed",
    }
}

lossless_wire_enum! {
    /// Content-free terminal failure category for model provisioning.
    pub enum ModelProvisionFailureDto {
        SourceRejected => "source_rejected",
        ImportFailed => "import_failed",
        CleanupFailed => "cleanup_failed",
        PublishedUnconfirmed => "published_unconfirmed",
        BindingFailed => "binding_failed",
        ActivationFailed => "activation_failed",
        ShuttingDown => "shutting_down",
        WorkerFailed => "worker_failed",
    }
}

lossless_wire_enum! {
    /// Content-free, closed reason microphone capture stopped accepting audio.
    /// Clients render it as state; it never carries audio or transcript data.
    pub enum CaptureEndDto {
        FinalizationRequested => "finalization_requested",
        EndpointDetected => "endpoint_detected",
        SafetyLimit => "safety_limit",
        SourceEnded => "source_ended",
    }
}

lossless_wire_enum! {
    /// Typed, closed activation source for one atomic [`Command::ToggleSession`]
    /// request. It is a fixed enumeration of Narrivy-owned activation
    /// surfaces, never an arbitrary caller policy string, and it carries no
    /// user content:
    ///
    /// - `installed_cli_command`: the installed `narrivy-cli toggle` command,
    ///   e.g. bound by the user to a desktop/compositor shortcut;
    /// - `desktop_client`: the Narrivy desktop client's own activation control;
    /// - `portal_global_shortcuts`: a live XDG GlobalShortcuts portal listener
    ///   owned by the daemon.
    ///
    /// Unknown values are preserved verbatim for forward compatibility, but a
    /// request that carries one must fail closed at execution: only the known
    /// variants are accepted activation sources.
    pub enum ActivationSourceDto {
        InstalledCliCommand => "installed_cli_command",
        DesktopClient => "desktop_client",
        PortalGlobalShortcuts => "portal_global_shortcuts",
    }
}

impl Default for ActivationSourceDto {
    /// The installed command is the only activation source the packaged
    /// fallback lane uses today.
    fn default() -> Self {
        Self::InstalledCliCommand
    }
}

lossless_wire_enum! {
    /// Typed outcome of one atomic toggle. `Started` means exactly one new
    /// session was admitted; the two stopping outcomes mean no second session
    /// was opened and the existing session's graceful stop was requested (or
    /// was already in progress).
    pub enum ToggleOutcomeDto {
        Started => "started",
        FinalizationRequested => "finalization_requested",
        AlreadyStopping => "already_stopping",
    }
}

/// Typed, content-free reply to one atomic [`Command::ToggleSession`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToggleResultDto {
    pub outcome: ToggleOutcomeDto,
    /// Echo of the typed activation source, never a caller policy string.
    pub activation: ActivationSourceDto,
    /// Identity of the session this toggle started or requested to stop.
    #[serde(default, with = "wire_u128::option")]
    pub session_id: Option<u128>,
    /// Transaction identity matching [`Self::session_id`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transaction_id: Option<String>,
    /// Pipeline identity of the session this toggle started or stopped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pipeline_id: Option<String>,
}

/// Typed, content-free snapshot of one daemon-owned offline model provision.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelProvisionStatusDto {
    #[serde(
        serialize_with = "serialize_model_provision_operation_id",
        deserialize_with = "deserialize_model_provision_operation_id"
    )]
    pub operation_id: String,
    pub state: ModelProvisionStateDto,
    pub publication: ModelProvisionPublicationDto,
    pub cleanup: ModelProvisionCleanupDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<ModelProvisionFailureDto>,
    pub revision: u64,
}

impl std::fmt::Debug for ModelProvisionStatusDto {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ModelProvisionStatusDto")
            .field("operation_id", &"<redacted>")
            .field("state", &self.state)
            .field("publication", &self.publication)
            .field("cleanup", &self.cleanup)
            .field("failure", &self.failure)
            .field("revision", &self.revision)
            .finish()
    }
}

lossless_wire_enum! {
    /// Read-only eligibility for starting a keyboard portal permission request.
    pub enum PortalEligibilityDto {
        Unavailable => "unavailable",
        ReadyToRequest => "ready_to_request",
        ProbeFailed => "probe_failed",
    }
}

lossless_wire_enum! {
    /// Truthful daemon-owned portal lifecycle state. `Active` means the portal
    /// transport is connected; it does not qualify direct text delivery.
    pub enum PortalLifecycleDto {
        Unavailable => "unavailable",
        Idle => "idle",
        Requesting => "requesting",
        Granted => "granted",
        Connecting => "connecting",
        Active => "active",
        Closing => "closing",
        Invalid => "invalid",
    }
}

lossless_wire_enum! {
    /// Content-free failure category for the last portal permission operation.
    pub enum PortalFailureDto {
        InvalidParent => "invalid_parent",
        AlreadyActive => "already_active",
        Invalidated => "invalidated",
        NotActive => "not_active",
        Cancelled => "cancelled",
        TimedOut => "timed_out",
        PermissionDenied => "permission_denied",
        PortalUnavailable => "portal_unavailable",
        RequestFailed => "request_failed",
        TransportFailed => "transport_failed",
        Revoked => "revoked",
        CloseFailed => "close_failed",
    }
}

lossless_wire_enum! {
    /// Cleanup certainty following the last portal permission operation.
    pub enum PortalCleanupDto {
        NotRequired => "not_required",
        ConfirmedClosed => "confirmed_closed",
        Failed => "failed",
    }
}

/// Typed portal permission status. Sender and delivery qualification remain
/// explicit so a connected transport cannot be mistaken for working delivery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortalKeyboardStatusDto {
    pub eligibility: PortalEligibilityDto,
    pub lifecycle: PortalLifecycleDto,
    pub can_request: bool,
    pub permission_active: bool,
    pub transport_connected: bool,
    pub sender_available: bool,
    pub delivery_qualified: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "optional_portal_operation_id"
    )]
    pub current_operation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_failure: Option<PortalFailureDto>,
    pub cleanup: PortalCleanupDto,
    pub sequence: u64,
}

/// Content-free monotonic stage offsets. The values are relative microseconds,
/// never wall-clock timestamps, and carry no transcript or target identity.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StageTimingsUs(pub BTreeMap<String, u64>);

impl Serialize for StageTimingsUs {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let object: serde_json::Value = self
            .0
            .iter()
            .map(|(key, micros)| (key.clone(), serde_json::Value::from(*micros)))
            .collect::<serde_json::Map<_, _>>()
            .into();
        object.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for StageTimingsUs {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        match value {
            serde_json::Value::Null => Ok(Self(BTreeMap::new())),
            serde_json::Value::Object(map) => {
                let mut out = BTreeMap::new();
                for (key, entry) in map {
                    let micros = entry.as_u64().ok_or_else(|| {
                        serde::de::Error::custom(format!(
                            "stage timing {key:?} is not an unsigned microsecond number"
                        ))
                    })?;
                    out.insert(key, micros);
                }
                Ok(Self(out))
            }
            _ => Err(serde::de::Error::custom(
                "stage_timings_us must be a JSON object",
            )),
        }
    }
}

/// One active session as observed by the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveSessionDto {
    #[serde(with = "wire_u128")]
    pub session_id: u128,
    pub transaction_id: Option<String>,
    pub pipeline_id: Option<String>,
    pub state: String,
    /// P6 ADDITIVE timing telemetry (serde default keeps wire compatibility);
    /// daemons that do not measure leave it absent/None.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_timings_us: Option<StageTimingsUs>,
}

/// One selectable pipeline (v1.3 first-class Pipelines).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineSummaryDto {
    pub id: String,
    pub name: String,
    pub version: u32,
    pub enabled: bool,
    /// The model this pipeline recognises with (protocol minor 15). Absent
    /// when it uses whatever model is loaded, and from older daemons.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    /// The device this pipeline asks for: `auto`, `gpu` or `cpu` (protocol
    /// minor 15). Absent from a daemon that cannot choose a device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compute: Option<String>,
}

/// Content-safe optional-History listing row. It must never be populated from
/// the short-lived recovery journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntryDto {
    pub txid: String,
    /// snake_case journal state (pending|committed|delivered|cancelled).
    pub state: String,
    pub pipeline_id: Option<String>,
    pub has_delivered_text: bool,
    pub updated_unix_ms: u64,
    /// How long transcription took, in microseconds (protocol minor 12).
    /// Content-free. Absent when it was not measured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcribe_us: Option<u64>,
}

/// Full transcript variants for one optional History entry.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntryDetailDto {
    pub txid: String,
    pub state: String,
    pub raw: Option<String>,
    pub corrected: Option<String>,
    pub delivered: Option<String>,
    pub pipeline_id: Option<String>,
    /// Typed, content-free reason microphone capture stopped for this
    /// transaction. Optional and additive: absent on older peers and on
    /// transactions that never finalized capture. `safety_limit` means the
    /// documented bounded maximum was reached and the text that was captured
    /// remains exactly what the transaction produced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_end: Option<CaptureEndDto>,
}

impl std::fmt::Debug for HistoryEntryDetailDto {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HistoryEntryDetailDto")
            .field("txid", &self.txid)
            .field("state", &self.state)
            .field("raw", &self.raw.as_ref().map(|_| "<redacted>"))
            .field("corrected", &self.corrected.as_ref().map(|_| "<redacted>"))
            .field("delivered", &self.delivered.as_ref().map(|_| "<redacted>"))
            .field("pipeline_id", &self.pipeline_id)
            .field("capture_end", &self.capture_end)
            .finish()
    }
}

/// Exact short-lived result for one recovery transaction. The distinct type
/// name documents that this payload is never an optional History record.
pub type RecoveryEntryDetailDto = HistoryEntryDetailDto;

/// Result of one isolated capability check (`RunCapabilityCheck`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityCheckResultDto {
    /// check id: `mic` | `delivery` | `shortcuts` | `theme-store`.
    pub id: String,
    pub available: bool,
    /// Content-free detail (paths/tool names/counts only).
    pub detail: String,
}

/// One keyboard shortcut bound to a pipeline. `modifiers` are `Super`,
/// `Ctrl`, `Alt` and `Shift`; `key` is one key name. The language tags are
/// optional and start the pipeline in those languages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShortcutBindingDto {
    pub modifiers: Vec<String>,
    pub key: String,
    pub pipeline_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spoken_language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_language: Option<String>,
}

/// Reply to `ListShortcuts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShortcutsDto {
    /// `managed` when the daemon can add and remove shortcuts in the
    /// desktop's settings, otherwise `manual:<reason>`.
    pub support: String,
    /// The command a user can bind by hand to toggle dictation; clients
    /// append `--pipeline ID` and language flags.
    pub manual_command: String,
    pub bindings: Vec<ShortcutBindingDto>,
}

/// Reply to `SetShortcut`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShortcutSetDto {
    /// `created` | `replaced` | `conflict` | `invalid` | `unsupported`.
    pub outcome: String,
    /// For `conflict`: `shortcut` (another custom shortcut) or `desktop`
    /// (a shortcut the desktop ships).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict_kind: Option<String>,
    /// For `conflict`: a short, content-free name of what holds the keys
    /// (a program name or a desktop action name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict_name: Option<String>,
}

/// One speech model and its state on this computer (reply to `ListModels`).
/// Every field is content-free catalog metadata or a daemon observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelDto {
    pub id: String,
    pub family: String,
    pub variant: String,
    /// Quantization as the publisher names it, for example `Q8_0` or `INT8`.
    pub quant: String,
    pub size_bytes: u64,
    pub license: String,
    /// The engine that runs this model, for example `transcribe.cpp`.
    pub engine: String,
    pub languages: Vec<String>,
    /// The model can produce text while the user is still speaking.
    pub streaming: bool,
    /// Device classes this model may be qualified on (`CPU`, `GPU`, `NPU`).
    /// Eligibility, not a claim that it works on this computer.
    #[serde(default)]
    pub devices: Vec<String>,
    pub source_repo: String,
    pub source_revision: String,
    /// This exact variant passed qualification.
    pub qualified: bool,
    /// `active` (loaded now) | `installed` (on this computer, not loaded) |
    /// `available` (published, not on this computer) | `needs_engine` (the
    /// engine it needs is not part of this daemon). Protocol minor 14 adds
    /// `loading`; minor 16 adds `downloading`.
    pub state: String,
    /// Whether this daemon can download the model when asked (protocol
    /// minor 16).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub downloadable: bool,
    /// Bytes on disk so far while `state` is `downloading`, or left by an
    /// earlier download that can be resumed (protocol minor 16).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub downloaded_bytes: Option<u64>,
    /// Whether `RemoveStoreItem` can delete this installed model (an add-on
    /// model pack; protocol minor 17).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub removable: bool,
}

/// One item the Store offers (reply to `ListStore`, protocol minor 17).
/// Everything here is decided by the daemon; the client only renders it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreItemDto {
    pub id: String,
    /// `runtime` (an accelerator runtime pack) or `model` (a model pack).
    pub kind: String,
    /// What it is for: `speech` for speech models; the runtime's name
    /// (`openvino`) for a runtime.
    pub category: String,
    pub name: String,
    pub size_bytes: u64,
    pub license: String,
    /// Device classes it can use (`CPU`, `GPU`, `NPU`).
    pub devices: Vec<String>,
    /// `available` | `installing` | `installed` | `in_use` | `failed`.
    pub state: String,
    /// Bytes received while installing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed_bytes: Option<u64>,
    /// The failure reason code when `state` is `failed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
    /// `fits` | `needs_runtime` | `does_not_fit` | `unknown`.
    pub fit: String,
    /// Content-free reason for the verdict, such as `npu_found`,
    /// `cpu_only` or `needs:openvino-runtime-…`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fit_reason: Option<String>,
    /// Where it would come from: `published` (the Narrivy Hugging Face
    /// organisation), `local` (a developer source) or `unavailable`.
    pub source: String,
    /// The runtime pack it needs, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires: Option<String>,
}

/// Reply payloads. Serialized as an adjacently tagged enum:
/// `{"result": "status", "payload": {...}}` — adjacent tagging keeps the
/// `Pipelines` vector shape valid on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", content = "payload", rename_all = "snake_case")]
pub enum ResponsePayload {
    Status(StatusDto),
    Pipelines(Vec<PipelineSummaryDto>),
    History(Vec<HistoryEntryDto>),
    HistoryEntry(HistoryEntryDetailDto),
    Shortcuts(ShortcutsDto),
    ShortcutSet(ShortcutSetDto),
    Models(Vec<ModelDto>),
    Store(Vec<StoreItemDto>),
    LanguageModels(Vec<LanguageModelDto>),
    /// Reply to `CheckWriting` (minor 24).
    WritingFindings(Vec<WritingFindingDto>),
    /// Reply to `ListWritingRules` (minor 24).
    WritingRules(Vec<WritingRuleDto>),
    /// Reply to `MeetingStart` (minor 25).
    Meeting(MeetingDto),
    /// Reply to `ListMeetings` (minor 25).
    Meetings(Vec<MeetingDto>),
    /// Reply to `GetMeeting` (minor 25).
    MeetingDetail(MeetingDetailDto),
    /// Reply to `GetAudioOptions` and `SetAudioOptions` (minor 27).
    AudioOptions(AudioOptionsDto),
    /// Reply to `GetVocabulary` and `SetVocabulary` (minor 30).
    Vocabulary(VocabularyDto),
    /// Reply to `GetHoldKey` and `SetHoldKey` (minor 32).
    HoldKey(HoldKeyDto),
    /// Reply to `GetTextOptions` and `SetTextOptions` (minor 35).
    TextOptions(TextOptionsDto),
    /// Reply to `GetMeetingOptions` and `SetMeetingOptions` (minor 39).
    MeetingOptions(MeetingOptionsDto),
    /// Reply to `AssistantLibrary` (minor 44): JSON.
    AssistantLibrary(String),
    /// Reply to `GetMeetingSlide` (minor 45): base64 PNG.
    MeetingSlide(String),
    RecoveryEntry(RecoveryEntryDetailDto),
    CapabilityCheck(CapabilityCheckResultDto),
    /// Typed result of one atomic [`Command::ToggleSession`] (minor 7).
    Toggle(ToggleResultDto),
    PortalKeyboardPermission(PortalKeyboardStatusDto),
    OfflineModelProvision(ModelProvisionStatusDto),
    Accepted,
    Rejected {
        code: IpcErrorCode,
        detail: String,
    },
}

/// Daemon-pushed events. Session state changes carry transaction identity
/// and a monotonic per-session sequence so consumers can reject stale
/// partial/final/status events (ipc.md "Events").
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    SessionStateChanged {
        #[serde(with = "wire_u128")]
        session_id: u128,
        #[serde(default)]
        transaction_id: Option<String>,
        #[serde(default)]
        pipeline_id: Option<String>,
        state: String,
        seq: u64,
        /// How the text reached its destination; only on `committed`
        /// (minor 18). Content-free.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delivery: Option<String>,
    },
    TranscriptPartial {
        #[serde(with = "wire_u128")]
        session_id: u128,
        transaction_id: String,
        pipeline_id: String,
        seq: u64,
        #[serde(
            serialize_with = "serialize_partial_transcript",
            deserialize_with = "deserialize_partial_transcript"
        )]
        text: String,
    },
    CapabilityChanged {
        capability: String,
        available: bool,
    },
    HealthChanged {
        module_id: String,
        healthy: bool,
    },
    PortalKeyboardPermissionChanged {
        status: PortalKeyboardStatusDto,
        seq: u64,
    },
    /// A piece of the assistant's reply to `turn_id` (minor 19), in order.
    AssistantDelta {
        turn_id: String,
        text: String,
    },
    /// The reply to `turn_id` ended (minor 19): `outcome` is `ok`,
    /// `cancelled`, or `failed:<code>`; `text` is the whole reply.
    AssistantDone {
        turn_id: String,
        outcome: String,
        text: String,
        /// A writing action's reply that failed a check (minor 24):
        /// `not_faithful` (a Fix changed too much) or `lost_details`
        /// (a number, link or address of the original is missing).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        warning: Option<String>,
    },
    /// A meeting started, stopped, moved on while transcribing, finished or
    /// failed (minor 25). Content-free: no title or transcript.
    MeetingChanged {
        id: String,
        state: String,
        progress: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stage: Option<String>,
    },
    /// Voice mode (minor 44). `kind`: `state` (`state` is `starting`,
    /// `listening`, `hearing`, `thinking`, `speaking`, `muted` or
    /// `stopped`), `partial` and `heard` (`text`, `language`: the words
    /// heard, live and final), `interrupt`, `repeat`, `timing`, `error`
    /// (`code`).
    Voice {
        kind: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        state: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        language: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timing: Option<VoiceTimingDto>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<String>,
    },
    /// A tool call of the assistant's reply to `turn_id` (minor 46).
    /// `state`: `ask` (waiting for the user's decision), `running`, `done`,
    /// `failed` or `refused`. `arguments` is the call's JSON; `summary` says
    /// what came back, shortly.
    AssistantTool {
        turn_id: String,
        call_id: String,
        server: String,
        tool: String,
        arguments: String,
        state: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        /// What the step found, as JSON for the activity panel (minor 47):
        /// search results, a page read, files, a command's output.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    /// Something in the assistant's library changed without a client
    /// asking (minor 44): a routine ran, or a reply was saved into a chat.
    /// Content-free.
    AssistantLibraryChanged {
        kind: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    /// How loud the microphone is while listening (minor 21), about 20 times
    /// a second: `level` 0..=1000, a perceptual scale (0 silence, 1000 very
    /// loud). Content-free.
    AudioLevel {
        #[serde(with = "wire_u128")]
        session_id: u128,
        level: u16,
    },
}

/// Voice-mode settings (minor 44); each is changed only when given.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceSettingsDto {
    /// Speaking speed in percent, 100..=175.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed_percent: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push_to_talk: Option<bool>,
    /// `stop` (speaking interrupts the reply) or `queue` (it is kept for
    /// after the reply).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interrupt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sounds: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub muted: Option<bool>,
    /// The English voice's number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<u16>,
}

/// Where one spoken turn's time went (minor 44), in milliseconds: the
/// final transcription, the model's first words, the first sentence's
/// voice, and from the end of speech to the first sound.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceTimingDto {
    pub asr_ms: u32,
    pub reply_ms: u32,
    pub voice_ms: u32,
    pub total_ms: u32,
}

/// A recorded meeting.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeetingDto {
    pub id: String,
    pub title: String,
    pub started_unix_ms: u64,
    pub duration_ms: u64,
    /// `recording`, `processing`, `ready` or `failed:<code>`.
    pub state: String,
    /// While processing: 0..=1000.
    pub progress: u16,
    /// While processing: `load`, `speakers` or `words`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    /// `turbo` or `large-v3`.
    pub model: String,
    pub speakers: u16,
    /// Recorded with "Watch with me" (minor 47).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub watch: bool,
    /// Recording or processing is paused (minor 48).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub paused: bool,
    /// Started as a live screen share (minor 49).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub share: bool,
    /// While recording: the screen and the sound are kept now (minor 50).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub screen_on: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sound_on: bool,
    /// While recording: the microphone is kept now (minor 51).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mic_on: bool,
    /// Kept as heard when it stopped: the live transcript and the screens,
    /// not yet gone over (minor 52).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub raw: bool,
}

impl std::fmt::Debug for MeetingDto {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MeetingDto")
            .field("id", &self.id)
            .field("state", &self.state)
            .field("progress", &self.progress)
            .finish_non_exhaustive()
    }
}

/// One turn of a meeting transcript.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeetingTurnDto {
    /// Label such as `Room 1` (heard by the microphone) or `Call 2` (heard
    /// through the computer).
    pub speaker: String,
    /// `room` or `call`.
    pub source: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    /// Language code Whisper found, such as `en` or `ur`.
    pub language: String,
    /// While recording: the words being said now, transcribed again as
    /// they go on until the piece ends (minor 53).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub partial: bool,
}

impl std::fmt::Debug for MeetingTurnDto {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MeetingTurnDto")
            .field("speaker", &self.speaker)
            .field("start_ms", &self.start_ms)
            .finish_non_exhaustive()
    }
}

/// Meeting options.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeetingOptionsDto {
    /// Write the detailed notes as soon as the transcript is ready.
    pub auto_notes: bool,
    /// Whether the call's sound is recorded as well (minor 43; older
    /// services always record it).
    #[serde(default = "call_audio_default")]
    pub call_audio: bool,
    /// Whether the microphone is recorded (minor 43; older services
    /// always record it).
    #[serde(default = "call_audio_default")]
    pub room_audio: bool,
    /// The chosen microphone, `""` following the Microphone stage (minor 43).
    #[serde(default)]
    pub microphone: String,
    /// The chosen output, `""` the system's default (minor 43).
    #[serde(default)]
    pub speaker: String,
    /// The microphones to choose from (minor 43).
    #[serde(default)]
    pub microphones: Vec<MicrophoneDto>,
    /// The outputs to choose from (minor 43).
    #[serde(default)]
    pub speakers: Vec<MicrophoneDto>,
    /// The microphone recording now, or that a meeting started now would
    /// record (minor 43).
    #[serde(default)]
    pub microphone_in_use: String,
    /// The output whose sound is recorded now, or would be (minor 43).
    #[serde(default)]
    pub speaker_in_use: String,
    /// Keep pictures of what is shown on screen (minor 45).
    #[serde(default)]
    pub screen: bool,
    /// Whether this computer can (the screen-sharing portal and GStreamer's
    /// PipeWire plugin are there) (minor 45).
    #[serde(default)]
    pub screen_available: bool,
    /// Go over a recording as soon as it stops; off keeps it as heard until
    /// asked (minor 52; older services always go over it).
    #[serde(default = "call_audio_default")]
    pub process_on_stop: bool,
}

/// One picture of the screen kept during a meeting (minor 45).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeetingSlideDto {
    /// The picture's name in the meeting, for `GetMeetingSlide`.
    pub file: String,
    pub at_ms: u64,
    /// What the model saw on it; empty until described.
    #[serde(default)]
    pub description: String,
}

const fn call_audio_default() -> bool {
    true
}

/// How the Clean pipeline writes its final text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextOptionsDto {
    /// "twenty five" written as 25 (zero to nine stay words).
    pub numbers_as_digits: bool,
    /// `paste` (clipboard, then put back) or `type` (minor 38).
    #[serde(default)]
    pub delivery_method: String,
    /// Minutes of no use before the clean-up model is unloaded; 0 never
    /// (minor 40).
    #[serde(default)]
    pub cleanup_idle_minutes: u32,
    /// "new line" / "new paragraph" become line breaks on Clean (minor 42).
    #[serde(default)]
    pub spoken_breaks: bool,
    /// The language a start without its own is dictated in; `None` is
    /// automatic (minor 54).
    #[serde(default)]
    pub dictation_language: Option<String>,
    /// The user's languages to choose from (minor 54).
    #[serde(default)]
    pub dictation_languages: Vec<String>,
}

/// Press and hold: the chosen key and whether it can be read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HoldKeyDto {
    /// The chosen Linux key code; `None` is off.
    #[serde(default)]
    pub key: Option<u16>,
    /// `off`, `starting`, `reading` (works), `denied` (the user is not in
    /// the `input` group) or `unavailable`.
    pub state: String,
    /// The keys that can be chosen.
    pub keys: Vec<HoldKeyChoiceDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HoldKeyChoiceDto {
    pub code: u16,
    pub name: String,
}

/// One pipeline's own Voice settings; `None` follows the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineVoiceDto {
    #[serde(default)]
    pub echo_cancel: Option<bool>,
    #[serde(default)]
    pub only_my_voice: Option<bool>,
}

/// A microphone: its PipeWire name and what to call it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MicrophoneDto {
    pub name: String,
    pub description: String,
}

/// The user's vocabulary: words to recognise, and replacements.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct VocabularyDto {
    #[serde(default)]
    pub words: Vec<String>,
    #[serde(default)]
    pub replacements: Vec<ReplacementDto>,
}

impl std::fmt::Debug for VocabularyDto {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VocabularyDto")
            .field("words", &self.words.len())
            .field("replacements", &self.replacements.len())
            .finish()
    }
}

/// Something said, and what to write instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplacementDto {
    pub from: String,
    pub to: String,
}

/// The audio options.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioOptionsDto {
    /// Remove the computer's own sound (videos, calls on speakers) from the
    /// microphone.
    pub echo_cancel: bool,
    /// Whether that is working now; when on but not working, the plain
    /// microphone is used.
    pub echo_cancel_active: bool,
    /// Whether the voice filter can run (minor 28).
    #[serde(default)]
    pub voice_filter_available: bool,
    /// Whether the user's voice is trained (minor 28).
    #[serde(default)]
    pub voice_trained: bool,
    /// Keep only the user's voice once trained (minor 28).
    #[serde(default)]
    pub only_my_voice: bool,
    /// `idle`, `recording` or `failed:<code>` (minor 28).
    #[serde(default)]
    pub voice_training: String,
    /// Background noise removed with the computer's sound: `off`,
    /// `standard` or `strong` (minor 29).
    #[serde(default)]
    pub noise_suppression: String,
    /// The chosen microphone's name, `""` for the system default (minor 31).
    #[serde(default)]
    pub microphone: String,
    /// The chosen microphone is not plugged in: the default is used until
    /// it is back (minor 31).
    #[serde(default)]
    pub microphone_missing: bool,
    /// The microphones to choose from (minor 31).
    #[serde(default)]
    pub microphones: Vec<MicrophoneDto>,
    /// A short sound when listening starts and stops (minor 33).
    #[serde(default)]
    pub sounds: bool,
    /// How strictly "Only my voice" keeps only the user: `relaxed`,
    /// `normal` or `strict` (minor 41).
    #[serde(default)]
    pub voice_strictness: String,
    /// Pipelines with their own Voice settings (minor 36).
    #[serde(default)]
    pub pipelines: std::collections::BTreeMap<String, PipelineVoiceDto>,
}

/// A meeting with its transcript and speaker names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeetingDetailDto {
    pub meeting: MeetingDto,
    pub turns: Vec<MeetingTurnDto>,
    /// Label to name, for the labels the user named.
    pub names: BTreeMap<String, String>,
    /// Where the recording and `transcript.md` are kept.
    pub folder: String,
    /// The Whisper models installed: `turbo`, `large-v3`.
    pub models: Vec<String>,
    /// Saved notes by kind (`notes`, `email`, `custom`), as Markdown
    /// (minor 26).
    #[serde(default)]
    pub notes: BTreeMap<String, String>,
    /// Whether the full recording (`recording.wav`) is kept (minor 26).
    #[serde(default)]
    pub recording: bool,
    /// What was shown on screen (minor 45).
    #[serde(default)]
    pub slides: Vec<MeetingSlideDto>,
    /// Why the screen could not be kept, when it was asked for (minor 45).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screen_error: Option<String>,
    /// The screens and windows being watched (minor 47); each one's live
    /// picture is `GetMeetingSlide` with `live-<source>.png`.
    #[serde(default)]
    pub screen_sources: Vec<ScreenSourceDto>,
}

/// A screen or window shared with a meeting (minor 47).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenSourceDto {
    pub source: u16,
    /// `monitor`, `window` or `screen`.
    pub kind: String,
    pub width: u32,
    pub height: u32,
}

/// One problem the writing checker found. `start`/`end` are UTF-16 offsets
/// into the checked text.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WritingFindingDto {
    pub start: u32,
    pub end: u32,
    /// Category: `Spelling`, `Grammar`, `Punctuation`, `Style`, ...
    pub kind: String,
    pub rule: String,
    pub message: String,
    pub suggestions: Vec<WritingFixDto>,
    /// Lower is more important.
    pub priority: u8,
}

impl std::fmt::Debug for WritingFindingDto {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WritingFindingDto")
            .field("start", &self.start)
            .field("end", &self.end)
            .field("kind", &self.kind)
            .field("rule", &self.rule)
            .finish_non_exhaustive()
    }
}

/// One way to resolve a finding: `op` is `replace`, `insert_after` or
/// `remove`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WritingFixDto {
    pub op: String,
    #[serde(default)]
    pub text: String,
}

impl std::fmt::Debug for WritingFixDto {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WritingFixDto")
            .field("op", &self.op)
            .finish_non_exhaustive()
    }
}

/// A rule of the writing checker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WritingRuleDto {
    pub name: String,
    pub description: String,
    pub enabled_by_default: bool,
}

/// A language model on this computer (reply to `ListLanguageModels`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanguageModelDto {
    pub id: String,
    /// As its licence requires it to be named.
    pub name: String,
    pub publisher: String,
    pub size_bytes: u64,
    pub license: String,
    /// A licence limit or naming rule the user should see.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license_notice: Option<String>,
    /// What it can do: `clean_up`, `assistant`.
    pub roles: Vec<String>,
    pub languages: Vec<String>,
    /// Roles it is recommended for.
    #[serde(default)]
    pub recommended_for: Vec<String>,
    /// Roles it is selected for now.
    #[serde(default)]
    pub in_use_for: Vec<String>,
    /// Clean-up only: `loading`, `ready:<device>` or `failed:<code>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clean_up_state: Option<String>,
    /// Assistant only (minor 22): `loaded:<device>` or `unloaded`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assistant_state: Option<String>,
}

/// One message of an assistant conversation.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssistantMessageDto {
    /// `system`, `user` or `assistant`.
    pub role: String,
    pub content: String,
}

impl std::fmt::Debug for AssistantMessageDto {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AssistantMessageDto")
            .field("role", &self.role)
            .field("content", &"<redacted>")
            .finish()
    }
}

fn deserialize_turn_id<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(serde::de::Error::custom(
            "turn_id must be 1..=64 of [A-Za-z0-9_-]",
        ));
    }
    Ok(value)
}

fn deserialize_meeting_id<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(serde::de::Error::custom(
            "meeting id must be 1..=64 of [A-Za-z0-9_-]",
        ));
    }
    Ok(value)
}

fn deserialize_meeting_text<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    let value = Option::<String>::deserialize(deserializer)?;
    if value.as_ref().is_some_and(|text| text.len() > 200) {
        return Err(serde::de::Error::custom("meeting title is too long"));
    }
    Ok(value)
}

fn deserialize_meeting_name<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    if value.len() > 80 {
        return Err(serde::de::Error::custom("speaker name is too long"));
    }
    Ok(value)
}

fn deserialize_writing_text<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    if value.len() > MAX_WRITING_TEXT_BYTES {
        return Err(serde::de::Error::custom("writing text is too long"));
    }
    Ok(value)
}

fn deserialize_writing_instruction<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    let value = Option::<String>::deserialize(deserializer)?;
    if value
        .as_ref()
        .is_some_and(|text| text.len() > MAX_WRITING_INSTRUCTION_BYTES)
    {
        return Err(serde::de::Error::custom("writing instruction is too long"));
    }
    Ok(value)
}

fn deserialize_writing_rules<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, bool>, D::Error> {
    let value = BTreeMap::<String, bool>::deserialize(deserializer)?;
    if value.len() > MAX_WRITING_RULES || value.keys().any(|name| name.len() > 128) {
        return Err(serde::de::Error::custom("too many writing rules"));
    }
    Ok(value)
}

fn deserialize_writing_words<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<String>, D::Error> {
    let value = Vec::<String>::deserialize(deserializer)?;
    if value.len() > MAX_WRITING_WORDS || value.iter().any(|word| word.len() > 64) {
        return Err(serde::de::Error::custom("too many dictionary words"));
    }
    Ok(value)
}

fn deserialize_assistant_messages<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<AssistantMessageDto>, D::Error> {
    let messages = Vec::<AssistantMessageDto>::deserialize(deserializer)?;
    if messages.is_empty() || messages.len() > MAX_ASSISTANT_MESSAGES {
        return Err(serde::de::Error::custom("1..=64 assistant messages"));
    }
    let mut total = 0_usize;
    for message in &messages {
        if !matches!(message.role.as_str(), "system" | "user" | "assistant") {
            return Err(serde::de::Error::custom("unknown assistant role"));
        }
        if message.content.len() > MAX_ASSISTANT_MESSAGE_BYTES {
            return Err(serde::de::Error::custom("assistant message too long"));
        }
        total += message.content.len();
    }
    if total > MAX_ASSISTANT_REQUEST_BYTES {
        return Err(serde::de::Error::custom("assistant conversation too long"));
    }
    Ok(messages)
}

fn validate_partial_transcript(value: &str) -> Result<(), &'static str> {
    if value.is_empty() || value.len() > MAX_PARTIAL_TRANSCRIPT_BYTES {
        return Err("partial transcript must be 1..=16384 UTF-8 bytes");
    }
    Ok(())
}

fn serialize_partial_transcript<S: Serializer>(
    value: &str,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    validate_partial_transcript(value).map_err(serde::ser::Error::custom)?;
    serializer.serialize_str(value)
}

fn deserialize_partial_transcript<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    validate_partial_transcript(&value).map_err(serde::de::Error::custom)?;
    Ok(value)
}

impl std::fmt::Debug for Event {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SessionStateChanged {
                session_id,
                transaction_id,
                pipeline_id,
                state,
                seq,
                delivery,
            } => formatter
                .debug_struct("SessionStateChanged")
                .field("session_id", session_id)
                .field("transaction_id", transaction_id)
                .field("pipeline_id", pipeline_id)
                .field("state", state)
                .field("seq", seq)
                .field("delivery", delivery)
                .finish(),
            Self::TranscriptPartial {
                session_id,
                transaction_id,
                pipeline_id,
                seq,
                ..
            } => formatter
                .debug_struct("TranscriptPartial")
                .field("session_id", session_id)
                .field("transaction_id", transaction_id)
                .field("pipeline_id", pipeline_id)
                .field("seq", seq)
                .field("text", &"<redacted>")
                .finish(),
            Self::CapabilityChanged {
                capability,
                available,
            } => formatter
                .debug_struct("CapabilityChanged")
                .field("capability", capability)
                .field("available", available)
                .finish(),
            Self::AssistantDelta { turn_id, .. } => formatter
                .debug_struct("AssistantDelta")
                .field("turn_id", turn_id)
                .field("text", &"<redacted>")
                .finish(),
            Self::MeetingChanged {
                id,
                state,
                progress,
                stage,
            } => formatter
                .debug_struct("MeetingChanged")
                .field("id", id)
                .field("state", state)
                .field("progress", progress)
                .field("stage", stage)
                .finish(),
            Self::Voice {
                kind,
                state,
                language,
                timing,
                code,
                ..
            } => formatter
                .debug_struct("Voice")
                .field("kind", kind)
                .field("state", state)
                .field("text", &"<redacted>")
                .field("language", language)
                .field("timing", timing)
                .field("code", code)
                .finish(),
            Self::AssistantTool {
                turn_id,
                call_id,
                server,
                tool,
                state,
                ..
            } => formatter
                .debug_struct("AssistantTool")
                .field("turn_id", turn_id)
                .field("call_id", call_id)
                .field("server", server)
                .field("tool", tool)
                .field("arguments", &"<redacted>")
                .field("state", state)
                .finish(),
            Self::AssistantLibraryChanged { kind, id } => formatter
                .debug_struct("AssistantLibraryChanged")
                .field("kind", kind)
                .field("id", id)
                .finish(),
            Self::AudioLevel { session_id, level } => formatter
                .debug_struct("AudioLevel")
                .field("session_id", session_id)
                .field("level", level)
                .finish(),
            Self::AssistantDone {
                turn_id, outcome, ..
            } => formatter
                .debug_struct("AssistantDone")
                .field("turn_id", turn_id)
                .field("outcome", outcome)
                .field("text", &"<redacted>")
                .finish(),
            Self::HealthChanged { module_id, healthy } => formatter
                .debug_struct("HealthChanged")
                .field("module_id", module_id)
                .field("healthy", healthy)
                .finish(),
            Self::PortalKeyboardPermissionChanged { status, seq } => formatter
                .debug_struct("PortalKeyboardPermissionChanged")
                .field("status", status)
                .field("seq", seq)
                .finish(),
        }
    }
}

/// Versioned, bounded envelope wrapping every request/response/event frame.
///
/// The `payload` is a bounds-checked JSON value; `session_id`,
/// `transaction_id` and `event_seq` are optional per spec (session/transaction
/// identity on session-scoped traffic, monotonic sequence on events).
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    pub version_major: u16,
    pub version_minor: u16,
    pub kind: MessageKind,
    pub request_id: RequestId,
    #[serde(default, with = "wire_u128::option")]
    pub session_id: Option<u128>,
    #[serde(default)]
    pub transaction_id: Option<String>,
    #[serde(default)]
    pub event_seq: Option<u64>,
    #[serde(default)]
    pub payload: serde_json::Value,
}

impl std::fmt::Debug for Envelope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Envelope")
            .field("version_major", &self.version_major)
            .field("version_minor", &self.version_minor)
            .field("kind", &self.kind)
            .field("request_id", &self.request_id)
            .field("session_id", &self.session_id)
            .field("transaction_id", &self.transaction_id)
            .field("event_seq", &self.event_seq)
            .field("payload", &"<redacted>")
            .finish()
    }
}

impl Envelope {
    /// Empty envelope stamped with this build's protocol versions.
    pub fn new(kind: MessageKind, request_id: RequestId) -> Self {
        Self {
            version_major: PROTOCOL_MAJOR,
            version_minor: PROTOCOL_MINOR,
            kind,
            request_id,
            session_id: None,
            transaction_id: None,
            event_seq: None,
            payload: serde_json::Value::Null,
        }
    }

    /// Attach a typed payload after enforcing message-size and
    /// collection-element bounds (ipc.md "Bounds").
    pub fn with_payload<T: Serialize>(mut self, payload: T) -> Result<Self, IpcError> {
        let value = serde_json::to_value(payload)
            .map_err(|e| IpcError::bad_request(format!("payload serialize failed: {e}")))?;
        check_payload_bounds(&value)?;
        self.payload = value;
        Ok(self)
    }

    /// Builder: attach an optional session id.
    pub fn with_session(mut self, session_id: impl Into<u128>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// Builder: attach an optional transaction id.
    pub fn with_transaction(mut self, transaction_id: impl Into<String>) -> Self {
        self.transaction_id = Some(transaction_id.into());
        self
    }

    /// Builder: attach the monotonic event sequence.
    pub fn with_event_seq(mut self, seq: u64) -> Self {
        self.event_seq = Some(seq);
        self
    }

    /// Decode the payload into a typed value.
    pub fn parse_payload<T: serde::de::DeserializeOwned>(&self) -> Result<T, IpcError> {
        serde_json::from_value(self.payload.clone())
            .map_err(|e| IpcError::bad_request(format!("payload decode failed: {e}")))
    }

    /// Structural validation: protocol major match plus payload bounds.
    pub fn validate(&self) -> Result<(), IpcError> {
        if self.version_major != PROTOCOL_MAJOR {
            return Err(IpcError::new(
                IpcErrorCode::IncompatibleProtocol,
                format!(
                    "envelope major {} does not match daemon major {PROTOCOL_MAJOR}",
                    self.version_major
                ),
            ));
        }
        check_payload_bounds(&self.payload)?;
        Ok(())
    }

    /// Serialize the full envelope, enforcing the total byte bound before it
    /// can enter the framing layer.
    pub fn to_json_bytes(&self) -> Result<Vec<u8>, IpcError> {
        let bytes = serde_json::to_vec(self)
            .map_err(|e| IpcError::bad_request(format!("envelope serialize failed: {e}")))?;
        if bytes.len() > MAX_MESSAGE_BYTES {
            return Err(IpcError::new(
                IpcErrorCode::PayloadTooLarge,
                format!(
                    "serialized envelope {} bytes exceeds bound {MAX_MESSAGE_BYTES}",
                    bytes.len()
                ),
            ));
        }
        Ok(bytes)
    }

    /// Decode an envelope from frame bytes, enforcing the byte bound first
    /// so hostile input cannot force unbounded work past the framing layer.
    pub fn from_json_bytes(bytes: &[u8]) -> Result<Self, IpcError> {
        if bytes.len() > MAX_MESSAGE_BYTES {
            return Err(IpcError::new(
                IpcErrorCode::PayloadTooLarge,
                format!(
                    "message {} bytes exceeds bound {MAX_MESSAGE_BYTES}",
                    bytes.len()
                ),
            ));
        }
        let envelope: Self = serde_json::from_slice(bytes)
            .map_err(|e| IpcError::bad_request(format!("envelope decode failed: {e}")))?;
        envelope.validate()?;
        Ok(envelope)
    }
}

/// Enforce payload bounds: total bytes, collection element counts, depth.
fn check_payload_bounds(value: &serde_json::Value) -> Result<(), IpcError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|e| IpcError::bad_request(format!("payload serialize failed: {e}")))?;
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err(IpcError::new(
            IpcErrorCode::PayloadTooLarge,
            format!(
                "payload {} bytes exceeds bound {MAX_MESSAGE_BYTES}",
                bytes.len()
            ),
        ));
    }
    check_collection_depth(value, 0)
}

fn check_collection_depth(value: &serde_json::Value, depth: usize) -> Result<(), IpcError> {
    if depth > MAX_JSON_DEPTH {
        return Err(IpcError::bad_request(format!(
            "payload nesting deeper than {MAX_JSON_DEPTH}"
        )));
    }
    match value {
        serde_json::Value::Array(items) => {
            if items.len() > MAX_COLLECTION_ELEMENTS {
                return Err(IpcError::new(
                    IpcErrorCode::PayloadTooLarge,
                    format!(
                        "collection of {} elements exceeds bound {MAX_COLLECTION_ELEMENTS}",
                        items.len()
                    ),
                ));
            }
            for item in items {
                check_collection_depth(item, depth + 1)?;
            }
        }
        serde_json::Value::Object(map) => {
            if map.len() > MAX_COLLECTION_ELEMENTS {
                return Err(IpcError::new(
                    IpcErrorCode::PayloadTooLarge,
                    format!(
                        "object with {} entries exceeds bound {MAX_COLLECTION_ELEMENTS}",
                        map.len()
                    ),
                ));
            }
            for item in map.values() {
                check_collection_depth(item, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipc_handshake_accepts_matching_versions() -> Result<(), Box<dyn std::error::Error>> {
        let hello = HandshakeHello::new("cli");
        hello.validate()?;
        match negotiate_handshake(&hello) {
            HandshakeResponse::Accepted {
                protocol_minor,
                daemon_build,
            } => {
                assert_eq!(protocol_minor, PROTOCOL_MINOR);
                assert_eq!(daemon_build, BUILD_IDENTITY);
            }
            other => return Err(format!("expected Accepted, got {other:?}").into()),
        }
        Ok(())
    }

    #[test]
    fn ipc_handshake_minor_skew_negotiates_explicitly() -> Result<(), Box<dyn std::error::Error>> {
        let mut hello = HandshakeHello::new("desktop");
        hello.protocol_minor = PROTOCOL_MINOR + 7;
        match negotiate_handshake(&hello) {
            HandshakeResponse::Accepted { protocol_minor, .. } => {
                // Daemon reports its OWN minor so the client gates features explicitly.
                assert_eq!(protocol_minor, PROTOCOL_MINOR);
            }
            other => return Err(format!("minor skew must be tolerated, got {other:?}").into()),
        }
        Ok(())
    }

    #[test]
    fn ipc_handshake_major_skew_rejected_incompatible() -> Result<(), Box<dyn std::error::Error>> {
        let mut hello = HandshakeHello::new("cli");
        hello.protocol_major = PROTOCOL_MAJOR + 1;
        match negotiate_handshake(&hello) {
            HandshakeResponse::Rejected { code, detail } => {
                assert_eq!(code, IpcErrorCode::IncompatibleProtocol);
                assert!(!detail.is_empty());
            }
            other => return Err(format!("major skew must reject, got {other:?}").into()),
        }
        // Skew below 1 rejects too.
        let mut older = HandshakeHello::new("legacy");
        older.protocol_major = 0;
        assert!(matches!(
            negotiate_handshake(&older),
            HandshakeResponse::Rejected { .. }
        ));
        Ok(())
    }

    #[test]
    fn ipc_envelope_rejects_oversize_payload() -> Result<(), Box<dyn std::error::Error>> {
        let envelope = Envelope::new(MessageKind::GetStatus, RequestId(1));
        let err = match envelope.with_payload(vec![b'x'; MAX_MESSAGE_BYTES + 1]) {
            Err(err) => err,
            Ok(built) => return Err(format!("oversize payload accepted: {built:?}").into()),
        };
        assert_eq!(err.code, IpcErrorCode::PayloadTooLarge);
        // Near-bound single string still passes.
        let near_bound = String::from_utf8(vec![b'x'; MAX_MESSAGE_BYTES - 1024])?;
        Envelope::new(MessageKind::GetStatus, RequestId(2)).with_payload(near_bound)?;
        Ok(())
    }

    #[test]
    fn ipc_envelope_rejects_oversize_collection() -> Result<(), Box<dyn std::error::Error>> {
        let too_many: Vec<&str> = std::iter::repeat_n("s", MAX_COLLECTION_ELEMENTS + 1).collect();
        let err =
            match Envelope::new(MessageKind::ListPipelines, RequestId(3)).with_payload(too_many) {
                Err(err) => err,
                Ok(built) => return Err(format!("oversize collection accepted: {built:?}").into()),
            };
        assert_eq!(err.code, IpcErrorCode::PayloadTooLarge);
        // At-bound collection (exactly 10_000 elements, well under byte bound) passes.
        let at_bound = vec![7u16; MAX_COLLECTION_ELEMENTS];
        Envelope::new(MessageKind::ListPipelines, RequestId(4)).with_payload(at_bound)?;
        Ok(())
    }

    #[test]
    fn ipc_envelope_parse_payload_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
        let status = StatusDto {
            daemon_build: "narrivyd 0.0.0".into(),
            protocol_minor: PROTOCOL_MINOR,
            active_session: Some(ActiveSessionDto {
                session_id: 42,
                transaction_id: Some("tx-1".into()),
                pipeline_id: Some("default".into()),
                state: "listening".into(),
                stage_timings_us: None,
            }),
            last_session_timings_us: None,
            capabilities: vec!["asr.local".into()],
            installed_model: Some(Box::new(InstalledModelLifecycleDto::Ready)),
            portal_keyboard: None,
            model_provisioning: None,
        };
        let envelope = Envelope::new(MessageKind::Response, RequestId(9)).with_payload(&status)?;
        let decoded: StatusDto = envelope.parse_payload()?;
        assert_eq!(decoded, status);
        // Wrong target type surfaces as BadRequest, not a panic.
        let wrong: Result<Vec<u8>, IpcError> = envelope.parse_payload();
        let err = match wrong {
            Err(err) => err,
            Ok(value) => return Err(format!("type mismatch decoded {value:?}").into()),
        };
        assert_eq!(err.code, IpcErrorCode::BadRequest);
        Ok(())
    }

    #[test]
    fn ipc_message_kind_unknown_passthrough_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
        let future_tag = "\"request.transcribe_stream_v2\"";
        let decoded: MessageKind = serde_json::from_str(future_tag)?;
        assert_eq!(
            decoded,
            MessageKind::Other("request.transcribe_stream_v2".into())
        );
        assert!(!decoded.is_request());
        let encoded = serde_json::to_string(&decoded)?;
        assert_eq!(encoded, future_tag);
        // Known kinds keep their exact stable strings.
        let known: MessageKind = serde_json::from_str("\"request.get_status\"")?;
        assert_eq!(known.wire_tag(), "request.get_status");
        assert!(known.is_request());
        let event_kind: MessageKind = serde_json::from_str("\"event\"")?;
        assert_eq!(event_kind.wire_tag(), "event");
        assert!(!event_kind.is_request());
        Ok(())
    }

    #[test]
    fn ipc_command_and_event_wire_tags_stable() -> Result<(), Box<dyn std::error::Error>> {
        let cmd = Command::StartSession {
            pipeline_id: Some("default".into()),
            activation: "hotkey".into(),
            recognition: None,
        };
        let json = serde_json::to_value(&cmd)?;
        assert_eq!(json["command"], "start_session");
        assert_eq!(json["activation"], "hotkey");
        assert_eq!(cmd.kind().wire_tag(), "request.start_session");

        let recovery = Command::GetRecoveryEntry {
            txid: "tx-1".into(),
        };
        let recovery_json = serde_json::to_value(&recovery)?;
        assert_eq!(recovery_json["command"], "get_recovery_entry");
        assert_eq!(recovery.kind().wire_tag(), "request.get_recovery_entry");
        let recovery_response = ResponsePayload::RecoveryEntry(HistoryEntryDetailDto {
            txid: "tx-1".into(),
            state: "delivered".into(),
            raw: None,
            corrected: None,
            delivered: Some("private fixture".into()),
            pipeline_id: Some("default".into()),
            capture_end: Some(CaptureEndDto::SafetyLimit),
        });
        let recovery_response_json = serde_json::to_value(&recovery_response)?;
        assert_eq!(recovery_response_json["result"], "recovery_entry");
        assert_eq!(
            recovery_response_json["payload"]["capture_end"],
            "safety_limit"
        );
        assert_eq!(
            serde_json::from_value::<ResponsePayload>(recovery_response_json)?,
            recovery_response
        );
        // An older payload without the additive field stays fully readable and
        // decodes to "capture end not reported".
        let legacy = serde_json::json!({
            "result": "recovery_entry",
            "payload": {
                "txid": "tx-1",
                "state": "delivered",
                "raw": null,
                "corrected": null,
                "delivered": null,
                "pipeline_id": null
            }
        });
        match serde_json::from_value::<ResponsePayload>(legacy)? {
            ResponsePayload::RecoveryEntry(entry) => assert_eq!(entry.capture_end, None),
            other => {
                return Err(std::io::Error::other(format!(
                    "legacy recovery payload decoded as {other:?}"
                ))
                .into());
            }
        }
        let recovery_debug = format!("{recovery_response:?}");
        assert!(!recovery_debug.contains("private fixture"));
        assert!(recovery_debug.contains("<redacted>"));

        let event = Event::SessionStateChanged {
            session_id: 42,
            transaction_id: Some("tx-1".into()),
            pipeline_id: Some("default".into()),
            state: "listening".into(),
            seq: 5,
            delivery: None,
        };
        let event_json = serde_json::to_value(&event)?;
        assert_eq!(event_json["event"], "session_state_changed");
        assert_eq!(event_json["seq"], 5);
        assert!(event_json.get("delivery").is_none());
        let back: Event = serde_json::from_value(event_json)?;
        assert_eq!(back, event);

        let committed = Event::SessionStateChanged {
            session_id: 42,
            transaction_id: Some("tx-1".into()),
            pipeline_id: Some("default".into()),
            state: "committed".into(),
            seq: 9,
            delivery: Some("pasted".into()),
        };
        let committed_json = serde_json::to_value(&committed)?;
        assert_eq!(committed_json["delivery"], "pasted");
        let back: Event = serde_json::from_value(committed_json)?;
        assert_eq!(back, committed);

        let secret = "volatile words must not enter debug output";
        let partial = Event::TranscriptPartial {
            session_id: 42,
            transaction_id: "tx-1".into(),
            pipeline_id: "streaming".into(),
            seq: 6,
            text: secret.into(),
        };
        let partial_json = serde_json::to_value(&partial)?;
        assert_eq!(partial_json["event"], "transcript_partial");
        assert_eq!(partial_json["text"], secret);
        assert_eq!(serde_json::from_value::<Event>(partial_json)?, partial);
        let debug = format!("{partial:?}");
        assert!(!debug.contains(secret));
        assert!(debug.contains("<redacted>"));

        let rejected_json = serde_json::to_value(ResponsePayload::Rejected {
            code: IpcErrorCode::UnknownCommand,
            detail: "no such command".into(),
        })?;
        assert_eq!(rejected_json["result"], "rejected");
        assert_eq!(rejected_json["payload"]["code"], "unknown_command");
        let pipelines_json =
            serde_json::to_value(ResponsePayload::Pipelines(vec![PipelineSummaryDto {
                id: "default".into(),
                name: "Default".into(),
                version: 1,
                enabled: true,
                model_id: None,
                compute: None,
            }]))?;
        assert_eq!(pipelines_json["result"], "pipelines");
        assert!(pipelines_json["payload"].is_array());

        let request = Command::RequestPortalKeyboardPermission {
            operation_id: "4b8606be-6713-4dd2-86c5-33943a996971".into(),
            parent_window: "opaque:narrivy-parent".into(),
        };
        let request_json = serde_json::to_value(&request)?;
        assert_eq!(
            request.kind().wire_tag(),
            "request.request_portal_keyboard_permission"
        );
        assert_eq!(
            request_json["command"],
            "request_portal_keyboard_permission"
        );
        assert_eq!(
            request_json["operation_id"],
            "4b8606be-6713-4dd2-86c5-33943a996971"
        );
        assert_eq!(request_json["parent_window"], "opaque:narrivy-parent");

        let cancel = Command::CancelPortalKeyboardPermission {
            operation_id: "4b8606be-6713-4dd2-86c5-33943a996971".into(),
        };
        assert_eq!(
            cancel.kind().wire_tag(),
            "request.cancel_portal_keyboard_permission"
        );
        assert_eq!(
            serde_json::to_value(&cancel)?["command"],
            "cancel_portal_keyboard_permission"
        );
        assert_eq!(
            Command::RevokePortalKeyboardPermission.kind().wire_tag(),
            "request.revoke_portal_keyboard_permission"
        );
        assert_eq!(
            serde_json::to_value(Command::RevokePortalKeyboardPermission)?["command"],
            "revoke_portal_keyboard_permission"
        );
        Ok(())
    }

    #[test]
    fn transcript_partial_rejects_empty_and_oversized_text() {
        let fixture = |text: String| {
            serde_json::json!({
                "event": "transcript_partial",
                "session_id": "42",
                "transaction_id": "tx-1",
                "pipeline_id": "streaming",
                "seq": 1,
                "text": text,
            })
        };
        assert!(serde_json::from_value::<Event>(fixture(String::new())).is_err());
        assert!(
            serde_json::from_value::<Event>(fixture("x".repeat(MAX_PARTIAL_TRANSCRIPT_BYTES + 1)))
                .is_err()
        );
        assert!(
            serde_json::from_value::<Event>(fixture("x".repeat(MAX_PARTIAL_TRANSCRIPT_BYTES)))
                .is_ok()
        );
    }

    fn portal_status_fixture() -> PortalKeyboardStatusDto {
        PortalKeyboardStatusDto {
            eligibility: PortalEligibilityDto::ReadyToRequest,
            lifecycle: PortalLifecycleDto::Active,
            can_request: false,
            permission_active: true,
            transport_connected: true,
            sender_available: false,
            delivery_qualified: false,
            current_operation_id: Some("4b8606be-6713-4dd2-86c5-33943a996971".into()),
            last_failure: None,
            cleanup: PortalCleanupDto::NotRequired,
            sequence: 7,
        }
    }

    #[test]
    fn ipc_portal_status_response_and_event_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
        let status = portal_status_fixture();
        let response = ResponsePayload::PortalKeyboardPermission(status.clone());
        let response_json = serde_json::to_value(&response)?;
        assert_eq!(response_json["result"], "portal_keyboard_permission");
        assert_eq!(response_json["payload"]["lifecycle"], "active");
        assert_eq!(response_json["payload"]["sender_available"], false);
        assert_eq!(response_json["payload"]["delivery_qualified"], false);
        assert_eq!(
            serde_json::from_value::<ResponsePayload>(response_json)?,
            response
        );

        let event = Event::PortalKeyboardPermissionChanged { status, seq: 8 };
        let event_json = serde_json::to_value(&event)?;
        assert_eq!(event_json["event"], "portal_keyboard_permission_changed");
        assert_eq!(event_json["seq"], 8);
        assert_eq!(serde_json::from_value::<Event>(event_json)?, event);
        Ok(())
    }

    #[test]
    fn ipc_portal_status_enums_preserve_future_wire_tags() -> Result<(), Box<dyn std::error::Error>>
    {
        let eligibility_wire = "\"future_eligibility_v9\"";
        let eligibility: PortalEligibilityDto = serde_json::from_str(eligibility_wire)?;
        assert_eq!(
            eligibility,
            PortalEligibilityDto::Unknown("future_eligibility_v9".into())
        );
        assert_eq!(serde_json::to_string(&eligibility)?, eligibility_wire);
        assert_eq!(
            PortalEligibilityDto::ReadyToRequest.wire_tag(),
            "ready_to_request"
        );

        let lifecycle_wire = "\"suspended_by_compositor_v9\"";
        let lifecycle: PortalLifecycleDto = serde_json::from_str(lifecycle_wire)?;
        assert_eq!(
            lifecycle,
            PortalLifecycleDto::Unknown("suspended_by_compositor_v9".into())
        );
        assert_eq!(serde_json::to_string(&lifecycle)?, lifecycle_wire);
        assert_eq!(PortalLifecycleDto::Active.wire_tag(), "active");

        let failure_wire = "\"future_transport_failure_v9\"";
        let failure: PortalFailureDto = serde_json::from_str(failure_wire)?;
        assert_eq!(
            failure,
            PortalFailureDto::Unknown("future_transport_failure_v9".into())
        );
        assert_eq!(serde_json::to_string(&failure)?, failure_wire);
        assert_eq!(
            PortalFailureDto::PermissionDenied.wire_tag(),
            "permission_denied"
        );

        let cleanup_wire = "\"remote_cleanup_pending_v9\"";
        let cleanup: PortalCleanupDto = serde_json::from_str(cleanup_wire)?;
        assert_eq!(
            cleanup,
            PortalCleanupDto::Unknown("remote_cleanup_pending_v9".into())
        );
        assert_eq!(serde_json::to_string(&cleanup)?, cleanup_wire);
        assert_eq!(
            PortalCleanupDto::ConfirmedClosed.wire_tag(),
            "confirmed_closed"
        );
        Ok(())
    }

    #[test]
    fn ipc_debug_redacts_portal_command_and_envelope_payload_canaries()
    -> Result<(), Box<dyn std::error::Error>> {
        const OPERATION_CANARY: &str = "OPERATION_CANARY_6ce64f64";
        const PARENT_CANARY: &str = "opaque:PARENT_CANARY_71385c93";
        const PAYLOAD_CANARY: &str = "ENVELOPE_PAYLOAD_CANARY_79f1582b";

        let request = Command::RequestPortalKeyboardPermission {
            operation_id: OPERATION_CANARY.into(),
            parent_window: PARENT_CANARY.into(),
        };
        let request_debug = format!("{request:?}");
        assert!(!request_debug.contains(OPERATION_CANARY));
        assert!(!request_debug.contains(PARENT_CANARY));
        assert!(request_debug.contains("<redacted>"));

        let cancel = Command::CancelPortalKeyboardPermission {
            operation_id: OPERATION_CANARY.into(),
        };
        let cancel_debug = format!("{cancel:?}");
        assert!(!cancel_debug.contains(OPERATION_CANARY));
        assert!(cancel_debug.contains("<redacted>"));

        let envelope = Envelope::new(MessageKind::Response, RequestId(42))
            .with_payload(serde_json::json!({"secret": PAYLOAD_CANARY}))?;
        let envelope_debug = format!("{envelope:?}");
        assert!(!envelope_debug.contains(PAYLOAD_CANARY));
        assert!(envelope_debug.contains("payload: \"<redacted>\""));
        Ok(())
    }

    #[test]
    fn ipc_status_optional_fields_are_minor_compatible() -> Result<(), Box<dyn std::error::Error>> {
        let legacy = serde_json::json!({
            "daemon_build": "legacy-daemon",
            "protocol_minor": 1,
            "active_session": null,
            "capabilities": []
        });
        let decoded: StatusDto = serde_json::from_value(legacy)?;
        assert_eq!(decoded.protocol_minor, 1);
        assert_eq!(decoded.last_session_timings_us, None);
        assert_eq!(decoded.installed_model, None);
        assert_eq!(decoded.portal_keyboard, None);
        assert_eq!(decoded.model_provisioning, None);

        let current = StatusDto {
            daemon_build: "current-daemon".into(),
            protocol_minor: PROTOCOL_MINOR,
            active_session: None,
            last_session_timings_us: Some(StageTimingsUs(BTreeMap::from([
                ("capture_start".into(), 10),
                ("capture_ready".into(), 20),
            ]))),
            capabilities: vec![],
            installed_model: Some(Box::new(InstalledModelLifecycleDto::AwaitingModel)),
            portal_keyboard: Some(portal_status_fixture()),
            model_provisioning: None,
        };
        let current_json = serde_json::to_value(&current)?;
        assert_eq!(
            current_json["portal_keyboard"]["eligibility"],
            "ready_to_request"
        );
        assert_eq!(current_json["last_session_timings_us"]["capture_ready"], 20);
        assert_eq!(serde_json::from_value::<StatusDto>(current_json)?, current);

        let future = serde_json::json!({
            "daemon_build": "future-daemon",
            "protocol_minor": PROTOCOL_MINOR,
            "active_session": null,
            "capabilities": [],
            "installed_model": "future_state"
        });
        assert_eq!(
            serde_json::from_value::<StatusDto>(future)?.installed_model,
            Some(Box::new(InstalledModelLifecycleDto::Unknown(
                "future_state".to_owned(),
            )))
        );

        let absent = StatusDto {
            last_session_timings_us: None,
            installed_model: None,
            portal_keyboard: None,
            model_provisioning: None,
            ..current
        };
        assert!(
            serde_json::to_value(absent)?
                .get("portal_keyboard")
                .is_none()
        );
        Ok(())
    }

    #[test]
    fn ipc_portal_command_fields_are_strictly_bounded() -> Result<(), Box<dyn std::error::Error>> {
        let max_operation_id = "a".repeat(MAX_PORTAL_OPERATION_ID_BYTES);
        let max_parent = "p".repeat(MAX_PORTAL_PARENT_WINDOW_BYTES);
        let at_bounds = Command::RequestPortalKeyboardPermission {
            operation_id: max_operation_id,
            parent_window: max_parent,
        };
        let encoded = serde_json::to_value(&at_bounds)?;
        assert_eq!(serde_json::from_value::<Command>(encoded)?, at_bounds);

        for invalid_operation_id in [
            String::new(),
            "a".repeat(MAX_PORTAL_OPERATION_ID_BYTES + 1),
            "contains/slash".into(),
            "contains space".into(),
        ] {
            let invalid = Command::CancelPortalKeyboardPermission {
                operation_id: invalid_operation_id,
            };
            assert!(serde_json::to_value(invalid).is_err());
        }

        for invalid_parent in [
            String::new(),
            "p".repeat(MAX_PORTAL_PARENT_WINDOW_BYTES + 1),
            "opaque:parent\nforged".into(),
        ] {
            let invalid = Command::RequestPortalKeyboardPermission {
                operation_id: "valid-operation".into(),
                parent_window: invalid_parent,
            };
            assert!(serde_json::to_value(invalid).is_err());
        }

        let invalid_inbound = serde_json::json!({
            "command": "request_portal_keyboard_permission",
            "operation_id": "invalid operation",
            "parent_window": "opaque:parent"
        });
        assert!(serde_json::from_value::<Command>(invalid_inbound).is_err());
        let invalid_parent_inbound = serde_json::json!({
            "command": "request_portal_keyboard_permission",
            "operation_id": "valid-operation",
            "parent_window": "opaque:1\u{0}suffix"
        });
        assert!(serde_json::from_value::<Command>(invalid_parent_inbound).is_err());

        let mut invalid_status = portal_status_fixture();
        invalid_status.current_operation_id = Some("invalid operation".into());
        assert!(serde_json::to_value(invalid_status).is_err());
        Ok(())
    }

    #[test]
    fn ipc_envelope_version_skew_detected_on_decode() -> Result<(), Box<dyn std::error::Error>> {
        let mut envelope = Envelope::new(MessageKind::GetStatus, RequestId(11));
        envelope.version_major = 99;
        let bytes = serde_json::to_vec(&envelope)?;
        let err = match Envelope::from_json_bytes(&bytes) {
            Err(err) => err,
            Ok(decoded) => return Err(format!("major skew decoded: {decoded:?}").into()),
        };
        assert_eq!(err.code, IpcErrorCode::IncompatibleProtocol);
        Ok(())
    }

    #[test]
    fn ipc_is_request_covers_all_request_kinds_including_recovery_entry()
    -> Result<(), Box<dyn std::error::Error>> {
        // Ensure is_request() stays truthful as new request kinds are added.
        // Optional History and exact RecoveryEntry are distinct request kinds;
        // Response/Event/unknown stay non-requests.
        let request_kinds = [
            MessageKind::GetStatus,
            MessageKind::StartSession,
            MessageKind::StopSession,
            MessageKind::ToggleSession,
            MessageKind::CancelSession,
            MessageKind::Subscribe,
            MessageKind::Unsubscribe,
            MessageKind::ListPipelines,
            MessageKind::SelectPipeline,
            MessageKind::GetHistory,
            MessageKind::GetHistoryEntry,
            MessageKind::DeleteHistoryEntry,
            MessageKind::ListShortcuts,
            MessageKind::SetShortcut,
            MessageKind::RemoveShortcut,
            MessageKind::ListModels,
            MessageKind::SelectModel,
            MessageKind::SetPipelineEngine,
            MessageKind::DownloadModel,
            MessageKind::CancelModelDownload,
            MessageKind::ListStore,
            MessageKind::RemoveStoreItem,
            MessageKind::AssistantSend,
            MessageKind::AssistantCancel,
            MessageKind::ListLanguageModels,
            MessageKind::SelectLanguageModel,
            MessageKind::AssistantUnload,
            MessageKind::CopyHistoryEntry,
            MessageKind::CheckWriting,
            MessageKind::ListWritingRules,
            MessageKind::WritingAction,
            MessageKind::MeetingStart,
            MessageKind::MeetingStop,
            MessageKind::ListMeetings,
            MessageKind::GetMeeting,
            MessageKind::ProcessMeeting,
            MessageKind::RenameMeetingSpeaker,
            MessageKind::DeleteMeeting,
            MessageKind::SummarizeMeeting,
            MessageKind::GetAudioOptions,
            MessageKind::SetAudioOptions,
            MessageKind::TrainVoice,
            MessageKind::ForgetVoice,
            MessageKind::GetVocabulary,
            MessageKind::SetVocabulary,
            MessageKind::GetHoldKey,
            MessageKind::SetHoldKey,
            MessageKind::SetHistoryRetention,
            MessageKind::ClearHistory,
            MessageKind::GetTextOptions,
            MessageKind::SetTextOptions,
            MessageKind::SetPipelineVoice,
            MessageKind::ImportMeeting,
            MessageKind::GetMeetingOptions,
            MessageKind::SetMeetingOptions,
            MessageKind::GetMeetingSlide,
            MessageKind::MeetingControl,
            MessageKind::MeetingSources,
            MessageKind::AssistantToolDecision,
            MessageKind::AssistantLibrary,
            MessageKind::VoiceMode,
            MessageKind::GetRecoveryEntry,
            MessageKind::RunCapabilityCheck,
            MessageKind::RequestPortalKeyboardPermission,
            MessageKind::CancelPortalKeyboardPermission,
            MessageKind::RevokePortalKeyboardPermission,
            MessageKind::StartOfflineModelProvision,
            MessageKind::CancelOfflineModelProvision,
        ];
        for kind in request_kinds {
            assert!(
                kind.is_request(),
                "kind {:?} ({}) must be a request",
                kind,
                kind.wire_tag()
            );
            // Command mapping must agree.
            let wire = kind.wire_tag().to_string();
            let decoded = MessageKind::from_wire_tag(&wire);
            assert_eq!(decoded, kind);
            assert!(decoded.is_request());
        }
        // Non-requests
        assert!(!MessageKind::Response.is_request());
        assert!(!MessageKind::Event.is_request());
        assert!(!MessageKind::Other("request.future_v2".into()).is_request());
        assert!(!MessageKind::Other("event".into()).is_request());
        // Wire tags for response/event are distinct.
        assert_eq!(MessageKind::Response.wire_tag(), "response");
        assert_eq!(MessageKind::Event.wire_tag(), "event");
        // Every Command's kind must be a request.
        let cmds = [
            Command::GetStatus,
            Command::StartSession {
                pipeline_id: None,
                activation: "hotkey".into(),
                recognition: None,
            },
            Command::StopSession,
            Command::ToggleSession {
                pipeline_id: Some("clean".into()),
                activation: ActivationSourceDto::InstalledCliCommand,
                recognition: None,
            },
            Command::CancelSession,
            Command::Subscribe,
            Command::Unsubscribe,
            Command::ListPipelines,
            Command::SelectPipeline {
                pipeline_id: "default".into(),
            },
            Command::GetHistory,
            Command::GetHistoryEntry {
                txid: "tx-1".into(),
            },
            Command::GetRecoveryEntry {
                txid: "tx-1".into(),
            },
            Command::RunCapabilityCheck {
                check_id: "mic".into(),
            },
            Command::RequestPortalKeyboardPermission {
                operation_id: "request-1".into(),
                parent_window: "opaque:parent".into(),
            },
            Command::CancelPortalKeyboardPermission {
                operation_id: "request-1".into(),
            },
            Command::RevokePortalKeyboardPermission,
            Command::StartOfflineModelProvision {
                source_path: "/var/tmp/model.gguf".into(),
            },
            Command::CancelOfflineModelProvision {
                operation_id: "model-provision-1".into(),
            },
        ];
        for cmd in &cmds {
            assert!(
                cmd.kind().is_request(),
                "command {:?} must map to request kind",
                cmd
            );
        }
        Ok(())
    }

    #[test]
    fn ipc_envelope_builder_covers_session_transaction_seq()
    -> Result<(), Box<dyn std::error::Error>> {
        let env = Envelope::new(MessageKind::GetStatus, RequestId(99))
            .with_session(42u128)
            .with_transaction("tx-abc".to_string())
            .with_event_seq(7)
            .with_payload(serde_json::json!({"ok": true}))?;
        assert_eq!(env.session_id, Some(42));
        assert_eq!(env.transaction_id.as_deref(), Some("tx-abc"));
        assert_eq!(env.event_seq, Some(7));
        // Round-trip through JSON bytes keeps those fields.
        let bytes = env.to_json_bytes()?;
        let back = Envelope::from_json_bytes(&bytes)?;
        assert_eq!(back.session_id, Some(42));
        assert_eq!(back.transaction_id.as_deref(), Some("tx-abc"));
        assert_eq!(back.event_seq, Some(7));
        // to_json_bytes enforces bound; from_json_bytes rejects oversize.
        let big = vec![b'x'; MAX_MESSAGE_BYTES + 1];
        let err = match Envelope::from_json_bytes(&big) {
            Err(err) => err,
            Ok(_) => return Err("oversize envelope was accepted".into()),
        };
        assert_eq!(err.code, IpcErrorCode::PayloadTooLarge);
        Ok(())
    }

    #[test]
    fn ipc_offline_model_provision_commands_and_status_roundtrip()
    -> Result<(), Box<dyn std::error::Error>> {
        // Offline model provisioning remains additive at minor 5; the typed
        // capture end reason, atomic toggle, timings, and recognition settings
        // are later additive minors.
        assert_eq!(OFFLINE_MODEL_PROVISION_PROTOCOL_MINOR, 5);
        assert_eq!(RECOVERY_ENTRY_CAPTURE_END_PROTOCOL_MINOR, 6);
        assert_eq!(TOGGLE_SESSION_PROTOCOL_MINOR, 7);
        assert_eq!(SESSION_TIMINGS_PROTOCOL_MINOR, 8);
        assert_eq!(RECOGNITION_SETTINGS_PROTOCOL_MINOR, 9);
        assert_eq!(DELETE_HISTORY_ENTRY_PROTOCOL_MINOR, 10);
        assert_eq!(SHORTCUT_BINDINGS_PROTOCOL_MINOR, 11);
        assert_eq!(HISTORY_TIMING_PROTOCOL_MINOR, 12);
        assert_eq!(LIST_MODELS_PROTOCOL_MINOR, 13);
        assert_eq!(SELECT_MODEL_PROTOCOL_MINOR, 14);
        assert_eq!(PIPELINE_ENGINE_PROTOCOL_MINOR, 15);
        assert_eq!(DOWNLOAD_MODEL_PROTOCOL_MINOR, 16);
        assert_eq!(STORE_PROTOCOL_MINOR, 17);
        assert_eq!(DELIVERY_RESULT_PROTOCOL_MINOR, 18);
        assert_eq!(ASSISTANT_PROTOCOL_MINOR, 19);
        assert_eq!(LANGUAGE_MODELS_PROTOCOL_MINOR, 20);
        assert_eq!(AUDIO_LEVEL_PROTOCOL_MINOR, 21);
        assert_eq!(ASSISTANT_UNLOAD_PROTOCOL_MINOR, 22);
        assert_eq!(COPY_HISTORY_ENTRY_PROTOCOL_MINOR, 23);
        assert_eq!(WRITING_PROTOCOL_MINOR, 24);
        assert_eq!(MEETINGS_PROTOCOL_MINOR, 25);
        assert_eq!(MEETING_NOTES_PROTOCOL_MINOR, 26);
        assert_eq!(AUDIO_OPTIONS_PROTOCOL_MINOR, 27);
        assert_eq!(VOICE_PROTOCOL_MINOR, 28);
        assert_eq!(NOISE_SUPPRESSION_PROTOCOL_MINOR, 29);
        assert_eq!(VOCABULARY_PROTOCOL_MINOR, 30);
        assert_eq!(MICROPHONE_PROTOCOL_MINOR, 31);
        assert_eq!(HOLD_KEY_PROTOCOL_MINOR, 32);
        assert_eq!(SOUNDS_PROTOCOL_MINOR, 33);
        assert_eq!(HISTORY_SETTINGS_PROTOCOL_MINOR, 34);
        assert_eq!(TEXT_OPTIONS_PROTOCOL_MINOR, 35);
        assert_eq!(PIPELINE_VOICE_PROTOCOL_MINOR, 36);
        assert_eq!(MEETING_IMPORT_PROTOCOL_MINOR, 37);
        assert_eq!(DELIVERY_METHOD_PROTOCOL_MINOR, 38);
        assert_eq!(MEETING_OPTIONS_PROTOCOL_MINOR, 39);
        assert_eq!(CLEANUP_IDLE_PROTOCOL_MINOR, 40);
        assert_eq!(VOICE_STRICTNESS_PROTOCOL_MINOR, 41);
        assert_eq!(SPOKEN_BREAKS_PROTOCOL_MINOR, 42);
        assert_eq!(MEETING_DEVICES_PROTOCOL_MINOR, 43);
        assert_eq!(ASSISTANT_LIBRARY_PROTOCOL_MINOR, 44);
        assert_eq!(MEETING_SCREEN_PROTOCOL_MINOR, 45);
        assert_eq!(ASSISTANT_TOOLS_PROTOCOL_MINOR, 46);
        assert_eq!(ASSISTANT_AGENT_PROTOCOL_MINOR, 47);
        assert_eq!(MEETING_CONTROL_PROTOCOL_MINOR, 48);
        assert_eq!(SCREEN_SESSIONS_PROTOCOL_MINOR, 49);
        assert_eq!(MEETING_SOURCES_PROTOCOL_MINOR, 50);
        assert_eq!(MEETING_MICROPHONE_PROTOCOL_MINOR, 51);
        assert_eq!(MEETING_QUICK_SAVE_PROTOCOL_MINOR, 52);
        assert_eq!(MEETING_LIVE_PARTIAL_PROTOCOL_MINOR, 53);
        assert_eq!(DICTATION_LANGUAGE_PROTOCOL_MINOR, 54);
        assert_eq!(PROTOCOL_MINOR, DICTATION_LANGUAGE_PROTOCOL_MINOR);

        let start = Command::StartOfflineModelProvision {
            source_path: "/home/user/Models/local model.gguf".into(),
        };
        assert_eq!(start.kind(), MessageKind::StartOfflineModelProvision);
        assert_eq!(
            start.kind().wire_tag(),
            "request.start_offline_model_provision"
        );
        assert!(start.kind().is_request());
        let start_json = serde_json::to_value(&start)?;
        assert_eq!(start_json["command"], "start_offline_model_provision");
        assert_eq!(serde_json::from_value::<Command>(start_json)?, start);

        let cancel = Command::CancelOfflineModelProvision {
            operation_id: "provision_01-local".into(),
        };
        assert_eq!(cancel.kind(), MessageKind::CancelOfflineModelProvision);
        assert_eq!(
            cancel.kind().wire_tag(),
            "request.cancel_offline_model_provision"
        );
        let cancel_json = serde_json::to_value(&cancel)?;
        assert_eq!(cancel_json["command"], "cancel_offline_model_provision");
        assert_eq!(serde_json::from_value::<Command>(cancel_json)?, cancel);

        let status = ModelProvisionStatusDto {
            operation_id: "provision_01-local".into(),
            state: ModelProvisionStateDto::Activating,
            publication: ModelProvisionPublicationDto::VerifiedInstalled,
            cleanup: ModelProvisionCleanupDto::Confirmed,
            failure: None,
            revision: 7,
        };
        let response = ResponsePayload::OfflineModelProvision(status.clone());
        let response_json = serde_json::to_value(&response)?;
        assert_eq!(response_json["result"], "offline_model_provision");
        assert_eq!(response_json["payload"]["state"], "activating");
        assert_eq!(
            serde_json::from_value::<ResponsePayload>(response_json)?,
            response
        );

        let full_status = StatusDto {
            daemon_build: "daemon-v5".into(),
            protocol_minor: PROTOCOL_MINOR,
            active_session: None,
            last_session_timings_us: None,
            capabilities: vec![],
            installed_model: Some(Box::new(InstalledModelLifecycleDto::Provisioning)),
            portal_keyboard: None,
            model_provisioning: Some(Box::new(status)),
        };
        let full_status_json = serde_json::to_value(&full_status)?;
        assert_eq!(full_status_json["installed_model"], "provisioning");
        assert_eq!(full_status_json["model_provisioning"]["revision"], 7);
        assert_eq!(
            serde_json::from_value::<StatusDto>(full_status_json)?,
            full_status
        );
        Ok(())
    }

    #[test]
    fn ipc_toggle_session_is_additive_and_carries_a_closed_activation_source()
    -> Result<(), Box<dyn std::error::Error>> {
        // The additive request keeps the older Start/Stop commands byte-shape
        // compatible while carrying only a canonical pipeline id and a closed
        // activation source.
        let toggle = Command::ToggleSession {
            pipeline_id: Some("raw".into()),
            activation: ActivationSourceDto::InstalledCliCommand,
            recognition: None,
        };
        assert_eq!(toggle.kind(), MessageKind::ToggleSession);
        assert_eq!(toggle.kind().wire_tag(), "request.toggle_session");
        assert!(toggle.kind().is_request());

        let json = serde_json::to_value(&toggle)?;
        assert_eq!(json["command"], "toggle_session");
        assert_eq!(json["pipeline_id"], "raw");
        assert_eq!(json["activation"], "installed_cli_command");
        assert_eq!(serde_json::from_value::<Command>(json)?, toggle);

        // Minimal shape: both the pipeline identity and the whole activation
        // source are optional/closed on the wire, never a free-form policy
        // string.
        let minimal = serde_json::json!({"command": "toggle_session"});
        assert_eq!(
            serde_json::from_value::<Command>(minimal)?,
            Command::ToggleSession {
                pipeline_id: None,
                activation: ActivationSourceDto::InstalledCliCommand,
                recognition: None,
            }
        );
        let desktop = serde_json::json!({
            "command": "toggle_session",
            "activation": "desktop_client"
        });
        assert_eq!(
            serde_json::from_value::<Command>(desktop)?,
            Command::ToggleSession {
                pipeline_id: None,
                activation: ActivationSourceDto::DesktopClient,
                recognition: None,
            }
        );
        // An unknown activation tag is preserved verbatim for forward
        // compatibility instead of becoming an arbitrary accepted policy.
        let unknown = serde_json::json!({
            "command": "toggle_session",
            "activation": "future_surface"
        });
        assert_eq!(
            serde_json::from_value::<Command>(unknown)?,
            Command::ToggleSession {
                pipeline_id: None,
                activation: ActivationSourceDto::Unknown("future_surface".into()),
                recognition: None,
            }
        );
        assert_eq!(
            ActivationSourceDto::from_wire_tag("future_surface").wire_tag(),
            "future_surface"
        );

        // Every typed outcome and the response payload round-trip.
        for outcome in [
            ToggleOutcomeDto::Started,
            ToggleOutcomeDto::FinalizationRequested,
            ToggleOutcomeDto::AlreadyStopping,
        ] {
            let expected_tag = outcome.wire_tag().to_owned();
            let result = ToggleResultDto {
                outcome,
                activation: ActivationSourceDto::InstalledCliCommand,
                session_id: Some(1u128 << 32 | 7),
                transaction_id: Some("tx-toggle".into()),
                pipeline_id: Some("clean".into()),
            };
            let payload = ResponsePayload::Toggle(result.clone());
            let encoded = serde_json::to_value(&payload)?;
            assert_eq!(encoded["result"], "toggle");
            assert_eq!(encoded["payload"]["session_id"], "4294967303");
            assert_eq!(serde_json::from_value::<ResponsePayload>(encoded)?, payload);
            assert_eq!(result.outcome.wire_tag(), expected_tag);
        }

        // Older peers still decode the untouched session commands.
        let start = serde_json::json!({
            "command": "start_session",
            "pipeline_id": null,
            "activation": "manual"
        });
        assert!(serde_json::from_value::<Command>(start).is_ok());
        assert_eq!(
            Command::StopSession.kind().wire_tag(),
            "request.stop_session"
        );
        Ok(())
    }

    #[test]
    fn ipc_offline_model_provision_fields_are_strictly_bounded()
    -> Result<(), Box<dyn std::error::Error>> {
        let at_bound = Command::StartOfflineModelProvision {
            source_path: "p".repeat(MAX_MODEL_PROVISION_SOURCE_PATH_BYTES),
        };
        let encoded = serde_json::to_value(&at_bound)?;
        assert_eq!(serde_json::from_value::<Command>(encoded)?, at_bound);

        for invalid_source in [
            String::new(),
            "p".repeat(MAX_MODEL_PROVISION_SOURCE_PATH_BYTES + 1),
            "/tmp/model\nforged.gguf".into(),
            "/tmp/model\u{0}suffix.gguf".into(),
        ] {
            let invalid = Command::StartOfflineModelProvision {
                source_path: invalid_source,
            };
            assert!(serde_json::to_value(invalid).is_err());
        }

        for invalid_operation_id in [
            String::new(),
            "a".repeat(MAX_MODEL_PROVISION_OPERATION_ID_BYTES + 1),
            "contains/slash".into(),
            "contains space".into(),
        ] {
            let invalid = Command::CancelOfflineModelProvision {
                operation_id: invalid_operation_id,
            };
            assert!(serde_json::to_value(invalid).is_err());
        }

        let invalid_source = serde_json::json!({
            "command": "start_offline_model_provision",
            "source_path": "/tmp/model\tforged.gguf"
        });
        assert!(serde_json::from_value::<Command>(invalid_source).is_err());
        let invalid_operation = serde_json::json!({
            "command": "cancel_offline_model_provision",
            "operation_id": "not opaque"
        });
        assert!(serde_json::from_value::<Command>(invalid_operation).is_err());
        Ok(())
    }

    #[test]
    fn ipc_offline_model_provision_debug_redacts_path_and_operation_id() {
        const PATH_CANARY: &str = "/private/PATH_CANARY_8638/model.gguf";
        const OPERATION_CANARY: &str = "OPERATION_CANARY_7218";

        let start = Command::StartOfflineModelProvision {
            source_path: PATH_CANARY.into(),
        };
        let start_debug = format!("{start:?}");
        assert!(!start_debug.contains(PATH_CANARY));
        assert!(start_debug.contains("<redacted>"));

        let cancel = Command::CancelOfflineModelProvision {
            operation_id: OPERATION_CANARY.into(),
        };
        let cancel_debug = format!("{cancel:?}");
        assert!(!cancel_debug.contains(OPERATION_CANARY));
        assert!(cancel_debug.contains("<redacted>"));

        let status = ModelProvisionStatusDto {
            operation_id: OPERATION_CANARY.into(),
            state: ModelProvisionStateDto::Failed,
            publication: ModelProvisionPublicationDto::PublishedUnconfirmed,
            cleanup: ModelProvisionCleanupDto::Failed,
            failure: Some(ModelProvisionFailureDto::PublishedUnconfirmed),
            revision: 9,
        };
        let status_debug = format!("{status:?}");
        assert!(!status_debug.contains(OPERATION_CANARY));
        assert!(status_debug.contains("<redacted>"));

        let response_debug = format!("{:?}", ResponsePayload::OfflineModelProvision(status));
        assert!(!response_debug.contains(OPERATION_CANARY));
        assert!(response_debug.contains("<redacted>"));
    }

    #[test]
    fn ipc_offline_model_provision_unknown_tags_roundtrip() -> Result<(), Box<dyn std::error::Error>>
    {
        macro_rules! assert_unknown_roundtrip {
            ($ty:ty, $variant:path, $wire:literal) => {{
                let decoded: $ty = serde_json::from_str(concat!("\"", $wire, "\""))?;
                assert_eq!(decoded, $variant($wire.into()));
                assert_eq!(serde_json::to_string(&decoded)?, concat!("\"", $wire, "\""));
            }};
        }

        assert_unknown_roundtrip!(
            ModelProvisionStateDto,
            ModelProvisionStateDto::Unknown,
            "future_state_v9"
        );
        assert_unknown_roundtrip!(
            ModelProvisionPublicationDto,
            ModelProvisionPublicationDto::Unknown,
            "future_publication_v9"
        );
        assert_unknown_roundtrip!(
            ModelProvisionCleanupDto,
            ModelProvisionCleanupDto::Unknown,
            "future_cleanup_v9"
        );
        assert_unknown_roundtrip!(
            ModelProvisionFailureDto,
            ModelProvisionFailureDto::Unknown,
            "future_failure_v9"
        );
        assert_unknown_roundtrip!(
            InstalledModelLifecycleDto,
            InstalledModelLifecycleDto::Unknown,
            "future_lifecycle_v9"
        );
        Ok(())
    }

    #[test]
    fn ipc_recognition_settings_roundtrip_distinct_fields() -> Result<(), Box<dyn std::error::Error>>
    {
        let tag = |value| RecognitionTagDto::parse(value);
        let settings = RecognitionSettingsDto {
            spoken_language: SpokenLanguageDto::Explicit { tag: tag("en-US")? },
            output_language: Some(tag("ur")?),
            output_script: Some(tag("Arab")?),
            translation_target: Some(tag("en")?),
            locale: Some(tag("ur-PK")?),
            context_hints: Some(RecognitionContextHintsDto::try_new(vec![
                "Narrivy".into(),
                "OpenVINO".into(),
            ])?),
        };
        let encoded = serde_json::to_value(&settings)?;
        assert_eq!(encoded["spoken_language"]["kind"], "explicit");
        assert_eq!(encoded["spoken_language"]["tag"], "en-US");
        assert_eq!(encoded["output_language"], "ur");
        assert_eq!(encoded["output_script"], "Arab");
        assert_eq!(encoded["translation_target"], "en");
        assert_eq!(encoded["locale"], "ur-PK");
        assert_eq!(
            serde_json::from_value::<RecognitionSettingsDto>(encoded)?,
            settings
        );
        Ok(())
    }

    #[test]
    fn ipc_recognition_settings_default_has_empty_additive_shape()
    -> Result<(), Box<dyn std::error::Error>> {
        let settings = RecognitionSettingsDto::default();
        let encoded = serde_json::to_value(&settings)?;
        assert_eq!(encoded, serde_json::json!({}));
        assert_eq!(
            serde_json::from_value::<RecognitionSettingsDto>(encoded)?,
            settings
        );
        Ok(())
    }

    #[test]
    fn ipc_recognition_settings_reject_invalid_tags_and_hints() {
        for invalid in ["", "en US", "en-", "-en", "en--US", "en/US"] {
            assert!(RecognitionTagDto::parse(invalid).is_err());
            assert!(
                serde_json::from_value::<RecognitionTagDto>(serde_json::json!(invalid)).is_err()
            );
        }
        let oversized_tag = "a".repeat(MAX_RECOGNITION_TAG_BYTES + 1);
        assert!(RecognitionTagDto::parse(oversized_tag.clone()).is_err());
        assert!(
            serde_json::from_value::<RecognitionTagDto>(serde_json::json!(oversized_tag)).is_err()
        );
        assert!(serde_json::to_value(RecognitionTagDto(String::new())).is_err());

        for invalid in [
            Vec::new(),
            vec![String::new()],
            vec!["x".repeat(MAX_RECOGNITION_CONTEXT_HINT_BYTES + 1)],
            vec!["contains\0nul".into()],
            vec!["hint".into(); MAX_RECOGNITION_CONTEXT_HINTS + 1],
        ] {
            assert!(RecognitionContextHintsDto::try_new(invalid.clone()).is_err());
            assert!(
                serde_json::from_value::<RecognitionContextHintsDto>(serde_json::json!(invalid))
                    .is_err()
            );
        }
        assert!(serde_json::to_value(RecognitionContextHintsDto(Vec::new())).is_err());
    }

    #[test]
    fn ipc_recognition_settings_debug_redacts_context() -> Result<(), Box<dyn std::error::Error>> {
        const CANARY: &str = "PRIVATE_CONTEXT_CANARY_4419";
        let settings = RecognitionSettingsDto {
            context_hints: Some(RecognitionContextHintsDto::try_new(vec![CANARY.into()])?),
            ..RecognitionSettingsDto::default()
        };
        let debug = format!("{settings:?}");
        assert!(!debug.contains(CANARY));
        assert!(debug.contains("hint_count: 1"));
        Ok(())
    }

    #[test]
    fn ipc_session_commands_carry_recognition_settings_from_minor_nine()
    -> Result<(), Box<dyn std::error::Error>> {
        const CANARY: &str = "PRIVATE_COMMAND_CONTEXT_8821";
        assert_eq!(RECOGNITION_SETTINGS_PROTOCOL_MINOR, 9);
        let recognition = RecognitionSettingsDto {
            spoken_language: SpokenLanguageDto::AutoDetect,
            context_hints: Some(RecognitionContextHintsDto::try_new(vec![CANARY.into()])?),
            ..RecognitionSettingsDto::default()
        };
        for command in [
            Command::StartSession {
                pipeline_id: Some("clean".into()),
                activation: "ui".into(),
                recognition: Some(recognition.clone()),
            },
            Command::ToggleSession {
                pipeline_id: Some("clean".into()),
                activation: ActivationSourceDto::DesktopClient,
                recognition: Some(recognition.clone()),
            },
        ] {
            let encoded = serde_json::to_value(&command)?;
            assert_eq!(
                encoded["recognition"]["spoken_language"]["kind"],
                "auto_detect"
            );
            assert_eq!(serde_json::from_value::<Command>(encoded)?, command);
            assert!(!format!("{command:?}").contains(CANARY));
        }
        Ok(())
    }
}
