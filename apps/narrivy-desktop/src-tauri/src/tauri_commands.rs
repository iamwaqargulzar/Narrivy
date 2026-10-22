//! Current Tauri invoke wrappers.
//!
//! All daemon and pipeline behaviour stays in the framework-neutral
//! [`crate::bridge`] module. A maintained replacement shell can call that
//! module without reproducing protocol or persistence policy.
//!
//! Every command is `async` at the Tauri level: Tauri then runs it on its
//! runtime's worker threads instead of the main UI thread, because the bridge
//! makes bounded but blocking IPC calls (AGENTS.md: no blocking on UI threads).

#![forbid(unsafe_code)]

use crate::bridge;
use crate::theme::{ThemeMode, ThemeStore};
use tauri::Manager;

#[tauri::command(async)]
pub fn daemon_status() -> bridge::CmdResult<bridge::StatusSnapshot> {
    bridge::daemon_status_with_start()
}

#[tauri::command(async)]
pub fn daemon_start(
    pipeline_id: Option<String>,
    recognition: Option<narrivy_ipc::RecognitionSettingsDto>,
) -> bridge::CmdResult<()> {
    bridge::daemon_start(pipeline_id, recognition)
}

#[tauri::command(async)]
pub fn daemon_stop() -> bridge::CmdResult<()> {
    bridge::daemon_stop()
}

#[tauri::command(async)]
pub fn daemon_cancel() -> bridge::CmdResult<()> {
    bridge::daemon_cancel()
}

#[tauri::command(async)]
pub fn daemon_list_pipelines() -> bridge::CmdResult<Vec<narrivy_ipc::PipelineSummaryDto>> {
    bridge::daemon_list_pipelines()
}

#[tauri::command(async)]
pub fn daemon_history() -> bridge::CmdResult<Vec<bridge::JournalEntryDto>> {
    bridge::daemon_history()
}

#[tauri::command(async)]
pub fn daemon_history_entry(txid: String) -> bridge::CmdResult<bridge::HistoryEntryDetailDto> {
    bridge::daemon_history_entry(txid)
}

#[tauri::command(async)]
pub fn daemon_history_delete(txid: String) -> bridge::CmdResult<()> {
    bridge::daemon_history_delete(txid)
}

#[tauri::command(async)]
pub fn daemon_models() -> bridge::CmdResult<Vec<narrivy_ipc::ModelDto>> {
    bridge::daemon_models()
}

#[tauri::command(async)]
pub fn daemon_model_select(id: String) -> bridge::CmdResult<()> {
    bridge::daemon_model_select(id)
}

#[tauri::command(async)]
pub fn daemon_model_download(id: String) -> bridge::CmdResult<()> {
    bridge::daemon_model_download(id, false)
}

#[tauri::command(async)]
pub fn daemon_model_download_cancel(id: String) -> bridge::CmdResult<()> {
    bridge::daemon_model_download(id, true)
}

#[tauri::command(async)]
pub fn daemon_store_list() -> bridge::CmdResult<Vec<narrivy_ipc::StoreItemDto>> {
    bridge::daemon_store_list()
}

#[tauri::command(async)]
pub fn daemon_store_remove(id: String) -> bridge::CmdResult<()> {
    bridge::daemon_store_remove(id)
}

#[tauri::command(async)]
pub fn daemon_assistant_send(
    turn_id: String,
    messages: Vec<narrivy_ipc::AssistantMessageDto>,
    chat_id: Option<String>,
    speak: Option<bool>,
    pictures: Option<Vec<String>>,
) -> bridge::CmdResult<()> {
    bridge::daemon_assistant_send(
        turn_id,
        messages,
        chat_id,
        speak.unwrap_or(false),
        pictures.unwrap_or_default(),
    )
}

#[tauri::command(async)]
pub fn daemon_assistant_library(
    op: String,
    kind: String,
    id: Option<String>,
    value: Option<String>,
) -> bridge::CmdResult<String> {
    bridge::daemon_assistant_library(op, kind, id, value)
}

#[tauri::command(async)]
pub fn daemon_voice_mode(
    action: String,
    settings: Option<narrivy_ipc::VoiceSettingsDto>,
    down: Option<bool>,
) -> bridge::CmdResult<()> {
    bridge::daemon_voice_mode(action, settings, down)
}

#[tauri::command(async)]
pub fn daemon_assistant_cancel(turn_id: String) -> bridge::CmdResult<()> {
    bridge::daemon_assistant_cancel(turn_id)
}

#[tauri::command(async)]
pub fn daemon_audio_options() -> bridge::CmdResult<narrivy_ipc::AudioOptionsDto> {
    bridge::daemon_audio_options()
}

#[tauri::command(async)]
pub fn daemon_set_echo_cancel(on: bool) -> bridge::CmdResult<narrivy_ipc::AudioOptionsDto> {
    bridge::daemon_set_echo_cancel(on)
}

#[tauri::command(async)]
pub fn daemon_set_only_my_voice(on: bool) -> bridge::CmdResult<narrivy_ipc::AudioOptionsDto> {
    bridge::daemon_set_only_my_voice(on)
}

#[tauri::command(async)]
pub fn daemon_set_noise_suppression(level: String) -> bridge::CmdResult<narrivy_ipc::AudioOptionsDto> {
    bridge::daemon_set_noise_suppression(level)
}

#[tauri::command(async)]
pub fn app_autostart() -> crate::autostart::AutostartDto {
    crate::autostart::get()
}

#[tauri::command(async)]
pub fn app_set_autostart(
    enabled: bool,
    hidden: bool,
) -> Result<crate::autostart::AutostartDto, String> {
    crate::autostart::set(enabled, hidden)
}

#[tauri::command(async)]
pub fn daemon_vocabulary() -> bridge::CmdResult<narrivy_ipc::VocabularyDto> {
    bridge::daemon_vocabulary()
}

#[tauri::command(async)]
pub fn daemon_set_vocabulary(
    vocabulary: narrivy_ipc::VocabularyDto,
) -> bridge::CmdResult<narrivy_ipc::VocabularyDto> {
    bridge::daemon_set_vocabulary(vocabulary)
}

#[tauri::command(async)]
pub fn daemon_set_microphone(name: String) -> bridge::CmdResult<narrivy_ipc::AudioOptionsDto> {
    bridge::daemon_set_microphone(name)
}

#[tauri::command(async)]
pub fn daemon_hold_key() -> bridge::CmdResult<narrivy_ipc::HoldKeyDto> {
    bridge::daemon_hold_key()
}

#[tauri::command(async)]
pub fn daemon_set_hold_key(key: Option<u16>) -> bridge::CmdResult<narrivy_ipc::HoldKeyDto> {
    bridge::daemon_set_hold_key(key)
}

#[tauri::command(async)]
pub fn daemon_set_sounds(on: bool) -> bridge::CmdResult<narrivy_ipc::AudioOptionsDto> {
    bridge::daemon_set_sounds(on)
}

#[tauri::command(async)]
pub fn daemon_set_history_retention(retention: String) -> bridge::CmdResult<()> {
    bridge::daemon_set_history_retention(retention)
}

#[tauri::command(async)]
pub fn daemon_clear_history() -> bridge::CmdResult<()> {
    bridge::daemon_clear_history()
}

#[tauri::command(async)]
pub fn daemon_text_options() -> bridge::CmdResult<narrivy_ipc::TextOptionsDto> {
    bridge::daemon_text_options()
}

#[tauri::command(async)]
pub fn daemon_set_numbers_as_digits(on: bool) -> bridge::CmdResult<narrivy_ipc::TextOptionsDto> {
    bridge::daemon_set_numbers_as_digits(on)
}

#[tauri::command(async)]
pub fn daemon_set_pipeline_voice(
    pipeline_id: String,
    echo_cancel: Option<bool>,
    only_my_voice: Option<bool>,
) -> bridge::CmdResult<narrivy_ipc::AudioOptionsDto> {
    bridge::daemon_set_pipeline_voice(pipeline_id, echo_cancel, only_my_voice)
}

#[tauri::command(async)]
pub fn daemon_meeting_import(path: String) -> bridge::CmdResult<narrivy_ipc::MeetingDto> {
    bridge::daemon_meeting_import(path)
}

#[tauri::command(async)]
pub fn daemon_set_delivery_method(method: String) -> bridge::CmdResult<narrivy_ipc::TextOptionsDto> {
    bridge::daemon_set_delivery_method(method)
}

#[tauri::command(async)]
pub fn daemon_meeting_options() -> bridge::CmdResult<narrivy_ipc::MeetingOptionsDto> {
    bridge::daemon_meeting_options()
}

#[tauri::command(async)]
pub fn daemon_set_auto_notes(on: bool) -> bridge::CmdResult<narrivy_ipc::MeetingOptionsDto> {
    bridge::daemon_set_auto_notes(on)
}

#[tauri::command(async)]
pub fn daemon_set_process_on_stop(on: bool) -> bridge::CmdResult<narrivy_ipc::MeetingOptionsDto> {
    bridge::daemon_set_process_on_stop(on)
}

#[tauri::command(async)]
pub fn daemon_set_meeting_devices(
    microphone: Option<String>,
    speaker: Option<String>,
    call_audio: Option<bool>,
    room_audio: Option<bool>,
) -> bridge::CmdResult<narrivy_ipc::MeetingOptionsDto> {
    bridge::daemon_set_meeting_devices(microphone, speaker, call_audio, room_audio)
}

#[tauri::command(async)]
pub fn daemon_set_cleanup_idle(minutes: u32) -> bridge::CmdResult<narrivy_ipc::TextOptionsDto> {
    bridge::daemon_set_cleanup_idle(minutes)
}

#[tauri::command(async)]
pub fn daemon_set_voice_strictness(level: String) -> bridge::CmdResult<narrivy_ipc::AudioOptionsDto> {
    bridge::daemon_set_voice_strictness(level)
}

#[tauri::command(async)]
pub fn daemon_set_spoken_breaks(on: bool) -> bridge::CmdResult<narrivy_ipc::TextOptionsDto> {
    bridge::daemon_set_spoken_breaks(on)
}

#[tauri::command(async)]
pub fn daemon_train_voice() -> bridge::CmdResult<narrivy_ipc::AudioOptionsDto> {
    bridge::daemon_train_voice()
}

#[tauri::command(async)]
pub fn daemon_forget_voice() -> bridge::CmdResult<narrivy_ipc::AudioOptionsDto> {
    bridge::daemon_forget_voice()
}

#[tauri::command(async)]
pub fn daemon_screen_start(
    title: Option<String>,
    screen: bool,
    share: bool,
    microphone: bool,
    computer_sound: bool,
) -> bridge::CmdResult<narrivy_ipc::MeetingDto> {
    bridge::daemon_screen_start(title, screen, share, microphone, computer_sound)
}

#[tauri::command(async)]
pub fn daemon_meeting_start(
    title: Option<String>,
    watch: Option<bool>,
) -> bridge::CmdResult<narrivy_ipc::MeetingDto> {
    bridge::daemon_meeting_start(title, watch.unwrap_or(false))
}

#[tauri::command(async)]
pub fn daemon_meeting_stop() -> bridge::CmdResult<()> {
    bridge::daemon_meeting_stop()
}

#[tauri::command(async)]
pub fn daemon_meeting_sources(
    id: String,
    screen: Option<bool>,
    sound: Option<bool>,
    microphone: Option<bool>,
) -> bridge::CmdResult<narrivy_ipc::MeetingDto> {
    bridge::daemon_meeting_sources(id, screen, sound, microphone)
}

#[tauri::command(async)]
pub fn daemon_meeting_control(id: String, action: String) -> bridge::CmdResult<()> {
    bridge::daemon_meeting_control(id, action)
}

#[tauri::command(async)]
pub fn daemon_meetings() -> bridge::CmdResult<Vec<narrivy_ipc::MeetingDto>> {
    bridge::daemon_meetings()
}

#[tauri::command(async)]
pub fn daemon_meeting(id: String) -> bridge::CmdResult<narrivy_ipc::MeetingDetailDto> {
    bridge::daemon_meeting(id)
}

#[tauri::command(async)]
pub fn daemon_meeting_process(
    id: String,
    model: Option<String>,
    room_speakers: Option<u16>,
    call_speakers: Option<u16>,
) -> bridge::CmdResult<()> {
    bridge::daemon_meeting_process(id, model, room_speakers, call_speakers)
}

#[tauri::command(async)]
pub fn daemon_meeting_rename(id: String, speaker: String, name: String) -> bridge::CmdResult<()> {
    bridge::daemon_meeting_rename(id, speaker, name)
}

#[tauri::command(async)]
pub fn daemon_meeting_delete(id: String) -> bridge::CmdResult<()> {
    bridge::daemon_meeting_delete(id)
}

#[tauri::command(async)]
pub fn daemon_meeting_summarize(
    id: String,
    turn_id: String,
    kind: String,
    instruction: Option<String>,
) -> bridge::CmdResult<()> {
    bridge::daemon_meeting_summarize(id, turn_id, kind, instruction)
}

/// Open a meeting's folder in the file manager, or one of its files
/// (`recording.wav`, `transcript.md`, `transcript.srt`) in the default app.
#[tauri::command(async)]
pub fn open_meeting_folder(path: String, file: Option<String>) -> bridge::CmdResult<()> {
    // Resolved first, so `..` cannot step outside the meetings folder.
    let dir = std::path::PathBuf::from(&path)
        .canonicalize()
        .map_err(|_| bridge::BridgeError::new("bad_request", "not a meeting folder"))?;
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share"))
        })
        .ok_or_else(|| bridge::BridgeError::new("internal", "no home folder"))?;
    let meetings_root = data
        .join("narrivy")
        .join("meetings")
        .canonicalize()
        .map_err(|_| bridge::BridgeError::new("bad_request", "no meetings yet"))?;
    // Only folders of Narrivy's own meetings are opened.
    if !dir.starts_with(&meetings_root) || !dir.is_dir() {
        return Err(bridge::BridgeError::new(
            "bad_request",
            "not a meeting folder",
        ));
    }
    let target = match file.as_deref() {
        None => dir,
        Some(name @ ("recording.wav" | "transcript.md" | "transcript.srt")) => dir.join(name),
        Some(_) => {
            return Err(bridge::BridgeError::new(
                "bad_request",
                "not a meeting file",
            ));
        }
    };
    std::process::Command::new("xdg-open")
        .arg(target)
        .spawn()
        .map(|_| ())
        .map_err(|_| bridge::BridgeError::new("internal", "could not open the folder"))
}

#[tauri::command(async)]
pub fn daemon_pipeline_engine_set(
    pipeline_id: String,
    model_id: Option<String>,
    compute: String,
) -> bridge::CmdResult<()> {
    bridge::daemon_pipeline_engine_set(pipeline_id, model_id, compute)
}

#[tauri::command(async)]
pub fn daemon_shortcuts_list() -> bridge::CmdResult<narrivy_ipc::ShortcutsDto> {
    bridge::daemon_shortcuts_list()
}

#[tauri::command(async)]
pub fn daemon_shortcut_set(
    binding: narrivy_ipc::ShortcutBindingDto,
    replace: bool,
) -> bridge::CmdResult<narrivy_ipc::ShortcutSetDto> {
    bridge::daemon_shortcut_set(binding, replace)
}

#[tauri::command(async)]
pub fn daemon_shortcut_remove(modifiers: Vec<String>, key: String) -> bridge::CmdResult<()> {
    bridge::daemon_shortcut_remove(modifiers, key)
}

#[tauri::command(async)]
pub fn daemon_recovery_entry(txid: String) -> bridge::CmdResult<bridge::RecoveryEntryDetailDto> {
    bridge::daemon_recovery_entry(txid)
}

#[tauri::command(async)]
pub fn daemon_capability_check(id: String) -> bridge::CmdResult<bridge::CapabilityCheckResult> {
    bridge::daemon_capability_check(id)
}

#[tauri::command(async)]
pub fn pipelines_export(path: String) -> bridge::CmdResult<bridge::PipelinesExportReport> {
    bridge::pipelines_export(path)
}

#[tauri::command(async)]
pub fn pipelines_import(
    path: String,
    dry_run: bool,
) -> bridge::CmdResult<bridge::PipelinesImportReport> {
    bridge::pipelines_import(path, dry_run)
}

#[tauri::command(async)]
pub fn theme_get(app: tauri::AppHandle) -> Result<ThemeMode, String> {
    let store = ThemeStore::resolve(app.path().app_config_dir().ok());
    Ok(store.load())
}

#[tauri::command(async)]
pub fn theme_set(app: tauri::AppHandle, mode: ThemeMode) -> Result<(), String> {
    let store = ThemeStore::resolve(app.path().app_config_dir().ok());
    store.save(mode)
}

/// The app window reports its resolved theme (`dark` or `light`), accent
/// (`mono`, `blue` or `gold`) and the overlay position chosen in Settings
/// (`top-left` … `bottom-right`), so the native overlay card matches them. On
/// hosts whose overlay is a web view this does nothing: that view styles and
/// places itself.
#[tauri::command(async)]
pub fn overlay_appearance(
    app: tauri::AppHandle,
    theme: String,
    accent: String,
    position: String,
) -> Result<(), String> {
    let appearance = crate::overlay_theme::Appearance::parse(&theme, &accent)
        .ok_or_else(|| "unknown overlay appearance".to_owned())?;
    let position = crate::overlay_theme::Position::parse(&position)
        .ok_or_else(|| "unknown overlay position".to_owned())?;
    #[cfg(target_os = "linux")]
    {
        crate::linux_overlay::set_appearance(&app, appearance);
        crate::linux_overlay::set_position(&app, position);
    }
    #[cfg(not(target_os = "linux"))]
    let _ = (app, appearance, position);
    Ok(())
}

#[tauri::command(async)]
pub fn model_install_bundled() -> bridge::CmdResult<bridge::ModelProvisionStatusDto> {
    bridge::model_install_bundled()
}

#[tauri::command(async)]
pub fn model_install_cancel(
    operation_id: String,
) -> bridge::CmdResult<bridge::ModelProvisionStatusDto> {
    bridge::model_install_cancel(operation_id)
}

#[tauri::command(async)]
pub fn daemon_language_models() -> bridge::CmdResult<Vec<narrivy_ipc::LanguageModelDto>> {
    bridge::daemon_language_models()
}

#[tauri::command(async)]
pub fn daemon_language_model_select(role: String, id: Option<String>) -> bridge::CmdResult<()> {
    bridge::daemon_language_model_select(role, id)
}

#[tauri::command(async)]
pub fn daemon_assistant_unload() -> bridge::CmdResult<()> {
    bridge::daemon_assistant_unload()
}

#[tauri::command(async)]
pub fn daemon_set_meeting_screen(on: bool) -> bridge::CmdResult<narrivy_ipc::MeetingOptionsDto> {
    bridge::daemon_set_meeting_screen(on)
}

#[tauri::command(async)]
pub fn daemon_meeting_slide(id: String, file: String) -> bridge::CmdResult<String> {
    bridge::daemon_meeting_slide(id, file)
}

#[tauri::command(async)]
pub fn daemon_assistant_tool_decision(call_id: String, allow: bool, always: bool) -> bridge::CmdResult<()> {
    bridge::daemon_assistant_tool_decision(call_id, allow, always)
}

/// Ask the user for a folder with the desktop's own chooser (the XDG
/// portal). `None` when they cancel.
#[tauri::command]
pub async fn pick_folder(title: String) -> bridge::CmdResult<Option<String>> {
    #[cfg(target_os = "linux")]
    {
        use ashpd::desktop::file_chooser::SelectedFiles;
        let request = SelectedFiles::open_file()
            .title(title.as_str())
            .directory(true)
            .modal(true)
            .send()
            .await
            .map_err(|_| bridge::BridgeError::new("folder_chooser_unavailable", "the desktop's folder chooser could not open"))?;
        Ok(request
            .response()
            .ok()
            .and_then(|files| files.uris().first().and_then(|uri| uri.to_file_path().ok()))
            .map(|path| path.display().to_string()))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = title;
        Err(bridge::BridgeError::new("folder_chooser_unavailable", "no folder chooser on this system"))
    }
}
