//! Developer probe: load an installed OpenVINO runtime pack, list its
//! devices, and time a model pack on one device with the JFK fixture.
//!
//! cargo run -p narrivy-asr-openvino --features worker --example openvino_probe -- \
//!     RUNTIME_DIR MODEL_DIR DEVICE CACHE_DIR

use std::path::PathBuf;
use std::time::Instant;

use narrivy_asr_api::self_test;
use narrivy_asr_openvino::genai::Runtime;
use narrivy_asr_openvino::runtime_lib_dir;

const WAV: &[u8] = include_bytes!("../../../tests/fixtures/jfk16.wav");

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [runtime, model, device, cache] = args.as_slice() else {
        return Err("usage: openvino_probe RUNTIME_DIR MODEL_DIR DEVICE CACHE_DIR".into());
    };
    let runtime = Runtime::load(&runtime_lib_dir(&PathBuf::from(runtime)), true)
        .map_err(|e| format!("runtime: {e:?}"))?;
    println!("devices: {:?}", runtime.devices());
    let mut properties = vec![("CACHE_DIR", cache.as_str())];
    if device == "NPU" {
        properties.push(("NPU_COMPILER_TYPE", "PLUGIN"));
    }
    let started = Instant::now();
    let mut whisper = runtime
        .whisper(&PathBuf::from(model), device, &properties)
        .map_err(|e| format!("load: {e:?}"))?;
    println!("load {:.1}s", started.elapsed().as_secs_f32());
    let samples = self_test::fixture_samples(WAV).ok_or("fixture")?;
    for run in 0..3 {
        let started = Instant::now();
        let text = whisper
            .transcribe(&samples, Some("en"))
            .map_err(|e| format!("run: {e:?}"))?;
        println!(
            "run{run} {:.2}s {}",
            started.elapsed().as_secs_f32(),
            self_test::phrase_matches(&text)
        );
    }
    Ok(())
}
