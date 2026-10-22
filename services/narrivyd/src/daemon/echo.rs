//! "Ignore the computer's sound": the daemon owns one echo canceller.
//!
//! With it on, dictation and meetings record the microphone through
//! PipeWire's echo canceller (see `narrivy_platform_linux::echo_cancel`), so
//! a video or a call playing through the speakers does not end up in the
//! text. It starts with the daemon, in the background, so the first
//! dictation never waits for it; until it is up, or when it cannot start,
//! capture uses the plain microphone.
//!
//! The same canceller also removes background noise, at a level the user
//! picks: `off`, `standard` (WebRTC's default) or `strong`.
//!
//! The choices are kept in `audio.json` in the state directory (on, at
//! `standard`, unless changed). `NARRIVY_ECHO_CANCEL=0` turns it off for
//! development.

#![forbid(unsafe_code)]

use std::sync::Mutex;
use std::time::{Duration, Instant};

use narrivy_platform_linux::echo_cancel::{self, EchoCancel, Noise};
use serde::{Deserialize, Serialize};

pub const ENV: &str = "NARRIVY_ECHO_CANCEL";

/// After a failed start, wait this long before trying again.
const RETRY_AFTER: Duration = Duration::from_secs(30);

struct Canceller {
    running: Option<EchoCancel>,
    starting: bool,
    last_attempt: Option<Instant>,
}

static CANCELLER: Mutex<Canceller> = Mutex::new(Canceller {
    running: None,
    starting: false,
    last_attempt: None,
});

/// The noise levels, as clients name them.
pub const NOISE_LEVELS: [&str; 3] = ["off", "standard", "strong"];

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Options {
    #[serde(default = "on")]
    echo_cancel: bool,
    #[serde(default = "standard")]
    noise: String,
    /// The chosen microphone's node name; `None` follows the system default.
    #[serde(default)]
    microphone: Option<String>,
    /// Per pipeline: settings that differ from the defaults above.
    #[serde(default)]
    pipelines: std::collections::BTreeMap<String, PipelineVoice>,
}

/// One pipeline's own Voice settings; `None` follows the default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineVoice {
    #[serde(default)]
    pub echo_cancel: Option<bool>,
    #[serde(default)]
    pub only_my_voice: Option<bool>,
}

const fn on() -> bool {
    true
}

fn standard() -> String {
    "standard".to_owned()
}

impl Default for Options {
    fn default() -> Self {
        Self {
            echo_cancel: true,
            noise: standard(),
            microphone: None,
            pipelines: std::collections::BTreeMap::new(),
        }
    }
}

fn read_options() -> Options {
    options_path()
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice::<Options>(&bytes).ok())
        .unwrap_or_default()
}

fn write_options(options: &Options) -> Result<(), &'static str> {
    let path = options_path().ok_or("audio_options_unavailable")?;
    let bytes = serde_json::to_vec_pretty(options).map_err(|_| "audio_options_unavailable")?;
    std::fs::write(path, bytes).map_err(|_| "audio_options_unavailable")
}

fn noise() -> Noise {
    match read_options().noise.as_str() {
        "off" => Noise::Off,
        "strong" => Noise::Strong,
        _ => Noise::Standard,
    }
}

/// The chosen microphone when it is plugged in; `None` means the system
/// default (nothing chosen, or the chosen one is missing: the choice is kept
/// for when it returns).
#[must_use]
pub fn chosen_microphone() -> Option<String> {
    let chosen = read_options().microphone?;
    echo_cancel::microphones()
        .iter()
        .any(|(name, _)| *name == chosen)
        .then_some(chosen)
}

/// Choose the microphone; `None` (or an empty name) follows the system
/// default. The canceller restarts with it.
///
/// # Errors
/// `audio_microphone_invalid` for a name PipeWire could not have, or
/// `audio_options_unavailable` when it cannot be saved.
pub fn set_microphone(name: Option<&str>) -> Result<(), &'static str> {
    let name = name.filter(|n| !n.is_empty());
    if name.is_some_and(|n| !echo_cancel::valid_node_name(n)) {
        return Err("audio_microphone_invalid");
    }
    let mut options = read_options();
    options.microphone = name.map(str::to_owned);
    write_options(&options)?;
    if let Ok(mut canceller) = CANCELLER.lock() {
        canceller.running = None;
        canceller.last_attempt = None;
    }
    warm();
    Ok(())
}

/// What to record for the user's voice: the echo-cancelled microphone when
/// that is on and running, else the chosen microphone, else `None` (the
/// system default). Never waits; restarts the canceller if it ended.
#[must_use]
pub fn capture_target() -> Option<String> {
    capture_target_for(None)
}

/// [`capture_target`] for a dictation with `pipeline`, which may have its
/// own setting.
#[must_use]
pub fn capture_target_for(pipeline: Option<&str>) -> Option<String> {
    source_for(pipeline)
        .map(str::to_owned)
        .or_else(chosen_microphone)
}

/// A pipeline's own Voice settings (empty when it follows the defaults).
#[must_use]
pub fn pipeline_voice(pipeline: &str) -> PipelineVoice {
    read_options().pipelines.get(pipeline).copied().unwrap_or_default()
}

fn valid_pipeline_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
}

/// Give `pipeline` its own Voice settings (`None` follows the default).
///
/// # Errors
/// `audio_pipeline_invalid` for an id that cannot be a pipeline's, or
/// `audio_options_unavailable` when it cannot be saved.
pub fn set_pipeline_voice(pipeline: &str, voice: PipelineVoice) -> Result<(), &'static str> {
    if !valid_pipeline_id(pipeline) {
        return Err("audio_pipeline_invalid");
    }
    let mut options = read_options();
    if voice == PipelineVoice::default() {
        options.pipelines.remove(pipeline);
    } else {
        options.pipelines.insert(pipeline.to_owned(), voice);
    }
    write_options(&options)?;
    if wanted() {
        warm();
    } else if let Ok(mut canceller) = CANCELLER.lock() {
        canceller.running = None;
    }
    Ok(())
}

/// Whether anything needs the canceller: the default, or a pipeline's own
/// setting.
fn wanted() -> bool {
    if std::env::var(ENV).is_ok_and(|v| v == "0") {
        return false;
    }
    let options = read_options();
    options.echo_cancel || options.pipelines.values().any(|p| p.echo_cancel == Some(true))
}

/// Set how much background noise is removed; the canceller restarts with it.
///
/// # Errors
/// `audio_noise_unknown` for a level not in [`NOISE_LEVELS`], or
/// `audio_options_unavailable` when it cannot be saved.
pub fn set_noise(level: &str) -> Result<(), &'static str> {
    if !NOISE_LEVELS.contains(&level) {
        return Err("audio_noise_unknown");
    }
    let mut options = read_options();
    level.clone_into(&mut options.noise);
    write_options(&options)?;
    if let Ok(mut canceller) = CANCELLER.lock() {
        canceller.running = None;
        canceller.last_attempt = None;
    }
    warm();
    Ok(())
}

fn options_path() -> Option<std::path::PathBuf> {
    narrivy_platform_linux::instance::state_dir()
        .ok()
        .map(|dir| dir.join("audio.json"))
}

/// Whether the user wants the computer's sound removed from the microphone.
#[must_use]
pub fn enabled() -> bool {
    if std::env::var(ENV).is_ok_and(|v| v == "0") {
        return false;
    }
    read_options().echo_cancel
}

/// Turn it on or off; on starts the canceller, off stops it.
///
/// # Errors
/// `audio_options_unavailable` when the choice cannot be saved.
pub fn set_enabled(on: bool) -> Result<(), &'static str> {
    let mut options = read_options();
    options.echo_cancel = on;
    write_options(&options)?;
    if wanted() {
        if let Ok(mut canceller) = CANCELLER.lock() {
            canceller.last_attempt = None;
        }
        warm();
    } else if let Ok(mut canceller) = CANCELLER.lock() {
        canceller.running = None;
    }
    Ok(())
}

/// The options as clients see them.
#[must_use]
pub fn options() -> narrivy_ipc::AudioOptionsDto {
    let echo_cancel_active = CANCELLER
        .lock()
        .is_ok_and(|mut c| c.running.as_mut().is_some_and(EchoCancel::is_running));
    narrivy_ipc::AudioOptionsDto {
        echo_cancel: enabled(),
        echo_cancel_active,
        voice_filter_available: super::voice::available(),
        voice_trained: super::voice::trained(),
        only_my_voice: super::voice::only_my_voice(),
        voice_training: super::voice::training_state(),
        noise_suppression: read_options().noise,
        microphone: read_options().microphone.unwrap_or_default(),
        microphone_missing: read_options().microphone.is_some() && chosen_microphone().is_none(),
        microphones: echo_cancel::microphones()
            .into_iter()
            .map(|(name, description)| narrivy_ipc::MicrophoneDto { name, description })
            .collect(),
        sounds: super::sounds::enabled(),
        voice_strictness: super::voice::strictness(),
        pipelines: read_options()
            .pipelines
            .into_iter()
            .map(|(id, voice)| {
                (
                    id,
                    narrivy_ipc::PipelineVoiceDto {
                        echo_cancel: voice.echo_cancel,
                        only_my_voice: voice.only_my_voice,
                    },
                )
            })
            .collect(),
    }
}

/// Start the canceller in the background when it is wanted and not running.
pub fn warm() {
    if !wanted() {
        return;
    }
    {
        let Ok(mut canceller) = CANCELLER.lock() else {
            return;
        };
        if canceller.starting
            || canceller
                .running
                .as_mut()
                .is_some_and(EchoCancel::is_running)
            || canceller
                .last_attempt
                .is_some_and(|at| at.elapsed() < RETRY_AFTER)
        {
            return;
        }
        canceller.running = None;
        canceller.starting = true;
        canceller.last_attempt = Some(Instant::now());
    }
    let spawned = std::thread::Builder::new()
        .name("narrivy-echo-cancel".into())
        .spawn(|| {
            let microphone = chosen_microphone();
            let started =
                EchoCancel::start(&echo_cancel::config_dir(), noise(), microphone.as_deref());
            if let Ok(mut canceller) = CANCELLER.lock() {
                canceller.starting = false;
                match started {
                    // Turned off while it was starting: drop it.
                    Ok(_) if !wanted() => {}
                    Ok(running) => canceller.running = Some(running),
                    Err(code) => eprintln!("narrivyd: echo cancellation unavailable ({code})"),
                }
            }
        });
    if spawned.is_err()
        && let Ok(mut canceller) = CANCELLER.lock()
    {
        canceller.starting = false;
    }
}

/// The source to record for the user's voice: the echo-cancelled
/// microphone when it is on and running, otherwise `None` (the plain
/// default microphone). Never waits; restarts the canceller if it ended.
#[must_use]
pub fn source() -> Option<&'static str> {
    source_for(None)
}

/// [`source`] for a dictation with `pipeline`.
#[must_use]
pub fn source_for(pipeline: Option<&str>) -> Option<&'static str> {
    let on = pipeline
        .and_then(|p| pipeline_voice(p).echo_cancel)
        .unwrap_or_else(enabled);
    if !on || std::env::var(ENV).is_ok_and(|v| v == "0") {
        return None;
    }
    let running = CANCELLER
        .lock()
        .is_ok_and(|mut c| c.running.as_mut().is_some_and(EchoCancel::is_running));
    if running {
        Some(echo_cancel::SOURCE)
    } else {
        warm();
        None
    }
}
