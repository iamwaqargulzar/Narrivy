//! Opt-in test aid: keep each previewed dictation's audio with what the
//! preview showed and what each piece recognised, so recognition can be
//! compared on the user's real voice.
//!
//! Off unless `NARRIVY_DEBUG_KEEP_AUDIO=1` is in the daemon's environment.
//! Narrivy otherwise never stores audio. Files go to
//! `$XDG_STATE_HOME/narrivy/debug-audio/` (owner-only; on Windows the
//! per-user state folder's `debug-audio`) as `<unix ms>.wav`
//! (16 kHz mono 16-bit) and `<unix ms>.json`; nothing deletes them, and
//! nothing is ever logged.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const SAMPLE_RATE: u32 = 16_000;

/// What the preview showed and what the pieces recognised, by sample.
#[derive(Default)]
pub struct DebugLog {
    /// (samples captured when the window was taken, text shown).
    pub partials: Vec<(usize, String)>,
    /// (first sample, end sample, recognised text).
    pub pieces: Vec<(usize, usize, String)>,
}

/// True when the switch is on.
#[must_use]
pub fn enabled() -> bool {
    std::env::var_os("NARRIVY_DEBUG_KEEP_AUDIO").is_some_and(|value| value == "1")
}

#[cfg(windows)]
fn directory() -> Option<PathBuf> {
    super::host::state_dir()
        .ok()
        .map(|state| state.join("debug-audio"))
}

#[cfg(not(windows))]
fn directory() -> Option<PathBuf> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
        })?;
    Some(state.join("narrivy").join("debug-audio"))
}

/// Saves one dictation. Failures are ignored: this is a test aid and must
/// never affect the dictation.
pub fn save(samples: &[f32], log: &DebugLog) {
    let Some(directory) = directory() else {
        return;
    };
    if super::host::fs::dir_builder(0o700)
        .create(&directory)
        .is_err()
    {
        return;
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or_default();
    let _ = write_private(&directory.join(format!("{stamp}.wav")), &wav(samples));
    let seconds = |at: usize| at as f64 / f64::from(SAMPLE_RATE);
    let record = serde_json::json!({
        "seconds": seconds(samples.len()),
        "partials": log.partials.iter().map(|(at, text)| {
            serde_json::json!({ "at_seconds": seconds(*at), "text": text })
        }).collect::<Vec<_>>(),
        "pieces": log.pieces.iter().map(|(start, end, text)| {
            serde_json::json!({ "from_seconds": seconds(*start), "to_seconds": seconds(*end), "text": text })
        }).collect::<Vec<_>>(),
    });
    if let Ok(json) = serde_json::to_vec_pretty(&record) {
        let _ = write_private(&directory.join(format!("{stamp}.json")), &json);
    }
}

fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file =
        super::host::fs::create_mode(OpenOptions::new().write(true).create_new(true), 0o600)
            .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn wav(samples: &[f32]) -> Vec<u8> {
    let data_len = u32::try_from(samples.len() * 2).unwrap_or(u32::MAX - 36);
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16_u32.to_le_bytes());
    out.extend_from_slice(&1_u16.to_le_bytes());
    out.extend_from_slice(&1_u16.to_le_bytes());
    out.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    out.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    out.extend_from_slice(&2_u16.to_le_bytes());
    out.extend_from_slice(&16_u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        let value = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16;
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wav_has_a_44_byte_header_and_two_bytes_a_sample() {
        let bytes = wav(&[0.0, 0.5, -1.0]);
        assert_eq!(bytes.len(), 44 + 6);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[36..40], b"data");
        assert_eq!(i16::from_le_bytes([bytes[46], bytes[47]]), 16_383);
        assert_eq!(i16::from_le_bytes([bytes[48], bytes[49]]), -i16::MAX);
    }
}
