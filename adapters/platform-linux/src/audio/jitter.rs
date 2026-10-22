//! Adaptive jitter buffer: detects high variance in PipeWire callback intervals.
//!
//! The PipeWire callback interval variance > 2× mean signals scheduling
//! jitter (e.g., GC pauses, competing inference threads). When detected we
//! emit a `jitter_high` health fact (never transcript content) that
//! diagnostics can surface without leaking audio.

use std::collections::VecDeque;
use std::time::Duration;

/// Number of intervals kept for variance/mean analysis.
const WINDOW: usize = 32;

/// Health code emitted when jitter is high.
pub const JITTER_HIGH_CODE: &str = "jitter_high";
/// Subsystem name for jitter health facts.
pub const JITTER_SUBSYSTEM: &str = "audio";

/// Adaptive jitter estimator: sliding window of recent callback intervals.
#[derive(Debug)]
pub struct JitterEstimator {
    intervals_us: VecDeque<u64>,
    capacity: usize,
}

impl Default for JitterEstimator {
    fn default() -> Self {
        Self::new(WINDOW)
    }
}

impl JitterEstimator {
    pub fn new(capacity: usize) -> Self {
        Self {
            intervals_us: VecDeque::with_capacity(capacity.max(1)),
            capacity: capacity.max(1),
        }
    }

    /// Record a callback interval. Returns `Some(true)` when jitter is high,
    /// `Some(false)` when healthy, `None` when not enough samples yet.
    pub fn observe(&mut self, interval: Duration) -> Option<bool> {
        let us = interval.as_micros() as u64;
        if self.intervals_us.len() >= self.capacity {
            self.intervals_us.pop_front();
        }
        self.intervals_us.push_back(us);
        self.evaluate()
    }

    /// Evaluate current window: variance > 2× mean => high jitter.
    /// Returns None when window not yet half full.
    pub fn evaluate(&self) -> Option<bool> {
        if self.intervals_us.len() < self.capacity / 2 {
            return None;
        }
        let n = self.intervals_us.len() as f64;
        let sum: f64 = self.intervals_us.iter().map(|&v| v as f64).sum();
        let mean = sum / n;
        if mean == 0.0 {
            return Some(false);
        }
        let var: f64 = self
            .intervals_us
            .iter()
            .map(|&v| {
                let diff = v as f64 - mean;
                diff * diff
            })
            .sum::<f64>()
            / n;
        Some(var > 2.0 * mean)
    }

    /// Convenience: is jitter currently high?
    pub fn is_high(&self) -> bool {
        self.evaluate().unwrap_or(false)
    }

    /// Health fact to emit when high, otherwise None.
    pub fn health_fact(&self) -> Option<(String, String, bool)> {
        match self.evaluate() {
            Some(true) => Some((
                JITTER_SUBSYSTEM.to_string(),
                JITTER_HIGH_CODE.to_string(),
                false,
            )),
            _ => None,
        }
    }

    pub fn len(&self) -> usize {
        self.intervals_us.len()
    }

    pub fn is_empty(&self) -> bool {
        self.intervals_us.is_empty()
    }

    pub fn clear(&mut self) {
        self.intervals_us.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn low_variance_is_healthy() {
        let mut est = JitterEstimator::new(8);
        for _ in 0..8 {
            est.observe(Duration::from_millis(10));
        }
        assert_eq!(est.evaluate(), Some(false));
        assert!(!est.is_high());
        assert!(est.health_fact().is_none());
    }

    #[test]
    fn high_variance_is_jitter_high() {
        let mut est = JitterEstimator::new(8);
        for i in 0..8 {
            let d = if i % 2 == 0 {
                Duration::from_millis(1)
            } else {
                Duration::from_millis(20)
            };
            est.observe(d);
        }
        assert_eq!(est.evaluate(), Some(true));
        assert!(est.is_high());
        let fact = est.health_fact();
        assert_eq!(
            fact.as_ref().map(|fact| fact.1.as_str()),
            Some(JITTER_HIGH_CODE)
        );
        assert_eq!(fact.as_ref().map(|fact| fact.2), Some(false));
        assert_eq!(
            fact.as_ref().map(|fact| fact.1.contains("transcript")),
            Some(false)
        );
    }

    #[test]
    fn not_enough_samples_returns_none() {
        let mut est = JitterEstimator::new(8);
        est.observe(Duration::from_millis(10));
        assert_eq!(est.evaluate(), None);
    }
}
