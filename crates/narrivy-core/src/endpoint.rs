//! Portable, allocation-free speech activity and endpoint policy contracts.
//!
//! These components run on the orchestration worker after capture has left the
//! platform producer thread. They never own a device, model, timer, journal,
//! or delivery side effect. VAD classifies one audio chunk; endpoint policy
//! decides whether already-captured speech has been followed by enough silence
//! to request graceful capture finalization.

use narrivy_types::AudioChunk;

/// Stable content-free failure from VAD or endpoint policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EndpointError {
    pub code: &'static str,
}

/// One analysis window's activity classification. This is not an ASR hypothesis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeechActivity {
    Speech,
    Silence,
}

/// Fixed analysis window for activity decisions.
///
/// Activity is classified per window rather than per delivered chunk, so the
/// decision sequence is a function of the audio alone. Chunk boundaries are a
/// property of the capture transport and must never change classification.
pub const ACTIVITY_WINDOW_MS: u32 = 20;

/// Windows per noise-floor estimation block. A block's quietest window is a
/// room-tone sample, so the floor tracks the room rather than the speech.
const FLOOR_BLOCK_WINDOWS: u64 = 50;

/// Largest number of completed windows one chunk may yield. Bounds the
/// allocation-free decision buffer; a chunk this large is a capture defect.
const MAX_ACTIVITY_DECISIONS: usize = 512;

/// Replaceable speech-activity boundary (`speech.vad`).
///
/// A chunk may complete zero, one, or many [`ACTIVITY_WINDOW_MS`] windows, so
/// `classify` returns every window the chunk completed. Each decision covers
/// exactly `window_frames` frames.
pub trait VoiceActivityDetector: Send {
    fn classify(&mut self, chunk: &AudioChunk) -> Result<&[SpeechActivity], EndpointError>;

    /// Frames one complete decision covers for a stream at `sample_rate_hz`.
    fn window_frames(&self, sample_rate_hz: u32) -> Result<u64, EndpointError>;
}

/// Energy VAD configuration.
///
/// Thresholds are ratios against a tracked noise floor rather than absolute
/// levels. A fixed absolute level cannot separate speech from room tone across
/// one session: measured room-tone window RMS is ~0.0085-0.0124 on archival
/// tape material and ~0.0006-0.0014 on a quiet-room USB condenser, a ~14x
/// spread. An absolute threshold therefore either sits above a quiet
/// microphone's speech, releasing on every breath, or below a noisy recording's
/// room tone, never releasing so endpointing goes inert and capture only ends on
/// the safety window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EnergyVadConfig {
    /// Window RMS must reach this multiple of the tracked floor to start speech.
    pub activation_rms_snr: f32,
    /// Window RMS must stay at this multiple of the tracked floor to hold speech.
    pub release_rms_snr: f32,
    /// Window peak must reach this multiple of the tracked floor to start speech.
    pub activation_peak_snr: f32,
    /// Window peak must stay at this multiple of the tracked floor to hold speech.
    pub release_peak_snr: f32,
    /// Rate the floor rises toward a block minimum. 0.5 tracks it within ~2 blocks.
    pub floor_rise: f32,
    /// Lower clamp on the floor, so digital silence cannot drive it to zero.
    pub floor_min: f32,
    /// Upper clamp on the floor, so continuous speech cannot inflate it.
    pub floor_max: f32,
}

impl Default for EnergyVadConfig {
    fn default() -> Self {
        Self {
            activation_rms_snr: 3.0,
            release_rms_snr: 1.8,
            activation_peak_snr: 3.0,
            release_peak_snr: 1.5,
            floor_rise: 0.5,
            floor_min: 0.000_2,
            floor_max: 0.5,
        }
    }
}

impl EnergyVadConfig {
    pub fn validate(self) -> Result<Self, EndpointError> {
        let ratios = [
            self.activation_rms_snr,
            self.release_rms_snr,
            self.activation_peak_snr,
            self.release_peak_snr,
        ];
        if ratios
            .iter()
            .any(|ratio| !ratio.is_finite() || *ratio <= 1.0)
            || self.release_rms_snr >= self.activation_rms_snr
            || self.release_peak_snr >= self.activation_peak_snr
            || !self.floor_rise.is_finite()
            || !(0.0..=1.0).contains(&self.floor_rise)
            || !self.floor_min.is_finite()
            || !self.floor_max.is_finite()
            || self.floor_min <= 0.0
            || self.floor_max < self.floor_min
        {
            return Err(EndpointError {
                code: "vad_config_invalid",
            });
        }
        Ok(self)
    }
}

/// Deterministic energy VAD used by the Linux 1.0 fallback slot.
///
/// Speech is decided per fixed [`ACTIVITY_WINDOW_MS`] window against a tracked
/// room-tone floor. Both RMS and peak must clear their ratio, and release uses
/// lower ratios than activation so energy near the boundary cannot flap speech
/// state. This intentionally makes no neural-VAD quality claim.
pub struct NativeEnergyVad {
    config: EnergyVadConfig,
    sample_rate_hz: u32,
    window_frames: u64,
    accumulated_samples: u64,
    sum_squares: f64,
    peak: f32,
    floor: f32,
    floor_ready: bool,
    block_min: f32,
    block_windows: u64,
    active: bool,
    decisions: [SpeechActivity; MAX_ACTIVITY_DECISIONS],
    decision_count: usize,
}

impl Default for NativeEnergyVad {
    fn default() -> Self {
        let config = EnergyVadConfig::default();
        Self {
            // Start at the optimistic prior so the un-measured first block
            // cannot read speech as silence.
            floor: config.floor_min,
            config,
            sample_rate_hz: 0,
            window_frames: 0,
            accumulated_samples: 0,
            sum_squares: 0.0,
            peak: 0.0,
            floor_ready: false,
            block_min: f32::MAX,
            block_windows: 0,
            active: false,
            decisions: [SpeechActivity::Silence; MAX_ACTIVITY_DECISIONS],
            decision_count: 0,
        }
    }
}

impl NativeEnergyVad {
    pub fn new(config: EnergyVadConfig) -> Result<Self, EndpointError> {
        Ok(Self {
            config: config.validate()?,
            ..Self::default()
        })
    }

    /// Bind the stream's sample rate once, so a mid-stream rate change cannot
    /// silently redefine what a window covers.
    fn bind_rate(&mut self, sample_rate_hz: u32) -> Result<u64, EndpointError> {
        if self.sample_rate_hz == 0 {
            self.window_frames = milliseconds_to_frames(ACTIVITY_WINDOW_MS, sample_rate_hz)?.max(1);
            self.sample_rate_hz = sample_rate_hz;
        } else if self.sample_rate_hz != sample_rate_hz {
            return Err(EndpointError {
                code: "vad_sample_rate_changed",
            });
        }
        Ok(self.window_frames)
    }

    /// Update the room-tone estimate from the quietest window of the block.
    ///
    /// The floor is not trusted until a full block has been measured, so the
    /// first block is classified against `floor_min` — an optimistic prior that
    /// prefers reading energy as speech. That direction is deliberate: a floor
    /// that is too low can only delay a hang-over, while one that is too high
    /// truncates the utterance, and truncation is the defect this module exists
    /// to prevent.
    fn update_floor(&mut self, rms: f32) {
        self.block_min = self.block_min.min(rms);
        self.block_windows += 1;
        if self.block_windows < FLOOR_BLOCK_WINDOWS {
            return;
        }
        let block_min = self.block_min;
        self.block_min = f32::MAX;
        self.block_windows = 0;
        // The floor tracks the block minimum: it falls immediately to a quieter
        // room and rises only as fast as that minimum itself moves, so speech
        // inside the block cannot pull it up.
        if !self.floor_ready || block_min < self.floor {
            self.floor = block_min;
            self.floor_ready = true;
        } else {
            self.floor += (block_min - self.floor) * self.config.floor_rise;
        }
        self.floor = self
            .floor
            .clamp(self.config.floor_min, self.config.floor_max);
    }

    /// Classify the accumulated window and emit one decision.
    fn decide_window(&mut self) -> Result<(), EndpointError> {
        if self.accumulated_samples == 0 {
            return Ok(());
        }
        let rms = (self.sum_squares / self.accumulated_samples as f64).sqrt() as f32;
        let peak = self.peak;
        self.sum_squares = 0.0;
        self.peak = 0.0;
        self.accumulated_samples = 0;
        self.update_floor(rms);
        let (rms_ratio, peak_ratio) = if self.active {
            (self.config.release_rms_snr, self.config.release_peak_snr)
        } else {
            (
                self.config.activation_rms_snr,
                self.config.activation_peak_snr,
            )
        };
        self.active = rms >= self.floor * rms_ratio && peak >= self.floor * peak_ratio;
        if self.decision_count >= MAX_ACTIVITY_DECISIONS {
            return Err(EndpointError {
                code: "vad_chunk_too_large",
            });
        }
        self.decisions[self.decision_count] = if self.active {
            SpeechActivity::Speech
        } else {
            SpeechActivity::Silence
        };
        self.decision_count += 1;
        Ok(())
    }

    fn decisions(&self) -> &[SpeechActivity] {
        &self.decisions[..self.decision_count]
    }
}

impl VoiceActivityDetector for NativeEnergyVad {
    fn classify(&mut self, chunk: &AudioChunk) -> Result<&[SpeechActivity], EndpointError> {
        if chunk.sample_rate_hz == 0
            || chunk.channels == 0
            || chunk.samples.is_empty()
            || chunk.samples.len() % usize::from(chunk.channels) != 0
        {
            return Err(EndpointError {
                code: "vad_audio_format_invalid",
            });
        }
        // Validate the whole chunk before mutating any state, so a rejected
        // chunk cannot leave the floor or speech state half-updated.
        if chunk.samples.iter().any(|sample| !sample.is_finite()) {
            return Err(EndpointError {
                code: "vad_audio_nonfinite",
            });
        }
        self.decision_count = 0;
        let window_samples = self.bind_rate(chunk.sample_rate_hz)? as usize;
        for sample in &chunk.samples {
            let magnitude = sample.abs().min(1.0);
            self.peak = self.peak.max(magnitude);
            self.sum_squares += f64::from(magnitude) * f64::from(magnitude);
            self.accumulated_samples += 1;
            if self.accumulated_samples == window_samples as u64 {
                self.decide_window()?;
            }
        }
        Ok(self.decisions())
    }

    fn window_frames(&self, sample_rate_hz: u32) -> Result<u64, EndpointError> {
        milliseconds_to_frames(ACTIVITY_WINDOW_MS, sample_rate_hz)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointDecision {
    Continue,
    Finalize,
}

/// Replaceable endpoint boundary (`speech.endpoint`).
pub trait SpeechEndpointer: Send {
    fn observe(
        &mut self,
        activity: SpeechActivity,
        frames: u64,
        sample_rate_hz: u32,
    ) -> Result<EndpointDecision, EndpointError>;
}

/// Sample-count-based endpoint policy; wall-clock adjustment cannot alter it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DictationEndpointConfig {
    pub minimum_speech_ms: u32,
    pub trailing_silence_ms: u32,
}

impl Default for DictationEndpointConfig {
    fn default() -> Self {
        Self {
            minimum_speech_ms: 240,
            // Natural clause pauses in continuous speech run well past half a
            // second; a shorter hangover truncates the utterance at a sentence
            // boundary, which is not an error the transcriber can recover from.
            trailing_silence_ms: 1_500,
        }
    }
}

impl DictationEndpointConfig {
    pub fn validate(self) -> Result<Self, EndpointError> {
        if !(80..=2_000).contains(&self.minimum_speech_ms)
            || !(160..=5_000).contains(&self.trailing_silence_ms)
        {
            return Err(EndpointError {
                code: "endpoint_config_invalid",
            });
        }
        Ok(self)
    }
}

/// Lean dictation endpoint: never ends before real speech has accumulated,
/// then waits for a bounded silence hangover to protect word endings.
#[derive(Default)]
pub struct DictationEndpointer {
    config: DictationEndpointConfig,
    speech_frames: u64,
    trailing_silence_frames: u64,
    finalized: bool,
}

impl DictationEndpointer {
    pub fn new(config: DictationEndpointConfig) -> Result<Self, EndpointError> {
        Ok(Self {
            config: config.validate()?,
            speech_frames: 0,
            trailing_silence_frames: 0,
            finalized: false,
        })
    }
}

impl SpeechEndpointer for DictationEndpointer {
    fn observe(
        &mut self,
        activity: SpeechActivity,
        frames: u64,
        sample_rate_hz: u32,
    ) -> Result<EndpointDecision, EndpointError> {
        if self.finalized || frames == 0 || sample_rate_hz == 0 {
            return Err(EndpointError {
                code: if self.finalized {
                    "endpoint_already_finalized"
                } else {
                    "endpoint_audio_format_invalid"
                },
            });
        }
        match activity {
            SpeechActivity::Speech => {
                self.speech_frames =
                    self.speech_frames
                        .checked_add(frames)
                        .ok_or(EndpointError {
                            code: "endpoint_duration_overflow",
                        })?;
                self.trailing_silence_frames = 0;
            }
            SpeechActivity::Silence => {
                if self.speech_frames
                    >= milliseconds_to_frames(self.config.minimum_speech_ms, sample_rate_hz)?
                {
                    self.trailing_silence_frames = self
                        .trailing_silence_frames
                        .checked_add(frames)
                        .ok_or(EndpointError {
                            code: "endpoint_duration_overflow",
                        })?;
                } else {
                    // Separate clicks/noise bursts cannot accumulate across a
                    // silence gap into a fabricated utterance.
                    self.speech_frames = 0;
                }
            }
        }
        if self.trailing_silence_frames
            >= milliseconds_to_frames(self.config.trailing_silence_ms, sample_rate_hz)?
        {
            self.finalized = true;
            Ok(EndpointDecision::Finalize)
        } else {
            Ok(EndpointDecision::Continue)
        }
    }
}

fn milliseconds_to_frames(ms: u32, sample_rate_hz: u32) -> Result<u64, EndpointError> {
    u64::from(ms)
        .checked_mul(u64::from(sample_rate_hz))
        .and_then(|samples_ms| samples_ms.checked_add(999))
        .map(|rounded| rounded / 1_000)
        .ok_or(EndpointError {
            code: "endpoint_duration_overflow",
        })
}

/// Pair of independently replaceable VAD and endpoint modules for one stream.
pub struct StreamingEndpointing<'a> {
    pub vad: &'a mut dyn VoiceActivityDetector,
    pub endpointer: &'a mut dyn SpeechEndpointer,
}

impl StreamingEndpointing<'_> {
    /// Feed one captured chunk. A chunk may complete several activity windows,
    /// so the endpointer is driven once per window rather than once per chunk.
    ///
    /// A partial window left at end of capture is never classified, which can
    /// only delay a hang-over that completes within the final
    /// [`ACTIVITY_WINDOW_MS`] of audio. It cannot shorten the hang-over.
    pub fn observe(&mut self, chunk: &AudioChunk) -> Result<EndpointDecision, EndpointError> {
        if chunk.sample_rate_hz == 0
            || chunk.channels == 0
            || chunk.samples.is_empty()
            || chunk.samples.len() % usize::from(chunk.channels) != 0
        {
            return Err(EndpointError {
                code: "endpoint_audio_format_invalid",
            });
        }
        let sample_rate_hz = chunk.sample_rate_hz;
        let window_frames = self.vad.window_frames(sample_rate_hz)?;
        let mut decision = EndpointDecision::Continue;
        for activity in self.vad.classify(chunk)? {
            if self
                .endpointer
                .observe(*activity, window_frames, sample_rate_hz)?
                == EndpointDecision::Finalize
            {
                decision = EndpointDecision::Finalize;
                break;
            }
        }
        Ok(decision)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW_FRAMES: usize = 320;

    fn chunk(value: f32, frames: usize) -> AudioChunk {
        AudioChunk {
            sample_rate_hz: 16_000,
            channels: 1,
            samples: vec![value; frames],
        }
    }

    /// Constant-level stream long enough for the floor estimator to converge.
    fn feed(vad: &mut NativeEnergyVad, value: f32, windows: usize) -> Vec<SpeechActivity> {
        let mut out = Vec::new();
        for _ in 0..windows {
            out.extend_from_slice(vad.classify(&chunk(value, WINDOW_FRAMES)).unwrap_or(&[]));
        }
        out
    }

    #[test]
    fn energy_vad_validates_configuration_and_audio() -> Result<(), EndpointError> {
        assert!(NativeEnergyVad::new(EnergyVadConfig::default()).is_ok());
        for invalid in [
            EnergyVadConfig {
                release_rms_snr: 9.0,
                ..EnergyVadConfig::default()
            },
            EnergyVadConfig {
                activation_rms_snr: 0.5,
                ..EnergyVadConfig::default()
            },
            EnergyVadConfig {
                release_peak_snr: 9.0,
                ..EnergyVadConfig::default()
            },
            EnergyVadConfig {
                floor_min: 0.0,
                ..EnergyVadConfig::default()
            },
            EnergyVadConfig {
                floor_max: 0.0,
                ..EnergyVadConfig::default()
            },
            EnergyVadConfig {
                floor_rise: 1.5,
                ..EnergyVadConfig::default()
            },
        ] {
            assert_eq!(
                NativeEnergyVad::new(invalid).err(),
                Some(EndpointError {
                    code: "vad_config_invalid"
                }),
                "expected rejection of {invalid:?}"
            );
        }
        let mut vad = NativeEnergyVad::new(EnergyVadConfig::default())?;
        assert_eq!(
            vad.classify(&chunk(f32::NAN, 1)).err(),
            Some(EndpointError {
                code: "vad_audio_nonfinite"
            })
        );
        assert_eq!(
            vad.classify(&AudioChunk {
                sample_rate_hz: 16_000,
                channels: 2,
                samples: vec![0.1; 3],
            })
            .err(),
            Some(EndpointError {
                code: "vad_audio_format_invalid"
            })
        );
        Ok(())
    }

    /// A rejected chunk must not disturb the tracked floor or speech state.
    #[test]
    fn energy_vad_rejected_chunk_leaves_state_untouched() -> Result<(), EndpointError> {
        let mut vad = NativeEnergyVad::new(EnergyVadConfig::default())?;
        let baseline = feed(&mut vad, 0.001, 120);
        let mut fresh = NativeEnergyVad::new(EnergyVadConfig::default())?;
        for _ in 0..60 {
            assert!(fresh.classify(&chunk(f32::NAN, WINDOW_FRAMES)).is_err());
        }
        assert_eq!(feed(&mut fresh, 0.001, 120), baseline);
        Ok(())
    }

    /// The first block is classified against the `floor_min` prior before any
    /// room tone has been measured. That direction is deliberate: reading early
    /// energy as speech can only delay a hang-over, never truncate a sentence.
    #[test]
    fn energy_vad_warmup_reads_leading_speech_before_the_floor_is_known()
    -> Result<(), EndpointError> {
        let mut vad = NativeEnergyVad::new(EnergyVadConfig::default())?;
        let leading = feed(&mut vad, 0.05, 20);
        assert!(
            leading.iter().all(|a| *a == SpeechActivity::Speech),
            "a stream opening on speech must not be classified as silence"
        );
        Ok(())
    }

    #[test]
    fn energy_vad_separates_speech_from_a_quiet_room_floor() -> Result<(), EndpointError> {
        let mut vad = NativeEnergyVad::new(EnergyVadConfig::default())?;
        // Converge the floor on a quiet room, then assert on steady state.
        feed(&mut vad, 0.0008, 150);
        assert!(
            feed(&mut vad, 0.0008, 40)
                .iter()
                .all(|a| *a == SpeechActivity::Silence)
        );
        assert!(
            feed(&mut vad, 0.05, 40)
                .iter()
                .all(|a| *a == SpeechActivity::Speech)
        );
        assert!(
            feed(&mut vad, 0.0008, 40)
                .iter()
                .all(|a| *a == SpeechActivity::Silence)
        );
        Ok(())
    }

    /// The tracked floor must follow the room, not the speech level. This is the
    /// case an absolute threshold cannot satisfy: archival tape room tone and a
    /// quiet-room USB microphone differ by roughly 14x.
    #[test]
    fn energy_vad_floor_adapts_across_a_wide_noise_level_range() -> Result<(), EndpointError> {
        for room_tone in [0.0008_f32, 0.0085, 0.012] {
            let mut vad = NativeEnergyVad::new(EnergyVadConfig::default())?;
            feed(&mut vad, room_tone, 150);
            assert!(
                feed(&mut vad, room_tone, 40)
                    .iter()
                    .all(|a| *a == SpeechActivity::Silence),
                "room tone {room_tone} must read as silence once converged"
            );
            assert!(
                feed(&mut vad, room_tone * 12.0, 40)
                    .iter()
                    .all(|a| *a == SpeechActivity::Speech),
                "speech 12x above a {room_tone} floor must read as speech"
            );
        }
        Ok(())
    }

    /// Hysteresis must be a property of the ratios, not of a drifting floor, so
    /// each level is probed with a single window inside one block. A constant
    /// signal cannot be used here: the floor would track it.
    #[test]
    fn energy_vad_hysteresis_prevents_boundary_flapping() -> Result<(), EndpointError> {
        let mut vad = NativeEnergyVad::new(EnergyVadConfig::default())?;
        // Converge the floor on 0.0008 and stop on a block boundary.
        feed(&mut vad, 0.0008, 150);
        // Below the activation ratio (3.0): does not start speech.
        assert_eq!(feed(&mut vad, 0.002, 1), vec![SpeechActivity::Silence]);
        // Above the activation ratio: starts speech.
        assert_eq!(feed(&mut vad, 0.005, 1), vec![SpeechActivity::Speech]);
        // Between the ratios: holds speech that it would not have started.
        assert_eq!(feed(&mut vad, 0.002, 1), vec![SpeechActivity::Speech]);
        // Below the release ratio (1.8): drops speech.
        assert_eq!(feed(&mut vad, 0.0008, 1), vec![SpeechActivity::Silence]);
        Ok(())
    }

    /// Regression: the activity sequence must not depend on how the capture
    /// layer split the byte stream. Before fixed analysis windows, five of six
    /// plausible chunk sizes produced a different sequence from identical audio.
    #[test]
    fn energy_vad_decisions_are_invariant_to_chunk_partitioning() -> Result<(), EndpointError> {
        // Alternating speech and room-tone blocks with a clause-length pause.
        let mut samples: Vec<f32> = Vec::new();
        for block in 0..12 {
            let level = if block % 3 == 2 { 0.001 } else { 0.09 };
            samples.extend(std::iter::repeat_n(level, 8_000));
        }
        let total = samples.len();
        let mut reference: Option<Vec<SpeechActivity>> = None;
        for frames in [160_usize, 320, 640, 960, 1_280, 1_920, 2_560] {
            let mut vad = NativeEnergyVad::new(EnergyVadConfig::default())?;
            let mut sequence = Vec::new();
            for chunk_samples in samples.chunks(frames) {
                sequence.extend_from_slice(
                    vad.classify(&AudioChunk {
                        sample_rate_hz: 16_000,
                        channels: 1,
                        samples: chunk_samples.to_vec(),
                    })
                    .unwrap_or(&[]),
                );
            }
            match &reference {
                None => reference = Some(sequence),
                Some(expected) => assert_eq!(
                    &sequence, expected,
                    "chunk size {frames} changed the activity sequence"
                ),
            }
        }
        let sequence = reference.unwrap_or_default();
        assert!(sequence.len() * WINDOW_FRAMES <= total);
        assert!(
            sequence.contains(&SpeechActivity::Speech)
                && sequence.contains(&SpeechActivity::Silence),
            "fixture must contain both classes for this test to mean anything"
        );
        Ok(())
    }

    #[test]
    fn energy_vad_rejects_a_mid_stream_sample_rate_change() -> Result<(), EndpointError> {
        let mut vad = NativeEnergyVad::new(EnergyVadConfig::default())?;
        assert!(vad.classify(&chunk(0.05, WINDOW_FRAMES)).is_ok());
        assert_eq!(
            vad.classify(&AudioChunk {
                sample_rate_hz: 48_000,
                channels: 1,
                samples: vec![0.05; 960],
            })
            .err(),
            Some(EndpointError {
                code: "vad_sample_rate_changed"
            })
        );
        Ok(())
    }

    #[test]
    fn energy_vad_emits_one_decision_per_fixed_window() -> Result<(), EndpointError> {
        let mut vad = NativeEnergyVad::new(EnergyVadConfig::default())?;
        // A chunk shorter than a window yields no decision at all.
        assert!(vad.classify(&chunk(0.05, 100))?.is_empty());
        // Two windows' worth yields exactly two.
        assert_eq!(vad.classify(&chunk(0.05, 2 * WINDOW_FRAMES))?.len(), 2);
        assert_eq!(vad.window_frames(16_000)?, WINDOW_FRAMES as u64);
        Ok(())
    }

    #[test]
    fn endpoint_ignores_initial_silence_and_short_noise() -> Result<(), EndpointError> {
        let mut endpoint = DictationEndpointer::new(DictationEndpointConfig::default())?;
        for _ in 0..100 {
            assert_eq!(
                endpoint.observe(SpeechActivity::Silence, 1_280, 16_000),
                Ok(EndpointDecision::Continue)
            );
        }
        assert_eq!(
            endpoint.observe(SpeechActivity::Speech, 2_560, 16_000),
            Ok(EndpointDecision::Continue)
        );
        for _ in 0..20 {
            assert_eq!(
                endpoint.observe(SpeechActivity::Silence, 1_280, 16_000),
                Ok(EndpointDecision::Continue)
            );
        }
        assert_eq!(
            endpoint.observe(SpeechActivity::Speech, 2_560, 16_000),
            Ok(EndpointDecision::Continue)
        );
        for _ in 0..20 {
            assert_eq!(
                endpoint.observe(SpeechActivity::Silence, 1_280, 16_000),
                Ok(EndpointDecision::Continue)
            );
        }
        Ok(())
    }

    #[test]
    fn endpoint_finalizes_exactly_after_minimum_speech_and_hangover() -> Result<(), EndpointError> {
        let config = DictationEndpointConfig::default();
        let hangover_frames = milliseconds_to_frames(config.trailing_silence_ms, 16_000)?;
        let mut endpoint = DictationEndpointer::new(config)?;
        // Accumulate real speech past the minimum (240ms == 3840 frames).
        for _ in 0..3 {
            assert_eq!(
                endpoint.observe(SpeechActivity::Speech, 1_280, 16_000),
                Ok(EndpointDecision::Continue)
            );
        }
        // One frame short of the hang-over must not finalize.
        for _ in 0..(hangover_frames / 1_280) {
            assert_eq!(
                endpoint.observe(SpeechActivity::Silence, 1_280, 16_000),
                Ok(EndpointDecision::Continue)
            );
        }
        let remainder = hangover_frames % 1_280;
        if remainder != 0 {
            assert_eq!(
                endpoint.observe(SpeechActivity::Silence, remainder, 16_000),
                Ok(EndpointDecision::Finalize)
            );
        } else {
            assert_eq!(
                endpoint.observe(SpeechActivity::Silence, 1_280, 16_000),
                Ok(EndpointDecision::Finalize)
            );
        }
        assert_eq!(
            endpoint.observe(SpeechActivity::Silence, 1_280, 16_000),
            Err(EndpointError {
                code: "endpoint_already_finalized"
            })
        );
        Ok(())
    }

    /// Regression for the qualification blocker: a natural clause pause in
    /// continuous dictation must not end the utterance. The pinned fixture's
    /// own intra-utterance pauses run 1.08s and 1.14s, so a hang-over below
    /// that truncates speech at a sentence boundary and the transcriber cannot
    /// recover the remainder.
    #[test]
    fn endpoint_does_not_finalize_on_a_natural_clause_pause() -> Result<(), EndpointError> {
        let config = DictationEndpointConfig::default();
        let mut endpoint = DictationEndpointer::new(config)?;
        assert!(
            config.trailing_silence_ms >= 1_200,
            "hang-over must exceed a clause-length pause"
        );
        for _ in 0..3 {
            assert_eq!(
                endpoint.observe(SpeechActivity::Speech, 1_280, 16_000),
                Ok(EndpointDecision::Continue)
            );
        }
        // 1.14s of pause: the longest pause in the qualification fixture.
        for _ in 0..14 {
            assert_eq!(
                endpoint.observe(SpeechActivity::Silence, 1_280, 16_000),
                Ok(EndpointDecision::Continue),
                "a 1.12s clause pause must not end the utterance"
            );
        }
        assert_eq!(
            endpoint.observe(SpeechActivity::Speech, 1_280, 16_000),
            Ok(EndpointDecision::Continue)
        );
        Ok(())
    }

    #[test]
    fn resumed_speech_resets_only_the_trailing_silence() -> Result<(), EndpointError> {
        let config = DictationEndpointConfig::default();
        let hangover_frames = milliseconds_to_frames(config.trailing_silence_ms, 16_000)?;
        let mut endpoint = DictationEndpointer::new(config)?;
        for _ in 0..3 {
            assert_eq!(
                endpoint.observe(SpeechActivity::Speech, 1_280, 16_000),
                Ok(EndpointDecision::Continue)
            );
        }
        // A pause that stops short of the hang-over, then speech resumes.
        for _ in 0..(hangover_frames / 1_280) {
            assert_eq!(
                endpoint.observe(SpeechActivity::Silence, 1_280, 16_000),
                Ok(EndpointDecision::Continue)
            );
        }
        assert_eq!(
            endpoint.observe(SpeechActivity::Speech, 1_280, 16_000),
            Ok(EndpointDecision::Continue)
        );
        for _ in 0..(hangover_frames / 1_280) {
            assert_eq!(
                endpoint.observe(SpeechActivity::Silence, 1_280, 16_000),
                Ok(EndpointDecision::Continue)
            );
        }
        let remainder = hangover_frames % 1_280;
        let final_frames = if remainder == 0 { 1_280 } else { remainder };
        assert_eq!(
            endpoint.observe(SpeechActivity::Silence, final_frames, 16_000),
            Ok(EndpointDecision::Finalize)
        );
        Ok(())
    }

    #[test]
    fn endpoint_duration_is_invariant_to_chunk_partitioning() -> Result<(), EndpointError> {
        let config = DictationEndpointConfig::default();
        let hangover_frames = milliseconds_to_frames(config.trailing_silence_ms, 16_000)?;
        for frames in [640_u64, 1_280, 2_560] {
            let mut endpoint = DictationEndpointer::new(config)?;
            let mut speech_remaining = 3_840_u64;
            while speech_remaining != 0 {
                let current = frames.min(speech_remaining);
                assert_eq!(
                    endpoint.observe(SpeechActivity::Speech, current, 16_000),
                    Ok(EndpointDecision::Continue)
                );
                speech_remaining -= current;
            }
            let mut silence_remaining = hangover_frames;
            while silence_remaining != 0 {
                let current = frames.min(silence_remaining);
                silence_remaining -= current;
                assert_eq!(
                    endpoint.observe(SpeechActivity::Silence, current, 16_000),
                    Ok(if silence_remaining == 0 {
                        EndpointDecision::Finalize
                    } else {
                        EndpointDecision::Continue
                    })
                );
            }
        }
        Ok(())
    }
}
