//! Developer probe: run a fixed-shape pack (scripts/npu/qwen35_npu.py)
//! with Narrivy's own runner and score it against cases with expected text.
//!
//! cargo run -p narrivy-asr-openvino --features worker --example static_probe -- \
//!     RUNTIME_DIR PACK_DIR DEVICE CACHE_DIR CASES_JSONL
//!
//! With `chat` in place of CASES_JSONL, a three-turn conversation is
//! streamed instead, showing how much each turn reuses.

use std::path::PathBuf;
use std::time::Instant;

use narrivy_asr_openvino::genai::Runtime;
use narrivy_asr_openvino::refine_protocol::{REFINE_PROFILE, RefineProfile};
use narrivy_asr_openvino::runtime_lib_dir;
use narrivy_asr_openvino::static_llm::StaticLlm;

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [runtime, pack, device, cache, cases] = args.as_slice() else {
        return Err("usage: static_probe RUNTIME_DIR PACK_DIR DEVICE CACHE_DIR CASES_JSONL".into());
    };
    let pack = PathBuf::from(pack);
    let profile = RefineProfile::parse(
        &std::fs::read(pack.join(REFINE_PROFILE)).map_err(|e| format!("profile: {e}"))?,
    )
    .ok_or("profile invalid")?;
    let runtime = Runtime::load(&runtime_lib_dir(&PathBuf::from(runtime)), device == "NPU")
        .map_err(|e| format!("runtime: {e:?}"))?;
    let started = Instant::now();
    let mut llm = StaticLlm::load(&runtime, &pack, device, &PathBuf::from(cache))
        .map_err(|e| format!("load: {e:?}"))?;
    println!("load {:.1}s on {device}", started.elapsed().as_secs_f32());
    if std::env::var_os("NARRIVY_PROBE_NO_GUESS").is_some() {
        llm.set_guessing(false);
    }
    if cases == "chat" {
        return chat(&mut llm);
    }
    let text = std::fs::read_to_string(cases).map_err(|e| format!("cases: {e}"))?;
    let (mut passed, mut total) = (0, 0);
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let case: serde_json::Value =
            serde_json::from_str(line).map_err(|e| format!("case: {e}"))?;
        let field = |key: &str| case.get(key).and_then(|v| v.as_str()).unwrap_or_default();
        let user = format!("{}{}", profile.user_prefix, field("input"));
        let started = Instant::now();
        let reply = llm
            .generate(&profile.system, &user, field("input"), 512, &mut || false)
            .map_err(|e| format!("generate: {e:?}"))?;
        let ok = reply.trim() == field("expected").trim();
        total += 1;
        passed += usize::from(ok);
        println!(
            "{} {:>5.0} ms {} [{}]",
            if ok { "PASS" } else { "FAIL" },
            started.elapsed().as_secs_f64() * 1000.0,
            field("id"),
            field("category"),
        );
        if !ok {
            println!(
                "   out: {:?}\n   exp: {:?}",
                reply.trim(),
                field("expected")
            );
        }
    }
    println!("{passed}/{total} exact");
    Ok(())
}

fn chat(llm: &mut StaticLlm<'_>) -> Result<(), String> {
    let mut messages = vec![(
        "system".to_owned(),
        "You are a helpful assistant. Answer briefly.".to_owned(),
    )];
    for question in [
        "What is the capital of France?",
        "And what river runs through it?",
        "Name one famous museum there.",
        "Explain in four sentences why the sky is blue.",
    ] {
        messages.push(("user".to_owned(), question.to_owned()));
        let started = Instant::now();
        let mut first: Option<f64> = None;
        let mut pieces = 0;
        let reply = llm
            .chat(&messages, 200, &mut || false, &mut |_| {
                first.get_or_insert(started.elapsed().as_secs_f64() * 1000.0);
                pieces += 1;
                false
            })
            .map_err(|e| format!("chat: {e:?}"))?;
        let total = started.elapsed().as_secs_f64();
        let words = reply.split_whitespace().count();
        let writing = total - first.unwrap_or(0.0) / 1000.0;
        println!(
            "{:>5.0} ms total, first text after {:>4.0} ms, {:.1} words/s while writing | {question} -> {:?}",
            total * 1000.0,
            first.unwrap_or(0.0),
            words as f64 / writing.max(0.001),
            reply.trim()
        );
        messages.push(("assistant".to_owned(), reply.trim().to_owned()));
    }
    Ok(())
}
