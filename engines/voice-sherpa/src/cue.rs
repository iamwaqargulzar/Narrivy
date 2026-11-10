//! The short soft sounds that mark listening and "heard you".

/// Rate the cues are made at.
pub const CUE_RATE: u32 = 24_000;

/// `(frequency Hz, seconds)` notes at `level`, with 15 ms edges.
#[must_use]
pub fn tone(rate: u32, notes: &[(f32, f32)], level: f32) -> Vec<f32> {
    let mut out = Vec::new();
    for &(frequency, seconds) in notes {
        let count = (rate as f32 * seconds) as usize;
        let edge = (count / 4).min((rate as f32 * 0.015) as usize);
        for i in 0..count {
            let t = i as f32 / rate as f32;
            let mut sample = (2.0 * std::f32::consts::PI * frequency * t).sin() * level;
            if edge > 0 {
                if i < edge {
                    sample *= i as f32 / edge as f32;
                } else if i >= count - edge {
                    sample *= (count - 1 - i) as f32 / edge as f32;
                }
            }
            out.push(sample);
        }
    }
    out
}

/// Rising: "listening".
#[must_use]
pub fn listening() -> Vec<f32> {
    tone(CUE_RATE, &[(660.0, 0.07), (880.0, 0.09)], 0.12)
}

/// Falling, softer: "heard you".
#[must_use]
pub fn heard() -> Vec<f32> {
    tone(CUE_RATE, &[(880.0, 0.06), (660.0, 0.08)], 0.08)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cues_are_short_quiet_and_start_and_end_silent() {
        for cue in [listening(), heard()] {
            assert!(cue.len() < CUE_RATE as usize / 4);
            assert!(cue.iter().all(|s| s.abs() <= 0.12));
            assert!(cue[0].abs() < 1e-6);
            assert!(cue[cue.len() - 1].abs() < 0.01);
        }
    }
}
