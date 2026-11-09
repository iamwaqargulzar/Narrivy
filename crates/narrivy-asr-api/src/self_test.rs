//! The worker's accelerator self-test: pure checks, no native code.
//!
//! Before a worker serves dictation from an accelerator (a GPU or an NPU) it
//! decodes a known recording there. The GPU is used only when the text contains the known phrase
//! and the decode was faster than real time; otherwise the worker loads the
//! model on the CPU. The same decode warms the device up, so the first real
//! dictation does not pay for shader compilation.

use std::time::Duration;

/// Words every correct decode of the fixture contains.
pub const KNOWN_PHRASE: &str = "ask not what your country can do for you";
/// The fixture's sample rate. The worker checks the model wants the same.
pub const SAMPLE_RATE_HZ: u32 = 16_000;
/// Decode a 16-bit mono PCM WAV at [`SAMPLE_RATE_HZ`] into samples. Returns
/// `None` for anything else, which fails the self-test.
#[must_use]
pub fn fixture_samples(wav: &[u8]) -> Option<Vec<f32>> {
    let u16_at =
        |bytes: &[u8], at: usize| Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?));
    let u32_at =
        |bytes: &[u8], at: usize| Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?));
    if wav.get(..4)? != b"RIFF" || wav.get(8..12)? != b"WAVE" {
        return None;
    }
    let mut format_checked = false;
    let mut offset = 12_usize;
    loop {
        let id = wav.get(offset..offset + 4)?;
        let length = usize::try_from(u32_at(wav, offset + 4)?).ok()?;
        let body = wav.get(offset + 8..(offset + 8).checked_add(length)?)?;
        match id {
            b"fmt " => {
                if u16_at(body, 0)? != 1
                    || u16_at(body, 2)? != 1
                    || u32_at(body, 4)? != SAMPLE_RATE_HZ
                    || u16_at(body, 14)? != 16
                {
                    return None;
                }
                format_checked = true;
            }
            b"data" => {
                if !format_checked || body.is_empty() || body.len() % 2 != 0 {
                    return None;
                }
                return Some(
                    body.chunks_exact(2)
                        .map(|pair| f32::from(i16::from_le_bytes([pair[0], pair[1]])) / 32_768.0)
                        .collect(),
                );
            }
            _ => {}
        }
        // Chunks are padded to an even length.
        offset = (offset + 8).checked_add(length + (length & 1))?;
    }
}

/// Whether `text` contains the known phrase, ignoring case and punctuation.
#[must_use]
pub fn phrase_matches(text: &str) -> bool {
    let words: Vec<String> = text
        .split(|character: char| !character.is_alphanumeric() && character != '\'')
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect();
    words.join(" ").contains(KNOWN_PHRASE)
}

/// Whether a decode of `sample_count` samples kept up with real time.
#[must_use]
pub fn faster_than_real_time(elapsed: Duration, sample_count: usize) -> bool {
    let audio = Duration::from_secs_f64(sample_count as f64 / f64::from(SAMPLE_RATE_HZ));
    elapsed < audio
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &[u8] = include_bytes!("../../../tests/fixtures/jfk16.wav");

    #[test]
    fn repository_fixture_decodes_to_eleven_seconds_of_samples() {
        let samples = fixture_samples(FIXTURE).unwrap_or_default();
        assert_eq!(samples.len(), 176_000);
        assert!(samples.iter().all(|sample| (-1.0..1.0).contains(sample)));
    }

    #[test]
    fn anything_but_sixteen_bit_mono_at_the_fixture_rate_is_refused() {
        assert_eq!(fixture_samples(&[]), None);
        assert_eq!(fixture_samples(&FIXTURE[..44]), None);
        // The format chunk starts at byte 12: tag, length, then the fields.
        let mut stereo = FIXTURE.to_vec();
        stereo[22] = 2;
        assert_eq!(fixture_samples(&stereo), None);
        let mut other_rate = FIXTURE.to_vec();
        other_rate[24..28].copy_from_slice(&44_100_u32.to_le_bytes());
        assert_eq!(fixture_samples(&other_rate), None);
        // A truncated data chunk is not decoded from what is left.
        assert_eq!(fixture_samples(&FIXTURE[..FIXTURE.len() - 2]), None);
    }

    #[test]
    fn phrase_check_ignores_case_and_punctuation_only() {
        assert!(phrase_matches(
            " And so, my fellow Americans: ask not what your country can do for you — ask what you can do for your country."
        ));
        assert!(phrase_matches("ASK NOT WHAT YOUR COUNTRY CAN DO FOR YOU"));
        assert!(!phrase_matches("ask not what your country can do"));
        assert!(!phrase_matches("ask what your country can do for you"));
        assert!(!phrase_matches(""));
    }

    #[test]
    fn real_time_budget_is_the_audio_length() {
        assert!(faster_than_real_time(Duration::from_secs(10), 176_000));
        assert!(!faster_than_real_time(Duration::from_secs(11), 176_000));
        assert!(!faster_than_real_time(Duration::from_secs(12), 176_000));
    }
}
