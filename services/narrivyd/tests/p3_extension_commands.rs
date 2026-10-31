//! Desktop extension commands over a real Unix socket. Optional History fails
//! closed and never reads recovery storage; exact recovery-result access uses
//! its own transaction-scoped command.

#![forbid(unsafe_code)]

#[allow(dead_code)] // includes the binary's never-called main()
mod daemon_bin {
    include!("../src/main.rs");
}

use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use daemon_bin::{DaemonState, build_handler, spawn_event_pump};
use narrivy_core::{Journal, JournalEntry};
use narrivy_ipc::{Command, IpcErrorCode, ResponsePayload};
use narrivy_ipc_unix::{IpcClient, UnixIpcServer};

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

fn private_tempdir() -> TestResult<tempfile::TempDir> {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir()?;
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
    Ok(dir)
}

struct Harness {
    server: Arc<UnixIpcServer>,
    journal: Arc<Journal>,
    state: Arc<DaemonState>,
    serve_handle: std::thread::JoinHandle<()>,
    pump_handle: std::thread::JoinHandle<()>,
    socket: std::path::PathBuf,
}

fn start_harness(dir: &std::path::Path) -> TestResult<Harness> {
    let journal = Arc::new(daemon_bin::open_journal(dir)?);
    let state = Arc::new(DaemonState::new());
    let sink: daemon_bin::DeliverySink = Arc::new(std::sync::Mutex::new(Vec::new()));
    let supervisor = Arc::new(narrivy_core::WorkerSupervisor::new());
    let socket = dir.join("narrivyd.sock");
    let server = Arc::new(UnixIpcServer::bind(&socket)?);
    let handler = build_handler(
        state.clone(),
        journal.clone(),
        None,
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
        serve_handle,
        pump_handle: pump,
        socket,
    })
}

impl Harness {
    fn shutdown(self) -> TestResult {
        self.server.request_shutdown();
        self.serve_handle.join().map_err(|_| "serve panicked")?;
        daemon_bin::stop_event_pump(&self.state);
        self.pump_handle.join().map_err(|_| "pump panicked")?;
        Ok(())
    }
}

const REQ_TIMEOUT: Duration = Duration::from_secs(2);

fn connect(socket: &std::path::Path) -> TestResult<IpcClient> {
    Ok(IpcClient::connect_as(socket, "desktop-test")?)
}

/// One delivered fixture entry with text that must stay out of listings.
fn write_fixture_entry(journal: &Journal) -> TestResult<()> {
    let mut entry = JournalEntry::new_pending("tx-p3-fixture", 1_000);
    entry.variants.raw = Some("P3-SECRET-RAW".to_owned());
    entry.variants.corrected = Some("P3-SECRET-CORRECTED".to_owned());
    entry.variants.delivered = Some("P3-SECRET-DELIVERED".to_owned());
    entry.pipeline_id = Some("mock-dictation".to_owned());
    entry.updated_unix_ms = 2_000;
    journal.write(&entry)?;
    Ok(())
}

#[test]
fn optional_history_commands_fail_closed_without_touching_recovery() -> TestResult {
    let dir = private_tempdir()?;
    let harness = start_harness(dir.path())?;
    write_fixture_entry(&harness.journal)?;
    let journal_path = dir.path().join("journal/tx-p3-fixture.json");
    let before = std::fs::read(&journal_path)?;
    let mut client = connect(&harness.socket)?;

    let ResponsePayload::Status(status) = client.request(&Command::GetStatus, REQ_TIMEOUT)? else {
        return Err("expected Status payload".into());
    };
    assert!(
        status
            .capabilities
            .iter()
            .any(|value| value == "history.optional=disabled:not_composed")
    );

    for command in [
        Command::GetHistory,
        Command::GetHistoryEntry {
            txid: "tx-p3-fixture".to_owned(),
        },
    ] {
        let error = match client.request(&command, REQ_TIMEOUT) {
            Ok(_) => return Err("optional History must be disabled".into()),
            Err(error) => error,
        };
        assert_eq!(error.code, IpcErrorCode::BadRequest);
        assert_eq!(
            error.message,
            "optional history is disabled and not composed"
        );
        for secret in [
            "tx-p3-fixture",
            "P3-SECRET-RAW",
            "P3-SECRET-CORRECTED",
            "P3-SECRET-DELIVERED",
        ] {
            assert!(!error.message.contains(secret));
        }
    }
    assert_eq!(std::fs::read(journal_path)?, before);
    harness.shutdown()
}

#[test]
fn exact_recovery_entry_returns_only_the_named_transaction() -> TestResult {
    let dir = private_tempdir()?;
    let harness = start_harness(dir.path())?;
    write_fixture_entry(&harness.journal)?;
    let mut client = connect(&harness.socket)?;

    let ResponsePayload::RecoveryEntry(detail) = client.request(
        &Command::GetRecoveryEntry {
            txid: "tx-p3-fixture".to_owned(),
        },
        REQ_TIMEOUT,
    )?
    else {
        return Err("expected RecoveryEntry payload".into());
    };
    assert_eq!(detail.txid, "tx-p3-fixture");
    assert_eq!(detail.raw.as_deref(), Some("P3-SECRET-RAW"));
    assert_eq!(detail.corrected.as_deref(), Some("P3-SECRET-CORRECTED"));
    assert_eq!(detail.delivered.as_deref(), Some("P3-SECRET-DELIVERED"));

    // Unknown txid is a typed rejection, not a panic and not an empty row.
    let err = match client.request(
        &Command::GetRecoveryEntry {
            txid: "tx-missing".to_owned(),
        },
        REQ_TIMEOUT,
    ) {
        Ok(_) => return Err("unknown txid must be rejected".into()),
        Err(err) => err,
    };
    assert_eq!(err.code, IpcErrorCode::UnknownCommand);

    // Invalid (path-traversal) txid is BadRequest via the journal validator.
    let err = match client.request(
        &Command::GetRecoveryEntry {
            txid: "../escape".to_owned(),
        },
        REQ_TIMEOUT,
    ) {
        Ok(_) => return Err("invalid txid must be rejected".into()),
        Err(err) => err,
    };
    assert_eq!(err.code, IpcErrorCode::BadRequest);
    harness.shutdown()
}

#[test]
fn p3_run_capability_check_closed_set_and_truthful_results() -> TestResult {
    let dir = private_tempdir()?;
    let harness = start_harness(dir.path())?;
    let mut client = connect(&harness.socket)?;

    // Unknown id -> truthful unavailable detail (closed set).
    let ResponsePayload::CapabilityCheck(dto) = client.request(
        &Command::RunCapabilityCheck {
            check_id: "not-a-check".to_owned(),
        },
        REQ_TIMEOUT,
    )?
    else {
        return Err("expected CapabilityCheck payload".into());
    };
    assert_eq!(dto.id, "not-a-check");
    assert!(!dto.available);
    assert!(dto.detail.contains("unknown id"));

    // theme-store over the socket: shape + content-free detail (the boolean
    // depends on host state; the deterministic tempdir-pass case is proven by
    // run_capability_check_with below).
    let ResponsePayload::CapabilityCheck(theme_dto) = client.request(
        &Command::RunCapabilityCheck {
            check_id: "theme-store".to_owned(),
        },
        REQ_TIMEOUT,
    )?
    else {
        return Err("expected CapabilityCheck payload".into());
    };
    assert_eq!(theme_dto.id, "theme-store");
    assert!(!theme_dto.detail.is_empty());

    // Deterministic test-lane pass: injected theme path in a fresh tempdir.
    let theme_file = dir.path().join("config/theme.json");
    let direct = daemon_bin::run_capability_check_with("theme-store", Some(&theme_file));
    assert!(
        direct.available,
        "injected theme lane should pass: {}",
        direct.detail
    );

    // Helper trust alone cannot pass a microphone check. The result stays
    // fail-closed until signal/routing are actually exercised.
    let ResponsePayload::CapabilityCheck(mic) = client.request(
        &Command::RunCapabilityCheck {
            check_id: "mic".to_owned(),
        },
        REQ_TIMEOUT,
    )?
    else {
        return Err("expected CapabilityCheck payload".into());
    };
    assert!(!mic.available);
    assert!(mic.detail.contains("pw-record"));

    // Malformed id shape -> BadRequest (bounds identical to other handlers).
    let err = match client.request(
        &Command::RunCapabilityCheck {
            check_id: format!("x{}", "y".repeat(64)),
        },
        REQ_TIMEOUT,
    ) {
        Ok(_) => return Err("oversized id must be rejected".into()),
        Err(err) => err,
    };
    assert_eq!(err.code, IpcErrorCode::BadRequest);
    harness.shutdown()
}

#[test]
fn p3_list_pipelines_unchanged_for_plain_clients() -> TestResult {
    let dir = private_tempdir()?;
    let harness = start_harness(dir.path())?;
    let mut client = connect(&harness.socket)?;
    let ResponsePayload::Pipelines(pipelines) =
        client.request(&Command::ListPipelines, REQ_TIMEOUT)?
    else {
        return Err("expected Pipelines payload".into());
    };
    // P7: the built-in Clean and Raw modes; the Clean default stays first.
    assert_eq!(pipelines.len(), 2);
    assert_eq!(pipelines[0].id, "clean");
    assert!(pipelines[0].enabled);
    assert!(pipelines.iter().any(|p| p.id == "raw"));
    harness.shutdown()
}

#[test]
fn command_rate_limit_survives_reconnect_and_ignores_claimed_role() -> TestResult {
    let dir = private_tempdir()?;
    let harness = start_harness(dir.path())?;
    let command = Command::StartOfflineModelProvision {
        source_path: dir.path().join("offline-model").display().to_string(),
    };

    // The production policy permits a burst of two starts per daemon minute.
    // This test uses a command that is intentionally uncomposed in this
    // harness, so it exercises policy without publishing or importing data.
    for role in ["desktop-test", "cli-test"] {
        let mut client = IpcClient::connect_as(&harness.socket, role)?;
        let error = match client.request(&command, REQ_TIMEOUT) {
            Ok(_) => return Err("uncomposed provisioning must reject".into()),
            Err(error) => error,
        };
        assert_eq!(error.code, IpcErrorCode::DaemonUnavailable);
    }

    // A new connection claiming an elevated-looking role shares the same
    // exact-command bucket. Role strings are routing diagnostics, not identity.
    let mut spoofed = IpcClient::connect_as(&harness.socket, "admin")?;
    let error = match spoofed.request(&command, REQ_TIMEOUT) {
        Ok(_) => return Err("third request must be rate limited".into()),
        Err(error) => error,
    };
    assert_eq!(error.code, IpcErrorCode::Busy);
    assert_eq!(error.message, "state-changing command rate limit exceeded");
    drop(spoofed);
    harness.shutdown()?;

    // Policy windows are daemon-generation state, not persistent security
    // credentials. An explicit daemon restart starts a fresh bounded window.
    let restarted = start_harness(dir.path())?;
    let mut client = IpcClient::connect_as(&restarted.socket, "admin")?;
    let error = match client.request(&command, REQ_TIMEOUT) {
        Ok(_) => return Err("uncomposed provisioning must still reject after restart".into()),
        Err(error) => error,
    };
    assert_eq!(error.code, IpcErrorCode::DaemonUnavailable);
    restarted.shutdown()
}
