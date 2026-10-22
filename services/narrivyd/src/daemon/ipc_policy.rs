//! Daemon-owned authorization and resource limits for IPC commands.
//!
//! The Unix transport authenticates one effective UID. It does not identify a
//! Narrivy binary, and the handshake role is deliberately absent from this
//! policy. Rules therefore constrain every same-UID caller uniformly and rely
//! on daemon-owned lifecycle state for exact session/transaction/operation
//! admission. See ADR-0031 and ADR-0061.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use narrivy_ipc::{Command, IpcErrorCode};
use narrivy_ipc_unix::PeerCredentials;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum CommandId {
    GetStatus,
    StartSession,
    StopSession,
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
    AssistantSend,
    AssistantCancel,
    ListLanguageModels,
    SelectLanguageModel,
    AssistantUnload,
    AssistantLibrary,
    VoiceMode,
    GetMeetingSlide,
    MeetingControl,
    MeetingSources,
    AssistantToolDecision,
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
    GetHoldKey,
    SetHistoryRetention,
    GetTextOptions,
    SetPipelineVoice,
    ImportMeeting,
    GetMeetingOptions,
    SetMeetingOptions,
    SetTextOptions,
    ClearHistory,
    SetHoldKey,
    SetVocabulary,
    RemoveStoreItem,
    GetRecoveryEntry,
    RunCapabilityCheck,
    RequestPortalKeyboardPermission,
    CancelPortalKeyboardPermission,
    RevokePortalKeyboardPermission,
    StartOfflineModelProvision,
    CancelOfflineModelProvision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandSecurityClass {
    ReadOnly,
    StateChanging,
    PrivacySensitive,
    Destructive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RateLimit {
    burst: u16,
    window: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CommandRule {
    id: CommandId,
    class: CommandSecurityClass,
    rate: RateLimit,
}

const fn rate(burst: u16, seconds: u64) -> RateLimit {
    RateLimit {
        burst,
        window: Duration::from_secs(seconds),
    }
}

/// Exhaustive command matrix. Adding a wire command forces this match to be
/// updated before the daemon compiles.
fn command_rule(command: &Command) -> CommandRule {
    use CommandSecurityClass::{Destructive, PrivacySensitive, ReadOnly, StateChanging};

    match command {
        Command::GetStatus => CommandRule {
            id: CommandId::GetStatus,
            class: ReadOnly,
            rate: rate(64, 1),
        },
        Command::StartSession { .. } => CommandRule {
            id: CommandId::StartSession,
            class: StateChanging,
            rate: rate(8, 1),
        },
        Command::StopSession => CommandRule {
            id: CommandId::StopSession,
            class: StateChanging,
            rate: rate(16, 1),
        },
        // One atomic activation can start capture, so it carries the Start
        // budget rather than a fresh unlimited bucket.
        Command::ToggleSession { .. } => CommandRule {
            id: CommandId::ToggleSession,
            class: StateChanging,
            rate: rate(8, 1),
        },
        Command::CancelSession => CommandRule {
            id: CommandId::CancelSession,
            class: Destructive,
            rate: rate(16, 1),
        },
        // Subscription can expose volatile transcript events, so it is a
        // privacy-sensitive read rather than an ordinary observation.
        Command::Subscribe => CommandRule {
            id: CommandId::Subscribe,
            class: PrivacySensitive,
            rate: rate(16, 1),
        },
        Command::Unsubscribe => CommandRule {
            id: CommandId::Unsubscribe,
            class: StateChanging,
            rate: rate(16, 1),
        },
        Command::ListPipelines => CommandRule {
            id: CommandId::ListPipelines,
            class: ReadOnly,
            rate: rate(32, 1),
        },
        Command::SelectPipeline { .. } => CommandRule {
            id: CommandId::SelectPipeline,
            class: StateChanging,
            rate: rate(8, 1),
        },
        Command::GetHistory => CommandRule {
            id: CommandId::GetHistory,
            class: PrivacySensitive,
            rate: rate(16, 1),
        },
        Command::GetHistoryEntry { .. } => CommandRule {
            id: CommandId::GetHistoryEntry,
            class: PrivacySensitive,
            rate: rate(32, 1),
        },
        Command::DeleteHistoryEntry { .. } => CommandRule {
            id: CommandId::DeleteHistoryEntry,
            class: Destructive,
            rate: rate(16, 1),
        },
        Command::ListShortcuts => CommandRule {
            id: CommandId::ListShortcuts,
            class: ReadOnly,
            rate: rate(32, 1),
        },
        // Both edit the user's desktop settings, so they share the tightest
        // class and a small budget.
        Command::SetShortcut { .. } => CommandRule {
            id: CommandId::SetShortcut,
            class: Destructive,
            rate: rate(8, 1),
        },
        Command::RemoveShortcut { .. } => CommandRule {
            id: CommandId::RemoveShortcut,
            class: Destructive,
            rate: rate(8, 1),
        },
        Command::ListModels => CommandRule {
            id: CommandId::ListModels,
            class: ReadOnly,
            rate: rate(16, 1),
        },
        Command::SelectModel { .. } => CommandRule {
            id: CommandId::SelectModel,
            class: StateChanging,
            rate: rate(4, 1),
        },
        Command::SetPipelineEngine { .. } => CommandRule {
            id: CommandId::SetPipelineEngine,
            class: StateChanging,
            rate: rate(8, 1),
        },
        // The only command that makes the daemon use the network.
        Command::DownloadModel { .. } => CommandRule {
            id: CommandId::DownloadModel,
            class: StateChanging,
            rate: rate(4, 1),
        },
        Command::CancelModelDownload { .. } => CommandRule {
            id: CommandId::CancelModelDownload,
            class: StateChanging,
            rate: rate(8, 1),
        },
        Command::ListStore => CommandRule {
            id: CommandId::ListStore,
            class: ReadOnly,
            rate: rate(16, 1),
        },
        // The conversation is the user's own text, so it is treated like
        // History; one turn runs at a time and costs real compute.
        Command::AssistantSend { .. } => CommandRule {
            id: CommandId::AssistantSend,
            class: PrivacySensitive,
            rate: rate(8, 1),
        },
        Command::CopyHistoryEntry { .. } => CommandRule {
            id: CommandId::CopyHistoryEntry,
            class: PrivacySensitive,
            rate: rate(8, 1),
        },
        // Checks run on every pause in typing, in every open tab.
        Command::CheckWriting { .. } => CommandRule {
            id: CommandId::CheckWriting,
            class: PrivacySensitive,
            rate: rate(32, 1),
        },
        Command::ListWritingRules => CommandRule {
            id: CommandId::ListWritingRules,
            class: ReadOnly,
            rate: rate(8, 1),
        },
        Command::WritingAction { .. } => CommandRule {
            id: CommandId::WritingAction,
            class: PrivacySensitive,
            rate: rate(8, 1),
        },
        // Recording a meeting captures the microphone and the computer's sound.
        Command::MeetingStart { .. } => CommandRule {
            id: CommandId::MeetingStart,
            class: PrivacySensitive,
            rate: rate(4, 1),
        },
        Command::MeetingStop => CommandRule {
            id: CommandId::MeetingStop,
            class: StateChanging,
            rate: rate(4, 1),
        },
        Command::ListMeetings => CommandRule {
            id: CommandId::ListMeetings,
            class: ReadOnly,
            rate: rate(16, 1),
        },
        Command::GetMeeting { .. } => CommandRule {
            id: CommandId::GetMeeting,
            class: PrivacySensitive,
            rate: rate(16, 1),
        },
        Command::ProcessMeeting { .. } => CommandRule {
            id: CommandId::ProcessMeeting,
            class: StateChanging,
            rate: rate(4, 1),
        },
        Command::RenameMeetingSpeaker { .. } => CommandRule {
            id: CommandId::RenameMeetingSpeaker,
            class: StateChanging,
            rate: rate(16, 1),
        },
        Command::SummarizeMeeting { .. } => CommandRule {
            id: CommandId::SummarizeMeeting,
            class: PrivacySensitive,
            rate: rate(4, 1),
        },
        Command::GetAudioOptions => CommandRule {
            id: CommandId::GetAudioOptions,
            class: ReadOnly,
            rate: rate(16, 1),
        },
        Command::SetAudioOptions { .. } => CommandRule {
            id: CommandId::SetAudioOptions,
            class: StateChanging,
            rate: rate(8, 1),
        },
        Command::GetMeetingOptions => CommandRule {
            id: CommandId::GetMeetingOptions,
            class: ReadOnly,
            rate: rate(16, 1),
        },
        Command::SetMeetingOptions { .. } => CommandRule {
            id: CommandId::SetMeetingOptions,
            class: StateChanging,
            rate: rate(8, 1),
        },
        Command::ImportMeeting { .. } => CommandRule {
            id: CommandId::ImportMeeting,
            class: PrivacySensitive,
            rate: rate(4, 1),
        },
        Command::SetPipelineVoice { .. } => CommandRule {
            id: CommandId::SetPipelineVoice,
            class: StateChanging,
            rate: rate(8, 1),
        },
        Command::GetTextOptions => CommandRule {
            id: CommandId::GetTextOptions,
            class: ReadOnly,
            rate: rate(16, 1),
        },
        Command::SetTextOptions { .. } => CommandRule {
            id: CommandId::SetTextOptions,
            class: StateChanging,
            rate: rate(8, 1),
        },
        Command::SetHistoryRetention { .. } => CommandRule {
            id: CommandId::SetHistoryRetention,
            class: StateChanging,
            rate: rate(8, 1),
        },
        Command::ClearHistory => CommandRule {
            id: CommandId::ClearHistory,
            class: Destructive,
            rate: rate(4, 1),
        },
        Command::GetHoldKey => CommandRule {
            id: CommandId::GetHoldKey,
            class: ReadOnly,
            rate: rate(16, 1),
        },
        Command::SetHoldKey { .. } => CommandRule {
            id: CommandId::SetHoldKey,
            class: StateChanging,
            rate: rate(8, 1),
        },
        Command::GetVocabulary => CommandRule {
            id: CommandId::GetVocabulary,
            class: ReadOnly,
            rate: rate(16, 1),
        },
        Command::SetVocabulary { .. } => CommandRule {
            id: CommandId::SetVocabulary,
            class: StateChanging,
            rate: rate(8, 1),
        },
        Command::TrainVoice => CommandRule {
            id: CommandId::TrainVoice,
            class: PrivacySensitive,
            rate: rate(2, 1),
        },
        Command::ForgetVoice => CommandRule {
            id: CommandId::ForgetVoice,
            class: Destructive,
            rate: rate(4, 1),
        },
        Command::DeleteMeeting { .. } => CommandRule {
            id: CommandId::DeleteMeeting,
            class: Destructive,
            rate: rate(8, 1),
        },
        Command::AssistantToolDecision { .. } => CommandRule {
            id: CommandId::AssistantToolDecision,
            class: PrivacySensitive,
            rate: rate(60, 20),
        },
        Command::MeetingControl { .. } => CommandRule {
            id: CommandId::MeetingControl,
            class: PrivacySensitive,
            rate: rate(60, 10),
        },
        Command::MeetingSources { .. } => CommandRule {
            id: CommandId::MeetingSources,
            class: PrivacySensitive,
            rate: rate(60, 20),
        },
        Command::GetMeetingSlide { .. } => CommandRule {
            id: CommandId::GetMeetingSlide,
            class: PrivacySensitive,
            rate: rate(120, 20),
        },
        Command::AssistantLibrary { .. } => CommandRule {
            id: CommandId::AssistantLibrary,
            class: PrivacySensitive,
            rate: rate(60, 20),
        },
        Command::VoiceMode { .. } => CommandRule {
            id: CommandId::VoiceMode,
            class: PrivacySensitive,
            rate: rate(60, 20),
        },
        Command::AssistantUnload => CommandRule {
            id: CommandId::AssistantUnload,
            class: StateChanging,
            rate: rate(8, 1),
        },
        Command::ListLanguageModels => CommandRule {
            id: CommandId::ListLanguageModels,
            class: ReadOnly,
            rate: rate(16, 1),
        },
        Command::SelectLanguageModel { .. } => CommandRule {
            id: CommandId::SelectLanguageModel,
            class: StateChanging,
            rate: rate(4, 1),
        },
        Command::AssistantCancel { .. } => CommandRule {
            id: CommandId::AssistantCancel,
            class: StateChanging,
            rate: rate(16, 1),
        },
        Command::RemoveStoreItem { .. } => CommandRule {
            id: CommandId::RemoveStoreItem,
            class: Destructive,
            rate: rate(4, 1),
        },
        Command::GetRecoveryEntry { .. } => CommandRule {
            id: CommandId::GetRecoveryEntry,
            class: PrivacySensitive,
            rate: rate(32, 1),
        },
        Command::RunCapabilityCheck { .. } => CommandRule {
            id: CommandId::RunCapabilityCheck,
            class: ReadOnly,
            rate: rate(4, 10),
        },
        Command::RequestPortalKeyboardPermission { .. } => CommandRule {
            id: CommandId::RequestPortalKeyboardPermission,
            class: StateChanging,
            rate: rate(2, 60),
        },
        Command::CancelPortalKeyboardPermission { .. } => CommandRule {
            id: CommandId::CancelPortalKeyboardPermission,
            class: Destructive,
            rate: rate(8, 10),
        },
        Command::RevokePortalKeyboardPermission => CommandRule {
            id: CommandId::RevokePortalKeyboardPermission,
            class: Destructive,
            rate: rate(4, 60),
        },
        Command::StartOfflineModelProvision { .. } => CommandRule {
            id: CommandId::StartOfflineModelProvision,
            class: StateChanging,
            rate: rate(2, 60),
        },
        Command::CancelOfflineModelProvision { .. } => CommandRule {
            id: CommandId::CancelOfflineModelProvision,
            class: Destructive,
            rate: rate(8, 10),
        },
    }
}

#[derive(Debug, Clone, Copy)]
struct UsageWindow {
    opened: Instant,
    used: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PolicyRejection {
    pub(crate) code: IpcErrorCode,
    pub(crate) detail: &'static str,
}

/// One bounded, daemon-generation-owned policy instance shared by every IPC
/// connection handler. Buckets are keyed by exact command rather than client
/// role or PID, so reconnecting and role spoofing cannot reset a limit.
pub(crate) struct IpcCommandPolicy {
    expected_uid: u32,
    windows: Mutex<HashMap<CommandId, UsageWindow>>,
}

impl IpcCommandPolicy {
    pub(crate) fn new(expected_uid: u32) -> Self {
        Self {
            expected_uid,
            windows: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn authorize(
        &self,
        credentials: PeerCredentials,
        command: &Command,
    ) -> Result<(), PolicyRejection> {
        if credentials.uid != self.expected_uid {
            return Err(PolicyRejection {
                code: IpcErrorCode::PermissionDenied,
                detail: "IPC principal is not authorized",
            });
        }

        let rule = command_rule(command);
        let now = Instant::now();
        let mut windows = self
            .windows
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let usage = windows.entry(rule.id).or_insert(UsageWindow {
            opened: now,
            used: 0,
        });
        if now.saturating_duration_since(usage.opened) >= rule.rate.window {
            *usage = UsageWindow {
                opened: now,
                used: 0,
            };
        }
        if usage.used >= rule.rate.burst {
            return Err(PolicyRejection {
                code: IpcErrorCode::Busy,
                detail: match rule.class {
                    CommandSecurityClass::ReadOnly => "read-only command rate limit exceeded",
                    CommandSecurityClass::StateChanging => {
                        "state-changing command rate limit exceeded"
                    }
                    CommandSecurityClass::PrivacySensitive => {
                        "privacy-sensitive command rate limit exceeded"
                    }
                    CommandSecurityClass::Destructive => "destructive command rate limit exceeded",
                },
            });
        }
        usage.used = usage.used.saturating_add(1);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(uid: u32) -> PeerCredentials {
        PeerCredentials {
            uid,
            gid: uid,
            pid: Some(7),
        }
    }

    #[test]
    fn every_command_has_the_expected_security_class() {
        use CommandSecurityClass::{Destructive, PrivacySensitive, ReadOnly, StateChanging};

        let cases = [
            (Command::GetStatus, ReadOnly),
            (
                Command::StartSession {
                    pipeline_id: None,
                    activation: "test".to_owned(),
                    recognition: None,
                },
                StateChanging,
            ),
            (Command::StopSession, StateChanging),
            (
                Command::ToggleSession {
                    pipeline_id: None,
                    activation: narrivy_ipc::ActivationSourceDto::InstalledCliCommand,
                    recognition: None,
                },
                StateChanging,
            ),
            (Command::CancelSession, Destructive),
            (Command::Subscribe, PrivacySensitive),
            (Command::Unsubscribe, StateChanging),
            (Command::ListPipelines, ReadOnly),
            (
                Command::SelectPipeline {
                    pipeline_id: "test".to_owned(),
                },
                StateChanging,
            ),
            (Command::GetHistory, PrivacySensitive),
            (
                Command::GetHistoryEntry {
                    txid: "test".to_owned(),
                },
                PrivacySensitive,
            ),
            (
                Command::GetRecoveryEntry {
                    txid: "test".to_owned(),
                },
                PrivacySensitive,
            ),
            (
                Command::RunCapabilityCheck {
                    check_id: "test".to_owned(),
                },
                ReadOnly,
            ),
            (
                Command::RequestPortalKeyboardPermission {
                    operation_id: "test".to_owned(),
                    parent_window: "wayland:test".to_owned(),
                },
                StateChanging,
            ),
            (
                Command::CancelPortalKeyboardPermission {
                    operation_id: "test".to_owned(),
                },
                Destructive,
            ),
            (Command::RevokePortalKeyboardPermission, Destructive),
            (
                Command::StartOfflineModelProvision {
                    source_path: "/tmp/test".to_owned(),
                },
                StateChanging,
            ),
            (
                Command::CancelOfflineModelProvision {
                    operation_id: "test".to_owned(),
                },
                Destructive,
            ),
        ];

        assert_eq!(cases.len(), 18);
        for (command, expected) in cases {
            assert_eq!(command_rule(&command).class, expected);
        }
    }

    #[test]
    fn exact_command_limit_is_global_and_new_daemon_generation_resets_it() {
        let uid = 1000;
        let policy = IpcCommandPolicy::new(uid);
        let command = Command::StartOfflineModelProvision {
            source_path: "/tmp/test".to_owned(),
        };
        assert_eq!(policy.authorize(peer(uid), &command), Ok(()));
        assert_eq!(policy.authorize(peer(uid), &command), Ok(()));
        assert_eq!(
            policy.authorize(peer(uid), &command),
            Err(PolicyRejection {
                code: IpcErrorCode::Busy,
                detail: "state-changing command rate limit exceeded",
            })
        );

        // Another command has its own bounded bucket.
        assert_eq!(policy.authorize(peer(uid), &Command::GetStatus), Ok(()));

        // Policy state belongs to one daemon generation; an explicit daemon
        // restart creates a fresh policy while reconnect alone does not.
        let restarted = IpcCommandPolicy::new(uid);
        assert_eq!(restarted.authorize(peer(uid), &command), Ok(()));
    }

    #[test]
    fn foreign_uid_is_denied_without_consuming_same_uid_capacity() {
        let uid = 1000;
        let policy = IpcCommandPolicy::new(uid);
        let command = Command::StartOfflineModelProvision {
            source_path: "/tmp/test".to_owned(),
        };
        assert_eq!(
            policy.authorize(peer(uid + 1), &command),
            Err(PolicyRejection {
                code: IpcErrorCode::PermissionDenied,
                detail: "IPC principal is not authorized",
            })
        );
        assert_eq!(policy.authorize(peer(uid), &command), Ok(()));
        assert_eq!(policy.authorize(peer(uid), &command), Ok(()));
    }
}
