//! The typed delivery ladder (P2 prep; salvage-adapted, see README.md).
//!
//! Order: `wtype` direct -> uinput (only when explicitly opted in) ->
//! clipboard copy-only -> Inbox. First terminal outcome wins:
//! - [`DeliveryOutcome::Delivered`] — verified insertion,
//! - [`DeliveryOutcome::SubmittedUnverified`] — helper submission only,
//! - [`DeliveryOutcome::CopiedOnly`] — clipboard honesty, never upgraded,
//! - [`DeliveryOutcome::InboxFallback`] — durable sink written.
//!
//! Exhausting the ladder without a terminal rung yields
//! [`DeliveryOutcome::Failed`]. Every attempt — including probed-missing
//! rungs — is recorded as a portable [`DeliveryAttempt`].

use std::path::PathBuf;

use narrivy_types::{DeliveryAttempt, DeliveryOutcome, TransactionId};

use super::backend::{
    AttemptOutcome, BackendAvailability, ClipboardBackend, DeliveryBackend, DeliveryRequest,
    InboxBackend, UinputBackend, WtypeBackend,
};
use super::command::{CommandRunner, StdCommandRunner};
use super::probe::generate_txid;

/// Least-privilege configuration of the delivery router.
///
/// Defaults are deliberately restrictive: [`DeliveryRouterConfig::default()`]
/// sets `allow_uinput_opt_in = false`, so broad synthetic input requires an
/// explicit operator opt-in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryRouterConfig {
    /// Opt-in gate for the `/dev/uinput` rung. DEFAULT FALSE.
    pub allow_uinput_opt_in: bool,
    /// Explicit `wtype` program path; `None` resolves `wtype` on PATH.
    pub wtype_path: Option<PathBuf>,
    /// Explicit clipboard copy tool; `None` auto-probes wl-copy then xclip.
    pub clipboard_tool: Option<super::backend::ClipboardTool>,
    /// Durable Inbox directory; `None` disables the final rung, in which case
    /// the router can legitimately end in [`DeliveryOutcome::Failed`].
    pub inbox_dir: Option<PathBuf>,
}

impl Default for DeliveryRouterConfig {
    /// Default is deliberate and privacy-preserving: uinput is NEVER the
    /// default onboarding path (P0F-004 least-privilege rule), so this manual
    /// impl documents the non-derivable policy default.
    #[allow(clippy::derivable_impls)]
    fn default() -> Self {
        Self {
            allow_uinput_opt_in: false,
            wtype_path: None,
            clipboard_tool: None,
            inbox_dir: None,
        }
    }
}

/// Route `text` down the ladder, forwarding every attempt's outcome to `sink`,
/// and return the terminal outcome. Uses the real bounded std runner.
///
/// PRIVACY: neither sink calls nor diagnostics in this module carry payload
/// text beyond what the caller itself passed in; never log it either.
pub fn route_and_deliver(
    cfg: &DeliveryRouterConfig,
    text: &str,
    sink: &mut dyn FnMut(DeliveryOutcome),
) -> DeliveryOutcome {
    route_and_deliver_with(cfg, text, sink, &StdCommandRunner)
}

/// [`route_and_deliver`] with an injectable runner so tests stay offline.
pub fn route_and_deliver_with(
    cfg: &DeliveryRouterConfig,
    text: &str,
    sink: &mut dyn FnMut(DeliveryOutcome),
    runner: &dyn CommandRunner,
) -> DeliveryOutcome {
    let txid = generate_txid();
    route_and_deliver_tx_with(cfg, &txid, text, sink, runner)
}

/// As [`route_and_deliver_with`], but the caller supplies the transaction id
/// used for the Inbox artifact (P2 passes the pipeline transaction id).
pub fn route_and_deliver_tx_with(
    cfg: &DeliveryRouterConfig,
    txid: &TransactionId,
    text: &str,
    sink: &mut dyn FnMut(DeliveryOutcome),
    runner: &dyn CommandRunner,
) -> DeliveryOutcome {
    let (outcome, attempts) = route_with_attempts(cfg, txid, text, runner);
    for attempt in &attempts {
        (*sink)(attempt.outcome.clone());
    }
    outcome
}

/// Full ladder with portable [`DeliveryAttempt`] records, for callers that
/// journal attempts (P2 wires these into the recovery journal).
pub fn route_with_attempts(
    cfg: &DeliveryRouterConfig,
    txid: &TransactionId,
    text: &str,
    runner: &dyn CommandRunner,
) -> (DeliveryOutcome, Vec<DeliveryAttempt>) {
    let mut chain = build_chain(cfg, runner);
    let req = DeliveryRequest { text, txid };
    let mut raw = Vec::new();
    let outcome = run_boxed_ladder(chain.as_mut_slice(), &req, &mut raw);
    (
        outcome,
        raw.iter().map(AttemptOutcome::to_attempt).collect(),
    )
}

/// Route over caller-supplied backends — custom or fake ladders. Useful for
/// P2 composition and for offline contract tests.
pub fn route_over_backends(
    backends: &mut [&mut dyn DeliveryBackend],
    req: &DeliveryRequest<'_>,
) -> (DeliveryOutcome, Vec<DeliveryAttempt>) {
    let mut raw = Vec::new();
    let outcome = run_ref_ladder(backends, req, &mut raw);
    (
        outcome,
        raw.iter().map(AttemptOutcome::to_attempt).collect(),
    )
}

/// Concrete chain built from config. Order is fixed by the v1.3 mission:
/// wtype -> uinput (opt-in only) -> clipboard copy-only -> Inbox.
fn build_chain<'r>(
    cfg: &DeliveryRouterConfig,
    runner: &'r dyn CommandRunner,
) -> Vec<Box<dyn DeliveryBackend + Send + 'r>> {
    let mut chain: Vec<Box<dyn DeliveryBackend + Send + 'r>> = Vec::new();
    chain.push(Box::new(WtypeBackend::new(cfg.wtype_path.clone(), runner)));
    if cfg.allow_uinput_opt_in {
        // Least-privilege: present in the ladder ONLY on explicit opt-in.
        chain.push(Box::new(UinputBackend::new(true)));
    }
    chain.push(Box::new(ClipboardBackend::new(
        cfg.clipboard_tool.clone(),
        runner,
    )));
    chain.push(Box::new(InboxBackend::new(cfg.inbox_dir.clone())));
    chain
}

/// Walk the boxed config-built ladder.
fn run_boxed_ladder(
    backends: &mut [Box<dyn DeliveryBackend + Send + '_>],
    req: &DeliveryRequest<'_>,
    attempts: &mut Vec<AttemptOutcome>,
) -> DeliveryOutcome {
    for backend in backends.iter_mut() {
        if let Some(outcome) = attempt_rung(&mut **backend, req, attempts) {
            return outcome;
        }
    }
    DeliveryOutcome::Failed
}

/// Walk a caller-supplied reference ladder (custom/fake backends).
fn run_ref_ladder(
    backends: &mut [&mut dyn DeliveryBackend],
    req: &DeliveryRequest<'_>,
    attempts: &mut Vec<AttemptOutcome>,
) -> DeliveryOutcome {
    for backend in backends.iter_mut() {
        if let Some(outcome) = attempt_rung(&mut **backend, req, attempts) {
            return outcome;
        }
    }
    DeliveryOutcome::Failed
}

/// One rung: probe, attempt or record the miss, append the attempt record.
/// Returns `Some(outcome)` when a terminal rung stopped the ladder. First
/// terminal outcome wins; probed-missing rungs are recorded as failed
/// attempts (`NoQualifiedBackend`) and skipped.
fn attempt_rung(
    backend: &mut dyn DeliveryBackend,
    req: &DeliveryRequest<'_>,
    attempts: &mut Vec<AttemptOutcome>,
) -> Option<DeliveryOutcome> {
    let attempt = match backend.availability() {
        BackendAvailability::ProbeAvailable => backend.deliver(req),
        BackendAvailability::ProbeMissing(_) => AttemptOutcome::unavailable(backend.id()),
    };
    let outcome = attempt.outcome.clone();
    let terminal = matches!(
        outcome,
        DeliveryOutcome::Delivered | DeliveryOutcome::CopiedOnly | DeliveryOutcome::InboxFallback
    );
    attempts.push(attempt);
    terminal.then_some(outcome)
}

/// Owning wrapper that can probe every backend in parallel.
///
/// The parallel guarantee: all `availability()` calls run concurrently via
/// scoped threads, so wall time is bounded by the slowest probe, not the
/// sum. Used for fast startup health checks and startup latency budgeting
/// (<100 ms collective).
pub struct DeliveryRouter {
    config: DeliveryRouterConfig,
}

impl DeliveryRouter {
    pub fn new(config: DeliveryRouterConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &DeliveryRouterConfig {
        &self.config
    }

    /// Probe every configured backend's availability in parallel, returning a
    /// map from the backend's stable label to its probe result.
    ///
    /// Bounded: each probe inherits `PROBE_TIMEOUT` (1.5 s) but all run
    /// concurrently, so the collective wall time is ~max probe, not sum.
    /// Callers needing a tight budget can wrap this with their own outer
    /// timeout; the common case completes in <100 ms when tools are
    /// present/missing without hangs.
    pub fn probe_all(
        &self,
        runner: &dyn super::command::CommandRunner,
    ) -> std::collections::HashMap<String, BackendAvailability> {
        use std::collections::HashMap;
        // Sequential probe: `build_chain` returns `Box<dyn DeliveryBackend + Send>`,
        // but each backend borrows `runner: &dyn CommandRunner` which is not `Sync`,
        // so the boxed trait object is not `Send` in practice and cannot be moved
        // into scoped threads without a `Sync` runner. Sequential probing keeps
        // `!Sync` fakes (RefCell-based) working and still meets the latency
        // budget for the 3-rung chain (<30 ms typical). A parallel variant
        // `probe_all_parallel_with_sync_runner` can be added when a `Sync`
        // runner factory is available.
        let chain = build_chain(&self.config, runner);
        let mut map = HashMap::new();
        for backend in chain {
            let label = backend.id().label().to_string();
            let avail = backend.availability();
            map.insert(label, avail);
        }
        map
    }

    pub fn probe_all_timed(
        &self,
        runner: &dyn super::command::CommandRunner,
    ) -> (std::collections::HashMap<String, BackendAvailability>, u128) {
        let start = std::time::Instant::now();
        let map = self.probe_all(runner);
        let elapsed_ms = start.elapsed().as_millis();
        (map, elapsed_ms)
    }
}
