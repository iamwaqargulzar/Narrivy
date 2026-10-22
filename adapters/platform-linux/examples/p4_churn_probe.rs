//! P4 stream-churn probe (VER-REL-004 evidence driver).
//!
//! Opens and closes `--cycles` live captures against a real PipeWire target
//! with short bounded windows, then reports open file-descriptor counts
//! before/after plus per-cycle timings as one JSON line. A stable fd count
//! across cycles proves the child + parse-thread teardown does not leak.
//!
//! Suspend/resume is NOT exercised here (documented PARTIAL in VER-REL-004;
//! the P0F phase owns platform suspend harnesses).
//!
//! Usage:
//!   p4_churn_probe [--target @DEFAULT_AUDIO_SOURCE@] [--cycles 20]
//!                  [--window-ms 150] [--rate 16000] [--channels 1]

use narrivy_platform_linux::capture::{CaptureConfig, PreRollRingBuffer};
use narrivy_platform_linux::pipewire_monitor::{LiveCapture, LiveCaptureConfig};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(serde::Serialize)]
struct Report {
    ok: bool,
    error: Option<String>,
    target: String,
    cycles: u32,
    fds_before: usize,
    fds_after: usize,
    max_cycle_ms: f64,
    total_ms: f64,
}

fn count_open_fds() -> usize {
    match std::fs::read_dir("/proc/self/fd") {
        Ok(entries) => entries.count(),
        Err(_) => 0,
    }
}

fn fail(target: String, msg: String) -> ! {
    let report = Report {
        ok: false,
        error: Some(msg),
        target,
        cycles: 0,
        fds_before: 0,
        fds_after: 0,
        max_cycle_ms: 0.0,
        total_ms: 0.0,
    };
    println!("{}", serde_json::to_string(&report).unwrap_or_default());
    std::process::exit(1)
}

fn main() {
    let mut target = "@DEFAULT_AUDIO_SOURCE@".to_string();
    let mut cycles = 20u32;
    let mut window_ms = 150u64;
    let mut rate = 16_000u32;
    let mut channels = 1u16;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--target" => target = args.next().unwrap_or(target),
            "--cycles" => cycles = args.next().and_then(|v| v.parse().ok()).unwrap_or(cycles),
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
        preroll_seconds: 1.5,
        device_hint: None,
    };
    if let Err(e) = config.validate() {
        fail(target.clone(), format!("config: {e}"));
    }

    let cfg = LiveCaptureConfig {
        target: target.clone(),
        sample_rate_hz: rate,
        channels,
    };

    // Settle: open+close one cycle before baseline so one-shot allocations
    // (thread stacks, pipe buffers) don't count as leaks.
    match run_one_cycle(&cfg, &config, window_ms) {
        Ok(_) => {}
        Err(e) => fail(target.clone(), format!("settle cycle: {e}")),
    }

    let fds_before = count_open_fds();
    let t_all = Instant::now();
    let mut max_cycle_ms = 0.0f64;

    for cycle in 0..cycles {
        let t0 = Instant::now();
        match run_one_cycle(&cfg, &config, window_ms) {
            Ok(()) => {}
            Err(e) => fail(target.clone(), format!("cycle {cycle}: {e}")),
        }
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        max_cycle_ms = max_cycle_ms.max(ms);
    }
    let total_ms = t_all.elapsed().as_secs_f64() * 1000.0;

    // Allow small transient slack; a real leak grows by pipes/threads per
    // cycle (2+ fds), which would blow far past this bound.
    let fds_after = count_open_fds();
    let ok = fds_after <= fds_before + 2;
    let report = Report {
        ok,
        error: if ok {
            None
        } else {
            Some(format!(
                "fd leak suspected: before={fds_before} after={fds_after}"
            ))
        },
        target,
        cycles,
        fds_before,
        fds_after,
        max_cycle_ms,
        total_ms,
    };
    println!("{}", serde_json::to_string(&report).unwrap_or_default());
    std::process::exit(if ok { 0 } else { 1 })
}

fn run_one_cycle(
    cfg: &LiveCaptureConfig,
    ring_config: &CaptureConfig,
    window_ms: u64,
) -> Result<(), String> {
    let ring = PreRollRingBuffer::from_config(ring_config).map_err(|e| e.to_string())?;
    let mut handle =
        LiveCapture::start(cfg, Arc::new(Mutex::new(ring))).map_err(|e| e.to_string())?;
    std::thread::sleep(Duration::from_millis(window_ms));
    handle
        .stop(Duration::from_secs(3))
        .map_err(|e| e.to_string())?;
    Ok(())
}
