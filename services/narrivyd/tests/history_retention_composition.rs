//! PRIV-002 evidence: the optional retained History store is really composed
//! by the daemon, really written by a finished dictation, and really read back
//! over the same unix socket.
//!
//! The daemon wiring (session orchestration + ipc command handler + event
//! pump) is the binary's own code, included verbatim, exactly as
//! `e2e_mock_dictation.rs` does it. Retention TTL and count bounds and Privacy
//! Zone precedence are driven through the same store the daemon composes, with
//! an injected instant so the evidence does not depend on the wall clock.

#![forbid(unsafe_code)]

#[allow(dead_code)] // includes the binary's never-called main()
mod daemon_bin {
    include!("../src/main.rs");
}

use std::error::Error;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use daemon_bin::daemon::retained_history::{
    HISTORY_LIST_LIMIT, MAX_HISTORY_ENTRIES, RecordOutcome, RetainedHistory, open_retained_history,
    open_retained_history_with_clock,
};
use daemon_bin::{DaemonState, build_handler, spawn_event_pump};
use narrivy_core::{Journal, JournalEntryState};
use narrivy_history::{PrivacyZone, PrivacyZoneAction, RetentionPolicy, ZoneMatcher};
use narrivy_ipc::{Command, Event, IpcErrorCode, ResponsePayload};
use narrivy_ipc_unix::{IpcClient, UnixIpcServer};

/// Deterministic mock-lane delivery result. The fixture text lives in this
/// test file only; it IS the fixture.
const EXPECTED_DELIVERED: &str = "Hello world from mock asr.";

const REQ_TIMEOUT: Duration = Duration::from_secs(2);
const EVENT_WAIT: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Fixed instant for the synthetic-time retention evidence.
const NOW: u64 = 1_700_000_000_000;

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

fn private_tempdir() -> TestResult<tempfile::TempDir> {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir()?;
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
    Ok(dir)
}

/// One in-process daemon with a composed retained History store, bound to
/// `<dir>/narrivyd.sock`. `retention` selects the composed policy, so the
/// global disable is exercised through the real composition root.
struct Harness {
    server: Arc<UnixIpcServer>,
    journal: Arc<Journal>,
    state: Arc<DaemonState>,
    history: Option<Arc<RetainedHistory>>,
    socket: std::path::PathBuf,
    serve_handle: std::thread::JoinHandle<()>,
    pump_handle: std::thread::JoinHandle<()>,
}

fn start_harness(dir: &Path, retention: Option<RetentionPolicy>) -> TestResult<Harness> {
    let journal = Arc::new(daemon_bin::open_journal(dir)?);
    let history = match retention {
        Some(policy) => Some(Arc::new(open_retained_history(dir, policy)?)),
        None => None,
    };
    let state = Arc::new(DaemonState::new());
    let sink: daemon_bin::DeliverySink = Arc::new(std::sync::Mutex::new(Vec::new()));
    let supervisor = Arc::new(narrivy_core::WorkerSupervisor::new());
    let socket = dir.join("narrivyd.sock");
    let server = Arc::new(UnixIpcServer::bind(&socket)?);
    let handler = build_handler(
        state.clone(),
        journal.clone(),
        history.clone(),
        sink,
        supervisor,
        None,
        Arc::new(daemon_bin::EngineConfig::new()),
        None,
    );
    let pump = spawn_event_pump(server.clone(), state.clone());
    let serve_server = server.clone();
    let serve_handle = std::thread::spawn(move || {
        let _ = serve_server.serve(handler);
    });
    Ok(Harness {
        server,
        journal,
        state,
        history,
        socket,
        serve_handle,
        pump_handle: pump,
    })
}

impl Harness {
    fn connect(&self) -> TestResult<IpcClient> {
        Ok(IpcClient::connect_as(&self.socket, "history-test")?)
    }

    fn shutdown(self) -> TestResult {
        self.server.request_shutdown();
        self.serve_handle.join().map_err(|_| "serve panicked")?;
        daemon_bin::stop_event_pump(&self.state);
        self.pump_handle.join().map_err(|_| "pump panicked")?;
        Ok(())
    }

    /// The exact transaction whose journal record is `delivered`.
    fn delivered_txid(&self) -> TestResult<String> {
        Ok(self
            .journal
            .entries()?
            .into_iter()
            .find(|entry| entry.state == JournalEntryState::Delivered)
            .ok_or("no delivered journal record")?
            .txid)
    }
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
    committed.ok_or_else(|| "session never reached committed".into())
}

fn capability_values(client: &mut IpcClient) -> TestResult<Vec<String>> {
    let ResponsePayload::Status(status) = client.request(&Command::GetStatus, REQ_TIMEOUT)? else {
        return Err("expected Status payload".into());
    };
    Ok(status.capabilities)
}

/// A committed dictation is retained by the composed store and read back over
/// the socket, and the advertised capability is the one the desktop gates on.
#[test]
fn committed_dictation_is_retained_and_served_over_the_socket() -> TestResult {
    let dir = private_tempdir()?;
    let harness = start_harness(dir.path(), Some(RetentionPolicy::Days7))?;
    let mut client = harness.connect()?;

    let capabilities = capability_values(&mut client)?;
    assert!(
        capabilities
            .iter()
            .any(|value| value == "history.optional=enabled"),
        "a composed store must advertise itself: {capabilities:?}"
    );
    assert!(
        capabilities
            .iter()
            .any(|value| value == "history.retention=7d"),
        "the effective retention must be reported: {capabilities:?}"
    );
    assert_eq!(
        harness
            .history
            .as_ref()
            .map(|service| service.count())
            .transpose()?,
        Some(0),
        "nothing is retained before the first dictation"
    );

    let txid = dictate(&mut client)?;
    assert_eq!(harness.delivered_txid()?, txid);

    // Listing row: content-safe fields only. History is written just after
    // the result is published, so wait (bounded) for the row rather than
    // racing the `committed` event (failed about 1 run in 20).
    let deadline = Instant::now() + EVENT_WAIT;
    let rows = loop {
        let ResponsePayload::History(rows) = client.request(&Command::GetHistory, REQ_TIMEOUT)?
        else {
            return Err("expected History payload".into());
        };
        if !rows.is_empty() || Instant::now() >= deadline {
            break rows;
        }
        std::thread::sleep(POLL_INTERVAL);
    };
    assert_eq!(rows.len(), 1, "exactly the committed dictation is retained");
    let row = rows
        .iter()
        .find(|row| row.txid == txid)
        .ok_or("the committed transaction is missing from History")?;
    assert_eq!(row.state, "delivered");
    assert!(row.has_delivered_text);
    assert!(row.updated_unix_ms > 0);
    let rendered = format!("{row:?}");
    assert!(
        !rendered.contains(EXPECTED_DELIVERED),
        "a listing row must never carry transcript text: {rendered}"
    );

    // Explicit reveal of the named row returns the retained variants.
    let ResponsePayload::HistoryEntry(detail) = client.request(
        &Command::GetHistoryEntry { txid: txid.clone() },
        REQ_TIMEOUT,
    )?
    else {
        return Err("expected HistoryEntry payload".into());
    };
    assert_eq!(detail.txid, txid);
    assert_eq!(detail.delivered.as_deref(), Some(EXPECTED_DELIVERED));
    assert_eq!(detail.state, "delivered");
    assert!(
        detail.capture_end.is_none(),
        "the retained schema stores no capture end"
    );

    // The recovered-but-undelivered transaction is a different store, and an
    // unknown id is a typed rejection rather than an empty row.
    let error = match client.request(
        &Command::GetHistoryEntry {
            txid: "tx-not-retained".to_owned(),
        },
        REQ_TIMEOUT,
    ) {
        Ok(_) => return Err("an unrecorded transaction must be rejected".into()),
        Err(error) => error,
    };
    assert_eq!(error.code, IpcErrorCode::UnknownCommand);
    let error = match client.request(
        &Command::GetHistoryEntry {
            txid: "../escape".to_owned(),
        },
        REQ_TIMEOUT,
    ) {
        Ok(_) => return Err("a traversal-shaped txid must be rejected".into()),
        Err(error) => error,
    };
    assert_eq!(error.code, IpcErrorCode::BadRequest);

    drop(client);
    harness.shutdown()
}

/// PRIV-002 global disable: with `off` composed, a finished dictation is not
/// retained and the History listing is empty, while the recovery journal keeps
/// its own record (disabling History must not make Narrivy fragile).
#[test]
fn global_retention_off_records_nothing_while_the_journal_still_recovers() -> TestResult {
    let dir = private_tempdir()?;
    let harness = start_harness(dir.path(), Some(RetentionPolicy::Off))?;
    let mut client = harness.connect()?;

    let capabilities = capability_values(&mut client)?;
    assert!(
        capabilities
            .iter()
            .any(|value| value == "history.optional=enabled"),
        "the store is still composed: {capabilities:?}"
    );
    assert!(
        capabilities
            .iter()
            .any(|value| value == "history.retention=off"),
        "the disable must be reported truthfully: {capabilities:?}"
    );

    let txid = dictate(&mut client)?;
    // The recovery journal is independent of the History switch.
    assert_eq!(harness.delivered_txid()?, txid);
    let journal_entry = harness
        .journal
        .read(&txid)?
        .ok_or("the recovery journal lost its record")?;
    assert_eq!(
        journal_entry.variants.delivered.as_deref(),
        Some(EXPECTED_DELIVERED)
    );

    let ResponsePayload::History(rows) = client.request(&Command::GetHistory, REQ_TIMEOUT)? else {
        return Err("expected History payload".into());
    };
    assert!(
        rows.is_empty(),
        "a disabled store must retain nothing: {rows:?}"
    );
    let error = match client.request(
        &Command::GetHistoryEntry { txid: txid.clone() },
        REQ_TIMEOUT,
    ) {
        Ok(_) => return Err("a disabled store must not reveal a transaction".into()),
        Err(error) => error,
    };
    assert_eq!(error.code, IpcErrorCode::UnknownCommand);

    drop(client);
    harness.shutdown()
}

/// With no store composed, History stays fail-closed and the capability keeps
/// saying so, while every other command still serves.
#[test]
fn an_uncomposed_store_keeps_history_fail_closed() -> TestResult {
    let dir = private_tempdir()?;
    let harness = start_harness(dir.path(), None)?;
    let mut client = harness.connect()?;

    let capabilities = capability_values(&mut client)?;
    assert!(
        capabilities
            .iter()
            .any(|value| value == "history.optional=disabled:not_composed"),
        "{capabilities:?}"
    );
    assert!(
        !capabilities
            .iter()
            .any(|value| value.starts_with("history.retention=")),
        "an uncomposed store must not report a retention fact: {capabilities:?}"
    );

    // A real dictation still commits and is still recovered.
    let txid = dictate(&mut client)?;
    assert_eq!(harness.delivered_txid()?, txid);

    for command in [
        Command::GetHistory,
        Command::GetHistoryEntry { txid: txid.clone() },
    ] {
        let error = match client.request(&command, REQ_TIMEOUT) {
            Ok(_) => return Err("uncomposed History must fail closed".into()),
            Err(error) => error,
        };
        assert_eq!(error.code, IpcErrorCode::BadRequest);
        assert_eq!(
            error.message,
            "optional history is disabled and not composed"
        );
    }

    drop(client);
    harness.shutdown()
}

fn committed(txid: &str, text: &str, at_ms: u64) -> narrivy_core::JournalEntry {
    let mut entry = narrivy_core::JournalEntry::new_pending(txid, at_ms);
    entry.state = JournalEntryState::Delivered;
    entry.pipeline_id = Some("mock-dictation".to_owned());
    entry.variants.raw = Some(text.to_owned());
    entry.variants.corrected = Some(text.to_owned());
    entry.variants.delivered = Some(text.to_owned());
    entry.updated_unix_ms = at_ms;
    entry
}

/// TTL: a row older than the composed policy's window is not retained and not
/// reported, while a fresh one is.
#[test]
fn retention_ttl_expires_rows_outside_the_composed_window() -> TestResult {
    let dir = private_tempdir()?;
    let service = open_retained_history_with_clock(dir.path(), RetentionPolicy::Days7, NOW)?;
    let day = 24 * 60 * 60 * 1000;

    assert_eq!(
        service.record_committed(&committed("tx-old", "old", NOW - 8 * day), "app")?,
        RecordOutcome::Recorded
    );
    assert_eq!(
        service.record_committed(&committed("tx-new", "new", NOW - day), "app")?,
        RecordOutcome::Recorded
    );

    let listed = service.list(HISTORY_LIST_LIMIT)?;
    assert_eq!(listed.len(), 1, "the expired row must not be reported");
    assert_eq!(listed[0].id, "tx-new");
    assert_eq!(service.entry("tx-old")?.map(|entry| entry.id), None);
    assert_eq!(service.count()?, 1);
    Ok(())
}

/// Count bound: the retained store never exceeds the composed cap, and the
/// newest rows are the ones kept.
#[test]
fn retention_count_bound_keeps_the_store_bounded() -> TestResult {
    let dir = private_tempdir()?;
    let service = open_retained_history_with_clock(dir.path(), RetentionPolicy::Forever, NOW)?;
    for index in 0..=MAX_HISTORY_ENTRIES {
        let txid = format!("tx-{index:04}");
        assert_eq!(
            service.record_committed(&committed(&txid, "text", NOW + index as u64), "app")?,
            RecordOutcome::Recorded
        );
    }
    assert_eq!(service.count()?, MAX_HISTORY_ENTRIES);
    assert_eq!(
        service.entry("tx-0000")?.map(|entry| entry.id),
        None,
        "the oldest row falls outside the count bound"
    );
    assert!(
        service
            .entry(&format!("tx-{:04}", MAX_HISTORY_ENTRIES))?
            .is_some(),
        "the newest row survives the count bound"
    );
    Ok(())
}

/// Privacy Zone precedence: a persisted deny zone wins over an allow zone for
/// the same application, and produces no retained row at all.
#[test]
fn a_persisted_privacy_zone_deny_overrides_an_allow_zone() -> TestResult {
    let dir = private_tempdir()?;
    {
        // Zones live in the same private store and are loaded by composition,
        // so they are proven through the real persisted path.
        let service = open_retained_history_with_clock(dir.path(), RetentionPolicy::Days7, NOW)?;
        ZoneMatcher::new(vec![
            PrivacyZone::new_deny("allow-bank", vec!["com.bank.app".to_owned()], false),
            PrivacyZone::new_deny("deny-bank", vec!["com.bank.app".to_owned()], true),
        ])
        .save_to_db(service.db_path())?;
    }
    let service = open_retained_history_with_clock(dir.path(), RetentionPolicy::Days7, NOW)?;

    assert_eq!(
        service.record_committed(&committed("tx-bank", "bank secret", NOW), "com.bank.app")?,
        RecordOutcome::DeniedByZone,
        "a deny zone must override an allow zone for the same application"
    );
    assert_eq!(
        service.record_committed(
            &committed("tx-editor", "editor text", NOW),
            "com.example.editor"
        )?,
        RecordOutcome::Recorded
    );
    assert_eq!(service.count()?, 1);
    assert_eq!(service.entry("tx-bank")?.map(|entry| entry.id), None);

    let listed = service.list(HISTORY_LIST_LIMIT)?;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, "tx-editor");
    Ok(())
}

/// A zone deny still wins when the global retention would have kept the row,
/// and an application that matched no zone is retained normally.
#[test]
fn a_zone_deny_wins_over_an_enabled_retention_policy() -> TestResult {
    let dir = private_tempdir()?;
    {
        let service = open_retained_history_with_clock(dir.path(), RetentionPolicy::Forever, NOW)?;
        ZoneMatcher::new(vec![PrivacyZone {
            id: "deny-secret".to_owned(),
            pattern: "secret".to_owned(),
            app_ids: vec![],
            deny: true,
            actions: vec![PrivacyZoneAction::DenyHistory],
        }])
        .save_to_db(service.db_path())?;
    }
    let service = open_retained_history_with_clock(dir.path(), RetentionPolicy::Forever, NOW)?;

    assert_eq!(
        service.record_committed(&committed("tx-secret", "secret", NOW), "com.secret.app")?,
        RecordOutcome::DeniedByZone
    );
    assert_eq!(
        service.record_committed(&committed("tx-plain", "plain", NOW), "com.plain.app")?,
        RecordOutcome::Recorded
    );
    assert_eq!(service.count()?, 1);
    Ok(())
}
