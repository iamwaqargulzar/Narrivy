//! Audio between the worker and the daemon, inside its JSON lines.
//!
//! The worker sends a turn to be recognised as
//! `{"ev":"audio","id":N,"kind":"rolling|early|final","pcm":"<base64>"}`,
//! where `pcm` is 16 kHz mono `f32` little-endian. The daemon answers
//! `{"op":"asr","id":N,"kind":...,"text":...,"ms":...}`. Neither side ever
//! writes this to disk or a log.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

/// Longest turn sent, in samples (30 s at 16 kHz, as voice detection caps
/// a turn).
pub const MAX_TURN_SAMPLES: usize = 30 * crate::RATE as usize + crate::RATE as usize;

/// What a piece of audio is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The words so far, while the user speaks.
    Rolling,
    /// At a pause: ready if the turn ends there.
    Early,
    /// The whole turn.
    Final,
}

impl Kind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rolling => "rolling",
            Self::Early => "early",
            Self::Final => "final",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "rolling" => Some(Self::Rolling),
            "early" => Some(Self::Early),
            "final" => Some(Self::Final),
            _ => None,
        }
    }
}

/// Samples as the `pcm` field.
#[must_use]
pub fn encode(samples: &[f32]) -> String {
    let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    STANDARD.encode(bytes)
}

/// The `pcm` field as samples: `None` when it is not well formed, too long,
/// or holds a value that is not a finite sample.
#[must_use]
pub fn decode(pcm: &str) -> Option<Vec<f32>> {
    if pcm.len() > MAX_TURN_SAMPLES * 4 / 3 * 4 + 8 {
        return None;
    }
    let bytes = STANDARD.decode(pcm).ok()?;
    if bytes.len() % 4 != 0 || bytes.len() / 4 > MAX_TURN_SAMPLES {
        return None;
    }
    let samples: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    samples
        .iter()
        .all(|s| s.is_finite() && s.abs() <= 8.0)
        .then_some(samples)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_survives_the_round_trip() {
        let samples = vec![0.0, 0.5, -1.0, 0.25];
        assert_eq!(decode(&encode(&samples)), Some(samples));
        assert_eq!(Kind::parse(Kind::Early.as_str()), Some(Kind::Early));
    }

    #[test]
    fn malformed_or_oversized_audio_is_refused() {
        assert_eq!(decode("not base64!"), None);
        assert_eq!(decode(&STANDARD.encode([0_u8; 3])), None);
        assert_eq!(decode(&encode(&[f32::NAN])), None);
        assert_eq!(decode(&encode(&vec![0.0; MAX_TURN_SAMPLES + 1])), None);
        assert!(decode(&encode(&vec![0.0; MAX_TURN_SAMPLES])).is_some());
    }
}
