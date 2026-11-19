//! Model residency tracking (PERF-007): Hot / Warm / Cold, persisted to
//! `residency.json`, with versioned invalidation.
//!
//! Invalidation key = sha256(model bytes) + runtime + backend + tool_version.
//! Any component change demotes the entry back to Cold on next observation.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// Residency file format version.
pub const RESIDENCY_VERSION: u32 = 1;

/// Residency states. `Cold` is also the default answer for unknown models and
/// for entries whose invalidation key changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Residency {
    /// Installed on disk, not loaded.
    Cold,
    /// Loaded recently; metadata kept warm in the tracker.
    Warm,
    /// In-use handle held by a live session.
    Hot,
}

/// Versioned invalidation key (PERF-007). Changing any field invalidates.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct InvalidationKey {
    pub model_sha256: String,
    pub runtime: String,
    pub backend: String,
    pub tool_version: String,
}

impl std::fmt::Display for InvalidationKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}/{}/{}/{}",
            self.model_sha256, self.runtime, self.backend, self.tool_version
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ResidencyRecord {
    key: InvalidationKey,
    state: Residency,
    last_used_unix: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ResidencyFile {
    version: u32,
    entries: BTreeMap<String, ResidencyRecord>,
}

/// Persistent residency tracker bound to one `residency.json`.
#[derive(Debug)]
pub struct ResidencyTracker {
    path: PathBuf,
    inner: BTreeMap<String, ResidencyRecord>,
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl ResidencyTracker {
    /// Load from disk. A missing or corrupt file yields an empty tracker:
    /// residency is an optimization, never a correctness record.
    pub fn load(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let inner = match std::fs::read_to_string(&path) {
            Ok(json) => match serde_json::from_str::<ResidencyFile>(&json) {
                Ok(file) if file.version == RESIDENCY_VERSION => file.entries,
                _ => BTreeMap::new(),
            },
            Err(_) => BTreeMap::new(),
        };
        Self { path, inner }
    }

    /// Persist current state; idempotent across restarts.
    pub fn persist(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = ResidencyFile {
            version: RESIDENCY_VERSION,
            entries: self.inner.clone(),
        };
        // Write-through via a temp file keeps the JSON never-half-written.
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&file)?)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    fn touch(&mut self, id: &str, key: InvalidationKey, state: Residency) {
        self.inner.insert(
            id.to_string(),
            ResidencyRecord {
                key,
                state,
                last_used_unix: now_unix(),
            },
        );
    }

    /// Record a Hot (in-use) handle for `id` under `key`.
    pub fn promote_hot(&mut self, id: &str, key: InvalidationKey) -> Result<Residency> {
        self.touch(id, key, Residency::Hot);
        self.persist()?;
        Ok(Residency::Hot)
    }

    /// Record that `id` was loaded recently (Warm).
    pub fn promote_warm(&mut self, id: &str, key: InvalidationKey) -> Result<Residency> {
        self.touch(id, key, Residency::Warm);
        self.persist()?;
        Ok(Residency::Warm)
    }

    /// Explicitly drop a model back to Cold (session end, eviction).
    pub fn cool(&mut self, id: &str) -> Result<Residency> {
        if let Some(record) = self.inner.get_mut(id) {
            record.state = Residency::Cold;
            record.last_used_unix = now_unix();
        }
        self.persist()?;
        Ok(self.state(id))
    }

    /// Current tracked state of `id` (Cold when absent).
    pub fn state(&self, id: &str) -> Residency {
        self.inner.get(id).map_or(Residency::Cold, |r| r.state)
    }

    /// Observe `id` under `key`. If the recorded key differs — new weights,
    /// runtime, backend, or tool version — the entry is invalidated back to
    /// Cold before answering. Returns `(state, invalidated_now)` and persists
    /// any demotion so a restart observes the same truth.
    pub fn state_for(&mut self, id: &str, key: InvalidationKey) -> Result<(Residency, bool)> {
        let mismatched = matches!(&self.inner.get(id), Some(record) if record.key != key);
        if mismatched {
            if let Some(record) = self.inner.get_mut(id) {
                record.key = key;
                record.state = Residency::Cold;
                record.last_used_unix = now_unix();
            }
            self.persist()?;
            return Ok((Residency::Cold, true));
        }
        Ok((self.state(id), false))
    }

    /// Ids currently Hot (in use).
    pub fn hot_ids(&self) -> Vec<String> {
        self.inner
            .iter()
            .filter(|(_, r)| r.state == Residency::Hot)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// File this tracker persists to.
    pub fn path(&self) -> &Path {
        &self.path
    }
}
