//! One monotonic deadline shared by every cleanup owner of a single shutdown.
//!
//! IPC connection cleanup, supervised sessions, the resident ASR owner, portal
//! cleanup, retries and thread joins must all derive their wait from the same
//! absolute deadline. A per-owner timeout accumulates: independently bounded
//! owners can add up past the packaged unit's `TimeoutStopSec`, and an owner
//! that starts late receives a fresh window instead of the remaining one.
//!
//! The budget is deliberately small and portable, like [`crate::SessionCancellation`]:
//! it owns no product policy. The composing service chooses the total; every
//! owner only observes the remaining time. Arming happens exactly once, when
//! shutdown begins (for the daemon that is the first accepted
//! shutdown/signal request), so time spent closing transport admission is
//! charged to the same budget as the later cleanup.

use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// A single monotonic shutdown budget: one start instant and one total bound.
///
/// Clones share the same start state, so a transport adapter and the service
/// that composes it cannot disagree about when shutdown began. An unarmed
/// budget reports the full total and a moving provisional deadline; callers
/// must [`ShutdownBudget::arm`] (or [`ShutdownBudget::arm_at`]) from the actual
/// shutdown entry point before treating a deadline as final.
#[derive(Debug, Clone)]
pub struct ShutdownBudget {
    total: Duration,
    started: Arc<OnceLock<Instant>>,
}

impl ShutdownBudget {
    /// Create a budget with an explicit total bound.
    #[must_use]
    pub fn new(total: Duration) -> Self {
        Self {
            total,
            started: Arc::new(OnceLock::new()),
        }
    }

    /// The total bound every cleanup owner shares.
    #[must_use]
    pub fn total(&self) -> Duration {
        self.total
    }

    /// Record the shutdown start, at most once.
    ///
    /// Returns `true` when this call established the epoch. Later calls are
    /// no-ops, which keeps the deadline monotonic even if several owners
    /// observe the same shutdown.
    pub fn arm(&self) -> bool {
        self.arm_at(Instant::now())
    }

    /// Record a specific shutdown start instant, at most once.
    ///
    /// The injectable instant keeps deterministic regressions possible without
    /// sleeping through real time.
    pub fn arm_at(&self, started: Instant) -> bool {
        self.started.set(started).is_ok()
    }

    /// Whether the shutdown epoch has been established.
    #[must_use]
    pub fn is_armed(&self) -> bool {
        self.started.get().is_some()
    }

    /// The recorded shutdown start instant, when armed.
    #[must_use]
    pub fn started_at(&self) -> Option<Instant> {
        self.started.get().copied()
    }

    /// Absolute deadline every cleanup owner must respect.
    ///
    /// While unarmed this is provisional (`now + total`) and does not arm the
    /// budget; an owner waiting on a provisional deadline still cannot wait
    /// past the total, it just is not yet pinned to the real start.
    #[must_use]
    pub fn deadline(&self) -> Instant {
        match self.started.get() {
            Some(started) => *started + self.total,
            None => Instant::now() + self.total,
        }
    }

    /// Remaining budget measured from `now`.
    ///
    /// Saturates at zero: an exhausted budget never goes negative and never
    /// grants an owner a fresh window.
    #[must_use]
    pub fn remaining_at(&self, now: Instant) -> Duration {
        match self.started.get() {
            Some(started) => started
                .checked_add(self.total)
                .map_or(Duration::ZERO, |deadline| {
                    deadline.saturating_duration_since(now)
                }),
            None => self.total,
        }
    }

    /// Remaining budget at the current instant.
    #[must_use]
    pub fn remaining(&self) -> Duration {
        self.remaining_at(Instant::now())
    }

    /// Whether the shared budget is exhausted at `now`.
    #[must_use]
    pub fn expired_at(&self, now: Instant) -> bool {
        self.is_armed() && self.remaining_at(now).is_zero()
    }

    /// Whether the shared budget is exhausted.
    #[must_use]
    pub fn expired(&self) -> bool {
        self.expired_at(Instant::now())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arming_is_once_and_fixes_one_monotonic_deadline() {
        let budget = ShutdownBudget::new(Duration::from_secs(2));
        let first = Instant::now();
        assert!(!budget.is_armed());
        assert!(budget.arm_at(first));
        assert!(budget.is_armed());
        assert_eq!(budget.started_at(), Some(first));
        assert_eq!(budget.deadline(), first + Duration::from_secs(2));

        // A later request cannot move the epoch, so no owner can extend the
        // window another owner already started spending.
        assert!(!budget.arm_at(first + Duration::from_secs(1)));
        assert_eq!(budget.deadline(), first + Duration::from_secs(2));
        assert_eq!(
            budget.remaining_at(first + Duration::from_secs(1)),
            Duration::from_secs(1)
        );
    }

    #[test]
    fn remaining_is_monotonic_and_saturates_at_zero() {
        let budget = ShutdownBudget::new(Duration::from_millis(500));
        let start = Instant::now();
        budget.arm_at(start);
        let mut previous = budget.remaining_at(start);
        for step in 1..=10 {
            let now = start + Duration::from_millis(step * 100);
            let remaining = budget.remaining_at(now);
            assert!(remaining <= previous, "remaining time grew after arming");
            previous = remaining;
        }
        assert_eq!(previous, Duration::ZERO);
        assert_eq!(
            budget.remaining_at(start + Duration::from_secs(60)),
            Duration::ZERO
        );
        assert!(budget.expired_at(start + Duration::from_secs(60)));
        assert!(!budget.expired_at(start));
    }

    #[test]
    fn unarmed_budget_grants_at_most_the_total_without_arming() {
        let budget = ShutdownBudget::new(Duration::from_millis(250));
        let deadline = budget.deadline();
        assert!(deadline <= Instant::now() + Duration::from_millis(250));
        assert_eq!(budget.remaining(), Duration::from_millis(250));
        // Observing a provisional deadline must not consume the real event.
        assert!(!budget.is_armed());
        assert!(!budget.expired());
    }
}
