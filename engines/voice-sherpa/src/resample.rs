//! Linear resampling and down-mixing between the worker's rates and a
//! device's. Good enough for speech and short cues.

/// `samples` at `from` Hz, as `to` Hz.
#[must_use]
pub fn linear(samples: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || samples.is_empty() || from == 0 || to == 0 {
        return samples.to_vec();
    }
    let count = (samples.len() as u64 * u64::from(to) / u64::from(from)) as usize;
    let step = f64::from(from) / f64::from(to);
    (0..count)
        .map(|i| {
            let position = i as f64 * step;
            let index = position as usize;
            let fraction = (position - index as f64) as f32;
            let a = samples[index.min(samples.len() - 1)];
            let b = samples[(index + 1).min(samples.len() - 1)];
            a + (b - a) * fraction
        })
        .collect()
}

/// Interleaved `channels` audio, averaged to one channel.
#[must_use]
pub fn mono(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    interleaved
        .chunks(channels)
        .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampling_keeps_duration_and_shape() {
        let one_second: Vec<f32> = (0..48_000).map(|i| (i as f32 / 48_000.0).sin()).collect();
        let down = linear(&one_second, 48_000, 16_000);
        assert_eq!(down.len(), 16_000);
        assert!((down[8_000] - one_second[24_000]).abs() < 1e-3);
        assert_eq!(linear(&down, 16_000, 16_000), down);
        assert!(linear(&[], 16_000, 48_000).is_empty());
    }

    #[test]
    fn stereo_is_averaged() {
        assert_eq!(mono(&[1.0, 0.0, 0.5, 0.5], 2), vec![0.5, 0.5]);
        assert_eq!(mono(&[0.25], 1), vec![0.25]);
    }
}
