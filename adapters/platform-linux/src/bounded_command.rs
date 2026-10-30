//! Run a short helper (`pactl`, `pw-dump`, `pw-play`, ...) with a deadline.
//!
//! `Command::output()` and `Command::status()` wait for as long as the helper
//! runs. A PipeWire helper talking to a wedged server never ends, so a caller
//! would wait forever. These functions poll the child, kill it when the
//! deadline passes, and read its output on their own threads so a full pipe
//! cannot stall it.
//!
//! The pipe readers end when the pipe closes. If a grandchild keeps a pipe
//! open past the deadline, its reader is left to finish on its own and the
//! output read so far is dropped; the caller is never held.
//!
//! Not for real-time threads: the wait sleeps between polls.

#![forbid(unsafe_code)]

use std::io::Read;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

/// Poll cadence of the wait loop.
const POLL: Duration = Duration::from_millis(10);

/// The most of each stream that is kept. A helper printing more is cut short,
/// not waited on.
pub const OUTPUT_CAP: u64 = 4 * 1024 * 1024;

/// Why a bounded run gave no result. Content-free: no helper output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoundedCommandError {
    /// The helper could not start (missing, not executable).
    Spawn(String),
    /// The helper ran past its deadline and was killed.
    TimedOut(Duration),
    /// Its status could not be observed.
    Wait(String),
}

impl std::fmt::Display for BoundedCommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn(reason) => write!(f, "helper did not start: {reason}"),
            Self::TimedOut(bound) => write!(f, "helper ran past {bound:?} and was killed"),
            Self::Wait(reason) => write!(f, "helper status unavailable: {reason}"),
        }
    }
}

impl std::error::Error for BoundedCommandError {}

/// What a bounded run produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Run `command` to completion within `timeout`, capturing stdout and stderr
/// (each up to [`OUTPUT_CAP`]). Stdin is closed.
///
/// # Errors
/// [`BoundedCommandError::TimedOut`] after the child was killed and reaped,
/// or the spawn/wait failure.
pub fn output_within(
    command: &mut Command,
    timeout: Duration,
) -> Result<BoundedOutput, BoundedCommandError> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let deadline = Instant::now() + timeout;
    let mut child = command
        .spawn()
        .map_err(|error| BoundedCommandError::Spawn(error.to_string()))?;
    let stdout = child.stdout.take().map(read_on_thread);
    let stderr = child.stderr.take().map(read_on_thread);
    let status = wait_until(&mut child, deadline, timeout)?;
    let drain = |reader: Option<Receiver<Vec<u8>>>| {
        reader
            .and_then(|reader| {
                reader
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()).max(POLL))
                    .ok()
            })
            .unwrap_or_default()
    };
    Ok(BoundedOutput {
        status,
        stdout: drain(stdout),
        stderr: drain(stderr),
    })
}

/// Run `command` to completion within `timeout` and return its status. The
/// caller chooses where stdin/stdout/stderr go (`Stdio::null()` is usual);
/// a piped stream would not be read.
///
/// # Errors
/// As [`output_within`].
pub fn status_within(
    command: &mut Command,
    timeout: Duration,
) -> Result<ExitStatus, BoundedCommandError> {
    let deadline = Instant::now() + timeout;
    let mut child = command
        .spawn()
        .map_err(|error| BoundedCommandError::Spawn(error.to_string()))?;
    wait_until(&mut child, deadline, timeout)
}

/// Wait for `child` until `deadline`; kill and reap it past that.
///
/// # Errors
/// [`BoundedCommandError::TimedOut`] once the child was killed, or the wait
/// failure (the child is killed then too).
pub fn wait_until(
    child: &mut std::process::Child,
    deadline: Instant,
    bound: Duration,
) -> Result<ExitStatus, BoundedCommandError> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                // A killed child is reaped at once; this wait does not block.
                let _ = child.wait();
                return Err(BoundedCommandError::TimedOut(bound));
            }
            Ok(None) => {
                std::thread::sleep(POLL.min(deadline.saturating_duration_since(Instant::now())));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(BoundedCommandError::Wait(error.to_string()));
            }
        }
    }
}

fn read_on_thread(pipe: impl Read + Send + 'static) -> Receiver<Vec<u8>> {
    let (sender, receiver) = mpsc::channel();
    // Without a reader the sender is dropped and the output is empty; the
    // wait is still bounded.
    let _ = std::thread::Builder::new()
        .name("narrivy-helper-pipe".to_owned())
        .spawn(move || {
            let mut bytes = Vec::new();
            let _ = pipe.take(OUTPUT_CAP).read_to_end(&mut bytes);
            let _ = sender.send(bytes);
        });
    receiver
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_is_captured() -> Result<(), BoundedCommandError> {
        let out = output_within(
            Command::new("/bin/sh").args(["-c", "printf out; printf err >&2"]),
            Duration::from_secs(5),
        )?;
        assert!(out.status.success());
        assert_eq!(out.stdout, b"out");
        assert_eq!(out.stderr, b"err");
        Ok(())
    }

    #[test]
    fn a_hung_helper_is_killed_at_the_deadline() {
        let started = Instant::now();
        let result = output_within(
            Command::new("/bin/sh").args(["-c", "exec sleep 30"]),
            Duration::from_millis(200),
        );
        assert_eq!(
            result,
            Err(BoundedCommandError::TimedOut(Duration::from_millis(200)))
        );
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_grandchild_holding_the_pipe_does_not_hold_the_caller() {
        // The shell exits at once; its background `sleep` keeps stdout open.
        let started = Instant::now();
        let result = output_within(
            Command::new("/bin/sh").args(["-c", "sleep 30 & printf done"]),
            Duration::from_millis(300),
        );
        assert!(result.is_ok_and(|out| out.status.success()));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn status_is_bounded_too() {
        let result = status_within(
            Command::new("/bin/sh")
                .args(["-c", "exec sleep 30"])
                .stdin(Stdio::null()),
            Duration::from_millis(200),
        );
        assert!(matches!(result, Err(BoundedCommandError::TimedOut(_))));
    }

    #[test]
    fn a_missing_helper_is_a_spawn_error() {
        let result = output_within(
            &mut Command::new("/nonexistent/narrivy-helper"),
            Duration::from_secs(1),
        );
        assert!(matches!(result, Err(BoundedCommandError::Spawn(_))));
    }
}
