//! Model manager orchestration: background prefetch with policy gating
//! (ADR-0016/ADR-0015) and budget-aware cache enforcement.
//!
//! Invariants:
//! - Never auto-downloads without explicit `remote_allowed` user consent.
//! - Cache budget eviction is LRU, never evicts pinned or cpu_fallback.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::registry::RegistryEntry;
use crate::store::ModelStore;

/// Suggested runtime pack when NPU is enumerated but runtime absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimePackSuggestion {
    pub runtime: String,
    pub install_hint: String,
}

impl RuntimePackSuggestion {
    pub fn openvino() -> Self {
        Self {
            runtime: "openvino".into(),
            install_hint:
                "Install the verified OpenVINO 2026.4 runtime pack from official OpenVINO packages"
                    .into(),
        }
    }
}

/// Cache budget enforcement handle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheBudget {
    /// Maximum allowed bytes under `<cache_root>/models`. When exceeded,
    /// LRU unpinned non-fallback variants are evicted.
    pub max_bytes: u64,
    /// Fallback id that is always kept even if budget is exceeded.
    pub fallback_id: String,
}

impl CacheBudget {
    pub fn new(max_bytes: u64, fallback_id: impl Into<String>) -> Self {
        Self {
            max_bytes,
            fallback_id: fallback_id.into(),
        }
    }
}

/// Report from a prefetch attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrefetchReport {
    pub requested: Vec<String>,
    pub fetched: Vec<String>,
    pub skipped_policy: Vec<String>,
    pub errors: Vec<String>,
    pub remote_allowed: bool,
}

/// Orchestrator over a cache root, registry, and verified store.
pub struct ModelManager {
    cache_root: PathBuf,
    registry: crate::registry::ModelRegistry,
    store: ModelStore,
    fallback_id: String,
    remote_allowed: bool,
}

impl ModelManager {
    pub fn new(
        cache_root: PathBuf,
        registry: crate::registry::ModelRegistry,
        store: ModelStore,
    ) -> Self {
        Self {
            cache_root,
            registry,
            store,
            fallback_id: "whisper-tiny.en-Q8_0".to_string(),
            remote_allowed: false,
        }
    }

    pub fn with_fallback(mut self, fallback_id: impl Into<String>) -> Self {
        self.fallback_id = fallback_id.into();
        self
    }

    pub fn with_remote_allowed(mut self, allowed: bool) -> Self {
        self.remote_allowed = allowed;
        self
    }

    pub fn set_remote_allowed(&mut self, allowed: bool) {
        self.remote_allowed = allowed;
    }

    pub fn fallback_id(&self) -> &str {
        &self.fallback_id
    }

    pub fn cache_root(&self) -> &Path {
        &self.cache_root
    }

    /// Prefetch requested model ids in background (synchronous for testability).
    /// Respects `remote_allowed` policy gate per ADR-0016: when `remote_allowed`
    /// is false, no network fetch is attempted and ids are recorded as
    /// `skipped_policy`. When true, each id is resolved via registry and
    /// acquired through the verified store.
    ///
    /// This is the user-consent gate: background work never bypasses policy.
    pub fn prefetch_in_background(&self, ids: &[String]) -> Result<PrefetchReport> {
        self.prefetch_in_background_with_policy(ids, self.remote_allowed)
    }

    /// Explicit policy override for tests/callers that manage consent separately.
    pub fn prefetch_in_background_with_policy(
        &self,
        ids: &[String],
        remote_allowed: bool,
    ) -> Result<PrefetchReport> {
        let mut report = PrefetchReport {
            requested: ids.to_vec(),
            fetched: Vec::new(),
            skipped_policy: Vec::new(),
            errors: Vec::new(),
            remote_allowed,
        };
        if !remote_allowed {
            report.skipped_policy = ids.to_vec();
            return Ok(report);
        }
        for id in ids {
            let entry = match self.registry.get(id) {
                Ok(e) => e,
                Err(_) => {
                    report.errors.push(format!("unknown model id `{id}`"));
                    continue;
                }
            };
            // Presence alone is not installation truth. Skip only after exact
            // metadata, size, and digest verification.
            if self.store.verify_installed(entry).is_ok() {
                continue;
            }
            match self.store.acquire(entry) {
                Ok(_) => report.fetched.push(id.clone()),
                Err(e) => report.errors.push(format!("{id}: {e}")),
            }
        }
        Ok(report)
    }

    /// Enforce a cache budget: evict LRU unpinned variants when total cache
    /// exceeds `budget.max_bytes`, always keeping `budget.fallback_id` and any
    /// registry-pinned entries.
    pub fn enforce_budget(&self, budget: &CacheBudget) -> Result<crate::cleanup::CleanupReport> {
        enforce_cache_budget(&self.cache_root, self.registry.entries(), budget)
    }

    /// Convenience: suggest runtime pack when NPU present but runtime absent.
    pub fn runtime_suggestion_for_npu(
        present: bool,
        runtime_available: bool,
    ) -> Option<RuntimePackSuggestion> {
        if present && !runtime_available {
            Some(RuntimePackSuggestion::openvino())
        } else {
            None
        }
    }
}

/// Budget-aware cleanup: LRU eviction of unpinned, non-fallback variants
/// when total exceeds `budget.max_bytes`.
pub fn enforce_cache_budget(
    cache_root: &Path,
    entries: &[RegistryEntry],
    budget: &CacheBudget,
) -> Result<crate::cleanup::CleanupReport> {
    use std::collections::HashSet;

    let models_dir = cache_root.join("models");
    crate::store::validate_cache_root(cache_root)?;
    for entry in entries {
        entry.validate()?;
    }
    crate::store::validate_cache_tree(cache_root, &models_dir)?;
    // Collect installed variant dirs with size + mtime.
    let mut variants: Vec<(String, PathBuf, u64, std::time::SystemTime)> = Vec::new();
    let mut total: u64 = 0;

    // Helper to list family dirs.
    let families = list_dirs(&models_dir);
    for family_dir in families {
        for variant_dir in list_dirs(&family_dir) {
            let Some(id) = variant_dir
                .file_name()
                .and_then(|n| n.to_str())
                .map(str::to_string)
            else {
                continue;
            };
            // Size of this variant (sum of tree).
            let size = tree_bytes(&variant_dir);
            total += size;
            let mtime = std::fs::metadata(&variant_dir)
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            variants.push((id, variant_dir, size, mtime));
        }
    }

    if total <= budget.max_bytes {
        return Ok(crate::cleanup::CleanupReport {
            dry_run: false,
            removed: Vec::new(),
            kept_protected: vec![budget.fallback_id.clone()],
            removed_bytes: 0,
        });
    }

    // Protected set: fallback + pinned
    let mut protected: HashSet<String> = HashSet::new();
    protected.insert(budget.fallback_id.clone());
    for e in entries {
        if e.pinned {
            protected.insert(e.id.clone());
        }
    }

    // Sort by LRU (oldest first)
    variants.sort_by_key(|(_, _, _, mt)| *mt);

    let mut to_remove: Vec<PathBuf> = Vec::new();
    let mut removed_bytes: u64 = 0;

    for (id, path, size, _) in variants {
        if total <= budget.max_bytes {
            break;
        }
        if protected.contains(&id) {
            continue;
        }
        // Evict this variant
        total = total.saturating_sub(size);
        removed_bytes += size;
        to_remove.push(path);
    }

    let mut report = crate::cleanup::CleanupReport {
        dry_run: false,
        removed: Vec::new(),
        kept_protected: protected.into_iter().collect(),
        removed_bytes,
    };

    for p in &to_remove {
        let metadata = std::fs::symlink_metadata(p)?;
        if metadata.file_type().is_symlink() {
            return Err(crate::error::ModelManagerError::UnsafeCacheRoot);
        }
        if metadata.is_dir() {
            std::fs::remove_dir_all(p)?;
        } else if metadata.is_file() {
            std::fs::remove_file(p)?;
        } else {
            return Err(crate::error::ModelManagerError::UnsafeCacheRoot);
        }
        report.removed.push(p.clone());
        // cleanup empty family dirs
        if let Some(parent) = p.parent() {
            if is_empty_dir(parent) {
                std::fs::remove_dir(parent)?;
            }
        }
    }

    Ok(report)
}

fn list_dirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    out.sort();
    out
}

fn is_empty_dir(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|mut it| it.next().is_none())
}

fn tree_bytes(path: &Path) -> u64 {
    if path.is_file() {
        return std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    }
    list_dirs(path).iter().map(|c| tree_bytes(c)).sum::<u64>()
        + std::fs::read_dir(path)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter(|e| e.path().is_file())
                    .map(|e| e.metadata().map(|m| m.len()).unwrap_or(0))
                    .sum::<u64>()
            })
            .unwrap_or(0)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::ModelRegistry;
    use crate::policy::test_support::RecordingPolicy;
    use crate::registry::RegistryEntry;
    use crate::store::ModelStore;
    use crate::transport::Transport;
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[derive(Default)]
    struct FakeTransport {
        payloads: Mutex<BTreeMap<String, Vec<u8>>>,
    }
    impl FakeTransport {
        fn serve(&self, url: &str, bytes: &[u8]) {
            self.payloads
                .lock()
                .unwrap()
                .insert(url.to_string(), bytes.to_vec());
        }
    }
    impl Transport for FakeTransport {
        fn fetch(&self, url: &str, dest: &Path, _timeout: Duration) -> crate::error::Result<()> {
            let map = self.payloads.lock().unwrap();
            let Some(b) = map.get(url) else {
                return Err(crate::error::ModelManagerError::Transport {
                    url: url.to_string(),
                    message: "no such fake source".into(),
                });
            };
            std::fs::write(dest, b)?;
            Ok(())
        }
    }

    fn gguf_like(len: usize) -> Vec<u8> {
        let mut v = vec![b'G', b'G', b'U', b'F'];
        v.resize(len, 0);
        for (i, b) in v[4..].iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        v
    }
    fn hash_str(b: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(b);
        hex::encode(h.finalize())
    }

    const URL: &str = "https://mirror.fake.invalid/prefetch/test.gguf";
    fn entry_for(payload: &[u8], id: &str) -> RegistryEntry {
        let json = format!(
            r#"{{ "id": "{id}", "family": "whisper", "variant": "test", "quant": "Q8_0",
                 "size_bytes": {size}, "sha256": "{sha}", "license": "MIT",
                 "source_revision": "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
                 "sources": [{{"kind": "url", "repo_or_url": "{url}", "filename": "test.gguf"}}],
                 "capabilities": {{"languages": ["en"], "streaming": false}} }}"#,
            size = payload.len(),
            sha = hash_str(payload),
            url = URL,
        );
        serde_json::from_str(&json).unwrap()
    }

    #[test]
    fn prefetch_respects_remote_allowed_false() {
        let dir = tempfile::tempdir().unwrap();
        let payload = gguf_like(1024);
        let entry = entry_for(&payload, "whisper-test-Q8_0");
        let reg_json = serde_json::json!({"version":1, "entries":[ entry.clone()]});
        let registry =
            ModelRegistry::from_json_str(&reg_json.to_string(), Path::new("test")).unwrap();
        let transport = Arc::new(FakeTransport::default());
        transport.serve(URL, &payload);
        let store = ModelStore::new(
            dir.path().to_path_buf(),
            transport.clone(),
            Arc::new(RecordingPolicy::default()),
        );
        let manager =
            ModelManager::new(dir.path().to_path_buf(), registry, store).with_remote_allowed(false);

        let report = manager
            .prefetch_in_background(std::slice::from_ref(&entry.id))
            .unwrap();
        assert_eq!(report.skipped_policy, vec![entry.id.clone()]);
        assert!(
            report.fetched.is_empty(),
            "never auto-downloads when remote_allowed false"
        );
        assert!(
            !dir.path().join("models/whisper/whisper-test-Q8_0").exists(),
            "no download attempted"
        );
    }

    #[test]
    fn prefetch_downloads_when_remote_allowed_true() {
        let dir = tempfile::tempdir().unwrap();
        let payload = gguf_like(2048);
        let entry = entry_for(&payload, "whisper-test-Q8_0");
        let reg_json = serde_json::json!({"version":1, "entries":[ entry.clone()]});
        let registry =
            ModelRegistry::from_json_str(&reg_json.to_string(), Path::new("test")).unwrap();
        let transport = Arc::new(FakeTransport::default());
        transport.serve(URL, &payload);
        let store = ModelStore::new(
            dir.path().to_path_buf(),
            transport,
            Arc::new(RecordingPolicy::default()),
        );
        let manager =
            ModelManager::new(dir.path().to_path_buf(), registry, store).with_remote_allowed(true);

        let report = manager
            .prefetch_in_background(std::slice::from_ref(&entry.id))
            .unwrap();
        assert!(report.skipped_policy.is_empty());
        assert_eq!(report.fetched, vec![entry.id.clone()]);
        // Check installed via a fresh store view.
        let transport2 = Arc::new(FakeTransport::default());
        let store2 = ModelStore::new(
            dir.path().to_path_buf(),
            transport2,
            Arc::new(RecordingPolicy::default()),
        );
        let state = store2.state(&entry);
        assert!(matches!(
            state,
            crate::store::InstallState::PresentUnverified { .. }
        ));
        store2
            .verify_installed(&entry)
            .expect("prefetch result is exactly verified");
    }

    #[test]
    fn prefetch_repairs_present_but_unverified_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let payload = gguf_like(2048);
        let entry = entry_for(&payload, "whisper-test-Q8_0");
        let reg_json = serde_json::json!({"version":1, "entries":[ entry.clone()]});
        let registry =
            ModelRegistry::from_json_str(&reg_json.to_string(), Path::new("test")).unwrap();
        let transport = Arc::new(FakeTransport::default());
        transport.serve(URL, &payload);
        let store = ModelStore::new(
            dir.path().to_path_buf(),
            transport.clone(),
            Arc::new(RecordingPolicy::default()),
        );
        let binding = store.acquire(&entry).unwrap();
        let path = binding.path().to_path_buf();
        std::fs::write(&path, vec![b'x'; payload.len()]).unwrap();
        std::fs::write(
            path.parent().unwrap().join("meta.json.part"),
            b"interrupted-sidecar",
        )
        .unwrap();
        let manager =
            ModelManager::new(dir.path().to_path_buf(), registry, store).with_remote_allowed(true);

        let report = manager
            .prefetch_in_background(std::slice::from_ref(&entry.id))
            .unwrap();
        assert_eq!(report.fetched, vec![entry.id.clone()]);
        assert!(report.errors.is_empty());
        assert_eq!(std::fs::read(&path).unwrap(), payload);
        assert!(!path.parent().unwrap().join("meta.json.part").exists());
    }

    #[test]
    fn cache_budget_enforces_lru_and_keeps_fallback() {
        // 10 fake variants, each 100 bytes, budget 500 => must evict ~5 LRU, keep fallback pinned.
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path();
        let fallback_id = "whisper-tiny.en-Q8_0";
        let mut entries: Vec<RegistryEntry> = Vec::new();
        // create fallback + 9 others + 1 pinned loser
        for i in 0..10 {
            let id = if i == 0 {
                fallback_id.to_string()
            } else {
                format!("whisper-variant-{i:02}-Q8_0")
            };
            let pinned = i == 9; // last one pinned, should survive even though old
            let sha = {
                use sha2::{Digest, Sha256};
                let mut h = Sha256::new();
                h.update(id.as_bytes());
                hex::encode(h.finalize())
            };
            let e: RegistryEntry = serde_json::from_value(serde_json::json!({
                "id": id, "family": "whisper", "variant": "v", "quant": "Q8_0",
                "size_bytes": 100, "sha256": sha, "license": "MIT", "source_revision": "r",
                "sources": [{"kind": "url", "repo_or_url": format!("https://x/{id}"), "filename": "m"}],
                "capabilities": {"languages": [], "streaming": false},
                "pinned": pinned
            }))
            .unwrap();
            entries.push(e);
        }
        // Install each variant on disk with staggered mtimes.
        for (idx, e) in entries.iter().enumerate() {
            let transport = Arc::new(FakeTransport::default());
            let store = ModelStore::new(
                cache.to_path_buf(),
                transport,
                Arc::new(RecordingPolicy::default()),
            );
            let d = store.install_dir(e).unwrap();
            std::fs::create_dir_all(&d).unwrap();
            // Write 100 bytes
            std::fs::write(d.join("model.gguf"), vec![b'x'; 100]).unwrap();
            // Stagger mtime by sleeping a tiny amount and touching file time
            // Use metadata to order: we sleep 10ms between creations to ensure ordering.
            if idx < 9 {
                std::thread::sleep(std::time::Duration::from_millis(15));
            }
        }
        // Verify total 1000 bytes.
        let budget = CacheBudget::new(550, fallback_id);
        let manager = {
            let reg_json = serde_json::json!({"version":1, "entries": entries.clone()});
            let registry =
                ModelRegistry::from_json_str(&reg_json.to_string(), Path::new("test")).unwrap();
            let store = ModelStore::new(
                cache.to_path_buf(),
                Arc::new(FakeTransport::default()),
                Arc::new(RecordingPolicy::default()),
            );
            ModelManager::new(cache.to_path_buf(), registry, store).with_fallback(fallback_id)
        };
        let report = manager.enforce_budget(&budget).unwrap();
        // Should have removed at least 4-5 variants to get under 550
        assert!(
            !report.removed.is_empty(),
            "budget exceeded -> some evicted"
        );
        // Fallback must survive
        let fallback_entry = entries.iter().find(|e| e.id == fallback_id).unwrap();
        let store_check = ModelStore::new(
            cache.to_path_buf(),
            Arc::new(FakeTransport::default()),
            Arc::new(RecordingPolicy::default()),
        );
        // fallback install dir still exists
        let fallback_dir = store_check.install_dir(fallback_entry).unwrap();
        assert!(
            fallback_dir.join("model.gguf").exists(),
            "cpu fallback always kept"
        );
        // Pinned must survive
        let pinned_id = "whisper-variant-09-Q8_0";
        let pinned_entry = entries.iter().find(|e| e.id == pinned_id).unwrap();
        let pin_dir = store_check.install_dir(pinned_entry).unwrap();
        assert!(pin_dir.join("model.gguf").exists(), "pinned always kept");
        // Check total after enforcement <= budget + one variant slack (allow pinned/fallback)
        let total_after: u64 = entries
            .iter()
            .filter(|e| {
                store_check
                    .install_dir(e)
                    .map(|p| p.join("model.gguf").exists())
                    .unwrap_or(false)
            })
            .map(|_| 100u64)
            .sum();
        assert!(
            total_after <= 700,
            "total after {total_after} should be near budget 550 + fallback/pinned"
        );
    }

    #[test]
    fn runtime_suggestion_when_npu_present_runtime_absent() {
        let s = ModelManager::runtime_suggestion_for_npu(true, false);
        assert!(s.is_some());
        let s = s.unwrap();
        assert_eq!(s.runtime, "openvino");
        assert!(s.install_hint.contains("OpenVINO 2026.4"));
        assert!(ModelManager::runtime_suggestion_for_npu(false, false).is_none());
        assert!(ModelManager::runtime_suggestion_for_npu(true, true).is_none());
    }
}
