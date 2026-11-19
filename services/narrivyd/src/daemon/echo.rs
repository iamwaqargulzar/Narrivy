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

use super::host::linux_features::echo_cancel::{self, EchoCancel, Noise};
use serde::{Deserialize, Serialize};

pub const ENV: &str = "NARRIVY_ECHO_CANCEL";

/// After a failed start, wait this long before trying again.
const RETRY_AFTER: Duration = Duration::from_secs(30);

struct Canceller {
    running: Option<EchoCancel>,
    /// What the running (or starting) canceller was built with.
    setup: Option<Setup>,
    starting: bool,
    last_attempt: Option<Instant>,
}

/// What a canceller is built with: the microphone, the noise level and
/// automatic gain. A dictation whose pipeline wants another set restarts it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Setup {
    microphone: Option<String>,
    noise: String,
    auto_gain: bool,
}

static CANCELLER: Mutex<Canceller> = Mutex::new(Canceller {
    running: None,
    setup: None,
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
    /// WebRTC's automatic gain control, which evens out quiet and loud
    /// speech; off unless turned on.
    #[serde(default)]
    auto_gain: bool,
}

/// One pipeline's own audio settings (D50); `None` keeps the starting
/// value above.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineVoice {
    #[serde(default)]
    pub echo_cancel: Option<bool>,
    #[serde(default)]
    pub only_my_voice: Option<bool>,
    /// A microphone's node name, or `""` for the system default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub microphone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub noise: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_gain: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sounds: Option<bool>,
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
            auto_gain: false,
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

/// The active pipeline's own audio settings.
fn own() -> PipelineVoice {
    pipeline_voice(&super::user_pipelines::active())
}

/// The noise level `pipeline` uses (`None`: the active pipeline).
fn noise_level(pipeline: Option<&str>) -> String {
    let own = pipeline.map_or_else(own, pipeline_voice);
    own.noise.unwrap_or_else(|| read_options().noise)
}

/// Whether `pipeline` uses automatic gain (`None`: the active pipeline).
fn auto_gain_for(pipeline: Option<&str>) -> bool {
    let own = pipeline.map_or_else(own, pipeline_voice);
    own.auto_gain.unwrap_or_else(|| read_options().auto_gain)
}

/// The microphone `pipeline` chose, plugged in or not (`None`: the active
/// pipeline); `None` is the system default.
fn wanted_microphone(pipeline: Option<&str>) -> Option<String> {
    let own = pipeline.map_or_else(own, pipeline_voice);
    match own.microphone {
        Some(name) => Some(name).filter(|n| !n.is_empty()),
        None => read_options().microphone,
    }
}

/// Whether `pipeline` plays the start and stop sounds, when it chose.
#[must_use]
pub fn pipeline_sounds(pipeline: Option<&str>) -> Option<bool> {
    pipeline.map_or_else(own, pipeline_voice).sounds
}

/// The canceller `pipeline` wants.
fn setup_for(pipeline: Option<&str>) -> Setup {
    Setup {
        microphone: chosen_microphone_for(pipeline),
        noise: noise_level(pipeline),
        auto_gain: auto_gain_for(pipeline),
    }
}

fn noise() -> Noise {
    match noise_level(None).as_str() {
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
    chosen_microphone_for(None)
}

/// [`chosen_microphone`] for `pipeline` (`None`: the active pipeline).
#[must_use]
pub fn chosen_microphone_for(pipeline: Option<&str>) -> Option<String> {
    let chosen = wanted_microphone(pipeline)?;
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
pub fn set_microphone(pipeline: Option<&str>, name: Option<&str>) -> Result<(), &'static str> {
    let name = name.filter(|n| !n.is_empty());
    if name.is_some_and(|n| !echo_cancel::valid_node_name(n)) {
        return Err("audio_microphone_invalid");
    }
    update_own(pipeline, |own| {
        own.microphone = Some(name.unwrap_or_default().to_owned());
    })?;
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
        .or_else(|| chosen_microphone_for(pipeline))
}

/// A pipeline's own Voice settings (empty when it follows the defaults).
#[must_use]
pub fn pipeline_voice(pipeline: &str) -> PipelineVoice {
    read_options()
        .pipelines
        .get(pipeline)
        .cloned()
        .unwrap_or_default()
}

fn valid_pipeline_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
}

/// Set `pipeline`'s "Ignore this computer's sound" and "Only my voice"
/// (`None` keeps the starting value); its other settings stay.
///
/// # Errors
/// `audio_pipeline_invalid` for an id that cannot be a pipeline's, or
/// `audio_options_unavailable` when it cannot be saved.
pub fn set_pipeline_voice(
    pipeline: &str,
    echo_cancel: Option<bool>,
    only_my_voice: Option<bool>,
) -> Result<(), &'static str> {
    update_own(Some(pipeline), |own| {
        own.echo_cancel = echo_cancel;
        own.only_my_voice = only_my_voice;
    })
}

/// Change `pipeline`'s own settings (`None`: the active pipeline's), then
/// start, restart or stop the canceller to match.
fn update_own(
    pipeline: Option<&str>,
    change: impl FnOnce(&mut PipelineVoice),
) -> Result<(), &'static str> {
    let id = pipeline.map_or_else(super::user_pipelines::active, str::to_owned);
    if !valid_pipeline_id(&id) {
        return Err("audio_pipeline_invalid");
    }
    let mut options = read_options();
    let own = options.pipelines.entry(id.clone()).or_default();
    change(own);
    if *own == PipelineVoice::default() {
        options.pipelines.remove(&id);
    }
    write_options(&options)?;
    if let Ok(mut canceller) = CANCELLER.lock()
        && (!wanted() || canceller.setup.as_ref() != Some(&setup_for(None)))
    {
        canceller.running = None;
        canceller.setup = None;
        canceller.last_attempt = None;
    }
    warm();
    Ok(())
}

/// A deleted pipeline's own settings go with it.
pub fn forget_pipeline(pipeline: &str) {
    let mut options = read_options();
    if options.pipelines.remove(pipeline).is_some() {
        let _ = write_options(&options);
    }
}

/// Whether anything needs the canceller: the default, or a pipeline's own
/// setting.
fn wanted() -> bool {
    if std::env::var(ENV).is_ok_and(|v| v == "0") {
        return false;
    }
    let options = read_options();
    options.echo_cancel
        || options
            .pipelines
            .values()
            .any(|p| p.echo_cancel == Some(true))
}

/// Set how much background noise is removed; the canceller restarts with it.
///
/// # Errors
/// `audio_noise_unknown` for a level not in [`NOISE_LEVELS`], or
/// `audio_options_unavailable` when it cannot be saved.
pub fn set_noise(pipeline: Option<&str>, level: &str) -> Result<(), &'static str> {
    if !NOISE_LEVELS.contains(&level) {
        return Err("audio_noise_unknown");
    }
    update_own(pipeline, |own| own.noise = Some(level.to_owned()))
}

/// Turn `pipeline`'s automatic gain on or off; the canceller restarts with it.
///
/// # Errors
/// `audio_pipeline_invalid`, or `audio_options_unavailable`.
pub fn set_auto_gain(pipeline: Option<&str>, on: bool) -> Result<(), &'static str> {
    update_own(pipeline, |own| own.auto_gain = Some(on))
}

/// Turn `pipeline`'s start and stop sounds on or off.
///
/// # Errors
/// `audio_pipeline_invalid`, or `audio_options_unavailable`.
pub fn set_sounds(pipeline: Option<&str>, on: bool) -> Result<(), &'static str> {
    update_own(pipeline, |own| own.sounds = Some(on))
}

fn options_path() -> Option<std::path::PathBuf> {
    super::host::state_dir()
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
pub fn set_enabled(pipeline: Option<&str>, on: bool) -> Result<(), &'static str> {
    if pipeline.is_some() {
        return update_own(pipeline, |own| own.echo_cancel = Some(on));
    }
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
pub fn options(pipeline: Option<&str>) -> narrivy_ipc::AudioOptionsDto {
    let echo_cancel_active = CANCELLER
        .lock()
        .is_ok_and(|mut c| c.running.as_mut().is_some_and(EchoCancel::is_running));
    let own = pipeline.map_or_else(own, pipeline_voice);
    let microphone = wanted_microphone(pipeline);
    narrivy_ipc::AudioOptionsDto {
        echo_cancel: own.echo_cancel.unwrap_or_else(enabled),
        echo_cancel_active,
        voice_filter_available: super::voice::available(),
        voice_trained: super::voice::trained(),
        only_my_voice: own
            .only_my_voice
            .unwrap_or_else(super::voice::only_my_voice),
        voice_training: super::voice::training_state(),
        noise_suppression: noise_level(pipeline),
        microphone_missing: microphone.is_some() && chosen_microphone_for(pipeline).is_none(),
        microphone: microphone.unwrap_or_default(),
        microphones: echo_cancel::microphones()
            .into_iter()
            .map(|(name, description)| narrivy_ipc::MicrophoneDto { name, description })
            .collect(),
        sounds: own.sounds.unwrap_or_else(super::sounds::enabled),
        auto_gain: auto_gain_for(pipeline),
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
            let setup = setup_for(None);
            let started = EchoCancel::start_with_gain(
                &echo_cancel::config_dir(),
                noise(),
                setup.auto_gain,
                setup.microphone.as_deref(),
            );
            if let Ok(mut canceller) = CANCELLER.lock() {
                canceller.starting = false;
                match started {
                    // Turned off while it was starting: drop it.
                    Ok(_) if !wanted() => {}
                    Ok(running) => {
                        canceller.running = Some(running);
                        canceller.setup = Some(setup);
                    }
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
    let wanted_setup = setup_for(pipeline);
    let running = CANCELLER.lock().is_ok_and(|mut c| {
        let up = c.running.as_mut().is_some_and(EchoCancel::is_running);
        if up && c.setup.as_ref() != Some(&wanted_setup) {
            // Another pipeline's microphone, noise level or gain: restart
            // with this one's; the plain microphone is used meanwhile.
            c.running = None;
            c.setup = None;
            c.last_attempt = None;
            return false;
        }
        up
    });
    if running {
        Some(echo_cancel::SOURCE)
    } else {
        warm();
        None
    }
}
