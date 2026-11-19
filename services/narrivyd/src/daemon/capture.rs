//! Audio fixture helpers and source selection.

#![forbid(unsafe_code)]
#![allow(unused_imports)]

use std::path::{Path, PathBuf};

/// Session audio source (`NARRIVY_AUDIO_SOURCE`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioSourceKind {
    /// Deterministic 440 Hz sine at 16 kHz mono — reproducible evidence lane.
    SineFixture,
    /// Live PipeWire capture, valid only through the owned strict Linux
    /// recovery adapter. Non-strict pipelines reject this source.
    PipeWire,
    /// Deterministic WAV file at 16 kHz (PCM16) — real speech fixture lane.
    FileWav(PathBuf),
}

/// ~100 ms of digital silence at 16 kHz mono: content-free audio stand-in.
pub const SAMPLE_RATE_HZ: u32 = 16_000;

/// ~1 s deterministic 440 Hz sine at 16 kHz mono (amplitude 0.25): the
/// reproducible stand-in for real capture on the correctness lanes.
pub fn sine_fixture_samples() -> Vec<f32> {
    const RATE_HZ: f32 = 16_000.0;
    const FREQ_HZ: f32 = 440.0;
    const SECONDS: f32 = 1.0;
    const AMP: f32 = 0.25;
    let n = (RATE_HZ * SECONDS) as usize;
    (0..n)
        .map(|i| AMP * (2.0 * std::f32::consts::PI * FREQ_HZ * i as f32 / RATE_HZ).sin())
        .collect()
}

/// Minimal canonical WAV parser: RIFF/WAVE, PCM16, N channels -> mono f32.
pub fn parse_wav_mono(path: &Path) -> Result<(u32, Vec<f32>), String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".to_owned());
    }
    let mut fmt_rate: Option<u32> = None;
    let mut fmt_channels: Option<u16> = None;
    let mut data: Option<&[u8]> = None;
    let mut pos = 12usize;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes(
            bytes[pos + 4..pos + 8]
                .try_into()
                .map_err(|_| "bad chunk header".to_owned())?,
        ) as usize;
        let body_end = pos + 8 + size;
        if body_end > bytes.len() {
            return Err("truncated chunk".to_owned());
        }
        match id {
            b"fmt " => {
                let audio_format =
                    u16::from_le_bytes(bytes[pos + 8..pos + 10].try_into().map_err(|_| "fmt")?);
                if audio_format != 1 {
                    return Err(format!(
                        "unsupported wav format tag {audio_format} (want PCM)"
                    ));
                }
                fmt_channels = Some(u16::from_le_bytes(
                    bytes[pos + 10..pos + 12].try_into().map_err(|_| "fmt ch")?,
                ));
                fmt_rate = Some(u32::from_le_bytes(
                    bytes[pos + 12..pos + 16]
                        .try_into()
                        .map_err(|_| "fmt rate")?,
                ));
            }
            b"data" => data = Some(&bytes[pos + 8..body_end]),
            _ => {}
        }
        pos = body_end + (size & 1); // chunks are word-aligned
    }
    let rate = fmt_rate.ok_or("no fmt chunk")?;
    let channels = fmt_channels.unwrap_or(1).max(1);
    let data = data.ok_or("no data chunk")?;
    let frame_size = usize::from(channels) * 2;
    if data.len() < frame_size || data.len() % frame_size != 0 {
        return Err(format!(
            "data chunk {} not whole {}-byte frames",
            data.len(),
            frame_size
        ));
    }
    let mut mono = Vec::with_capacity(data.len() / frame_size);
    for frame in data.chunks_exact(frame_size) {
        let acc: i64 = frame
            .chunks_exact(2)
            .map(|b| -> Result<i64, String> {
                Ok(i64::from(i16::from_le_bytes(
                    b.try_into().map_err(|_| "bad pcm frame".to_owned())?,
                )))
            })
            .sum::<Result<i64, String>>()?;
        let channels_i64 = i64::from(channels);
        let avg = acc / channels_i64;
        mono.push(avg as f32 / 32_767.0);
    }
    Ok((rate, mono))
}

#[cfg(test)]
mod tests {
    #[test]
    fn fixture_module_contains_no_native_payload_writer() {
        let source = include_str!("capture.rs");
        let forbidden = [
            ["capture", "_pipewire"].concat(),
            ["capture", ".wav"].concat(),
            ["temp", "_dir"].concat(),
        ];
        for token in forbidden {
            assert!(
                !source.contains(&token),
                "removed native payload writer token returned"
            );
        }
    }
}
