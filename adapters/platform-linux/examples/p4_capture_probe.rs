//! P4 live-capture probe (VER-FUN-002 / VER-FUN-006 evidence driver).
//!
//! Captures a real PipeWire target through [`LiveCapture`] into a
//! [`PreRollRingBuffer`] for `--seconds`, then drains and reports level
//! metrics as one JSON line on stdout. No sample data ever leaves this
//! process — only RMS aggregates.
//!
//! Usage:
//!   p4_capture_probe --target narrivy_p4.monitor --seconds 12 \
//!                    [--window-ms 700] [--rate 16000] [--channels 1]

use narrivy_platform_linux::capture::{CaptureConfig, PreRollRingBuffer};
use narrivy_platform_linux::pipewire_monitor::{LiveCapture, LiveCaptureConfig};
use narrivy_types::AudioChunk;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn rms(samples: &[f32]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|s| (*s as f64) * (*s as f64)).sum();
    (sum / samples.len() as f64).sqrt()
}

#[derive(serde::Serialize)]
struct Report {
    ok: bool,
    error: Option<String>,
    target: String,
    rate: u32,
    channels: u16,
    samples_pushed: u64,
    pushes_rejected: u64,
    window_samples: usize,
    rms_window: f64,
    rms_overall: f64,
}

fn emit(report: &Report) {
    println!("{}", serde_json::to_string(report).unwrap_or_default());
}

fn fail(target: &str, rate: u32, channels: u16, msg: String) -> ! {
    let report = Report {
        ok: false,
        error: Some(msg),
        target: target.to_string(),
        rate,
        channels,
        samples_pushed: 0,
        pushes_rejected: 0,
        window_samples: 0,
        rms_window: 0.0,
        rms_overall: 0.0,
    };
    emit(&report);
    std::process::exit(1)
}

fn main() {
    let mut target = "@DEFAULT_AUDIO_SOURCE@".to_string();
    let mut seconds = 5.0f32;
    let mut window_ms = 700u64;
    let mut rate = 16_000u32;
    let mut channels = 1u16;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--target" => target = args.next().unwrap_or(target),
            "--seconds" => seconds = args.next().and_then(|v| v.parse().ok()).unwrap_or(seconds),
            "--window-ms" => {
                window_ms = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(window_ms)
            }
            "--rate" => rate = args.next().and_then(|v| v.parse().ok()).unwrap_or(rate),
            "--channels" => channels = args.next().and_then(|v| v.parse().ok()).unwrap_or(channels),
            other => eprintln!("unknown arg {other}"),
        }
    }

    let config = CaptureConfig {
        sample_rate_hz: rate,
        channels,
        preroll_seconds: 10.0,
        device_hint: None,
    };
    let ring = match PreRollRingBuffer::from_config(&config) {
        Ok(r) => Arc::new(Mutex::new(r)),
        Err(e) => fail(&target, rate, channels, format!("ring init: {e}")),
    };

    let cfg = LiveCaptureConfig {
        target: target.clone(),
        sample_rate_hz: rate,
        channels,
    };
    let mut handle = match LiveCapture::start(&cfg, Arc::clone(&ring)) {
        Ok(h) => h,
        Err(e) => fail(&target, rate, channels, format!("start: {e}")),
    };

    std::thread::sleep(Duration::from_secs_f32(seconds));
    // Bounded stop; the parse thread must EOF within this budget.
    if let Err(e) = handle.stop(Duration::from_secs(3)) {
        eprintln!("stop error: {e}");
    }

    let pushed = handle.stats().samples_pushed.load(Ordering::Acquire);
    let rejected = handle.stats().pushes_rejected.load(Ordering::Acquire);

    let mut guard = match ring.try_lock() {
        Ok(g) => g,
        Err(_) => fail(&target, rate, channels, "ring mutex poisoned".into()),
    };
    let mut drained_chunk = AudioChunk {
        sample_rate_hz: rate,
        channels,
        samples: Vec::new(),
    };
    let _n = guard.drain_into(&mut drained_chunk);
    drop(guard);

    let window_samples = (window_ms as usize * rate as usize * channels as usize) / 1000;
    let drained = &drained_chunk.samples;
    let head_len = window_samples.min(drained.len());
    let report = Report {
        ok: true,
        error: None,
        target,
        rate,
        channels,
        samples_pushed: pushed,
        pushes_rejected: rejected,
        window_samples: head_len,
        rms_window: rms(&drained[..head_len]),
        rms_overall: rms(drained),
    };
    emit(&report);
}
