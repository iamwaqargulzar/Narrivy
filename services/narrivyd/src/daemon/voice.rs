//! The voice filter: what reaches the speech model during dictation.
//!
//! Every dictation's audio passes through a resident worker
//! (`scripts/voice_filter.py`) before the speech model reads it:
//!
//! - with no speech at all, nothing is recognised, so silence never becomes
//!   "Thank you." (Whisper invents such phrases for silence);
//! - the silence around and between the speech is cut, where Whisper also
//!   invents words;
//! - once the user has trained their voice ("Only my voice"), speech that
//!   does not sound like them is left out.
//!
//! The filter is a help, never a gate on dictation: when the worker is
//! missing, slow or fails, the audio goes to the model unchanged. Only a
//! recording with less than [`MIN_SPEECH_MS`] of speech is then dropped,
//! with the same rule for invented phrases. The speech is measured by the
//! packaged voice worker's Silero detector
//! ([`super::voice_mode::speech_ms`]), else by energy alone
//! ([`energy_speech_ms`]). Windows and the installed Linux package have no
//! filter worker, so this is what they get: a quiet room with nobody
//! speaking gives no text (Whisper writes whole made-up sentences for room
//! noise, not only "Thank you."). Each
//! request is bounded by [`REQUEST_BOUND`]. The voice profile (a fingerprint,
//! not audio) is `voice-profile.json` in the state directory.
//!
//! Development configuration: `NARRIVY_MEETING_PYTHON` (the Python with
//! sherpa-onnx) and `NARRIVY_VOICE_SCRIPT`. Nothing said is logged.

#![forbid(unsafe_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use narrivy_asr_api::{AsrError, AsrRequest, OneShotAsrPort};
use narrivy_types::{AudioChunk, SessionCancellation, SessionId, Transcript};
use serde::{Deserialize, Serialize};

pub const SCRIPT_ENV: &str = "NARRIVY_VOICE_SCRIPT";

/// The longest wait for one filter request.
const REQUEST_BOUND: Duration = Duration::from_secs(5);
/// Training records this long.
pub const TRAINING_SECONDS: u16 = 20;
const RATE: u32 = 16_000;

/// How much longer than asked a training recording may take (start-up and
/// draining) before the recorder is killed.
#[cfg(not(windows))]
const RECORD_SLACK: Duration = Duration::from_secs(10);

/// Phrases Whisper writes for silence or noise. Kept only when there was
/// clearly more speech than such a phrase takes to say.
const INVENTED: [&str; 10] = [
    "thank you",
    "thanks",
    "thank you for watching",
    "thanks for watching",
    "thank you very much",
    "please subscribe",
    "bye",
    "you",
    "subtitles by the amaraorg community",
    // A Korean news sign-off, seen on Windows for a silent dictation.
    "mbc 뉴스 김수근입니다",
];

/// Below this loudness (RMS) a window is silence however loud the room is
/// (the live preview's limit).
const SILENCE_RMS: f32 = 0.004;

/// Less speech than this in a dictation is no speech: nothing is
/// recognised. One short word is about twice as long.
const MIN_SPEECH_MS: u64 = 200;

/// The energy measure's window.
const ENERGY_WINDOW_MS: usize = 20;
/// A window is speech-loud at this multiple of the room's level (about
/// 10 dB, the energy VAD's activation ratio).
const ENERGY_SNR: f32 = 3.0;
/// Speech-loud windows count only in runs at least this long, so clicks
/// and knocks are not speech.
const ENERGY_MIN_RUN: usize = 5;

#[derive(Deserialize)]
struct FilterReply {
    kept: usize,
    speech_ms: u64,
    /// Speech left out as another person's voice.
    #[serde(default)]
    other_ms: u64,
}

#[derive(Deserialize)]
struct EnrollReply {
    ok: bool,
    #[serde(default)]
    error: Option<String>,
}

struct Worker {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Worker {
    fn start() -> Option<Self> {
        let python = std::env::var_os(super::meetings::PYTHON_ENV)?;
        let script = std::env::var_os(SCRIPT_ENV)?;
        let mut child = Command::new(python)
            .arg(script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let stdin = child.stdin.take()?;
        let mut stdout = BufReader::new(child.stdout.take()?);
        let mut ready = String::new();
        if stdout.read_line(&mut ready).ok()? == 0 || !ready.contains("ready") {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        Some(Self {
            child,
            stdin,
            stdout,
        })
    }

    fn send(&mut self, header: &serde_json::Value, samples: &[f32]) -> std::io::Result<String> {
        let mut bytes = serde_json::to_vec(header)?;
        bytes.push(b'\n');
        bytes.reserve(samples.len() * 4);
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        self.stdin.write_all(&bytes)?;
        self.stdin.flush()?;
        let mut line = String::new();
        if self.stdout.read_line(&mut line)? == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        Ok(line)
    }

    fn filter(
        &mut self,
        samples: &[f32],
        profile: Option<&str>,
        threshold: f32,
    ) -> Option<(Vec<f32>, u64)> {
        let header = serde_json::json!({
            "op": "filter",
            "samples": samples.len(),
            "profile": profile,
            "threshold": threshold,
        });
        let reply: FilterReply = serde_json::from_str(&self.send(&header, samples).ok()?).ok()?;
        if reply.kept > samples.len() + RATE as usize {
            return None;
        }
        let mut raw = vec![0u8; reply.kept * 4];
        self.stdout.read_exact(&mut raw).ok()?;
        let kept = raw
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        // Durations only: how much was kept and how much left out, so a
        // dropped word can be traced to the filter without any content.
        eprintln!(
            "narrivyd: voice filter kept {} ms of speech, left out {} ms",
            reply.speech_ms, reply.other_ms
        );
        Some((kept, reply.speech_ms))
    }

    fn enroll(&mut self, samples: &[f32], profile: &str) -> Result<(), String> {
        let header =
            serde_json::json!({ "op": "enroll", "samples": samples.len(), "profile": profile });
        let line = self
            .send(&header, samples)
            .map_err(|_| "voice_filter_failed".to_owned())?;
        let reply: EnrollReply =
            serde_json::from_str(&line).map_err(|_| "voice_filter_failed".to_owned())?;
        if reply.ok {
            Ok(())
        } else {
            Err(reply
                .error
                .unwrap_or_else(|| "voice_training_failed".to_owned()))
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The resident worker, lent to one request at a time. A request that runs
/// past its bound keeps the worker (the thread still owns it) and the
/// worker is started afresh next time.
static WORKER: Mutex<Option<Worker>> = Mutex::new(None);

fn with_worker<T: Send + 'static>(
    bound: Duration,
    job: impl FnOnce(&mut Worker) -> Option<T> + Send + 'static,
) -> Option<T> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("narrivy-voice-filter".into())
        .spawn(move || {
            // Another request (a live preview) may hold the worker briefly;
            // a worker stuck past the bound is not waited for.
            let deadline = Instant::now() + bound;
            let mut slot = loop {
                match WORKER.try_lock() {
                    Ok(slot) => break slot,
                    Err(std::sync::TryLockError::Poisoned(p)) => break p.into_inner(),
                    Err(std::sync::TryLockError::WouldBlock) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(std::sync::TryLockError::WouldBlock) => {
                        let _ = tx.send(None);
                        return;
                    }
                }
            };
            if slot
                .as_mut()
                .is_none_or(|w| !matches!(w.child.try_wait(), Ok(None)))
            {
                *slot = Worker::start();
            }
            let result = slot.as_mut().and_then(job);
            if result.is_none() {
                // A failed worker is not trusted with the next dictation.
                *slot = None;
            }
            let _ = tx.send(result);
        })
        .ok()?;
    rx.recv_timeout(bound).ok().flatten()
}

/// Start the worker in the background, so the first dictation does not wait.
pub fn warm() {
    let _ = std::thread::Builder::new()
        .name("narrivy-voice-warm".into())
        .spawn(|| with_worker(Duration::from_secs(30), |_| Some(())));
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Options {
    /// Keep only the user's voice (once trained).
    #[serde(default = "yes")]
    only_my_voice: bool,
    /// How alike speech must sound to the user's voice to be kept:
    /// `relaxed`, `normal` or `strict`.
    #[serde(default = "normal")]
    strictness: String,
}

fn normal() -> String {
    "normal".to_owned()
}

/// The strictness levels, as clients name them.
pub const STRICTNESS: [&str; 3] = ["relaxed", "normal", "strict"];

fn threshold(strictness: &str) -> f32 {
    match strictness {
        "relaxed" => 0.22,
        "strict" => 0.42,
        _ => 0.3,
    }
}

#[must_use]
pub fn strictness() -> String {
    options().strictness
}

/// # Errors
/// `voice_strictness_unknown`, or `audio_options_unavailable`.
pub fn set_strictness(level: &str) -> Result<(), &'static str> {
    if !STRICTNESS.contains(&level) {
        return Err("voice_strictness_unknown");
    }
    let mut current = options();
    level.clone_into(&mut current.strictness);
    save_options(&current)
}

fn save_options(options: &Options) -> Result<(), &'static str> {
    let path = state_file("voice.json").ok_or("audio_options_unavailable")?;
    let bytes = serde_json::to_vec_pretty(options).map_err(|_| "audio_options_unavailable")?;
    std::fs::write(path, bytes).map_err(|_| "audio_options_unavailable")
}

const fn yes() -> bool {
    true
}

fn state_file(name: &str) -> Option<PathBuf> {
    super::host::state_dir().ok().map(|dir| dir.join(name))
}

fn profile_path() -> Option<PathBuf> {
    state_file("voice-profile.json")
}

fn options() -> Options {
    state_file("voice.json")
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(Options {
            only_my_voice: true,
            strictness: normal(),
        })
}

/// Whether the worker can be run at all.
#[must_use]
pub fn available() -> bool {
    std::env::var_os(super::meetings::PYTHON_ENV).is_some()
        && std::env::var_os(SCRIPT_ENV).is_some()
}

/// The trained voice profile, for labelling the user "You" in meetings.
#[must_use]
pub fn trained_profile() -> Option<PathBuf> {
    if trained() { profile_path() } else { None }
}

/// Whether a voice profile exists.
#[must_use]
pub fn trained() -> bool {
    profile_path().is_some_and(|path| path.is_file())
}

/// Whether "Only my voice" is on (it applies once trained).
#[must_use]
pub fn only_my_voice() -> bool {
    options().only_my_voice
}

/// Turn "Only my voice" on or off.
///
/// # Errors
/// `audio_options_unavailable` when the choice cannot be saved.
pub fn set_only_my_voice(on: bool) -> Result<(), &'static str> {
    let mut current = options();
    current.only_my_voice = on;
    save_options(&current)
}

/// Forget the trained voice.
///
/// # Errors
/// `audio_options_unavailable` when the profile cannot be removed.
pub fn forget() -> Result<(), &'static str> {
    match profile_path() {
        Some(path) if path.is_file() => {
            std::fs::remove_file(path).map_err(|_| "audio_options_unavailable")
        }
        _ => Ok(()),
    }
}

/// Training: `idle`, `recording`, `failed:<code>`.
static TRAINING: Mutex<String> = Mutex::new(String::new());

#[must_use]
pub fn training_state() -> String {
    let state = TRAINING.lock().map(|s| s.clone()).unwrap_or_default();
    if state.is_empty() {
        "idle".to_owned()
    } else {
        state
    }
}

fn set_training(state: &str) {
    if let Ok(mut s) = TRAINING.lock() {
        state.clone_into(&mut s);
    }
}

/// Record the user reading for [`TRAINING_SECONDS`] and save their voice
/// profile. Runs in the background; [`training_state`] follows it.
///
/// # Errors
/// A short code: already training, no worker, or the recorder missing.
pub fn train() -> Result<(), &'static str> {
    if !available() {
        return Err("voice_filter_missing");
    }
    {
        let mut state = TRAINING.lock().map_err(|_| "voice_training_failed")?;
        if *state == "recording" {
            return Err("voice_training_busy");
        }
        "recording".clone_into(&mut state);
    }
    let profile = profile_path().ok_or("audio_options_unavailable")?;
    std::thread::Builder::new()
        .name("narrivy-voice-training".into())
        .spawn(move || {
            let outcome = record(TRAINING_SECONDS).and_then(|samples| {
                let path = profile.display().to_string();
                with_worker(Duration::from_secs(60), move |worker| {
                    Some(worker.enroll(&samples, &path))
                })
                .unwrap_or_else(|| Err("voice_filter_failed".to_owned()))
            });
            match outcome {
                Ok(()) => set_training("idle"),
                Err(code) => set_training(&format!("failed:{code}")),
            }
        })
        .map_err(|_| "voice_training_failed")?;
    Ok(())
}

/// Record the microphone the way dictation does (through the echo canceller
/// when that is on), as 16 kHz mono samples.
fn record(seconds: u16) -> Result<Vec<f32>, String> {
    record_from(super::echo::capture_target().as_deref(), seconds)
}

/// Record the chosen microphone itself (not the echo canceller) for
/// `seconds` and say how loud it was: the loudest sample, 0 to 1. The audio
/// is dropped here; only the level leaves.
pub fn microphone_peak(seconds: u16) -> Result<f32, String> {
    let samples = record_from(super::echo::chosen_microphone().as_deref(), seconds)?;
    if samples.is_empty() {
        return Err("voice_recorder_failed".to_owned());
    }
    Ok(samples.iter().fold(0.0_f32, |peak, s| peak.max(s.abs())))
}

/// Record `seconds` from `target` (else the default microphone).
#[cfg(windows)]
fn record_from(target: Option<&str>, seconds: u16) -> Result<Vec<f32>, String> {
    super::host::record_microphone(target, seconds)
}

#[cfg(not(windows))]
fn record_from(target: Option<&str>, seconds: u16) -> Result<Vec<f32>, String> {
    let target = target.unwrap_or("@DEFAULT_AUDIO_SOURCE@");
    let frames = u32::from(seconds) * RATE;
    let mut child = Command::new("pw-record")
        .args([
            "--raw",
            "--rate",
            "16000",
            "--channels",
            "1",
            "--format",
            "f32",
        ])
        .args(["--target", target, "-n", &frames.to_string(), "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "voice_recorder_missing".to_owned())?;
    let started = Instant::now();
    // `pw-record -n` ends after `frames`; one that waits on a missing source
    // or a wedged server is killed past the bound instead of holding the
    // request forever. The reader hands back what it read once the pipe
    // closes, which killing the recorder does.
    let (sender, received) = mpsc::channel();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = std::thread::Builder::new()
            .name("narrivy-voice-record".into())
            .spawn(move || {
                let mut raw = Vec::new();
                let _ = stdout.read_to_end(&mut raw);
                let _ = sender.send(raw);
            });
    }
    let bound = Duration::from_secs(u64::from(seconds)) + RECORD_SLACK;
    let finished = super::host::linux_features::bounded_command::wait_until(
        &mut child,
        started + bound,
        bound,
    );
    let _ = child.kill();
    let _ = child.wait();
    if finished.is_err() {
        return Err("voice_recorder_failed".to_owned());
    }
    let raw = received
        .recv_timeout(RECORD_SLACK)
        .map_err(|_| "voice_recorder_failed".to_owned())?;
    if started.elapsed() < Duration::from_secs(u64::from(seconds) / 2) {
        return Err("voice_recorder_failed".to_owned());
    }
    Ok(raw
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect())
}

/// The audio to recognise and how much speech it holds, or `None` to use
/// the audio unchanged.
fn filtered(audio: &AudioChunk, pipeline: Option<&str>) -> Option<(Vec<f32>, u64)> {
    if !available()
        || audio.sample_rate_hz != RATE
        || audio.channels != 1
        || audio.samples.is_empty()
    {
        return None;
    }
    let only_me = pipeline
        .and_then(|p| super::echo::pipeline_voice(p).only_my_voice)
        .unwrap_or_else(only_my_voice);
    let profile = (only_me && trained())
        .then(profile_path)
        .flatten()
        .map(|p| p.display().to_string());
    let samples = audio.samples.clone();
    let threshold = threshold(&strictness());
    with_worker(REQUEST_BOUND, move |worker| {
        worker.filter(&samples, profile.as_deref(), threshold)
    })
}

/// How much speech `audio` holds when the voice filter cannot run: the voice
/// worker's Silero measure, else energy alone. Logs durations only.
fn no_filter_speech_ms(audio: &AudioChunk) -> Option<u64> {
    let (speech_ms, by) = match super::voice_mode::speech_ms(audio) {
        Some(ms) => (Some(ms), "speech detector"),
        None => (energy_speech_ms(audio), "energy"),
    };
    if let Some(ms) = speech_ms {
        eprintln!("narrivyd: no-speech check ({by}) found {ms} ms of speech");
    }
    speech_ms
}

/// Whether `text` is only a phrase Whisper invents for silence.
fn invented(text: &str) -> bool {
    let plain: String = text
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .to_lowercase();
    let plain = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    INVENTED.contains(&plain.as_str())
}

/// Milliseconds of speech in `audio` by energy alone, for when neither the
/// voice filter nor the voice worker can measure it: 20 ms windows at least
/// [`ENERGY_SNR`] times the room's level (the 10th-percentile window, so
/// the whole recording sets it, not only its first second) and louder than
/// [`SILENCE_RMS`], in runs of at least [`ENERGY_MIN_RUN`] windows. `None`
/// for audio it cannot measure.
fn energy_speech_ms(audio: &AudioChunk) -> Option<u64> {
    if audio.channels != 1 || audio.sample_rate_hz == 0 {
        return None;
    }
    let window = usize::try_from(audio.sample_rate_hz).ok()? * ENERGY_WINDOW_MS / 1000;
    if window == 0 {
        return None;
    }
    let levels: Vec<f32> = audio
        .samples
        .chunks_exact(window)
        .map(|w| (w.iter().map(|s| s * s).sum::<f32>() / window as f32).sqrt())
        .collect();
    let mut sorted = levels.clone();
    sorted.sort_by(f32::total_cmp);
    let room = sorted.get(sorted.len() / 10).copied().unwrap_or(0.0);
    let loud = (room * ENERGY_SNR).max(SILENCE_RMS);
    let (mut windows, mut run) = (0_usize, 0_usize);
    for level in levels.iter().chain(std::iter::once(&0.0)) {
        if *level >= loud {
            run += 1;
        } else {
            if run >= ENERGY_MIN_RUN {
                windows += run;
            }
            run = 0;
        }
    }
    u64::try_from(windows * ENERGY_WINDOW_MS).ok()
}

/// The longest plain-microphone recording kept (the dictation limit).
const PLAIN_MAX_SAMPLES: usize = 300 * RATE as usize;

/// The plain microphone, recorded in memory beside the echo-cancelled one.
///
/// While the speakers play speech (a video, a call), the echo canceller can
/// take the user's voice away with it and the dictation comes out empty.
/// When it does, the voice filter tries this recording instead: "Only my
/// voice" keeps the user and leaves the other speaker out. Started only
/// when both are on; the audio never leaves memory and is dropped with the
/// dictation.
struct PlainMic {
    child: Child,
    reader: Option<std::thread::JoinHandle<Vec<f32>>>,
}

/// Whether a dictation in `pipeline` also records the plain microphone
/// (see [`VoiceFiltered::with_plain_mic`]): only with an echo-cancelled
/// source and a trained voice with "Only my voice" on.
#[must_use]
pub fn plain_mic_wanted(pipeline: &str) -> bool {
    super::echo::source_for(Some(pipeline)).is_some()
        && available()
        && trained()
        && super::echo::pipeline_voice(pipeline)
            .only_my_voice
            .unwrap_or_else(only_my_voice)
}

impl PlainMic {
    fn start(pipeline: &str) -> Option<Self> {
        if !plain_mic_wanted(pipeline) {
            return None;
        }
        let target =
            super::echo::chosen_microphone().unwrap_or_else(|| "@DEFAULT_AUDIO_SOURCE@".to_owned());
        let mut child = Command::new("pw-record")
            .args([
                "--raw",
                "--rate",
                "16000",
                "--channels",
                "1",
                "--format",
                "f32",
            ])
            .args(["--target", &target, "-"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let mut stdout = child.stdout.take()?;
        let reader = std::thread::Builder::new()
            .name("narrivy-plain-mic".into())
            .spawn(move || {
                let mut samples = Vec::new();
                let mut buf = [0u8; 4096];
                let mut carry = Vec::new();
                while let Ok(n) = stdout.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    carry.extend_from_slice(&buf[..n]);
                    let whole = carry.len() / 4 * 4;
                    if samples.len() < PLAIN_MAX_SAMPLES {
                        samples.extend(
                            carry[..whole]
                                .chunks_exact(4)
                                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
                        );
                    }
                    carry.drain(..whole);
                }
                samples.truncate(PLAIN_MAX_SAMPLES);
                samples
            });
        match reader {
            Ok(reader) => Some(Self {
                child,
                reader: Some(reader),
            }),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                None
            }
        }
    }

    /// Stop recording and hand over what was heard.
    fn finish(mut self) -> Vec<f32> {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.reader
            .take()
            .and_then(|reader| reader.join().ok())
            .unwrap_or_default()
    }
}

impl Drop for PlainMic {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A speech model behind the voice filter.
pub struct VoiceFiltered {
    inner: Box<dyn OneShotAsrPort>,
    /// Whether the model takes the user's words as a prompt.
    takes_words: bool,
    /// The dictation's pipeline, which may have its own Voice settings.
    pipeline: Option<String>,
    /// The plain microphone, for when the echo canceller left nothing.
    plain: Option<PlainMic>,
}

impl VoiceFiltered {
    /// `takes_words`: the model attests context bias, so the user's words
    /// go to it as its prompt. Either way, near misses of them are corrected
    /// and replacements applied after recognition.
    #[must_use]
    pub fn new(inner: Box<dyn OneShotAsrPort>, takes_words: bool) -> Self {
        Self {
            inner,
            takes_words,
            pipeline: None,
            plain: None,
        }
    }

    /// Also record the plain microphone for this dictation, so speech the
    /// echo canceller removed with the computer's sound can still be found
    /// (only with "Only my voice", which keeps other speakers out). Call as
    /// the dictation starts, after [`VoiceFiltered::for_pipeline`].
    #[must_use]
    pub fn with_plain_mic(mut self) -> Self {
        self.plain = self.pipeline.as_deref().and_then(PlainMic::start);
        self
    }

    /// Use `pipeline`'s own Voice settings.
    #[must_use]
    pub fn for_pipeline(mut self, pipeline: &str) -> Self {
        self.pipeline = Some(pipeline.to_owned());
        self
    }

    fn recognise(
        &mut self,
        session: SessionId,
        audio: &AudioChunk,
        cancellation: &SessionCancellation,
        request: &AsrRequest,
        vocabulary: &super::vocabulary::Vocabulary,
    ) -> Result<Transcript, AsrError> {
        let mut biased = request.clone();
        if self.takes_words {
            // A client's own hints first, then the user's words.
            biased.context_bias =
                super::vocabulary::context_bias_with(request.context_bias.as_ref(), vocabulary);
        }
        if biased.context_bias.is_some() && biased != *request {
            match self
                .inner
                .recognize_with(session, audio, cancellation, &biased)
            {
                Err(err) if err.code == narrivy_asr_api::UNSUPPORTED_CONTEXT_BIAS => {}
                outcome => return outcome,
            }
        }
        if request.is_default() {
            self.inner.recognize(session, audio, cancellation)
        } else {
            self.inner
                .recognize_with(session, audio, cancellation, request)
        }
    }

    fn run(
        &mut self,
        session: SessionId,
        audio: &AudioChunk,
        cancellation: &SessionCancellation,
        request: &AsrRequest,
    ) -> Result<Transcript, AsrError> {
        let vocabulary = super::vocabulary::load();
        let finish = |transcript: Transcript| Transcript {
            text: super::vocabulary::apply(&transcript.text, &vocabulary),
            is_final: transcript.is_final,
        };
        let plain = self.plain.take().map(PlainMic::finish);
        let Some((samples, speech_ms)) = filtered(audio, self.pipeline.as_deref()) else {
            // No filter: the whole audio, unless it holds no speech.
            let speech_ms = no_filter_speech_ms(audio);
            if speech_ms.is_some_and(|ms| ms < MIN_SPEECH_MS) {
                return Ok(Transcript {
                    text: String::new(),
                    is_final: true,
                });
            }
            let transcript = self.recognise(session, audio, cancellation, request, &vocabulary)?;
            if speech_ms.is_some_and(|ms| ms < 1500) && invented(&transcript.text) {
                return Ok(Transcript {
                    text: String::new(),
                    is_final: true,
                });
            }
            return Ok(finish(transcript));
        };
        let transcript = self.speech(
            session,
            audio,
            samples,
            speech_ms,
            cancellation,
            request,
            &vocabulary,
        )?;
        if !transcript.text.trim().is_empty() {
            return Ok(finish(transcript));
        }
        // Nothing through the echo canceller: try the plain microphone.
        let Some(plain) = plain.filter(|p| !p.is_empty()) else {
            return Ok(transcript);
        };
        let plain = AudioChunk {
            sample_rate_hz: RATE,
            channels: 1,
            samples: plain,
        };
        let Some((samples, speech_ms)) = filtered(&plain, self.pipeline.as_deref()) else {
            return Ok(transcript);
        };
        self.speech(
            session,
            &plain,
            samples,
            speech_ms,
            cancellation,
            request,
            &vocabulary,
        )
        .map(finish)
    }

    /// Recognise the speech the filter kept; empty when there was none or
    /// the model only invented a phrase for silence.
    #[allow(clippy::too_many_arguments)]
    fn speech(
        &mut self,
        session: SessionId,
        audio: &AudioChunk,
        samples: Vec<f32>,
        speech_ms: u64,
        cancellation: &SessionCancellation,
        request: &AsrRequest,
        vocabulary: &super::vocabulary::Vocabulary,
    ) -> Result<Transcript, AsrError> {
        let empty = Transcript {
            text: String::new(),
            is_final: true,
        };
        if samples.is_empty() {
            // No speech (or none in the user's voice): nothing to write.
            return Ok(empty);
        }
        let chunk = AudioChunk {
            sample_rate_hz: audio.sample_rate_hz,
            channels: 1,
            samples,
        };
        let transcript = self.recognise(session, &chunk, cancellation, request, vocabulary)?;
        if speech_ms < 1500 && invented(&transcript.text) {
            return Ok(empty);
        }
        Ok(transcript)
    }
}

impl OneShotAsrPort for VoiceFiltered {
    fn recognize(
        &mut self,
        session: SessionId,
        audio: &AudioChunk,
        cancellation: &SessionCancellation,
    ) -> Result<Transcript, AsrError> {
        self.run(session, audio, cancellation, &AsrRequest::default())
            .map(without_wordless)
    }

    fn recognize_with(
        &mut self,
        session: SessionId,
        audio: &AudioChunk,
        cancellation: &SessionCancellation,
        request: &AsrRequest,
    ) -> Result<Transcript, AsrError> {
        self.run(session, audio, cancellation, request)
            .map(without_wordless)
    }
}

/// Text with no letter or digit (Whisper's "......" for noise) is no text.
fn without_wordless(transcript: Transcript) -> Transcript {
    if transcript.text.chars().any(char::is_alphanumeric) {
        transcript
    } else {
        Transcript {
            text: String::new(),
            is_final: transcript.is_final,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phrases_invented_for_silence_are_recognised() {
        assert!(invented("Thank you."));
        assert!(invented("  thanks for watching! "));
        assert!(invented("You"));
        assert!(!invented("Thank you for the update on the release."));
        assert!(!invented("Send it today."));
        assert!(invented("MBC 뉴스 김수근입니다."));
    }

    fn mono(samples: Vec<f32>) -> AudioChunk {
        AudioChunk {
            sample_rate_hz: RATE,
            channels: 1,
            samples,
        }
    }

    /// A 200 Hz tone of `amplitude` for `seconds`.
    fn tone(amplitude: f32, seconds: f32) -> Vec<f32> {
        (0..(seconds * RATE as f32) as usize)
            .map(|i| amplitude * (i as f32 * 200.0 * std::f32::consts::TAU / RATE as f32).sin())
            .collect()
    }

    #[test]
    fn energy_measure_finds_no_speech_in_silence_or_room_tone() {
        assert_eq!(
            energy_speech_ms(&mono(vec![0.0; 3 * RATE as usize])),
            Some(0)
        );
        // Steady room tone below the silence level.
        assert_eq!(energy_speech_ms(&mono(tone(0.003, 3.0))), Some(0));
    }

    #[test]
    fn energy_measure_counts_speech_after_quiet() {
        let mut samples = tone(0.001, 1.0);
        samples.extend(tone(0.2, 2.0));
        samples.extend(tone(0.001, 1.0));
        let ms = energy_speech_ms(&mono(samples));
        assert!(
            ms.is_some_and(|ms| (1500..=2600).contains(&ms)),
            "speech_ms {ms:?}"
        );
    }

    /// Steady room noise at `rms` for `seconds` (a fixed pseudo-random
    /// sequence, so the test is repeatable).
    fn room_noise(rms: f32, seconds: f32) -> Vec<f32> {
        let mut state = 0x2545_f491_u32;
        let scale = rms * 3.0_f32.sqrt();
        (0..(seconds * RATE as f32) as usize)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                (state as f32 / u32::MAX as f32 * 2.0 - 1.0) * scale
            })
            .collect()
    }

    #[test]
    fn energy_measure_finds_no_speech_in_loud_room_noise() {
        // The Windows VM's room: -37 dBFS at the microphone with echo
        // cancellation off, about -54 dBFS after noise suppression. The
        // first second used to count as speech (no room level known yet).
        assert!(
            energy_speech_ms(&mono(room_noise(0.014, 15.0))).is_some_and(|ms| ms < MIN_SPEECH_MS)
        );
        assert!(
            energy_speech_ms(&mono(room_noise(0.002, 15.0))).is_some_and(|ms| ms < MIN_SPEECH_MS)
        );
        // A 60 ms knock is not speech either.
        let mut knock = room_noise(0.014, 4.0);
        for sample in &mut knock[RATE as usize..RATE as usize + 960] {
            *sample *= 10.0;
        }
        assert!(energy_speech_ms(&mono(knock)).is_some_and(|ms| ms < MIN_SPEECH_MS));
    }

    #[test]
    fn energy_measure_finds_a_short_word_in_room_noise() {
        let mut samples = room_noise(0.014, 4.0);
        for (sample, word) in samples[RATE as usize..].iter_mut().zip(tone(0.1, 0.4)) {
            *sample += word;
        }
        let ms = energy_speech_ms(&mono(samples));
        assert!(
            ms.is_some_and(|ms| (300..=500).contains(&ms)),
            "speech_ms {ms:?}"
        );
    }

    /// A speech model that writes a sentence whatever it hears, as Whisper
    /// does for room noise.
    struct MakesThingsUp(usize);

    impl OneShotAsrPort for MakesThingsUp {
        fn recognize(
            &mut self,
            _: SessionId,
            _: &AudioChunk,
            _: &SessionCancellation,
        ) -> Result<Transcript, AsrError> {
            self.0 += 1;
            Ok(Transcript {
                text: "Especially on the home table for some.".to_owned(),
                is_final: true,
            })
        }
    }

    #[test]
    fn a_dictation_of_room_noise_writes_nothing() {
        let mut filtered = VoiceFiltered::new(Box::new(MakesThingsUp(0)), false);
        let heard = filtered
            .recognize(
                SessionId(1),
                &mono(room_noise(0.014, 15.0)),
                &SessionCancellation::new(),
            )
            .map(|t| t.text);
        assert_eq!(heard.as_deref(), Ok(""));
    }

    #[test]
    fn a_dictation_with_speech_in_room_noise_is_recognised() {
        let mut samples = room_noise(0.014, 6.0);
        for (sample, word) in samples[RATE as usize..].iter_mut().zip(tone(0.1, 2.0)) {
            *sample += word;
        }
        let mut filtered = VoiceFiltered::new(Box::new(MakesThingsUp(0)), false);
        let heard = filtered
            .recognize(SessionId(1), &mono(samples), &SessionCancellation::new())
            .map(|t| t.text);
        assert_eq!(
            heard.as_deref(),
            Ok("Especially on the home table for some.")
        );
    }

    #[test]
    fn text_without_a_letter_is_no_text() {
        let dots = Transcript {
            text: "......".to_owned(),
            is_final: true,
        };
        assert_eq!(without_wordless(dots).text, "");
        let word = Transcript {
            text: "Hut!".to_owned(),
            is_final: true,
        };
        assert_eq!(without_wordless(word).text, "Hut!");
    }

    #[test]
    fn energy_measure_skips_audio_it_cannot_read() {
        let stereo = AudioChunk {
            sample_rate_hz: RATE,
            channels: 2,
            samples: vec![0.0; 640],
        };
        assert_eq!(energy_speech_ms(&stereo), None);
    }
}
