//! Conversion of a capture endpoint's native frames into the dictation
//! capture contract: mono f32 at [`narrivy_types::CAPTURE_SAMPLE_RATE_HZ`].
//!
//! WASAPI is asked for that format directly (the audio engine's own
//! converter); when an endpoint refuses, the session falls back to the
//! engine's mix format (typically 48 kHz stereo f32) and this module does the
//! conversion: decode, average the channels, then resample with a windowed
//! sinc low-pass so content above the new Nyquist rate is removed instead of
//! folded back into the speech band.
//!
//! Everything here is pure computation over sample values. It never logs and
//! never retains audio beyond the few input samples the filter needs.

#![forbid(unsafe_code)]

use narrivy_types::CAPTURE_SAMPLE_RATE_HZ;

/// Sample container layouts a shared-mode endpoint may present.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    /// IEEE float, 32 bits.
    F32,
    /// Signed PCM, 16 bits.
    I16,
    /// Signed PCM, 24 bits packed in 3 bytes.
    I24,
    /// Signed PCM in a 32-bit container (24 or 32 valid bits, left-justified).
    I32,
}

impl SampleFormat {
    #[must_use]
    pub fn bytes(self) -> usize {
        match self {
            Self::I16 => 2,
            Self::I24 => 3,
            Self::F32 | Self::I32 => 4,
        }
    }
}

/// The native format of a capture stream, as negotiated with the endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputFormat {
    pub sample_rate_hz: u32,
    pub channels: u16,
    pub sample: SampleFormat,
    /// Bytes per frame (all channels). At least `channels * sample.bytes()`.
    pub block_align: usize,
}

/// Content-free reason a format cannot be converted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatError {
    /// Rate, channel count or frame size outside what this module accepts.
    Unsupported,
    /// A buffer did not hold the frames it claimed, or a sample was NaN/inf.
    InvalidData,
}

impl FormatError {
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::Unsupported => "capture_format_unsupported",
            Self::InvalidData => "capture_output_invalid",
        }
    }
}

/// `WAVEFORMATEX::wFormatTag` values.
pub const WAVE_FORMAT_PCM: u16 = 0x0001;
pub const WAVE_FORMAT_IEEE_FLOAT: u16 = 0x0003;
pub const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

/// The sub-format of a `WAVEFORMATEXTENSIBLE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaveSubFormat {
    Pcm,
    Float,
    Other,
}

/// The fields of a `WAVEFORMATEX` (and, when extensible, of its extension)
/// that decide the sample layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaveFormatFields {
    pub tag: u16,
    pub channels: u16,
    pub sample_rate_hz: u32,
    pub block_align: u16,
    pub bits_per_sample: u16,
    /// `(sub-format, valid bits per sample)` for `WAVE_FORMAT_EXTENSIBLE`.
    pub extensible: Option<(WaveSubFormat, u16)>,
}

impl InputFormat {
    /// The layout described by an endpoint's `WAVEFORMATEX`.
    ///
    /// # Errors
    /// [`FormatError::Unsupported`] for compressed or unusual layouts.
    pub fn from_wave(fields: WaveFormatFields) -> Result<Self, FormatError> {
        let float = match (fields.tag, fields.extensible) {
            (WAVE_FORMAT_IEEE_FLOAT, _) => true,
            (WAVE_FORMAT_PCM, _) => false,
            (WAVE_FORMAT_EXTENSIBLE, Some((WaveSubFormat::Float, _))) => true,
            (WAVE_FORMAT_EXTENSIBLE, Some((WaveSubFormat::Pcm, _))) => false,
            _ => return Err(FormatError::Unsupported),
        };
        let sample = match (float, fields.bits_per_sample) {
            (true, 32) => SampleFormat::F32,
            (false, 16) => SampleFormat::I16,
            (false, 24) => SampleFormat::I24,
            // 24 valid bits left-justified in 32 decode the same way.
            (false, 32) => SampleFormat::I32,
            _ => return Err(FormatError::Unsupported),
        };
        let format = Self {
            sample_rate_hz: fields.sample_rate_hz,
            channels: fields.channels,
            sample,
            block_align: usize::from(fields.block_align),
        };
        // Validate through the converter's own rules.
        MonoConverter::new(format).map(|_| format)
    }
}

/// Lowest and highest endpoint rates accepted. Outside this range a device is
/// almost certainly misreporting its format.
const MIN_RATE_HZ: u32 = 8_000;
const MAX_RATE_HZ: u32 = 384_000;
const MAX_CHANNELS: u16 = 32;

/// Streaming converter from one [`InputFormat`] to the capture contract.
pub struct MonoConverter {
    input: InputFormat,
    resampler: Option<Resampler>,
    mono: Vec<f32>,
}

impl MonoConverter {
    /// # Errors
    /// [`FormatError::Unsupported`] for a rate, channel count or frame size
    /// this module does not accept.
    pub fn new(input: InputFormat) -> Result<Self, FormatError> {
        if !(MIN_RATE_HZ..=MAX_RATE_HZ).contains(&input.sample_rate_hz)
            || input.channels == 0
            || input.channels > MAX_CHANNELS
            || input.block_align < usize::from(input.channels) * input.sample.bytes()
        {
            return Err(FormatError::Unsupported);
        }
        let resampler = (input.sample_rate_hz != CAPTURE_SAMPLE_RATE_HZ)
            .then(|| Resampler::new(input.sample_rate_hz, CAPTURE_SAMPLE_RATE_HZ));
        Ok(Self {
            input,
            resampler,
            mono: Vec::new(),
        })
    }

    #[must_use]
    pub fn input(&self) -> InputFormat {
        self.input
    }

    /// Convert `frames` native frames from `bytes`, appending contract
    /// samples to `out`.
    ///
    /// # Errors
    /// [`FormatError::InvalidData`] when `bytes` is shorter than `frames`
    /// frames or a float sample is not finite. Nothing is appended then.
    pub fn push(
        &mut self,
        bytes: &[u8],
        frames: usize,
        out: &mut Vec<f32>,
    ) -> Result<(), FormatError> {
        let needed = frames
            .checked_mul(self.input.block_align)
            .ok_or(FormatError::InvalidData)?;
        let Some(bytes) = bytes.get(..needed) else {
            return Err(FormatError::InvalidData);
        };
        self.mono.clear();
        self.mono.reserve(frames);
        let channels = usize::from(self.input.channels);
        let width = self.input.sample.bytes();
        let scale = 1.0 / f32::from(self.input.channels);
        for frame in bytes.chunks_exact(self.input.block_align) {
            let mut sum = 0.0_f32;
            for channel in 0..channels {
                let at = channel * width;
                sum += decode(self.input.sample, &frame[at..at + width])?;
            }
            self.mono.push(sum * scale);
        }
        self.emit(out);
        Ok(())
    }

    /// Append `frames` frames of digital silence (a WASAPI packet flagged
    /// silent carries no meaningful bytes).
    pub fn push_silence(&mut self, frames: usize, out: &mut Vec<f32>) {
        self.mono.clear();
        self.mono.resize(frames, 0.0);
        self.emit(out);
    }

    /// Emit the filter's tail at the end of a stream so the last few
    /// milliseconds of speech are not held back.
    pub fn flush(&mut self, out: &mut Vec<f32>) {
        if let Some(resampler) = self.resampler.as_mut() {
            resampler.flush(out);
        }
    }

    fn emit(&mut self, out: &mut Vec<f32>) {
        match self.resampler.as_mut() {
            Some(resampler) => resampler.push(&self.mono, out),
            None => out.extend_from_slice(&self.mono),
        }
    }
}

fn decode(format: SampleFormat, bytes: &[u8]) -> Result<f32, FormatError> {
    let value = match (format, bytes) {
        (SampleFormat::F32, [a, b, c, d]) => {
            let value = f32::from_le_bytes([*a, *b, *c, *d]);
            if !value.is_finite() {
                return Err(FormatError::InvalidData);
            }
            value
        }
        (SampleFormat::I16, [a, b]) => f32::from(i16::from_le_bytes([*a, *b])) / 32_768.0,
        (SampleFormat::I24, [a, b, c]) => {
            // Sign-extend by placing the 24 bits in the top of an i32.
            let value = i32::from_le_bytes([0, *a, *b, *c]) >> 8;
            value as f32 / 8_388_608.0
        }
        (SampleFormat::I32, [a, b, c, d]) => {
            i32::from_le_bytes([*a, *b, *c, *d]) as f32 / 2_147_483_648.0
        }
        _ => return Err(FormatError::InvalidData),
    };
    Ok(value)
}

/// Taps on each side of the interpolation point at the output rate's
/// bandwidth. 16 keeps the stop band well below speech-relevant levels while
/// costing about 32 multiply-adds per output sample.
const HALF_TAPS: usize = 16;

/// Streaming band-limited sample-rate converter (windowed sinc, Blackman).
///
/// Each output sample at input position `t` is the weighted sum of the
/// `2 * span` input samples around `t`. The weights are normalised to sum to
/// one, so a constant input gives exactly that constant out.
pub struct Resampler {
    /// Input samples per output sample.
    step: f64,
    /// Low-pass cutoff relative to the input Nyquist rate (≤ 1).
    cutoff: f64,
    /// Input samples on each side of the interpolation point.
    span: usize,
    /// Buffered input; `buffer[0]` is absolute input index `base`.
    buffer: Vec<f32>,
    base: u64,
    /// Absolute input position of the next output sample.
    next: f64,
    weights: Vec<f64>,
}

impl Resampler {
    #[must_use]
    pub fn new(input_rate_hz: u32, output_rate_hz: u32) -> Self {
        let step = f64::from(input_rate_hz) / f64::from(output_rate_hz);
        let cutoff = (1.0 / step).min(1.0);
        // Widen the window when band-limiting so the transition band keeps
        // the same width in output samples.
        let span = ((HALF_TAPS as f64) / cutoff).ceil() as usize;
        Self {
            step,
            cutoff,
            span,
            // Leading zeros so the first output sample has full history.
            buffer: vec![0.0; span],
            base: 0,
            next: span as f64,
            weights: Vec::with_capacity(2 * span),
        }
    }

    pub fn push(&mut self, input: &[f32], out: &mut Vec<f32>) {
        self.buffer.extend_from_slice(input);
        self.drain(out);
    }

    /// End of stream: feed enough silence to emit every output whose
    /// position lies inside the real input. Call once, last.
    pub fn flush(&mut self, out: &mut Vec<f32>) {
        let end = (self.base + self.buffer.len() as u64) as f64;
        self.buffer.resize(self.buffer.len() + self.span + 1, 0.0);
        while self.next < end && self.emit_one(out) {}
        // Nothing buffered is real input any more; a second flush emits
        // nothing.
        self.base += self.buffer.len() as u64;
        self.buffer.clear();
        self.next = self.next.max(self.base as f64);
    }

    fn drain(&mut self, out: &mut Vec<f32>) {
        while self.emit_one(out) {}
        // Keep only what the next output still needs.
        let keep_from = (self.next.floor() as u64).saturating_sub(self.span as u64);
        if keep_from > self.base {
            let drop = usize::try_from(keep_from - self.base)
                .unwrap_or(self.buffer.len())
                .min(self.buffer.len());
            self.buffer.drain(..drop);
            self.base += drop as u64;
        }
    }

    /// Emit one output sample if its whole window is buffered.
    fn emit_one(&mut self, out: &mut Vec<f32>) -> bool {
        let centre = self.next.floor() as u64;
        let first = centre + 1 - self.span as u64;
        let last = centre + self.span as u64;
        let end = self.base + self.buffer.len() as u64;
        if first < self.base || last >= end {
            return false;
        }
        let frac = self.next - centre as f64;
        self.weights.clear();
        let mut total = 0.0;
        for k in 0..2 * self.span {
            // Distance from the interpolation point to tap `first + k`.
            let distance = (k as f64 + 1.0 - self.span as f64) - frac;
            let weight = kernel(distance, self.cutoff, self.span as f64);
            self.weights.push(weight);
            total += weight;
        }
        let offset = usize::try_from(first - self.base).unwrap_or(0);
        let mut sum = 0.0;
        for (k, weight) in self.weights.iter().enumerate() {
            sum += f64::from(self.buffer[offset + k]) * weight;
        }
        out.push(if total.abs() > f64::EPSILON {
            (sum / total) as f32
        } else {
            0.0
        });
        self.next += self.step;
        true
    }
}

fn kernel(distance: f64, cutoff: f64, span: f64) -> f64 {
    let x = distance * cutoff;
    let sinc = if x.abs() < 1e-12 {
        1.0
    } else {
        (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
    };
    // Blackman window over [-span, span].
    let n = (distance + span) / (2.0 * span);
    if !(0.0..=1.0).contains(&n) {
        return 0.0;
    }
    let tau = 2.0 * std::f64::consts::PI;
    let window = 0.42 - 0.5 * (tau * n).cos() + 0.08 * (2.0 * tau * n).cos();
    cutoff * sinc * window
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f32_bytes(samples: &[f32]) -> Vec<u8> {
        samples.iter().flat_map(|s| s.to_le_bytes()).collect()
    }

    fn converter(rate: u32, channels: u16, sample: SampleFormat) -> MonoConverter {
        let block_align = usize::from(channels) * sample.bytes();
        match MonoConverter::new(InputFormat {
            sample_rate_hz: rate,
            channels,
            sample,
            block_align,
        }) {
            Ok(converter) => converter,
            Err(error) => unreachable!("format rejected: {error:?}"),
        }
    }

    #[test]
    fn contract_format_passes_through_unchanged() {
        let mut conv = converter(16_000, 1, SampleFormat::F32);
        let input = [0.0, 0.25, -0.5, 1.0];
        let mut out = Vec::new();
        assert_eq!(conv.push(&f32_bytes(&input), 4, &mut out), Ok(()));
        conv.flush(&mut out);
        assert_eq!(out, input);
    }

    #[test]
    fn stereo_i16_is_averaged_to_mono() {
        let mut conv = converter(16_000, 2, SampleFormat::I16);
        let frames: [[i16; 2]; 2] = [[16_384, -16_384], [16_384, 16_384]];
        let bytes: Vec<u8> = frames
            .iter()
            .flatten()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        let mut out = Vec::new();
        assert_eq!(conv.push(&bytes, 2, &mut out), Ok(()));
        assert_eq!(out, [0.0, 0.5]);
    }

    #[test]
    fn packed_24_bit_samples_are_sign_extended() {
        let mut conv = converter(16_000, 1, SampleFormat::I24);
        // -1.0 (0x800000) and +0.5 (0x400000), little endian.
        let bytes = [0x00, 0x00, 0x80, 0x00, 0x00, 0x40];
        let mut out = Vec::new();
        assert_eq!(conv.push(&bytes, 2, &mut out), Ok(()));
        assert_eq!(out, [-1.0, 0.5]);
    }

    #[test]
    fn padded_frames_skip_the_padding() {
        // Two 16-bit channels in an 8-byte frame (as some drivers report).
        let mut conv = match MonoConverter::new(InputFormat {
            sample_rate_hz: 16_000,
            channels: 2,
            sample: SampleFormat::I16,
            block_align: 8,
        }) {
            Ok(conv) => conv,
            Err(error) => unreachable!("{error:?}"),
        };
        let bytes = [0x00, 0x40, 0x00, 0x40, 0xFF, 0xFF, 0xFF, 0xFF];
        let mut out = Vec::new();
        assert_eq!(conv.push(&bytes, 1, &mut out), Ok(()));
        assert_eq!(out, [0.5]);
    }

    #[test]
    fn short_buffers_and_non_finite_floats_are_rejected() {
        let mut conv = converter(16_000, 1, SampleFormat::F32);
        let mut out = Vec::new();
        assert_eq!(
            conv.push(&[0, 0, 0], 1, &mut out),
            Err(FormatError::InvalidData)
        );
        assert_eq!(
            conv.push(&f32_bytes(&[f32::NAN]), 1, &mut out),
            Err(FormatError::InvalidData)
        );
        assert!(out.is_empty());
    }

    #[test]
    fn wave_formats_map_to_layouts() {
        let mix = WaveFormatFields {
            tag: WAVE_FORMAT_EXTENSIBLE,
            channels: 2,
            sample_rate_hz: 48_000,
            block_align: 8,
            bits_per_sample: 32,
            extensible: Some((WaveSubFormat::Float, 32)),
        };
        assert_eq!(
            InputFormat::from_wave(mix).map(|f| f.sample),
            Ok(SampleFormat::F32)
        );
        let pcm24in32 = WaveFormatFields {
            extensible: Some((WaveSubFormat::Pcm, 24)),
            ..mix
        };
        assert_eq!(
            InputFormat::from_wave(pcm24in32).map(|f| f.sample),
            Ok(SampleFormat::I32)
        );
        let plain16 = WaveFormatFields {
            tag: WAVE_FORMAT_PCM,
            block_align: 4,
            bits_per_sample: 16,
            extensible: None,
            ..mix
        };
        assert_eq!(
            InputFormat::from_wave(plain16).map(|f| f.sample),
            Ok(SampleFormat::I16)
        );
        for refused in [
            WaveFormatFields {
                extensible: Some((WaveSubFormat::Other, 32)),
                ..mix
            },
            WaveFormatFields {
                extensible: None,
                ..mix
            },
            WaveFormatFields {
                tag: 0x0055,
                ..plain16
            },
            WaveFormatFields {
                bits_per_sample: 8,
                ..plain16
            },
            WaveFormatFields {
                block_align: 2,
                ..plain16
            },
        ] {
            assert_eq!(
                InputFormat::from_wave(refused),
                Err(FormatError::Unsupported)
            );
        }
    }

    #[test]
    fn implausible_formats_are_refused() {
        for (rate, channels, block_align) in [(4_000, 1, 4), (16_000, 0, 4), (16_000, 2, 4)] {
            assert_eq!(
                MonoConverter::new(InputFormat {
                    sample_rate_hz: rate,
                    channels,
                    sample: SampleFormat::F32,
                    block_align,
                })
                .err(),
                Some(FormatError::Unsupported)
            );
        }
    }

    fn resample_all(rate: u32, input: &[f32], piece: usize) -> Vec<f32> {
        let mut conv = converter(rate, 1, SampleFormat::F32);
        let mut out = Vec::new();
        for block in input.chunks(piece) {
            assert_eq!(conv.push(&f32_bytes(block), block.len(), &mut out), Ok(()));
        }
        conv.flush(&mut out);
        out
    }

    #[test]
    fn resampling_keeps_duration() {
        for rate in [8_000, 22_050, 44_100, 48_000, 96_000] {
            let seconds = 2;
            let input = vec![0.1_f32; rate as usize * seconds];
            let out = resample_all(rate, &input, 441);
            let expected = 16_000 * seconds;
            let difference = out.len().abs_diff(expected);
            assert!(difference <= 2, "{rate} Hz -> {} samples", out.len());
        }
    }

    #[test]
    fn resampling_keeps_a_constant_level() {
        let input = vec![0.25_f32; 48_000];
        let out = resample_all(48_000, &input, 480);
        // Away from the zero-padded start and the flushed end.
        for value in &out[100..out.len() - 100] {
            assert!((value - 0.25).abs() < 1e-4, "{value}");
        }
    }

    #[test]
    fn block_size_does_not_change_the_result() {
        let input: Vec<f32> = (0..9_600)
            .map(|i| ((i as f32) * 0.05).sin() * 0.5)
            .collect();
        let whole = resample_all(48_000, &input, input.len());
        let pieces = resample_all(48_000, &input, 137);
        assert_eq!(whole.len(), pieces.len());
        for (a, b) in whole.iter().zip(&pieces) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
    }

    fn tone(rate: u32, hz: f32, seconds: f32) -> Vec<f32> {
        let count = (rate as f32 * seconds) as usize;
        (0..count)
            .map(|i| (2.0 * std::f32::consts::PI * hz * i as f32 / rate as f32).sin() * 0.5)
            .collect()
    }

    #[test]
    fn speech_band_passes_and_content_above_nyquist_is_removed() {
        let speech = resample_all(48_000, &tone(48_000, 1_000.0, 1.0), 480);
        let speech_rms = rms(&speech[200..speech.len() - 200]);
        assert!(
            (speech_rms - 0.5 / 2.0_f32.sqrt()).abs() < 0.01,
            "{speech_rms}"
        );

        // 12 kHz would alias to 4 kHz at 16 kHz without the low-pass.
        let alias = resample_all(48_000, &tone(48_000, 12_000.0, 1.0), 480);
        let alias_rms = rms(&alias[200..alias.len() - 200]);
        assert!(alias_rms < 0.005, "{alias_rms}");
    }
}
