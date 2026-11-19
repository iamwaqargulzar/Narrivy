//! Bounded latency sampling with deterministic percentile helpers.

use std::collections::VecDeque;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// Default ring capacity of [`LatencyTracker`].
pub const DEFAULT_RING_CAPACITY: usize = 512;

/// One latency observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LatencySample {
    /// Pipeline stage name, e.g. `asr_decode`.
    pub stage: String,
    /// Observed duration in microseconds.
    pub micros: u64,
}

impl LatencySample {
    /// Build a sample.
    #[must_use]
    pub fn new(stage: impl Into<String>, micros: u64) -> Self {
        Self {
            stage: stage.into(),
            micros,
        }
    }
}

/// Compact summary of a tracker's contents, safe to embed in reports.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LatencySummary {
    /// Number of retained samples.
    pub count: u64,
    /// 50th percentile in microseconds, `None` when empty.
    pub p50_micros: Option<u64>,
    /// 95th percentile in microseconds, `None` when empty.
    pub p95_micros: Option<u64>,
}

impl LatencySummary {
    /// Build a summary from samples.
    #[must_use]
    pub fn from_samples(samples: &[LatencySample]) -> Self {
        let mut micros: Vec<u64> = samples.iter().map(|s| s.micros).collect();
        micros.sort_unstable();
        Self {
            count: micros.len() as u64,
            p50_micros: percentile_of_sorted(&micros, 50),
            p95_micros: percentile_of_sorted(&micros, 95),
        }
    }
}

impl From<&[LatencySample]> for LatencySummary {
    fn from(samples: &[LatencySample]) -> Self {
        Self::from_samples(samples)
    }
}

impl From<&LatencyTracker> for LatencySummary {
    fn from(tracker: &LatencyTracker) -> Self {
        Self::from_samples(&tracker.snapshot())
    }
}

#[derive(Debug)]
struct LatencyInner {
    ring: VecDeque<LatencySample>,
    capacity: usize,
}

/// Bounded ring of latency samples.
///
/// Recording beyond the capacity drops the oldest sample. Percentiles are
/// computed by sorting a copy of the retained microsecond values and taking
/// the nearest-rank element, so results are deterministic for a given set of
/// samples. Thread-safe; a poisoned lock is recovered by continuing with the
/// retained samples.
#[derive(Debug)]
pub struct LatencyTracker {
    inner: Mutex<LatencyInner>,
}

impl Default for LatencyTracker {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_RING_CAPACITY)
    }
}

impl LatencyTracker {
    /// Tracker with the default ring capacity.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Tracker with an explicit ring capacity (0 records nothing).
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(LatencyInner {
                ring: VecDeque::with_capacity(capacity.min(4096)),
                capacity,
            }),
        }
    }

    /// Record one observation; oldest samples are evicted past capacity.
    pub fn record(&self, stage: impl Into<String>, micros: u64) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if inner.capacity == 0 {
            return;
        }
        if inner.ring.len() == inner.capacity {
            inner.ring.pop_front();
        }
        inner.ring.push_back(LatencySample::new(stage, micros));
    }

    /// Number of retained samples.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .ring
            .len()
    }

    /// True when no samples are retained.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Retained samples in insertion order.
    #[must_use]
    pub fn snapshot(&self) -> Vec<LatencySample> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .ring
            .iter()
            .cloned()
            .collect()
    }

    /// 50th percentile in microseconds; `None` when empty.
    #[must_use]
    pub fn p50(&self) -> Option<u64> {
        self.percentile(50)
    }

    /// 95th percentile in microseconds; `None` when empty.
    #[must_use]
    pub fn p95(&self) -> Option<u64> {
        self.percentile(95)
    }

    /// Nearest-rank percentile for `percent` in `0..=100`; `None` when empty.
    #[must_use]
    pub fn percentile(&self, percent: u8) -> Option<u64> {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut micros: Vec<u64> = inner.ring.iter().map(|s| s.micros).collect();
        micros.sort_unstable();
        percentile_of_sorted(&micros, percent)
    }
}

/// Nearest-rank percentile over ascending-sorted values.
fn percentile_of_sorted(sorted: &[u64], percent: u8) -> Option<u64> {
    if sorted.is_empty() || percent > 100 {
        return None;
    }
    let n = sorted.len();
    // rank = ceil(percent * n / 100), clamped to [1, n].
    let rank = (u64::from(percent) * n as u64).div_ceil(100);
    let rank = rank.clamp(1, n as u64) as usize;
    sorted.get(rank - 1).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_is_bounded_and_preserves_insertion_order() {
        let tracker = LatencyTracker::with_capacity(4);
        for micros in [10u64, 20, 30, 40, 50, 60] {
            tracker.record("stage", micros);
        }
        let snap = tracker.snapshot();
        assert_eq!(snap.len(), 4);
        assert_eq!(
            snap.iter().map(|s| s.micros).collect::<Vec<_>>(),
            [30, 40, 50, 60]
        );
        assert!(!tracker.is_empty());
    }

    #[test]
    fn percentiles_are_deterministic_nearest_rank() {
        let tracker = LatencyTracker::new();
        for micros in [5u64, 1, 9, 3, 7] {
            tracker.record("asr", micros);
        }
        // sorted: [1, 3, 5, 7, 9]; p50 rank=ceil(2.5)=3 -> 5; p95 rank=5 -> 9.
        assert_eq!(tracker.p50(), Some(5));
        assert_eq!(tracker.p95(), Some(9));
        assert_eq!(tracker.percentile(100), Some(9));
        assert_eq!(tracker.percentile(0), Some(1));
    }

    #[test]
    fn empty_tracker_returns_none_not_panic() {
        let tracker = LatencyTracker::new();
        assert_eq!(tracker.p50(), None);
        assert_eq!(tracker.p95(), None);
        assert_eq!(tracker.percentile(50), None);
        assert!(tracker.is_empty());
        assert_eq!(tracker.snapshot(), Vec::new());
        assert_eq!(LatencySummary::from(&tracker), LatencySummary::default());
    }

    #[test]
    fn single_sample_percentiles_equal_the_sample() {
        let tracker = LatencyTracker::new();
        tracker.record("mic", 42);
        assert_eq!(tracker.p50(), Some(42));
        assert_eq!(tracker.p95(), Some(42));
    }

    #[test]
    fn zero_capacity_tracker_records_nothing() {
        let tracker = LatencyTracker::with_capacity(0);
        tracker.record("stage", 1);
        assert!(tracker.is_empty());
        assert_eq!(tracker.p50(), None);
    }

    #[test]
    fn summary_matches_tracker_percentiles() {
        let tracker = LatencyTracker::new();
        for micros in [100u64, 200, 300, 400] {
            tracker.record("s", micros);
        }
        let summary = LatencySummary::from(&tracker);
        assert_eq!(summary.count, 4);
        assert_eq!(summary.p50_micros, tracker.p50());
        assert_eq!(summary.p95_micros, tracker.p95());
        // sorted [100,200,300,400]: p50 rank=2 -> 200; p95 rank=ceil(3.8)=4 -> 400.
        assert_eq!(summary.p50_micros, Some(200));
        assert_eq!(summary.p95_micros, Some(400));
    }

    #[test]
    fn samples_serialize_round_trip() {
        let tracker = LatencyTracker::new();
        tracker.record("asr", 123);
        let snap = tracker.snapshot();
        let text = serde_json::to_string(&snap).unwrap_or_default();
        let back: Vec<LatencySample> = serde_json::from_str(&text).unwrap_or_default();
        assert_eq!(back, snap);
    }
}
