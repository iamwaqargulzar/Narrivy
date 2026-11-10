//! Speak a sentence to a WAV file with a Kokoro pack, for checking.
//! `speak_probe <pack dir> <text> <out.wav> [speaker]`

use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let [_, pack, text, out, rest @ ..] = args.as_slice() else {
        return Err("usage: speak_probe <pack dir> <text> <out.wav> [speaker]".into());
    };
    let speaker = rest.first().and_then(|s| s.parse().ok()).unwrap_or(3);
    let voice = narrivy_voice_sherpa::speech::Voice::kokoro(std::path::Path::new(pack), 4)?;
    let started = std::time::Instant::now();
    let (samples, rate) = voice.speak(text, speaker, 1.0).ok_or("no audio")?;
    eprintln!(
        "{} samples at {rate} Hz in {:?}",
        samples.len(),
        started.elapsed()
    );
    let mut file = std::fs::File::create(out)?;
    let data = (samples.len() * 2) as u32;
    file.write_all(b"RIFF")?;
    file.write_all(&(36 + data).to_le_bytes())?;
    file.write_all(b"WAVEfmt ")?;
    file.write_all(&16_u32.to_le_bytes())?;
    file.write_all(&1_u16.to_le_bytes())?;
    file.write_all(&1_u16.to_le_bytes())?;
    file.write_all(&rate.to_le_bytes())?;
    file.write_all(&(rate * 2).to_le_bytes())?;
    file.write_all(&2_u16.to_le_bytes())?;
    file.write_all(&16_u16.to_le_bytes())?;
    file.write_all(b"data")?;
    file.write_all(&data.to_le_bytes())?;
    for sample in samples {
        file.write_all(&((sample.clamp(-1.0, 1.0) * 32_767.0) as i16).to_le_bytes())?;
    }
    Ok(())
}
