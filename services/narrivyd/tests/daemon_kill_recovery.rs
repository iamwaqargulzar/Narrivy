//! F-M18: committed text survives a `SIGKILL` of the real daemon.
//!
//! The test launches the actual `narrivyd` binary (mock ASR) against a private
//! temporary XDG tree, holds the worker in the window after the transaction is
//! journaled as committed and before delivery
//! (`NARRIVY_MOCK_DELIVERY_HOLD_MS`), sends `SIGKILL` inside that window, then
//! starts a second daemon on the same state. Startup recovery must replay the
//! exact corrected text into the durable, private Inbox under the state dir
//! (F-M3), never the runtime dir. The user's own daemon, session bus,
//! microphone and durable state are never touched.

use std::error::Error;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use narrivy_ipc::{Command as IpcCommand, Event, IpcErrorCode, ResponsePayload};
use narrivy_ipc_unix::IpcClient;

/// The mock lane's deterministic corrected text.
const EXPECTED_TEXT: &str = "Hello world from mock asr.";

const READY_DEADLINE: Duration = Duration::from_secs(20);
const EVENT_DEADLINE: Duration = Duration::from_secs(10);
const EXIT_DEADLINE: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(25);
/// Far longer than the test needs: the kill always lands inside the hold.
const DELIVERY_HOLD_MS: &str = "30000";

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

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
        let root = tempfile::tempdir()?;
        let base = root.path();
        std::fs::set_permissions(base, std::fs::Permissions::from_mode(0o700))?;
        let [home, runtime, state, cache, data] =
            ["home", "runtime", "state", "cache", "data"].map(|name| base.join(name));
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

    fn inbox(&self) -> PathBuf {
        self.state.join("narrivy").join("inbox")
    }
}

struct DaemonProcess {
    child: Child,
}

impl DaemonProcess {
    fn start(host: &IsolatedHost, extra_env: &[(&str, &str)]) -> TestResult<Self> {
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
            .stderr(Stdio::null());
        for (key, value) in extra_env {
            command.env(key, value);
        }
        if let Some(path) = std::env::var_os("PATH") {
            command.env("PATH", path);
        }
        let mut daemon = Self {
            child: command.spawn()?,
        };
        daemon.wait_until_ready(&host.socket)?;
        Ok(daemon)
    }

    fn wait_until_ready(&mut self, socket: &Path) -> TestResult {
        let deadline = Instant::now() + READY_DEADLINE;
        while Instant::now() < deadline {
            if let Some(status) = self.child.try_wait()? {
                return Err(format!("daemon exited before IPC readiness: {status}").into());
            }
            if let Ok(mut client) = IpcClient::connect_as(socket, "kill-recovery-test")
                && matches!(
                    client.request(&IpcCommand::GetStatus, Duration::from_secs(2)),
                    Ok(ResponsePayload::Status(_))
                )
            {
                return Ok(());
            }
            std::thread::sleep(POLL);
        }
        Err(format!("daemon was not ready within {READY_DEADLINE:?}").into())
    }

    /// Send `SIGKILL` and reap the process.
    fn kill_9(&mut self) -> TestResult {
        let pid = self.child.id();
        let status = Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("kill -s KILL {pid}"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if !status.success() {
            return Err(format!("kill -s KILL {pid} failed with {status}").into());
        }
        let deadline = Instant::now() + EXIT_DEADLINE;
        loop {
            if let Some(status) = self.child.try_wait()? {
                use std::os::unix::process::ExitStatusExt;
                if status.signal() != Some(9) {
                    return Err(format!("daemon did not die of SIGKILL: {status}").into());
                }
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err("daemon survived SIGKILL".into());
            }
            std::thread::sleep(POLL);
        }
    }
}

impl Drop for DaemonProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Start one session and return its transaction id once the daemon reports
/// `delivering`, which it does only after journaling the commit.
fn start_and_wait_for_delivering(socket: &Path) -> TestResult<String> {
    let mut client = IpcClient::connect_as(socket, "kill-recovery-test")?;
    if client.request(&IpcCommand::Subscribe, EVENT_DEADLINE)? != ResponsePayload::Accepted {
        return Err("subscribe was not accepted".into());
    }
    let started = client.request(
        &IpcCommand::StartSession {
            pipeline_id: Some("mock-dictation".to_owned()),
            activation: "test".to_owned(),
            recognition: None,
        },
        EVENT_DEADLINE,
    )?;
    if started != ResponsePayload::Accepted {
        return Err(format!("session was not accepted: {started:?}").into());
    }
    let deadline = Instant::now() + EVENT_DEADLINE;
    let mut seen = Vec::new();
    while Instant::now() < deadline {
        match client.next_event(POLL) {
            Ok(Event::SessionStateChanged {
                state,
                transaction_id,
                ..
            }) => {
                if state == "committed" {
                    return Err("delivery finished before the kill window".into());
                }
                if state == "delivering" {
                    return transaction_id.ok_or_else(|| "delivering without a txid".into());
                }
                seen.push(state);
            }
            Ok(_) => {}
            Err(error) if error.code == IpcErrorCode::Timeout => {}
            Err(error) => return Err(error.into()),
        }
    }
    Err(format!("delivering not reached within {EVENT_DEADLINE:?}; saw {seen:?}").into())
}

#[test]
fn committed_text_survives_sigkill_before_delivery_and_is_replayed_to_the_inbox() -> TestResult {
    let host = IsolatedHost::new()?;
    let mut first = DaemonProcess::start(
        &host,
        &[("NARRIVY_MOCK_DELIVERY_HOLD_MS", DELIVERY_HOLD_MS)],
    )?;
    let txid = start_and_wait_for_delivering(&host.socket)?;
    first.kill_9()?;
    drop(first);

    let recovered = host.inbox().join(format!("{txid}.txt"));
    assert!(
        !recovered.exists(),
        "nothing may be in the Inbox before the restart replays it"
    );

    // The runtime dir is tmpfs in production and is gone after a reboot.
    std::fs::remove_dir_all(&host.runtime)?;
    std::fs::create_dir(&host.runtime)?;
    std::fs::set_permissions(&host.runtime, std::fs::Permissions::from_mode(0o700))?;

    let _second = DaemonProcess::start(&host, &[])?;
    assert_eq!(std::fs::read_to_string(&recovered)?, EXPECTED_TEXT);
    assert_eq!(
        std::fs::metadata(host.inbox())?.permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(&recovered)?.permissions().mode() & 0o777,
        0o600
    );
    assert!(
        !host
            .runtime
            .join("inbox")
            .join(format!("{txid}.txt"))
            .exists(),
        "recovered text must not live on the runtime dir"
    );
    Ok(())
}
