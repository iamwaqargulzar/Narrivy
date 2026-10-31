//! One bounded daemon shutdown budget shared by every cleanup owner.
//!
//! The packaged user service sets `TimeoutStopSec=3s`. Independently bounded
//! owners (IPC connection cleanup, portal cleanup, supervised sessions, the
//! resident ASR engine, retries and pump joins) could each spend their own
//! fresh window and accumulate past that bound, and an owner that starts late
//! would receive a new window instead of the remaining one. ADR-0069 fixes the
//! contract: the daemon arms one [`ShutdownBudget`] when shutdown begins (the
//! first signal/serve/fatal shutdown request, before `serve` returns) and every
//! owner below derives its wait from that deadline.
//!
//! The budget is deliberately below the unit bound: a daemon that cannot
//! confirm an owner's reap must still exit on its own and report failure rather
//! than spending past `TimeoutStopSec` and being killed by systemd, which would
//! hide the failure.

use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Total stop budget shared by every daemon cleanup owner.
///
/// Chosen below the packaged `TimeoutStopSec=3s` so the process exits on its
/// own with a truthful failure even when an owner cannot be confirmed.
pub const DAEMON_SHUTDOWN_TOTAL: Duration = Duration::from_millis(2500);

/// Poll interval for deadline-bounded joins of owned daemon threads.
const SHUTDOWN_JOIN_POLL: Duration = Duration::from_millis(2);

pub use narrivy_types::ShutdownBudget;

/// Join one owned daemon thread without waiting past `deadline`.
///
/// Returns `Ok(())` only when the thread really finished; the handle is then
/// consumed. On a timeout the handle stays owned by the caller (still `Some`),
/// so unfinished work is reported as a failure and never silently detached or
/// mistaken for reaped state.
pub fn join_thread_bounded<T>(
    handle: &mut Option<JoinHandle<T>>,
    deadline: Instant,
) -> Result<(), &'static str> {
    loop {
        if handle.as_ref().is_some_and(JoinHandle::is_finished) {
            return match handle.take() {
                Some(handle) => handle
                    .join()
                    .map(drop)
                    .map_err(|_| "owned daemon thread terminated unexpectedly"),
                None => Ok(()),
            };
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("owned daemon thread did not stop before the shared shutdown deadline");
        }
        std::thread::sleep(remaining.min(SHUTDOWN_JOIN_POLL));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn bounded_join_reports_unfinished_owner_without_detaching_it() {
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let mut handle = Some(std::thread::spawn(move || {
            while !worker_stop.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(1));
            }
        }));
        let started = Instant::now();
        assert!(
            join_thread_bounded(&mut handle, started + Duration::from_millis(50)).is_err(),
            "an unfinished thread must be reported as a failure"
        );
        assert!(started.elapsed() < Duration::from_millis(400));
        assert!(
            handle.is_some(),
            "the helper must leave the handle owned on a timeout"
        );

        stop.store(true, Ordering::SeqCst);
        assert!(
            join_thread_bounded(&mut handle, Instant::now() + Duration::from_secs(2)).is_ok(),
            "the released thread must join inside a fresh explicit bound"
        );
        assert!(handle.is_none(), "a confirmed join consumes the handle");
    }

    #[test]
    fn shared_budget_never_refreshes_a_spent_window() {
        let budget = ShutdownBudget::new(Duration::from_millis(100));
        let started = Instant::now();
        budget.arm_at(started);
        let deadline = budget.deadline();

        // First owner spends the whole window.
        while Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(budget.expired());

        // A later owner receives the same absolute deadline. A refreshed
        // window would move the deadline or leave non-zero remaining time.
        assert_eq!(budget.deadline(), deadline);
        assert_eq!(budget.remaining(), Duration::ZERO);
        assert!(
            Instant::now() < deadline + Duration::from_millis(50),
            "the assertion itself must not have opened a new window"
        );
    }
}
