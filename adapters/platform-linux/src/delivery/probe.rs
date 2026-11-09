//! Availability probes and transaction-id minting for delivery backends.
//!
//! Probes answer "can this rung plausibly run here?". They never panic and
//! never log payload content; spawn failures map to
//! [`BackendAvailability::ProbeMissing`].

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use narrivy_types::TransactionId;

use super::backend::BackendAvailability;
use super::command::{CommandRunner, CommandSpec, RunError, StdCommandRunner};

/// Upper bound for one probe invocation.
pub const PROBE_TIMEOUT: Duration = Duration::from_millis(1500);

/// Probe for the `wtype` virtual-keyboard helper by running `wtype --version`
/// under a bounded timeout via [`StdCommandRunner`] (std::process::Command).
///
/// Never panics: any spawn failure maps to
/// [`BackendAvailability::ProbeMissing`]. "The binary ran" is the availability
/// bar — even a nonzero version exit proves presence (older wtype builds may
/// reject the flag), while a hang or spawn failure means unusable.
pub fn probe_wtype() -> BackendAvailability {
    probe_wtype_at(Path::new("wtype"), &StdCommandRunner)
}

/// [`probe_wtype`] with an injectable program path and runner (tests and
/// explicit configuration).
pub fn probe_wtype_at(program: &Path, runner: &dyn CommandRunner) -> BackendAvailability {
    probe_tool(program, &["--version"], runner, PROBE_TIMEOUT)
}

/// Generic bounded presence probe: run `program <version_args…>` and classify
/// the result. Spawn failures map to `ProbeMissing("spawn-failed")`.
pub fn probe_tool(
    program: &Path,
    version_args: &[&str],
    runner: &dyn CommandRunner,
    timeout: Duration,
) -> BackendAvailability {
    let mut spec = CommandSpec::new(program);
    for a in version_args {
        spec.arg(*a);
    }
    match runner.run(&spec, None, timeout) {
        Ok(_) => BackendAvailability::ProbeAvailable,
        Err(RunError::Spawn(_)) => BackendAvailability::ProbeMissing("spawn-failed".into()),
        Err(RunError::Timeout) => BackendAvailability::ProbeMissing("probe-timeout".into()),
        Err(_) => BackendAvailability::ProbeMissing("probe-error".into()),
    }
}

static TX_SEQ: AtomicU64 = AtomicU64::new(0);

/// Mint a router-internal transaction id (`tx-<unix_ms hex>-<seq>`) that is
/// valid per [`validate_txid`](super::inbox::validate_txid). Callers that own
/// a real pipeline transaction pass their own id to
/// [`route_and_deliver_tx_with`](super::router::route_and_deliver_tx_with).
pub fn generate_txid() -> TransactionId {
    let ms = now_unix_ms();
    let seq = TX_SEQ.fetch_add(1, Ordering::Relaxed);
    TransactionId::new(format!("tx-{ms:x}-{seq:04}"))
}

/// Milliseconds since the unix epoch; 0 before clock sanity (no panic path).
pub(crate) fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
