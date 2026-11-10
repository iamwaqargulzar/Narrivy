//! Voice mode's Urdu speaker (GPL-3.0-or-later, D51).
//!
//! Started by `narrivy-voice-worker` with an empty environment:
//!
//!     narrivy-urdu-speaker --voice <pack dir> [--threads N]
//!
//! It loads Piper's Urdu voice (with eSpeak NG's data from the pack), prints
//! `{"ready":true,"rate":N}`, then answers one line per request:
//! `{"text":"…","speed":1.0}` → `{"rate":N,"pcm":"<base64 f32 little-endian>"}`,
//! or `{"error":"…"}`. It ends when its input closes. It writes nothing
//! else, and never the text it is given, anywhere.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use base64::Engine as _;
use serde_json::{Value, json};
use sherpa_onnx::{GenerationConfig, OfflineTts, OfflineTtsConfig};

/// Longest text one request may carry.
const MAX_TEXT: usize = 4096;
/// The files an Urdu voice pack holds (eSpeak NG's data for Urdu beside
/// the model).
const MODEL: &str = "model.onnx";
const TOKENS: &str = "tokens.txt";
const ESPEAK: &[&str] = &["phontab", "phondata", "phonindex", "intonations", "ur_dict", "lang/ur"];

fn text_of(dir: &Path, name: &str) -> Option<String> {
    dir.join(name).to_str().map(str::to_owned)
}

fn load(dir: &Path, threads: i32) -> Result<OfflineTts, &'static str> {
    if [MODEL, TOKENS].iter().chain(ESPEAK).any(|name| !dir.join(name).is_file()) {
        return Err("urdu_voice_missing");
    }
    let mut config = OfflineTtsConfig::default();
    config.model.vits.model = text_of(dir, MODEL);
    config.model.vits.tokens = text_of(dir, TOKENS);
    // eSpeak NG reads its data from the pack itself (`phontab`, `ur_dict`,
    // `lang/ur`, …): the Store keeps packs at most two folders deep.
    config.model.vits.data_dir = dir.to_str().map(str::to_owned);
    config.model.num_threads = threads;
    config.max_num_sentences = 1;
    OfflineTts::create(&config).ok_or("urdu_voice_invalid")
}

fn answer(tts: &OfflineTts, request: &str) -> Value {
    let Ok(request) = serde_json::from_str::<Value>(request) else {
        return json!({"error": "request_invalid"});
    };
    let Some(text) = request.get("text").and_then(Value::as_str) else {
        return json!({"error": "request_invalid"});
    };
    if text.len() > MAX_TEXT {
        return json!({"error": "text_too_long"});
    }
    let speed = request
        .get("speed")
        .and_then(Value::as_f64)
        .map_or(1.0, |speed| speed.clamp(0.5, 2.0) as f32);
    let config = GenerationConfig {
        speed,
        ..GenerationConfig::default()
    };
    let Some(audio) = tts.generate_with_config::<fn(&[f32], f32) -> bool>(text, &config, None)
    else {
        return json!({"error": "speech_failed"});
    };
    let mut bytes = Vec::with_capacity(audio.samples().len() * 4);
    for sample in audio.samples() {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    json!({
        "rate": audio.sample_rate(),
        "pcm": base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}

fn run() -> Result<(), &'static str> {
    let mut voice: Option<PathBuf> = None;
    let mut threads = 2;
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let value = args.next().ok_or("arguments_invalid")?;
        match flag.as_str() {
            "--voice" => voice = Some(PathBuf::from(value)),
            "--threads" => {
                threads = value
                    .parse::<i32>()
                    .map_err(|_| "arguments_invalid")?
                    .clamp(1, 8)
            }
            _ => return Err("arguments_invalid"),
        }
    }
    let tts = load(&voice.ok_or("arguments_invalid")?, threads)?;
    let stdout = std::io::stdout();
    let say = |value: &Value| -> Result<(), &'static str> {
        let mut out = stdout.lock();
        writeln!(out, "{value}")
            .and_then(|()| out.flush())
            .map_err(|_| "output_closed")
    };
    say(&json!({"ready": true, "rate": tts.sample_rate()}))?;
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        say(&answer(&tts, &line))?;
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => {
            println!("{}", json!({"error": code}));
            ExitCode::FAILURE
        }
    }
}
