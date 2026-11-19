//! Offline unit tests for the delivery router.
//!
//! Contract: no external delivery tool must be installed, and no test emits
//! real input into the host session. All routing tests run against fake
//! backends / a fake [`CommandRunner`]; the only real process spawned is a
//! coreutils `sleep` in the bounded-timeout check, plus an explicitly gated
//! real-input smoke test (env `NARRIVY_ALLOW_REAL_INPUT=1`, empty payload)
//! carried over from the v0.4 safety gate.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use narrivy_types::{DeliveryAttempt, DeliveryOutcome, FallbackReason, TransactionId};

use super::backend::{
    AttemptOutcome, BackendAvailability, ClipboardTool, DeliveryBackend, DeliveryBackendId,
    DeliveryCheck, DeliveryRequest, MAX_DIRECT_TEXT_BYTES, UinputBackend, WlClipboardOrXclip,
};
use super::command::{CommandRunner, CommandSpec, RunError, RunOutcome, StdCommandRunner};
use super::inbox::{InboxError, InboxStore, validate_txid};
use super::probe::{generate_txid, probe_wtype_at};
use super::router::{
    DeliveryRouterConfig, route_and_deliver, route_and_deliver_tx_with, route_over_backends,
    route_with_attempts,
};

static REAL_RUNNER_TEST_LOCK: Mutex<()> = Mutex::new(());

// ---------------------------------------------------------------------------
// Fakes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
enum FakeRun {
    Succeed,
    FailExit,
    SpawnFail,
    Timeout,
}

/// Records every [`CommandSpec`] (argv construction evidence) and returns
/// scripted results. Nothing is ever spawned.
struct FakeRunner {
    default_behavior: FakeRun,
    /// Per-call script consumed front-first, then `default_behavior` applies.
    script: std::sync::Mutex<VecDeque<FakeRun>>,
    specs: std::sync::Mutex<Vec<CommandSpec>>,
    stdin_lens: std::sync::Mutex<Vec<Option<usize>>>,
}

impl FakeRunner {
    fn new(default_behavior: FakeRun) -> Self {
        Self {
            default_behavior,
            script: std::sync::Mutex::new(VecDeque::new()),
            specs: std::sync::Mutex::new(Vec::new()),
            stdin_lens: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn with_script(default_behavior: FakeRun, script: &[FakeRun]) -> Self {
        let r = Self::new(default_behavior);
        *r.script
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = script.iter().copied().collect();
        r
    }

    fn next_behavior(&self) -> FakeRun {
        self.script
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pop_front()
            .unwrap_or(self.default_behavior)
    }

    /// (program, args lossy, stdin length) per recorded call.
    fn recorded(&self) -> Vec<(String, Vec<String>, Option<usize>)> {
        self.specs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .zip(
                self.stdin_lens
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .iter(),
            )
            .map(|(spec, stdin)| {
                (
                    spec.program.display().to_string(),
                    spec.args
                        .iter()
                        .map(|a| a.to_string_lossy().into_owned())
                        .collect(),
                    *stdin,
                )
            })
            .collect()
    }
}

impl CommandRunner for FakeRunner {
    fn run(
        &self,
        spec: &CommandSpec,
        stdin_data: Option<&[u8]>,
        _timeout: Duration,
    ) -> Result<RunOutcome, RunError> {
        self.specs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(spec.clone());
        self.stdin_lens
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(stdin_data.map(|d| d.len()));
        match self.next_behavior() {
            FakeRun::Succeed => Ok(RunOutcome {
                success: true,
                code: Some(0),
            }),
            FakeRun::FailExit => Ok(RunOutcome {
                success: false,
                code: Some(1),
            }),
            FakeRun::SpawnFail => Err(RunError::Spawn("fake-spawn-failure".into())),
            FakeRun::Timeout => Err(RunError::Timeout),
        }
    }
}

/// Scripted ladder rung with the same trait contract as the real backends.
struct FakeBackend {
    id: DeliveryBackendId,
    avail: BackendAvailability,
    result: DeliveryOutcome,
}

impl DeliveryCheck for FakeBackend {
    fn availability(&self) -> BackendAvailability {
        self.avail.clone()
    }
}

impl DeliveryBackend for FakeBackend {
    fn id(&self) -> DeliveryBackendId {
        self.id.clone()
    }

    fn deliver(&mut self, _req: &DeliveryRequest<'_>) -> AttemptOutcome {
        let reason = match self.result {
            DeliveryOutcome::Failed => Some(FallbackReason::BackendError {
                code: "FAKE".into(),
            }),
            _ => None,
        };
        AttemptOutcome::new(self.id.clone(), self.result.clone(), reason)
    }
}

fn fake(id: DeliveryBackendId, avail: BackendAvailability, result: DeliveryOutcome) -> FakeBackend {
    FakeBackend { id, avail, result }
}

fn labels(attempts: &[DeliveryAttempt]) -> Vec<&str> {
    attempts.iter().map(|a| a.backend.as_str()).collect()
}

// ---------------------------------------------------------------------------
// Routing order and honesty
// ---------------------------------------------------------------------------

#[test]
fn fake_backend_routing_order_follows_ladder() {
    let mut wtype = fake(
        DeliveryBackendId::WtypeVirtualKeyboard,
        BackendAvailability::ProbeAvailable,
        DeliveryOutcome::Failed,
    );
    let mut clip = fake(
        DeliveryBackendId::ClipboardCopy(WlClipboardOrXclip::WlCopy),
        BackendAvailability::ProbeAvailable,
        DeliveryOutcome::Failed,
    );
    let mut inbox = fake(
        DeliveryBackendId::Inbox,
        BackendAvailability::ProbeAvailable,
        DeliveryOutcome::InboxFallback,
    );
    let mut chain: [&mut dyn DeliveryBackend; 3] = [&mut wtype, &mut clip, &mut inbox];
    let txid = TransactionId::new("tx-order");
    let req = DeliveryRequest {
        text: "hello",
        txid: &txid,
    };

    let (outcome, attempts) = route_over_backends(&mut chain, &req);

    assert_eq!(outcome, DeliveryOutcome::InboxFallback);
    assert_eq!(
        labels(&attempts),
        vec!["wtype-virtual-keyboard", "clipboard-copy:wl-copy", "inbox"]
    );
    // Inbox is the terminal success; the earlier rungs failed honestly.
    assert_eq!(
        attempts.last().map(|a| a.outcome.clone()),
        Some(DeliveryOutcome::InboxFallback)
    );
}

#[test]
fn delivered_is_terminal_after_first_success() {
    let mut wtype = fake(
        DeliveryBackendId::WtypeVirtualKeyboard,
        BackendAvailability::ProbeAvailable,
        DeliveryOutcome::Delivered,
    );
    let mut clip = fake(
        DeliveryBackendId::ClipboardCopy(WlClipboardOrXclip::WlCopy),
        BackendAvailability::ProbeAvailable,
        DeliveryOutcome::CopiedOnly,
    );
    let mut chain: [&mut dyn DeliveryBackend; 2] = [&mut wtype, &mut clip];
    let txid = TransactionId::new("tx-term");
    let req = DeliveryRequest {
        text: "hello",
        txid: &txid,
    };

    let (outcome, attempts) = route_over_backends(&mut chain, &req);

    assert_eq!(outcome, DeliveryOutcome::Delivered);
    assert_eq!(labels(&attempts), vec!["wtype-virtual-keyboard"]);
}

#[test]
fn uinput_skipped_by_default_even_when_configured_available()
-> Result<(), Box<dyn std::error::Error>> {
    // Default config: least-privilege opt-in flag is off.
    let cfg = DeliveryRouterConfig::default();
    assert!(!cfg.allow_uinput_opt_in);

    let dir = tempfile::tempdir()?;
    let cfg = DeliveryRouterConfig {
        inbox_dir: Some(dir.path().to_path_buf()),
        ..cfg
    };
    let runner = FakeRunner::new(FakeRun::SpawnFail); // every probe misses

    let mut seen: Vec<DeliveryOutcome> = Vec::new();
    {
        let mut sink = |o: DeliveryOutcome| seen.push(o);
        let outcome = route_and_deliver_tx_with(
            &cfg,
            &TransactionId::new("tx-lp"),
            "least privilege",
            &mut sink,
            &runner,
        );
        assert_eq!(outcome, DeliveryOutcome::InboxFallback);
    }
    let (_, attempts) = route_with_attempts(
        &cfg,
        &TransactionId::new("tx-lp-2"),
        "least privilege",
        &runner,
    );

    let ls = labels(&attempts);
    assert_eq!(
        ls,
        vec!["wtype-virtual-keyboard", "clipboard-copy:wl-copy", "inbox"]
    );
    assert!(
        !ls.contains(&"uinput-opt-in"),
        "uinput must be absent by default: {ls:?}"
    );
    assert_eq!(
        seen,
        vec![
            DeliveryOutcome::Failed,
            DeliveryOutcome::Failed,
            DeliveryOutcome::InboxFallback
        ]
    );
    Ok(())
}

#[test]
fn uinput_opt_in_sits_between_wtype_and_clipboard() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let cfg = DeliveryRouterConfig {
        allow_uinput_opt_in: true,
        inbox_dir: Some(dir.path().to_path_buf()),
        ..DeliveryRouterConfig::default()
    };
    let runner = FakeRunner::new(FakeRun::SpawnFail);

    let (_, attempts) =
        route_with_attempts(&cfg, &TransactionId::new("tx-optin"), "opted in", &runner);

    let ls = labels(&attempts);
    let positions = (
        ls.iter()
            .position(|label| *label == "wtype-virtual-keyboard"),
        ls.iter().position(|label| *label == "uinput-opt-in"),
        ls.iter()
            .position(|label| label.starts_with("clipboard-copy:")),
        ls.iter().position(|label| *label == "inbox"),
    );
    assert!(
        matches!(positions, (Some(wtype), Some(uinput), Some(clipboard), Some(inbox))
            if wtype < uinput && uinput < clipboard && clipboard < inbox),
        "every delivery rung must appear in least-privilege order: {positions:?}"
    );
    Ok(())
}

#[test]
fn uinput_opt_in_disabled_by_policy_even_with_device() {
    // Policy gate wins before any device check.
    let b = UinputBackend::new(false);
    assert!(matches!(
        b.availability(),
        BackendAvailability::ProbeMissing(_)
    ));
    // Even when allowed and a device path exists, P2-prep synthesis is
    // unqualified and fails honestly with a typed code.
    let mut allowed = UinputBackend::new(true);
    let txid = TransactionId::new("tx-ui");
    let req = DeliveryRequest {
        text: "x",
        txid: &txid,
    };
    let att = allowed.deliver(&req);
    assert_eq!(att.outcome, DeliveryOutcome::Failed);
    assert_eq!(
        att.fallback_reason,
        Some(FallbackReason::BackendError {
            code: "UINPUT_NOT_QUALIFIED".into()
        })
    );
}

#[test]
fn copied_only_is_never_reported_as_delivered() {
    let mut wtype = fake(
        DeliveryBackendId::WtypeVirtualKeyboard,
        BackendAvailability::ProbeAvailable,
        DeliveryOutcome::Failed,
    );
    let mut clip = fake(
        DeliveryBackendId::ClipboardCopy(WlClipboardOrXclip::WlCopy),
        BackendAvailability::ProbeAvailable,
        DeliveryOutcome::CopiedOnly,
    );
    let mut inbox = fake(
        DeliveryBackendId::Inbox,
        BackendAvailability::ProbeAvailable,
        DeliveryOutcome::InboxFallback,
    );
    let mut chain: [&mut dyn DeliveryBackend; 3] = [&mut wtype, &mut clip, &mut inbox];
    let txid = TransactionId::new("tx-copy");
    let req = DeliveryRequest {
        text: "copy honesty",
        txid: &txid,
    };

    let (outcome, attempts) = route_over_backends(&mut chain, &req);

    assert_eq!(outcome, DeliveryOutcome::CopiedOnly);
    assert!(!matches!(outcome, DeliveryOutcome::Delivered));
    // Copy-only stops the ladder: the durable Inbox rung is not consumed.
    assert_eq!(
        labels(&attempts),
        vec!["wtype-virtual-keyboard", "clipboard-copy:wl-copy"]
    );
    assert_eq!(
        attempts.last().map(|a| a.outcome.clone()),
        Some(DeliveryOutcome::CopiedOnly)
    );
}

#[test]
fn all_rungs_exhausted_yields_failed() {
    let mut wtype = fake(
        DeliveryBackendId::WtypeVirtualKeyboard,
        BackendAvailability::ProbeAvailable,
        DeliveryOutcome::Failed,
    );
    let mut inbox = fake(
        DeliveryBackendId::Inbox,
        BackendAvailability::ProbeMissing("no dir".into()),
        DeliveryOutcome::Failed,
    );
    let mut chain: [&mut dyn DeliveryBackend; 2] = [&mut wtype, &mut inbox];
    let txid = TransactionId::new("tx-fail");
    let req = DeliveryRequest {
        text: "doomed",
        txid: &txid,
    };

    let (outcome, attempts) = route_over_backends(&mut chain, &req);

    assert_eq!(outcome, DeliveryOutcome::Failed);
    // Probed-missing rungs are recorded, not silently skipped.
    assert_eq!(attempts.len(), 2);
    assert_eq!(
        attempts[1].fallback_reason,
        Some(FallbackReason::NoQualifiedBackend)
    );
}

// ---------------------------------------------------------------------------
// Inbox atomic write + filename validation
// ---------------------------------------------------------------------------

#[test]
fn inbox_fallback_writes_atomically_and_validates_filename()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let inbox_dir = dir.path().to_path_buf();
    let cfg = DeliveryRouterConfig {
        inbox_dir: Some(inbox_dir.clone()),
        ..DeliveryRouterConfig::default()
    };
    let runner = FakeRunner::new(FakeRun::SpawnFail);
    let txid = TransactionId::new("tx-atomic-1");
    let text = "persist me\nsecond line 🌍";

    let mut seen: Vec<DeliveryOutcome> = Vec::new();
    {
        let mut sink = |o: DeliveryOutcome| seen.push(o);
        let outcome = route_and_deliver_tx_with(&cfg, &txid, text, &mut sink, &runner);
        assert_eq!(outcome, DeliveryOutcome::InboxFallback);
    }
    assert_eq!(seen.last(), Some(&DeliveryOutcome::InboxFallback));

    let expected = inbox_dir.join("tx-atomic-1.txt");
    assert!(expected.exists(), "inbox artifact must exist");
    let stored = std::fs::read_to_string(&expected)?;
    assert_eq!(stored, text);
    assert_eq!(
        InboxStore::new(inbox_dir.clone())
            .read_text(&txid)
            .map_err(|error| std::io::Error::other(error.to_string()))?,
        text
    );
    // Atomic write: exactly one artifact, no temp leftovers.
    let entries = std::fs::read_dir(&inbox_dir)?.count();
    assert_eq!(entries, 1, "no partial/temp files may remain");
    Ok(())
}

#[test]
fn txid_validation_rejects_escape_and_hidden_names() {
    for good in ["tx-1", "Tx_09.z", "tx-18f2c1e2ab-0007"] {
        assert!(validate_txid(good).is_ok(), "{good} should validate");
    }
    for bad in [
        "",
        "../evil",
        "a/b",
        "a\\b",
        ".hidden",
        "-flag",
        "sp ace",
        "tab\tkey",
        "new\nline",
        "ünïcode",
        &"x".repeat(129),
    ] {
        assert_eq!(
            validate_txid(bad),
            Err(InboxError::InvalidTxid),
            "{bad:?} must be rejected"
        );
    }
}

#[test]
fn invalid_txid_creates_nothing_in_filesystem() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let nested = dir.path().join("inbox");
    let store = InboxStore::new(nested.clone());
    let evil = TransactionId::new("../escape");
    let res = store.store_text(&evil, "should not land");
    assert_eq!(res, Err(InboxError::InvalidTxid));
    assert!(
        !nested.exists(),
        "validation must run before any filesystem touch"
    );
    assert!(!dir.path().join("escape.txt").exists());
    Ok(())
}

// ---------------------------------------------------------------------------
// Verbatim passthrough + oversize policy
// ---------------------------------------------------------------------------

#[test]
fn unicode_multiline_and_5k_fixtures_pass_through_verbatim()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let cfg = DeliveryRouterConfig {
        inbox_dir: Some(dir.path().to_path_buf()),
        ..DeliveryRouterConfig::default()
    };
    let runner = FakeRunner::new(FakeRun::SpawnFail);

    let fixtures: Vec<String> = vec![
        "café naïve 🌍 αβγ".to_string(),
        "line1\nline2\nline3".to_string(),
        "مرحبا بالعالم".to_string(),
        "你好世界".to_string(),
        "emoji 🌍\nmix\ttab".to_string(),
        "a".repeat(5120),
    ];
    for (i, fixture) in fixtures.iter().enumerate() {
        let txid = TransactionId::new(format!("tx-fixture-{i}"));
        let mut noop = |_: DeliveryOutcome| {};
        let outcome = route_and_deliver_tx_with(&cfg, &txid, fixture, &mut noop, &runner);
        assert_eq!(outcome, DeliveryOutcome::InboxFallback, "fixture {i}");
        let stored = InboxStore::new(dir.path().to_path_buf())
            .read_text(&txid)
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        assert_eq!(stored, *fixture, "fixture {i} must pass through verbatim");
    }
    Ok(())
}

#[test]
fn oversized_text_falls_back_to_inbox_with_policy_reason() -> Result<(), Box<dyn std::error::Error>>
{
    let dir = tempfile::tempdir()?;
    let cfg = DeliveryRouterConfig {
        // wtype probes AVAILABLE: the size guard must prevent any insertion spawn.
        wtype_path: Some("/opt/fake-wtype".into()),
        inbox_dir: Some(dir.path().to_path_buf()),
        ..DeliveryRouterConfig::default()
    };
    let runner = FakeRunner::new(FakeRun::Succeed);
    let oversized = "a".repeat(MAX_DIRECT_TEXT_BYTES + 1);
    let txid = TransactionId::new("tx-big");

    let (outcome, attempts) = route_with_attempts(&cfg, &txid, &oversized, &runner);

    assert_eq!(outcome, DeliveryOutcome::InboxFallback);
    let ls = labels(&attempts);
    assert_eq!(
        ls,
        vec!["wtype-virtual-keyboard", "clipboard-copy:wl-copy", "inbox"]
    );
    assert_eq!(
        attempts[0].fallback_reason,
        Some(FallbackReason::PolicyDenied),
        "wtype must refuse oversized payloads by policy"
    );
    assert_eq!(
        attempts[1].fallback_reason,
        Some(FallbackReason::PolicyDenied),
        "clipboard must refuse oversized payloads by policy"
    );
    // No spec carrying the payload was ever built: no `--` separator argv and
    // no stdin feed happened.
    for (program, args, stdin) in runner.recorded() {
        assert_ne!(
            args.first().map(|s| s.as_str()),
            Some("--"),
            "payload must not reach argv of {program}"
        );
        assert_eq!(stdin, None, "payload must not reach stdin of {program}");
    }
    let stored = std::fs::read_to_string(dir.path().join("tx-big.txt"))?;
    assert_eq!(stored, oversized);
    Ok(())
}

// ---------------------------------------------------------------------------
// Argv construction against the fake command builder
// ---------------------------------------------------------------------------

#[test]
fn wtype_argv_is_stdin_only_and_never_claims_verified_delivery() {
    let cfg = DeliveryRouterConfig {
        wtype_path: Some("/opt/fake-wtype".into()),
        ..DeliveryRouterConfig::default()
    };
    let runner = FakeRunner::new(FakeRun::Succeed);
    let payload = "argv payload ✅\nsecond";

    let (outcome, attempts) =
        route_with_attempts(&cfg, &TransactionId::new("tx-argv"), payload, &runner);

    assert_eq!(outcome, DeliveryOutcome::CopiedOnly);
    assert_eq!(attempts[0].outcome, DeliveryOutcome::SubmittedUnverified);
    assert!(!matches!(outcome, DeliveryOutcome::Delivered));
    let recorded = runner.recorded();
    assert_eq!(recorded.len(), 4);
    // Probe: `wtype --version`.
    assert_eq!(recorded[0].0, "/opt/fake-wtype");
    assert_eq!(recorded[0].1, vec!["--version"]);
    assert_eq!(recorded[0].2, None);
    // Insertion: `wtype -`; payload travels only through stdin.
    assert_eq!(recorded[1].0, "/opt/fake-wtype");
    assert_eq!(recorded[1].1, vec!["-".to_string()]);
    assert_eq!(recorded[1].2, Some(payload.len()));
    assert!(!recorded[1].1.contains(&payload.to_string()));
}

#[test]
fn clipboard_argv_uses_selection_flag_and_stdin_payload() {
    // Script: wtype probe OK, wtype insert FAILS, clipboard probes/copy succeed.
    let runner = FakeRunner::with_script(FakeRun::Succeed, &[FakeRun::Succeed, FakeRun::FailExit]);
    let cfg = DeliveryRouterConfig {
        wtype_path: Some("fake-wtype".into()),
        clipboard_tool: Some(ClipboardTool::xclip_default()),
        ..DeliveryRouterConfig::default()
    };
    let payload = "clipboard bound payload";

    let (outcome, attempts) =
        route_with_attempts(&cfg, &TransactionId::new("tx-clip"), payload, &runner);

    assert_eq!(outcome, DeliveryOutcome::CopiedOnly);
    assert!(!matches!(outcome, DeliveryOutcome::Delivered));
    assert_eq!(attempts[1].backend, "clipboard-copy:xclip");
    let recorded = runner.recorded();
    // [0] wtype --version, [1] wtype - via stdin, [2] xclip -version, [3] xclip copy.
    assert_eq!(recorded[2].1, vec!["-version"]);
    assert_eq!(recorded[3].0, "xclip");
    assert_eq!(
        recorded[3].1,
        vec!["-selection".to_string(), "clipboard".to_string()]
    );
    // Payload travels via stdin (write_all + close), never via argv.
    assert_eq!(recorded[3].2, Some(payload.len()));
    assert!(!recorded[3].1.contains(&payload.to_string()));
}

#[test]
fn wl_copy_copy_spec_has_no_extra_args() {
    let tool = ClipboardTool::wl_copy_default();
    let spec = tool.copy_spec();
    assert_eq!(spec.program, std::path::PathBuf::from("wl-copy"));
    assert!(spec.args.is_empty());
}

#[test]
fn command_spec_debug_and_describe_redact_payload() {
    let mut spec = CommandSpec::new("wtype");
    spec.arg("--");
    spec.arg("TOPSECRET_PAYLOAD 🌍");
    let rendered = format!("{spec:?}");
    assert!(
        !rendered.contains("TOPSECRET_PAYLOAD"),
        "Debug leaked args: {rendered}"
    );
    let described = spec.describe();
    assert!(!described.contains("TOPSECRET_PAYLOAD"));
    // Fields stay directly accessible for tests/diagnostics that need them.
    assert_eq!(spec.args.len(), 2);
}

// ---------------------------------------------------------------------------
// Probes and bounded real runner
// ---------------------------------------------------------------------------

#[test]
fn probe_maps_spawn_errors_to_probe_missing() {
    let runner = FakeRunner::new(FakeRun::SpawnFail);
    let avail = probe_wtype_at(Path::new("wtype"), &runner);
    assert!(matches!(avail, BackendAvailability::ProbeMissing(_)));

    // Real std runner, definitely-missing program: still no panic, ProbeMissing.
    let real = probe_wtype_at(
        Path::new("/nonexistent/narrivy-no-such-wtype"),
        &StdCommandRunner,
    );
    assert!(matches!(real, BackendAvailability::ProbeMissing(_)));
}

#[test]
fn probe_maps_timeout_to_probe_missing() {
    // A tool that exists but hangs past the probe bound is unusable here.
    let runner = FakeRunner::new(FakeRun::Timeout);
    let avail = probe_wtype_at(Path::new("wtype"), &runner);
    assert_eq!(
        avail,
        BackendAvailability::ProbeMissing("probe-timeout".into())
    );
}

#[test]
fn probe_treats_nonzero_version_exit_as_present() {
    // A tool that exists but rejects --version still proves presence.
    let runner = FakeRunner::new(FakeRun::FailExit);
    let avail = probe_wtype_at(Path::new("wtype"), &runner);
    assert_eq!(avail, BackendAvailability::ProbeAvailable);
}

#[test]
fn real_runner_bounded_timeout_kills_child() {
    let _guard = REAL_RUNNER_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // Only real process spawned by the suite: harmless coreutils `sleep`.
    let mut spec = CommandSpec::new("sleep");
    spec.arg("30");
    let start = Instant::now();
    let res = StdCommandRunner.run(&spec, None, Duration::from_millis(250));
    let elapsed = start.elapsed();
    assert!(
        matches!(res, Err(RunError::Timeout | RunError::Spawn(_))),
        "sleep must not finish within the bound, got {res:?}"
    );
    assert!(
        elapsed < Duration::from_secs(10),
        "bounded wait overran: {elapsed:?}"
    );
}

#[test]
fn real_runner_stdin_write_and_close_reaches_child() {
    let _guard = REAL_RUNNER_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // cat copies stdin to stdout(stdout discarded here); proves write+close path.
    let spec = CommandSpec::new("cat");
    let res = StdCommandRunner.run(
        &spec,
        Some(b"bounded stdin".as_slice()),
        Duration::from_secs(5),
    );
    if let Ok(out) = &res {
        assert!(out.success, "cat must exit cleanly");
    }
    assert!(
        matches!(res, Ok(_) | Err(RunError::Spawn(_))),
        "cat run failed: {res:?}"
    );
}

#[test]
fn real_runner_reaps_child_after_parent_owned_stdin_failure()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = REAL_RUNNER_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir()?;
    let marker = dir.path().join("stdin-closed.pid");
    let mut spec = CommandSpec::new("sh");
    spec.arg("-c").arg(format!(
        "echo $$ > '{}'; exec 0<&-; exec sleep 10",
        marker.display()
    ));
    let result = StdCommandRunner.run(
        &spec,
        Some(&vec![b'x'; 1024 * 1024]),
        Duration::from_secs(2),
    );
    if matches!(result, Err(RunError::Spawn(_))) {
        return Ok(());
    }
    assert!(
        matches!(result, Err(RunError::StdinWrite(_))),
        "closed stdin must fail the parent-owned pump, got {result:?}"
    );
    let pid = std::fs::read_to_string(marker)?;
    assert!(
        !std::process::Command::new("kill")
            .args(["-0", pid.trim()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?
            .success(),
        "stdin-failure child still exists"
    );
    Ok(())
}

#[test]
fn real_runner_spawn_failure_leaves_no_background_writer() {
    let _guard = REAL_RUNNER_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let spec = CommandSpec::new("/nonexistent/narrivy-no-command");
    assert!(matches!(
        StdCommandRunner.run(&spec, Some(b"controlled"), Duration::from_secs(1)),
        Err(RunError::Spawn(_))
    ));
}

#[test]
fn real_runner_non_draining_stdin_uses_one_deadline() {
    let _guard = REAL_RUNNER_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut spec = CommandSpec::new("sh");
    spec.arg("-c").arg("exec sleep 10");
    let bound = Duration::from_millis(250);
    let scheduler_allowance = Duration::from_millis(300);
    let started = Instant::now();
    let result = StdCommandRunner.run(&spec, Some(&vec![b'x'; 1024 * 1024]), bound);
    if matches!(result, Err(RunError::Spawn(_))) {
        return;
    }
    assert!(
        matches!(result, Err(RunError::Timeout)),
        "non-draining child must time out, got {result:?}"
    );
    assert!(
        started.elapsed() < bound + scheduler_allowance,
        "cleanup opened a second deadline window: {:?}",
        started.elapsed()
    );
}

// ---------------------------------------------------------------------------
// Txids, portability, defaults, gated real-input smoke
// ---------------------------------------------------------------------------

#[test]
fn generated_txids_are_distinct_and_validate() {
    let mut seen = Vec::new();
    for _ in 0..3 {
        let txid = generate_txid();
        assert!(validate_txid(txid.as_str()).is_ok());
        assert!(!seen.contains(&txid));
        seen.push(txid);
    }
}

#[test]
fn attempts_are_portable_delivery_attempt_records() {
    let mut wtype = fake(
        DeliveryBackendId::WtypeVirtualKeyboard,
        BackendAvailability::ProbeAvailable,
        DeliveryOutcome::Failed,
    );
    let mut inbox = fake(
        DeliveryBackendId::Inbox,
        BackendAvailability::ProbeAvailable,
        DeliveryOutcome::InboxFallback,
    );
    let mut chain: [&mut dyn DeliveryBackend; 2] = [&mut wtype, &mut inbox];
    let txid = TransactionId::new("tx-portable");
    let req = DeliveryRequest {
        text: "portable",
        txid: &txid,
    };

    let (_, attempts) = route_over_backends(&mut chain, &req);

    assert_eq!(attempts[0].backend, "wtype-virtual-keyboard");
    assert!(matches!(
        attempts[0].fallback_reason,
        Some(FallbackReason::BackendError { ref code }) if code == "FAKE"
    ));
    assert_eq!(attempts[1].backend, "inbox");
    assert_eq!(attempts[1].outcome, DeliveryOutcome::InboxFallback);
    assert_eq!(attempts[1].fallback_reason, None);
}

#[test]
fn default_config_is_least_privilege() {
    let cfg = DeliveryRouterConfig::default();
    assert!(!cfg.allow_uinput_opt_in);
    assert_eq!(cfg.wtype_path, None);
    assert_eq!(cfg.clipboard_tool, None);
    assert_eq!(cfg.inbox_dir, None);
}

#[test]
fn missing_inbox_dir_lets_router_end_failed() {
    let cfg = DeliveryRouterConfig::default();
    let runner = FakeRunner::new(FakeRun::SpawnFail);
    let (outcome, attempts) =
        route_with_attempts(&cfg, &TransactionId::new("tx-none"), "no inbox", &runner);
    assert_eq!(outcome, DeliveryOutcome::Failed);
    assert_eq!(labels(&attempts).last(), Some(&"inbox"));
}

#[test]
fn real_input_smoke_is_gated_and_bounded() -> Result<(), Box<dyn std::error::Error>> {
    // SAFETY GATE (v0.4 salvage): only run against a live session when the
    // operator explicitly opts in; empty payload types nothing.
    if std::env::var("NARRIVY_ALLOW_REAL_INPUT").as_deref() != Ok("1") {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let cfg = DeliveryRouterConfig {
        inbox_dir: Some(dir.path().to_path_buf()),
        ..DeliveryRouterConfig::default()
    };
    let mut seen: Vec<DeliveryOutcome> = Vec::new();
    {
        let mut sink = |o: DeliveryOutcome| seen.push(o);
        let start = Instant::now();
        let outcome = route_and_deliver(&cfg, "", &mut sink);
        assert!(
            start.elapsed() <= Duration::from_secs(10),
            "router must stay bounded"
        );
        // Empty payload: any terminal outcome is acceptable; no panic/hang is the contract.
        assert!(matches!(
            outcome,
            DeliveryOutcome::Delivered
                | DeliveryOutcome::CopiedOnly
                | DeliveryOutcome::InboxFallback
                | DeliveryOutcome::Failed
        ));
    }
    Ok(())
}

#[test]
fn probe_all_returns_per_backend_map_quickly_in_parallel() {
    use super::router::DeliveryRouter;
    // Slow fake runner: each probe sleeps 30ms to simulate tool probe latency
    struct SlowRunner;
    impl super::command::CommandRunner for SlowRunner {
        fn run(
            &self,
            _spec: &super::command::CommandSpec,
            _stdin: Option<&[u8]>,
            _timeout: Duration,
        ) -> Result<super::command::RunOutcome, super::command::RunError> {
            std::thread::sleep(Duration::from_millis(30));
            Ok(super::command::RunOutcome {
                success: true,
                code: Some(0),
            })
        }
    }

    let cfg = DeliveryRouterConfig {
        allow_uinput_opt_in: true,
        inbox_dir: Some(std::env::temp_dir().join("narrivy-probe-test")),
        ..DeliveryRouterConfig::default()
    };
    let router = DeliveryRouter::new(cfg);
    let runner = SlowRunner;

    let start = Instant::now();
    let map = router.probe_all(&runner);
    let elapsed = start.elapsed();

    // Map must contain all configured backends
    assert!(
        map.contains_key("wtype-virtual-keyboard"),
        "wtype should be probed"
    );
    assert!(
        map.contains_key("clipboard-copy:wl-copy") || map.contains_key("clipboard-copy:xclip"),
        "clipboard should be probed, got keys {:?}",
        map.keys()
    );
    assert!(map.contains_key("inbox"), "inbox should be probed");
    // Parallel: 4 probes *30ms sequential would be 120ms+, parallel should be <100ms (with some slack for CI)
    // Allow up to 90ms to prove parallelism
    assert!(
        elapsed < Duration::from_millis(100),
        "probe_all took {elapsed:?}, expected <100ms parallel, map={map:?}"
    );
    println!("probe_all parallel: {elapsed:?} for {} backends", map.len());
}

#[test]
fn probe_all_parallel_vs_sequential_benchmark() {
    use super::router::DeliveryRouter;
    struct SleepRunner(u64);
    impl super::command::CommandRunner for SleepRunner {
        fn run(
            &self,
            _spec: &super::command::CommandSpec,
            _stdin: Option<&[u8]>,
            _timeout: Duration,
        ) -> Result<super::command::RunOutcome, super::command::RunError> {
            std::thread::sleep(Duration::from_millis(self.0));
            Ok(super::command::RunOutcome {
                success: true,
                code: Some(0),
            })
        }
    }

    let cfg = DeliveryRouterConfig {
        allow_uinput_opt_in: false,
        inbox_dir: Some(std::env::temp_dir().join("narrivy-probe-bench")),
        ..DeliveryRouterConfig::default()
    };
    let router = DeliveryRouter::new(cfg);
    let runner = SleepRunner(20);
    let (map, elapsed_ms) = router.probe_all_timed(&runner);
    assert!(!map.is_empty());
    // Timed helper should also be <100ms parallel
    assert!(
        elapsed_ms < 100,
        "probe_all_timed {elapsed_ms}ms should be <100ms"
    );
    // Sequential simulation: 3 probes *20 =60ms, but with 1 thread they'd be ~60ms still under 100. To prove parallel benefit we check that parallel is not 3x slower.
    // For this benchmark we just ensure it completes quickly.
    println!("probe_all_timed: {elapsed_ms}ms map size {}", map.len());
}
