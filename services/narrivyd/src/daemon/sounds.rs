//! Start and stop sounds: a short rising tone when Narrivy starts listening,
//! a falling one when it stops. Off unless turned on (`sounds.json` in the
//! state directory).
//!
//! The tones are made here (no sound files shipped), written once to the
//! runtime directory and played with PipeWire's `pw-play`, never waited on.
//! The start tone plays while the microphone listens: the echo canceller
//! removes it from the microphone when that is on, and the voice filter
//! keeps only speech.

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

const RATE: u32 = 48_000;

#[derive(Debug, Clone, Copy)]
pub enum Cue {
    Start,
    Stop,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
struct Options {
    #[serde(default)]
    enabled: bool,
}

fn options_path() -> Option<PathBuf> {
    narrivy_platform_linux::instance::state_dir()
        .ok()
        .map(|dir| dir.join("sounds.json"))
}

#[must_use]
pub fn enabled() -> bool {
    options_path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|bytes| serde_json::from_slice::<Options>(&bytes).ok())
        .is_some_and(|o| o.enabled)
}

/// Turn the sounds on or off.
///
/// # Errors
/// `audio_options_unavailable` when the choice cannot be saved.
pub fn set_enabled(on: bool) -> Result<(), &'static str> {
    let path = options_path().ok_or("audio_options_unavailable")?;
    let bytes =
        serde_json::to_vec(&Options { enabled: on }).map_err(|_| "audio_options_unavailable")?;
    std::fs::write(path, bytes).map_err(|_| "audio_options_unavailable")
}

/// Two short notes with soft edges, as 16-bit mono samples.
fn tone(notes: [f32; 2]) -> Vec<i16> {
    let note_len = (RATE as usize) * 80 / 1000;
    let fade = (RATE as usize) * 10 / 1000;
    let mut samples = Vec::with_capacity(note_len * 2);
    for frequency in notes {
        for i in 0..note_len {
            #[allow(clippy::cast_precision_loss)]
            let t = i as f32 / RATE as f32;
            #[allow(clippy::cast_precision_loss)]
            let edge = (i.min(note_len - 1 - i) as f32 / fade as f32).min(1.0);
            let value = (std::f32::consts::TAU * frequency * t).sin() * 0.22 * edge;
            #[allow(clippy::cast_possible_truncation)]
            samples.push((value * f32::from(i16::MAX)) as i16);
        }
    }
    samples
}

fn wav(samples: &[i16]) -> Vec<u8> {
    let data_len = u32::try_from(samples.len() * 2).unwrap_or(0);
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

fn file(cue: Cue) -> Option<PathBuf> {
    let dir = narrivy_platform_linux::echo_cancel::config_dir();
    let (name, notes) = match cue {
        Cue::Start => ("start.wav", [660.0, 880.0]),
        Cue::Stop => ("stop.wav", [880.0, 660.0]),
    };
    let path = dir.join(name);
    if !path.is_file() {
        std::fs::create_dir_all(&dir).ok()?;
        std::fs::write(&path, wav(&tone(notes))).ok()?;
    }
    Some(path)
}

/// Play a cue when the sounds are on. Never waits.
pub fn play(cue: Cue) {
    if !enabled() {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("narrivy-sound".into())
        .spawn(move || {
            if let Some(path) = file(cue) {
                let _ = Command::new("pw-play")
                    .arg(path)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tones_are_short_quiet_wav_files() {
        let samples = tone([660.0, 880.0]);
        assert_eq!(samples.len(), 2 * 48_000 * 80 / 1000);
        assert!(samples.iter().all(|s| s.unsigned_abs() < 8_000));
        let bytes = wav(&samples);
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(bytes.len(), 44 + samples.len() * 2);
    }
}
