//! The AI assistant extension's voice mode (D33, F24): a spoken
//! conversation with the assistant.
//!
//! `scripts/voice_mode.py` (run by the Python named by
//! `NARRIVY_MEETING_PYTHON`, path from `NARRIVY_VOICE_MODE_SCRIPT`) listens,
//! finds the end of each turn, transcribes it and speaks the replies. The
//! daemon starts and stops it, passes on what it hears as `Voice` events,
//! feeds it the pieces of spoken replies as the model writes them, and
//! cancels a reply the user talks over. Its settings are kept in
//! `<state>/voice-mode.json`. Nothing it hears is logged or saved here; a
//! question asked by voice is saved in its chat like a typed one.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};

use narrivy_ipc::{Event, VoiceSettingsDto, VoiceTimingDto};
use serde::{Deserialize, Serialize};

use super::session::{DaemonEvent, DaemonState};

pub const SCRIPT_ENV: &str = "NARRIVY_VOICE_MODE_SCRIPT";
/// The device Whisper runs on for voice mode (default `GPU`).
pub const DEVICE_ENV: &str = "NARRIVY_VOICE_DEVICE";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    speed_percent: u16,
    push_to_talk: bool,
    interrupt: String,
    sounds: bool,
    voice: u16,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            speed_percent: 100,
            push_to_talk: false,
            interrupt: "stop".to_owned(),
            sounds: true,
            // Kokoro's "af_heart".
            voice: 3,
        }
    }
}

struct Running {
    child: Child,
    stdin: ChildStdin,
    /// The spoken reply being written, to cancel when the user talks over it.
    turn: Option<String>,
}

static RUNNING: Mutex<Option<Running>> = Mutex::new(None);

fn settings_path() -> Option<PathBuf> {
    narrivy_platform_linux::instance::state_dir()
        .ok()
        .map(|dir| dir.join("voice-mode.json"))
}

fn settings() -> Settings {
    settings_path()
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save(settings: &Settings) {
    let Some(path) = settings_path() else {
        return;
    };
    let Ok(bytes) = serde_json::to_vec(settings) else {
        return;
    };
    let part = path.with_extension("part");
    if std::fs::write(&part, bytes).is_ok() {
        let _ = std::fs::rename(part, path);
    }
}

fn publish(state: &DaemonState, event: Event) {
    state.publish_assistant(DaemonEvent::Extension(event));
}

#[derive(Default)]
struct VoiceEvent {
    state: Option<String>,
    text: Option<String>,
    language: Option<String>,
    timing: Option<VoiceTimingDto>,
    code: Option<String>,
}

fn voice(kind: &str, event: VoiceEvent) -> Event {
    Event::Voice {
        kind: kind.to_owned(),
        state: event.state,
        text: event.text,
        language: event.language,
        timing: event.timing,
        code: event.code,
    }
}

fn voice_event(kind: &str) -> Event {
    voice(kind, VoiceEvent::default())
}

fn state_event(state: &str) -> Event {
    voice(
        "state",
        VoiceEvent {
            state: Some(state.to_owned()),
            ..VoiceEvent::default()
        },
    )
}

/// Handle a `VoiceMode` request.
pub fn handle(
    action: &str,
    update: Option<VoiceSettingsDto>,
    down: Option<bool>,
    state: &Arc<DaemonState>,
) -> Result<(), &'static str> {
    match action {
        "start" => {
            if let Some(update) = update {
                apply(&update)?;
            }
            start(state)
        }
        "stop" => {
            stop(state);
            Ok(())
        }
        "settings" => {
            let update = update.ok_or("voice_settings_missing")?;
            apply(&update)?;
            let mut command = serde_json::json!({"op": "settings"});
            let current = settings();
            command["speed"] = (f64::from(current.speed_percent) / 100.0).into();
            command["push_to_talk"] = current.push_to_talk.into();
            command["interrupt"] = current.interrupt.into();
            command["sounds"] = current.sounds.into();
            command["voice"] = current.voice.into();
            if let Some(muted) = update.muted {
                command["muted"] = muted.into();
            }
            send(&command);
            Ok(())
        }
        "ptt" => {
            send(&serde_json::json!({"op": "ptt", "down": down.unwrap_or(false)}));
            Ok(())
        }
        _ => Err("voice_action_invalid"),
    }
}

/// The saved settings, with `update` applied.
fn apply(update: &VoiceSettingsDto) -> Result<(), &'static str> {
    let mut current = settings();
    if let Some(speed) = update.speed_percent {
        if !(100..=175).contains(&speed) {
            return Err("voice_speed_invalid");
        }
        current.speed_percent = speed;
    }
    if let Some(on) = update.push_to_talk {
        current.push_to_talk = on;
    }
    if let Some(interrupt) = &update.interrupt {
        if !matches!(interrupt.as_str(), "stop" | "queue") {
            return Err("voice_interrupt_invalid");
        }
        current.interrupt.clone_from(interrupt);
    }
    if let Some(on) = update.sounds {
        current.sounds = on;
    }
    if let Some(voice) = update.voice {
        if voice > 52 {
            return Err("voice_invalid");
        }
        current.voice = voice;
    }
    save(&current);
    Ok(())
}

/// The voice settings, for clients.
#[must_use]
pub fn settings_json() -> String {
    let current = settings();
    let running = RUNNING.lock().ok().is_some_and(|r| r.is_some());
    serde_json::json!({
        "speedPercent": current.speed_percent,
        "pushToTalk": current.push_to_talk,
        "interrupt": current.interrupt,
        "sounds": current.sounds,
        "voice": current.voice,
        "running": running,
        "available": available(),
    })
    .to_string()
}

/// Whether voice mode can run here: its worker and a speaking voice exist.
fn available() -> bool {
    std::env::var_os(super::meetings::PYTHON_ENV).is_some()
        && std::env::var_os(SCRIPT_ENV).is_some()
        && voice_models_installed()
}

fn voice_models_installed() -> bool {
    std::env::var_os("HOME").is_some_and(|home| {
        let dir = PathBuf::from(home).join(".cache/narrivy/tts");
        dir.join("kokoro-multi-lang-v1_0/model.onnx").is_file()
            || dir.join("vits-piper-ur_PK-fasih-medium").is_dir()
    })
}

fn start(state: &Arc<DaemonState>) -> Result<(), &'static str> {
    let mut slot = RUNNING.lock().map_err(|_| "voice_unavailable")?;
    if let Some(running) = slot.as_mut()
        && running.child.try_wait().ok().flatten().is_none()
    {
        return Ok(());
    }
    let python = std::env::var_os(super::meetings::PYTHON_ENV).ok_or("voice_worker_missing")?;
    let script = std::env::var_os(SCRIPT_ENV).ok_or("voice_worker_missing")?;
    if !voice_models_installed() {
        return Err("voice_model_missing");
    }
    let installed = super::meetings::installed_models();
    let model = installed
        .iter()
        .find(|m| m.as_str() == "turbo")
        .or_else(|| installed.first())
        .and_then(|m| super::meetings::model_path(m))
        .ok_or("voice_speech_model_missing")?;
    let current = settings();
    let echo_cancelled = super::echo::source();
    let microphone = echo_cancelled
        .map(str::to_owned)
        .or_else(super::echo::chosen_microphone);
    let device = std::env::var(DEVICE_ENV).unwrap_or_else(|_| "GPU".to_owned());
    if !matches!(device.as_str(), "GPU" | "CPU" | "NPU") {
        return Err("voice_device_invalid");
    }
    let mut command = Command::new(python);
    command
        .arg(script)
        .arg("--model")
        .arg(model)
        .args(["--device", &device])
        .args([
            "--speed",
            &format!("{:.2}", f64::from(current.speed_percent) / 100.0),
        ])
        .args(["--speaker", &current.voice.to_string()])
        .args(["--interrupt", &current.interrupt]);
    if let Some(microphone) = &microphone {
        command.args(["--mic", microphone]);
    }
    if echo_cancelled.is_some() {
        command.arg("--echo-cancelled");
    }
    if current.push_to_talk {
        command.arg("--push-to-talk");
    }
    if current.sounds {
        command.arg("--sounds");
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "voice_worker_spawn_failed")?;
    let stdin = child.stdin.take().ok_or("voice_worker_spawn_failed")?;
    let stdout = child.stdout.take().ok_or("voice_worker_spawn_failed")?;
    let pid = child.id();
    *slot = Some(Running {
        child,
        stdin,
        turn: None,
    });
    drop(slot);
    publish(state, state_event("starting"));
    let state = Arc::clone(state);
    std::thread::Builder::new()
        .name("narrivy-voice-mode".into())
        .spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                on_worker_line(&line, &state);
            }
            // The worker ended (stopped, or failed).
            let mine = RUNNING
                .lock()
                .ok()
                .is_some_and(|r| r.as_ref().is_some_and(|r| r.child.id() == pid));
            if mine {
                stop(&state);
            }
        })
        .map_err(|_| "voice_unavailable")?;
    Ok(())
}

fn on_worker_line(line: &str, state: &Arc<DaemonState>) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        return;
    };
    let text_of = |key: &str| value.get(key).and_then(|v| v.as_str()).map(str::to_owned);
    match value.get("ev").and_then(|v| v.as_str()).unwrap_or_default() {
        "state" => {
            if let Some(now) = text_of("state") {
                publish(state, state_event(&now));
            }
        }
        "partial" => publish(
            state,
            voice(
                "partial",
                VoiceEvent {
                    text: text_of("text"),
                    ..VoiceEvent::default()
                },
            ),
        ),
        "heard" => publish(
            state,
            voice(
                "heard",
                VoiceEvent {
                    text: text_of("text"),
                    language: text_of("language"),
                    ..VoiceEvent::default()
                },
            ),
        ),
        "interrupt" => {
            // The user talked over the reply: stop writing it.
            let turn = RUNNING
                .lock()
                .ok()
                .and_then(|mut r| r.as_mut().and_then(|r| r.turn.take()));
            if let Some(turn) = turn
                && let Ok(assistant) = super::assistant::shared(state)
            {
                assistant.cancel(&turn);
            }
            publish(state, voice_event("interrupt"));
        }
        "repeat" => publish(state, voice_event("repeat")),
        "timing" => {
            let ms = |key: &str| {
                value
                    .get(key)
                    .and_then(serde_json::Value::as_u64)
                    .map_or(0, |v| u32::try_from(v).unwrap_or(u32::MAX))
            };
            publish(
                state,
                voice(
                    "timing",
                    VoiceEvent {
                        timing: Some(VoiceTimingDto {
                            asr_ms: ms("asr_ms"),
                            reply_ms: ms("reply_ms"),
                            voice_ms: ms("voice_ms"),
                            total_ms: ms("total_ms"),
                        }),
                        ..VoiceEvent::default()
                    },
                ),
            );
        }
        "error" => publish(
            state,
            voice(
                "error",
                VoiceEvent {
                    code: text_of("code"),
                    ..VoiceEvent::default()
                },
            ),
        ),
        _ => {}
    }
}

fn stop(state: &Arc<DaemonState>) {
    let running = RUNNING.lock().ok().and_then(|mut r| r.take());
    if let Some(mut running) = running {
        let _ = writeln!(running.stdin, "{}", serde_json::json!({"op": "quit"}));
        let _ = running.stdin.flush();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(800));
            let _ = running.child.kill();
            let _ = running.child.wait();
        });
        publish(state, state_event("stopped"));
    }
}

fn send(command: &serde_json::Value) {
    if let Ok(mut running) = RUNNING.lock()
        && let Some(running) = running.as_mut()
    {
        let _ = writeln!(running.stdin, "{command}");
        let _ = running.stdin.flush();
    }
}

/// A spoken reply starts: its pieces are read aloud.
pub fn reply_started(turn_id: &str) {
    if let Ok(mut running) = RUNNING.lock()
        && let Some(running) = running.as_mut()
    {
        running.turn = Some(turn_id.to_owned());
    }
}

/// A piece of the spoken reply `turn_id`.
pub fn reply_piece(turn_id: &str, piece: &str) {
    if current_turn().as_deref() == Some(turn_id) {
        send(&serde_json::json!({"op": "say", "text": piece}));
    }
}

/// The spoken reply `turn_id` ended (`ok`, `cancelled` or `failed:…`).
pub fn reply_done(turn_id: &str, outcome: &str) {
    if current_turn().as_deref() != Some(turn_id) {
        return;
    }
    if let Ok(mut running) = RUNNING.lock()
        && let Some(running) = running.as_mut()
    {
        running.turn = None;
    }
    if outcome == "ok" {
        send(&serde_json::json!({"op": "end"}));
    } else {
        send(&serde_json::json!({"op": "stop"}));
    }
}

fn current_turn() -> Option<String> {
    RUNNING
        .lock()
        .ok()
        .and_then(|r| r.as_ref().and_then(|r| r.turn.clone()))
}
