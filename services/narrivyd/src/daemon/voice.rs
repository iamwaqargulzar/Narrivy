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
//! missing, slow or fails, the audio goes to the model unchanged. Each
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

/// Phrases Whisper writes for silence or noise. Kept only when there was
/// clearly more speech than such a phrase takes to say.
const INVENTED: [&str; 9] = [
    "thank you",
    "thanks",
    "thank you for watching",
    "thanks for watching",
    "thank you very much",
    "please subscribe",
    "bye",
    "you",
    "subtitles by the amaraorg community",
];

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
    narrivy_platform_linux::instance::state_dir()
        .ok()
        .map(|dir| dir.join(name))
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
    let target =
        super::echo::capture_target().unwrap_or_else(|| "@DEFAULT_AUDIO_SOURCE@".to_owned());
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
        .args(["--target", &target, "-n", &frames.to_string(), "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "voice_recorder_missing".to_owned())?;
    let mut raw = Vec::new();
    let started = Instant::now();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_end(&mut raw);
    }
    let _ = child.kill();
    let _ = child.wait();
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

impl PlainMic {
    fn start(pipeline: &str) -> Option<Self> {
        if super::echo::source_for(Some(pipeline)).is_none() || !available() || !trained() {
            return None;
        }
        let only_me = super::echo::pipeline_voice(pipeline)
            .only_my_voice
            .unwrap_or_else(only_my_voice);
        if !only_me {
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
            return self
                .recognise(session, audio, cancellation, request, &vocabulary)
                .map(finish);
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
    }

    fn recognize_with(
        &mut self,
        session: SessionId,
        audio: &AudioChunk,
        cancellation: &SessionCancellation,
        request: &AsrRequest,
    ) -> Result<Transcript, AsrError> {
        self.run(session, audio, cancellation, request)
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
    }
}
