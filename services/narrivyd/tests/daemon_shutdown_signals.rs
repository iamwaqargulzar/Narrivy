//! Real subprocess shutdown tests for the daemon's SIGTERM/SIGINT bridge.
//!
//! Every test launches the actual `narrivyd` binary against a private
//! temporary XDG runtime/state/cache tree with a cleared environment and an
//! explicit socket path, waits for authenticated IPC readiness (a real client
//! handshake plus one `GetStatus` round-trip), delivers SIGTERM/SIGINT, and
//! asserts a bounded successful exit with the endpoint removed. The user's own
//! daemon/service, session bus, microphone, audio and durable state are never
//! touched.

use std::error::Error;
use std::io::ErrorKind;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use narrivy_ipc::{Command as IpcCommand, ResponsePayload};
use narrivy_ipc_unix::IpcClient;

/// Readiness bound only; it absorbs the daemon's bounded portal probe and
/// process startup. It is not an assertion about shutdown time.
const READY_DEADLINE: Duration = Duration::from_secs(20);

/// Hard hang bound: a daemon still alive after this is treated as a failure.
const EXIT_DEADLINE: Duration = Duration::from_secs(10);

/// Idle-shutdown budget implied by the packaged `TimeoutStopSec=3s` unit.
const IDLE_STOP_BUDGET: Duration = Duration::from_secs(3);

const READY_POLL: Duration = Duration::from_millis(25);
const EXIT_POLL: Duration = Duration::from_millis(20);

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

/// Host-isolated XDG tree for one daemon lifetime.
struct IsolatedHost {
    root: tempfile::TempDir,
    home: PathBuf,
    runtime: PathBuf,
    state: PathBuf,
    cache: PathBuf,
    data: PathBuf,
    socket: PathBuf,
}

impl IsolatedHost {
    fn new() -> TestResult<Self> {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir()?;
        let base = root.path();
        // `tempfile` creates the root as `0o777 & !umask`; the daemon's state
        // and runtime directories must not be group/other writable, so tighten
        // every component including the root before launching.
        std::fs::set_permissions(base, std::fs::Permissions::from_mode(0o700))?;
        let home = base.join("home");
        let runtime = base.join("runtime");
        let state = base.join("state");
        let cache = base.join("cache");
        let data = base.join("data");
        for dir in [&home, &runtime, &state, &cache, &data] {
            std::fs::create_dir(dir)?;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
        let socket = runtime.join("narrivyd.sock");
        Ok(Self {
            root,
            home,
            runtime,
            state,
            cache,
            data,
            socket,
        })
    }
}

/// A launched daemon plus its stderr diagnostics. Dropping it reaps the child
/// even when an assertion fails, so no test leaves an orphan behind.
struct DaemonProcess {
    child: Child,
    socket: PathBuf,
}

impl DaemonProcess {
    fn start(host: &IsolatedHost) -> TestResult<Self> {
        Self::start_with_env(host, &[])
    }

    /// Launch the real daemon binary with additional test-only environment.
    fn start_with_env(host: &IsolatedHost, extra_env: &[(&str, &str)]) -> TestResult<Self> {
        let mut command = Command::new(env!("CARGO_BIN_EXE_narrivyd"));
        command
            .env_clear()
            .current_dir(host.root.path())
            .env("HOME", &host.home)
            .env("XDG_RUNTIME_DIR", &host.runtime)
            .env("XDG_STATE_HOME", &host.state)
            .env("XDG_CACHE_HOME", &host.cache)
            .env("XDG_DATA_HOME", &host.data)
            .env("NARRIVY_RUNTIME_DIR", &host.runtime)
            .env("NARRIVY_SOCKET", &host.socket)
            .env("NARRIVY_ASR_ENGINE", "mock")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        for (key, value) in extra_env {
            command.env(key, value);
        }
        if let Some(path) = std::env::var_os("PATH") {
            command.env("PATH", path);
        }
        let child = command.spawn()?;
        Ok(Self {
            child,
            socket: host.socket.clone(),
        })
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Wait until an authenticated `GetStatus` round-trip succeeds, or the
    /// child exits first.
    fn wait_until_ready(&mut self, timeout: Duration) -> TestResult {
        let deadline = Instant::now() + timeout;
        let mut last_error = "no readiness attempt completed".to_owned();
        while Instant::now() < deadline {
            if let Some(status) = self.child.try_wait()? {
                return Err(format!(
                    "daemon exited before IPC readiness: {status}; {}",
                    self.stderr_diagnostics()
                )
                .into());
            }
            match probe_status(&self.socket) {
                Ok(()) => return Ok(()),
                Err(error) => last_error = error.to_string(),
            }
            std::thread::sleep(READY_POLL);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        Err(format!(
            "daemon was not ready within {timeout:?}: {last_error}; {}",
            self.stderr_diagnostics()
        )
        .into())
    }

    fn wait_for_exit(&mut self, timeout: Duration) -> TestResult<ExitStatus> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Ok(status);
            }
            if Instant::now() >= deadline {
                let _ = self.child.kill();
                let _ = self.child.wait();
                return Err(format!(
                    "daemon did not exit within {timeout:?}; {}",
                    self.stderr_diagnostics()
                )
                .into());
            }
            std::thread::sleep(EXIT_POLL);
        }
    }

    /// Read child stderr after it exited or was reaped. Content-free daemon
    /// diagnostics only; never called while the child can still write.
    fn stderr_diagnostics(&mut self) -> String {
        use std::io::Read;

        let Some(mut stderr) = self.child.stderr.take() else {
            return "stderr unavailable".to_owned();
        };
        let mut text = String::new();
        match stderr.read_to_string(&mut text) {
            Ok(_) => text.trim().to_owned(),
            Err(error) => format!("stderr read failed: {error}"),
        }
    }
}

impl Drop for DaemonProcess {
    fn drop(&mut self) {
        // Reap on every failure path. std refuses to signal a process that was
        // already waited on, so this cannot hit a recycled pid.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Deliver `signal` (for example `TERM`) to `pid`.
///
/// Rust's standard library cannot send arbitrary signals, and this crate
/// forbids `unsafe`, so the tests use the POSIX `kill` shell builtin — the
/// same mechanism a user or service manager uses.
fn deliver_signal(pid: u32, signal: &str) -> TestResult {
    let status = Command::new("/bin/sh")
        .arg("-c")
        .arg(format!("kill -s {signal} {pid}"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if !status.success() {
        return Err(format!("kill -s {signal} {pid} failed with {status}").into());
    }
    Ok(())
}

/// Authenticated readiness probe: real handshake plus one request round-trip.
fn probe_status(socket: &Path) -> TestResult {
    let mut client = IpcClient::connect_as(socket, "shutdown-test")?;
    match client.request(&IpcCommand::GetStatus, Duration::from_secs(2))? {
        ResponsePayload::Status(_) => Ok(()),
        _ => Err("daemon returned an unexpected status payload".into()),
    }
}

fn require_socket_present(socket: &Path) -> TestResult {
    if socket.exists() {
        Ok(())
    } else {
        Err(format!("daemon does not own its endpoint at {}", socket.display()).into())
    }
}

fn assert_socket_removed(socket: &Path) -> TestResult {
    match std::fs::symlink_metadata(socket) {
        Ok(_) => Err(format!("socket {} still exists after shutdown", socket.display()).into()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("stat {} failed: {error}", socket.display()).into()),
    }
}

fn assert_success(status: ExitStatus) -> TestResult {
    if status.success() {
        Ok(())
    } else {
        Err(format!("daemon exited with {status}").into())
    }
}

fn assert_within_idle_budget(elapsed: Duration) -> TestResult {
    if elapsed <= IDLE_STOP_BUDGET {
        Ok(())
    } else {
        Err(format!(
            "idle shutdown took {elapsed:?}, exceeding the packaged TimeoutStopSec={IDLE_STOP_BUDGET:?}"
        )
        .into())
    }
}

#[test]
fn sigterm_stops_an_idle_daemon_with_success_and_removes_the_socket() -> TestResult {
    let host = IsolatedHost::new()?;
    let mut daemon = DaemonProcess::start(&host)?;
    daemon.wait_until_ready(READY_DEADLINE)?;
    require_socket_present(&host.socket)?;

    let signalled = Instant::now();
    deliver_signal(daemon.pid(), "TERM")?;
    let status = daemon.wait_for_exit(EXIT_DEADLINE)?;
    let elapsed = signalled.elapsed();

    assert_success(status)?;
    assert_within_idle_budget(elapsed)?;
    assert_socket_removed(&host.socket)?;
    Ok(())
}

#[test]
fn sigint_stops_an_idle_daemon_with_success_and_removes_the_socket() -> TestResult {
    let host = IsolatedHost::new()?;
    let mut daemon = DaemonProcess::start(&host)?;
    daemon.wait_until_ready(READY_DEADLINE)?;
    require_socket_present(&host.socket)?;

    let signalled = Instant::now();
    deliver_signal(daemon.pid(), "INT")?;
    let status = daemon.wait_for_exit(EXIT_DEADLINE)?;
    let elapsed = signalled.elapsed();

    assert_success(status)?;
    assert_within_idle_budget(elapsed)?;
    assert_socket_removed(&host.socket)?;
    Ok(())
}

#[test]
fn daemon_restarts_on_the_same_socket_and_state_after_a_signal_shutdown() -> TestResult {
    let host = IsolatedHost::new()?;

    let mut first = DaemonProcess::start(&host)?;
    first.wait_until_ready(READY_DEADLINE)?;
    deliver_signal(first.pid(), "TERM")?;
    let first_status = first.wait_for_exit(EXIT_DEADLINE)?;
    assert_success(first_status)?;
    assert_socket_removed(&host.socket)?;
    drop(first);

    // The first run must have created durable state that the restart reuses
    // rather than recreating.
    let journal_dir = host.state.join("narrivy").join("journal");
    let first_inode = std::fs::metadata(&journal_dir)
        .map_err(|error| {
            format!(
                "first run did not create durable state at {}: {error}",
                journal_dir.display()
            )
        })?
        .ino();

    let mut second = DaemonProcess::start(&host)?;
    second.wait_until_ready(READY_DEADLINE)?;
    probe_status(&host.socket)?;

    let signalled = Instant::now();
    deliver_signal(second.pid(), "INT")?;
    let second_status = second.wait_for_exit(EXIT_DEADLINE)?;
    let elapsed = signalled.elapsed();

    assert_success(second_status)?;
    assert_within_idle_budget(elapsed)?;
    assert_socket_removed(&host.socket)?;
    drop(second);

    let second_inode = std::fs::metadata(&journal_dir)?.ino();
    if first_inode != second_inode {
        return Err(format!(
            "restart did not reuse durable state at {}",
            journal_dir.display()
        )
        .into());
    }
    Ok(())
}

/// Start one real supervised session and wait until the daemon reports it as
/// active. `NARRIVY_WORKER_START_DELAY_MS` keeps the session worker inside
/// supervised, cancellable work while the signal arrives, so the shutdown
/// deadline is exercised with a live owner rather than an idle daemon.
fn start_active_session(client: &mut IpcClient) -> TestResult {
    assert_eq!(
        client.request(
            &IpcCommand::StartSession {
                pipeline_id: None,
                activation: "shutdown-test".to_owned(),
                recognition: None,
            },
            Duration::from_secs(2),
        )?,
        ResponsePayload::Accepted
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match client.request(&IpcCommand::GetStatus, Duration::from_secs(2))? {
            ResponsePayload::Status(status) => {
                if status.active_session.is_some() {
                    return Ok(());
                }
            }
            _ => return Err("daemon returned an unexpected status payload".into()),
        }
        if Instant::now() >= deadline {
            return Err("supervised session never became active".into());
        }
        std::thread::sleep(READY_POLL);
    }
}

/// Deliver `signal` to a daemon that owns an active supervised session and
/// require a bounded, truthful, socket-free exit within the packaged budget.
fn assert_active_session_shutdown(signal: &str) -> TestResult {
    let host = IsolatedHost::new()?;
    let mut daemon =
        DaemonProcess::start_with_env(&host, &[("NARRIVY_WORKER_START_DELAY_MS", "60000")])?;
    daemon.wait_until_ready(READY_DEADLINE)?;

    // The client stays connected for the whole case: the daemon must stop from
    // its own signal handling, not because a UI disappeared.
    let mut client = IpcClient::connect_as(&host.socket, "active-shutdown-test")?;
    start_active_session(&mut client)?;

    let signalled = Instant::now();
    deliver_signal(daemon.pid(), signal)?;
    let status = daemon.wait_for_exit(EXIT_DEADLINE)?;
    let elapsed = signalled.elapsed();

    assert_success(status)?;
    assert_within_idle_budget(elapsed)?;
    assert_socket_removed(&host.socket)?;
    let diagnostics = daemon.stderr_diagnostics();
    for unexpected in [
        "did not stop within the shutdown budget",
        "exceeded the shutdown deadline",
    ] {
        if diagnostics.contains(unexpected) {
            return Err(format!(
                "active shutdown reported {unexpected:?} despite success: {diagnostics}"
            )
            .into());
        }
    }
    Ok(())
}

#[test]
fn sigterm_stops_a_daemon_that_owns_an_active_session() -> TestResult {
    assert_active_session_shutdown("TERM")
}

#[test]
fn sigint_stops_a_daemon_that_owns_an_active_session() -> TestResult {
    assert_active_session_shutdown("INT")
}

/// F-M13: a daemon started by hand without the unit's environment refuses to
/// start rather than serving the mock engine's fake transcripts.
#[test]
fn a_daemon_without_an_engine_choice_refuses_to_start() -> TestResult {
    let host = IsolatedHost::new()?;
    let output = Command::new(env!("CARGO_BIN_EXE_narrivyd"))
        .env_clear()
        .current_dir(host.root.path())
        .env("HOME", &host.home)
        .env("XDG_RUNTIME_DIR", &host.runtime)
        .env("XDG_STATE_HOME", &host.state)
        .env("XDG_CACHE_HOME", &host.cache)
        .env("NARRIVY_RUNTIME_DIR", &host.runtime)
        .env("NARRIVY_SOCKET", &host.socket)
        .stdin(Stdio::null())
        .output()?;
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("NARRIVY_ASR_ENGINE is not set"), "{stderr}");
    assert!(!host.socket.exists());
    Ok(())
}

// ---------------------------------------------------------------------------
// Packaged shape (F-M16). The tests above run the mock engine with a test
// socket path. These run the daemon exactly as the shipped user unit does:
// its `Environment=` lines, read from `packaging/deb/narrivy-daemon.service`
// (`%S`/`%C` resolved to the isolated host), the default socket under
// `$XDG_RUNTIME_DIR`, and nothing test-only.
// ---------------------------------------------------------------------------

#[cfg(feature = "asr-transcribe-resident")]
mod packaged {
    use super::*;

    /// The shipped unit's environment, `%S`/`%C` resolved to `host`.
    fn packaged_unit_environment(host: &IsolatedHost) -> TestResult<Vec<(String, String)>> {
        let unit = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../packaging/deb/narrivy-daemon.service");
        let text = std::fs::read_to_string(&unit)?;
        let mut environment = Vec::new();
        for line in text.lines() {
            let Some(assignment) = line.strip_prefix("Environment=") else {
                continue;
            };
            let (key, value) = assignment
                .split_once('=')
                .ok_or("malformed Environment= line in the shipped unit")?;
            let value = match value {
                "%S" => host.state.display().to_string(),
                "%C" => host.cache.display().to_string(),
                other if other.contains('%') => {
                    return Err(format!("unresolved unit specifier in {key}={other}").into());
                }
                other => other.to_owned(),
            };
            environment.push((key.to_owned(), value));
        }
        if !environment
            .iter()
            .any(|(key, value)| key == "NARRIVY_TRANSCRIBE_RUNTIME" && value == "installed")
        {
            return Err("the shipped unit no longer selects the installed runtime".into());
        }
        Ok(environment)
    }

    /// Launch the daemon as the shipped unit does (systemd also provides `HOME`
    /// and `XDG_RUNTIME_DIR` for a user service).
    fn packaged_command(host: &IsolatedHost) -> TestResult<Command> {
        let mut command = Command::new(env!("CARGO_BIN_EXE_narrivyd"));
        command
            .env_clear()
            .current_dir(host.root.path())
            .env("HOME", &host.home)
            .env("XDG_RUNTIME_DIR", &host.runtime)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        for (key, value) in packaged_unit_environment(host)? {
            command.env(key, value);
        }
        if let Some(path) = std::env::var_os("PATH") {
            command.env("PATH", path);
        }
        Ok(command)
    }

    const INSTALLED_RUNTIME_MANIFEST: &str = "/usr/share/narrivy/installed-runtime-manifest.json";

    /// Without the installed runtime the packaged daemon refuses to start with
    /// exit 1 (which the unit's `RestartPreventExitStatus=1` does not restart)
    /// and never serves.
    #[test]
    fn the_packaged_daemon_refuses_to_start_without_the_installed_runtime() -> TestResult {
        if Path::new(INSTALLED_RUNTIME_MANIFEST).exists() {
            // Covered by the ignored packaged-shape shutdown test instead.
            return Ok(());
        }
        let host = IsolatedHost::new()?;
        // Bounded: a daemon that wrongly starts serving fails the test at the
        // deadline instead of hanging it.
        let mut daemon = DaemonProcess {
            child: packaged_command(&host)?.spawn()?,
            socket: host.runtime.join(narrivy_ipc_unix::SOCKET_SUBPATH),
        };
        let status = daemon.wait_for_exit(EXIT_DEADLINE)?;
        let stderr = daemon.stderr_diagnostics();
        assert_eq!(status.code(), Some(1), "{stderr}");
        assert!(
            stderr.contains("installed resident runtime failed verification"),
            "{stderr}"
        );
        assert!(!host.runtime.join(narrivy_ipc_unix::SOCKET_SUBPATH).exists());
        Ok(())
    }

    /// SIGTERM and SIGINT stop the daemon in its packaged shape within the
    /// unit's `TimeoutStopSec`, with success and the socket removed.
    #[test]
    #[ignore = "needs the installed Narrivy runtime (/usr/lib/narrivy, /usr/share/narrivy); \
                run on a machine with the package installed: cargo test -p narrivyd \
                --features asr-transcribe-resident --test daemon_shutdown_signals -- --ignored"]
    fn the_packaged_daemon_stops_on_sigterm_and_sigint() -> TestResult {
        if !Path::new(INSTALLED_RUNTIME_MANIFEST).exists() {
            return Err(
                format!("{INSTALLED_RUNTIME_MANIFEST} is missing: install the package").into(),
            );
        }
        for signal in ["TERM", "INT"] {
            let host = IsolatedHost::new()?;
            let socket = host.runtime.join(narrivy_ipc_unix::SOCKET_SUBPATH);
            let mut daemon = DaemonProcess {
                child: packaged_command(&host)?.spawn()?,
                socket: socket.clone(),
            };
            daemon.wait_until_ready(READY_DEADLINE)?;
            require_socket_present(&socket)?;
            let signalled = Instant::now();
            deliver_signal(daemon.pid(), signal)?;
            let status = daemon.wait_for_exit(EXIT_DEADLINE)?;
            let elapsed = signalled.elapsed();
            assert_success(status)?;
            assert_within_idle_budget(elapsed)?;
            assert_socket_removed(&socket)?;
        }
        Ok(())
    }
}
