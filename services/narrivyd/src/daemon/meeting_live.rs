//! Meeting transcription built into the daemon (D57): no Python, no add-on.
//!
//! A meeting's two sound files (`mic.wav` for the room, `system.wav` for
//! the call; 16 kHz mono 16-bit, still growing while recording) are read
//! as they grow, cut into pieces of speech at pauses by their loudness
//! against each file's own room level, and every piece is recognised with
//! the speech model the daemon already has loaded for dictation. Each turn
//! goes to the caller as soon as it is recognised; while recording, the
//! words being said now are recognised again every [`PARTIAL_EVERY`] for
//! the live view.
//!
//! The same loop reads a finished meeting from the start ("go over it"),
//! where it simply finds both files complete.
//!
//! Speakers are told apart by where the sound came from (the room or the
//! call), not yet by voice. Text never reaches a log.

#![forbid(unsafe_code)]

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::Serialize;

/// Samples a second in the meeting files.
pub const RATE: usize = 16_000;
/// Loudness is measured over 20 ms frames.
const FRAME: usize = RATE / 50;
/// Below this loudness (RMS) a frame is quiet, whatever the room.
const SILENCE_RMS: f32 = 0.004;
/// A frame is speech when it is this many times the room's level.
const SNR: f32 = 3.0;
/// This much speech (100 ms) in a row starts a piece...
const START_FRAMES: usize = 5;
/// ...which keeps this much before it (0.3 s), for the first sound...
const BEFORE_FRAMES: usize = 15;
/// ...and ends after this much quiet (0.7 s), kept with it: Whisper changes
/// the last word of audio that stops right after it.
const END_FRAMES: usize = 35;
/// Speech going on longer than this is cut at its quietest moment of the
/// last [`CUT_LOOK`], so words show while a long talk goes on.
const PIECE_MAX: usize = RATE * 8;
const CUT_LOOK: usize = RATE * 3 / 2;
/// A piece shorter than this is a click or a cough.
const PIECE_MIN: usize = RATE * 2 / 5;
/// The words being said now are recognised again this often...
pub const PARTIAL_EVERY: Duration = Duration::from_millis(1_500);
/// ...once there is this much of them.
const PARTIAL_MIN: usize = RATE;
/// At most this much new sound is read from a file in one step (30 s), so
/// a long finished meeting is never held in memory whole.
const READ_MAX_BYTES: usize = RATE * 2 * 30;
/// How long the loop rests when no file grew.
const TICK: Duration = Duration::from_millis(250);

/// The sound files and their streams, as the meeting folder names them.
pub const STREAMS: [(&str, &str); 2] = [("room", "mic.wav"), ("call", "system.wav")];

/// What the loop is asked to do; the owner changes it at any time.
pub const RUN: u8 = 0;
/// Recognise what is left in both files, then end.
pub const FINISH: u8 = 1;
/// End now, leaving what is not recognised yet.
pub const QUIT: u8 = 2;

/// One recognised turn, in the meeting worker's `live.jsonl` /
/// `transcript.json` form. Times are seconds of the stream's sound.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Turn {
    pub stream: String,
    pub start: f64,
    pub end: f64,
    pub speaker: String,
    pub text: String,
    pub language: String,
}

/// What recognising one piece gave.
pub enum Heard {
    /// Its words and the language they are in (empty when unknown).
    Text(String, String),
    /// No words (silence, noise, or only an invented phrase).
    Nothing,
    /// No speech model is ready: a short code for the meeting to show.
    Unavailable(&'static str),
}

/// What the loop reports to its owner.
pub trait Sink {
    /// A finished turn.
    fn turn(&mut self, turn: &Turn);
    /// The words being said now, or `None` when nothing is going on.
    fn partial(&mut self, turn: Option<&Turn>);
    /// How much of the files has been read: `(read, total)` bytes, the
    /// total as large as the files were when the loop started or now.
    fn progress(&mut self, read: u64, total: u64);
    /// A piece could not be recognised (`Some(code)`), or could again
    /// (`None`).
    fn trouble(&mut self, code: Option<&'static str>);
}

/// Recognise the speech in the meeting folder `dir` until `control` says
/// [`FINISH`] (and both files are read to their end) or [`QUIT`].
/// `partials` turns on the words-being-said-now view (while recording).
pub fn run(
    dir: &Path,
    control: &AtomicU8,
    partials: bool,
    recognise: &mut dyn FnMut(&[f32]) -> Heard,
    sink: &mut dyn Sink,
) {
    let mut tails: Vec<(Tail, Cutter)> = STREAMS
        .iter()
        .map(|(stream, file)| (Tail::new(dir.join(file)), Cutter::new(stream)))
        .collect();
    let mut speakers: Vec<&'static str> = Vec::new();
    let mut recent: Vec<Turn> = Vec::new();
    let mut partial_shown = false;
    let mut partial_at: Option<Instant> = None;
    let mut trouble = false;
    loop {
        let asked = control.load(Ordering::Acquire);
        if asked == QUIT {
            break;
        }
        let mut grew = false;
        let mut pieces = Vec::new();
        for (tail, cutter) in &mut tails {
            if let Some(samples) = tail.read() {
                grew = true;
                cutter.feed(&samples);
            }
            pieces.append(&mut cutter.pieces());
        }
        // Finished only once both files gave all they had.
        let last = asked == FINISH && !grew;
        if last {
            for (_, cutter) in &mut tails {
                pieces.extend(cutter.flush());
            }
        }
        let (read, total) = tails.iter().fold((0, 0), |(r, t), (tail, _)| {
            (r + tail.read_bytes(), t + tail.total_bytes())
        });
        sink.progress(read, total);
        pieces.sort_by_key(|piece| piece.start);
        for piece in &pieces {
            if control.load(Ordering::Acquire) == QUIT {
                return;
            }
            let Some(turn) = recognise_piece(piece, recognise, sink, &mut trouble, &mut speakers)
            else {
                continue;
            };
            // The microphone hearing the call through the speakers says the
            // same words at the same time: keep the call's copy.
            if echoes_the_call(&turn, &recent) {
                continue;
            }
            recent.retain(|r| r.end > turn.start - 30.0);
            recent.push(turn.clone());
            sink.turn(&turn);
        }
        if !pieces.is_empty() && partial_shown {
            sink.partial(None);
            partial_shown = false;
        }
        if last {
            if partial_shown {
                sink.partial(None);
            }
            break;
        }
        if partials {
            let going = tails
                .iter()
                .filter_map(|(_, cutter)| cutter.speaking())
                .max_by_key(|piece| piece.samples.len());
            match going {
                Some(piece)
                    if pieces.is_empty()
                        && partial_at.is_none_or(|at| at.elapsed() >= PARTIAL_EVERY) =>
                {
                    partial_at = Some(Instant::now());
                    if let Heard::Text(text, language) = recognise(&piece.samples)
                        && !invented_for(&text, piece.seconds())
                    {
                        sink.partial(Some(&Turn {
                            stream: piece.stream.to_owned(),
                            start: piece.start_seconds(),
                            end: piece.end_seconds(),
                            speaker: String::new(),
                            text,
                            language,
                        }));
                        partial_shown = true;
                    }
                }
                None if partial_shown => {
                    sink.partial(None);
                    partial_shown = false;
                }
                _ => {}
            }
        }
        if !grew && pieces.is_empty() {
            std::thread::sleep(TICK);
        }
    }
}

fn recognise_piece(
    piece: &Piece,
    recognise: &mut dyn FnMut(&[f32]) -> Heard,
    sink: &mut dyn Sink,
    trouble: &mut bool,
    speakers: &mut Vec<&'static str>,
) -> Option<Turn> {
    if piece.samples.len() < PIECE_MIN {
        return None;
    }
    let (text, language) = match recognise(&piece.samples) {
        Heard::Text(text, language) => (text, language),
        Heard::Nothing => return None,
        Heard::Unavailable(code) => {
            sink.trouble(Some(code));
            *trouble = true;
            return None;
        }
    };
    if *trouble {
        sink.trouble(None);
        *trouble = false;
    }
    let text = text.trim().to_owned();
    if text.is_empty() || invented_for(&text, piece.seconds()) {
        return None;
    }
    Some(Turn {
        stream: piece.stream.to_owned(),
        start: piece.start_seconds(),
        end: piece.end_seconds(),
        speaker: speaker_of(piece.stream, speakers),
        text,
        language,
    })
}

/// "Person N", one per stream, numbered as they are first heard.
fn speaker_of(stream: &'static str, speakers: &mut Vec<&'static str>) -> String {
    let n = speakers
        .iter()
        .position(|s| *s == stream)
        .unwrap_or_else(|| {
            speakers.push(stream);
            speakers.len() - 1
        });
    format!("Person {}", n + 1)
}

/// Whisper's phrase for silence, in a piece too short to have said more.
fn invented_for(text: &str, seconds: f64) -> bool {
    seconds < 1.5 && super::voice::invented(text)
}

fn echoes_the_call(turn: &Turn, recent: &[Turn]) -> bool {
    turn.stream == "room"
        && recent.iter().any(|other| {
            other.stream == "call"
                && other.start < turn.end + 1.0
                && other.end > turn.start - 1.0
                && same_words(&turn.text, &other.text)
        })
}

/// Whether two texts say mostly the same words (in any order).
fn same_words(a: &str, b: &str) -> bool {
    let words = |text: &str| -> std::collections::BTreeSet<String> {
        text.split_whitespace()
            .map(|w| {
                w.chars()
                    .filter(|c| c.is_alphanumeric())
                    .flat_map(char::to_lowercase)
                    .collect::<String>()
            })
            .filter(|w| !w.is_empty())
            .collect()
    };
    let (a, b) = (words(a), words(b));
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let shared = a.intersection(&b).count();
    shared * 10 > a.len().max(b.len()) * 6
}

/// A piece of speech of one stream; `start` and `end` count samples from
/// the start of its sound.
#[derive(Debug)]
struct Piece {
    stream: &'static str,
    start: usize,
    end: usize,
    samples: Vec<f32>,
}

impl Piece {
    #[allow(clippy::cast_precision_loss)] // sample counts stay far below 2^52
    fn start_seconds(&self) -> f64 {
        self.start as f64 / RATE as f64
    }

    #[allow(clippy::cast_precision_loss)]
    fn end_seconds(&self) -> f64 {
        self.end as f64 / RATE as f64
    }

    fn seconds(&self) -> f64 {
        self.end_seconds() - self.start_seconds()
    }
}

/// Cuts one stream's sound into pieces of speech.
struct Cutter {
    stream: &'static str,
    /// The sound from sample `base` on (what a piece may still need).
    audio: Vec<f32>,
    base: usize,
    /// The next frame to measure, as a sample count.
    scanned: usize,
    /// The room's level: follows a quieter frame at once, a louder one
    /// slowly, so speech does not become the room.
    floor: f32,
    loud_run: usize,
    quiet_run: usize,
    /// Where the piece going on started.
    piece: Option<usize>,
    done: Vec<Piece>,
}

impl Cutter {
    fn new(stream: &'static str) -> Self {
        Self {
            stream,
            audio: Vec::new(),
            base: 0,
            scanned: 0,
            floor: 0.0,
            loud_run: 0,
            quiet_run: 0,
            piece: None,
            done: Vec::new(),
        }
    }

    fn end(&self) -> usize {
        self.base + self.audio.len()
    }

    fn slice(&self, start: usize, end: usize) -> Vec<f32> {
        let from = start.saturating_sub(self.base).min(self.audio.len());
        let to = end.saturating_sub(self.base).min(self.audio.len());
        self.audio[from..to].to_vec()
    }

    #[allow(clippy::cast_precision_loss)] // FRAME is 320
    fn level(&self, at: usize) -> f32 {
        let frame = &self.audio[at - self.base..at - self.base + FRAME];
        (frame.iter().map(|s| s * s).sum::<f32>() / FRAME as f32).sqrt()
    }

    fn feed(&mut self, samples: &[f32]) {
        self.audio.extend_from_slice(samples);
        while self.scanned + FRAME <= self.end() {
            let at = self.scanned;
            let level = self.level(at);
            if self.floor <= 0.0 || level < self.floor {
                self.floor = level.max(1e-5);
            } else {
                self.floor += (level - self.floor) * 0.0005;
            }
            let loud = level >= (self.floor * SNR).max(SILENCE_RMS);
            self.scanned += FRAME;
            match self.piece {
                None if loud => {
                    self.loud_run += 1;
                    if self.loud_run >= START_FRAMES {
                        let back = (self.loud_run + BEFORE_FRAMES) * FRAME;
                        self.piece = Some(self.scanned.saturating_sub(back).max(self.base));
                        self.quiet_run = 0;
                    }
                }
                None => self.loud_run = 0,
                Some(start) => {
                    self.quiet_run = if loud { 0 } else { self.quiet_run + 1 };
                    if self.quiet_run >= END_FRAMES {
                        self.close(start, self.scanned);
                    } else if self.scanned - start >= PIECE_MAX {
                        let cut = self.quietest(self.scanned - CUT_LOOK, self.scanned);
                        self.close(start, cut);
                        self.piece = Some(cut);
                    }
                }
            }
        }
        self.trim();
    }

    /// The start of the quietest frame between `from` and `to`.
    fn quietest(&self, from: usize, to: usize) -> usize {
        let mut best = (f32::MAX, to);
        let mut at = from.max(self.base);
        while at + FRAME <= to {
            let level = self.level(at);
            if level < best.0 {
                best = (level, at);
            }
            at += FRAME;
        }
        best.1
    }

    fn close(&mut self, start: usize, end: usize) {
        let samples = self.slice(start, end);
        self.done.push(Piece {
            stream: self.stream,
            start,
            end,
            samples,
        });
        self.piece = None;
        self.loud_run = 0;
        self.quiet_run = 0;
    }

    /// Drop the sound no piece can need any more.
    fn trim(&mut self) {
        let keep_from = self.piece.unwrap_or_else(|| {
            self.scanned
                .saturating_sub((BEFORE_FRAMES + START_FRAMES) * FRAME)
        });
        let drop = keep_from.saturating_sub(self.base).min(self.audio.len());
        if drop > 0 {
            self.audio.drain(..drop);
            self.base += drop;
        }
    }

    fn pieces(&mut self) -> Vec<Piece> {
        std::mem::take(&mut self.done)
    }

    /// The end of the sound: the piece going on ends here.
    fn flush(&mut self) -> Vec<Piece> {
        if let Some(start) = self.piece {
            self.close(start, self.end());
        }
        self.pieces()
    }

    /// The piece going on, when it is long enough to show.
    fn speaking(&self) -> Option<Piece> {
        let start = self.piece?;
        let end = self.end();
        (end - start >= PARTIAL_MIN).then(|| Piece {
            stream: self.stream,
            start,
            end,
            samples: self.slice(start, end),
        })
    }
}

/// New samples appended to a WAV file that may still be growing.
struct Tail {
    path: PathBuf,
    file: Option<File>,
    /// Where the samples start, once the header is there.
    data: Option<u64>,
    offset: u64,
    carry: Option<u8>,
}

impl Tail {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            file: None,
            data: None,
            offset: 0,
            carry: None,
        }
    }

    fn read_bytes(&self) -> u64 {
        self.offset.saturating_sub(self.data.unwrap_or(0))
    }

    fn total_bytes(&self) -> u64 {
        std::fs::metadata(&self.path)
            .map(|m| m.len().saturating_sub(self.data.unwrap_or(44)))
            .unwrap_or(0)
            .max(self.read_bytes())
    }

    fn read(&mut self) -> Option<Vec<f32>> {
        if self.file.is_none() {
            self.file = File::open(&self.path).ok();
        }
        let file = self.file.as_mut()?;
        if self.data.is_none() {
            let mut head = vec![0_u8; 4096];
            file.seek(SeekFrom::Start(0)).ok()?;
            let n = read_some(file, &mut head)?;
            self.data = data_offset(&head[..n]);
            self.offset = self.data?;
        }
        file.seek(SeekFrom::Start(self.offset)).ok()?;
        let mut bytes = vec![0_u8; READ_MAX_BYTES];
        let n = read_some(file, &mut bytes)?;
        if n == 0 {
            return None;
        }
        self.offset += n as u64;
        bytes.truncate(n);
        if let Some(first) = self.carry.take() {
            bytes.insert(0, first);
        }
        if bytes.len() % 2 == 1 {
            self.carry = bytes.pop();
        }
        (!bytes.is_empty()).then(|| {
            bytes
                .chunks_exact(2)
                .map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / 32_768.0)
                .collect()
        })
    }
}

fn read_some(file: &mut File, buffer: &mut [u8]) -> Option<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match file.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
    Some(filled)
}

/// Where a WAV file's samples start: after its `data` chunk's header.
fn data_offset(head: &[u8]) -> Option<u64> {
    if head.len() < 12 || &head[0..4] != b"RIFF" || &head[8..12] != b"WAVE" {
        return None;
    }
    let mut at = 12_usize;
    while at + 8 <= head.len() {
        let id = &head[at..at + 4];
        let size = u32::from_le_bytes([head[at + 4], head[at + 5], head[at + 6], head[at + 7]]);
        if id == b"data" {
            return Some((at + 8) as u64);
        }
        at = at
            .checked_add(8)?
            .checked_add(usize::try_from(size).ok()?)?;
        at += at % 2;
    }
    None
}

/// The live transcription of a meeting being recorded, run by the daemon
/// on its own thread.
pub struct Live {
    control: Arc<AtomicU8>,
    done: mpsc::Receiver<()>,
}

impl Live {
    /// Start the loop on a thread named for it; `work` runs [`run`].
    ///
    /// # Errors
    /// The thread could not start.
    pub fn start(work: impl FnOnce(&AtomicU8) + Send + 'static) -> std::io::Result<Self> {
        let control = Arc::new(AtomicU8::new(RUN));
        let (tx, done) = mpsc::channel();
        let theirs = Arc::clone(&control);
        std::thread::Builder::new()
            .name("narrivy-meeting-live".into())
            .spawn(move || {
                work(&theirs);
                let _ = tx.send(());
            })?;
        Ok(Self { control, done })
    }

    /// Recognise what is left, waiting `bound` at most; past it the loop is
    /// told to end now.
    pub fn finish(self, bound: Duration) {
        self.control.store(FINISH, Ordering::Release);
        if self.done.recv_timeout(bound).is_err() {
            self.control.store(QUIT, Ordering::Release);
        }
    }

    /// End now, leaving what is not recognised yet; waits `bound` at most
    /// for a piece being recognised.
    pub fn quit(self, bound: Duration) {
        self.control.store(QUIT, Ordering::Release);
        let _ = self.done.recv_timeout(bound);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(seconds: f32, amplitude: f32) -> Vec<f32> {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let n = (seconds * RATE as f32) as usize;
        (0..n)
            .map(|i| {
                #[allow(clippy::cast_precision_loss)]
                let t = i as f32 / RATE as f32;
                amplitude * (t * 220.0 * std::f32::consts::TAU).sin()
            })
            .collect()
    }

    fn hiss(seconds: f32) -> Vec<f32> {
        tone(seconds, 0.001)
    }

    #[test]
    fn speech_between_pauses_becomes_one_piece_each() {
        let mut cutter = Cutter::new("room");
        let mut sound = hiss(1.0);
        sound.extend(tone(1.5, 0.2));
        sound.extend(hiss(1.0));
        sound.extend(tone(2.0, 0.2));
        sound.extend(hiss(1.0));
        cutter.feed(&sound);
        let pieces = cutter.flush();
        assert_eq!(pieces.len(), 2, "{pieces:?}");
        let first = &pieces[0];
        assert!(first.start_seconds() > 0.5 && first.start_seconds() < 1.0);
        assert!(first.end_seconds() > 2.5 && first.end_seconds() < 3.5);
        assert!(pieces[1].start_seconds() > 3.0);
    }

    #[test]
    fn silence_and_room_tone_give_no_piece() {
        let mut cutter = Cutter::new("call");
        cutter.feed(&vec![0.0; RATE * 5]);
        cutter.feed(&hiss(5.0));
        assert!(cutter.flush().is_empty());
    }

    #[test]
    fn long_speech_is_cut_into_pieces_no_longer_than_the_limit() {
        let mut cutter = Cutter::new("room");
        cutter.feed(&hiss(0.5));
        // 20 s of talk with short dips every second.
        for _ in 0..20 {
            cutter.feed(&tone(0.9, 0.2));
            cutter.feed(&tone(0.1, 0.02));
        }
        let pieces = cutter.flush();
        assert!(pieces.len() >= 3, "{}", pieces.len());
        for piece in &pieces {
            assert!(piece.end - piece.start <= PIECE_MAX);
        }
        // Nothing lost between pieces.
        for pair in pieces.windows(2) {
            assert_eq!(pair[0].end, pair[1].start);
        }
    }

    #[test]
    fn sound_fed_in_small_steps_cuts_the_same() {
        let mut sound = hiss(1.0);
        sound.extend(tone(1.5, 0.2));
        sound.extend(hiss(1.0));
        let mut whole = Cutter::new("room");
        whole.feed(&sound);
        let mut steps = Cutter::new("room");
        for chunk in sound.chunks(777) {
            steps.feed(chunk);
        }
        let (a, b) = (whole.flush(), steps.flush());
        assert_eq!(a.len(), b.len());
        assert_eq!((a[0].start, a[0].end), (b[0].start, b[0].end));
    }

    #[test]
    fn the_data_chunk_is_found_after_other_chunks() {
        let mut head = b"RIFF\0\0\0\0WAVE".to_vec();
        head.extend(b"fmt \x10\0\0\0");
        head.extend([0_u8; 16]);
        head.extend(b"LIST\x03\0\0\0abc\0");
        head.extend(b"data\0\0\0\0");
        assert_eq!(data_offset(&head), Some(head.len() as u64));
        assert_eq!(data_offset(b"RIFF"), None);
        assert_eq!(data_offset(b"RIFF\0\0\0\0WAVEfmt "), None);
    }

    #[test]
    fn the_room_copy_of_the_call_is_an_echo() {
        let turn = |stream: &str, start: f64, text: &str| Turn {
            stream: stream.into(),
            start,
            end: start + 2.0,
            speaker: String::new(),
            text: text.into(),
            language: String::new(),
        };
        let call = turn("call", 10.0, "We ship the release on Friday.");
        assert!(echoes_the_call(
            &turn("room", 10.2, "we ship the release on friday"),
            std::slice::from_ref(&call)
        ));
        assert!(!echoes_the_call(
            &turn("room", 10.2, "Sounds good to me"),
            std::slice::from_ref(&call)
        ));
        assert!(!echoes_the_call(
            &turn("room", 40.0, "we ship the release on friday"),
            std::slice::from_ref(&call)
        ));
    }

    struct Collect {
        turns: Vec<Turn>,
        partials: usize,
        trouble: Vec<Option<&'static str>>,
    }

    impl Sink for Collect {
        fn turn(&mut self, turn: &Turn) {
            self.turns.push(turn.clone());
        }
        fn partial(&mut self, turn: Option<&Turn>) {
            self.partials += usize::from(turn.is_some());
        }
        fn progress(&mut self, _: u64, _: u64) {}
        fn trouble(&mut self, code: Option<&'static str>) {
            self.trouble.push(code);
        }
    }

    fn write_wav(path: &Path, samples: &[f32]) -> std::io::Result<()> {
        let mut bytes = b"RIFF\0\0\0\0WAVEfmt \x10\0\0\0".to_vec();
        bytes.extend(1_u16.to_le_bytes()); // PCM
        bytes.extend(1_u16.to_le_bytes()); // mono
        bytes.extend(16_000_u32.to_le_bytes());
        bytes.extend(32_000_u32.to_le_bytes());
        bytes.extend(2_u16.to_le_bytes());
        bytes.extend(16_u16.to_le_bytes());
        bytes.extend(b"data\0\0\0\0");
        for s in samples {
            #[allow(clippy::cast_possible_truncation)]
            bytes.extend(((s * 32_767.0) as i16).to_le_bytes());
        }
        std::fs::write(path, bytes)
    }

    #[test]
    fn a_finished_meeting_is_read_to_its_end_with_both_streams() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        let mut room = hiss(0.5);
        room.extend(tone(2.0, 0.2));
        room.extend(hiss(1.0));
        let mut call = hiss(4.0);
        call.extend(tone(2.0, 0.2));
        write_wav(&dir.path().join("mic.wav"), &room)?;
        write_wav(&dir.path().join("system.wav"), &call)?;
        let control = AtomicU8::new(FINISH);
        let mut sink = Collect {
            turns: Vec::new(),
            partials: 0,
            trouble: Vec::new(),
        };
        let mut n = 0;
        run(
            dir.path(),
            &control,
            false,
            &mut |_| {
                n += 1;
                Heard::Text(format!("words {n}"), "en".into())
            },
            &mut sink,
        );
        let streams: Vec<_> = sink.turns.iter().map(|t| t.stream.as_str()).collect();
        assert_eq!(streams, ["room", "call"]);
        assert_eq!(sink.turns[0].speaker, "Person 1");
        assert_eq!(sink.turns[1].speaker, "Person 2");
        assert!(sink.turns[1].start > 3.5);
        Ok(())
    }

    #[test]
    fn no_model_is_reported_and_cleared_when_it_comes() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        let mut room = hiss(0.5);
        room.extend(tone(1.5, 0.2));
        room.extend(hiss(1.0));
        room.extend(tone(1.5, 0.2));
        room.extend(hiss(1.0));
        write_wav(&dir.path().join("mic.wav"), &room)?;
        let control = AtomicU8::new(FINISH);
        let mut sink = Collect {
            turns: Vec::new(),
            partials: 0,
            trouble: Vec::new(),
        };
        let mut calls = 0;
        run(
            dir.path(),
            &control,
            false,
            &mut |_| {
                calls += 1;
                if calls == 1 {
                    Heard::Unavailable("meeting_model_missing")
                } else {
                    Heard::Text("hello there everyone".into(), String::new())
                }
            },
            &mut sink,
        );
        assert_eq!(sink.trouble, [Some("meeting_model_missing"), None]);
        assert_eq!(sink.turns.len(), 1);
        Ok(())
    }

    #[test]
    fn a_growing_file_gives_turns_while_recording_and_finishes() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("mic.wav");
        let mut first = hiss(0.5);
        first.extend(tone(1.5, 0.2));
        first.extend(hiss(1.0));
        write_wav(&path, &first)?;
        let control = Arc::new(AtomicU8::new(RUN));
        let (tx, rx) = mpsc::channel();
        let theirs = Arc::clone(&control);
        let dir_path = dir.path().to_owned();
        let worker = std::thread::spawn(move || {
            struct Send(mpsc::Sender<Turn>);
            impl Sink for Send {
                fn turn(&mut self, turn: &Turn) {
                    let _ = self.0.send(turn.clone());
                }
                fn partial(&mut self, _: Option<&Turn>) {}
                fn progress(&mut self, _: u64, _: u64) {}
                fn trouble(&mut self, _: Option<&'static str>) {}
            }
            run(
                &dir_path,
                &theirs,
                true,
                &mut |_| Heard::Text("one two three".into(), String::new()),
                &mut Send(tx),
            );
        });
        let turn = rx
            .recv_timeout(Duration::from_secs(10))
            .map_err(std::io::Error::other)?;
        assert_eq!(turn.stream, "room");
        // More sound arrives, then the meeting stops: its last words count.
        let mut more = Vec::new();
        for s in tone(1.0, 0.2) {
            #[allow(clippy::cast_possible_truncation)]
            more.extend(((s * 32_767.0) as i16).to_le_bytes());
        }
        let mut file = std::fs::OpenOptions::new().append(true).open(&path)?;
        std::io::Write::write_all(&mut file, &more)?;
        control.store(FINISH, Ordering::Release);
        worker
            .join()
            .map_err(|_| std::io::Error::other("the transcription thread panicked"))?;
        assert!(rx.try_recv().is_ok(), "the last piece was not recognised");
        Ok(())
    }
}
