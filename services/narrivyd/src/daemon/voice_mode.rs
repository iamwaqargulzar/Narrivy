//! The AI assistant extension's voice mode (D33, F24): a spoken
//! conversation with the assistant.
//!
//! `narrivy-voice-worker` (`engines/voice-sherpa`, installed beside the
//! daemon and bound by its own manifest) listens, finds the end of each
//! turn and speaks the replies with a voice from the Store
//! ([`super::voices`]). Each turn's audio comes back here and is recognised
//! with the speech model already loaded for dictation. The daemon starts
//! and stops the worker, passes on what it hears as `Voice` events, feeds
//! it the pieces of spoken replies as the model writes them, and cancels a
//! reply the user talks over. Its settings are kept in
//! `<state>/voice-mode.json`. Nothing it hears is logged or saved here; a
//! question asked by voice is saved in its chat like a typed one.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

use narrivy_ipc::{Event, VoiceSettingsDto, VoiceTimingDto};
use narrivy_voice_sherpa::wire::{self, Kind};
use serde::{Deserialize, Serialize};

use super::host::process::{HelperEnvironment, VerifiedArtifactExecutable};
use super::session::{DaemonEvent, DaemonState};

/// A developer worker instead of the installed one, bound by the digest and
/// size its environment names.
pub const WORKER_ENV: &str = "NARRIVY_VOICE_WORKER";
pub const WORKER_SHA256_ENV: &str = "NARRIVY_VOICE_WORKER_SHA256";
pub const WORKER_SIZE_ENV: &str = "NARRIVY_VOICE_WORKER_SIZE";
/// A developer copy of the Silero detector (checked against the digest
/// below like the installed one).
pub const DETECTOR_ENV: &str = "NARRIVY_VOICE_DETECTOR";
/// The developer Urdu speaker, its SHA-256 and its size.
pub const URDU_ENV: &str = "NARRIVY_URDU_SPEAKER";
pub const URDU_SHA256_ENV: &str = "NARRIVY_URDU_SPEAKER_SHA256";
pub const URDU_SIZE_ENV: &str = "NARRIVY_URDU_SPEAKER_SIZE";
/// Silero VAD v5 (`silero_vad.onnx`, MIT), as the package installs it.
const DETECTOR_SHA256: &str = "9e2449e1087496d8d4caba907f23e0bd3f78d91fa552479bb9c23ac09cbb1fd6";
const DETECTOR_SIZE: u64 = 643_854;

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
    /// Turns to recognise, in order.
    recognise: Sender<(u64, Kind, Vec<f32>)>,
}

static RUNNING: Mutex<Option<Running>> = Mutex::new(None);

fn settings_path() -> Option<PathBuf> {
    super::host::state_dir()
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

/// The worker executable: the developer one named by [`WORKER_ENV`], else
/// the one the package installs, bound through its own manifest.
fn worker() -> Result<VerifiedArtifactExecutable, &'static str> {
    let Some(path) = std::env::var_os(WORKER_ENV) else {
        return super::host::linux_features::installed_runtime::resolve_installed_voice_worker()
            .map_err(|error| {
                if error
                    == super::host::linux_features::installed_runtime::InstalledRuntimeError::ManifestMissing
                {
                    "voice_worker_missing"
                } else {
                    "voice_worker_invalid"
                }
            });
    };
    let digest = std::env::var(WORKER_SHA256_ENV).map_err(|_| "voice_worker_missing")?;
    let size = std::env::var(WORKER_SIZE_ENV)
        .ok()
        .and_then(|size| size.parse::<u64>().ok())
        .ok_or("voice_worker_missing")?;
    VerifiedArtifactExecutable::bind_controlled(Path::new(&path), &digest, size)
        .map_err(|_| "voice_worker_invalid")
}

/// The Silero detector, checked against its digest.
fn detector() -> Option<PathBuf> {
    let path = std::env::var_os(DETECTOR_ENV).map_or_else(
        super::host::linux_features::installed_runtime::installed_voice_detector,
        PathBuf::from,
    );
    let meta = std::fs::symlink_metadata(&path).ok()?;
    (meta.is_file()
        && meta.len() == DETECTOR_SIZE
        && narrivy_model_manager::store::sha256_file(&path)
            .is_ok_and(|digest| digest.eq_ignore_ascii_case(DETECTOR_SHA256)))
    .then_some(path)
}

/// The longest wait for the worker to measure a dictation's speech.
const MEASURE_BOUND: std::time::Duration = std::time::Duration::from_secs(5);

/// Milliseconds of speech Silero finds in a dictation's audio (16 kHz
/// mono), measured by the voice worker (`--measure-speech`) with the
/// packaged detector; the dictation's no-speech check ([`super::voice`]).
/// `None` when the worker or detector is missing, the audio is in another
/// format, or the worker fails or takes longer than [`MEASURE_BOUND`]: the
/// caller then falls back to its energy measure. The audio goes to the
/// worker through a pipe and nothing of it is kept or logged.
#[must_use]
pub fn speech_ms(audio: &narrivy_types::AudioChunk) -> Option<u64> {
    if audio.sample_rate_hz != narrivy_voice_sherpa::RATE
        || audio.channels != 1
        || audio.samples.is_empty()
    {
        return None;
    }
    let executable = worker().ok()?;
    let detector = detector()?;
    let mut command = executable.command(HelperEnvironment::ResidentAsr).ok()?;
    command
        .arg("--measure-speech")
        .arg("--detector")
        .arg(&detector)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // Started on the caller's thread, which waits for it below.
    let mut child = command.spawn().ok()?;
    let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    };
    let mut bytes = Vec::with_capacity(audio.samples.len() * 4);
    for sample in &audio.samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    let (tx, rx) = mpsc::channel();
    let io = std::thread::Builder::new()
        .name("narrivy-speech-measure".into())
        .spawn(move || {
            // The worker reads everything before it answers, so writing
            // all of it first cannot deadlock.
            let written = stdin.write_all(&bytes).and_then(|()| stdin.flush());
            drop(stdin);
            let mut line = String::new();
            let read = BufReader::new(stdout).read_line(&mut line);
            let _ = tx.send((written.is_ok() && read.is_ok()).then_some(line));
        });
    let line = io
        .ok()
        .and_then(|_| rx.recv_timeout(MEASURE_BOUND).ok())
        .flatten();
    // Ended either way; a worker past its bound is killed, which closes
    // its pipes and ends the reading thread.
    let _ = child.kill();
    let _ = child.wait();
    let reply: serde_json::Value = serde_json::from_str(line?.trim()).ok()?;
    (reply.get("ev")?.as_str()? == "speech")
        .then(|| reply.get("ms")?.as_u64())
        .flatten()
}

/// The Urdu speaker program (GPL, D51): the developer one named by
/// [`URDU_ENV`] (checked against [`URDU_SHA256_ENV`] and [`URDU_SIZE_ENV`]),
/// else the packaged one, bound through its own manifest. The voice worker
/// starts it by this path.
fn urdu_speaker() -> Option<PathBuf> {
    let Some(path) = std::env::var_os(URDU_ENV) else {
        return super::host::linux_features::installed_runtime::resolve_installed_urdu_speaker()
            .ok();
    };
    let path = PathBuf::from(path);
    let digest = std::env::var(URDU_SHA256_ENV).ok()?;
    let size = std::env::var(URDU_SIZE_ENV).ok()?.parse::<u64>().ok()?;
    let meta = std::fs::symlink_metadata(&path).ok()?;
    (path.is_absolute()
        && meta.is_file()
        && meta.len() == size
        && narrivy_model_manager::store::sha256_file(&path)
            .is_ok_and(|actual| actual.eq_ignore_ascii_case(&digest)))
    .then_some(path)
}

fn models_dir() -> Option<PathBuf> {
    super::model_selection::models_root_from_env()
}

/// Whether a voice-mode session is running now.
#[must_use]
pub fn running() -> bool {
    RUNNING.lock().is_ok_and(|running| running.is_some())
}

/// Whether voice mode can run here: its worker, its detector and a voice.
fn available() -> bool {
    worker().is_ok()
        && detector().is_some()
        && models_dir().is_some_and(|models| super::voices::any_installed(&models))
}

fn start(state: &Arc<DaemonState>) -> Result<(), &'static str> {
    let mut slot = RUNNING.lock().map_err(|_| "voice_unavailable")?;
    if let Some(running) = slot.as_mut()
        && running.child.try_wait().ok().flatten().is_none()
    {
        return Ok(());
    }
    let executable = worker()?;
    let detector = detector().ok_or("voice_detector_missing")?;
    let models = models_dir().ok_or("voice_unavailable")?;
    let english = super::voices::english(&models).ok_or("voice_model_missing")?;
    if !speech_model_loaded(state) {
        return Err("voice_speech_model_missing");
    }
    let current = settings();
    // The worker records with its own capture: on Windows it cannot open
    // the in-process echo-cancelled microphone, so it gets the chosen one.
    #[cfg(not(windows))]
    let echo_cancelled = super::echo::source();
    #[cfg(windows)]
    let echo_cancelled: Option<&str> = None;
    let microphone = echo_cancelled
        .map(str::to_owned)
        .or_else(super::echo::chosen_microphone);
    let mut command = executable
        .command(HelperEnvironment::ResidentAsr)
        .map_err(|_| "voice_worker_invalid")?;
    // The worker starts with an empty environment; only PipeWire's session
    // routing is passed on, for its pw-record and pw-play.
    for key in narrivy_voice_sherpa::ROUTING_KEYS {
        if let Some(value) = std::env::var_os(key).and_then(|v| v.into_string().ok())
            && !value.is_empty()
        {
            command.arg("--env").arg(format!("{key}={value}"));
        }
    }
    command
        .arg("--english")
        .arg(&english)
        .arg("--detector")
        .arg(&detector)
        .args([
            "--speed",
            &format!("{:.2}", f64::from(current.speed_percent) / 100.0),
        ])
        .args(["--speaker", &current.voice.to_string()])
        .args(["--interrupt", &current.interrupt]);
    if let Some(microphone) = &microphone {
        command.args(["--mic", microphone]);
    }
    // Urdu replies are read by the Urdu speaker when it and its voice are
    // both installed; otherwise English reads everything.
    if let (Some(program), Some(voice)) = (urdu_speaker(), super::voices::urdu(&models)) {
        command
            .arg("--urdu-speaker")
            .arg(program)
            .arg("--urdu-voice")
            .arg(voice);
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
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // The worker is started on the thread that reads it and lives as long
    // as it does: the launcher asks the kernel to kill a worker when the
    // thread that started it ends, so a short-lived request thread must not
    // start it.
    let (started, spawned) = mpsc::channel::<Result<(Child, ChildStdin), &'static str>>();
    let reader_state = Arc::clone(state);
    std::thread::Builder::new()
        .name("narrivy-voice-mode".into())
        .spawn(move || {
            let mut child = match command.spawn() {
                Ok(child) => child,
                Err(_) => {
                    let _ = started.send(Err("voice_worker_spawn_failed"));
                    return;
                }
            };
            let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
                let _ = child.kill();
                let _ = child.wait();
                let _ = started.send(Err("voice_worker_spawn_failed"));
                return;
            };
            let pid = child.id();
            if started.send(Ok((child, stdin))).is_err() {
                return;
            }
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                on_worker_line(&line, &reader_state);
            }
            // The worker ended (stopped, or failed).
            let mine = RUNNING
                .lock()
                .ok()
                .is_some_and(|r| r.as_ref().is_some_and(|r| r.child.id() == pid));
            if mine {
                stop(&reader_state);
            }
        })
        .map_err(|_| "voice_unavailable")?;
    let (child, stdin) = spawned.recv().map_err(|_| "voice_worker_spawn_failed")??;
    let (recognise, turns) = mpsc::channel::<(u64, Kind, Vec<f32>)>();
    *slot = Some(Running {
        child,
        stdin,
        turn: None,
        recognise,
    });
    drop(slot);
    publish(state, state_event("starting"));
    let recogniser_state = Arc::clone(state);
    std::thread::Builder::new()
        .name("narrivy-voice-asr".into())
        .spawn(move || {
            for (id, kind, samples) in turns {
                let started = std::time::Instant::now();
                let heard = recognise_turn(&recogniser_state, samples).unwrap_or_default();
                send(&serde_json::json!({
                    "op": "asr", "id": id, "kind": kind.as_str(), "text": heard,
                    "ms": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                }));
            }
        })
        .map_err(|_| "voice_unavailable")?;
    Ok(())
}

/// Whether a speech model is loaded to recognise turns with.
fn speech_model_loaded(state: &DaemonState) -> bool {
    #[cfg(feature = "asr-transcribe-resident")]
    {
        state.resident_asr().is_some()
    }
    #[cfg(not(feature = "asr-transcribe-resident"))]
    {
        let _ = state;
        false
    }
}

/// One turn, recognised with the loaded speech model.
fn recognise_turn(state: &DaemonState, samples: Vec<f32>) -> Option<String> {
    #[cfg(feature = "asr-transcribe-resident")]
    {
        use narrivy_asr_api::OneShotAsrPort as _;
        let mut engine = state.resident_asr()?;
        let audio = narrivy_types::AudioChunk {
            sample_rate_hz: narrivy_voice_sherpa::RATE,
            channels: 1,
            samples,
        };
        engine
            .recognize(
                narrivy_types::SessionId(0),
                &audio,
                &narrivy_types::SessionCancellation::new(),
            )
            .ok()
            .map(|transcript| transcript.text)
    }
    #[cfg(not(feature = "asr-transcribe-resident"))]
    {
        let _ = (state, samples);
        None
    }
}

fn on_worker_line(line: &str, state: &Arc<DaemonState>) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        return;
    };
    let text_of = |key: &str| value.get(key).and_then(|v| v.as_str()).map(str::to_owned);
    match value.get("ev").and_then(|v| v.as_str()).unwrap_or_default() {
        "audio" => {
            let id = value.get("id").and_then(serde_json::Value::as_u64);
            let kind = value
                .get("kind")
                .and_then(|v| v.as_str())
                .and_then(Kind::parse);
            let samples = value
                .get("pcm")
                .and_then(|v| v.as_str())
                .and_then(wire::decode);
            if let (Some(id), Some(kind), Some(samples)) = (id, kind, samples)
                && let Ok(running) = RUNNING.lock()
                && let Some(running) = running.as_ref()
            {
                let _ = running.recognise.send((id, kind, samples));
            }
        }
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
