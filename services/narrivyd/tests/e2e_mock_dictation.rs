//! P1 exit evidence: a real client drives mock ASR -> deterministic corrector
//! -> delivery through an IN-PROCESS daemon bound to a tempdir unix socket
//! (never $XDG_RUNTIME_DIR).
//!
//! The daemon wiring (session orchestration + ipc command handler + event
//! pump) is the binary's own code, included verbatim: integration tests
//! cannot link a bin target and lib.rs intentionally excludes the drifted
//! src/daemon.rs (see INTEGRATION-NOTES.md).

#![forbid(unsafe_code)]

#[allow(dead_code)] // includes the binary's never-called main()
mod daemon_bin {
    include!("../src/main.rs");
}

use std::error::Error;
use std::time::{Duration, Instant};

use daemon_bin::{CLEAN_PIPELINE_ID, DaemonState, build_handler, spawn_event_pump};
use narrivy_core::JournalEntryState;
use narrivy_ipc::{Command, Event, IpcErrorCode, PROTOCOL_MINOR, ResponsePayload};
use narrivy_ipc_unix::{IpcClient, UnixIpcServer};

/// Deterministic corrector output asserted end-to-end. Fixture text appears
/// in this test file only (it IS the fixture).
const EXPECTED_DELIVERED: &str = "Hello world from mock asr.";

const EVENT_WAIT: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

fn private_tempdir() -> TestResult<tempfile::TempDir> {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir()?;
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
    Ok(dir)
}

/// One in-process daemon bound to `<dir>/narrivyd.sock`.
struct Harness {
    server: std::sync::Arc<UnixIpcServer>,
    sink: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    journal: std::sync::Arc<narrivy_core::Journal>,
    supervisor: std::sync::Arc<narrivy_core::WorkerSupervisor>,
    state: std::sync::Arc<DaemonState>,
    serve_handle: std::thread::JoinHandle<()>,
    pump_handle: std::thread::JoinHandle<()>,
}

fn start_harness(
    dir: &std::path::Path,
    worker_start_delay: Option<Duration>,
) -> TestResult<Harness> {
    let journal = std::sync::Arc::new(daemon_bin::open_journal(dir)?);
    // The harness keeps the production layout: <dir>/journal under <dir>.
    let state = std::sync::Arc::new(DaemonState::new());
    let sink: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let supervisor = std::sync::Arc::new(narrivy_core::WorkerSupervisor::new());
    let server = std::sync::Arc::new(UnixIpcServer::bind(&dir.join("narrivyd.sock"))?);
    let handler = build_handler(
        state.clone(),
        journal.clone(),
        None,
        sink.clone(),
        supervisor.clone(),
        worker_start_delay,
        std::sync::Arc::new(daemon_bin::EngineConfig::new()),
        None,
    );
    let pump = spawn_event_pump(server.clone(), state.clone());
    let serve_server = server.clone();
    let serve_handle = std::thread::spawn(move || {
        let _ = serve_server.serve(handler);
    });
    Ok(Harness {
        server,
        sink,
        journal,
        supervisor,
        state,
        serve_handle,
        pump_handle: pump,
    })
}

impl Harness {
    fn shutdown(self) -> TestResult {
        self.server.request_shutdown();
        let outcomes = self.supervisor.shutdown_all(Duration::from_secs(2))?;
        if outcomes
            .iter()
            .any(|(_, outcome)| *outcome == narrivy_core::WorkerStop::TimedOut)
        {
            return Err("worker exceeded shutdown deadline".into());
        }
        self.serve_handle.join().map_err(|_| "serve panicked")?;
        daemon_bin::stop_event_pump(&self.state);
        self.pump_handle.join().map_err(|_| "pump panicked")?;
        Ok(())
    }

    /// Parse every committed journal entry on disk.
    fn journal_entries(&self) -> TestResult<Vec<narrivy_core::JournalEntry>> {
        Ok(self.journal.entries()?)
    }

    fn delivered_snapshot(&self) -> Vec<String> {
        self.sink.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

/// Collect SessionStateChanged states until `want` arrives or `budget`
/// elapses. Returns every state observed along the way.
fn collect_until(client: &mut IpcClient, want: &str, budget: Duration) -> TestResult<Vec<String>> {
    let deadline = Instant::now() + budget;
    let mut states = Vec::new();
    while Instant::now() < deadline {
        match client.next_event(POLL_INTERVAL) {
            Ok(Event::SessionStateChanged { state, .. }) => {
                let reached = state == want;
                states.push(state);
                if reached {
                    return Ok(states);
                }
            }
            Ok(_) => {}
            Err(err) if err.code == IpcErrorCode::Timeout => {}
            Err(err) => return Err(err.into()),
        }
    }
    Err(format!("state {want:?} not reached within {budget:?}; saw {states:?}").into())
}

/// Poll GetStatus until the daemon reports no active session.
fn wait_no_active_session(client: &mut IpcClient) -> TestResult {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let ResponsePayload::Status(status) =
            client.request(&Command::GetStatus, Duration::from_secs(1))?
        else {
            return Err("expected Status payload".into());
        };
        if status.active_session.is_none() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("active session never cleared".into());
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

fn drop_and_shutdown(client: IpcClient, harness: Harness) -> TestResult {
    drop(client);
    harness.shutdown()
}

/// Happy path: status -> subscribe -> start -> bounded event wait to
/// Committed; corrected text lands in the delivery sink AND the journal
/// marks the transaction delivered.
#[test]
fn e2e_mock_dictation_full_transaction_over_unix_socket() -> TestResult {
    let dir = private_tempdir()?;
    let harness = start_harness(dir.path(), None)?;

    let mut client = IpcClient::connect_as(&dir.path().join("narrivyd.sock"), "e2e-test")?;
    assert_eq!(client.daemon_build(), narrivy_ipc::BUILD_IDENTITY);
    assert_eq!(client.negotiated_minor(), PROTOCOL_MINOR);

    // Initial status: idle daemon with the composed capability set, including
    // the separate activation facts accepted with the atomic-toggle work.
    let ResponsePayload::Status(status) = client.request(&Command::GetStatus, EVENT_WAIT)? else {
        return Err("expected Status payload".into());
    };
    assert_eq!(status.protocol_minor, PROTOCOL_MINOR);
    assert!(status.active_session.is_none(), "no session yet");
    // Resource use changes on every read, so those tokens are checked by
    // shape below and the rest of the list exactly.
    let (resources, fixed): (Vec<&String>, Vec<&String>) = status
        .capabilities
        .iter()
        .partition(|cap| cap.starts_with("diagnostics.resources."));
    assert!(
        resources
            .iter()
            .any(|cap| cap.starts_with("diagnostics.resources.rss_bytes=")),
        "resource use is reported: {resources:?}"
    );
    assert!(
        resources
            .contains(&&"diagnostics.resources.cpu_permille=unavailable:warming_up".to_owned())
    );
    assert_eq!(
        fixed,
        vec![
            "mock.asr",
            "null.delivery",
            "compute.device=unavailable:no_qualified_plan",
            "history.optional=disabled:not_composed",
            "activation.atomic-command=available",
            "activation.portal-global-shortcuts=unavailable:not_probed",
            "activation.portal-listener=unavailable:not_composed",
            "diagnostics.support-report=available",
            "diagnostics.health.overall=ok",
            "diagnostics.health.subsystems=0",
            "diagnostics.latency.sessions=0",
            "diagnostics.latency.p50_us=unavailable:no_samples",
            "diagnostics.latency.p95_us=unavailable:no_samples",
            "asr.recognition-settings=unavailable",
        ]
    );

    // Subscribe is accepted (connections are registered post-handshake).
    assert_eq!(
        client.request(&Command::Subscribe, EVENT_WAIT)?,
        ResponsePayload::Accepted
    );

    // Start the transaction over the REAL socket.
    assert_eq!(
        client.request(
            &Command::StartSession {
                pipeline_id: Some(CLEAN_PIPELINE_ID.to_owned()),
                activation: "test".to_owned(),
                recognition: None,
            },
            EVENT_WAIT,
        )?,
        ResponsePayload::Accepted
    );

    // Bounded event wait: machine must walk the pipeline states in order.
    let states = collect_until(&mut client, "committed", EVENT_WAIT)?;
    for expected in ["listening", "transcribing", "transforming", "delivering"] {
        assert!(
            states.iter().any(|s| s == expected),
            "missing intermediate state {expected:?}; saw {states:?}"
        );
    }

    // Worker finished: status goes idle again.
    wait_no_active_session(&mut client)?;
    assert_eq!(harness.supervisor.live_worker_count(), 0);

    // Delivery sink received exactly the deterministic correction.
    assert_eq!(
        harness.delivered_snapshot(),
        vec![EXPECTED_DELIVERED.to_owned()]
    );

    // Journal contains a .json entry whose state is delivered and whose
    // delivered variant equals the correction.
    let entries = harness.journal_entries()?;
    let delivered: Vec<_> = entries
        .iter()
        .filter(|entry| entry.state == JournalEntryState::Delivered)
        .collect();
    assert_eq!(delivered.len(), 1, "entries: {entries:?}");
    assert_eq!(
        delivered[0].variants.delivered.as_deref(),
        Some(EXPECTED_DELIVERED)
    );

    drop_and_shutdown(client, harness)
}

/// Cancel right after start: no delivered text anywhere, machine reaches
/// Cancelled (never Committed), journal keeps no delivered transcript.
#[test]
fn e2e_cancel_right_after_start_delivers_nothing() -> TestResult {
    let dir = private_tempdir()?;
    // Widen the window so CancelSession deterministically lands before the
    // worker reaches its first cancellation checkpoint.
    let harness = start_harness(dir.path(), Some(Duration::from_millis(600)))?;

    let mut client = IpcClient::connect_as(&dir.path().join("narrivyd.sock"), "e2e-test")?;
    assert_eq!(
        client.request(&Command::Subscribe, EVENT_WAIT)?,
        ResponsePayload::Accepted
    );
    assert_eq!(
        client.request(
            &Command::StartSession {
                pipeline_id: None,
                activation: "test".to_owned(),
                recognition: None,
            },
            EVENT_WAIT,
        )?,
        ResponsePayload::Accepted
    );
    let ResponsePayload::Status(active_status) = client.request(&Command::GetStatus, EVENT_WAIT)?
    else {
        return Err("expected active Status payload".into());
    };
    let active = active_status
        .active_session
        .ok_or("session was not active during controlled delay")?;
    assert!(
        active.stage_timings_us.is_none(),
        "legacy wall-clock stage timings must remain retired"
    );
    assert_eq!(
        client.request(&Command::CancelSession, EVENT_WAIT)?,
        ResponsePayload::Accepted
    );

    let states = collect_until(&mut client, "cancelled", EVENT_WAIT)?;
    assert!(
        !states.iter().any(|s| s == "committed"),
        "cancelled session must not commit; saw {states:?}"
    );

    // Worker observed the flag and finished without delivering anything.
    wait_no_active_session(&mut client)?;
    assert!(
        harness.delivered_snapshot().is_empty(),
        "nothing may be delivered on cancel"
    );

    // No journal entry may carry a delivered transcript.
    for entry in harness.journal_entries()? {
        assert!(
            entry.variants.delivered.is_none(),
            "journal entry {} carries delivered text after cancel",
            entry.txid
        );
    }

    drop_and_shutdown(client, harness)
}

/// Controlled daemon shutdown uses the same owned cancellation signal as a
/// user cancel. The worker is held before capture/ASR starts, then supervisor
/// shutdown must leave no live worker, Inbox-like delivery, or Pending record.
#[test]
fn e2e_supervisor_shutdown_during_active_session_is_bounded_and_terminal() -> TestResult {
    let dir = private_tempdir()?;
    let harness = start_harness(dir.path(), Some(Duration::from_millis(600)))?;
    let mut client = IpcClient::connect_as(&dir.path().join("narrivyd.sock"), "e2e-test")?;
    assert_eq!(
        client.request(
            &Command::StartSession {
                pipeline_id: None,
                activation: "test".to_owned(),
                recognition: None,
            },
            EVENT_WAIT,
        )?,
        ResponsePayload::Accepted
    );
    let deadline = Instant::now() + Duration::from_secs(1);
    while harness.supervisor.live_worker_count() == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(harness.supervisor.live_worker_count(), 1);
    let stops = harness.supervisor.shutdown_all(Duration::from_secs(2))?;
    assert_eq!(stops.len(), 1);
    assert_eq!(harness.supervisor.live_worker_count(), 0);
    wait_no_active_session(&mut client)?;
    assert!(harness.delivered_snapshot().is_empty());
    for entry in harness.journal_entries()? {
        assert_eq!(entry.state, JournalEntryState::Cancelled);
        assert!(entry.variants.delivered.is_none());
        assert_ne!(entry.state, JournalEntryState::Pending);
    }
    drop_and_shutdown(client, harness)
}
