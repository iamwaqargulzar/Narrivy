//! Subsystem health facts with latest-wins state and bounded history.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// Default number of retained history entries per subsystem.
pub const DEFAULT_HISTORY_CAP_PER_SUBSYSTEM: usize = 64;

/// One observable health signal for a subsystem.
///
/// Keep this type content-free: subsystem and code are short stable
/// identifiers, never free-form messages that could carry user content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthFact {
    /// Subsystem identifier, e.g. `asr`.
    pub subsystem: String,
    /// Stable fact code, e.g. `model_loaded`.
    pub code: String,
    /// Whether the subsystem reports healthy for this fact.
    pub ok: bool,
}

impl HealthFact {
    /// Build a fact from identifiers.
    #[must_use]
    pub fn new(subsystem: impl Into<String>, code: impl Into<String>, ok: bool) -> Self {
        Self {
            subsystem: subsystem.into(),
            code: code.into(),
            ok,
        }
    }
}

#[derive(Debug, Default)]
struct HealthInner {
    current: BTreeMap<String, HealthFact>,
    history: BTreeMap<String, VecDeque<HealthFact>>,
    history_cap: usize,
}

/// Registry of [`HealthFact`] entries keyed by subsystem.
///
/// Observing a subsystem replaces its current fact (latest-wins) and appends
/// to that subsystem's bounded history (oldest entries are dropped first).
/// All methods are safe to call from multiple threads; a poisoned lock is
/// recovered by continuing with the last observed state.
#[derive(Debug, Default)]
pub struct HealthRegistry {
    inner: Mutex<HealthInner>,
}

impl HealthRegistry {
    /// Registry with the default per-subsystem history cap.
    #[must_use]
    pub fn new() -> Self {
        Self::with_history_cap(DEFAULT_HISTORY_CAP_PER_SUBSYSTEM)
    }

    /// Registry with an explicit per-subsystem history cap (0 keeps only the
    /// latest fact and no history).
    #[must_use]
    pub fn with_history_cap(cap: usize) -> Self {
        Self {
            inner: Mutex::new(HealthInner {
                current: BTreeMap::new(),
                history: BTreeMap::new(),
                history_cap: cap,
            }),
        }
    }

    /// Record a fact. Latest observation for a subsystem wins.
    pub fn observe(&self, fact: HealthFact) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let subsystem = fact.subsystem.clone();
        inner.current.insert(subsystem.clone(), fact.clone());
        let history_cap = inner.history_cap;
        let ring = inner.history.entry(subsystem).or_default();
        ring.push_back(fact);
        while ring.len() > history_cap {
            ring.pop_front();
        }
    }

    /// Convenience wrapper around [`Self::observe`].
    pub fn observe_fact(&self, subsystem: &str, code: &str, ok: bool) {
        self.observe(HealthFact::new(subsystem, code, ok));
    }

    /// Latest fact per subsystem, sorted by subsystem name.
    #[must_use]
    pub fn snapshot(&self) -> Vec<HealthFact> {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.current.values().cloned().collect()
    }

    /// Full bounded history, grouped per subsystem, oldest first within each
    /// subsystem, sorted by subsystem name.
    #[must_use]
    pub fn history_snapshot(&self) -> Vec<HealthFact> {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner
            .history
            .values()
            .flat_map(|ring| ring.iter().cloned())
            .collect()
    }

    /// Overall health: true only when every observed subsystem fact is `ok`
    /// (an empty registry is healthy).
    #[must_use]
    pub fn healthy(&self) -> bool {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.current.values().all(|fact| fact.ok)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latest_observation_wins_per_subsystem() {
        let reg = HealthRegistry::new();
        reg.observe_fact("asr", "model_loaded", false);
        reg.observe_fact("asr", "model_loaded", true);
        reg.observe_fact("ipc", "server_up", true);
        let snap = reg.snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].subsystem, "asr");
        assert!(snap[0].ok);
        assert_eq!(snap[0].code, "model_loaded");
    }

    #[test]
    fn snapshot_is_sorted_by_subsystem() {
        let reg = HealthRegistry::new();
        reg.observe_fact("zeta", "ok", true);
        reg.observe_fact("alpha", "ok", true);
        reg.observe_fact("mid", "ok", false);
        let snap = reg.snapshot();
        let subs: Vec<&str> = snap.iter().map(|f| f.subsystem.as_str()).collect();
        assert_eq!(subs, ["alpha", "mid", "zeta"]);
    }

    #[test]
    fn history_is_bounded_per_subsystem() {
        let reg = HealthRegistry::with_history_cap(3);
        for i in 0..10 {
            reg.observe(HealthFact::new("asr", "tick", i % 2 == 0));
        }
        let history = reg.history_snapshot();
        // Kept entries are the last three observations: i = 7, 8, 9.
        let oks: Vec<bool> = history.iter().map(|f| f.ok).collect();
        assert_eq!(oks, [false, true, false]);
        // Latest-wins current state is still the last observation.
        assert_eq!(reg.snapshot().len(), 1);
        assert!(!reg.snapshot()[0].ok);
    }

    #[test]
    fn healthy_reflects_all_facts_and_empty_registry() {
        let reg = HealthRegistry::new();
        assert!(reg.healthy());
        reg.observe_fact("a", "x", true);
        assert!(reg.healthy());
        reg.observe_fact("b", "y", false);
        assert!(!reg.healthy());
        reg.observe_fact("b", "y", true);
        assert!(reg.healthy());
    }

    #[test]
    fn facts_serialize_round_trip() {
        let reg = HealthRegistry::new();
        reg.observe_fact("asr", "model_loaded", true);
        let snap = reg.snapshot();
        let text = serde_json::to_string(&snap).unwrap_or_default();
        let back: Vec<HealthFact> = serde_json::from_str(&text).unwrap_or_default();
        assert_eq!(back, snap);
    }
}
