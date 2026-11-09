//! "Ignore this computer's sound" on Windows: WebRTC audio processing inside
//! Narrivy, as Linux gets it from PipeWire's echo-cancel module.
//!
//! [`VoiceProcessor`] takes the microphone and what the speakers play (a
//! WASAPI loopback, see `os::wasapi`), both already in the capture contract
//! (mono f32, 16 kHz), and runs them through `sonora`, a pure-Rust port of
//! WebRTC's audio processing: the echo canceller (AEC3), noise suppression
//! at the user's level, and, when asked, automatic gain (AGC2). It works in
//! WebRTC's 10 ms frames (160 samples): microphone audio is held until a
//! frame is whole, and each microphone frame is paired with one speaker
//! frame, or silence when the speakers played nothing.
//!
//! WebRTC's canceller leaves a faint residue of the computer's sound, and a
//! speech model turns even that into a few made-up words. [`EchoGate`]
//! silences the frames the canceller found to be only the computer's sound
//! (most of the frame removed while the speakers play) and opens at once,
//! with a short hold, for a frame where much is left: the user talking.
//!
//! [`ProcessedSource`] puts it between two capture sources (the microphone
//! and the speakers' loopback) and is itself a [`CaptureSource`], so the
//! ordinary capture sessions carry the processed audio. It needs no Win32:
//! `os::processed_capture` opens the two WASAPI streams.
//!
//! Portable: no Win32 here, so it is unit-tested on Linux too.

#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use narrivy_platform_api::PlatformError;

use sonora::config::{
    AdaptiveDigital, EchoCanceller, GainController2, HighPassFilter, NoiseSuppression,
    NoiseSuppressionLevel,
};
use sonora::{AudioProcessing, Config, StreamConfig};

use crate::capture::{CAPTURE_CHANNELS, CAPTURE_RATE_HZ, CaptureSource, SourceRead};

/// One WebRTC frame: 10 ms at the contract rate.
pub const FRAME_SAMPLES: usize = (CAPTURE_RATE_HZ / 100) as usize;

/// Speaker audio held for pairing, at most 200 ms. The loopback stream and
/// the microphone run on different clocks; when the speakers run ahead, the
/// oldest audio is dropped so the delay the canceller sees stays bounded.
pub const MAX_RENDER_BACKLOG: usize = FRAME_SAMPLES * 20;

/// Speakers count as playing above this frame energy (about -60 dBFS).
const PLAYING_ENERGY: f32 = 1e-6;
/// The microphone holds the user's voice when it is this much louder than
/// the echo expected from the speakers (6 dB).
const VOICE_MARGIN: f32 = 4.0;
/// The user's voice is also this much above the room's noise floor (10 dB),
/// so the room's own noise and ringing between the speakers' words are not
/// taken for it.
const VOICE_OVER_NOISE: f32 = 10.0;
/// How fast the noise floor follows a louder room, per frame (about
/// 1 dB per second).
const NOISE_FLOOR_RISE: f32 = 1.0023;
/// How loud the speakers are at the microphone compared with what they
/// play, before the gate has heard the room: as loud (0 dB).
const INITIAL_COUPLING: f32 = 1.0;
/// The learned coupling falls back this much per frame when the room gets
/// quieter (about 3 dB per second).
const COUPLING_DECAY: f32 = 0.993;
/// Speaker frames remembered for the echo prediction (the canceller's delay
/// is at most 500 ms, plus the room's reverberation).
const HISTORY_FRAMES: usize = 72;
/// The room keeps sounding after the speakers: an echo can come from up to
/// this many frames before the delay (150 ms).
const REVERB_FRAMES: usize = 15;
/// Frames of slack after the delay, for its estimate's jitter (30 ms).
const JITTER_FRAMES: usize = 3;
/// After the speakers stop, the room still rings: for this long (500 ms)
/// a frame passes only as a voice.
const TAIL_FRAMES: u32 = 50;
/// Frames the gate stays open after a frame with the user's voice (200 ms),
/// so the quiet ends of words are kept.
const GATE_HOLD_FRAMES: u32 = 20;
/// After the canceller, the user's voice is this much louder than what is
/// left of the speakers' echo.
const RESIDUAL_MARGIN: f32 = 4.0;
/// How loud the canceller's leftover echo is compared with what the
/// speakers play, before the gate has heard the room.
const INITIAL_RESIDUAL_COUPLING: f32 = 0.1;
/// A learned coupling rises at most this much per frame (about 2 dB per
/// 100 ms), so the first frames of the user's voice that the canceller
/// dimmed are not learned as echo at once.
const COUPLING_RISE: f32 = 1.05;
/// The canceller is still learning the room for its first 3 s of
/// playback: until then a coupling follows any louder echo at once.
const WARMUP_FRAMES: u32 = 300;
/// The learned leftover falls back this much per frame when the canceller
/// does better (about 9 dB per second).
const RESIDUAL_DECAY: f32 = 0.98;

/// Silences what is left of the computer's sound after the canceller.
///
/// From the canceller's delay and how loud the speakers have been coming
/// back (learned from frames that are only their sound) it predicts how
/// loud the computer's sound is at the microphone now, and how loud the
/// canceller's leftover of it is. While the speakers play, and for a short
/// tail after, a frame passes only if the microphone is clearly louder than
/// the predicted echo, or the canceller's output clearly louder than the
/// predicted leftover (the user talking under a louder echo), and above the
/// room's noise; and for a short hold after. The learned levels rise only
/// slowly after the first 3 s of playback, so the start of the user's voice,
/// which the canceller dims for a moment, is not learned as echo.
#[derive(Debug, Clone)]
pub struct EchoGate {
    gain: f32,
    hold: u32,
    /// Speaker frame energies, newest last.
    history: VecDeque<f32>,
    /// The quietest the microphone has been while nothing played: the
    /// room's noise.
    noise_floor: Option<f32>,
    /// How loud the speakers come back at the microphone (energy ratio),
    /// learned from frames that are only their sound.
    coupling: f32,
    /// How loud the canceller's leftover echo is (energy ratio to the
    /// speakers), learned like `coupling`.
    residual_coupling: f32,
    /// The canceller output's quietest while nothing played.
    residual_floor: Option<f32>,
    /// Frames the couplings have been learned from.
    learned_frames: u32,
    /// Frames since the speakers' sound last reached the microphone.
    since_playing: u32,
    /// Content-free counts for diagnostics: frames seen, frames while the
    /// speakers played (or rang), frames silenced.
    frames: u64,
    guarded_frames: u64,
    closed_frames: u64,
}

impl Default for EchoGate {
    fn default() -> Self {
        Self {
            gain: 1.0,
            hold: 0,
            history: VecDeque::with_capacity(HISTORY_FRAMES),
            noise_floor: None,
            coupling: INITIAL_COUPLING,
            residual_coupling: INITIAL_RESIDUAL_COUPLING,
            residual_floor: None,
            learned_frames: 0,
            since_playing: TAIL_FRAMES,
            frames: 0,
            guarded_frames: 0,
            closed_frames: 0,
        }
    }
}

impl EchoGate {
    /// Gain for the next frame: `microphone` and `speakers` are the frame
    /// energies and `delay_frames` the canceller's delay estimate.
    pub fn next_gain(&mut self, microphone: f32, speakers: f32, delay_frames: usize) -> f32 {
        self.next_gain_after(microphone, None, speakers, delay_frames)
    }

    /// As [`Self::next_gain`], also given the canceller's output energy for
    /// the frame (`residual`): the user's voice stands out there even when
    /// the speakers' echo is louder at the microphone.
    pub fn next_gain_after(
        &mut self,
        microphone: f32,
        residual: Option<f32>,
        speakers: f32,
        delay_frames: usize,
    ) -> f32 {
        if self.history.len() == HISTORY_FRAMES {
            self.history.pop_front();
        }
        self.history.push_back(speakers);
        let newest = self.history.len() - 1;
        let early = newest.saturating_sub(delay_frames + REVERB_FRAMES);
        let loudest_in = |from: usize, to: usize| {
            self.history
                .range(from..=to)
                .fold(0.0_f32, |loudest, energy| loudest.max(*energy))
        };
        // The echo now comes from what played one delay ago (and the room's
        // ringing before that).
        let loudest = loudest_in(
            early,
            newest.saturating_sub(delay_frames.saturating_sub(JITTER_FRAMES)),
        );
        // Playing counts from the speakers' first frame, not only after the
        // delay: an estimate that is late must not let the first word in.
        let playing = loudest_in(early, newest) > PLAYING_ENERGY;
        self.since_playing = if playing {
            0
        } else {
            self.since_playing.saturating_add(1)
        };
        let guarded = self.since_playing < TAIL_FRAMES;
        if !guarded {
            self.noise_floor = Some(self.noise_floor.map_or(microphone, |floor| {
                (floor * NOISE_FLOOR_RISE).min(microphone)
            }));
            if let Some(residual) = residual {
                self.residual_floor = Some(
                    self.residual_floor
                        .map_or(residual, |floor| (floor * NOISE_FLOOR_RISE).min(residual)),
                );
            }
        }
        let echo = loudest * self.coupling;
        let above_noise = self
            .noise_floor
            .is_none_or(|floor| microphone > floor * VOICE_OVER_NOISE);
        let heard = microphone > echo * VOICE_MARGIN && above_noise;
        let left = residual.is_some_and(|residual| {
            residual > loudest * self.residual_coupling * RESIDUAL_MARGIN
                && self
                    .residual_floor
                    .is_none_or(|floor| residual > floor * VOICE_OVER_NOISE)
                && above_noise
        });
        let voice = heard || left;
        if playing && !voice && loudest > PLAYING_ENERGY * 100.0 {
            // Only the speakers' sound: how loud it comes back, before and
            // after the canceller.
            let rise = if self.learned_frames < WARMUP_FRAMES {
                f32::MAX
            } else {
                COUPLING_RISE
            };
            self.learned_frames = self.learned_frames.saturating_add(1);
            self.coupling = (microphone / loudest)
                .max(self.coupling * COUPLING_DECAY)
                .min(self.coupling * rise);
            if let Some(residual) = residual {
                self.residual_coupling = (residual / loudest)
                    .max(self.residual_coupling * RESIDUAL_DECAY)
                    .min(self.residual_coupling * rise);
            }
        }
        self.frames += 1;
        self.guarded_frames += u64::from(guarded);
        if !guarded {
            self.hold = 0;
            self.gain = 1.0;
        } else if voice {
            self.hold = GATE_HOLD_FRAMES;
            self.gain = 1.0;
        } else if self.hold > 0 {
            self.hold -= 1;
        } else {
            self.gain = 0.0;
        }
        self.closed_frames += u64::from(self.gain == 0.0);
        self.gain
    }

    /// Frames seen, frames while the speakers played, frames silenced.
    #[must_use]
    pub fn counts(&self) -> (u64, u64, u64) {
        (self.frames, self.guarded_frames, self.closed_frames)
    }

    /// Apply the step from the last gain to `gain` across `frame`, as a
    /// ramp, so the gate makes no click.
    pub fn apply(from: f32, to: f32, frame: &mut [f32]) {
        let steps = frame.len().max(1) as f32;
        for (i, sample) in frame.iter_mut().enumerate() {
            *sample *= from + (to - from) * (i as f32 + 1.0) / steps;
        }
    }
}

/// How much background noise is removed, as the user names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoiseLevel {
    Off,
    /// WebRTC's default level.
    Standard,
    Strong,
}

/// What the processor does besides cancelling the computer's sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Processing {
    pub noise: NoiseLevel,
    /// WebRTC's automatic gain, which evens out quiet and loud speech.
    pub auto_gain: bool,
}

fn config(processing: Processing) -> Config {
    let noise_suppression = match processing.noise {
        NoiseLevel::Off => None,
        NoiseLevel::Standard => Some(NoiseSuppression::default()),
        NoiseLevel::Strong => Some(NoiseSuppression {
            level: NoiseSuppressionLevel::VeryHigh,
            ..NoiseSuppression::default()
        }),
    };
    let gain_controller2 = processing.auto_gain.then(|| GainController2 {
        adaptive_digital: Some(AdaptiveDigital::default()),
        ..GainController2::default()
    });
    Config {
        high_pass_filter: Some(HighPassFilter::default()),
        echo_canceller: Some(EchoCanceller::default()),
        noise_suppression,
        gain_controller2,
        ..Config::default()
    }
}

/// Echo cancellation, noise suppression and gain over contract audio.
pub struct VoiceProcessor {
    apm: AudioProcessing,
    /// Microphone samples short of a whole frame.
    pending: Vec<f32>,
    /// Speaker samples not yet paired with a microphone frame.
    render: VecDeque<f32>,
    render_frame: Vec<f32>,
    render_out: Vec<f32>,
    capture_out: Vec<f32>,
    gate: EchoGate,
}

impl VoiceProcessor {
    #[must_use]
    pub fn new(processing: Processing) -> Self {
        let stream = StreamConfig::new(CAPTURE_RATE_HZ, CAPTURE_CHANNELS);
        let apm = AudioProcessing::builder()
            .config(config(processing))
            .capture_config(stream)
            .render_config(stream)
            .build();
        Self {
            apm,
            pending: Vec::with_capacity(FRAME_SAMPLES),
            render: VecDeque::with_capacity(MAX_RENDER_BACKLOG),
            render_frame: vec![0.0; FRAME_SAMPLES],
            render_out: vec![0.0; FRAME_SAMPLES],
            capture_out: vec![0.0; FRAME_SAMPLES],
            gate: EchoGate::default(),
        }
    }

    /// Speaker audio (contract format) that may come back through the
    /// microphone.
    pub fn push_render(&mut self, samples: &[f32]) {
        self.render.extend(samples.iter().copied());
        let excess = self.render.len().saturating_sub(MAX_RENDER_BACKLOG);
        self.render.drain(..excess);
    }

    /// Microphone audio in; every whole frame processed is appended to
    /// `out`. A partial frame waits for the next call (or [`Self::flush`]).
    pub fn process(&mut self, microphone: &[f32], out: &mut Vec<f32>) {
        let mut rest = microphone;
        while !rest.is_empty() {
            let wanted = FRAME_SAMPLES - self.pending.len();
            let take = wanted.min(rest.len());
            self.pending.extend_from_slice(&rest[..take]);
            rest = &rest[take..];
            if self.pending.len() == FRAME_SAMPLES {
                self.process_frame(out, FRAME_SAMPLES);
            }
        }
    }

    /// The stream is over: the partial frame is padded with silence,
    /// processed, and only its real samples are kept.
    pub fn flush(&mut self, out: &mut Vec<f32>) {
        let real = self.pending.len();
        if real == 0 {
            return;
        }
        self.pending.resize(FRAME_SAMPLES, 0.0);
        self.process_frame(out, real);
    }

    /// Speaker audio waiting to be paired, in samples.
    #[must_use]
    pub fn render_backlog(&self) -> usize {
        self.render.len()
    }

    fn process_frame(&mut self, out: &mut Vec<f32>, keep: usize) {
        // One speaker frame per microphone frame: the next one when the
        // loopback delivered it, silence when nothing played.
        if self.render.len() >= FRAME_SAMPLES {
            for (slot, sample) in self
                .render_frame
                .iter_mut()
                .zip(self.render.drain(..FRAME_SAMPLES))
            {
                *slot = sample;
            }
        } else {
            self.render_frame.fill(0.0);
        }
        // Errors are only for stream formats, fixed above; were one to
        // happen, the microphone audio goes on unprocessed rather than lost.
        let _ = self
            .apm
            .process_render_f32(&[&self.render_frame], &mut [&mut self.render_out]);
        match self
            .apm
            .process_capture_f32(&[&self.pending], &mut [&mut self.capture_out])
        {
            Ok(()) => {
                let before = self.gate.gain;
                let stats = self.apm.statistics();
                let delay_frames = stats
                    .delay_ms
                    .map_or(0, |ms| usize::try_from(ms / 10).unwrap_or(0));
                let after = self.gate.next_gain_after(
                    energy(&self.pending),
                    Some(energy(&self.capture_out)),
                    energy(&self.render_frame),
                    delay_frames,
                );
                EchoGate::apply(before, after, &mut self.capture_out);
                out.extend_from_slice(&self.capture_out[..keep]);
            }
            Err(_) => out.extend_from_slice(&self.pending[..keep]),
        }
        self.pending.clear();
    }
}

/// Mean square of a frame.
fn energy(samples: &[f32]) -> f32 {
    samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32
}

/// Opens the speakers' loopback again after it failed or ended.
pub type SpeakerOpener =
    Box<dyn FnMut() -> Result<Box<dyn CaptureSource>, PlatformError> + 'static>;

/// How long after a speaker failure the loopback is opened again.
pub const SPEAKER_RETRY: Duration = Duration::from_secs(1);

/// The microphone through a [`VoiceProcessor`], with what the speakers
/// play as its echo reference. Field order is drop order: both streams stop
/// before the processor goes.
pub struct ProcessedSource {
    microphone: Box<dyn CaptureSource>,
    /// `None` when the speakers could not be opened or went away: noise
    /// suppression and gain go on without the reference.
    speakers: Option<Box<dyn CaptureSource>>,
    processor: VoiceProcessor,
    raw: Vec<f32>,
    played: Vec<f32>,
    /// Content-free count of speaker samples received, for diagnostics.
    speaker_samples: u64,
    /// Reopens the speakers after a failure (a glitch or a device change
    /// must not end echo cancellation for the rest of the dictation).
    reopen: Option<SpeakerOpener>,
    retry_at: Option<Instant>,
    /// Why the speakers were last lost (a stable code), for diagnostics.
    speaker_failure: Option<&'static str>,
}

impl ProcessedSource {
    #[must_use]
    pub fn new(
        microphone: Box<dyn CaptureSource>,
        speakers: Option<Box<dyn CaptureSource>>,
        processing: Processing,
    ) -> Self {
        Self {
            microphone,
            speakers,
            processor: VoiceProcessor::new(processing),
            raw: Vec::new(),
            played: Vec::new(),
            speaker_samples: 0,
            reopen: None,
            retry_at: None,
            speaker_failure: None,
        }
    }

    /// Open the speakers again with `opener` whenever they fail or end,
    /// at most once per [`SPEAKER_RETRY`].
    #[must_use]
    pub fn with_reopen(mut self, opener: SpeakerOpener) -> Self {
        if self.speakers.is_none() {
            self.retry_at = Some(Instant::now() + SPEAKER_RETRY);
        }
        self.reopen = Some(opener);
        self
    }

    /// Why the speakers were last lost, as a stable code.
    #[must_use]
    pub fn speaker_failure(&self) -> Option<&'static str> {
        self.speaker_failure
    }

    /// Speaker samples received so far (a count, no audio).
    #[must_use]
    pub fn speaker_samples(&self) -> u64 {
        self.speaker_samples
    }

    /// Hand what the speakers played since the last call to the processor,
    /// without waiting.
    fn take_speakers(&mut self) {
        if self.speakers.is_none()
            && let Some(reopen) = self.reopen.as_mut()
            && self.retry_at.is_none_or(|at| Instant::now() >= at)
        {
            match reopen() {
                Ok(speakers) => {
                    self.speakers = Some(speakers);
                    self.retry_at = None;
                }
                Err(failure) => {
                    self.speaker_failure = Some(failure.code);
                    self.retry_at = Some(Instant::now() + SPEAKER_RETRY);
                }
            }
        }
        let Some(speakers) = self.speakers.as_mut() else {
            return;
        };
        self.played.clear();
        let lost = match speakers.read(&mut self.played, Duration::ZERO) {
            Ok(SourceRead::Ended) => Some("speakers_ended"),
            Err(failure) => Some(failure.code),
            Ok(SourceRead::Data | SourceRead::Idle) => None,
        };
        if let Some(code) = lost {
            self.speakers = None;
            self.speaker_failure = Some(code);
            self.retry_at = Some(Instant::now() + SPEAKER_RETRY);
        }
        self.speaker_samples += self.played.len() as u64;
        self.processor.push_render(&self.played);
    }

    /// One content-free line for the service log when a dictation ends.
    #[must_use]
    pub fn summary(&self) -> String {
        let (frames, guarded, closed) = self.processor.gate.counts();
        let stats = self.processor.apm.statistics();
        format!(
            "echo reference: speaker_samples={} lost={} frames={frames} playing={guarded} silenced={closed} delay_ms={:?} erle_db={:?}",
            self.speaker_samples,
            self.speaker_failure.unwrap_or("none"),
            stats.delay_ms,
            stats.echo_return_loss_enhancement.map(|v| v.round()),
        )
    }

    /// Whether the speakers' sound is still the echo reference.
    #[must_use]
    pub fn has_speakers(&self) -> bool {
        self.speakers.is_some()
    }
}

impl Drop for ProcessedSource {
    fn drop(&mut self) {
        // Counts and codes only: no audio or text reaches the log.
        eprintln!("narrivyd: {}", self.summary());
    }
}

impl CaptureSource for ProcessedSource {
    fn read(&mut self, out: &mut Vec<f32>, wait: Duration) -> Result<SourceRead, PlatformError> {
        self.raw.clear();
        let read = self.microphone.read(&mut self.raw, wait)?;
        self.take_speakers();
        self.processor.process(&self.raw, out);
        Ok(read)
    }

    fn finish(&mut self, out: &mut Vec<f32>) {
        self.raw.clear();
        self.microphone.finish(&mut self.raw);
        self.take_speakers();
        self.processor.process(&self.raw, out);
        self.processor.flush(out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hands out its chunks one per read, then `Ended`.
    struct Fake {
        chunks: VecDeque<Vec<f32>>,
        fail: bool,
    }

    impl Fake {
        fn boxed(chunks: Vec<Vec<f32>>) -> Box<dyn CaptureSource> {
            Box::new(Self {
                chunks: chunks.into(),
                fail: false,
            })
        }
    }

    impl CaptureSource for Fake {
        fn read(
            &mut self,
            out: &mut Vec<f32>,
            _wait: Duration,
        ) -> Result<SourceRead, PlatformError> {
            if self.fail {
                return Err(crate::error("capture_output_unreadable"));
            }
            match self.chunks.pop_front() {
                Some(chunk) => {
                    out.extend_from_slice(&chunk);
                    Ok(SourceRead::Data)
                }
                None => Ok(SourceRead::Ended),
            }
        }

        fn finish(&mut self, out: &mut Vec<f32>) {
            out.extend_from_slice(&[0.0; 7]);
        }
    }

    const NOISE_ONLY: Processing = Processing {
        noise: NoiseLevel::Standard,
        auto_gain: false,
    };

    #[test]
    fn processed_source_keeps_every_microphone_sample() {
        let microphone = Fake::boxed(vec![vec![0.0; 100], vec![0.0; 300], vec![0.0; 1]]);
        let speakers = Fake::boxed(vec![vec![0.0; 160]; 8]);
        let mut source = ProcessedSource::new(microphone, Some(speakers), NOISE_ONLY);
        let mut out = Vec::new();
        for _ in 0..3 {
            assert!(matches!(
                source.read(&mut out, Duration::ZERO),
                Ok(SourceRead::Data)
            ));
        }
        assert!(matches!(
            source.read(&mut out, Duration::ZERO),
            Ok(SourceRead::Ended)
        ));
        source.finish(&mut out);
        // 401 read plus the 7 the microphone's converter still held.
        assert_eq!(out.len(), 408);
    }

    #[test]
    fn speakers_that_end_or_fail_are_dropped_and_capture_goes_on() {
        let microphone = Fake::boxed(vec![vec![0.0; 160]; 3]);
        let speakers = Fake::boxed(vec![vec![0.0; 160]]);
        let mut source = ProcessedSource::new(microphone, Some(speakers), NOISE_ONLY);
        let mut out = Vec::new();
        assert!(source.read(&mut out, Duration::ZERO).is_ok());
        assert!(source.has_speakers());
        assert!(source.read(&mut out, Duration::ZERO).is_ok());
        assert!(!source.has_speakers());
        assert_eq!(out.len(), 320);

        let failing = Box::new(Fake {
            chunks: VecDeque::new(),
            fail: true,
        });
        let microphone = Fake::boxed(vec![vec![0.0; 160]]);
        let mut source = ProcessedSource::new(microphone, Some(failing), NOISE_ONLY);
        assert!(source.read(&mut out, Duration::ZERO).is_ok());
        assert!(!source.has_speakers());
    }

    #[test]
    fn failed_speakers_are_opened_again() {
        let microphone = Fake::boxed(vec![vec![0.0; 160]; 4]);
        let failing = Box::new(Fake {
            chunks: VecDeque::new(),
            fail: true,
        });
        let opened = std::rc::Rc::new(std::cell::Cell::new(0));
        let count = opened.clone();
        let mut source = ProcessedSource::new(microphone, Some(failing), NOISE_ONLY).with_reopen(
            Box::new(move || {
                count.set(count.get() + 1);
                Ok(Fake::boxed(vec![vec![0.0; 160]; 8]))
            }),
        );
        let mut out = Vec::new();
        assert!(source.read(&mut out, Duration::ZERO).is_ok());
        assert!(!source.has_speakers());
        assert_eq!(source.speaker_failure(), Some("capture_output_unreadable"));
        // Not before the retry delay.
        assert!(source.read(&mut out, Duration::ZERO).is_ok());
        assert_eq!(opened.get(), 0);
        source.retry_at = Some(Instant::now());
        assert!(source.read(&mut out, Duration::ZERO).is_ok());
        assert_eq!(opened.get(), 1);
        assert!(source.has_speakers());
        assert_eq!(source.speaker_samples(), 160);
    }

    #[test]
    fn a_microphone_failure_is_the_sessions_failure() {
        let microphone = Box::new(Fake {
            chunks: VecDeque::new(),
            fail: true,
        });
        let mut source = ProcessedSource::new(microphone, None, NOISE_ONLY);
        let mut out = Vec::new();
        assert!(source.read(&mut out, Duration::ZERO).is_err());
    }

    fn processor(noise: NoiseLevel, auto_gain: bool) -> VoiceProcessor {
        VoiceProcessor::new(Processing { noise, auto_gain })
    }

    fn tone(len: usize, hz: f32, amplitude: f32, offset: usize) -> Vec<f32> {
        (0..len)
            .map(|i| {
                let t = (i + offset) as f32 / CAPTURE_RATE_HZ as f32;
                amplitude * (2.0 * std::f32::consts::PI * hz * t).sin()
            })
            .collect()
    }

    #[test]
    fn frame_is_ten_milliseconds() {
        assert_eq!(FRAME_SAMPLES, 160);
    }

    #[test]
    fn whole_frames_come_out_and_partial_frames_wait() {
        let mut p = processor(NoiseLevel::Off, false);
        let mut out = Vec::new();
        p.process(&[0.0; 100], &mut out);
        assert!(out.is_empty());
        p.process(&[0.0; 100], &mut out);
        assert_eq!(out.len(), FRAME_SAMPLES);
        p.process(&[0.0; 3 * FRAME_SAMPLES], &mut out);
        assert_eq!(out.len(), 4 * FRAME_SAMPLES);
    }

    #[test]
    fn flush_keeps_exactly_the_real_samples() {
        let mut p = processor(NoiseLevel::Standard, false);
        let mut out = Vec::new();
        p.process(&[0.0; FRAME_SAMPLES + 37], &mut out);
        p.flush(&mut out);
        assert_eq!(out.len(), FRAME_SAMPLES + 37);
        // Nothing left: a second flush adds nothing.
        p.flush(&mut out);
        assert_eq!(out.len(), FRAME_SAMPLES + 37);
    }

    #[test]
    fn every_input_sample_has_one_output_sample() {
        let mut p = processor(NoiseLevel::Strong, true);
        let mut out = Vec::new();
        let mut total = 0;
        for len in [1, 159, 160, 161, 333, 7, 1280] {
            p.process(&vec![0.01; len], &mut out);
            total += len;
        }
        p.flush(&mut out);
        assert_eq!(out.len(), total);
        assert!(out.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn speaker_backlog_is_bounded_and_consumed_per_frame() {
        let mut p = processor(NoiseLevel::Off, false);
        p.push_render(&vec![0.1; MAX_RENDER_BACKLOG + 500]);
        assert_eq!(p.render_backlog(), MAX_RENDER_BACKLOG);
        let mut out = Vec::new();
        p.process(&[0.0; 2 * FRAME_SAMPLES], &mut out);
        assert_eq!(p.render_backlog(), MAX_RENDER_BACKLOG - 2 * FRAME_SAMPLES);
        // A partial speaker frame is held, not used.
        let mut q = processor(NoiseLevel::Off, false);
        q.push_render(&[0.1; 50]);
        q.process(&[0.0; FRAME_SAMPLES], &mut out);
        assert_eq!(q.render_backlog(), 50);
    }

    #[test]
    fn the_users_voice_passes_when_the_speakers_are_silent() {
        let mut p = processor(NoiseLevel::Off, false);
        let mut out = Vec::new();
        let voice = tone(300 * FRAME_SAMPLES, 220.0, 0.3, 0);
        p.process(&voice, &mut out);
        let tail = out.len() - 100 * FRAME_SAMPLES;
        let before = energy(&voice[tail..]);
        let after = energy(&out[tail..]);
        assert!(after > before * 0.5, "before {before}, after {after}");
    }

    #[test]
    fn speaker_sound_heard_by_the_microphone_is_reduced() {
        // The microphone hears only the speakers (an echo 10 ms late at
        // half the level); after the canceller converges the output is
        // much quieter than the input.
        let mut p = processor(NoiseLevel::Off, false);
        let mut out = Vec::new();
        let frames = 400; // 4 s
        let delay = FRAME_SAMPLES;
        // Broadband sound (as speech and music are), from a fixed seed.
        let mut seed = 0x2545_f491_u32;
        let speaker: Vec<f32> = (0..(frames + 1) * FRAME_SAMPLES)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                (seed as f32 / u32::MAX as f32 - 0.5) * 0.4
            })
            .collect();
        let mut microphone = Vec::new();
        for frame in 0..frames {
            let start = frame * FRAME_SAMPLES;
            p.push_render(&speaker[start..start + FRAME_SAMPLES]);
            let echo: Vec<f32> = (start..start + FRAME_SAMPLES)
                .map(|i| {
                    if i >= delay {
                        0.5 * speaker[i - delay]
                    } else {
                        0.0
                    }
                })
                .collect();
            microphone.extend_from_slice(&echo);
            p.process(&echo, &mut out);
        }
        let tail = out.len() - 100 * FRAME_SAMPLES;
        let before = energy(&microphone[tail..]);
        let after = energy(&out[tail..]);
        assert!(after < before * 0.1, "before {before}, after {after}");
    }

    #[test]
    fn the_gate_closes_on_the_predicted_echo_after_its_hold() {
        let mut gate = EchoGate::default();
        // Speakers at 0.1 and an echo 10 dB quieter: 4 s of only that
        // teaches the gate how loud the room returns them.
        for _ in 0..400 {
            gate.next_gain(0.01, 0.1, 5);
        }
        // The user talks over it: open, with the hold.
        assert_eq!(gate.next_gain(0.2, 0.1, 5), 1.0);
        for _ in 0..GATE_HOLD_FRAMES {
            assert_eq!(gate.next_gain(0.01, 0.1, 5), 1.0);
        }
        // Only the echo, the hold used up: silent.
        assert_eq!(gate.next_gain(0.01, 0.1, 5), 0.0);
        // The user speaks again: open at once.
        assert_eq!(gate.next_gain(0.1, 0.1, 5), 1.0);
    }

    #[test]
    fn the_gate_waits_out_the_delay_and_the_rooms_echo() {
        let mut gate = EchoGate::default();
        // A quiet room sets the noise floor.
        for _ in 0..50 {
            assert_eq!(gate.next_gain(1e-4, 0.0, 10), 1.0);
        }
        // A loud burst from the speakers, then silence: its echo arrives
        // 100 ms later and the room rings after it, above its noise.
        gate.next_gain(1e-4, 0.1, 10);
        let closed = 10 + REVERB_FRAMES + TAIL_FRAMES as usize;
        for frame in 1..closed {
            assert_eq!(gate.next_gain(5e-4, 0.0, 10), 0.0, "frame {frame}");
        }
        // Then the speakers' sound is over and the gate opens.
        assert_eq!(gate.next_gain(5e-4, 0.0, 10), 1.0);
    }

    #[test]
    fn the_rooms_noise_between_the_speakers_words_is_not_a_voice() {
        let mut gate = EchoGate::default();
        // A quiet room (1e-4) while nothing plays sets the noise floor.
        for _ in 0..50 {
            assert_eq!(gate.next_gain(1e-4, 0.0, 5), 1.0);
        }
        // The speakers play very quietly: the predicted echo is below the
        // room's noise, which still is no voice.
        for _ in 0..(GATE_HOLD_FRAMES + 1) {
            gate.next_gain(4e-4, 1e-5, 5);
        }
        assert_eq!(gate.next_gain(4e-4, 1e-5, 5), 0.0);
        // A voice 20 dB over the room opens it.
        assert_eq!(gate.next_gain(1e-2, 1e-5, 5), 1.0);
    }

    #[test]
    fn the_gate_stays_open_while_nothing_plays() {
        let mut gate = EchoGate::default();
        for _ in 0..100 {
            assert_eq!(gate.next_gain(1e-4, 0.0, 9), 1.0);
        }
    }

    #[test]
    fn the_gate_ramps_without_a_step() {
        let mut frame = vec![1.0; FRAME_SAMPLES];
        EchoGate::apply(1.0, 0.0, &mut frame);
        assert!(frame[0] > 0.99);
        assert_eq!(frame[FRAME_SAMPLES - 1], 0.0);
        assert!(frame.windows(2).all(|w| w[1] <= w[0]));
    }

    #[test]
    fn the_computers_sound_alone_comes_out_silent() {
        // As the echo test below, but the residue must be gone entirely
        // once the canceller has converged.
        let mut p = processor(NoiseLevel::Standard, false);
        let mut out = Vec::new();
        let frames = 400;
        let mut seed = 0x1234_5678_u32;
        for _ in 0..frames {
            let speaker: Vec<f32> = (0..FRAME_SAMPLES)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    (seed as f32 / u32::MAX as f32 - 0.5) * 0.4
                })
                .collect();
            p.push_render(&speaker);
            let echo: Vec<f32> = speaker.iter().map(|s| s * 0.5).collect();
            p.process(&echo, &mut out);
        }
        let tail = &out[out.len() - 100 * FRAME_SAMPLES..];
        assert!(energy(tail) < 1e-9, "residue {}", energy(tail));
    }

    #[test]
    fn the_users_voice_over_the_computers_sound_passes() {
        // The computer plays while the user talks louder than its echo.
        let mut p = processor(NoiseLevel::Off, false);
        let mut out = Vec::new();
        let mut microphone = Vec::new();
        let mut seed = 0x0bad_cafe_u32;
        for frame in 0..400 {
            let speaker: Vec<f32> = (0..FRAME_SAMPLES)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    (seed as f32 / u32::MAX as f32 - 0.5) * 0.2
                })
                .collect();
            p.push_render(&speaker);
            let voice = tone(FRAME_SAMPLES, 220.0, 0.3, frame * FRAME_SAMPLES);
            let heard: Vec<f32> = voice
                .iter()
                .zip(&speaker)
                .map(|(v, s)| v + 0.25 * s)
                .collect();
            microphone.extend_from_slice(&voice);
            p.process(&heard, &mut out);
        }
        let tail = out.len() - 100 * FRAME_SAMPLES;
        let voice = energy(&microphone[tail..]);
        let kept = energy(&out[tail..]);
        assert!(kept > voice * 0.3, "voice {voice}, kept {kept}");
    }

    #[test]
    fn the_users_voice_as_loud_as_the_echo_passes() {
        // The speakers' echo is as loud at the microphone as the user
        // (the old gate wanted the voice 6 dB louder); after the canceller
        // the voice stands out.
        let mut p = processor(NoiseLevel::Off, false);
        let (mut out, mut microphone) = (Vec::new(), Vec::new());
        let mut seed = 0x5eed_1234_u32;
        for frame in 0..700 {
            let speaker: Vec<f32> = (0..FRAME_SAMPLES)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    (seed as f32 / u32::MAX as f32 - 0.5) * 0.2
                })
                .collect();
            p.push_render(&speaker);
            // The user starts after the gate has learned the room (its
            // first 3 s of playback), and talks in syllables (150 ms on,
            // 100 ms off).
            let voice = if frame < 400 || frame % 25 >= 15 {
                vec![0.0; FRAME_SAMPLES]
            } else {
                tone(FRAME_SAMPLES, 220.0, 0.08, frame * FRAME_SAMPLES)
            };
            let heard: Vec<f32> = voice.iter().zip(&speaker).map(|(v, s)| v + s).collect();
            microphone.extend_from_slice(&voice);
            p.process(&heard, &mut out);
        }
        let tail = out.len() - 200 * FRAME_SAMPLES;
        let voice = energy(&microphone[tail..]);
        let kept = energy(&out[tail..]);
        assert!(kept > voice * 0.3, "voice {voice}, kept {kept}");
        let echo_alone = &out[300 * FRAME_SAMPLES..400 * FRAME_SAMPLES];
        assert!(
            energy(echo_alone) < 1e-7,
            "echo left {}",
            energy(echo_alone)
        );
    }

    /// Scores a take given as raw f32 files (16 kHz mono): the microphone
    /// (`NARRIVY_TAKE_MIC`), what the speakers played (`NARRIVY_TAKE_SPK`)
    /// and, optionally, the user's voice alone as it is in the microphone
    /// (`NARRIVY_TAKE_VOICE`). Prints how many frames with the voice the
    /// gate kept while the speakers played, and how many frames with only
    /// the speakers' echo came out above -40 dB; `NARRIVY_TAKE_OUT` gets the
    /// output.
    #[test]
    #[ignore = "needs a take"]
    fn score_take() {
        let read = |key: &str| -> Option<Vec<f32>> {
            let bytes = std::fs::read(std::env::var_os(key)?).ok()?;
            Some(
                bytes
                    .chunks_exact(4)
                    .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                    .collect(),
            )
        };
        let (Some(microphone), Some(speakers)) =
            (read("NARRIVY_TAKE_MIC"), read("NARRIVY_TAKE_SPK"))
        else {
            return;
        };
        let voice = read("NARRIVY_TAKE_VOICE");
        let mut p = processor(NoiseLevel::Standard, false);
        let (mut out, mut gains) = (Vec::new(), Vec::new());
        for (i, frame) in microphone.chunks_exact(FRAME_SAMPLES).enumerate() {
            if let Some(speaker) = speakers.get(i * FRAME_SAMPLES..(i + 1) * FRAME_SAMPLES) {
                p.push_render(speaker);
            }
            p.process(frame, &mut out);
            gains.push(p.gate.gain);
        }
        let db = |v: &[f32]| 10.0 * energy(v).max(1e-12).log10();
        let frames = gains.len();
        let speaking: Vec<bool> = (0..frames)
            .map(|i| {
                speakers
                    .get(i.saturating_sub(60) * FRAME_SAMPLES..(i + 1) * FRAME_SAMPLES)
                    .is_some_and(|w| w.chunks(FRAME_SAMPLES).any(|f| db(f) > -50.0))
            })
            .collect();
        let voiced: Vec<bool> = (0..frames)
            .map(|i| {
                voice.as_ref().is_some_and(|v| {
                    v.get(i * FRAME_SAMPLES..(i + 1) * FRAME_SAMPLES)
                        .is_some_and(|f| db(f) > -40.0)
                })
            })
            .collect();
        let near_voice = |i: usize| {
            voiced[i.saturating_sub(20)..(i + 20).min(frames)]
                .iter()
                .any(|v| *v)
        };
        let count = |f: &dyn Fn(usize) -> bool| (0..frames).filter(|i| f(*i)).count();
        let talk = count(&|i| speaking[i] && voiced[i]);
        let kept = count(&|i| speaking[i] && voiced[i] && gains[i] > 0.5);
        let alone = count(&|i| !speaking[i] && voiced[i]);
        let alone_kept = count(&|i| !speaking[i] && voiced[i] && gains[i] > 0.5);
        let echo_only = count(&|i| speaking[i] && !near_voice(i));
        let leaks = count(&|i| {
            speaking[i]
                && !near_voice(i)
                && out
                    .get(i * FRAME_SAMPLES..(i + 1) * FRAME_SAMPLES)
                    .is_some_and(|f| db(f) > -40.0)
        });
        if std::env::var_os("NARRIVY_TAKE_WHERE").is_some() {
            let at: Vec<String> = (0..frames)
                .filter(|i| {
                    speaking[*i]
                        && !near_voice(*i)
                        && out
                            .get(i * FRAME_SAMPLES..(i + 1) * FRAME_SAMPLES)
                            .is_some_and(|f| db(f) > -40.0)
                })
                .map(|i| {
                    let mic = db(&microphone[i * FRAME_SAMPLES..(i + 1) * FRAME_SAMPLES]);
                    let o = db(&out[i * FRAME_SAMPLES..(i + 1) * FRAME_SAMPLES]);
                    let v = voice.as_ref().map_or(-120.0, |v| {
                        db(&v[i * FRAME_SAMPLES..(i + 1) * FRAME_SAMPLES])
                    });
                    format!(
                        "{:.2}s(mic {mic:.0} out {o:.0} voice {v:.0})",
                        i as f32 / 100.0
                    )
                })
                .collect();
            println!("leaks at {}", at.join(" "));
        }
        println!("stats {:?}", p.apm.statistics());
        println!(
            "voice kept while playing {kept}/{talk}, voice kept otherwise {alone_kept}/{alone}, echo-only leaks {leaks}/{echo_only}"
        );
        if let Some(path) = std::env::var_os("NARRIVY_TAKE_OUT") {
            let raw: Vec<u8> = out.iter().flat_map(|s| s.to_le_bytes()).collect();
            assert!(std::fs::write(path, raw).is_ok());
        }
    }

    /// Replays a take recorded by `os::processed_capture`'s
    /// `record_room_take` (path in `NARRIVY_TAKE_PATH`) through the
    /// processor and prints, per 100 ms, the microphone, output and speaker
    /// levels; `NARRIVY_TAKE_OUT` gets the output as raw f32.
    #[test]
    #[ignore = "needs a recorded room take"]
    fn replay_room_take() {
        let Some(path) = std::env::var_os("NARRIVY_TAKE_PATH") else {
            return;
        };
        let Ok(bytes) = std::fs::read(path) else {
            return;
        };
        let mut p = processor(NoiseLevel::Standard, false);
        let delay: Option<i32> = std::env::var("NARRIVY_TAKE_DELAY")
            .ok()
            .and_then(|v| v.parse().ok());
        let (mut microphone, mut out, mut speakers) = (Vec::new(), Vec::new(), Vec::new());
        let mut at = 0;
        while at + 5 <= bytes.len() {
            let kind = bytes[at];
            let len =
                u32::from_le_bytes([bytes[at + 1], bytes[at + 2], bytes[at + 3], bytes[at + 4]])
                    as usize;
            at += 5;
            let chunk: Vec<f32> = bytes[at..at + 4 * len]
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .collect();
            at += 4 * len;
            if kind == 0 {
                if let Some(delay) = delay {
                    let _ = p.apm.set_stream_delay_ms(delay);
                }
                microphone.extend_from_slice(&chunk);
                p.process(&chunk, &mut out);
            } else {
                speakers.extend_from_slice(&chunk);
                p.push_render(&chunk);
            }
        }
        let db = |v: &[f32]| 10.0 * energy(v).max(1e-12).log10();
        println!("stats {:?}", p.apm.statistics());
        // Leak score: 10 ms output frames above -40 dB while the computer
        // plays (speaker frame above -50 dB within the last 600 ms).
        let playing: Vec<bool> = speakers
            .chunks(FRAME_SAMPLES)
            .map(|f| db(f) > -50.0)
            .collect();
        let leaks = out
            .chunks(FRAME_SAMPLES)
            .enumerate()
            .filter(|(i, f)| {
                let from = i.saturating_sub(60);
                playing
                    .get(from..(*i).min(playing.len()))
                    .is_some_and(|w| w.iter().any(|p| *p))
                    && db(f) > -40.0
            })
            .count();
        println!("leaks {leaks}");
        for (i, (mic, done)) in microphone.chunks(1600).zip(out.chunks(1600)).enumerate() {
            let spk = speakers.get(i * 1600..(i + 1) * 1600).unwrap_or(&[]);
            println!(
                "{:5.1}s mic {:6.1} out {:6.1} spk {:6.1}",
                i as f32 / 10.0,
                db(mic),
                db(done),
                db(spk)
            );
        }
        if let Some(path) = std::env::var_os("NARRIVY_TAKE_OUT") {
            let raw: Vec<u8> = out.iter().flat_map(|s| s.to_le_bytes()).collect();
            assert!(std::fs::write(path, raw).is_ok());
        }
    }
}
