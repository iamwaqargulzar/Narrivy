//! Deterministic regressions for the single bounded daemon shutdown budget.
//!
//! These tests compose the real owners (the unix IPC server and the worker
//! supervisor) under one [`ShutdownBudget`] without a daemon subprocess, so
//! exhaustion and owner cancellation are asserted deterministically. The
//! packaged `TimeoutStopSec=3s` behaviour of the actual binary is exercised
//! separately in `daemon_shutdown_signals.rs`.

use std::error::Error;
use std::io::ErrorKind;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use narrivy_core::{CancelFlag, WorkerStop, WorkerSupervisor};
use narrivy_ipc_unix::UnixIpcServer;
use narrivy_types::ShutdownBudget;

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

const TEST_TIMEOUT: Duration = Duration::from_secs(5);

/// A supervised owner that observes cancellation immediately but refuses to
/// complete until released, so it stays live and reapable while the shared
/// budget is spent and cannot be refreshed for free.
fn stubborn_worker(
    observed: Arc<AtomicUsize>,
    release: Arc<AtomicUsize>,
) -> impl FnOnce(CancelFlag) + Send + 'static {
    move |cancel: CancelFlag| {
        while !cancel.is_cancelled() {
            std::thread::sleep(Duration::from_millis(1));
        }
        observed.fetch_add(1, Ordering::SeqCst);
        while release.load(Ordering::SeqCst) == 0 {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

fn wait_for_cancellations(observed: &AtomicUsize, want: usize) -> TestResult {
    let deadline = Instant::now() + TEST_TIMEOUT;
    while observed.load(Ordering::SeqCst) < want {
        if Instant::now() >= deadline {
            return Err(format!(
                "only {} of {want} owners observed cancellation",
                observed.load(Ordering::SeqCst)
            )
            .into());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    Ok(())
}

fn assert_socket_removed(socket: &Path) -> TestResult {
    match std::fs::symlink_metadata(socket) {
        Ok(_) => Err(format!("socket {} still exists after shutdown", socket.display()).into()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("stat {} failed: {error}", socket.display()).into()),
    }
}

/// Every owner must spend the same monotonic window. A second pass and a late
/// owner receive the remaining budget — including zero — never a fresh one,
/// and an exhausted budget still cancels every owner and reports truthfully.
#[test]
fn cumulative_cleanup_shares_one_budget_and_never_refreshes_it() -> TestResult {
    let dir = tempfile::tempdir()?;
    let socket = dir.path().join("budget.sock");
    let budget = ShutdownBudget::new(Duration::from_millis(300));
    let server = UnixIpcServer::bind_with_shutdown_budget(&socket, budget.clone())?;
    // The first shutdown request arms the epoch, exactly like a signal.
    server.request_shutdown();
    assert!(budget.is_armed());

    let supervisor = Arc::new(WorkerSupervisor::new());
    let observed = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(AtomicUsize::new(0));
    for index in 0..2 {
        let worker = stubborn_worker(Arc::clone(&observed), Arc::clone(&release));
        supervisor.spawn(format!("stubborn-{index}"), worker)?;
    }

    // First owner: the IPC endpoint is confirmed closed inside the shared
    // window (no admitted connections remain in this fixture).
    let started = Instant::now();
    server.shutdown_and_join(TEST_TIMEOUT)?;
    assert_socket_removed(&socket)?;

    // Second owner: cancellation is delivered immediately, then the join
    // spends only the remaining shared time and reports the live workers.
    supervisor.cancel_all();
    let outcomes = supervisor.shutdown_all_until(budget.deadline())?;
    assert_eq!(outcomes.len(), 2);
    assert!(
        outcomes
            .iter()
            .all(|(_, stop)| *stop == WorkerStop::TimedOut),
        "unfinished owners must be reported truthfully: {outcomes:?}"
    );
    wait_for_cancellations(&observed, 2)?;
    assert_eq!(
        supervisor.live_worker_count(),
        2,
        "timed-out owners must stay tracked, not dropped"
    );
    assert!(budget.expired(), "the shared window was not spent");
    let elapsed = started.elapsed();
    assert!(
        elapsed <= Duration::from_millis(700),
        "cumulative cleanup exceeded one shared budget: {elapsed:?}"
    );

    // A retry, and an IPC cleanup that starts after exhaustion, both receive a
    // zero-width window instead of a new one.
    let retry_started = Instant::now();
    let retry = supervisor.shutdown_all_until(budget.deadline())?;
    assert!(retry.iter().all(|(_, stop)| *stop == WorkerStop::TimedOut));
    server.shutdown_and_join(TEST_TIMEOUT)?;
    let exhausted_elapsed = retry_started.elapsed();
    assert!(
        exhausted_elapsed < Duration::from_millis(200),
        "a spent budget was refreshed for a later owner: {exhausted_elapsed:?}"
    );

    // Ownership survived the exhausted pass: a later explicitly bounded reap
    // still resolves every worker.
    release.store(1, Ordering::SeqCst);
    let reaped = supervisor.shutdown_all_until(Instant::now() + Duration::from_secs(2))?;
    assert!(reaped.iter().all(|(_, stop)| *stop == WorkerStop::Stopped));
    assert_eq!(supervisor.live_worker_count(), 0);
    Ok(())
}

/// An already-exhausted budget must not skip admission closure or owner
/// cancellation; it may only make the confirmed reap impossible.
#[test]
fn every_owner_is_cancelled_when_the_budget_is_already_spent() -> TestResult {
    let dir = tempfile::tempdir()?;
    let socket = dir.path().join("spent.sock");
    let budget = ShutdownBudget::new(Duration::from_millis(50));
    budget.arm_at(Instant::now() - Duration::from_secs(1));
    assert!(budget.expired());

    let server = UnixIpcServer::bind_with_shutdown_budget(&socket, budget.clone())?;
    let supervisor = Arc::new(WorkerSupervisor::new());
    let observed = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(AtomicUsize::new(0));
    for index in 0..2 {
        let worker = stubborn_worker(Arc::clone(&observed), Arc::clone(&release));
        supervisor.spawn(format!("stubborn-{index}"), worker)?;
    }

    supervisor.cancel_all();
    let outcomes = supervisor.shutdown_all_until(budget.deadline())?;
    assert_eq!(outcomes.len(), 2);
    assert!(
        outcomes
            .iter()
            .all(|(_, stop)| *stop == WorkerStop::TimedOut)
    );
    wait_for_cancellations(&observed, 2)?;
    assert_eq!(supervisor.live_worker_count(), 2);

    // IPC admission still closes and the owned endpoint is still removed.
    server.shutdown_and_join(TEST_TIMEOUT)?;
    assert_socket_removed(&socket)?;
    assert_eq!(server.active_connection_count(), 0);

    release.store(1, Ordering::SeqCst);
    let reaped = supervisor.shutdown_all_until(Instant::now() + Duration::from_secs(2))?;
    assert!(reaped.iter().all(|(_, stop)| *stop == WorkerStop::Stopped));
    assert_eq!(supervisor.live_worker_count(), 0);
    Ok(())
}
