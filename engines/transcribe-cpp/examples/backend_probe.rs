//! Development probe: list the compute devices this build sees, then time one
//! decode of a 16 kHz mono 16-bit WAV on a chosen backend.
//!
//! `cargo run --release -p narrivy-asr-transcribe --features vulkan --example
//! backend_probe -- <model.gguf> <audio.wav> <cpu|vulkan|auto>`
//!
//! Prints device facts and timings only, never the transcript.

use std::time::Instant;

use transcribe_cpp::{Backend, Model, ModelOptions, RunOptions, StreamOptions};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let (Some(model_path), Some(wav_path), Some(backend)) = (args.next(), args.next(), args.next())
    else {
        return Err("usage: backend_probe <model.gguf> <audio.wav> <cpu|vulkan|auto>".into());
    };
    let backend = match backend.as_str() {
        "cpu" => Backend::Cpu,
        "vulkan" => Backend::Vulkan,
        "auto" => Backend::Auto,
        _ => return Err("backend must be cpu, vulkan or auto".into()),
    };
    transcribe_cpp::disable_logging();
    for device in transcribe_cpp::devices() {
        println!(
            "device name={} kind={} type={:?} description={} memory_total={}",
            device.name, device.kind, device.device_type, device.description, device.memory_total
        );
    }
    println!(
        "vulkan_available={}",
        transcribe_cpp::backend_available(Backend::Vulkan)
    );

    let bytes = std::fs::read(wav_path)?;
    // Canonical 44-byte header, 16-bit PCM: enough for the repository fixture.
    let samples: Vec<f32> = bytes
        .get(44..)
        .ok_or("wav too short")?
        .chunks_exact(2)
        .map(|pair| f32::from(i16::from_le_bytes([pair[0], pair[1]])) / 32_768.0)
        .collect();
    let audio_seconds = samples.len() as f64 / 16_000.0;

    let started = Instant::now();
    let model = Model::load_with(
        &model_path,
        &ModelOptions {
            backend,
            ..ModelOptions::default()
        },
    )?;
    println!(
        "loaded backend={} load_ms={}",
        model.backend(),
        started.elapsed().as_millis()
    );
    for run in 0..3 {
        let started = Instant::now();
        let transcript = model.session()?.run(&samples, &RunOptions::default())?;
        let seconds = started.elapsed().as_secs_f64();
        // Whether the known recording decoded to its known phrase (the GPU
        // self-test's check); the transcript itself is never printed.
        println!(
            "run={run} decode_ms={:.0} rtf={:.3} chars={} matches_reference={}",
            seconds * 1000.0,
            seconds / audio_seconds,
            transcript.text.len(),
            narrivy_asr_transcribe::self_test::phrase_matches(&transcript.text)
        );
    }
    if model.capabilities()?.supports_streaming {
        // Feed 80 ms chunks, as live capture does, and time each feed.
        let mut session = model.session()?;
        let mut stream = session.stream(&RunOptions::default(), &StreamOptions::default())?;
        let mut feeds_ms: Vec<f64> = Vec::new();
        let started = Instant::now();
        for chunk in samples.chunks(1_280) {
            let feed = Instant::now();
            stream.feed(chunk)?;
            feeds_ms.push(feed.elapsed().as_secs_f64() * 1000.0);
        }
        let finalize = Instant::now();
        stream.finalize()?;
        feeds_ms.sort_by(f64::total_cmp);
        println!(
            "stream chunks={} total_ms={:.0} feed_p50_ms={:.1} feed_p95_ms={:.1} feed_max_ms={:.1} finalize_ms={:.0}",
            feeds_ms.len(),
            started.elapsed().as_secs_f64() * 1000.0,
            feeds_ms[feeds_ms.len() / 2],
            feeds_ms[feeds_ms.len() * 95 / 100],
            feeds_ms[feeds_ms.len() - 1],
            finalize.elapsed().as_secs_f64() * 1000.0,
        );
    }
    Ok(())
}
