//! Pure audio preconditioning for the transcribe.cpp lane.
//!
//! The hypothesized upstream C API consumes 16 kHz mono f32 audio; Narrivy
//! captures arbitrary `AudioChunk`s. This module is always compiled and does
//! no allocation-heavy work beyond the required buffers. It is pure: no FFI,
//! no I/O, no panics.

use narrivy_asr_api::AsrError;
use narrivy_types::AudioChunk;

/// Target sample rate of the transcribe.cpp decode path.
pub const TARGET_SAMPLE_RATE_HZ: u32 = 16_000;

/// Converts any captured [`AudioChunk`] into 16 kHz mono i16 PCM.
///
/// Pipeline: strict frame validation -> per-frame channel average (stereo ->
/// mono) -> linear resample to [`TARGET_SAMPLE_RATE_HZ`] -> clamp to [-1, 1]
/// with NaN mapped to silence -> scale to i16 (symmetric ±32767).
///
/// Errors (typed, no panics):
/// - `empty_audio`: chunk carries zero samples.
/// - `invalid_chunk`: `channels == 0`, `sample_rate_hz == 0`, or the sample
///   count is not a whole number of frames.
pub fn ensure_mono_16k(chunk: &AudioChunk) -> Result<Vec<i16>, AsrError> {
    if chunk.samples.is_empty() {
        return Err(AsrError::new("empty_audio", "audio chunk has no samples"));
    }
    if chunk.channels == 0 {
        return Err(AsrError::new(
            "invalid_chunk",
            "channel count must be at least 1",
        ));
    }
    if chunk.sample_rate_hz == 0 {
        return Err(AsrError::new(
            "invalid_chunk",
            "sample rate must be non-zero",
        ));
    }
    let channels = usize::from(chunk.channels);
    if chunk.samples.len() % channels != 0 {
        return Err(AsrError::new(
            "invalid_chunk",
            format!(
                "sample count {} is not a whole number of {}-channel frames",
                chunk.samples.len(),
                channels
            ),
        ));
    }

    // Stereo/multichannel -> mono by per-frame arithmetic mean.
    let frames = chunk.samples.len() / channels;
    let mono: Vec<f32> = if channels == 1 {
        chunk.samples.clone()
    } else {
        chunk
            .samples
            .chunks_exact(channels)
            .map(|frame| frame.iter().sum::<f32>() / channels as f32)
            .collect()
    };
    debug_assert_eq!(mono.len(), frames);

    let resampled = if chunk.sample_rate_hz == TARGET_SAMPLE_RATE_HZ {
        mono
    } else {
        linear_resample(&mono, chunk.sample_rate_hz, TARGET_SAMPLE_RATE_HZ)
    };

    Ok(resampled.iter().map(|sample| f32_to_i16(*sample)).collect())
}

/// Floor-length linear resampler: output length is `(frames * dst) / src`
/// (integer division). Interior samples interpolate between their two nearest
/// input neighbors; single-sample inputs replicate.
fn linear_resample(input: &[f32], source_hz: u32, target_hz: u32) -> Vec<f32> {
    let frames = input.len();
    if frames == 0 {
        return Vec::new();
    }
    let out_len = (frames as u64 * u64::from(target_hz) / u64::from(source_hz)) as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src_pos = i as f64 * f64::from(source_hz) / f64::from(target_hz);
        let i0 = (src_pos.floor() as usize).min(frames - 1);
        let i1 = (i0 + 1).min(frames - 1);
        let frac = (src_pos - i0 as f64) as f32;
        out.push(input[i0] * (1.0 - frac) + input[i1] * frac);
    }
    out
}

/// Clamp + scale to symmetric i16; NaN becomes digital silence.
fn f32_to_i16(sample: f32) -> i16 {
    if sample.is_nan() {
        return 0;
    }
    let clamped = sample.clamp(-1.0, 1.0) * 32_767.0;
    clamped.round() as i16
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn chunk(rate: u32, channels: u16, samples: Vec<f32>) -> AudioChunk {
        AudioChunk {
            sample_rate_hz: rate,
            channels,
            samples,
        }
    }

    #[test]
    fn empty_audio_is_a_typed_error() {
        let err = ensure_mono_16k(&chunk(16_000, 1, vec![])).unwrap_err();
        assert_eq!(err.code, "empty_audio");
    }

    #[test]
    fn invalid_chunk_shapes_error() {
        assert_eq!(
            ensure_mono_16k(&chunk(16_000, 0, vec![0.0]))
                .unwrap_err()
                .code,
            "invalid_chunk"
        );
        assert_eq!(
            ensure_mono_16k(&chunk(0, 1, vec![0.0])).unwrap_err().code,
            "invalid_chunk"
        );
        // Two channels but an odd sample count: not whole frames.
        assert_eq!(
            ensure_mono_16k(&chunk(16_000, 2, vec![0.5, 0.5, 0.5]))
                .unwrap_err()
                .code,
            "invalid_chunk"
        );
    }

    #[test]
    fn resample_48k_to_16k_length_math() {
        let one_second = vec![0.25_f32; 48_000];
        let out = ensure_mono_16k(&chunk(48_000, 1, one_second)).unwrap();
        assert_eq!(out.len(), 16_000);

        // Non-divisible ratio truncates by floor: 960 @ 48k -> 320 @ 16k.
        let out = ensure_mono_16k(&chunk(48_000, 1, vec![0.0; 960])).unwrap();
        assert_eq!(out.len(), 320);
    }

    #[test]
    fn stereo_frames_are_averaged_to_mono() {
        // Frame 0 averages to silence, frame 1 stays at half amplitude.
        let out = ensure_mono_16k(&chunk(16_000, 2, vec![1.0, -1.0, 0.5, 0.5])).unwrap();
        assert_eq!(out, vec![0, 16_384]);
    }

    #[test]
    fn out_of_range_and_nan_samples_are_clamped() {
        let out = ensure_mono_16k(&chunk(16_000, 1, vec![2.0, f32::NAN, -7.0])).unwrap();
        assert_eq!(out, vec![32_767, 0, -32_767]);
    }

    #[test]
    fn passthrough_keeps_length_at_target_rate() {
        let out = ensure_mono_16k(&chunk(16_000, 1, vec![0.5; 160])).unwrap();
        assert_eq!(out.len(), 160);
    }
}
