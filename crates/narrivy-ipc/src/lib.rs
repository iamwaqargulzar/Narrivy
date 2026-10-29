//! Portable versioned and bounded local-IPC wire contracts.
//!
//! This crate owns the transport-neutral protocol surface from
//! `spec/protocols/ipc.md`: typed errors, version negotiation, envelopes,
//! commands, responses, events and bounded length-prefixed framing over
//! generic [`std::io::Read`] / [`std::io::Write`] streams.
//!
//! Concrete endpoint transports stay outside this crate. Linux and other
//! Unix clients compose `narrivy-ipc-unix`, which owns Unix sockets, peer
//! authentication, endpoint permissions, locking and connection lifecycle.

#![cfg_attr(docsrs, feature(doc_cfg))]
#![forbid(unsafe_code)]

pub mod error;
pub mod framing;
pub mod protocol;

#[cfg(test)]
mod security_audit;

pub use error::{IpcError, IpcErrorCode};
pub use protocol::{
    ASSISTANT_AGENT_PROTOCOL_MINOR, ASSISTANT_LIBRARY_PROTOCOL_MINOR, ASSISTANT_PROTOCOL_MINOR,
    ASSISTANT_TOOLS_PROTOCOL_MINOR, ASSISTANT_UNLOAD_PROTOCOL_MINOR, AUDIO_LEVEL_PROTOCOL_MINOR,
    AUDIO_OPTIONS_PROTOCOL_MINOR, ActivationSourceDto, ActiveSessionDto, AssistantMessageDto,
    AudioOptionsDto, BUILD_IDENTITY, CLEANUP_IDLE_PROTOCOL_MINOR,
    COPY_HISTORY_ENTRY_PROTOCOL_MINOR, Command, DELETE_HISTORY_ENTRY_PROTOCOL_MINOR,
    DELIVERY_METHOD_PROTOCOL_MINOR, DELIVERY_RESULT_PROTOCOL_MINOR,
    DICTATION_LANGUAGE_PROTOCOL_MINOR, DOWNLOAD_MODEL_PROTOCOL_MINOR, Envelope, Event,
    HISTORY_SETTINGS_PROTOCOL_MINOR, HISTORY_TIMING_PROTOCOL_MINOR, HOLD_KEY_PROTOCOL_MINOR,
    HandshakeHello, HandshakeResponse, HoldKeyChoiceDto, HoldKeyDto, InstalledModelLifecycleDto,
    LANGUAGE_MODELS_PROTOCOL_MINOR, LIST_MODELS_PROTOCOL_MINOR, LanguageModelDto,
    MAX_ASSISTANT_MESSAGES, MAX_COLLECTION_ELEMENTS, MAX_MESSAGE_BYTES, MAX_MODEL_ID_BYTES,
    MAX_MODEL_PROVISION_OPERATION_ID_BYTES, MAX_MODEL_PROVISION_SOURCE_PATH_BYTES,
    MAX_PARTIAL_TRANSCRIPT_BYTES, MAX_PORTAL_OPERATION_ID_BYTES, MAX_PORTAL_PARENT_WINDOW_BYTES,
    MAX_SHORTCUT_KEY_BYTES, MAX_SHORTCUT_MODIFIERS, MEETING_CONTROL_PROTOCOL_MINOR,
    MEETING_DEVICES_PROTOCOL_MINOR, MEETING_IMPORT_PROTOCOL_MINOR,
    MEETING_LIVE_PARTIAL_PROTOCOL_MINOR, MEETING_MICROPHONE_PROTOCOL_MINOR,
    MEETING_NOTES_PROTOCOL_MINOR, MEETING_OPTIONS_PROTOCOL_MINOR,
    MEETING_QUICK_SAVE_PROTOCOL_MINOR, MEETING_SCREEN_PROTOCOL_MINOR,
    MEETING_SOURCES_PROTOCOL_MINOR, MEETINGS_PROTOCOL_MINOR, MICROPHONE_PROTOCOL_MINOR,
    MeetingDetailDto, MeetingDto, MeetingOptionsDto, MeetingSlideDto, MeetingTurnDto, MessageKind,
    MicrophoneDto, ModelDto, ModelProvisionCleanupDto, ModelProvisionFailureDto,
    ModelProvisionPublicationDto, ModelProvisionStateDto, ModelProvisionStatusDto,
    NOISE_SUPPRESSION_PROTOCOL_MINOR, OFFLINE_MODEL_PROVISION_PROTOCOL_MINOR,
    PIPELINE_ENGINE_PROTOCOL_MINOR, PIPELINE_VOICE_PROTOCOL_MINOR, PROTOCOL_MAJOR, PROTOCOL_MINOR,
    PipelineSummaryDto, PipelineVoiceDto, PortalCleanupDto, PortalEligibilityDto, PortalFailureDto,
    PortalKeyboardStatusDto, PortalLifecycleDto, RECOGNITION_SETTINGS_PROTOCOL_MINOR,
    RECOVERY_ENTRY_CAPTURE_END_PROTOCOL_MINOR, RECOVERY_ENTRY_PROTOCOL_MINOR,
    REFINE_PREVIEW_SEQ_BASE, RecognitionContextHintsDto, RecognitionSettingsDto, RecognitionTagDto,
    RecoveryEntryDetailDto, ReplacementDto, RequestId, ResponsePayload,
    SCREEN_SESSIONS_PROTOCOL_MINOR, SELECT_MODEL_PROTOCOL_MINOR, SESSION_TIMINGS_PROTOCOL_MINOR,
    SHORTCUT_BINDINGS_PROTOCOL_MINOR, SOUNDS_PROTOCOL_MINOR, SPOKEN_BREAKS_PROTOCOL_MINOR,
    STORE_PROTOCOL_MINOR, ScreenSourceDto, ShortcutBindingDto, ShortcutSetDto, ShortcutsDto,
    SpokenLanguageDto, StageTimingsUs, StatusDto, StoreItemDto, TEXT_OPTIONS_PROTOCOL_MINOR,
    TOGGLE_SESSION_PROTOCOL_MINOR, TRANSCRIPT_PARTIAL_PROTOCOL_MINOR, TextOptionsDto,
    ToggleOutcomeDto, ToggleResultDto, VOCABULARY_PROTOCOL_MINOR, VOICE_PROTOCOL_MINOR,
    VOICE_STRICTNESS_PROTOCOL_MINOR, VocabularyDto, VoiceSettingsDto, VoiceTimingDto,
    WRITING_PROTOCOL_MINOR, WritingFindingDto, WritingFixDto, WritingRuleDto, negotiate_handshake,
};
