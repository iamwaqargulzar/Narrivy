//! Bounded external-process invocation for delivery backends.
//!
//! Rules (AGENTS.md Rust/security rules, adapted from the v0.4 salvage):
//! - explicit argv arrays only ([`CommandSpec`]); no shell is ever spawned;
//! - every child runs under a bounded timeout and is killed on overrun
//!   (best-effort — see [`wait_bounded`]);
//! - stdout/stderr are discarded so tool output can never leak into ordinary
//!   logs next to transcript-bearing code paths;
//! - stdin payloads are written by the same thread that owns the child, using
//!   a nonblocking pipe and one absolute deadline. This avoids a detached
//!   transcript-bearing writer and any unbounded thread join.

use std::ffi::OsString;
use std::io::{self, ErrorKind, Write};
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::{Duration, Instant};

/// Poll cadence of the bounded wait loop.
const WAIT_POLL: Duration = Duration::from_millis(5);
/// Reserve a small, documented portion of the *same* absolute deadline for
/// terminating and reaping a child. It never starts a second timeout window.
const REAP_RESERVE: Duration = Duration::from_millis(50);

/// One external invocation as pure data: a program plus an explicit argv
/// array. Never a shell string; transcript-bearing payloads travel through
/// stdin only (`wtype -` and copy tools), never re-quoted.
///
/// `Debug` intentionally redacts the argument vector: arguments can carry
/// transcript text, and the privacy rule forbids user content in logs or
/// panic messages. Tests may read the public fields directly.
#[derive(Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
}

impl CommandSpec {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
        }
    }

    /// Append one explicit argv element.
    pub fn arg(&mut self, arg: impl Into<OsString>) -> &mut Self {
        self.args.push(arg.into());
        self
    }

    /// Content-free shape for diagnostics: program plus argument count only.
    pub fn describe(&self) -> String {
        format!(
            "{} [{} arg(s), redacted]",
            self.program.display(),
            self.args.len()
        )
    }
}

// PRIVACY: never render argument contents; they may hold transcript text.
impl std::fmt::Debug for CommandSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommandSpec")
            .field("program", &self.program)
            .field("args", &format_args!("[{} redacted]", self.args.len()))
            .finish()
    }
}

/// What a bounded run produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunOutcome {
    pub success: bool,
    pub code: Option<i32>,
}

/// Typed failure of a bounded run. Strings are OS/tool diagnostics (spawn or
/// wait errors); they never include stdin payloads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunError {
    Spawn(String),
    StdinWrite(String),
    Timeout,
    /// The original absolute deadline elapsed before a killed child could be
    /// observed as reaped. Callers must not treat this as a successful run.
    TerminationTimeout,
    Wait(String),
}

/// Execution abstraction so routing logic is unit-testable without spawning
/// anything real. Implementations must never panic and must respect `timeout`
/// on every path.
pub trait CommandRunner: Send + Sync {
    fn run(
        &self,
        spec: &CommandSpec,
        stdin_data: Option<&[u8]>,
        timeout: Duration,
    ) -> Result<RunOutcome, RunError>;
}

/// Real std-based runner used in production paths.
pub struct StdCommandRunner;

/// Parent-owned nonblocking stdin state. It never clones the payload or
/// moves it into a background thread: dropping `stdin` after the final write
/// closes EOF on the child under the same owner and deadline.
pub(crate) struct OwnedStdinPump {
    stdin: Option<ChildStdin>,
    offset: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StdinPumpState {
    Pending,
    Complete,
}

impl OwnedStdinPump {
    pub(crate) fn new(stdin: ChildStdin) -> io::Result<Self> {
        set_nonblocking(&stdin)?;
        Ok(Self {
            stdin: Some(stdin),
            offset: 0,
        })
    }

    /// Advance one nonblocking write. `Pending` means the child has not
    /// drained enough pipe capacity yet; the caller remains responsible for
    /// child polling, cancellation, and the absolute deadline.
    pub(crate) fn advance(&mut self, data: &[u8]) -> io::Result<StdinPumpState> {
        if self.stdin.is_none() {
            return Ok(StdinPumpState::Complete);
        }
        if self.offset == data.len() {
            self.stdin.take();
            return Ok(StdinPumpState::Complete);
        }

        let Some(stdin) = self.stdin.as_mut() else {
            return Ok(StdinPumpState::Complete);
        };
        let result = stdin.write(&data[self.offset..]);
        match result {
            Ok(0) => Err(io::Error::new(
                ErrorKind::WriteZero,
                "stdin pipe accepted zero bytes",
            )),
            Ok(written) => {
                self.offset += written;
                if self.offset == data.len() {
                    self.stdin.take();
                    Ok(StdinPumpState::Complete)
                } else {
                    Ok(StdinPumpState::Pending)
                }
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => Ok(StdinPumpState::Pending),
            Err(error) => Err(error),
        }
    }

    /// Whether the operating system has accepted any payload bytes. A positive
    /// `write` is the strict dispatch boundary: after it, a cancellation
    /// cannot truthfully promise that the focused target saw no text.
    #[must_use]
    pub(crate) fn has_accepted_bytes(&self) -> bool {
        self.offset != 0
    }
}

fn set_nonblocking(stdin: &ChildStdin) -> io::Result<()> {
    let fd = stdin.as_raw_fd();
    // SAFETY: `stdin` owns a live Unix file descriptor for this entire call;
    // `F_GETFL` only reads its status flags.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` is still owned by `stdin`; this only adds O_NONBLOCK to
    // its status flags and does not transfer ownership or expose the payload.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

impl StdCommandRunner {
    fn spawn(spec: &CommandSpec, piped_stdin: bool) -> Result<Child, RunError> {
        let mut cmd = Command::new(&spec.program);
        cmd.args(&spec.args)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if piped_stdin {
            cmd.stdin(Stdio::piped());
        } else {
            cmd.stdin(Stdio::null());
        }
        cmd.spawn().map_err(|e| RunError::Spawn(e.to_string()))
    }
}

impl CommandRunner for StdCommandRunner {
    fn run(
        &self,
        spec: &CommandSpec,
        stdin_data: Option<&[u8]>,
        timeout: Duration,
    ) -> Result<RunOutcome, RunError> {
        let mut child = Self::spawn(spec, stdin_data.is_some())?;
        let deadline = Instant::now() + timeout;

        if let Some(data) = stdin_data {
            let stdin = match child.stdin.take() {
                Some(s) => s,
                None => {
                    return Err(cleanup_or_termination_timeout(
                        &mut child,
                        deadline,
                        RunError::StdinWrite("stdin pipe missing".into()),
                    ));
                }
            };
            let mut stdin = match OwnedStdinPump::new(stdin) {
                Ok(stdin) => stdin,
                Err(error) => {
                    return Err(cleanup_or_termination_timeout(
                        &mut child,
                        deadline,
                        RunError::StdinWrite(error.to_string()),
                    ));
                }
            };
            feed_stdin_bounded(&mut child, &mut stdin, data, deadline)?;
        }

        wait_bounded(&mut child, deadline)
    }
}

/// Pump stdin without a background writer. Every error first terminates and
/// reaps the still-owned child within `deadline`; no error path can return a
/// live `ChildStdin` or a detached transcript-bearing writer.
fn feed_stdin_bounded(
    child: &mut Child,
    stdin: &mut OwnedStdinPump,
    data: &[u8],
    deadline: Instant,
) -> Result<(), RunError> {
    loop {
        match stdin.advance(data) {
            Ok(StdinPumpState::Complete) => return Ok(()),
            Ok(StdinPumpState::Pending) => {}
            Err(error) => {
                return Err(cleanup_or_termination_timeout(
                    child,
                    deadline,
                    RunError::StdinWrite(error.to_string()),
                ));
            }
        }
        match child.try_wait() {
            Ok(Some(_)) => {
                return Err(RunError::StdinWrite(
                    "child exited before stdin completed".to_owned(),
                ));
            }
            Ok(None) => {}
            Err(error) => {
                return Err(cleanup_or_termination_timeout(
                    child,
                    deadline,
                    RunError::Wait(error.to_string()),
                ));
            }
        }
        if cleanup_window_open(deadline) {
            return Err(cleanup_or_termination_timeout(
                child,
                deadline,
                RunError::Timeout,
            ));
        }
        std::thread::sleep(WAIT_POLL);
    }
}

/// Bounded wait owned by the same thread that owns the child. There is no
/// detached deadline/ticker thread; an absolute deadline bounds both stdin
/// delivery and process completion.
fn wait_bounded(child: &mut Child, deadline: Instant) -> Result<RunOutcome, RunError> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return Ok(RunOutcome {
                    success: status.success(),
                    code: status.code(),
                });
            }
            Ok(None) => {}
            Err(e) => {
                return Err(cleanup_or_termination_timeout(
                    child,
                    deadline,
                    RunError::Wait(e.to_string()),
                ));
            }
        }
        if cleanup_window_open(deadline) {
            return Err(cleanup_or_termination_timeout(
                child,
                deadline,
                RunError::Timeout,
            ));
        }
        std::thread::sleep(WAIT_POLL);
    }
}

fn cleanup_or_termination_timeout(
    child: &mut Child,
    deadline: Instant,
    original: RunError,
) -> RunError {
    if kill_and_reap(child, deadline) {
        original
    } else {
        RunError::TerminationTimeout
    }
}

/// Kill and reap a child before the absolute deadline. This never uses an
/// unbounded `wait`, and callers retain ownership even if the OS cannot reap
/// a wedged process inside the documented bound.
fn kill_and_reap(child: &mut Child, deadline: Instant) -> bool {
    let _ = child.kill();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(WAIT_POLL),
            Ok(None) | Err(_) => return false,
        }
    }
}

fn cleanup_window_open(deadline: Instant) -> bool {
    deadline
        .checked_sub(REAP_RESERVE)
        .is_none_or(|cleanup_start| Instant::now() >= cleanup_start)
}
