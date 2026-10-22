//! Meeting transcription (F22): the daemon side.
//!
//! A meeting records two sources into its own folder under the state
//! directory (`meetings/<id>/`):
//!
//! - the microphone, for the people in the room (`mic.wav`);
//! - the computer's sound, for the people on a call (`system.wav`).
//!
//! Both are recorded through PipeWire's `pw-record`. While recording, a live
//! worker follows both files and appends each turn to `live.jsonl` as soon as
//! it is said, so the transcript shows during the meeting. When recording
//! stops, the live worker finishes what is left, then a worker splits each
//! source by voice over the whole meeting and transcribes every turn with
//! Whisper again, which tells speakers apart more reliably.
//! The worker reports progress as JSON lines on stdout, which carry counts and
//! never what was said. It writes `transcript.json`; this module adds
//! `transcript.md`, speaker names and the meeting's state.
//!
//! Development configuration: `NARRIVY_MEETING_PYTHON` and
//! `NARRIVY_MEETING_SCRIPT` name the worker (`scripts/meeting.py` run by the
//! Python that has sherpa-onnx and openvino-genai), and the Whisper models
//! come from `NARRIVY_MEETING_MODELS_DIR` (default
//! `~/.cache/narrivy/models`). Text is never logged.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use narrivy_platform_linux::echo_cancel;
use base64::Engine as _;
use narrivy_ipc::{MeetingDetailDto, MeetingDto, MeetingSlideDto, MeetingTurnDto};
use serde::{Deserialize, Serialize};

use super::session::{DaemonEvent, DaemonState};

pub const PYTHON_ENV: &str = "NARRIVY_MEETING_PYTHON";
pub const SCRIPT_ENV: &str = "NARRIVY_MEETING_SCRIPT";
/// `scripts/screen_capture.py`: keeps pictures of the shared screen (D34).
pub const SCREEN_SCRIPT_ENV: &str = "NARRIVY_SCREEN_SCRIPT";
/// The Python with PyGObject and GStreamer for it (default the system's).
pub const SCREEN_PYTHON_ENV: &str = "NARRIVY_SCREEN_PYTHON";
/// The folder in a meeting with the screen's pictures.
const SCREENS: &str = "screens";
/// What the model is asked about each screen picture.
const SLIDE_TASK: &str = "You describe pictures of a computer screen taken during a meeting or \
a video the user watched, so they can be asked about later. Say what it shows: for slides, \
documents or pages, the title, the key text, numbers, and any chart or table's values; for a \
video scene, the people, what they do, the setting and any text on screen. Be factual and \
concise, at most 120 words, no preamble. If it is only a grid of video-call participants \
(faces or names, nothing else), answer exactly: Video call.";
pub const MODELS_DIR_ENV: &str = "NARRIVY_MEETING_MODELS_DIR";

/// Whisper models a meeting can be transcribed with, by folder.
const MODELS: [(&str, &str); 2] = [
    ("large-v3", "whisper-large-v3-openvino-int8"),
    ("turbo", "whisper-large-v3-turbo-openvino-int8"),
];

/// The model the live view prefers.
const LIVE_MODEL: &str = "turbo";

/// Kinds of meeting notes, each saved as `notes-<kind>.md`.
const NOTE_KINDS: [&str; 3] = ["notes", "email", "custom"];

/// Transcript characters condensed per step: about 1,500 tokens, so a step
/// fits a model that holds 4,096 with its instructions and reply.
const PART_CHARS: usize = 6000;

const MATERIAL_LAYOUT: &str = "The meeting is given as transcript lines `[time] Speaker: what was said`. \
A speaker is a person's name when the user named them, otherwise a label such as `Person 2`. \
Speech recognition can misspell names. \
Parts may be in Urdu or other languages: write in English and translate what you use.";

const PART_TASK: &str = "You condense one part of a longer meeting transcript so the whole meeting \
can be summarised later. Write concise bullet points that keep every topic, decision, commitment \
(with who made it), question, name, number and date in this part. Use only what is said. \
Return only the bullets.";

fn notes_task(kind: &str, instruction: Option<&str>) -> Result<String, &'static str> {
    let task = match kind {
        "notes" => {
            "Write clear meeting notes in Markdown from the material. Use only what is supported by it; \
never invent facts, names, owners or dates, and keep names, numbers and dates exactly. Tell apart \
what was only discussed or proposed from what was decided. Leave out greetings, filler and repetition. \
Use these sections and skip any that would be empty:\n\
## Summary\n3 to 5 bullets: why the meeting happened, the main subjects, outcomes and next steps.\n\
## Discussion\nA short subheading per topic with what was said and why it matters.\n\
## Decisions\nOnly what was clearly agreed.\n\
## Action items\n`- [ ] Action — Owner` for each thing someone agreed or was asked to do, with any date.\n\
## Open questions\nWhat is still unresolved or needs follow-up.\n\
If nothing substantive was said, reply with one sentence saying what the recording contains. \
Return only the notes."
                .to_owned()
        }
        "email" => {
            "Draft the follow-up email the user would send to the others after this meeting, using only \
what the material supports. Keep names, numbers and dates exactly, and separate decisions from open \
items. Write in the first person, warm and professional, under 250 words, with [brackets] for anything \
the material does not give (such as a recipient's name). Start with `Subject:`, then a greeting, one \
short recap paragraph, `Decisions`, `Next steps` (owner, action, date) and `Open questions` as short \
lists (skip empty ones), and a sign-off. Return only the email."
                .to_owned()
        }
        _ => {
            let instruction = instruction
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .ok_or("writing_instruction_missing")?;
            format!(
                "You answer the user's request about a meeting, using only what the material supports and \
saying so when it does not cover something. Request: {instruction}"
            )
        }
    };
    Ok(format!("{task}\n\n{MATERIAL_LAYOUT}"))
}

/// Split transcript lines into parts of at most `limit` characters, at line
/// ends (a single longer line becomes a part of its own).
fn split_material(material: &str, limit: usize) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    for line in material.lines() {
        if !current.is_empty() && current.len() + line.len() + 1 > limit {
            parts.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(line);
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

fn short_clock(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

/// The longest meeting recorded; recording stops by itself after this.
const MAX_RECORDING: Duration = Duration::from_secs(4 * 60 * 60);

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Meta {
    id: String,
    title: String,
    started_unix_ms: u64,
    duration_ms: u64,
    state: String,
    #[serde(default)]
    progress: u16,
    #[serde(default)]
    stage: Option<String>,
    model: String,
    #[serde(default)]
    speakers: u16,
    /// Recorded with "Watch with me".
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    watch: bool,
    /// A live screen share (its chat saw it while it lasted); kept as a
    /// session like a recording once it stops.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    share: bool,
    /// While recording: the screen and the sound are being kept now.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    screen_on: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    sound_on: bool,
    /// While recording: the microphone is being kept now.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    mic_on: bool,
    /// Kept as heard when it stopped (the live transcript, the screens not
    /// described), not gone over yet.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    raw: bool,
    /// Spans when a sound was switched off: the recorded sound skips
    /// them, so its times are moved by them to match the clock.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    gaps: Vec<Gap>,
}

/// A span without sound, from `at_ms` after the start, `ms` long, in one
/// stream (`room`: the microphone, `call`: the computer's sound) or, with
/// none, in both.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Gap {
    at_ms: u64,
    ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stream: Option<String>,
}

/// A time in the recorded sound of `stream` (which skips its gaps) as a
/// time on the meeting's clock.
fn sound_to_clock(sound_ms: u64, gaps: &[Gap], stream: &str) -> u64 {
    let mut shift = 0;
    for gap in gaps.iter().filter(|g| g.stream.as_deref().is_none_or(|s| s == stream)) {
        // Where the gap falls in the sound: its clock time less the gaps
        // before it.
        if gap.at_ms.saturating_sub(shift) <= sound_ms {
            shift += gap.ms;
        } else {
            break;
        }
    }
    sound_ms + shift
}

impl Meta {
    fn dto(&self) -> MeetingDto {
        MeetingDto {
            id: self.id.clone(),
            title: self.title.clone(),
            started_unix_ms: self.started_unix_ms,
            duration_ms: self.duration_ms,
            state: self.state.clone(),
            progress: self.progress,
            stage: self.stage.clone(),
            model: self.model.clone(),
            speakers: self.speakers,
            watch: self.watch,
            paused: false,
            share: self.share,
            screen_on: self.screen_on,
            sound_on: self.sound_on,
            mic_on: self.mic_on,
            raw: self.raw,
        }
    }
}

/// How a meeting starts: "Watch with me" (`watch`, the screen is kept),
/// a live screen share (`share`), and what sound to record this time.
#[derive(Clone, Copy, Debug, Default)]
pub struct Start {
    pub watch: bool,
    pub share: bool,
    pub microphone: Option<bool>,
    pub computer_sound: Option<bool>,
}

/// A turn as the worker writes it.
#[derive(Debug, Deserialize)]
struct WorkerTurn {
    stream: String,
    start: f64,
    end: f64,
    speaker: String,
    text: String,
    #[serde(default)]
    language: String,
}

/// One sound being recorded: the microphone (`room`) or the computer's
/// sound (`call`).
struct Recorder {
    stream: &'static str,
    /// `pw-record` writing raw samples, gone while the stream is off so the
    /// desktop no longer shows it as recording.
    child: Option<Child>,
    /// Appends what the recorder writes to the stream's file.
    pump: Option<std::thread::JoinHandle<()>>,
    /// Switched off since this many ms after the start.
    off_since: Option<u64>,
}

impl Recorder {
    fn start(dir: &Path, stream: &'static str, target: &str) -> Result<Self, &'static str> {
        let (child, pump) = start_recorder(dir, stream, target)?;
        Ok(Self { stream, child: Some(child), pump: Some(pump), off_since: None })
    }
}

struct Recording {
    id: String,
    started: Instant,
    children: Vec<Recorder>,
    /// The live transcription worker, when it could start.
    live: Option<Child>,
    /// What is being recorded, as the user knows the devices.
    microphone: String,
    speaker: String,
    /// Keeps pictures of the shared screen, when asked for.
    screen: Option<Child>,
    started_unix_ms: u64,
    /// The portal's restore token for the screen chosen in this recording:
    /// switching the screen back on asks no second time. Memory only.
    screen_token: Arc<Mutex<Option<String>>>,
    moving_every: u32,
    /// What was chosen to record (for sound switched on later).
    chosen: MeetingOptions,
}

/// How long the live worker may take to transcribe what is left at the end.
const LIVE_FINISH_BOUND: Duration = Duration::from_secs(120);

#[derive(Default)]
struct Inner {
    recording: Option<Recording>,
    /// The meeting being transcribed.
    processing: Option<String>,
    /// The transcription worker: meeting and process id.
    worker: Option<(String, u32)>,
    /// The notes being written: meeting and assistant turn.
    asking: Option<(String, String)>,
}

pub struct Meetings {
    root: PathBuf,
    inner: Mutex<Inner>,
    /// Meetings whose processing is paused (`false`) or stopped (`true`).
    /// Its own lock: `publish` reads it, and is called with `inner` held.
    control: Mutex<std::collections::HashMap<String, bool>>,
    state: Arc<DaemonState>,
}

static SHARED: OnceLock<Arc<Meetings>> = OnceLock::new();

/// The meetings service, if something already used it.
pub fn existing() -> Option<Arc<Meetings>> {
    SHARED.get().cloned()
}

/// The meetings service, created on first use.
///
/// # Errors
/// When the state directory is unavailable.
pub fn shared(state: &Arc<DaemonState>) -> Result<Arc<Meetings>, &'static str> {
    if let Some(meetings) = SHARED.get() {
        return Ok(Arc::clone(meetings));
    }
    let root = narrivy_platform_linux::instance::state_dir()
        .map_err(|_| "meeting_storage_unavailable")?
        .join("meetings");
    create_private_dir(&root)?;
    let meetings = Arc::new(Meetings {
        root,
        inner: Mutex::new(Inner::default()),
        control: Mutex::new(std::collections::HashMap::new()),
        state: Arc::clone(state),
    });
    meetings.recover();
    Ok(Arc::clone(SHARED.get_or_init(|| meetings)))
}

fn create_private_dir(path: &Path) -> Result<(), &'static str> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .map_err(|_| "meeting_storage_unavailable")
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn models_dir() -> PathBuf {
    std::env::var_os(MODELS_DIR_ENV).map_or_else(
        || {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default()
                .join(".cache/narrivy/models")
        },
        PathBuf::from,
    )
}

/// The Whisper models installed, most accurate first.
#[must_use]
pub fn installed_models() -> Vec<String> {
    let dir = models_dir();
    MODELS
        .iter()
        .filter(|(_, folder)| {
            dir.join(folder)
                .join("openvino_encoder_model.xml")
                .is_file()
        })
        .map(|(name, _)| (*name).to_owned())
        .collect()
}

pub(crate) fn model_path(name: &str) -> Option<PathBuf> {
    MODELS
        .iter()
        .find(|(model, _)| *model == name)
        .map(|(_, folder)| models_dir().join(folder))
        .filter(|path| path.join("openvino_encoder_model.xml").is_file())
}

fn pactl(what: &str) -> Option<String> {
    let out = Command::new("pactl").arg(what).output().ok()?;
    let name = String::from_utf8(out.stdout).ok()?.trim().to_owned();
    (out.status.success() && !name.is_empty()).then_some(name)
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

impl Meetings {
    fn dir(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }

    fn read_meta(&self, id: &str) -> Option<Meta> {
        let bytes = std::fs::read(self.dir(id).join("meeting.json")).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    fn write_meta(&self, meta: &Meta) {
        let dir = self.dir(&meta.id);
        let part = dir.join("meeting.json.part");
        if let Ok(bytes) = serde_json::to_vec_pretty(meta)
            && std::fs::write(&part, bytes).is_ok()
        {
            let _ = std::fs::rename(&part, dir.join("meeting.json"));
        }
    }

    fn publish(&self, meta: &Meta) {
        self.state.publish_assistant(DaemonEvent::MeetingChanged {
            id: meta.id.clone(),
            state: meta.state.clone(),
            progress: meta.progress,
            stage: if self.paused(&meta.id) { Some("paused".to_owned()) } else { meta.stage.clone() },
        });
    }

    fn update(&self, id: &str, change: impl FnOnce(&mut Meta)) -> Option<Meta> {
        let mut meta = self.read_meta(id)?;
        change(&mut meta);
        self.write_meta(&meta);
        self.publish(&meta);
        Some(meta)
    }

    /// Meetings left recording or transcribing by an earlier daemon.
    fn recover(&self) {
        for meta in self.all() {
            if meta.state == "recording" || meta.state == "processing" {
                let mut meta = meta;
                meta.state = "failed:interrupted".to_owned();
                meta.stage = None;
                self.write_meta(&meta);
            }
        }
    }

    fn all(&self) -> Vec<Meta> {
        let mut found: Vec<Meta> = std::fs::read_dir(&self.root)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| valid_id(name))
            .filter_map(|name| self.read_meta(&name))
            .collect();
        found.sort_by_key(|meta| std::cmp::Reverse(meta.started_unix_ms));
        found
    }

    /// Start recording.
    ///
    /// # Errors
    /// A short code: already recording, no PipeWire, no storage.
    pub fn start(self: &Arc<Self>, title: Option<String>) -> Result<MeetingDto, &'static str> {
        self.start_with(title, Start::default())
    }

    /// Start recording; with `watch` ("Watch with me") the screen is kept
    /// whatever the options say, and moving pictures more often.
    ///
    /// # Errors
    /// As [`Self::start`].
    pub fn start_with(
        self: &Arc<Self>,
        title: Option<String>,
        start: Start,
    ) -> Result<MeetingDto, &'static str> {
        let Start {
            watch,
            share,
            microphone,
            computer_sound,
        } = start;
        // Started from the assistant (sound chosen for this one): the
        // screen is kept only when asked, whatever the meeting options say.
        let from_assistant = microphone.is_some() || computer_sound.is_some();
        let mut inner = self.inner.lock().map_err(|_| "meeting_unavailable")?;
        if inner.recording.is_some() {
            return Err("meeting_already_recording");
        }
        let mut chosen = options();
        if let Some(on) = microphone {
            chosen.microphone_off = !on;
        }
        if let Some(on) = computer_sound {
            chosen.call_audio_off = !on;
        }
        // Only the screen is fine when the screen is what is kept.
        if chosen.microphone_off && chosen.call_audio_off && !watch {
            return Err("meeting_nothing_to_record");
        }
        let devices = devices_for(chosen.clone());
        let started_unix_ms = now_ms();
        let id = format!("m{started_unix_ms}");
        let dir = self.dir(&id);
        create_private_dir(&dir)?;
        let children = match start_recorders(&dir, &devices) {
            Ok(children) => children,
            Err(code) => {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(code);
            }
        };
        let targets_empty = children.is_empty();
        let title = title
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "Meeting".to_owned());
        let meta = Meta {
            id: id.clone(),
            title,
            started_unix_ms,
            duration_ms: 0,
            state: "recording".to_owned(),
            progress: 0,
            stage: None,
            model: installed_models().into_iter().next().unwrap_or_default(),
            speakers: 0,
            watch,
            share,
            screen_on: false,
            sound_on: children.iter().any(|r| r.stream == CALL),
            mic_on: children.iter().any(|r| r.stream == ROOM),
            raw: false,
            gaps: Vec::new(),
        };
        self.write_meta(&meta);
        self.publish(&meta);
        let live = (!targets_empty).then(|| self.start_live(&id, &meta.model)).flatten();
        let moving_every = if watch || from_assistant { 10 } else { 30 };
        let token = Arc::new(Mutex::new(None));
        let screen = (watch || (!from_assistant && options().screen))
            .then(|| start_screen(&dir, started_unix_ms, moving_every, &token))
            .flatten();
        if screen.is_some() {
            self.update(&id, |meta| meta.screen_on = true);
        }
        inner.recording = Some(Recording {
            id: id.clone(),
            started: Instant::now(),
            started_unix_ms,
            children,
            live,
            microphone: devices.microphone,
            speaker: devices.speaker,
            screen,
            screen_token: token,
            moving_every,
            chosen,
        });
        drop(inner);
        // Stop by itself after the longest allowed meeting.
        let meetings = Arc::clone(self);
        std::thread::Builder::new()
            .name("narrivy-meeting-limit".into())
            .spawn(move || {
                std::thread::sleep(MAX_RECORDING);
                let still = meetings
                    .inner
                    .lock()
                    .ok()
                    .and_then(|inner| inner.recording.as_ref().map(|r| r.id.clone()));
                if still.as_deref() == Some(id.as_str()) {
                    let _ = meetings.stop();
                }
            })
            .map_err(|_| "meeting_unavailable")?;
        Ok(meta.dto())
    }

    /// Import a recording (an audio or video file) as a meeting: it is
    /// converted to 16 kHz mono in the meeting's folder, then transcribed
    /// like a recorded meeting. The file itself is only read.
    ///
    /// # Errors
    /// A short code: the file is not usable, no converter, no storage.
    pub fn import(self: &Arc<Self>, path: &str, title: Option<String>) -> Result<MeetingDto, &'static str> {
        let source = PathBuf::from(path);
        let meta = std::fs::metadata(&source).map_err(|_| "meeting_import_missing")?;
        let extension = source
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        if !source.is_absolute() || !meta.is_file() || !IMPORT_EXTENSIONS.contains(&extension.as_str()) {
            return Err("meeting_import_unsupported");
        }
        if meta.len() > IMPORT_MAX_BYTES {
            return Err("meeting_import_too_large");
        }
        let converter = converter().ok_or("meeting_import_no_converter")?;
        let started_unix_ms = now_ms();
        let id = format!("m{started_unix_ms}");
        let dir = self.dir(&id);
        create_private_dir(&dir)?;
        let title = title
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty())
            .or_else(|| {
                source
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .map(|s| s.chars().take(80).collect())
            })
            .unwrap_or_else(|| "Imported recording".to_owned());
        let meta = Meta {
            id: id.clone(),
            title,
            started_unix_ms,
            duration_ms: 0,
            state: "processing".to_owned(),
            progress: 0,
            stage: Some("import".to_owned()),
            model: installed_models().into_iter().next().unwrap_or_default(),
            speakers: 0,
            watch: false,
            share: false,
            ..Meta::default()
        };
        self.write_meta(&meta);
        self.publish(&meta);
        let meetings = Arc::clone(self);
        let model = meta.model.clone();
        std::thread::Builder::new()
            .name("narrivy-meeting-import".into())
            .spawn(move || {
                let outcome = convert(&converter, &source, &dir.join("mic.wav")).and_then(|duration_ms| {
                    meetings.update(&id, |meta| meta.duration_ms = duration_ms);
                    meetings.process(&id, Some(model), None, None)
                });
                if let Err(code) = outcome {
                    meetings.update(&id, |meta| {
                        meta.state = format!("failed:{code}");
                        meta.stage = None;
                    });
                }
            })
            .map_err(|_| "meeting_unavailable")?;
        Ok(meta.dto())
    }

    /// Stop recording and start transcribing.
    ///
    /// # Errors
    /// `meeting_not_recording`.
    pub fn stop(self: &Arc<Self>) -> Result<(), &'static str> {
        let recording = {
            let mut inner = self.inner.lock().map_err(|_| "meeting_unavailable")?;
            inner.recording.take().ok_or("meeting_not_recording")?
        };
        let Recording {
            id,
            started,
            children,
            live,
            screen,
            ..
        } = recording;
        let now = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let mut gaps = Vec::new();
        for recorder in &children {
            if let Some(since) = recorder.off_since {
                gaps.push(Gap { at_ms: since, ms: now.saturating_sub(since), stream: Some(recorder.stream.to_owned()) });
            }
        }
        self.update(&id, |meta| {
            meta.gaps.extend(gaps);
            meta.screen_on = false;
            meta.sound_on = false;
            meta.mic_on = false;
        });
        let dir = self.dir(&id);
        finish_recorders(children);
        for file in ["mic.wav", "system.wav"] {
            seal_wav(&dir.join(file));
        }
        if let Some(mut screen) = screen {
            let _ = screen.kill();
            let _ = screen.wait();
        }
        let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let model = self
            .update(&id, |meta| {
                meta.duration_ms = duration_ms;
                meta.state = "processing".to_owned();
                meta.progress = 0;
                meta.stage = Some("finish".to_owned());
            })
            .map(|meta| meta.model)
            .unwrap_or_default();
        // The live worker finishes the last words first (it holds the model).
        // Then the whole meeting is transcribed again, or, unless the user
        // asked for that, it is kept as heard until they ask.
        // Without a live transcript there is nothing to keep but the sound.
        let heard = ["mic.wav", "system.wav"]
            .iter()
            .any(|file| std::fs::metadata(dir.join(file)).is_ok_and(|m| m.len() > 44));
        let go_over = options().process_on_stop || (live.is_none() && heard);
        let meetings = Arc::clone(self);
        std::thread::Builder::new()
            .name("narrivy-meeting-finish".into())
            .spawn(move || {
                if let Some(mut live) = live {
                    finish_live(&mut live);
                }
                if !go_over {
                    meetings.update(&id, |meta| {
                        meta.state = "ready".to_owned();
                        meta.progress = 1000;
                        meta.stage = None;
                        meta.raw = true;
                    });
                    return;
                }
                if let Err(code) = meetings.process(&id, Some(model), None, None) {
                    meetings.update(&id, |meta| {
                        meta.state = format!("failed:{code}");
                        meta.stage = None;
                    });
                }
            })
            .map_err(|_| "meeting_unavailable")?;
        Ok(())
    }

    /// Start the live worker for a meeting being recorded. It is a bonus:
    /// when it cannot start, the meeting is still recorded and transcribed
    /// at the end.
    fn start_live(self: &Arc<Self>, id: &str, model: &str) -> Option<Child> {
        // The live view is a preview, transcribed again and again as words
        // come: the faster turbo when it is there. The finished meeting is
        // gone over with the model chosen for it.
        let model_path = model_path(LIVE_MODEL).or_else(|| model_path(model))?;
        let python = std::env::var_os(PYTHON_ENV)?;
        let script = std::env::var_os(SCRIPT_ENV)?;
        let dir = self.dir(id);
        let log = std::fs::File::create(dir.join("live.log")).ok();
        let mut child = Command::new(python)
            .arg(script)
            .arg("live")
            .arg(&dir)
            .arg("--model")
            .arg(&model_path)
            .args(voice_args())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(log.map_or_else(Stdio::null, Stdio::from))
            .spawn()
            .ok()?;
        let stdout = child.stdout.take()?;
        let meetings = Arc::clone(self);
        let id = id.to_owned();
        // Each new turn: tell clients to show it. Lines carry counts only.
        let watcher = std::thread::Builder::new()
            .name("narrivy-meeting-live".into())
            .spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    if live_turns(&line).is_some_and(|n| n > 0)
                        && let Some(meta) = meetings.read_meta(&id)
                    {
                        meetings.publish(&meta);
                    }
                }
            });
        if watcher.is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        Some(child)
    }

    /// Switch the screen or the sound of the meeting being recorded on or
    /// off (`None` leaves it). The screen asks the portal only the first
    /// time; the sound skips the time it is off (kept as a gap).
    ///
    /// # Errors
    /// `meeting_not_recording`, or what starting a recorder reports.
    pub fn sources(
        self: &Arc<Self>,
        id: &str,
        screen: Option<bool>,
        sound: Option<bool>,
        microphone: Option<bool>,
    ) -> Result<MeetingDto, &'static str> {
        let mut inner = self.inner.lock().map_err(|_| "meeting_unavailable")?;
        let recording = inner
            .recording
            .as_mut()
            .filter(|r| r.id == id)
            .ok_or("meeting_not_recording")?;
        let dir = self.dir(id);
        let now = u64::try_from(recording.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let mut gaps = Vec::new();
        for (stream, on) in [(CALL, sound), (ROOM, microphone)] {
            if let Some(on) = on {
                gaps.extend(switch_sound(recording, &dir, stream, on, now)?);
            }
        }
        match screen {
            Some(false) => {
                if let Some(mut child) = recording.screen.take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
            Some(true) if recording.screen.is_none() => {
                recording.screen = start_screen(
                    &dir,
                    recording.started_unix_ms,
                    recording.moving_every,
                    &recording.screen_token,
                );
                if recording.screen.is_none() {
                    return Err("screen_capture_unavailable");
                }
            }
            _ => {}
        }
        let screen_on = recording.screen.is_some();
        let hearing = |stream: &str| {
            recording
                .children
                .iter()
                .any(|r| r.stream == stream && r.off_since.is_none())
        };
        let (sound_on, mic_on) = (hearing(CALL), hearing(ROOM));
        // The live transcription starts with the first sound.
        let started_sound = recording.live.is_none() && !recording.children.is_empty();
        drop(inner);
        let meta = self
            .update(id, |meta| {
                meta.screen_on = screen_on;
                meta.sound_on = sound_on;
                meta.mic_on = mic_on;
                if screen_on {
                    meta.watch = true;
                }
                meta.gaps.extend(gaps);
            })
            .ok_or("meeting_not_found")?;
        if started_sound {
            let live = self.start_live(id, &meta.model);
            if let Ok(mut inner) = self.inner.lock()
                && let Some(recording) = inner.recording.as_mut().filter(|r| r.id == id)
            {
                recording.live = live;
            }
        }
        Ok(meta.dto())
    }

    /// Pause, resume or stop processing a meeting (`pause`, `resume`,
    /// `stop`). Stopping a recording ends it as the Stop button does. A
    /// pause takes hold at once for transcription and after the current
    /// picture or part of the notes otherwise.
    ///
    /// # Errors
    /// A short code: unknown meeting or action.
    pub fn control(self: &Arc<Self>, id: &str, action: &str) -> Result<(), &'static str> {
        self.read_meta(id).ok_or("meeting_not_found")?;
        let inner = self.inner.lock().map_err(|_| "meeting_unavailable")?;
        if inner.recording.as_ref().is_some_and(|r| r.id == id) {
            // Stopped while recording: the recording ends and is not gone
            // over (the meeting ends failed:meeting_stopped).
            if action != "stop" {
                return Err("meeting_still_recording");
            }
            drop(inner);
            self.set_control(id, Some(true));
            return self.stop();
        }
        let worker = inner.worker.as_ref().filter(|(w, _)| w == id).map(|(_, pid)| *pid);
        let mut cancel = None;
        match action {
            "pause" => {
                self.set_control(id, Some(false));
                if let Some(pid) = worker {
                    signal(pid, "-STOP");
                }
            }
            "resume" => {
                self.set_control(id, None);
                if let Some(pid) = worker {
                    signal(pid, "-CONT");
                }
            }
            "stop" => {
                self.set_control(id, Some(true));
                if let Some(pid) = worker {
                    signal(pid, "-CONT");
                    signal(pid, "-TERM");
                }
                cancel = inner.asking.as_ref().filter(|(m, _)| m == id).map(|(_, t)| t.clone());
            }
            _ => return Err("meeting_control_unknown"),
        }
        drop(inner);
        if let (Some(turn), Ok(assistant)) = (cancel, super::assistant::shared(&self.state)) {
            assistant.cancel(&turn);
        }
        if let Some(meta) = self.read_meta(id) {
            self.publish(&meta);
        }
        Ok(())
    }

    /// Whether processing `id` may go on: waits while it is paused, false
    /// once it was stopped.
    fn proceed(&self, id: &str) -> bool {
        loop {
            match self.control.lock().ok().and_then(|control| control.get(id).copied()) {
                None => return true,
                Some(true) => return false,
                Some(false) => std::thread::sleep(Duration::from_millis(300)),
            }
        }
    }

    /// Mark `id` paused (`Some(false)`), stopped (`Some(true)`) or neither.
    fn set_control(&self, id: &str, value: Option<bool>) {
        if let Ok(mut control) = self.control.lock() {
            match value {
                Some(stopped) => control.insert(id.to_owned(), stopped),
                None => control.remove(id),
            };
        }
    }

    fn paused(&self, id: &str) -> bool {
        self.control
            .lock()
            .is_ok_and(|control| control.get(id) == Some(&false))
    }

    /// Transcribe a recorded meeting (again).
    ///
    /// # Errors
    /// A short code: unknown meeting, busy, model or worker missing.
    pub fn process(
        self: &Arc<Self>,
        id: &str,
        model: Option<String>,
        room_speakers: Option<u16>,
        call_speakers: Option<u16>,
    ) -> Result<(), &'static str> {
        let meta = self.read_meta(id).ok_or("meeting_not_found")?;
        let model = model
            .filter(|m| !m.is_empty())
            .or_else(|| installed_models().into_iter().next())
            .ok_or("meeting_model_missing")?;
        let model_path = model_path(&model).ok_or("meeting_model_missing")?;
        let python = std::env::var_os(PYTHON_ENV).ok_or("meeting_worker_missing")?;
        let script = std::env::var_os(SCRIPT_ENV).ok_or("meeting_worker_missing")?;
        {
            let mut inner = self.inner.lock().map_err(|_| "meeting_unavailable")?;
            if inner.recording.as_ref().is_some_and(|r| r.id == id) {
                return Err("meeting_still_recording");
            }
            if inner.processing.is_some() {
                return Err("meeting_busy");
            }
            inner.processing = Some(id.to_owned());
        }
        let _ = meta;
        self.update(id, |meta| {
            meta.raw = false;
            meta.state = "processing".to_owned();
            meta.progress = 0;
            meta.stage = Some("load".to_owned());
            meta.model.clone_from(&model);
        });
        let dir = self.dir(id);
        let heard = ["mic.wav", "system.wav"]
            .iter()
            .any(|file| std::fs::metadata(dir.join(file)).is_ok_and(|m| m.len() > 44));
        if !heard {
            // Only the screen was recorded: nothing to transcribe, the
            // pictures are described straight away.
            let _ = std::fs::write(dir.join("transcript.json"), "[]");
            if let Ok(mut inner) = self.inner.lock() {
                inner.processing = None;
            }
            self.update(id, |meta| {
                meta.state = "ready".to_owned();
                meta.progress = 1000;
                meta.stage = None;
                meta.speakers = 0;
            });
            let meetings = Arc::clone(self);
            let id = id.to_owned();
            std::thread::Builder::new()
                .name("narrivy-meeting".into())
                .spawn(move || {
                    if meetings.proceed(&id) {
                        meetings.describe_slides(&id);
                    }
                    meetings.set_control(&id, None);
                })
                .map_err(|_| "meeting_unavailable")?;
            return Ok(());
        }
        let mut command = Command::new(python);
        command
            .arg(script)
            .arg("transcribe")
            .arg(&dir)
            .arg("--progress")
            .arg("--model")
            .arg(&model_path)
            .args(voice_args());
        if let Some(n) = room_speakers.filter(|n| *n > 0) {
            command.args(["--room-speakers", &n.to_string()]);
        }
        if let Some(n) = call_speakers.filter(|n| *n > 0) {
            command.args(["--call-speakers", &n.to_string()]);
        }
        let log = std::fs::File::create(dir.join("transcribe.log")).ok();
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(log.map_or_else(Stdio::null, Stdio::from));
        let meetings = Arc::clone(self);
        let id = id.to_owned();
        std::thread::Builder::new()
            .name("narrivy-meeting".into())
            .spawn(move || {
                if !meetings.proceed(&id) {
                    meetings.update(&id, |meta| {
                        meta.state = "failed:meeting_stopped".to_owned();
                        meta.stage = None;
                    });
                    meetings.set_control(&id, None);
                    return;
                }
                meetings.run_worker(&id, command);
                if meetings.proceed(&id) {
                    meetings.describe_slides(&id);
                }
                // Notes written straight away, when the user asked for that.
                if meetings.proceed(&id)
                    && auto_notes()
                    && meetings.read_meta(&id).is_some_and(|m| m.state == "ready")
                    && !meetings.notes(&id).contains_key("notes")
                    && let Ok(assistant) = super::assistant::shared(&meetings.state)
                {
                    let _ = meetings.summarize(assistant, &id, format!("auto-{id}"), "notes", None);
                } else {
                    meetings.set_control(&id, None);
                }
            })
            .map_err(|_| "meeting_unavailable")?;
        Ok(())
    }

    fn run_worker(&self, id: &str, command: Command) {
        // Whatever happens in the worker, the meeting ends ready or failed,
        // and the next one can be transcribed.
        let outcome =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.work(id, command)))
                .unwrap_or(Err("meeting_worker_failed"));
        if let Err(code) = outcome {
            eprintln!("narrivyd: meeting transcription failed ({code})");
            self.update(id, |meta| {
                meta.state = format!("failed:{code}");
                meta.stage = None;
            });
        }
        if let Ok(mut inner) = self.inner.lock() {
            inner.processing = None;
        }
    }

    fn work(&self, id: &str, mut command: Command) -> Result<(), &'static str> {
        {
            let mut child = command.spawn().map_err(|_| "meeting_worker_missing")?;
            if let Ok(mut inner) = self.inner.lock() {
                inner.worker = Some((id.to_owned(), child.id()));
            }
            let stdout = child.stdout.take().ok_or("meeting_worker_failed")?;
            let mut last = (String::new(), 0u16);
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let Some((stage, progress)) = progress_of(&line) else {
                    continue;
                };
                // Publish when the stage changes or progress moves 1%.
                if stage != last.0 || progress >= last.1 + 10 {
                    last = (stage.clone(), progress);
                    self.update(id, |meta| {
                        meta.stage = Some(stage);
                        meta.progress = progress;
                    });
                }
            }
            let status = child.wait().map_err(|_| "meeting_worker_failed")?;
            if let Ok(mut inner) = self.inner.lock() {
                inner.worker = None;
            }
            if !self.proceed(id) {
                return Err("meeting_stopped");
            }
            if !status.success() {
                return Err("meeting_worker_failed");
            }
            let turns = self.turns(id).ok_or("meeting_worker_failed")?;
            let speakers = turns
                .iter()
                .map(|t| t.speaker.as_str())
                .collect::<std::collections::BTreeSet<_>>()
                .len();
            self.write_markdown(id);
            self.update(id, |meta| {
                meta.state = "ready".to_owned();
                meta.progress = 1000;
                meta.stage = None;
                meta.speakers = u16::try_from(speakers).unwrap_or(u16::MAX);
            });
            Ok(())
        }
    }

    /// What is being recorded now (a meeting, a live share or a session):
    /// its id, its title, and whether the screen is kept.
    pub fn recording_now(&self) -> Option<(String, String, bool)> {
        let id = self.inner.lock().ok()?.recording.as_ref()?.id.clone();
        let meta = self.read_meta(&id);
        let title = meta.as_ref().map(|m| m.title.clone()).unwrap_or_default();
        let screen = meta.is_some_and(|m| m.screen_on);
        Some((id, title, screen))
    }

    /// While `id` is being recorded: what each shared screen shows now
    /// (`screens/now-N.png`) and what has been said so far, newest last,
    /// cut to about `budget` bytes. For the assistant during a live share.
    pub fn live_view(&self, id: &str, budget: usize) -> Option<(Vec<PathBuf>, String)> {
        let recording = self
            .inner
            .lock()
            .ok()
            .is_some_and(|inner| inner.recording.as_ref().is_some_and(|r| r.id == id));
        if !recording {
            return None;
        }
        // With the screen switched off, its last picture is not "now".
        let screen_on = self.read_meta(id).is_some_and(|meta| meta.screen_on);
        let screens = self.dir(id).join(SCREENS);
        let mut pictures: Vec<PathBuf> = (0..4)
            .filter(|_| screen_on)
            .map(|n| screens.join(format!("now-{n}.png")))
            .filter(|path| path.is_file())
            .collect();
        pictures.truncate(2);
        let mut lines: Vec<String> = Vec::new();
        let mut used = 0;
        for turn in self.turns(id).unwrap_or_default().iter().rev() {
            let seconds = turn.start_ms / 1000;
            let line = format!("[{}:{:02}] {}: {}", seconds / 60, seconds % 60, turn.speaker, turn.text.trim());
            used += line.len() + 1;
            if used > budget {
                break;
            }
            lines.push(line);
        }
        lines.reverse();
        Some((pictures, lines.join("\n")))
    }

    /// The transcript: the final one, or until it is written, the turns
    /// transcribed live.
    fn turns(&self, id: &str) -> Option<Vec<MeetingTurnDto>> {
        let dir = self.dir(id);
        let turns: Vec<WorkerTurn> = match std::fs::read(dir.join("transcript.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes).ok()?,
            Err(_) => {
                let text = std::fs::read_to_string(dir.join("live.jsonl")).ok()?;
                let mut turns: Vec<WorkerTurn> = text
                    .lines()
                    .filter_map(|line| serde_json::from_str(line).ok())
                    .collect();
                turns.sort_by(|a, b| a.start.total_cmp(&b.start));
                turns
            }
        };
        let gaps = self.read_meta(id).map(|meta| meta.gaps).unwrap_or_default();
        Some(
            turns
                .into_iter()
                .map(|t| MeetingTurnDto {
                    start_ms: sound_to_clock(seconds_to_ms(t.start), &gaps, &t.stream),
                    end_ms: sound_to_clock(seconds_to_ms(t.end), &gaps, &t.stream),
                    speaker: t.speaker,
                    source: t.stream,
                    text: t.text,
                    language: t.language,
                    partial: false,
                })
                .collect(),
        )
    }

/// While recording: the words being said now (`live-partial.json`, which
    /// the live worker rewrites), as a last turn marked partial.
    fn partial_turn(&self, id: &str) -> Option<MeetingTurnDto> {
        if self.recording_now().is_none_or(|(recording, _, _)| recording != id) {
            return None;
        }
        #[derive(Deserialize)]
        struct Partial {
            stream: String,
            start: f64,
            end: f64,
            text: String,
            #[serde(default)]
            language: String,
        }
        let bytes = std::fs::read(self.dir(id).join("live-partial.json")).ok()?;
        let partial: Partial = serde_json::from_slice(&bytes).ok()?;
        if partial.text.trim().is_empty() {
            return None;
        }
        let gaps = self.read_meta(id).map(|meta| meta.gaps).unwrap_or_default();
        Some(MeetingTurnDto {
            speaker: String::new(),
            start_ms: sound_to_clock(seconds_to_ms(partial.start), &gaps, &partial.stream),
            end_ms: sound_to_clock(seconds_to_ms(partial.end), &gaps, &partial.stream),
            source: partial.stream,
            text: partial.text,
            language: partial.language,
            partial: true,
        })
    }

    fn names(&self, id: &str) -> BTreeMap<String, String> {
        std::fs::read(self.dir(id).join("names.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// `transcript.md` beside the recording, with the speakers' names.
    fn write_markdown(&self, id: &str) {
        let (Some(meta), Some(turns)) = (self.read_meta(id), self.turns(id)) else {
            return;
        };
        let names = self.names(id);
        let dir = self.dir(id);
        let mut text = markdown(&meta, &turns, &names);
        let slides = self.slides(id);
        if !slides.is_empty() {
            text.push_str("\n## On screen\n");
            for slide in &slides {
                text.push_str(&format!(
                    "\n**[{}]** ![Screen at {0}]({SCREENS}/{})\n",
                    short_clock(slide.at_ms),
                    slide.file
                ));
                if !slide.description.is_empty() {
                    text.push_str(&format!("\n{}\n", slide.description));
                }
            }
        }
        let _ = std::fs::write(dir.join("transcript.md"), text);
        // Subtitles, for a recording that is a video (or to read along).
        let _ = std::fs::write(dir.join("transcript.srt"), subtitles(&turns, &names, false));
        let _ = std::fs::write(dir.join("transcript.vtt"), subtitles(&turns, &names, true));
    }

    #[must_use]
    pub fn list(&self) -> Vec<MeetingDto> {
        let recording = self.recording_elapsed();
        self.all()
            .into_iter()
            .map(|mut meta| {
                if let Some((id, elapsed)) = &recording
                    && *id == meta.id
                {
                    meta.duration_ms = *elapsed;
                }
                meta.dto()
            })
            .collect()
    }

    fn recording_elapsed(&self) -> Option<(String, u64)> {
        let inner = self.inner.lock().ok()?;
        let recording = inner.recording.as_ref()?;
        Some((
            recording.id.clone(),
            u64::try_from(recording.started.elapsed().as_millis()).unwrap_or(u64::MAX),
        ))
    }

    /// One meeting with its transcript.
    ///
    /// # Errors
    /// `meeting_not_found`.
    pub fn get(&self, id: &str) -> Result<MeetingDetailDto, &'static str> {
        let mut meta = self.read_meta(id).ok_or("meeting_not_found")?;
        if let Some((recording, elapsed)) = self.recording_elapsed()
            && recording == id
        {
            meta.duration_ms = elapsed;
        }
        Ok(MeetingDetailDto {
            meeting: MeetingDto { paused: self.paused(id), ..meta.dto() },
            turns: {
                let mut turns = self.turns(id).unwrap_or_default();
                turns.extend(self.partial_turn(id));
                turns
            },
            names: self.names(id),
            folder: self.dir(id).display().to_string(),
            models: installed_models(),
            notes: self.notes(id),
            recording: self.dir(id).join("recording.wav").is_file(),
            slides: self.slides(id),
            screen_error: std::fs::read_to_string(self.dir(id).join(SCREENS).join("error"))
                .ok()
                .map(|code| code.trim().to_owned())
                .filter(|code| !code.is_empty()),
            screen_sources: std::fs::read(self.dir(id).join(SCREENS).join("sources.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or_default(),
        })
    }

    /// The pictures kept of the screen, with what the model saw on them.
    fn slides(&self, id: &str) -> Vec<MeetingSlideDto> {
        let dir = self.dir(id).join(SCREENS);
        let Ok(text) = std::fs::read_to_string(dir.join("frames.jsonl")) else {
            return Vec::new();
        };
        let described: BTreeMap<String, String> = std::fs::read(dir.join("slides.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        text.lines()
            .filter_map(|line| serde_json::from_str::<Frame>(line).ok())
            .filter(|frame| valid_frame_name(&frame.file))
            .map(|frame| MeetingSlideDto {
                description: described.get(&frame.file).cloned().unwrap_or_default(),
                file: frame.file,
                at_ms: frame.at_ms,
            })
            .collect()
    }

    /// A screen picture, small, as base64 PNG.
    ///
    /// # Errors
    /// `meeting_slide_not_found`.
    pub fn slide(&self, id: &str, file: &str) -> Result<String, &'static str> {
        // `live-0.png`: what a shared screen shows now.
        let live = file.len() == 10
            && file.starts_with("live-")
            && file.ends_with(".png")
            && file.as_bytes()[5].is_ascii_digit();
        if !valid_id(id) || !(live || valid_frame_name(file)) {
            return Err("meeting_slide_not_found");
        }
        let thumb = if live { file.to_owned() } else { file.replace("frame-", "thumb-") };
        let bytes = std::fs::read(self.dir(id).join(SCREENS).join(thumb))
            .map_err(|_| "meeting_slide_not_found")?;
        Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    /// Ask the assistant's model what each new screen picture shows (after
    /// the transcript, so the model is not loaded during the call).
    fn describe_slides(self: &Arc<Self>, id: &str) {
        let slides = self.slides(id);
        if slides.iter().all(|slide| !slide.description.is_empty()) {
            return;
        }
        let Ok(assistant) = super::assistant::shared(&self.state) else {
            return;
        };
        let dir = self.dir(id).join(SCREENS);
        let mut described: BTreeMap<String, String> = std::fs::read(dir.join("slides.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let total = slides.len();
        for (i, slide) in slides.iter().enumerate() {
            if !slide.description.is_empty() {
                continue;
            }
            if !self.proceed(id) {
                break;
            }
            self.state.publish_assistant(DaemonEvent::MeetingChanged {
                id: id.to_owned(),
                state: "ready".to_owned(),
                progress: u16::try_from((i + 1) * 1000 / total.max(1)).unwrap_or(1000),
                stage: Some(format!("slides:{}/{total}", i + 1)),
            });
            let picture = dir.join(&slide.file);
            let messages = serde_json::json!([
                {"role": "system", "content": SLIDE_TASK},
                {"role": "user", "content": format!(
                    "Screen at {}.", short_clock(slide.at_ms)
                ), "images": [picture.to_string_lossy()]},
            ]);
            let answer = assistant.ask(None, &messages).ok().and_then(|reply| reply.recv().ok());
            match answer {
                Some((outcome, text)) if outcome == "ok" && !text.trim().is_empty() => {
                    described.insert(slide.file.clone(), text.trim().to_owned());
                    if let Ok(bytes) = serde_json::to_vec(&described) {
                        let _ = std::fs::write(dir.join("slides.json"), bytes);
                    }
                }
                // The model is missing or cannot see pictures: stop asking.
                _ => break,
            }
        }
        self.write_markdown(id);
        if let Some(meta) = self.read_meta(id) {
            self.publish(&meta);
        }
    }

    pub(crate) fn notes(&self, id: &str) -> BTreeMap<String, String> {
        NOTE_KINDS
            .iter()
            .filter_map(|kind| {
                std::fs::read_to_string(self.dir(id).join(format!("notes-{kind}.md")))
                    .ok()
                    .map(|text| ((*kind).to_owned(), text))
            })
            .collect()
    }

    /// The transcript as `[mm:ss] Speaker: text` lines, with names.
    fn material(&self, id: &str) -> Option<String> {
        let turns = self.turns(id)?;
        let names = self.names(id);
        let mut lines: Vec<(u64, String)> = turns
            .iter()
            .map(|turn| {
                let who = names.get(&turn.speaker).unwrap_or(&turn.speaker);
                (
                    turn.start_ms,
                    format!("[{}] {who}: {}", short_clock(turn.start_ms), turn.text),
                )
            })
            .collect();
        // What was on screen, where it was shown.
        for slide in self.slides(id) {
            if !slide.description.is_empty() && slide.description != "Video call." {
                lines.push((
                    slide.at_ms,
                    format!(
                        "[{}] (On screen) {}",
                        short_clock(slide.at_ms),
                        slide.description.replace('\n', " ")
                    ),
                ));
            }
        }
        lines.sort_by_key(|(at, _)| *at);
        Some(lines.into_iter().map(|(_, line)| line).collect::<Vec<_>>().join("\n"))
    }

    /// Write notes of `kind` for a meeting with the assistant model. A long
    /// transcript is first condensed part by part (the assistant's model may
    /// hold only a few thousand tokens), quietly; the final notes stream to
    /// clients under `turn_id` and are saved with the meeting.
    ///
    /// # Errors
    /// A short code: unknown meeting or kind, no transcript, no assistant.
    pub fn summarize(
        self: &Arc<Self>,
        assistant: Arc<super::assistant::Assistant>,
        id: &str,
        turn_id: String,
        kind: &str,
        instruction: Option<String>,
    ) -> Result<(), &'static str> {
        let kind = NOTE_KINDS
            .iter()
            .copied()
            .find(|k| *k == kind)
            .ok_or("meeting_notes_kind_unknown")?;
        let meta = self.read_meta(id).ok_or("meeting_not_found")?;
        if meta.state != "ready" {
            return Err("meeting_not_transcribed");
        }
        let material = self
            .material(id)
            .filter(|m| !m.trim().is_empty())
            .ok_or("meeting_no_transcript")?;
        let task = notes_task(kind, instruction.as_deref())?;
        let meetings = Arc::clone(self);
        let id = id.to_owned();
        std::thread::Builder::new()
            .name("narrivy-meeting-notes".into())
            .spawn(move || {
                let outcome = meetings.write_notes(&assistant, &meta, &material, &task, turn_id);
                match outcome {
                    Ok(text) => {
                        let _ = std::fs::write(
                            meetings.dir(&id).join(format!("notes-{kind}.md")),
                            text,
                        );
                    }
                    Err(code) => eprintln!("narrivyd: meeting notes failed ({code})"),
                }
                meetings.set_control(&id, None);
                if let Some(meta) = meetings.read_meta(&id) {
                    meetings.publish(&meta);
                }
            })
            .map_err(|_| "meeting_unavailable")?;
        Ok(())
    }

    fn write_notes(
        &self,
        assistant: &super::assistant::Assistant,
        meta: &Meta,
        material: &str,
        task: &str,
        turn_id: String,
    ) -> Result<String, &'static str> {
        let parts = split_material(material, PART_CHARS);
        let source = if parts.len() == 1 {
            format!("Meeting: {}\n\nTranscript:\n{material}", meta.title)
        } else {
            let mut condensed = Vec::with_capacity(parts.len());
            for (i, part) in parts.iter().enumerate() {
                if !self.proceed(&meta.id) {
                    return Err("meeting_stopped");
                }
                self.publish_notes_stage(&meta.id, i + 1, parts.len());
                let messages = serde_json::json!([
                    {"role": "system", "content": PART_TASK},
                    {"role": "user", "content": format!(
                        "Meeting: {}\nPart {} of {}:\n{part}", meta.title, i + 1, parts.len()
                    )},
                ]);
                let (outcome, text) = assistant
                    .ask(None, &messages)?
                    .recv()
                    .map_err(|_| "assistant_unavailable")?;
                if outcome != "ok" {
                    return Err("meeting_notes_failed");
                }
                condensed.push(format!(
                    "Part {} of {}:\n{}",
                    i + 1,
                    parts.len(),
                    text.trim()
                ));
            }
            format!(
                "Meeting: {}\n\nThe transcript was long, so these are condensed notes of its consecutive parts:\n\n{}",
                meta.title,
                condensed.join("\n\n")
            )
        };
        if !self.proceed(&meta.id) {
            return Err("meeting_stopped");
        }
        let messages = serde_json::json!([
            {"role": "system", "content": task},
            {"role": "user", "content": source},
        ]);
        if let Ok(mut inner) = self.inner.lock() {
            inner.asking = Some((meta.id.clone(), turn_id.clone()));
        }
        let answer = assistant.ask(Some(turn_id), &messages).map(|reply| reply.recv());
        if let Ok(mut inner) = self.inner.lock() {
            inner.asking = None;
        }
        let (outcome, text) = answer?.map_err(|_| "assistant_unavailable")?;
        if outcome == "ok" && !text.trim().is_empty() {
            Ok(text)
        } else {
            Err("meeting_notes_failed")
        }
    }

    fn publish_notes_stage(&self, id: &str, part: usize, parts: usize) {
        self.state.publish_assistant(DaemonEvent::MeetingChanged {
            id: id.to_owned(),
            state: "ready".to_owned(),
            progress: u16::try_from(part * 1000 / parts.max(1)).unwrap_or(1000),
            stage: Some(format!("notes:{part}/{parts}")),
        });
    }

    /// Name a speaker label; an empty name restores the label.
    ///
    /// # Errors
    /// `meeting_not_found`.
    pub fn rename(&self, id: &str, speaker: &str, name: &str) -> Result<(), &'static str> {
        let meta = self.read_meta(id).ok_or("meeting_not_found")?;
        let mut names = self.names(id);
        let name = name.trim();
        if name.is_empty() {
            names.remove(speaker);
        } else {
            names.insert(speaker.to_owned(), name.to_owned());
        }
        let bytes = serde_json::to_vec_pretty(&names).map_err(|_| "meeting_storage_unavailable")?;
        std::fs::write(self.dir(id).join("names.json"), bytes)
            .map_err(|_| "meeting_storage_unavailable")?;
        self.write_markdown(id);
        if !name.is_empty() {
            remember_voice(&self.dir(id), speaker, name);
        }
        // People are counted by name: voices given one name are one person.
        let people = self
            .turns(id)
            .unwrap_or_default()
            .iter()
            .map(|t| names.get(&t.speaker).cloned().unwrap_or_else(|| t.speaker.clone()))
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        let meta = self
            .update(id, |meta| meta.speakers = u16::try_from(people).unwrap_or(u16::MAX))
            .unwrap_or(meta);
        self.publish(&meta);
        Ok(())
    }

    /// Delete a meeting's recording and transcript.
    ///
    /// # Errors
    /// `meeting_not_found`, or the meeting is recording or transcribing.
    pub fn delete(&self, id: &str) -> Result<(), &'static str> {
        {
            let inner = self.inner.lock().map_err(|_| "meeting_unavailable")?;
            if inner.recording.as_ref().is_some_and(|r| r.id == id)
                || inner.processing.as_deref() == Some(id)
            {
                return Err("meeting_in_use");
            }
        }
        let meta = self.read_meta(id).ok_or("meeting_not_found")?;
        std::fs::remove_dir_all(self.dir(id)).map_err(|_| "meeting_storage_unavailable")?;
        self.state.publish_assistant(DaemonEvent::MeetingChanged {
            id: meta.id,
            state: "deleted".to_owned(),
            progress: 0,
            stage: None,
        });
        Ok(())
    }
}

/// Start `pw-record` for the microphone (`mic.wav`) and the computer's
/// sound (`system.wav`) as `devices` says.
/// The microphone's stream and the computer sound's, as the worker names
/// them.
const ROOM: &str = "room";
const CALL: &str = "call";

fn start_recorders(dir: &Path, devices: &Devices) -> Result<Vec<Recorder>, &'static str> {
    let mut targets = Vec::new();
    match devices.microphone_target.clone() {
        Some(mic) => targets.push((ROOM, mic)),
        None if !devices.microphone_off => return Err("meeting_no_microphone"),
        None => {}
    }
    if let Some(sink) = devices.speaker_target.clone() {
        targets.push((CALL, sink));
    }
    let mut recorders: Vec<Recorder> = Vec::new();
    for (stream, target) in targets {
        match Recorder::start(dir, stream, &target) {
            Ok(recorder) => recorders.push(recorder),
            Err(code) => {
                finish_recorders(recorders);
                return Err(code);
            }
        }
    }
    Ok(recorders)
}

const RATE: u32 = 16_000;

/// A 16 kHz mono 16-bit WAV header for `data` bytes of samples. While the
/// file grows the sizes are left at their largest; `seal_wav` sets them.
fn wav_header(data: u32) -> [u8; 44] {
    let mut h = [0u8; 44];
    h[0..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&data.saturating_add(36).to_le_bytes());
    h[8..16].copy_from_slice(b"WAVEfmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&1u16.to_le_bytes());
    h[22..24].copy_from_slice(&1u16.to_le_bytes());
    h[24..28].copy_from_slice(&RATE.to_le_bytes());
    h[28..32].copy_from_slice(&(RATE * 2).to_le_bytes());
    h[32..34].copy_from_slice(&2u16.to_le_bytes());
    h[34..36].copy_from_slice(&16u16.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&data.to_le_bytes());
    h
}

/// Set the sizes in a finished recording's header from its length.
fn seal_wav(path: &Path) {
    use std::io::{Seek, SeekFrom, Write};
    let Ok(mut file) = std::fs::OpenOptions::new().write(true).open(path) else {
        return;
    };
    let Ok(len) = file.metadata().map(|m| m.len()) else {
        return;
    };
    // A whole number of samples.
    let data = u32::try_from(len.saturating_sub(44)).unwrap_or(u32::MAX - 36) & !1;
    let _ = file.set_len(44 + u64::from(data));
    let _ = file.seek(SeekFrom::Start(0)).and_then(|_| file.write_all(&wav_header(data)));
}

/// Record one stream, appending to its file (`mic.wav` or `system.wav`),
/// which starts with a header the first time.
fn start_recorder(
    dir: &Path,
    stream: &str,
    target: &str,
) -> Result<(Child, std::thread::JoinHandle<()>), &'static str> {
    use std::io::Write;
    let file = if stream == ROOM { "mic.wav" } else { "system.wav" };
    let path = dir.join(file);
    let mut out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|_| "meeting_storage_unavailable")?;
    if out.metadata().map_err(|_| "meeting_storage_unavailable")?.len() == 0 {
        out.write_all(&wav_header(u32::MAX - 36)).map_err(|_| "meeting_storage_unavailable")?;
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(format!("{file}.log")))
        .map_err(|_| "meeting_storage_unavailable")?;
    let mut child = Command::new("pw-record")
        .args(["--raw", "--target", target, "--rate", "16000", "--channels", "1"])
        .args(["--format", "s16"])
        // PipeWire has no `<sink>.monitor` node: asking for one silently
        // records the default microphone instead. The output's sound is
        // captured by targeting the sink itself with `stream.capture.sink`.
        .args(if stream == CALL {
            &["-P", "{ stream.capture.sink = true }"][..]
        } else {
            &[][..]
        })
        .arg("-")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(log)
        .spawn()
        .map_err(|_| "meeting_recorder_missing")?;
    let Some(mut samples) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("meeting_recorder_missing");
    };
    let pump = std::thread::Builder::new()
        .name("narrivy-recorder".into())
        .spawn(move || {
            let _ = std::io::copy(&mut samples, &mut out);
        });
    match pump {
        Ok(pump) => Ok((child, pump)),
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            Err("meeting_unavailable")
        }
    }
}

/// End the recorders and wait for what they wrote to reach the files.
fn finish_recorders(recorders: Vec<Recorder>) {
    let mut children = Vec::new();
    let mut pumps = Vec::new();
    for recorder in recorders {
        children.extend(recorder.child);
        pumps.extend(recorder.pump);
    }
    stop_children(&mut children);
    for pump in pumps {
        let _ = pump.join();
    }
}

/// Switch one stream of a recording on or off. Off ends its recorder, so
/// the desktop stops showing the sound as recorded (its file skips the
/// time); on starts a new one appending to the same file, noting the gap,
/// or starts it late, the time before it a gap.
fn switch_sound(
    recording: &mut Recording,
    dir: &Path,
    stream: &'static str,
    on: bool,
    now: u64,
) -> Result<Option<Gap>, &'static str> {
    let gap = |at_ms: u64, ms: u64| Gap { at_ms, ms, stream: Some(stream.to_owned()) };
    if let Some(recorder) = recording.children.iter_mut().find(|r| r.stream == stream) {
        return Ok(match (on, recorder.off_since) {
            (false, None) => {
                let mut children: Vec<Child> = recorder.child.take().into_iter().collect();
                stop_children(&mut children);
                if let Some(pump) = recorder.pump.take() {
                    let _ = pump.join();
                }
                recorder.off_since = Some(now);
                None
            }
            (true, Some(since)) => {
                let target = target_for(recording.chosen.clone(), stream)?.0;
                let recorder = recording
                    .children
                    .iter_mut()
                    .find(|r| r.stream == stream)
                    .ok_or("meeting_unavailable")?;
                let (child, pump) = start_recorder(dir, stream, &target)?;
                recorder.child = Some(child);
                recorder.pump = Some(pump);
                recorder.off_since = None;
                Some(gap(since, now.saturating_sub(since)))
            }
            _ => None,
        });
    }
    if !on {
        return Ok(None);
    }
    let (target, devices) = target_for(recording.chosen.clone(), stream)?;
    recording.children.push(Recorder::start(dir, stream, &target)?);
    if stream == ROOM && recording.microphone.is_empty() {
        recording.microphone = devices.microphone;
    }
    if stream == CALL && recording.speaker.is_empty() {
        recording.speaker = devices.speaker;
    }
    Ok((now > 0).then(|| gap(0, now)))
}

/// The device a stream records from, as chosen for the recording.
fn target_for(mut chosen: MeetingOptions, stream: &str) -> Result<(String, Devices), &'static str> {
    if stream == ROOM {
        chosen.microphone_off = false;
    } else {
        chosen.call_audio_off = false;
    }
    let devices = devices_for(chosen);
    let target = if stream == ROOM {
        devices.microphone_target.clone().ok_or("meeting_no_microphone")?
    } else {
        devices.speaker_target.clone().ok_or("meeting_no_speaker")?
    };
    Ok((target, devices))
}

fn signal(pid: u32, which: &str) {
    let _ = Command::new("kill").args([which, &pid.to_string()]).status();
}

/// Ask the recorders to finish (SIGINT), then make sure they are gone.
fn stop_children(children: &mut [Child]) {
    for child in children.iter() {
        let _ = Command::new("kill")
            .args(["-INT", &child.id().to_string()])
            .status();
    }
    for child in children.iter_mut() {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if matches!(child.try_wait(), Ok(Some(_))) {
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// Ask the live worker to finish (it transcribes what is left), within a
/// bound.
fn finish_live(live: &mut Child) {
    let _ = Command::new("kill")
        .args(["-INT", &live.id().to_string()])
        .status();
    let deadline = Instant::now() + LIVE_FINISH_BOUND;
    while Instant::now() < deadline {
        if matches!(live.try_wait(), Ok(Some(_)) | Err(_)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = live.kill();
    let _ = live.wait();
}

/// The number of turns written so far, from one live worker line.
fn live_turns(line: &str) -> Option<u32> {
    #[derive(Deserialize)]
    struct Line {
        stage: String,
        done: u32,
    }
    let line: Line = serde_json::from_str(line).ok()?;
    (line.stage == "live").then_some(line.done)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct MeetingOptions {
    /// Write the detailed notes as soon as the transcript is ready.
    #[serde(default)]
    auto_notes: bool,
    /// The microphone meetings record; `None` follows the Microphone stage.
    #[serde(default)]
    microphone: Option<String>,
    /// The output whose sound is recorded (the call); `None` is the
    /// system's default output.
    #[serde(default)]
    speaker: Option<String>,
    /// Leave the call's sound out: only the microphone is recorded.
    #[serde(default)]
    call_audio_off: bool,
    /// Leave the microphone out: only the computer's sound is recorded
    /// (transcribing a video or a call you only listen to).
    #[serde(default)]
    microphone_off: bool,
    /// Keep pictures of what is shown on screen (D34).
    #[serde(default)]
    screen: bool,
    /// Go over a recording as soon as it stops. Off, it is kept as heard
    /// (quick, light) until the user asks.
    #[serde(default)]
    process_on_stop: bool,
}

fn options_path() -> Option<PathBuf> {
    narrivy_platform_linux::instance::state_dir()
        .ok()
        .map(|dir| dir.join("meetings.json"))
}

fn options() -> MeetingOptions {
    options_path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save(options: &MeetingOptions) -> Result<(), &'static str> {
    let path = options_path().ok_or("meeting_storage_unavailable")?;
    let bytes = serde_json::to_vec(options).map_err(|_| "meeting_storage_unavailable")?;
    std::fs::write(path, bytes).map_err(|_| "meeting_storage_unavailable")
}

/// Whether notes are written as soon as a meeting is transcribed.
#[must_use]
pub fn auto_notes() -> bool {
    options().auto_notes
}

/// # Errors
/// `meeting_storage_unavailable` when the choice cannot be saved.
pub fn set_auto_notes(on: bool) -> Result<(), &'static str> {
    save(&MeetingOptions {
        auto_notes: on,
        ..options()
    })
}

/// Go over a recording as soon as it stops.
///
/// # Errors
/// `meeting_storage_unavailable` when the choice cannot be saved.
pub fn set_process_on_stop(on: bool) -> Result<(), &'static str> {
    save(&MeetingOptions {
        process_on_stop: on,
        ..options()
    })
}

/// Keep pictures of the screen in meetings (asked for with the portal when
/// a meeting starts).
///
/// # Errors
/// `meeting_storage_unavailable` when the choice cannot be saved.
pub fn set_screen(on: bool) -> Result<(), &'static str> {
    save(&MeetingOptions {
        screen: on,
        ..options()
    })
}

/// Whether screens can be kept here: the helper and GStreamer's PipeWire
/// plugin are installed.
fn screen_available() -> bool {
    std::env::var_os(SCREEN_SCRIPT_ENV).is_some()
        && ["/usr/lib/x86_64-linux-gnu", "/usr/lib/aarch64-linux-gnu", "/usr/lib64", "/usr/lib"]
            .iter()
            .any(|lib| Path::new(lib).join("gstreamer-1.0/libgstpipewire.so").is_file())
}

/// Start keeping pictures of the screen the user shares; the portal asks
/// which. When it cannot, the reason is left in `screens/error` and the
/// meeting goes on without.
fn start_screen(
    dir: &Path,
    started_unix_ms: u64,
    moving_every: u32,
    token: &Arc<Mutex<Option<String>>>,
) -> Option<Child> {
    let screens = dir.join(SCREENS);
    let script = std::env::var_os(SCREEN_SCRIPT_ENV)?;
    let python = std::env::var_os(SCREEN_PYTHON_ENV).unwrap_or_else(|| "/usr/bin/python3".into());
    if create_private_dir(&screens).is_err() {
        return None;
    }
    let mut command = Command::new(python);
    command
        .arg(script)
        .arg(&screens)
        .args(["--start-ms", &started_unix_ms.to_string()])
        .args(["--moving-every", &moving_every.to_string()]);
    // Switched back on: the screen chosen before, without asking again.
    if let Some(saved) = token.lock().ok().and_then(|t| t.clone()) {
        command.env("NARRIVY_SCREEN_RESTORE", saved);
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    let token = Arc::clone(token);
    let _ = std::thread::Builder::new()
        .name("narrivy-meeting-screen".into())
        .spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Some(saved) = serde_json::from_str::<serde_json::Value>(&line)
                    .ok()
                    .and_then(|v| v.get("restore_token").and_then(|t| t.as_str()).map(str::to_owned))
                    && let Ok(mut slot) = token.lock()
                {
                    *slot = Some(saved);
                }
                let code = serde_json::from_str::<serde_json::Value>(&line)
                    .ok()
                    .filter(|v| v.get("ev").and_then(|e| e.as_str()) == Some("error"))
                    .and_then(|v| v.get("code").and_then(|c| c.as_str()).map(str::to_owned));
                if let Some(code) = code.filter(|c| c.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')) {
                    let _ = std::fs::write(screens.join("error"), code);
                }
            }
        });
    Some(child)
}

/// `frame-0001.png`: a picture's name as the capture writes it.
fn valid_frame_name(name: &str) -> bool {
    name.len() == 14
        && name.starts_with("frame-")
        && name.ends_with(".png")
        && name[6..10].bytes().all(|b| b.is_ascii_digit())
}

#[derive(Deserialize)]
struct Frame {
    file: String,
    at_ms: u64,
}

/// Choose the meeting's microphone and output; `Some("")` goes back to the
/// default, `None` leaves a choice as it is. A device that is unplugged
/// stays chosen, and the default is used until it returns.
///
/// # Errors
/// `meeting_device_invalid` for a name PipeWire could not have,
/// `meeting_storage_unavailable` when it cannot be saved.
pub fn set_devices(
    microphone: Option<String>,
    speaker: Option<String>,
    call_audio: Option<bool>,
    room_audio: Option<bool>,
) -> Result<(), &'static str> {
    let pick = |name: Option<String>, current: Option<String>| match name {
        None => Ok(current),
        Some(name) if name.is_empty() => Ok(None),
        Some(name) if echo_cancel::valid_node_name(&name) => Ok(Some(name)),
        Some(_) => Err("meeting_device_invalid"),
    };
    let current = options();
    let microphone = pick(microphone, current.microphone.clone())?;
    let speaker = pick(speaker, current.speaker.clone())?;
    let call_audio_off = call_audio.map_or(current.call_audio_off, |on| !on);
    let microphone_off = room_audio.map_or(current.microphone_off, |on| !on);
    if call_audio_off && microphone_off {
        return Err("meeting_nothing_to_record");
    }
    save(&MeetingOptions {
        microphone,
        speaker,
        call_audio_off,
        microphone_off,
        ..current
    })
}

/// What a meeting started now would record.
struct Devices {
    microphones: Vec<(String, String)>,
    speakers: Vec<(String, String)>,
    /// The PipeWire node to record for the room (may be the echo
    /// canceller's source, which carries the chosen microphone).
    microphone_target: Option<String>,
    speaker_target: Option<String>,
    /// The user left the microphone out.
    microphone_off: bool,
    /// The devices as the user knows them.
    microphone: String,
    speaker: String,
}

fn devices() -> Devices {
    devices_for(options())
}

/// The devices for these options (a start may override what to record).
fn devices_for(chosen: MeetingOptions) -> Devices {
    let microphones = echo_cancel::microphones();
    let speakers = echo_cancel::speakers();
    let describe = |list: &[(String, String)], name: &str| {
        list.iter()
            .find(|(n, _)| n == name)
            .map_or_else(|| name.to_owned(), |(_, d)| d.clone())
    };
    let plugged = |list: &[(String, String)], name: Option<String>| {
        name.filter(|n| list.iter().any(|(m, _)| m == n))
    };
    // The Microphone stage's microphone, through the echo canceller when
    // that is on: in the room the speakers play the call too.
    let stage = super::echo::chosen_microphone().or_else(|| pactl("get-default-source"));
    let (microphone_target, microphone) = match plugged(&microphones, chosen.microphone) {
        _ if chosen.microphone_off => (None, None),
        Some(mic) if stage.as_deref() != Some(mic.as_str()) => (Some(mic.clone()), Some(mic)),
        _ => (
            super::echo::capture_target().or_else(|| stage.clone()),
            stage,
        ),
    };
    let speaker = if chosen.call_audio_off {
        None
    } else {
        plugged(&speakers, chosen.speaker).or_else(|| pactl("get-default-sink"))
    };
    Devices {
        microphone: microphone.map(|m| describe(&microphones, &m)).unwrap_or_default(),
        speaker: speaker.as_deref().map(|s| describe(&speakers, s)).unwrap_or_default(),
        microphone_target,
        speaker_target: speaker,
        microphone_off: chosen.microphone_off,
        microphones,
        speakers,
    }
}

/// The options for clients, with the devices to choose from and the ones
/// recording now (or that would record, when no meeting is).
#[must_use]
pub fn options_dto(meetings: Option<&Meetings>) -> narrivy_ipc::MeetingOptionsDto {
    let chosen = options();
    let found = devices();
    let recording = meetings
        .and_then(|m| m.inner.lock().ok())
        .and_then(|inner| {
            inner
                .recording
                .as_ref()
                .map(|r| (r.microphone.clone(), r.speaker.clone()))
        });
    let (microphone_in_use, speaker_in_use) =
        recording.unwrap_or((found.microphone, found.speaker));
    let list = |devices: Vec<(String, String)>| {
        devices
            .into_iter()
            .map(|(name, description)| narrivy_ipc::MicrophoneDto { name, description })
            .collect()
    };
    narrivy_ipc::MeetingOptionsDto {
        auto_notes: chosen.auto_notes,
        call_audio: !chosen.call_audio_off,
        room_audio: !chosen.microphone_off,
        microphone: chosen.microphone.unwrap_or_default(),
        speaker: chosen.speaker.unwrap_or_default(),
        microphones: list(found.microphones),
        speakers: list(found.speakers),
        microphone_in_use,
        speaker_in_use,
        screen: chosen.screen,
        screen_available: screen_available(),
        process_on_stop: chosen.process_on_stop,
    }
}

/// Remembered people's voices: fingerprints of people the user named.
fn known_voices() -> Option<PathBuf> {
    narrivy_platform_linux::instance::state_dir()
        .ok()
        .map(|dir| dir.join("speakers.json"))
}

/// The user's trained voice (labelled "You") and the remembered people,
/// for the meeting worker.
fn voice_args() -> Vec<std::ffi::OsString> {
    let mut args = Vec::new();
    if let Some(profile) = super::voice::trained_profile() {
        args.push("--profile".into());
        args.push(profile.into_os_string());
    }
    if let Some(known) = known_voices() {
        args.push("--known".into());
        args.push(known.into_os_string());
    }
    args
}

/// Remember the voice the user just named, so later meetings recognise
/// them. Runs on its own; a failure only means the voice is not learnt.
fn remember_voice(dir: &Path, speaker: &str, name: &str) {
    let (Some(python), Some(script), Some(known)) = (
        std::env::var_os(PYTHON_ENV),
        std::env::var_os(SCRIPT_ENV),
        known_voices(),
    ) else {
        return;
    };
    if !dir.join("transcript.json").is_file() {
        return;
    }
    let child = Command::new(python)
        .arg(script)
        .arg("remember")
        .arg(dir)
        .args(["--speaker", speaker, "--name", name])
        .arg("--known")
        .arg(known)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if let Ok(mut child) = child {
        let _ = std::thread::Builder::new()
            .name("narrivy-remember-voice".into())
            .spawn(move || {
                let _ = child.wait();
            });
    }
}

/// Files a meeting can be imported from.
const IMPORT_EXTENSIONS: [&str; 14] = [
    "wav", "mp3", "m4a", "aac", "ogg", "oga", "opus", "flac", "webm", "mp4", "mkv", "mov", "avi", "wma",
];
const IMPORT_MAX_BYTES: u64 = 8 * 1024 * 1024 * 1024;
/// The longest a conversion may take.
const IMPORT_BOUND: Duration = Duration::from_secs(30 * 60);

#[derive(Clone)]
enum Converter {
    Ffmpeg(PathBuf),
    Gstreamer(PathBuf),
}

fn on_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")?
        .to_str()?
        .split(':')
        .map(|dir| PathBuf::from(dir).join(name))
        .find(|p| p.is_file())
}

fn converter() -> Option<Converter> {
    on_path("ffmpeg")
        .map(Converter::Ffmpeg)
        .or_else(|| on_path("gst-launch-1.0").map(Converter::Gstreamer))
}

/// Convert `source` to 16 kHz mono 16-bit WAV at `out`; the duration in ms.
fn convert(converter: &Converter, source: &Path, out: &Path) -> Result<u64, &'static str> {
    let mut command = match converter {
        Converter::Ffmpeg(ffmpeg) => {
            let mut c = Command::new(ffmpeg);
            c.args(["-nostdin", "-hide_banner", "-loglevel", "error", "-i"])
                .arg(source)
                .args(["-vn", "-ac", "1", "-ar", "16000", "-sample_fmt", "s16", "-f", "wav", "-y"])
                .arg(out);
            c
        }
        Converter::Gstreamer(gst) => {
            let mut c = Command::new(gst);
            c.arg("-q")
                .arg("filesrc")
                .arg(format!("location={}", source.display()))
                .args(["!", "decodebin", "!", "audioconvert", "!", "audioresample", "!"])
                .arg("audio/x-raw,format=S16LE,channels=1,rate=16000")
                .args(["!", "wavenc", "!", "filesink"])
                .arg(format!("location={}", out.display()));
            c
        }
    };
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "meeting_import_no_converter")?;
    let deadline = Instant::now() + IMPORT_BOUND;
    let status = loop {
        if let Ok(Some(status)) = child.try_wait() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("meeting_import_timeout");
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    let size = std::fs::metadata(out).map(|m| m.len()).unwrap_or(0);
    if !status.success() || size <= 44 + 16_000 {
        return Err("meeting_import_no_audio");
    }
    // 16 kHz, 2 bytes a sample: 32 bytes a millisecond.
    Ok((size - 44) / 32)
}

fn seconds_to_ms(seconds: f64) -> u64 {
    if seconds.is_finite() && seconds > 0.0 {
        // Meetings are hours long at most: well inside u64 and f64 precision.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let ms = (seconds * 1000.0).round() as u64;
        ms
    } else {
        0
    }
}

/// `(stage, progress 0..=1000)` from one worker progress line. Loading the
/// model is the first 5%, finding speakers up to 25%, transcribing the rest.
fn progress_of(line: &str) -> Option<(String, u16)> {
    #[derive(Deserialize)]
    struct Line {
        stage: String,
        done: u32,
        total: u32,
    }
    let line: Line = serde_json::from_str(line).ok()?;
    let fraction = (u64::from(line.done.min(line.total)) * 1000)
        .checked_div(u64::from(line.total))
        .unwrap_or(0);
    let (from, span): (u64, u64) = match line.stage.as_str() {
        "load" => (0, 50),
        "speakers" => (50, 200),
        "words" => (250, 750),
        _ => return None,
    };
    let progress = u16::try_from(from + fraction * span / 1000).unwrap_or(1000);
    Some((line.stage, progress))
}

/// Subtitles: SubRip (`.srt`) or WebVTT (`.vtt`), one cue a turn, with the
/// speaker's name.
fn subtitles(turns: &[MeetingTurnDto], names: &BTreeMap<String, String>, vtt: bool) -> String {
    let time = |ms: u64| {
        let separator = if vtt { '.' } else { ',' };
        format!(
            "{:02}:{:02}:{:02}{separator}{:03}",
            ms / 3_600_000,
            ms / 60_000 % 60,
            ms / 1000 % 60,
            ms % 1000
        )
    };
    let mut out = if vtt { String::from("WEBVTT\n\n") } else { String::new() };
    for (i, turn) in turns.iter().filter(|t| !t.text.trim().is_empty()).enumerate() {
        let who = names.get(&turn.speaker).unwrap_or(&turn.speaker);
        let end = turn.end_ms.max(turn.start_ms + 500);
        if !vtt {
            out.push_str(&format!("{}\n", i + 1));
        }
        out.push_str(&format!(
            "{} --> {}\n{who}: {}\n\n",
            time(turn.start_ms),
            time(end),
            turn.text.trim()
        ));
    }
    out
}

fn clock(ms: u64) -> String {
    let s = ms / 1000;
    format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

fn markdown(meta: &Meta, turns: &[MeetingTurnDto], names: &BTreeMap<String, String>) -> String {
    let mut out = format!(
        "# {}\n\n_Recorded with Narrivy. People are told apart by voice._\n",
        meta.title
    );
    let mut previous: Option<&str> = None;
    for turn in turns {
        let who = names
            .get(&turn.speaker)
            .map_or(turn.speaker.as_str(), String::as_str);
        if previous == Some(who) {
            out.push_str(&format!("{}\n", turn.text));
        } else {
            let lang = if turn.language.is_empty() || turn.language == "en" {
                String::new()
            } else {
                format!(" ({})", turn.language)
            };
            out.push_str(&format!(
                "\n**{who}** · {}{lang}\n\n{}\n",
                clock(turn.start_ms),
                turn.text
            ));
        }
        previous = Some(who);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recording_added_to_in_parts_is_sealed_as_one_wav() -> std::io::Result<()> {
        use std::io::Write;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("system.wav");
        let mut file = std::fs::File::create(&path)?;
        file.write_all(&wav_header(u32::MAX - 36))?;
        // Two parts, as after switching the sound off and on, and half a
        // sample cut off.
        file.write_all(&[1, 0, 2, 0])?;
        file.write_all(&[3, 0, 4])?;
        drop(file);
        seal_wav(&path);
        let bytes = std::fs::read(&path)?;
        assert_eq!(bytes.len(), 50);
        assert_eq!(&bytes[..44], &wav_header(6));
        assert_eq!(u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]), 42);
        Ok(())
    }

    #[test]
    fn sound_times_move_past_the_spans_the_sound_was_off() {
        // Off from 10 s to 15 s, and again from 30 s for 10 s.
        let gaps = [Gap { at_ms: 10_000, ms: 5_000, stream: None }, Gap { at_ms: 30_000, ms: 10_000, stream: None }];
        assert_eq!(sound_to_clock(4_000, &gaps, "call"), 4_000);
        // 12 s into the sound is 17 s on the clock.
        assert_eq!(sound_to_clock(12_000, &gaps, "call"), 17_000);
        // The second gap starts 25 s into the sound.
        assert_eq!(sound_to_clock(24_000, &gaps, "call"), 29_000);
        assert_eq!(sound_to_clock(26_000, &gaps, "call"), 41_000);
        // Sound switched on late: everything starts at the gap's end.
        assert_eq!(sound_to_clock(1_000, &[Gap { at_ms: 0, ms: 60_000, stream: None }], "call"), 61_000);
        // A gap in one stream moves only that stream's times.
        let mic_off = [Gap { at_ms: 5_000, ms: 20_000, stream: Some("room".to_owned()) }];
        assert_eq!(sound_to_clock(6_000, &mic_off, "room"), 26_000);
        assert_eq!(sound_to_clock(6_000, &mic_off, "call"), 6_000);
    }

    #[test]
    fn publishing_while_the_meetings_lock_is_held_does_not_wait() {
        // Starting a meeting publishes it with `inner` held; publishing
        // reads the pause state, which must not need `inner` (a deadlock
        // once hung "Watch with me").
        let root = std::env::temp_dir().join(format!("narrivy-meetings-lock-{}", std::process::id()));
        let meetings = Arc::new(Meetings {
            root,
            inner: Mutex::new(Inner::default()),
            control: Mutex::new(std::collections::HashMap::new()),
            state: Arc::new(DaemonState::new()),
        });
        meetings.set_control("m1", Some(false));
        let (done, finished) = std::sync::mpsc::channel();
        let worker = Arc::clone(&meetings);
        std::thread::spawn(move || {
            let _held = worker.inner.lock();
            let _ = done.send(worker.paused("m1"));
        });
        assert_eq!(finished.recv_timeout(Duration::from_secs(5)), Ok(true));
    }

    #[test]
    fn subtitles_carry_times_and_names() {
        let turn = MeetingTurnDto {
            speaker: "Person 1".into(),
            source: "room".into(),
            start_ms: 61_500,
            end_ms: 63_250,
            text: "Hello.".into(),
            language: "en".into(),
            partial: false,
        };
        let names = BTreeMap::from([("Person 1".to_owned(), "Waqar".to_owned())]);
        assert_eq!(
            subtitles(std::slice::from_ref(&turn), &names, false),
            "1\n00:01:01,500 --> 00:01:03,250\nWaqar: Hello.\n\n"
        );
        assert!(subtitles(&[turn], &names, true).starts_with("WEBVTT\n\n00:01:01.500 --> 00:01:03.250\n"));
    }

    #[test]
    fn live_lines_count_turns() {
        assert_eq!(
            live_turns(r#"{"stage":"live","done":3,"total":3}"#),
            Some(3)
        );
        assert_eq!(live_turns(r#"{"stage":"words","done":3,"total":9}"#), None);
        assert_eq!(live_turns("not json"), None);
    }

    #[test]
    fn progress_lines_map_onto_one_scale() {
        assert_eq!(
            progress_of(r#"{"stage":"load","done":1,"total":1}"#),
            Some(("load".into(), 50))
        );
        assert_eq!(
            progress_of(r#"{"stage":"speakers","done":1,"total":2}"#),
            Some(("speakers".into(), 150))
        );
        assert_eq!(
            progress_of(r#"{"stage":"words","done":7,"total":7}"#),
            Some(("words".into(), 1000))
        );
        assert_eq!(progress_of("Loading Whisper…"), None);
        assert_eq!(progress_of(r#"{"stage":"other","done":1,"total":1}"#), None);
    }

    #[test]
    fn markdown_uses_names_and_groups_a_speakers_turns() {
        let meta = Meta {
            id: "m1".into(),
            title: "Weekly".into(),
            started_unix_ms: 0,
            duration_ms: 0,
            state: "ready".into(),
            progress: 1000,
            stage: None,
            model: "turbo".into(),
            speakers: 2,
            watch: false,
            share: false,
            ..Meta::default()
        };
        let turn = |speaker: &str, start_ms, text: &str, language: &str| MeetingTurnDto {
            speaker: speaker.into(),
            source: "room".into(),
            start_ms,
            end_ms: start_ms + 1000,
            text: text.into(),
            language: language.into(),
            partial: false,
        };
        let turns = [
            turn("Person 1", 0, "Hello.", "en"),
            turn("Person 1", 2000, "Shall we start?", "en"),
            turn("Person 2", 65_000, "جی ہاں", "ur"),
        ];
        let names = BTreeMap::from([("Person 1".to_owned(), "Waqar".to_owned())]);
        let text = markdown(&meta, &turns, &names);
        assert!(text.starts_with("# Weekly"));
        assert!(text.contains("**Waqar** · 00:00:00\n\nHello.\nShall we start?\n"));
        assert!(text.contains("**Person 2** · 00:01:05 (ur)\n\nجی ہاں\n"));
    }

    #[test]
    fn long_material_is_split_at_line_ends() {
        let line = "[00:01] Person 1: ".to_owned() + &"word ".repeat(30);
        let material = vec![line.as_str(); 100].join("\n");
        let parts = split_material(&material, PART_CHARS);
        assert!(parts.len() > 1);
        assert!(parts.iter().all(|p| p.len() <= PART_CHARS));
        assert_eq!(parts.join("\n"), material);
        assert_eq!(split_material("[00:01] A: hi", PART_CHARS).len(), 1);
    }

    #[test]
    fn note_tasks_exist_for_every_kind() {
        assert!(notes_task("notes", None).is_ok_and(|t| t.contains("## Action items")));
        assert!(notes_task("email", None).is_ok_and(|t| t.contains("Subject:")));
        assert!(notes_task("custom", None).is_err());
        assert!(
            notes_task("custom", Some("Who owns the launch?"))
                .is_ok_and(|t| t.contains("Who owns"))
        );
    }

    #[test]
    fn ids_are_checked() {
        assert!(valid_id("m1791224082892"));
        assert!(!valid_id("../etc"));
        assert!(!valid_id(""));
    }
}
