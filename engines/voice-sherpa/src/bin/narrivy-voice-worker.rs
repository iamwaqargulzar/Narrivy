//! Voice mode's worker: listens, finds turns, and speaks the reply.
//!
//! Started by narrivyd, one process per voice-mode session. Everything
//! stays on this computer; nothing is written to disk or a log, and stderr
//! is discarded by the daemon.
//!
//! - Silero finds speech; about a second of quiet ends the turn. While the
//!   user speaks, the audio so far goes to the daemon every ~2 s (and once
//!   at a pause) to be recognised with the loaded speech model, so the
//!   words are ready when the turn ends.
//! - The reply arrives in pieces on stdin; each sentence is spoken with
//!   Kokoro as soon as it is complete, the next one made while the current
//!   one plays.
//! - Speaking over the reply stops it (after ~0.1 s of speech, only with
//!   the echo-cancelled microphone), or is kept for after it, as chosen.
//! - "Say that again" replays the last reply without asking the model.
//!
//! stdin, one JSON object per line from the daemon:
//!   `{"op":"say","text":...}` `{"op":"end"}` `{"op":"stop"}`
//!   `{"op":"settings",...}` `{"op":"ptt","down":bool}` `{"op":"quit"}`
//!   `{"op":"asr","id":N,"kind":...,"text":...,"ms":N}` (a recognised turn)
//! stdout, one JSON object per line for the daemon:
//!   `{"ev":"ready"}` `{"ev":"state","state":...}` `{"ev":"partial","text":...}`
//!   `{"ev":"heard","text":...,"language":...}` `{"ev":"interrupt"}`
//!   `{"ev":"repeat"}` `{"ev":"timing",...}` `{"ev":"error","code":...}`
//!   `{"ev":"audio","id":N,"kind":...,"pcm":...}` (a turn to recognise)

use std::collections::VecDeque;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use narrivy_voice_sherpa::audio::{self, Output};
use narrivy_voice_sherpa::speech::{self, Voice, WINDOW};
use narrivy_voice_sherpa::text::{self, Sentences};
use narrivy_voice_sherpa::urdu::{Urdu, UrduFiles};
use narrivy_voice_sherpa::wire::{self, Kind};
use narrivy_voice_sherpa::{RATE, cue};
use serde_json::{Value, json};

/// Quiet that ends a turn, in seconds.
const TURN_SILENCE: f32 = 1.0;
/// Speech that interrupts the reply, in seconds.
const BARGE_IN: f32 = 0.1;
/// Recognise while the user speaks this often, in seconds.
const ROLLING: f32 = 2.0;
/// Shortest turn worth recognising, in samples.
const SHORTEST_TURN: usize = (RATE as usize * 3) / 10;
/// The reply's sentences made ahead of what plays.
const AHEAD: usize = 2;

static OUT: Mutex<()> = Mutex::new(());

fn emit(event: &Value) {
    let _guard = OUT.lock();
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "{event}");
    let _ = stdout.flush();
}

fn emit_state(state: &str) {
    emit(&json!({"ev": "state", "state": state}));
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Interrupt {
    Stop,
    Queue,
}

struct Arguments {
    english: PathBuf,
    detector: PathBuf,
    microphone: Option<String>,
    speaker_sink: Option<String>,
    echo_cancelled: bool,
    speed: f32,
    voice: i32,
    interrupt: Interrupt,
    push_to_talk: bool,
    sounds: bool,
    threads: i32,
    routing: audio::Routing,
    urdu: Option<UrduFiles>,
}

fn arguments() -> Result<Arguments, &'static str> {
    let mut english = None;
    let mut detector = None;
    let mut parsed = Arguments {
        english: PathBuf::new(),
        detector: PathBuf::new(),
        microphone: None,
        speaker_sink: None,
        echo_cancelled: false,
        speed: 1.0,
        voice: 3,
        interrupt: Interrupt::Stop,
        push_to_talk: false,
        sounds: false,
        threads: 4,
        routing: Vec::new(),
        urdu: None,
    };
    let (mut urdu_program, mut urdu_voice) = (None, None);
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or("voice_arguments_invalid");
        match flag.as_str() {
            "--english" => english = Some(PathBuf::from(value()?)),
            "--detector" => detector = Some(PathBuf::from(value()?)),
            "--urdu-speaker" => urdu_program = Some(PathBuf::from(value()?)),
            "--urdu-voice" => urdu_voice = Some(PathBuf::from(value()?)),
            "--mic" => parsed.microphone = Some(value()?),
            "--output" => parsed.speaker_sink = Some(value()?),
            "--echo-cancelled" => parsed.echo_cancelled = true,
            "--speed" => parsed.speed = value()?.parse().map_err(|_| "voice_arguments_invalid")?,
            "--speaker" => {
                parsed.voice = value()?.parse().map_err(|_| "voice_arguments_invalid")?
            }
            "--interrupt" => {
                parsed.interrupt = match value()?.as_str() {
                    "stop" => Interrupt::Stop,
                    "queue" => Interrupt::Queue,
                    _ => return Err("voice_arguments_invalid"),
                }
            }
            "--push-to-talk" => parsed.push_to_talk = true,
            "--sounds" => parsed.sounds = true,
            "--threads" => {
                parsed.threads = value()?.parse().map_err(|_| "voice_arguments_invalid")?
            }
            "--env" => {
                let pair = value()?;
                let (key, val) = pair.split_once('=').ok_or("voice_arguments_invalid")?;
                if !audio::ROUTING_KEYS.contains(&key) || val.is_empty() || val.contains('\0') {
                    return Err("voice_arguments_invalid");
                }
                parsed.routing.push((key.to_owned(), val.to_owned()));
            }
            _ => return Err("voice_arguments_invalid"),
        }
    }
    parsed.english = english.ok_or("voice_model_missing")?;
    parsed.detector = detector.ok_or("voice_detector_missing")?;
    parsed.urdu = match (urdu_program, urdu_voice) {
        (Some(program), Some(voice)) => Some(UrduFiles { program, voice }),
        (None, None) => None,
        _ => return Err("voice_arguments_invalid"),
    };
    parsed.speed = parsed.speed.clamp(0.5, 2.0);
    parsed.threads = parsed.threads.clamp(1, 8);
    Ok(parsed)
}

// ---------------------------------------------------------------- player

type OnStart = Box<dyn FnOnce() + Send>;

struct Clip {
    generation: u64,
    samples: Arc<Vec<f32>>,
    rate: u32,
    on_start: Option<OnStart>,
}

/// Plays clips one after another; `stop` cuts off at once.
struct Player {
    clips: Mutex<VecDeque<Clip>>,
    wake: Condvar,
    generation: AtomicU64,
    playing: AtomicBool,
}

impl Player {
    fn start(mut output: Box<dyn Output>) -> Arc<Self> {
        let player = Arc::new(Self {
            clips: Mutex::new(VecDeque::new()),
            wake: Condvar::new(),
            generation: AtomicU64::new(0),
            playing: AtomicBool::new(false),
        });
        let runner = Arc::clone(&player);
        let _ = std::thread::Builder::new()
            .name("voice-player".into())
            .spawn(move || runner.run(output.as_mut()));
        player
    }

    fn play(&self, samples: Arc<Vec<f32>>, rate: u32, on_start: Option<OnStart>) {
        if let Ok(mut clips) = self.clips.lock() {
            clips.push_back(Clip {
                generation: self.generation.load(Ordering::Acquire),
                samples,
                rate,
                on_start,
            });
        }
        self.wake.notify_one();
    }

    fn queued(&self) -> usize {
        self.clips.lock().map_or(0, |clips| clips.len())
    }

    fn idle(&self) -> bool {
        self.queued() == 0 && !self.playing.load(Ordering::Acquire)
    }

    fn stop(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        if let Ok(mut clips) = self.clips.lock() {
            clips.clear();
        }
        self.wake.notify_one();
    }

    fn run(&self, output: &mut dyn Output) {
        loop {
            let clip = {
                let Ok(mut clips) = self.clips.lock() else {
                    return;
                };
                loop {
                    if let Some(clip) = clips.pop_front() {
                        self.playing.store(true, Ordering::Release);
                        break clip;
                    }
                    self.playing.store(false, Ordering::Release);
                    clips = match self.wake.wait(clips) {
                        Ok(clips) => clips,
                        Err(_) => return,
                    };
                }
            };
            let current = || self.generation.load(Ordering::Acquire);
            if clip.generation != current() || !output.open(clip.rate) {
                continue;
            }
            if let Some(on_start) = clip.on_start {
                on_start();
            }
            // Small pieces, paced to real time, so `stop` takes effect
            // quickly and the queue reflects what is heard.
            let step = (clip.rate / 20).max(1) as usize;
            let begin = Instant::now();
            let mut cut = false;
            for (i, piece) in clip.samples.chunks(step).enumerate() {
                if clip.generation != current() || !output.write(piece) {
                    cut = true;
                    break;
                }
                let sent = ((i + 1) * step) as f32 / clip.rate as f32;
                let ahead = sent - begin.elapsed().as_secs_f32() - 0.15;
                if ahead > 0.0 {
                    std::thread::sleep(Duration::from_secs_f32(ahead));
                }
            }
            if cut || clip.generation != current() {
                output.close();
            } else {
                // Let the last piece finish sounding.
                std::thread::sleep(Duration::from_millis(150));
            }
        }
    }
}

// ---------------------------------------------------------------- voice

struct Spoken {
    samples: Arc<Vec<f32>>,
    rate: u32,
}

/// Turns sentences into audio, at most [`AHEAD`] ahead of what plays.
struct Speaker {
    jobs: Sender<(u64, String, bool)>,
    generation: Arc<AtomicU64>,
    pending: Arc<AtomicU32>,
    spoken: Arc<Mutex<Vec<Spoken>>>,
}

#[derive(Default)]
struct Timing {
    ended_at: Option<Instant>,
    heard_at: Option<Instant>,
    first_text_at: Option<Instant>,
    asr_ms: u64,
    queued: bool,
}

impl Speaker {
    fn start(
        voice: Voice,
        mut urdu: Option<Urdu>,
        player: Arc<Player>,
        speaker: Arc<AtomicI32>,
        speed: Arc<Mutex<f32>>,
        timing: Arc<Mutex<Timing>>,
        state: Arc<Mutex<String>>,
    ) -> Self {
        let (jobs, received) = mpsc::channel::<(u64, String, bool)>();
        let generation = Arc::new(AtomicU64::new(0));
        let pending = Arc::new(AtomicU32::new(0));
        let spoken = Arc::new(Mutex::new(Vec::new()));
        let (job_generation, job_pending, job_spoken) = (
            Arc::clone(&generation),
            Arc::clone(&pending),
            Arc::clone(&spoken),
        );
        let _ = std::thread::Builder::new()
            .name("voice-speaker".into())
            .spawn(move || {
                for (made_for, sentence, first) in received {
                    let current = || job_generation.load(Ordering::Acquire);
                    while player.queued() >= AHEAD && made_for == current() {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    if made_for != current() {
                        continue;
                    }
                    let started = Instant::now();
                    let rate_of_speech = speed.lock().map_or(1.0, |speed| *speed);
                    let mut made = None;
                    if text::has_urdu(&sentence)
                        && let Some(speaking) = urdu.as_mut()
                    {
                        made = speaking.speak(&sentence, rate_of_speech);
                        if made.is_none() {
                            // It stopped working: English reads the rest.
                            urdu = None;
                            emit(&json!({"ev": "error", "code": "urdu_speaker_failed"}));
                        }
                    }
                    let made = made.or_else(|| {
                        voice.speak(&sentence, speaker.load(Ordering::Acquire), rate_of_speech)
                    });
                    let done = || {
                        if made_for == current() {
                            let _ = job_pending.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                                Some(n.saturating_sub(1))
                            });
                        }
                    };
                    let Some((samples, rate)) = made else {
                        done();
                        continue;
                    };
                    if made_for != current() {
                        continue;
                    }
                    let samples = Arc::new(samples);
                    if let Ok(mut spoken) = job_spoken.lock() {
                        spoken.push(Spoken {
                            samples: Arc::clone(&samples),
                            rate,
                        });
                    }
                    let on_start: Option<OnStart> = first.then(|| {
                        let synth_ms = started.elapsed().as_millis() as u64;
                        let timing = Arc::clone(&timing);
                        let state = Arc::clone(&state);
                        Box::new(move || {
                            if let Ok(timing) = timing.lock()
                                && let Some(heard) = timing.heard_at
                            {
                                let reply_ms = timing
                                    .first_text_at
                                    .map_or(0, |at| at.saturating_duration_since(heard).as_millis());
                                let total_ms = timing.ended_at.unwrap_or(heard).elapsed().as_millis();
                                emit(&json!({"ev": "timing", "asr_ms": timing.asr_ms,
                                    "reply_ms": reply_ms, "voice_ms": synth_ms, "total_ms": total_ms}));
                            }
                            set_state(&state, "speaking");
                        }) as OnStart
                    });
                    player.play(samples, rate, on_start);
                    done();
                }
            });
        Self {
            jobs,
            generation,
            pending,
            spoken,
        }
    }

    fn queue(&self, sentence: String, timing: &Mutex<Timing>) {
        let first = timing.lock().is_ok_and(|mut timing| {
            let first = !timing.queued;
            timing.queued = true;
            first
        }) && self.spoken.lock().is_ok_and(|spoken| spoken.is_empty());
        self.pending.fetch_add(1, Ordering::AcqRel);
        let _ = self
            .jobs
            .send((self.generation.load(Ordering::Acquire), sentence, first));
    }

    fn cancel(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.pending.store(0, Ordering::Release);
    }

    fn pending(&self) -> u32 {
        self.pending.load(Ordering::Acquire)
    }

    fn take_spoken(&self) -> Vec<Spoken> {
        self.spoken
            .lock()
            .map_or_else(|_| Vec::new(), |mut spoken| std::mem::take(&mut *spoken))
    }
}

fn set_state(state: &Mutex<String>, now: &str) {
    if let Ok(mut current) = state.lock()
        && *current != now
    {
        now.clone_into(&mut current);
        emit_state(now);
    }
}

// ---------------------------------------------------------------- the loop

enum Input {
    Command(Value),
    Closed,
}

struct VoiceMode {
    player: Arc<Player>,
    speaker: Speaker,
    voice_choice: Arc<AtomicI32>,
    speed: Arc<Mutex<f32>>,
    timing: Arc<Mutex<Timing>>,
    state: Arc<Mutex<String>>,
    vad: sherpa_onnx::VoiceActivityDetector,
    quick: sherpa_onnx::VoiceActivityDetector,
    push_to_talk: bool,
    interrupt: Interrupt,
    sounds: bool,
    muted: bool,
    ptt_down: bool,
    barge_in: bool,
    reply: Sentences,
    reply_open: bool,
    after_reply: bool,
    last_reply: Vec<Spoken>,
    utterance: Vec<f32>,
    recent: VecDeque<Vec<f32>>,
    speaking_for: f32,
    silent_for: f32,
    since_rolling: f32,
    /// At a pause: (samples it covers, text).
    early_text: Option<(usize, String)>,
    early_sent: bool,
    rolling_busy: bool,
    next_id: u64,
    /// Ids of audio sent and not yet answered, with what they were for.
    asked: Vec<(u64, Kind, usize)>,
    cue_listen: Arc<Vec<f32>>,
    cue_heard: Arc<Vec<f32>>,
}

impl VoiceMode {
    fn state(&self) -> String {
        self.state
            .lock()
            .map(|state| state.clone())
            .unwrap_or_default()
    }

    fn set_state(&self, now: &str) {
        set_state(&self.state, now);
    }

    fn listening_state(&self) -> &'static str {
        if self.muted { "muted" } else { "listening" }
    }

    fn cue(&self, cue: &Arc<Vec<f32>>) {
        if self.sounds {
            self.player.play(Arc::clone(cue), cue::CUE_RATE, None);
        }
    }

    fn stop_speaking(&mut self) {
        self.speaker.cancel();
        self.player.stop();
        let _ = self.speaker.take_spoken();
        self.reply = Sentences::new();
        self.reply_open = false;
    }

    fn ask(&mut self, kind: Kind, samples: &[f32]) {
        self.next_id += 1;
        let id = self.next_id;
        self.asked.push((id, kind, samples.len()));
        // An old question nobody answers is forgotten.
        if self.asked.len() > 8 {
            self.asked.remove(0);
        }
        emit(
            &json!({"ev": "audio", "id": id, "kind": kind.as_str(), "pcm": wire::encode(samples)}),
        );
    }

    fn finish_turn(&mut self, samples: &[f32]) {
        if let Ok(mut timing) = self.timing.lock() {
            timing.ended_at = Instant::now().checked_sub(Duration::from_secs_f32(TURN_SILENCE));
        }
        self.utterance.clear();
        self.speaking_for = 0.0;
        self.silent_for = 0.0;
        self.since_rolling = 0.0;
        self.early_sent = false;
        // Use the words found at the pause when nothing was said after it.
        if let Some((_, text)) = self.early_text.take() {
            self.turn_heard(&text, 0, samples.len());
        } else {
            self.set_state("thinking");
            self.ask(Kind::Final, samples);
        }
    }

    fn turn_heard(&mut self, heard: &str, asr_ms: u64, samples: usize) {
        let heard = heard.trim();
        if heard.is_empty() || text::is_invented(heard, samples) {
            let now = if self.player.idle() {
                self.listening_state()
            } else {
                "speaking"
            };
            self.set_state(now);
            return;
        }
        if text::is_repeat(heard) && !self.last_reply.is_empty() {
            self.stop_speaking();
            emit(&json!({"ev": "repeat"}));
            self.set_state("speaking");
            for clip in &self.last_reply {
                self.player.play(Arc::clone(&clip.samples), clip.rate, None);
            }
            return;
        }
        self.cue(&self.cue_heard.clone());
        if let Ok(mut timing) = self.timing.lock() {
            let ended_at = timing.ended_at;
            *timing = Timing {
                ended_at: ended_at.or_else(|| Some(Instant::now())),
                heard_at: Some(Instant::now()),
                asr_ms,
                ..Timing::default()
            };
        }
        let _ = self.speaker.take_spoken();
        self.reply = Sentences::new();
        self.reply_open = true;
        if self.state() != "speaking" {
            self.set_state("thinking");
        }
        let language = if text::has_urdu(heard) { "ur" } else { "" };
        emit(&json!({"ev": "heard", "text": heard, "language": language}));
    }

    fn drop_utterance(&mut self) {
        self.early_sent = false;
        self.vad.reset();
        self.utterance.clear();
        self.early_text = None;
        self.speaking_for = 0.0;
        self.silent_for = 0.0;
        self.since_rolling = 0.0;
    }

    fn collect_segments(&mut self, force: bool) {
        while let Some(segment) = self.vad.front() {
            let samples = segment.samples().to_vec();
            self.vad.pop();
            if samples.len() >= SHORTEST_TURN {
                self.finish_turn(&samples);
            } else {
                self.drop_utterance();
            }
        }
        if force && !self.utterance.is_empty() {
            let samples = std::mem::take(&mut self.utterance);
            self.drop_utterance();
            if samples.len() >= SHORTEST_TURN {
                self.finish_turn(&samples);
            }
        }
    }

    fn feed(&mut self, samples: Vec<f32>) {
        let mut listening = !self.muted && (!self.push_to_talk || self.ptt_down);
        let mut speaking = !self.player.idle();
        // Without echo cancelling, the microphone would hear the reply.
        if speaking && !self.barge_in && !self.push_to_talk {
            listening = false;
        }
        if !listening {
            return;
        }
        let seconds = samples.len() as f32 / RATE as f32;
        for window in samples.chunks_exact(WINDOW) {
            self.vad.accept_waveform(window);
            self.quick.accept_waveform(window);
        }
        while !self.quick.is_empty() {
            self.quick.pop();
        }
        if self.quick.detected() {
            if self.utterance.is_empty() {
                // Speech is detected a little after it starts.
                for piece in &self.recent {
                    self.utterance.extend_from_slice(piece);
                }
            }
            self.utterance.extend_from_slice(&samples);
            self.speaking_for += seconds;
            self.silent_for = 0.0;
            self.since_rolling += seconds;
            self.early_text = None;
            self.early_sent = false;
            if speaking && self.speaking_for >= BARGE_IN && self.interrupt == Interrupt::Stop {
                self.stop_speaking();
                emit(&json!({"ev": "interrupt"}));
                speaking = false;
            }
            if !speaking && self.state() != "hearing" {
                self.set_state("hearing");
            }
            if self.since_rolling >= ROLLING && !self.rolling_busy {
                self.since_rolling = 0.0;
                self.rolling_busy = true;
                let heard = self.utterance.clone();
                self.ask(Kind::Rolling, &heard);
            }
        } else if !self.utterance.is_empty() {
            self.utterance.extend_from_slice(&samples);
            self.silent_for += seconds;
            if !self.early_sent {
                // A pause: recognise now, so the words are ready if the
                // turn ends here.
                self.early_sent = true;
                let heard = self.utterance.clone();
                self.ask(Kind::Early, &heard);
            }
        }
        self.recent.push_back(samples);
        while self.recent.len() > 8 {
            self.recent.pop_front();
        }
        self.collect_segments(false);
        if self.utterance.is_empty() && self.state() == "hearing" {
            let now = if self.player.idle() {
                self.listening_state()
            } else {
                "speaking"
            };
            self.set_state(now);
        }
    }

    /// False when the worker should end.
    fn handle(&mut self, command: &Value) -> bool {
        let text_of = |key: &str| command.get(key).and_then(Value::as_str).unwrap_or_default();
        let flag = |key: &str| command.get(key).and_then(Value::as_bool);
        match text_of("op") {
            "quit" => return false,
            "say" if self.reply_open => {
                if let Ok(mut timing) = self.timing.lock() {
                    timing.first_text_at.get_or_insert_with(Instant::now);
                }
                for sentence in self.reply.add(text_of("text")) {
                    self.speaker.queue(sentence, &self.timing);
                }
            }
            "end" => {
                for sentence in self.reply.rest() {
                    self.speaker.queue(sentence, &self.timing);
                }
                self.reply_open = false;
                self.after_reply = true;
            }
            "stop" => {
                self.stop_speaking();
                self.set_state(self.listening_state());
            }
            "settings" => {
                if let Some(speed) = command.get("speed").and_then(Value::as_f64)
                    && let Ok(mut current) = self.speed.lock()
                {
                    *current = (speed as f32).clamp(1.0, 1.75);
                }
                if let Some(on) = flag("push_to_talk") {
                    self.push_to_talk = on;
                }
                match command.get("interrupt").and_then(Value::as_str) {
                    Some("stop") => self.interrupt = Interrupt::Stop,
                    Some("queue") => self.interrupt = Interrupt::Queue,
                    _ => {}
                }
                if let Some(on) = flag("sounds") {
                    self.sounds = on;
                }
                if let Some(voice) = command.get("voice").and_then(Value::as_i64) {
                    let voice = voice.clamp(0, i64::from(narrivy_voice_sherpa::MAX_SPEAKER));
                    self.voice_choice.store(voice as i32, Ordering::Release);
                }
                if let Some(muted) = flag("muted") {
                    self.muted = muted;
                    if muted {
                        self.drop_utterance();
                    }
                    if matches!(self.state().as_str(), "listening" | "muted") {
                        self.set_state(self.listening_state());
                    }
                }
            }
            "ptt" => {
                let down = flag("down").unwrap_or(false);
                if self.push_to_talk && self.ptt_down && !down {
                    // Releasing the key ends the turn at once.
                    self.vad.flush();
                    self.collect_segments(true);
                }
                self.ptt_down = down;
                if down && self.interrupt == Interrupt::Stop && !self.player.idle() {
                    self.stop_speaking();
                    emit(&json!({"ev": "interrupt"}));
                }
            }
            "asr" => self.answered(command),
            _ => {}
        }
        true
    }

    fn answered(&mut self, command: &Value) {
        let Some(id) = command.get("id").and_then(Value::as_u64) else {
            return;
        };
        let Some(at) = self.asked.iter().position(|&(asked, _, _)| asked == id) else {
            return;
        };
        let (_, kind, length) = self.asked.remove(at);
        let heard = command
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let ms = command.get("ms").and_then(Value::as_u64).unwrap_or(0);
        match kind {
            Kind::Final => self.turn_heard(&heard, ms, length),
            Kind::Early => {
                if !self.utterance.is_empty() && self.early_sent {
                    if !heard.is_empty() {
                        emit(&json!({"ev": "partial", "text": heard}));
                    }
                    self.early_text = Some((length, heard));
                }
            }
            Kind::Rolling => {
                self.rolling_busy = false;
                if !self.utterance.is_empty() && !heard.is_empty() {
                    emit(&json!({"ev": "partial", "text": heard}));
                }
            }
        }
    }

    fn after_reply_check(&mut self) {
        if self.after_reply && self.speaker.pending() == 0 && self.player.idle() && !self.reply_open
        {
            self.after_reply = false;
            self.last_reply = self.speaker.take_spoken();
            if matches!(self.state().as_str(), "speaking" | "thinking") {
                self.set_state(self.listening_state());
                if !self.last_reply.is_empty() {
                    self.cue(&self.cue_listen.clone());
                }
            }
        }
    }
}

fn stdin_reader(sender: Sender<Input>) {
    let stdin = std::io::stdin();
    for line in stdin.lock().lines().map_while(Result::ok) {
        if line.len() > 64 * 1024 {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(&line)
            && sender.send(Input::Command(value)).is_err()
        {
            return;
        }
    }
    let _ = sender.send(Input::Closed);
}

fn run(args: Arguments) -> Result<(), &'static str> {
    let voice = Voice::kokoro(&args.english, args.threads)?;
    let vad = speech::detector(&args.detector, TURN_SILENCE, 0.25)?;
    // A quick detector for "speaking right now": pauses and talking over
    // the reply. The one above keeps reporting speech through the pause
    // that ends a turn.
    let quick = speech::detector(&args.detector, 0.2, 0.15)?;
    let capture = audio::capture(args.microphone.as_deref(), &args.routing)?;
    let player = Player::start(audio::output(
        args.speaker_sink.clone(),
        args.routing.clone(),
    ));
    let voice_choice = Arc::new(AtomicI32::new(args.voice));
    let speed = Arc::new(Mutex::new(args.speed));
    let timing = Arc::new(Mutex::new(Timing::default()));
    let state = Arc::new(Mutex::new(String::new()));
    // Urdu is optional: without it, or when it fails to start, English
    // reads everything.
    let urdu = args
        .urdu
        .as_ref()
        .and_then(|files| Urdu::start(&files.program, &files.voice, args.threads.min(2)).ok());
    let has_urdu = urdu.is_some();
    let speaker = Speaker::start(
        voice,
        urdu,
        Arc::clone(&player),
        Arc::clone(&voice_choice),
        Arc::clone(&speed),
        Arc::clone(&timing),
        Arc::clone(&state),
    );
    let (commands, inputs) = mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("voice-commands".into())
        .spawn(move || stdin_reader(commands));
    let mut mode = VoiceMode {
        player,
        speaker,
        voice_choice,
        speed,
        timing,
        state,
        vad,
        quick,
        push_to_talk: args.push_to_talk,
        interrupt: args.interrupt,
        sounds: args.sounds,
        muted: false,
        ptt_down: false,
        barge_in: args.echo_cancelled,
        reply: Sentences::new(),
        reply_open: false,
        after_reply: false,
        last_reply: Vec::new(),
        utterance: Vec::new(),
        recent: VecDeque::new(),
        speaking_for: 0.0,
        silent_for: 0.0,
        since_rolling: 0.0,
        early_text: None,
        early_sent: false,
        rolling_busy: false,
        next_id: 0,
        asked: Vec::new(),
        cue_listen: Arc::new(cue::listening()),
        cue_heard: Arc::new(cue::heard()),
    };
    emit(&json!({"ev": "ready", "english": true, "urdu": has_urdu}));
    mode.set_state(mode.listening_state());
    mode.cue(&mode.cue_listen.clone());
    loop {
        if !drain(&mut mode, &inputs) {
            return Ok(());
        }
        match capture.pieces.recv_timeout(Duration::from_millis(50)) {
            Ok(piece) => mode.feed(piece),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Err("voice_microphone_lost"),
        }
        mode.after_reply_check();
    }
}

/// Handle every command waiting; false when the worker should end.
fn drain(mode: &mut VoiceMode, inputs: &Receiver<Input>) -> bool {
    while let Ok(input) = inputs.try_recv() {
        match input {
            Input::Command(command) => {
                if !mode.handle(&command) {
                    return false;
                }
            }
            Input::Closed => return false,
        }
    }
    true
}

/// The longest audio `--measure-speech` reads: the dictation limit.
const MEASURE_MAX_SAMPLES: usize = 300 * RATE as usize;

/// `--measure-speech --detector <silero_vad.onnx>`: read 16 kHz mono f32
/// samples from standard input until it closes and print how much of them
/// is speech (`{"ev":"speech","ms":N}`), for the daemon's no-speech check
/// before a dictation is recognised. Nothing else is printed or kept.
fn measure_speech() -> Result<(), &'static str> {
    let mut args = std::env::args().skip(2);
    let detector = match (args.next().as_deref(), args.next(), args.next()) {
        (Some("--detector"), Some(path), None) => PathBuf::from(path),
        _ => return Err("voice_arguments_invalid"),
    };
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(
        &mut std::io::Read::take(std::io::stdin().lock(), (MEASURE_MAX_SAMPLES * 4) as u64),
        &mut bytes,
    )
    .map_err(|_| "voice_input_failed")?;
    let samples: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    let ms = speech::speech_ms(&detector, &samples)?;
    emit(&json!({"ev": "speech", "ms": ms}));
    Ok(())
}

fn main() -> std::process::ExitCode {
    let outcome = if std::env::args().nth(1).as_deref() == Some("--measure-speech") {
        measure_speech()
    } else {
        arguments().and_then(run)
    };
    match outcome {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(code) => {
            emit(&json!({"ev": "error", "code": code}));
            std::process::ExitCode::FAILURE
        }
    }
}
