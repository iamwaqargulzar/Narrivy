//! Developer probe: transcribe a 16 kHz mono WAV in pieces, as live
//! meetings do, with each language mode (HANDOFF 000c): the language forced
//! to English, forced to Urdu, detected, and Whisper's translate task.
//! Reads the file only; nothing is played.
//!
//! cargo run -p narrivy-asr-openvino --features worker --example language_probe -- \
//!     RUNTIME_DIR MODEL_DIR DEVICE CACHE_DIR WAV FROM_S TO_S [PIECE_S]

use std::path::PathBuf;
use std::time::Instant;

use narrivy_asr_openvino::genai::Runtime;
use narrivy_asr_openvino::runtime_lib_dir;

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (runtime, model, device, cache, wav, from, to, piece) = match args.as_slice() {
        [r, m, d, c, w, f, t] => (r, m, d, c, w, f, t, "7.5"),
        [r, m, d, c, w, f, t, p] => (r, m, d, c, w, f, t, p.as_str()),
        _ => {
            return Err(
                "usage: language_probe RUNTIME MODEL DEVICE CACHE WAV FROM_S TO_S [PIECE_S]".into(),
            );
        }
    };
    let seconds = |s: &str| s.parse::<f64>().map_err(|e| e.to_string());
    let (from, to, piece) = (seconds(from)?, seconds(to)?, seconds(piece)?);
    let bytes = std::fs::read(wav).map_err(|e| e.to_string())?;
    let samples: Vec<f32> = bytes[44..]
        .chunks_exact(2)
        .map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.0)
        .collect();
    let runtime = Runtime::load(&runtime_lib_dir(&PathBuf::from(runtime)), device == "NPU")
        .map_err(|e| format!("runtime: {e:?}"))?;
    let mut properties = vec![("CACHE_DIR", cache.as_str())];
    if device == "NPU" {
        properties.push(("NPU_COMPILER_TYPE", "PLUGIN"));
    }
    let mut whisper = runtime
        .whisper(&PathBuf::from(model), device, &properties)
        .map_err(|e| format!("load: {e:?}"))?;
    let mut at = from;
    let mut time = [0.0_f64; 4];
    let mut told = false;
    while at < to {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let (a, b) = (
            (at * 16_000.0) as usize,
            (((at + piece).min(to)) * 16_000.0) as usize,
        );
        let chunk = &samples[a.min(samples.len())..b.min(samples.len())];
        println!("== {at:.1}s");
        for (i, (name, language, translate)) in [
            ("en  ", Some("en"), false),
            ("ur  ", Some("ur"), false),
            ("auto", None, false),
            ("tr  ", None, true),
        ]
        .into_iter()
        .enumerate()
        // PROBE_MODES=auto,ur picks the modes (default all four).
        .filter(|(_, (name, _, _))| {
            std::env::var("PROBE_MODES").map_or(true, |modes| {
                modes.split(',').any(|mode| mode == name.trim())
            })
        }) {
            // As the worker does on the NPU: a decode that detects the
            // language after one that was told it gets a fresh pipeline
            // (PROBE_RELOAD=0 turns this off to see the problem).
            if language.is_none()
                && told
                && device == "NPU"
                && std::env::var("PROBE_RELOAD").as_deref() != Ok("0")
            {
                whisper = runtime
                    .whisper(&PathBuf::from(model), device, &properties)
                    .map_err(|e| format!("reload: {e:?}"))?;
                told = false;
            }
            told |= language.is_some();
            let started = Instant::now();
            let text = if translate {
                whisper.translate(chunk, language)
            } else {
                whisper.transcribe(chunk, language)
            };
            time[i] += started.elapsed().as_secs_f64();
            println!("{name} {}", text.unwrap_or_else(|e| format!("<{e:?}>")));
        }
        at += piece;
    }
    println!("seconds per mode (en, ur, auto, translate): {time:.1?}");
    Ok(())
}
