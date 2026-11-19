//! Linux host probe for the portable compute-planning contract.
//!
//! This is the only place in the release graph that may read an accelerator
//! device node, spawn a bounded discovery helper, or count the host's cores on
//! Narrivy's behalf. It owns no planning policy: which probe answers which
//! question, and how a failure is read, are decided in `narrivy-compute`.
//!
//! Every method here is total. An unreadable device directory, a helper that
//! is not installed, and a kernel that will not name a core count are all
//! reported as "unknown", never as a negative capability claim.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use narrivy_compute::probe::{HostProbe, ProbeCommand, ProbeEnvironment, ProbeError, ProbeOutcome};

/// Accelerator device directory published by the kernel.
const ACCEL_DIR: &str = "/dev/accel";

/// Procfs core inventory. Absent or unparsable means "unknown", not "one".
const CPU_INFO: &str = "/proc/cpuinfo";

/// How often a running probe is checked against its deadline.
const POLL: Duration = Duration::from_millis(20);

/// The most of a probe's standard output that is kept. `ldconfig -p` prints
/// about 100 KB; nothing a probe answers needs more than this.
const STDOUT_CAP: u64 = 8 * 1024 * 1024;

/// The Linux host: a real device directory, real bounded helpers, a real core
/// count. Cheap to construct and safe to share; it holds no handle. Each probe
/// run reads the helper's output on a short-lived thread that ends when the
/// pipe closes.
#[derive(Debug, Clone, Copy, Default)]
pub struct LinuxHostProbe;

impl LinuxHostProbe {
    pub const fn new() -> Self {
        Self
    }

    /// The enumeration sidecar to ask for Vulkan devices, if one is configured.
    ///
    /// A path that is not a regular file yields `None` so the caller falls
    /// through to the inventory helper instead of spawning something that
    /// cannot run.
    pub fn enumeration_sidecar(&self, configured: Option<&Path>) -> Option<PathBuf> {
        configured
            .filter(|path| path.is_file())
            .map(Path::to_path_buf)
    }
}

impl HostProbe for LinuxHostProbe {
    fn accel_nodes(&self) -> Result<Vec<String>, ProbeError> {
        let entries = match std::fs::read_dir(ACCEL_DIR) {
            Ok(entries) => entries,
            // No accelerator class, or no permission to look: the truthful
            // answer is "this host enumerated none", not an error the caller
            // would have to guess about.
            Err(_) => return Ok(Vec::new()),
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
                // A scrubbed probe starts from nothing but what the request
                // publishes: it must not be able to answer "yes" because the
                // daemon's own environment happens to point at a runtime.
                process.env_clear();
                for (key, value) in command.published_environment() {
                    process.env(key, value);
                }
            }
            ProbeEnvironment::Inherited => {}
        }
        // Standard error is dropped here rather than returned: no Narrivy probe
        // decision reads it, and a helper's diagnostics must not reach a log.
        process.stdout(Stdio::piped()).stderr(Stdio::null());

        let mut child = process
            .spawn()
            .map_err(|error| ProbeError::unavailable(command.program(), error.to_string()))?;
        // Read standard output while the child runs: a helper that prints
        // more than a pipe holds (`ldconfig -p` prints about 100 KB) would
        // otherwise block until the deadline killed it (F-H7). The reader
        // ends when the pipe closes; if a grandchild keeps it open past the
        // deadline, the reader is left to finish on its own.
        let (sender, output) = std::sync::mpsc::channel();
        if let Some(pipe) = child.stdout.take() {
            std::thread::spawn(move || {
                let mut stdout = Vec::new();
                let _ = pipe.take(STDOUT_CAP).read_to_end(&mut stdout);
                let _ = sender.send(stdout);
            });
        }
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
                None => std::thread::sleep(POLL),
            }
        };
        let stdout = output
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap_or_default();
        Ok(ProbeOutcome {
            success: status.success(),
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
        })
    }

    fn logical_cores(&self) -> Option<u32> {
        let from_procfs = std::fs::read_to_string(CPU_INFO)
            .ok()
            .map(|text| {
                text.lines()
                    .filter(|line| line.starts_with("processor"))
                    .count()
            })
            .filter(|count| *count > 0)
            .map(|count| count as u32);
        from_procfs.or_else(|| {
            std::thread::available_parallelism()
                .ok()
                .map(|cores| cores.get() as u32)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// F-H7: a helper that prints more than a pipe holds finishes at once
    /// with all of its output, instead of blocking until the deadline.
    #[test]
    fn a_probe_printing_a_mebibyte_completes_well_before_its_deadline() -> Result<(), ProbeError> {
        let command = ProbeCommand::inheriting_environment("/bin/sh")
            .args(["-c", "head -c 1048576 /dev/zero"]);
        let started = Instant::now();
        let outcome = LinuxHostProbe::new().run_probe(&command, Duration::from_secs(10))?;
        assert!(outcome.success);
        assert_eq!(outcome.stdout.len(), 1_048_576);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        Ok(())
    }

    #[test]
    fn a_probe_past_its_deadline_times_out() {
        let command = ProbeCommand::inheriting_environment("/bin/sh").args(["-c", "sleep 5"]);
        let result = LinuxHostProbe::new().run_probe(&command, Duration::from_millis(200));
        assert!(result.is_err());
    }
}
