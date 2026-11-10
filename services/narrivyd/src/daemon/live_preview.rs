//! Live text for whole-recording models (F3): while the user speaks, the
//! recent audio is decoded every [`CADENCE`] and published as volatile
//! partials, so the overlay shows text for models that cannot stream (such
//! as Whisper on the NPU). Partials never count as the final text.
//!
//! Pieces: given a pieces engine, every finished stretch of
//! [`PIECE_MIN`] to [`PIECE_MAX`] of audio is cut at its quietest moment and
//! recognised with the final engine and request while recording goes on. The
//! recording's final recognition then covers only the audio after the last
//! recognised piece ([`LivePreviewPort::committed`]), so the wait after Stop
//! stays about one piece long however long the dictation was. Pieces are
//! recognised in order; after a failed piece, no later one counts.

#![cfg(feature = "asr-transcribe-resident")]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use narrivy_asr_api::{AsrRequest, OneShotAsrPort};
use narrivy_core::{CommittedPrefix, LivePreviewPort, VolatilePartial};
use narrivy_types::{AudioChunk, SessionCancellation, SessionId};

use super::debug_audio::{self, DebugLog};

/// How often the preview decodes. A decode takes about half a second on
/// the NPU, so the next window is ready as each decode ends and the words
/// trail the voice by about that much; a newer window replaces one still
/// waiting.
pub const CADENCE: Duration = Duration::from_millis(500);
/// The preview decodes at most this much recent audio (Whisper's window).
const WINDOW_SAMPLES: usize = 16_000 * 28;
/// Below this loudness (RMS) the recent audio is silence: a model would
/// only invent words for it.
const SILENCE_RMS: f32 = 0.004;
/// Less new audio than this since the last decode is not worth a decode.
const MIN_NEW_SAMPLES: usize = 16_000 * 3 / 10;
/// A piece is at least this long...
const PIECE_MIN: usize = 16_000 * 17;
/// ...and at most this long, so it stays inside one Whisper window.
const PIECE_MAX: usize = 16_000 * 28;
/// Loudness is measured over frames of this many samples (100 ms).
const CUT_FRAME: usize = 1_600;
/// A piece is only cut in a pause at least this long (1.1 s)...
const PAUSE_FRAMES: usize = 11;
/// ...and keeps this much of it (0.9 s). Whisper changes the last word of
/// audio that stops right after it (a cut 50 ms after "prediction" gave
/// "verdict"); with 0.9 s of quiet after it, no word changed in 15 tests.
const KEEP_FRAMES: usize = 9;

/// Where the next piece ends, if anywhere.
#[derive(Debug, PartialEq, Eq)]
enum Cut {
    /// Keep listening: a pause may still come.
    Wait,
    /// The piece ends at this sample.
    At(usize),
    /// No pause came in time; the rest is recognised at Stop.
    GiveUp,
}

/// The pieces recognised so far: the samples they cover and their text.
#[derive(Default)]
struct Committed {
    samples: usize,
    text: String,
    failed: bool,
}

struct Shared {
    /// The latest window handed over by the capture loop, and how many
    /// samples had been captured when it was taken.
    pending: Mutex<Option<(Vec<f32>, usize)>>,
    /// Pieces waiting for recognition: the piece and the sample it ends at.
    pieces: Mutex<VecDeque<(Vec<f32>, usize)>>,
    committed: Mutex<Committed>,
    /// Only with the debug switch on ([`super::debug_audio`]).
    log: Option<Mutex<DebugLog>>,
    wake: Condvar,
    stop: AtomicBool,
}

impl Shared {
    fn committed(&self) -> std::sync::MutexGuard<'_, Committed> {
        self.committed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Where the piece that starts at `start` ends: [`KEEP_FRAMES`] into the
/// first pause of at least [`PAUSE_FRAMES`], so the piece is between
/// [`PIECE_MIN`] and [`PIECE_MAX`] long. A cut in a shorter dip can split a
/// word, and the last word of a piece needs quiet after it.
fn piece_cut(samples: &[f32], start: usize) -> Cut {
    let quiet = |frame: usize| {
        let at = start + frame * CUT_FRAME;
        let frame = &samples[at..at + CUT_FRAME];
        let power = frame.iter().map(|s| s * s).sum::<f32>() / CUT_FRAME as f32;
        power.sqrt() < SILENCE_RMS
    };
    let first = (PIECE_MIN / CUT_FRAME).saturating_sub(KEEP_FRAMES);
    let last = PIECE_MAX / CUT_FRAME - KEEP_FRAMES;
    let heard = samples.len().saturating_sub(start) / CUT_FRAME;
    for frame in first..=last {
        if frame + PAUSE_FRAMES > heard {
            return Cut::Wait;
        }
        if (frame..frame + PAUSE_FRAMES).all(quiet) {
            return Cut::At(start + (frame + KEEP_FRAMES) * CUT_FRAME);
        }
    }
    Cut::GiveUp
}

/// Two consecutive pieces of text, separated by one space.
fn joined(first: &str, second: &str) -> String {
    match (first.trim(), second.trim()) {
        (first, "") => first.to_owned(),
        ("", second) => second.to_owned(),
        (first, second) => format!("{first} {second}"),
    }
}

/// The daemon's [`LivePreviewPort`]: a thread that decodes the latest window
/// with the session's engine and publishes what it hears.
pub struct LivePreview {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    last_handoff: Option<Instant>,
    handed: usize,
    /// Where the next piece starts; `None` without a pieces engine.
    next_piece: Option<usize>,
    /// Receives the microphone level (0..=1000) about every [`LEVEL_EVERY`].
    on_level: Box<dyn FnMut(u16) + Send>,
    last_level: Option<Instant>,
    /// The whole recording, only with the debug switch on.
    kept: Option<Vec<f32>>,
}

/// How often the microphone level is reported.
const LEVEL_EVERY: Duration = Duration::from_millis(50);
/// The level is measured over this many recent samples (50 ms).
const LEVEL_SAMPLES: usize = 800;

/// A perceptual loudness for the overlay: -60 dBFS and below is 0, -12 dBFS
/// and above is 1000.
#[must_use]
pub fn level_of(samples: &[f32]) -> u16 {
    if samples.is_empty() {
        return 0;
    }
    let power = samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32;
    let db = 10.0 * power.max(1e-12).log10();
    let scaled = ((db + 60.0) / 48.0).clamp(0.0, 1.0);
    (scaled * 1000.0).round() as u16
}

impl LivePreview {
    /// Start previewing with `engine`, publishing each preview through
    /// `publish`. With `pieces`, finished pieces are recognised with it
    /// (the final engine and its filters) while recording.
    pub fn start(
        mut engine: Box<dyn OneShotAsrPort>,
        mut pieces: Option<Box<dyn OneShotAsrPort>>,
        session: SessionId,
        request: AsrRequest,
        cancellation: SessionCancellation,
        mut publish: impl FnMut(VolatilePartial) + Send + 'static,
        on_level: impl FnMut(u16) + Send + 'static,
    ) -> Option<Self> {
        let shared = Arc::new(Shared {
            pending: Mutex::new(None),
            pieces: Mutex::new(VecDeque::new()),
            committed: Mutex::new(Committed::default()),
            log: debug_audio::enabled().then(|| Mutex::new(DebugLog::default())),
            wake: Condvar::new(),
            stop: AtomicBool::new(false),
        });
        let next_piece = pieces.is_some().then_some(0);
        let worker = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("narrivy-live-preview".into())
            .spawn(move || {
                let mut seq = 0_u64;
                let mut last_text = String::new();
                loop {
                    let window = {
                        let mut pending = worker
                            .pending
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        let piece_waiting = || {
                            !worker
                                .pieces
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .is_empty()
                        };
                        while pending.is_none()
                            && !piece_waiting()
                            && !worker.stop.load(Ordering::Acquire)
                        {
                            pending = worker
                                .wake
                                .wait(pending)
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                        }
                        if worker.stop.load(Ordering::Acquire) {
                            return;
                        }
                        let piece = worker
                            .pieces
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .pop_front();
                        // A piece goes first; the window waits for the next turn.
                        match piece {
                            Some(piece) => {
                                drop(pending);
                                if let Some(engine) = pieces.as_mut() {
                                    recognise_piece(
                                        &worker,
                                        &mut **engine,
                                        session,
                                        &cancellation,
                                        &request,
                                        piece,
                                    );
                                }
                                continue;
                            }
                            None => pending.take(),
                        }
                    };
                    let Some((samples, captured)) = window else {
                        continue;
                    };
                    let audio = AudioChunk {
                        sample_rate_hz: 16_000,
                        channels: 1,
                        samples,
                    };
                    let Ok(transcript) =
                        engine.recognize_with(session, &audio, &cancellation, &request)
                    else {
                        continue;
                    };
                    let preview = transcript.text.trim().to_owned();
                    if worker.stop.load(Ordering::Acquire) {
                        return;
                    }
                    let mut text = joined(&worker.committed().text, &preview);
                    if text.len() > 16_384 {
                        text = preview;
                    }
                    if text.is_empty() || text == last_text || text.len() > 16_384 {
                        continue;
                    }
                    if let Some(log) = &worker.log
                        && let Ok(mut log) = log.lock()
                    {
                        log.partials.push((captured, text.clone()));
                    }
                    seq += 1;
                    last_text.clone_from(&text);
                    publish(VolatilePartial { seq, text });
                }
            })
            .ok()?;
        Some(Self {
            shared,
            thread: Some(thread),
            last_handoff: None,
            handed: 0,
            next_piece,
            on_level: Box::new(on_level),
            last_level: None,
            kept: debug_audio::enabled().then(Vec::new),
        })
    }
}

impl LivePreviewPort for LivePreview {
    fn audio(&mut self, samples: &[f32]) {
        if let Some(kept) = self.kept.as_mut()
            && samples.len() > kept.len()
        {
            kept.extend_from_slice(&samples[kept.len()..]);
        }
        if self.last_level.is_none_or(|at| at.elapsed() >= LEVEL_EVERY) {
            self.last_level = Some(Instant::now());
            (self.on_level)(level_of(
                &samples[samples.len().saturating_sub(LEVEL_SAMPLES)..],
            ));
        }
        let due = self.last_handoff.is_none_or(|at| at.elapsed() >= CADENCE);
        if !due {
            return;
        }
        if let Some(start) = self.next_piece {
            match piece_cut(samples, start) {
                Cut::Wait => {}
                Cut::GiveUp => self.next_piece = None,
                Cut::At(cut) => {
                    self.next_piece = Some(cut);
                    self.shared
                        .pieces
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .push_back((samples[start..cut].to_vec(), cut));
                    self.shared.wake.notify_one();
                }
            }
        }
        if samples.len() < self.handed + MIN_NEW_SAMPLES {
            return;
        }
        // The preview covers what no recognised piece has.
        let from = self
            .shared
            .committed()
            .samples
            .max(samples.len().saturating_sub(WINDOW_SAMPLES))
            .min(samples.len());
        let window = &samples[from..];
        // Right after a recognised piece, wait for a second of new audio.
        if window.len() < 16_000 {
            return;
        }
        // The last second decides: a pause is not worth a decode.
        let tail = &window[window.len().saturating_sub(16_000)..];
        let energy = tail.iter().map(|s| s * s).sum::<f32>() / tail.len().max(1) as f32;
        self.last_handoff = Some(Instant::now());
        if energy.sqrt() < SILENCE_RMS {
            return;
        }
        self.handed = samples.len();
        let mut pending = self
            .shared
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // A decode still running means the newest window replaces any
        // waiting one.
        *pending = Some((window.to_vec(), samples.len()));
        drop(pending);
        self.shared.wake.notify_one();
    }

    fn finish(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Ok(mut pending) = self.shared.pending.lock() {
            pending.take();
        }
        self.shared.wake.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        if let (Some(kept), Some(log)) = (self.kept.take(), &self.shared.log) {
            let log = log.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            debug_audio::save(&kept, &log);
        }
    }

    fn committed(&mut self) -> Option<CommittedPrefix> {
        let committed = self.shared.committed();
        (committed.samples > 0).then(|| CommittedPrefix {
            samples: committed.samples,
            text: committed.text.clone(),
        })
    }
}

/// Recognise one piece and, when it follows the pieces before it, count it.
fn recognise_piece(
    shared: &Shared,
    engine: &mut dyn OneShotAsrPort,
    session: SessionId,
    cancellation: &SessionCancellation,
    request: &AsrRequest,
    (samples, end): (Vec<f32>, usize),
) {
    if shared.committed().failed {
        return;
    }
    let start = end - samples.len();
    let power = samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32;
    // A silent piece says nothing; a model would only invent words for it.
    let outcome = if power.sqrt() < SILENCE_RMS {
        Ok(narrivy_types::Transcript {
            text: String::new(),
            is_final: true,
        })
    } else {
        let audio = AudioChunk {
            sample_rate_hz: 16_000,
            channels: 1,
            samples,
        };
        engine.recognize_with(session, &audio, cancellation, request)
    };
    let mut committed = shared.committed();
    match outcome {
        Ok(transcript) if committed.samples == start => {
            if let Some(log) = &shared.log
                && let Ok(mut log) = log.lock()
            {
                log.pieces.push((start, end, transcript.text.clone()));
            }
            committed.text = joined(&committed.text, &transcript.text);
            committed.samples = end;
        }
        _ => committed.failed = true,
    }
}

impl Drop for LivePreview {
    fn drop(&mut self) {
        self.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use narrivy_asr_api::AsrError;
    use narrivy_types::Transcript;

    const SECOND: usize = 16_000;

    /// Speech-loud audio of `seconds`, with silence over `pause` (seconds).
    fn speech(seconds: usize, pause: Option<(f32, f32)>) -> Vec<f32> {
        (0..seconds * SECOND)
            .map(|i| {
                let t = i as f32 / SECOND as f32;
                match pause {
                    Some((from, to)) if t >= from && t < to => 0.0,
                    _ => 0.1,
                }
            })
            .collect()
    }

    fn seconds(cut: &Cut) -> Option<f32> {
        match cut {
            Cut::At(at) => Some(*at as f32 / SECOND as f32),
            _ => None,
        }
    }

    #[test]
    fn a_piece_keeps_most_of_its_pause() {
        let cut = piece_cut(&speech(22, Some((18.0, 19.5))), 0);
        let at = seconds(&cut);
        assert!(at.is_some_and(|t| (18.85..18.95).contains(&t)), "{cut:?}");
    }

    #[test]
    fn a_short_dip_is_not_a_pause() {
        // The 18.05 s cut that split "prediction" fell in a dip like this.
        assert_eq!(piece_cut(&speech(22, Some((18.0, 18.4))), 0), Cut::Wait);
        assert_eq!(piece_cut(&speech(22, Some((18.0, 18.9))), 0), Cut::Wait);
    }

    #[test]
    fn a_pause_must_be_heard_whole_before_the_cut() {
        // The pause began at 18 s but only 0.6 s of it is in.
        assert_eq!(
            piece_cut(
                &speech(19, None)
                    .into_iter()
                    .chain(vec![0.0; SECOND * 6 / 10])
                    .collect::<Vec<_>>(),
                0
            ),
            Cut::Wait
        );
    }

    #[test]
    fn without_a_pause_in_time_pieces_stop() {
        assert_eq!(piece_cut(&speech(20, None), 0), Cut::Wait, "still growing");
        assert_eq!(
            piece_cut(&speech(16, Some((5.0, 7.0))), 0),
            Cut::Wait,
            "too early to cut"
        );
        assert_eq!(piece_cut(&speech(31, None), 0), Cut::GiveUp);
    }

    #[test]
    fn a_pause_just_before_the_shortest_piece_still_counts() {
        // Quiet from 16.5 s: the cut lands 0.9 s in, past the 17 s minimum.
        let at = seconds(&piece_cut(&speech(20, Some((16.5, 18.0))), 0));
        assert!(at.is_some_and(|t| (17.35..17.45).contains(&t)), "{at:?}");
    }

    #[test]
    fn a_later_piece_starts_where_the_last_ended() {
        let samples = speech(45, Some((39.0, 40.5)));
        let at = seconds(&piece_cut(&samples, 20 * SECOND));
        assert!(at.is_some_and(|t| (39.85..39.95).contains(&t)), "{at:?}");
    }

    struct Fixed(&'static str, Arc<std::sync::atomic::AtomicUsize>);
    impl OneShotAsrPort for Fixed {
        fn recognize(
            &mut self,
            _: SessionId,
            _: &AudioChunk,
            _: &SessionCancellation,
        ) -> Result<Transcript, AsrError> {
            self.1.fetch_add(1, Ordering::SeqCst);
            Ok(Transcript {
                text: self.0.to_owned(),
                is_final: true,
            })
        }
    }

    #[test]
    fn a_finished_piece_is_recognised_and_committed() -> Result<(), &'static str> {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut preview = LivePreview::start(
            Box::new(Fixed("preview", Arc::new(Default::default()))),
            Some(Box::new(Fixed("first piece", Arc::clone(&calls)))),
            SessionId(1),
            AsrRequest::default(),
            narrivy_types::SessionControl::new().cancellation().clone(),
            |_| {},
            |_| {},
        )
        .ok_or("the preview thread starts")?;
        let samples = speech(25, Some((20.0, 21.5)));
        preview.audio(&samples);
        let deadline = Instant::now() + Duration::from_secs(5);
        while preview.shared.committed().samples == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        preview.finish();
        let committed = preview.committed();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let committed = committed.ok_or("the piece was committed")?;
        assert_eq!(committed.text, "first piece");
        let at = committed.samples as f32 / SECOND as f32;
        assert!((20.85..20.95).contains(&at), "{at}");
        Ok(())
    }

    #[test]
    fn without_a_pieces_engine_nothing_is_committed() -> Result<(), &'static str> {
        let mut preview = LivePreview::start(
            Box::new(Fixed("preview", Arc::new(Default::default()))),
            None,
            SessionId(1),
            AsrRequest::default(),
            narrivy_types::SessionControl::new().cancellation().clone(),
            |_| {},
            |_| {},
        )
        .ok_or("the preview thread starts")?;
        preview.audio(&speech(25, Some((20.0, 20.5))));
        preview.finish();
        assert_eq!(preview.committed(), None);
        Ok(())
    }

    #[test]
    fn the_level_follows_loudness() {
        assert_eq!(level_of(&[]), 0);
        assert_eq!(level_of(&[0.0; 800]), 0);
        let quiet = level_of(&[0.003; 800]);
        let speech = level_of(&[0.05; 800]);
        let loud = level_of(&[0.5; 800]);
        assert!(quiet < speech && speech < loud, "{quiet} {speech} {loud}");
        assert_eq!(loud, 1000);
    }
}
