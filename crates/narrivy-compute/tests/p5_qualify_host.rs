//! Host-dependent qualification coverage.
//!
//! These two cases are the real-decode proof that a measured qualification
//! tuple means what it says, and they only run where a developer host actually
//! has the sidecar, the pinned model and the fixture installed. They live in
//! an integration target rather than in `src/qualify.rs` for one reason: the
//! library must contain no process-spawning implementation at all, so the
//! `HostProbe` that really runs a command is injected here, from outside the
//! portable crate. The product implementation of the same contract is the
//! Linux one in `adapters/platform-linux`.

#![allow(clippy::expect_used)]

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use narrivy_compute::discovery::DeviceClass;
use narrivy_compute::probe::{HostProbe, ProbeCommand, ProbeError, ProbeOutcome};
use narrivy_compute::qualify::qualify_decode;

const SIDECAR: &str = "code/vendor/transcribe.cpp/build/bin/transcribe-cli";
const MODEL: &str = ".cache/narrivy/models/whisper-tiny.en-Q8_0.gguf";
const FIXTURE: &str = ".cache/narrivy/fixtures/jfk16.wav";
const FRAGMENT: &str = "ask not what your country";

/// The host's own probe: spawn the request, kill it once `timeout` elapses,
/// then drain the pipes (safe only after the child has exited or been killed).
struct SystemHostProbe;

impl HostProbe for SystemHostProbe {
    fn accel_nodes(&self) -> Result<Vec<String>, ProbeError> {
        let Ok(entries) = std::fs::read_dir("/dev/accel") else {
            return Ok(Vec::new());
        };
        let mut names: Vec<String> = entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("accel"))
            .collect();
        names.sort();
        Ok(names)
    }

    fn run_probe(
        &self,
        command: &ProbeCommand,
        timeout: Duration,
    ) -> Result<ProbeOutcome, ProbeError> {
        let mut process = Command::new(command.program());
        process.args(command.arguments());
        process.stdout(Stdio::piped()).stderr(Stdio::null());

        let mut child = process
            .spawn()
            .map_err(|error| ProbeError::unavailable(command.program(), error.to_string()))?;
        let deadline = std::time::Instant::now() + timeout;
        let status = loop {
            match child
                .try_wait()
                .map_err(|error| ProbeError::unavailable(command.program(), error.to_string()))?
            {
                Some(status) => break status,
                None if std::time::Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(ProbeError::timed_out(command.program(), timeout));
                }
                None => std::thread::sleep(Duration::from_millis(20)),
            }
        };
        let mut stdout = String::new();
        if let Some(mut pipe) = child.stdout.take() {
            let _ = pipe.read_to_string(&mut stdout);
        }
        Ok(ProbeOutcome {
            success: status.success(),
            stdout,
        })
    }

    fn logical_cores(&self) -> Option<u32> {
        std::thread::available_parallelism()
            .ok()
            .map(|cores| cores.get() as u32)
    }
}

/// The developer home that holds the optional sidecar fixtures.
fn home() -> PathBuf {
    match std::env::var("HOME") {
        Ok(home) => PathBuf::from(home),
        Err(_) => PathBuf::from("/tmp"),
    }
}

fn real_decode_inputs() -> Option<(PathBuf, PathBuf, PathBuf)> {
    let cli = PathBuf::from(SIDECAR);
    let model = home().join(MODEL);
    let wav = home().join(FIXTURE);
    if cli.is_file() && model.is_file() && wav.is_file() {
        Some((cli, model, wav))
    } else {
        None
    }
}

#[test]
fn real_cpu_decode_qualifies_when_sidecar_present() {
    let Some((cli, model, wav)) = real_decode_inputs() else {
        return;
    };
    let tuple = qualify_decode(
        &SystemHostProbe,
        &cli,
        "cpu",
        DeviceClass::Cpu,
        &model,
        &wav,
        11.0,
        FRAGMENT,
        Duration::from_secs(300),
    )
    .expect("real decode qualification");
    assert!(tuple.pass, "expected fragment match on known-good fixture");
    assert!(tuple.metrics.rtf.unwrap_or(0.0) > 0.1);
    assert!(matches!(tuple.device_class, DeviceClass::Cpu));
}

#[test]
fn real_cpu_decode_tiny_en_wer_is_one_when_sidecar_present() {
    let Some((cli, model, wav)) = real_decode_inputs() else {
        return;
    };
    let tuple = qualify_decode(
        &SystemHostProbe,
        &cli,
        "cpu",
        DeviceClass::Cpu,
        &model,
        &wav,
        11.0,
        FRAGMENT,
        Duration::from_secs(300),
    )
    .expect("real decode qualification");
    let wer = tuple.metrics.wer_fragment.unwrap_or(0.0);
    assert!(
        (wer - 1.0).abs() < 1e-6,
        "expected 1.0 on jfk fixture, got {wer}"
    );
    assert!(tuple.pass);
}

#[test]
fn a_wedged_sidecar_is_reported_as_a_timeout_instead_of_hanging() {
    // A real process that outlives its budget: the point of the assertion is
    // that the caller gets a typed failure within the budget it asked for.
    let sleeper =
        std::env::temp_dir().join(format!("narrivy-qualify-timeout-{}", std::process::id()));
    std::fs::write(&sleeper, b"#!/bin/sh\nsleep 30\n").expect("write sleeper");
    make_executable(&sleeper);

    let outcome = qualify_decode(
        &SystemHostProbe,
        &sleeper,
        "cpu",
        DeviceClass::Cpu,
        Path::new("/models/m-Q8_0.gguf"),
        Path::new("/fixtures/a.wav"),
        11.0,
        FRAGMENT,
        Duration::from_millis(200),
    );
    let _ = std::fs::remove_file(&sleeper);
    assert!(
        outcome.is_err_and(|error| {
            matches!(
                error,
                narrivy_compute::planner::ComputeError::SidecarTimeout(_)
            )
        }),
        "a wedged sidecar must surface as SidecarTimeout, not as a hang"
    );
}

fn make_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(path).expect("metadata").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).expect("chmod");
    }
}
