//! REL-008 evidence, shipped-binary half: the installed `narrivy-cli` really
//! produces a support report over the real unix socket, and the report a user
//! would attach to a bug report contains no transcript, audio, clipboard or
//! environment content.
//!
//! This runs the compiled binary (`CARGO_BIN_EXE_narrivy-cli`) against a real
//! `UnixIpcServer` that answers `GetStatus` with a scripted, deliberately
//! hostile capability set: every value that could carry content is planted as a
//! canary, and the report must still be content-free. The daemon-side half — a
//! real daemon that composes the surface and advertises real facts after a real
//! dictation — is `services/narrivyd/tests/support_report_composition.rs`.

#![forbid(unsafe_code)]
#![allow(clippy::expect_used)]

use std::path::PathBuf;
use std::sync::Arc;

use narrivy_diagnostics::SupportReport;
use narrivy_ipc::{Command, Envelope, MessageKind, ResponsePayload, StatusDto};
use narrivy_ipc_unix::{PeerCredentials, UnixIpcServer};

/// Content canaries planted in every field a client could plausibly copy.
const CANARIES: [&str; 4] = [
    "canary-transcript-alpha",
    "canary-audio-pcm-bravo",
    "canary-clipboard-charlie",
    "canary-context-delta",
];

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A daemon endpoint that answers exactly one command kind.
struct FakeDaemon {
    _dir: tempfile::TempDir,
    socket: PathBuf,
    server: Arc<UnixIpcServer>,
    serve: Option<std::thread::JoinHandle<()>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gate {
    Composed,
    NotComposed,
}

impl FakeDaemon {
    fn start(gate: Gate) -> TestResult<FakeDaemon> {
        let dir = tempfile::tempdir()?;
        let socket = dir.path().join("narrivyd.sock");
        let server = Arc::new(UnixIpcServer::bind(&socket)?);
        let serve_server = Arc::clone(&server);
        let serve = std::thread::spawn(move || {
            let _ = serve_server.serve(move |_credentials: PeerCredentials, envelope: Envelope| {
                let request_id = envelope.request_id;
                let command: Command = envelope.parse_payload().ok()?;
                let payload = match command {
                    Command::GetStatus => ResponsePayload::Status(hostile_status(gate)),
                    // A support report must never ask for session content. If
                    // it ever did, a rejection here fails the test loudly
                    // instead of silently returning text.
                    other => ResponsePayload::Rejected {
                        code: narrivy_ipc::IpcErrorCode::UnknownCommand,
                        detail: format!("unexpected command: {other:?}"),
                    },
                };
                Envelope::new(MessageKind::Response, request_id)
                    .with_payload(&payload)
                    .ok()
            });
        });
        Ok(FakeDaemon {
            _dir: dir,
            socket,
            server,
            serve: Some(serve),
        })
    }

    fn socket(&self) -> String {
        self.socket.to_string_lossy().into_owned()
    }

    fn stop(mut self) {
        self.server.request_shutdown();
        if let Some(serve) = self.serve.take() {
            let _ = serve.join();
        }
    }
}

/// A status snapshot whose every content-bearing slot carries a canary.
fn hostile_status(gate: Gate) -> StatusDto {
    let (gate_capability, extra) = match gate {
        Gate::Composed => (
            "diagnostics.support-report=available",
            vec![
                // Hostile values a real daemon never emits. A client that
                // copied a value verbatim would leak them.
                format!("diagnostics.notes=text={}", CANARIES[0]),
                format!("delivery.rung.audio-samples={}", CANARIES[1]),
                format!("history.clipboard={}", CANARIES[2]),
                format!("recognition.context_hint={}", CANARIES[3]),
                "diagnostics.latency.sessions=4".to_owned(),
                "diagnostics.latency.p50_us=250000".to_owned(),
                "diagnostics.latency.p95_us=900000".to_owned(),
            ],
        ),
        Gate::NotComposed => (
            "diagnostics.support-report=unavailable:not_composed",
            Vec::new(),
        ),
    };
    let mut capabilities = vec![
        "mock.asr".to_owned(),
        "null.delivery".to_owned(),
        "history.optional=enabled".to_owned(),
        "history.retention=7d".to_owned(),
        "activation.atomic-command=available".to_owned(),
        "activation.portal-global-shortcuts=unavailable:not_probed".to_owned(),
        gate_capability.to_owned(),
        "diagnostics.health.overall=ok".to_owned(),
        "diagnostics.health.subsystems=2".to_owned(),
    ];
    capabilities.extend(extra);
    StatusDto {
        daemon_build: "0.0.0".to_owned(),
        protocol_minor: narrivy_ipc::PROTOCOL_MINOR,
        // An active session is present, so the report is produced from a
        // running daemon rather than an idle one.
        active_session: Some(narrivy_ipc::ActiveSessionDto {
            session_id: 4_294_967_296,
            transaction_id: Some(format!("tx-{}", CANARIES[0])),
            pipeline_id: Some(CANARIES[1].to_owned()),
            state: "listening".to_owned(),
            stage_timings_us: Some(narrivy_ipc::StageTimingsUs(
                [(String::from("capture_start"), 12_345_u64)]
                    .into_iter()
                    .collect(),
            )),
        }),
        last_session_timings_us: None,
        capabilities,
        installed_model: None,
        portal_keyboard: None,
        model_provisioning: None,
    }
}

/// Run the shipped binary and return its stdout.
fn run_binary(args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_narrivy-cli"))
        .args(args)
        .env("NARRIVY_TRANSCRIPT", CANARIES[0])
        .env("NARRIVY_TOKEN", CANARIES[2])
        .env("LC_ALL", "C")
        .output()
        .expect("the installed CLI binary must be runnable")
}

/// The installed binary produces a support report on stdout, and the report a
/// user would share carries no canary from any content-bearing input.
#[test]
fn the_shipped_binary_produces_a_content_free_support_report() -> TestResult {
    let daemon = FakeDaemon::start(Gate::Composed)?;
    let output = run_binary(&["support-report", "--socket", &daemon.socket()]);
    daemon.stop();

    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout)?;
    // The report is a real, parseable support report — not a stub message.
    let report = SupportReport::from_json_str(&stdout)
        .map_err(|error| format!("stdout is not a support report: {error}\n{stdout}"))?;
    assert_eq!(report.schema_version, 1);
    assert!(report.generated_unix_ms > 0, "the report is time-stamped");
    assert_eq!(report.build_id, "0.0.0");
    assert_eq!(report.latency.count, 4);
    assert_eq!(report.latency.p95_micros, Some(900_000));
    assert!(!report.health.is_empty(), "the report carries health facts");
    assert!(
        report
            .health
            .iter()
            .any(|fact| fact.subsystem == "diagnostics" && fact.code == "support-report"),
        "the gate capability is reported as a fact: {:?}",
        report.health
    );
    // The report is available on disk, owner-only, for a bug attachment.
    for canary in CANARIES {
        assert!(
            !stdout.contains(canary),
            "a support report must never carry {canary}:\n{stdout}"
        );
    }
    // The content-safety verdict is reported to the user, not silently assumed.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("content_check=clean denied_fields=0"),
        "the shipped command must state its own content-safety verdict: {stderr}"
    );
    Ok(())
}

/// The report the shipped binary writes to `--output` is the same content-free
/// report, and the file is owner-only.
#[test]
fn the_shipped_binary_writes_an_owner_only_report_file() -> TestResult {
    let daemon = FakeDaemon::start(Gate::Composed)?;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("support-report.json");
    let output = run_binary(&[
        "support-report",
        "--socket",
        &daemon.socket(),
        "--output",
        path.to_str().ok_or("temp path is not utf-8")?,
    ]);
    daemon.stop();
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let written = std::fs::read_to_string(&path)?;
    let report = SupportReport::from_json_str(&written)
        .map_err(|error| format!("written file is not a support report: {error}"))?;
    assert_eq!(report.latency.count, 4);
    for canary in CANARIES {
        assert!(
            !written.contains(canary),
            "{canary} reached the written report"
        );
    }
    use std::os::unix::fs::PermissionsExt as _;
    let mode = std::fs::metadata(&path)?.permissions().mode();
    assert_eq!(mode & 0o777, 0o600, "a shareable report must be owner-only");
    Ok(())
}

/// A daemon without the composed surface fails closed: no report, no file, and
/// a non-zero exit code.
#[test]
fn the_shipped_binary_fails_closed_without_the_advertised_capability() -> TestResult {
    let daemon = FakeDaemon::start(Gate::NotComposed)?;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("support-report.json");
    let output = run_binary(&[
        "support-report",
        "--socket",
        &daemon.socket(),
        "--output",
        path.to_str().ok_or("temp path is not utf-8")?,
    ]);
    daemon.stop();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        !path.exists(),
        "a withheld report must not leave a partial file behind"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not_composed"), "{stderr}");
    for canary in CANARIES {
        assert!(
            !stderr.contains(canary),
            "{canary} reached the failure output"
        );
    }
    Ok(())
}

/// The command is documented in the installed usage text and never doubles as
/// a session control.
#[test]
fn the_installed_usage_documents_the_support_report_command() {
    let output = run_binary(&["--not-a-command"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr.contains("support-report"), "{stderr}");
    assert!(
        stderr.contains("--output"),
        "the usage must document the report destination: {stderr}"
    );
    // A usage error is not a report: nothing is rendered to stdout.
    assert!(output.stdout.is_empty());
}
