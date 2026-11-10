//! Latency histogram: AtomicU64 ring for push latency measurement.
//!
//! The LiveCapture thread records each `PreRollRingBuffer::push` duration
//! into a lock-free ring of `AtomicU64` nanoseconds. Readers compute
//! p50/p95/p99 without blocking the writer. This keeps the RT path allocation-free
//! and logging-free while still enabling p99 <50µs assertions even under load.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Capacity of the latency ring (power of two not required).
const RING_CAP: usize = 2048;

/// Latency statistics snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatencyStats {
    /// Total observations recorded.
    pub count: u64,
    /// p50 in microseconds (None when empty).
    pub p50_us: Option<u64>,
    /// p95 in microseconds.
    pub p95_us: Option<u64>,
    /// p99 in microseconds.
    pub p99_us: Option<u64>,
    /// Max observed in microseconds.
    pub max_us: Option<u64>,
    /// Mean in microseconds (rounded).
    pub mean_us: Option<u64>,
}

/// Lock-free ring buffer for latency measurements (nanoseconds).
pub struct LatencyHistogram {
    ring: Box<[AtomicU64]>,
    write_pos: AtomicU64,
    count: AtomicU64,
}

impl Default for LatencyHistogram {
    fn default() -> Self {
        Self::new(RING_CAP)
    }
}

impl LatencyHistogram {
    pub fn new(capacity: usize) -> Self {
        let cap = capacity.max(1);
        let ring = (0..cap)
            .map(|_| AtomicU64::new(0))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            ring,
            write_pos: AtomicU64::new(0),
            count: AtomicU64::new(0),
        }
    }

    /// Record a latency duration (lock-free, single writer assumed but safe for multiple).
    pub fn record(&self, d: Duration) {
        let nanos = d.as_nanos().min(u128::from(u64::MAX)) as u64;
        let pos = self.write_pos.fetch_add(1, Ordering::Relaxed) as usize % self.ring.len();
        self.ring[pos].store(nanos, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
    }

    /// Snapshot current stats (sorts a copy of observed values).
    pub fn snapshot(&self) -> LatencyStats {
        let total = self.count.load(Ordering::Relaxed);
        if total == 0 {
            return LatencyStats {
                count: 0,
                p50_us: None,
                p95_us: None,
                p99_us: None,
                max_us: None,
                mean_us: None,
            };
        }
        let cap = self.ring.len();
        let n = (total as usize).min(cap);
        let mut vals: Vec<u64> = Vec::with_capacity(n);
        let pos = self.write_pos.load(Ordering::Relaxed) as usize;
        for i in 0..n {
            let idx = (pos + cap - n + i) % cap;
            let v = self.ring[idx].load(Ordering::Relaxed);
            if total >= cap as u64 || v != 0 || i == 0 {
                vals.push(v);
            }
        }
        if vals.is_empty() {
            return LatencyStats {
                count: total,
                p50_us: None,
                p95_us: None,
                p99_us: None,
                max_us: None,
                mean_us: None,
            };
        }
        vals.sort_unstable();
        let mean = vals.iter().copied().map(u128::from).sum::<u128>() / vals.len() as u128;
        LatencyStats {
            count: total,
            p50_us: Some(percentile(&vals, 50) / 1000),
            p95_us: Some(percentile(&vals, 95) / 1000),
            p99_us: Some(percentile(&vals, 99) / 1000),
            max_us: vals.last().copied().map(|v| v / 1000),
            mean_us: Some((mean as u64) / 1000),
        }
    }

    pub fn count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    pub fn clear(&self) {
        for a in self.ring.iter() {
            a.store(0, Ordering::Relaxed);
        }
        self.write_pos.store(0, Ordering::Relaxed);
        self.count.store(0, Ordering::Relaxed);
    }
}

fn percentile(sorted: &[u64], p: u32) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let k = ((p as f64 / 100.0) * sorted.len() as f64).ceil() as usize;
    let idx = k.saturating_sub(1).min(sorted.len() - 1);
    sorted[idx]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn empty_histogram_has_none_percentiles() {
        let h = LatencyHistogram::new(8);
        let s = h.snapshot();
        assert_eq!(s.count, 0);
        assert!(s.p50_us.is_none());
    }

    #[test]
    fn percentiles_are_monotonic() {
        let h = LatencyHistogram::new(16);
        for i in 1..=16 {
            h.record(Duration::from_micros(i * 10));
        }
        let s = h.snapshot();
        assert!(
            s.p50_us.is_some() && s.p95_us.is_some() && s.p99_us.is_some() && s.max_us.is_some(),
            "a populated histogram must expose all percentiles"
        );
        assert!(s.p50_us <= s.p95_us);
        assert!(s.p95_us <= s.p99_us);
        assert!(s.p99_us <= s.max_us);
    }

    #[test]
    fn ring_overwrites_oldest() {
        let h = LatencyHistogram::new(4);
        for i in 0..6 {
            h.record(Duration::from_micros(i));
        }
        assert_eq!(h.count(), 6);
        let s = h.snapshot();
        assert_eq!(s.max_us, Some(5));
    }
}
