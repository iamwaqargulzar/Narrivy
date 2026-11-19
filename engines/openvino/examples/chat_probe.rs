//! Developer probe: load a chat model (an OpenVINO LLM or VLM export) on one
//! device and stream a reply to a question, timing the first piece and the
//! whole reply.
//!
//! cargo run -p narrivy-asr-openvino --features worker --example chat_probe -- \
//!     RUNTIME_DIR MODEL_DIR DEVICE CACHE_DIR "question"

use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;

use narrivy_asr_openvino::genai::Runtime;
use narrivy_asr_openvino::runtime_lib_dir;

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [runtime, model, device, cache, question] = args.as_slice() else {
        return Err("usage: chat_probe RUNTIME_DIR MODEL_DIR DEVICE CACHE_DIR QUESTION".into());
    };
    let runtime = Runtime::load(&runtime_lib_dir(&PathBuf::from(runtime)), true)
        .map_err(|e| format!("runtime: {e:?}"))?;
    let mut properties = vec![("CACHE_DIR", cache.as_str())];
    if device == "NPU" {
        properties.extend([("MAX_PROMPT_LEN", "2048"), ("NPU_COMPILER_TYPE", "PLUGIN")]);
    }
    let started = Instant::now();
    let mut llm = runtime
        .llm(&PathBuf::from(model), device, &properties)
        .map_err(|e| format!("load: {e:?}"))?;
    println!("load {:.1}s on {device}", started.elapsed().as_secs_f32());
    let messages = serde_json::json!([
        {"role": "system", "content": "You are a helpful assistant. Answer concisely."},
        {"role": "user", "content": question},
    ])
    .to_string();
    let started = Instant::now();
    let mut first = None;
    let mut pieces = 0_usize;
    let mut on_text = |piece: &str| {
        first.get_or_insert_with(|| started.elapsed());
        pieces += 1;
        print!("{piece}");
        let _ = std::io::stdout().flush();
        false
    };
    let reply = llm
        .chat_streaming(
            &messages,
            Some(r#"{"enable_thinking": false}"#),
            400,
            Some(&mut on_text),
        )
        .map_err(|e| format!("generate: {e:?}"))?;
    let total = started.elapsed().as_secs_f64();
    println!(
        "\n--- first piece {:.0} ms, {pieces} pieces in {total:.1}s (~{:.1}/s), {} bytes",
        first.map_or(0.0, |d| d.as_secs_f64() * 1000.0),
        pieces as f64 / total,
        reply.len()
    );
    Ok(())
}
