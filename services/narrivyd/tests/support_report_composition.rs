//! REL-008 evidence, daemon half: the daemon really composes the content-safe
//! diagnostics surface, really records a real session into it, and advertises
//! facts that stay content-free while a real transcript exists on the system.
//!
//! One in-process daemon is driven through a full mock dictation over the real
//! unix socket, exactly as `e2e_mock_dictation.rs` does, with a composed
//! retained History store so the dictated text is provably present in the
//! product at the moment the diagnostics facts are read. The advertised
//! capabilities must then carry the session's own outcome and duration, and no
//! capability may carry the transcript, audio, clipboard or environment
//! content that now exists.
//!
//! The second half drives the same composition with the surface deliberately
//! omitted and asserts the gate fails closed, so a client can never offer a
//! support report for a daemon that does not back one.

#![forbid(unsafe_code)]

#[allow(dead_code)] // includes the binary's never-called main()
mod daemon_bin {
    include!("../src/main.rs");
}

use std::error::Error;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use daemon_bin::daemon::diagnostics::{
    DaemonDiagnostics, SUPPORT_REPORT_AVAILABLE_CAPABILITY, SUPPORT_REPORT_NOT_COMPOSED_CAPABILITY,
};
use daemon_bin::daemon::retained_history::{RetainedHistory, open_retained_history};
use daemon_bin::{DaemonState, build_handler_with_portal, spawn_event_pump, stop_event_pump};
use narrivy_core::Journal;
use narrivy_history::RetentionPolicy;
use narrivy_ipc::{Command, Event, IpcErrorCode, ResponsePayload, StatusDto};
use narrivy_ipc_unix::{IpcClient, UnixIpcServer};

/// The exact text the mock ASR transcribes for the driven session. It is the
/// canary: if any of it appears in an advertised fact, the fact leaked.
const TRANSCRIPT_CANARY: &str = daemon_bin::daemon::policy::MOCK_TRANSCRIPT;

/// Content that exists in the product while the report is produced.
const AUDIO_CANARY_MARKER: &str = "44100-sample-mono-pcm";

// Generous bounds: they only cap a hung daemon. Correctness never depends on
// how fast the daemon is (the tests wait for states, not for time), so a
// loaded machine makes these tests slower, not flaky.
const REQ_TIMEOUT: Duration = Duration::from_secs(10);
const EVENT_WAIT: Duration = Duration::from_secs(60);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

fn private_tempdir() -> TestResult<tempfile::TempDir> {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = tempfile::tempdir()?;
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
    Ok(dir)
}

struct Harness {
    server: Arc<UnixIpcServer>,
    state: Arc<DaemonState>,
    socket: std::path::PathBuf,
    serve_handle: std::thread::JoinHandle<()>,
    pump_handle: std::thread::JoinHandle<()>,
}

impl Harness {
    fn connect(&self) -> TestResult<IpcClient> {
        Ok(IpcClient::connect_as(&self.socket, "support-report-test")?)
    }

    fn shutdown(self) -> TestResult {
        self.server.request_shutdown();
        self.serve_handle.join().map_err(|_| "serve panicked")?;
        stop_event_pump(&self.state);
        self.pump_handle.join().map_err(|_| "pump panicked")?;
        Ok(())
    }
}

/// Start one real daemon over `<dir>/narrivyd.sock`, composing the optional
/// dependencies this test needs. `diagnostics: None` is the deliberate
/// fail-closed composition.
fn start_harness(
    dir: &Path,
    history: Option<Arc<RetainedHistory>>,
    diagnostics: bool,
) -> TestResult<Harness> {
    let journal = Arc::new(daemon_bin::open_journal(dir)?);
    let state = Arc::new(DaemonState::new());
    let sink: daemon_bin::DeliverySink = Arc::new(std::sync::Mutex::new(Vec::new()));
    let supervisor = Arc::new(narrivy_core::WorkerSupervisor::new());
    let socket = dir.join("narrivyd.sock");
    let server = Arc::new(UnixIpcServer::bind(&socket)?);
    let handler = build_handler_with_portal(
        state.clone(),
        journal,
        history,
        sink,
        supervisor,
        None,
        Arc::new(daemon_bin::EngineConfig::new()),
        None,
        diagnostics.then(|| Arc::new(DaemonDiagnostics::new())),
        None,
    );
    let pump_handle = spawn_event_pump(server.clone(), state.clone());
    let serve_server = server.clone();
    let serve_handle = std::thread::spawn(move || {
        let _ = serve_server.serve(handler);
    });
    Ok(Harness {
        server,
        state,
        socket,
        serve_handle,
        pump_handle,
    })
}

fn status(client: &mut IpcClient) -> TestResult<StatusDto> {
    let ResponsePayload::Status(status) = client.request(&Command::GetStatus, REQ_TIMEOUT)? else {
        return Err("expected Status payload".into());
    };
    Ok(status)
}

fn capability<'a>(status: &'a StatusDto, name: &str) -> Option<&'a str> {
    status
        .capabilities
        .iter()
        .filter_map(|value| value.split_once('='))
        .find_map(|(key, value)| (key == name).then_some(value))
}

/// Drive one full mock dictation to `committed` and return its transaction id.
fn dictate(client: &mut IpcClient) -> TestResult<String> {
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
    let deadline = Instant::now() + EVENT_WAIT;
    let mut committed = None;
    while Instant::now() < deadline {
        match client.next_event(POLL_INTERVAL) {
            Ok(Event::SessionStateChanged {
                transaction_id,
                state,
                ..
            }) if state == "committed" => {
                committed = transaction_id;
                break;
            }
            Ok(_) => {}
            Err(err) if err.code == IpcErrorCode::Timeout => {}
            Err(err) => return Err(err.into()),
        }
    }
    let txid: String = committed.ok_or("session never reached committed")?;
    wait_session_finished(client)?;
    Ok(txid)
}

/// Wait until the daemon no longer reports the session (F-M16).
///
/// `committed` is published while the session worker still runs: it then
/// records the diagnostics observation and mirrors the result into History.
/// The worker's cleanup guard is dropped last and only then removes the
/// session slot, so `active_session == None` in a status read means both are
/// done. The old wait raced `committed` against the History write and, when
/// its shared deadline ran out, carried on silently.
fn wait_session_finished(client: &mut IpcClient) -> TestResult {
    let deadline = Instant::now() + EVENT_WAIT;
    loop {
        if status(client)?.active_session.is_none() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("the session never finished".into());
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// A composed daemon advertises the support-report gate, and a real finished
/// session shows up in the advertised health and latency facts — with a real
/// transcript sitting in the retained History the whole time.
#[test]
fn a_composed_daemon_advertises_real_content_free_diagnostics_facts() -> TestResult {
    let dir = private_tempdir()?;
    let history = Arc::new(open_retained_history(dir.path(), RetentionPolicy::Days7)?);
    let harness = start_harness(dir.path(), Some(Arc::clone(&history)), true)?;
    let mut client = harness.connect()?;

    // Before any session: composed but empty, and explicit about having no
    // latency samples rather than reporting an impossibly fast session.
    let before = status(&mut client)?;
    assert!(
        before
            .capabilities
            .iter()
            .any(|value| value == SUPPORT_REPORT_AVAILABLE_CAPABILITY),
        "{:?}",
        before.capabilities
    );
    assert_eq!(
        capability(&before, "diagnostics.latency.sessions"),
        Some("0")
    );
    assert_eq!(
        capability(&before, "diagnostics.latency.p50_us"),
        Some("unavailable:no_samples")
    );
    assert_eq!(
        capability(&before, "diagnostics.health.subsystems"),
        Some("0")
    );

    dictate(&mut client)?;

    // The dictated text is provably present in the product right now.
    let rows = match client.request(&Command::GetHistory, REQ_TIMEOUT)? {
        ResponsePayload::History(rows) => rows,
        _ => return Err("expected History payload".into()),
    };
    assert_eq!(rows.len(), 1, "exactly the committed dictation is retained");
    let ResponsePayload::HistoryEntry(detail) = client.request(
        &Command::GetHistoryEntry {
            txid: rows[0].txid.clone(),
        },
        REQ_TIMEOUT,
    )?
    else {
        return Err("expected HistoryEntry payload".into());
    };
    let delivered = detail
        .delivered
        .clone()
        .ok_or("the retained row must hold the delivered transcript")?;
    assert!(
        delivered.contains("mock asr"),
        "the delivered transcript must really exist: {delivered}"
    );

    // The finished session is now a real, advertised fact.
    let after = status(&mut client)?;
    let sessions = capability(&after, "diagnostics.latency.sessions")
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or("the latency sample count must be advertised")?;
    assert_eq!(sessions, 1, "one durable session contributed one sample");
    let p50 = capability(&after, "diagnostics.latency.p50_us")
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or("a retained sample must publish a p50")?;
    assert!(p50 > 0, "a measured duration is never zero");
    assert_eq!(capability(&after, "diagnostics.health.overall"), Some("ok"));
    assert_eq!(
        capability(&after, "diagnostics.health.subsystems"),
        Some("1")
    );

    // No advertised fact may carry the transcript, the audio, or any
    // environment content — the exact REL-008 exclusion.
    let rendered = after.capabilities.join("\n");
    for (label, canary) in [
        ("transcript", delivered.as_str()),
        ("transcript fragment", "mock asr"),
        ("engine transcript", TRANSCRIPT_CANARY),
        ("audio", AUDIO_CANARY_MARKER),
        ("environment", "NARRIVY_"),
    ] {
        assert!(
            !rendered.contains(canary),
            "an advertised capability carried {label} content:\n{rendered}"
        );
    }
    // The state the daemon advertises is bounded closed vocabulary, and the
    // diagnostics surface holds no free-form message anywhere.
    for value in &after.capabilities {
        assert!(value.len() <= 96, "unbounded capability value: {value}");
        assert!(
            value
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || "=:_.-".contains(ch)),
            "capability value is not a closed token: {value}"
        );
    }
    harness.shutdown()
}

/// A daemon that does not compose the surface fails closed: the gate is the
/// only diagnostics token, and no health or latency fact is published.
#[test]
fn an_uncomposed_daemon_fails_closed_on_the_support_report_gate() -> TestResult {
    let dir = private_tempdir()?;
    let history = Arc::new(open_retained_history(dir.path(), RetentionPolicy::Days7)?);
    let harness = start_harness(dir.path(), Some(Arc::clone(&history)), false)?;
    let mut client = harness.connect()?;

    let before = status(&mut client)?;
    assert!(
        before
            .capabilities
            .iter()
            .any(|value| value == SUPPORT_REPORT_NOT_COMPOSED_CAPABILITY),
        "{:?}",
        before.capabilities
    );
    assert!(
        !before
            .capabilities
            .iter()
            .any(|value| value == SUPPORT_REPORT_AVAILABLE_CAPABILITY),
        "{:?}",
        before.capabilities
    );

    // A real session still runs and is retained; only the report facts are
    // absent, and they are absent for the whole generation of the daemon.
    dictate(&mut client)?;
    let after = status(&mut client)?;
    let diagnostics: Vec<&String> = after
        .capabilities
        .iter()
        .filter(|value| value.starts_with("diagnostics."))
        .collect();
    assert_eq!(
        diagnostics,
        [&SUPPORT_REPORT_NOT_COMPOSED_CAPABILITY.to_owned()],
        "an uncomposed surface must publish no facts at all"
    );
    let ResponsePayload::History(rows) = client.request(&Command::GetHistory, REQ_TIMEOUT)? else {
        return Err("expected History payload".into());
    };
    assert_eq!(rows.len(), 1, "the session itself is unaffected");
    harness.shutdown()
}

/// The daemon's own redaction facility removes denied content from a free-form
/// diagnostic message, so a hostile engine message cannot reach a log sink.
#[test]
fn a_hostile_engine_message_is_redacted_before_it_reaches_a_log_sink() -> TestResult {
    use daemon_bin::content_safe;

    let hostile = format!(
        "asr engine failed: text={TRANSCRIPT_CANARY} audio={AUDIO_CANARY_MARKER} secret=hunter2"
    );
    let scrubbed = content_safe(&hostile);
    assert!(!scrubbed.contains(TRANSCRIPT_CANARY), "{scrubbed}");
    assert!(!scrubbed.contains(AUDIO_CANARY_MARKER), "{scrubbed}");
    assert!(!scrubbed.contains("hunter2"), "{scrubbed}");
    assert!(scrubbed.contains("[REDACTED:"), "{scrubbed}");
    // An ordinary content-free message is unchanged.
    assert_eq!(
        content_safe("session 7 failed: worker_failure"),
        "session 7 failed: worker_failure"
    );
    // The same fact stored in the journal is a closed code, never a message.
    let dir = private_tempdir()?;
    let journal: Arc<Journal> = Arc::new(daemon_bin::open_journal(dir.path())?);
    let harness = start_harness(dir.path(), None, true)?;
    let mut client = harness.connect()?;
    let txid = dictate(&mut client)?;
    let entry = journal
        .read(&txid)
        .map_err(|error| format!("journal read failed: {error:?}"))?
        .ok_or("the committed transaction must be journaled")?;
    if let Some(code) = entry.failure_code {
        assert!(
            !code.contains(TRANSCRIPT_CANARY) && code.len() <= 64,
            "a journal failure code must stay a closed identifier: {code}"
        );
    }
    harness.shutdown()
}
