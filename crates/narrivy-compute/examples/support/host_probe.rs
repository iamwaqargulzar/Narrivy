//! Example-only host probe.
//!
//! `narrivy-compute` ships no implementation of [`HostProbe`] on purpose, so
//! this demo supplies the smallest honest one for a Linux developer host. The
//! product implementation of the same contract is the reviewed one in
//! `adapters/platform-linux`; this file exists so `p5_discover` still reports
//! the machine it is run on, and it is deliberately not a place for product
//! policy.

#![allow(clippy::expect_used)]

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use narrivy_compute::probe::{HostProbe, ProbeCommand, ProbeEnvironment, ProbeError, ProbeOutcome};

/// Accelerator device directory published by the kernel.
const ACCEL_DIR: &str = "/dev/accel";

/// Procfs core inventory; absent on non-Linux hosts, which then fall back to
/// the scheduler's own view.
const CPU_INFO: &str = "/proc/cpuinfo";

pub struct SystemHostProbe;

impl HostProbe for SystemHostProbe {
    fn accel_nodes(&self) -> Result<Vec<String>, ProbeError> {
        let Ok(entries) = std::fs::read_dir(ACCEL_DIR) else {
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
        match command.environment() {
            ProbeEnvironment::Scrubbed => {
                process.env_clear();
                for (key, value) in command.published_environment() {
                    process.env(key, value);
                }
            }
            ProbeEnvironment::Inherited => {}
        }
        process.stdout(Stdio::piped()).stderr(Stdio::null());

        let mut child = process
            .spawn()
            .map_err(|error| ProbeError::unavailable(command.program(), error.to_string()))?;
        let deadline = Instant::now() + timeout;
        let status = loop {
            match child
                .try_wait()
                .map_err(|error| ProbeError::unavailable(command.program(), error.to_string()))?
            {
                Some(status) => break status,
                None if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(ProbeError::timed_out(command.program(), timeout));
                }
                None => std::thread::sleep(Duration::from_millis(20)),
            }
        };
        let mut stdout = Vec::new();
        if let Some(mut pipe) = child.stdout.take() {
            let _ = pipe.read_to_end(&mut stdout);
        }
        Ok(ProbeOutcome {
            success: status.success(),
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
        })
    }

    fn logical_cores(&self) -> Option<u32> {
        let from_proc = std::fs::read_to_string(CPU_INFO)
            .ok()
            .map(|text| {
                text.lines()
                    .filter(|line| line.starts_with("processor"))
                    .count()
            })
            .filter(|count| *count > 0)
            .map(|count| count as u32);
        from_proc.or_else(|| {
            std::thread::available_parallelism()
                .ok()
                .map(|cores| cores.get() as u32)
        })
    }
}
