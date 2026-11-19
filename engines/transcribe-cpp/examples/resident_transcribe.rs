//! Development harness: start the resident engine exactly as the daemon does
//! (digest-bound worker snapshot, digest-bound model snapshot, contained
//! worker process, private pipes) and time decodes of a 16 kHz mono 16-bit
//! WAV through it.
//!
//! The worker binding comes from `NARRIVY_TRANSCRIBE_WORKER`,
//! `NARRIVY_TRANSCRIBE_WORKER_SHA256` and `NARRIVY_TRANSCRIBE_WORKER_SIZE`;
//! the model's digest and size are computed here.
//!
//! `cargo run --release -p narrivy-asr-transcribe --features resident-worker
//! --example resident_transcribe -- <model.gguf> <audio.wav> [runs]`
//!
//! Prints timings and whether the text holds the repository fixture's known
//! phrase, never the transcript.

use std::io::Read;
use std::time::Instant;

use narrivy_asr_api::OneShotAsrPort;
use narrivy_asr_transcribe::resident::ResidentWorkerEngine;
use narrivy_types::{AudioChunk, SessionCancellation, SessionId};
use sha2::{Digest, Sha256};

/// A phrase from `tests/fixtures/jfk16.wav`.
const KNOWN_PHRASE: &str = "ask not what your country can do for you";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let (Some(model_path), Some(wav_path)) = (args.next(), args.next()) else {
        return Err("usage: resident_transcribe <model.gguf> <audio.wav> [runs]".into());
    };
    let runs: u32 = args.next().map_or(Ok(3), |value| value.parse())?;

    let (sha256, size) = digest_file(&model_path)?;
    let bytes = std::fs::read(wav_path)?;
    // Canonical 44-byte header, 16-bit PCM: enough for the repository fixture.
    let samples: Vec<f32> = bytes
        .get(44..)
        .ok_or("wav too short")?
        .chunks_exact(2)
        .map(|pair| f32::from(i16::from_le_bytes([pair[0], pair[1]])) / 32_768.0)
        .collect();
    let audio_seconds = samples.len() as f64 / 16_000.0;
    let audio = AudioChunk {
        sample_rate_hz: 16_000,
        channels: 1,
        samples,
    };

    let started = Instant::now();
    let mut engine = ResidentWorkerEngine::with_model_binding(&model_path, &sha256, size)
        .map_err(|error| format!("engine start failed: {}", error.code))?;
    println!(
        "ready compute={:?} startup_ms={}",
        engine.compute(),
        started.elapsed().as_millis()
    );
    for run in 0..runs {
        let started = Instant::now();
        let transcript = engine
            .recognize(
                SessionId(u128::from(run) + 1),
                &audio,
                &SessionCancellation::new(),
            )
            .map_err(|error| format!("decode failed: {}", error.code))?;
        let seconds = started.elapsed().as_secs_f64();
        let normalized: String = transcript
            .text
            .to_lowercase()
            .chars()
            .filter(|c| c.is_alphanumeric() || c.is_whitespace())
            .collect();
        println!(
            "run={run} decode_ms={:.0} audio_s={audio_seconds:.2} rtf={:.3} text_chars={} known_phrase={}",
            seconds * 1000.0,
            seconds / audio_seconds,
            transcript.text.chars().count(),
            normalized.contains(KNOWN_PHRASE)
        );
    }
    engine
        .shutdown(std::time::Duration::from_secs(2))
        .map_err(|error| format!("shutdown failed: {}", error.code))?;
    println!("shutdown=ok");
    Ok(())
}

fn digest_file(path: &str) -> Result<(String, u64), Box<dyn std::error::Error>> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1 << 20];
    let mut size = 0_u64;
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        size += count as u64;
        hasher.update(&buffer[..count]);
    }
    let hex = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok((hex, size))
}
