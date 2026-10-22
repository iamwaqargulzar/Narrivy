//! p5-model-cache scenario (SEC-002 / SEC-007): trusted registry acquisition,
//! checksum verification, corrupt-download recovery, cold-missing
//! re-acquisition. All transports are local; the network is never touched.

// Test code: asserts via panic!/expect! are the point; workspace production
// lints stay enforced on the library itself.
#![allow(clippy::panic, clippy::expect_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use narrivy_model_manager::error::ModelManagerError;
use narrivy_model_manager::policy::{CacheEntryKind, CacheFilePolicy, CacheOwner};
use narrivy_model_manager::{
    BundledModelRegistry, FileTransport, InstallState, InvalidationKey, ModelRegistry, ModelStore,
    NoNetworkTransport, RegistryEntry, Residency, ResidencyTracker, SourceKind, Transport,
};

/// The platform the integration suite runs against.
///
/// On a POSIX host the real policy is injected, so every scenario below proves
/// the same owner-only guarantee production relies on. Elsewhere the recording
/// policy answers from a script: the scenarios assert store behaviour, and the
/// permission *bits* are a platform concern with its own POSIX test.
#[cfg(unix)]
type TestPolicy = PosixPolicy;
#[cfg(not(unix))]
type TestPolicy = RecordingPolicy;

fn test_policy() -> Arc<dyn CacheFilePolicy> {
    Arc::new(TestPolicy::default())
}

/// A policy with no OS behaviour that records the obligations the store imposed
/// and can refuse any of them on demand.
struct RecordingPolicy {
    restricted: Mutex<Vec<(PathBuf, CacheEntryKind)>>,
    created: Mutex<Vec<PathBuf>>,
    restrict_failure: Option<String>,
    create_failure: Option<String>,
    foreign_descendant: bool,
    objects_match: bool,
    owner_queries: AtomicUsize,
    file_restrict_failure: Option<String>,
}

impl Default for RecordingPolicy {
    fn default() -> Self {
        Self {
            restricted: Mutex::new(Vec::new()),
            created: Mutex::new(Vec::new()),
            restrict_failure: None,
            create_failure: None,
            foreign_descendant: false,
            objects_match: true,
            owner_queries: AtomicUsize::new(0),
            file_restrict_failure: None,
        }
    }
}

impl RecordingPolicy {
    fn refusing_to_restrict(mut self, reason: &str) -> Self {
        self.restrict_failure = Some(reason.to_owned());
        self
    }

    /// Refuse only the owner-only obligation on regular files, so a scenario
    /// can reach the file-tightening step with directories already accepted.
    fn refusing_to_restrict_files(mut self, reason: &str) -> Self {
        self.file_restrict_failure = Some(reason.to_owned());
        self
    }

    fn refusing_to_create(mut self, reason: &str) -> Self {
        self.create_failure = Some(reason.to_owned());
        self
    }

    /// Report every directory below the cache root as owned by somebody else.
    /// The store always asks about the root first, so the first answer is the
    /// root's own identity and the next one is foreign.
    fn with_foreign_descendant(mut self) -> Self {
        self.foreign_descendant = true;
        self
    }

    /// Report that an opened handle is not the observed path.
    fn with_substituted_objects(mut self) -> Self {
        self.objects_match = false;
        self
    }
}

impl CacheFilePolicy for RecordingPolicy {
    fn restrict_private(
        &self,
        path: &Path,
        kind: CacheEntryKind,
    ) -> narrivy_model_manager::Result<()> {
        self.restricted
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push((path.to_path_buf(), kind));
        let refusal = match (&self.restrict_failure, &self.file_restrict_failure) {
            (Some(reason), _) => Some(reason.clone()),
            (None, Some(reason)) if kind == CacheEntryKind::File => Some(reason.clone()),
            _ => None,
        };
        match refusal {
            Some(reason) => Err(ModelManagerError::CacheProtectionUnavailable {
                path: path.display().to_string(),
                kind: match kind {
                    CacheEntryKind::File => "file",
                    CacheEntryKind::Directory => "directory",
                },
                reason,
            }),
            None => Ok(()),
        }
    }

    fn create_private_file(&self, path: &Path) -> narrivy_model_manager::Result<std::fs::File> {
        self.created
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(path.to_path_buf());
        match &self.create_failure {
            Some(reason) => Err(ModelManagerError::CacheProtectionUnavailable {
                path: path.display().to_string(),
                kind: "file",
                reason: reason.clone(),
            }),
            None => std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .map_err(Into::into),
        }
    }

    fn owner_of(&self, _metadata: &std::fs::Metadata) -> narrivy_model_manager::Result<CacheOwner> {
        let index = self.owner_queries.fetch_add(1, AtomicOrdering::Relaxed);
        if self.foreign_descendant && index > 0 {
            return Ok(CacheOwner::new("somebody-else"));
        }
        Ok(CacheOwner::new("owner-0"))
    }

    fn same_object(&self, _path: &std::fs::Metadata, _opened: &std::fs::Metadata) -> bool {
        self.objects_match
    }
}

/// The real POSIX guarantee: 0600 files, 0700 directories, a uid comparison for
/// directory ownership, and device+inode identity for the stat/open race.
///
/// It lives in the integration target rather than in `src/` on purpose: the
/// library must contain no platform implementation, so the product copy of this
/// policy is the one in `adapters/platform-linux`.
#[cfg(unix)]
#[derive(Default)]
struct PosixPolicy;

#[cfg(unix)]
impl PosixPolicy {
    fn fail(path: &Path, kind: CacheEntryKind, reason: &str) -> ModelManagerError {
        ModelManagerError::CacheProtectionUnavailable {
            path: path.display().to_string(),
            kind: match kind {
                CacheEntryKind::File => "file",
                CacheEntryKind::Directory => "directory",
            },
            reason: reason.to_owned(),
        }
    }
}

#[cfg(unix)]
impl CacheFilePolicy for PosixPolicy {
    fn restrict_private(
        &self,
        path: &Path,
        kind: CacheEntryKind,
    ) -> narrivy_model_manager::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let mode = match kind {
            CacheEntryKind::File => 0o600,
            CacheEntryKind::Directory => 0o700,
        };
        let mut permissions = std::fs::metadata(path)
            .map_err(|error| Self::fail(path, kind, &error.to_string()))?
            .permissions();
        permissions.set_mode(mode);
        std::fs::set_permissions(path, permissions)
            .map_err(|error| Self::fail(path, kind, &error.to_string()))
    }

    fn create_private_file(&self, path: &Path) -> narrivy_model_manager::Result<std::fs::File> {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(|error| Self::fail(path, CacheEntryKind::File, &error.to_string()))
    }

    fn owner_of(&self, metadata: &std::fs::Metadata) -> narrivy_model_manager::Result<CacheOwner> {
        use std::os::unix::fs::MetadataExt;
        Ok(CacheOwner::new(metadata.uid().to_string()))
    }

    fn same_object(&self, path: &std::fs::Metadata, opened: &std::fs::Metadata) -> bool {
        use std::os::unix::fs::MetadataExt;
        path.dev() == opened.dev() && path.ino() == opened.ino()
    }
}

/// Minimal GGUF-shaped payload: magic bytes + deterministic filler.
fn gguf_like(len: usize) -> Vec<u8> {
    let mut v = vec![b'G', b'G', b'U', b'F'];
    v.resize(len, 0);
    for (i, b) in v[4..].iter_mut().enumerate() {
        *b = (i % 251) as u8;
    }
    v
}

/// Fake transport: serves in-memory byte maps keyed by URL. `set_corrupt`
/// flips one payload byte to simulate a tampered mirror without touching disk.
#[derive(Default)]
struct FakeTransport {
    payloads: Mutex<BTreeMap<String, Vec<u8>>>,
    corrupt: Mutex<BTreeMap<String, bool>>,
}

impl FakeTransport {
    fn serve(&self, url: &str, bytes: &[u8]) {
        self.payloads
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(url.to_string(), bytes.to_vec());
    }
    fn set_corrupt(&self, url: &str, on: bool) {
        self.corrupt
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(url.to_string(), on);
    }
}

impl Transport for FakeTransport {
    fn fetch(
        &self,
        url: &str,
        dest: &Path,
        _timeout: Duration,
    ) -> narrivy_model_manager::Result<()> {
        let corrupt = *self
            .corrupt
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(url)
            .unwrap_or(&false);
        let payloads = self.payloads.lock().unwrap_or_else(|e| e.into_inner());
        let Some(bytes) = payloads.get(url) else {
            return Err(ModelManagerError::Transport {
                url: url.to_string(),
                message: "no such fake source".to_string(),
            });
        };
        let mut out = bytes.clone();
        if corrupt && !out.is_empty() {
            let last = out.len() - 1;
            out[last] ^= 0xFF; // tamper exactly one byte
        }
        std::fs::write(dest, out)?;
        Ok(())
    }
}

/// Build an entry whose single source is a `file://` URL over `payload`,
/// pinned to its true sha256.
fn file_entry(id: &str, family: &str, payload: &[u8], source_file: &Path) -> RegistryEntry {
    let json = format!(
        r#"{{
            "id": "{id}", "family": "{family}", "variant": "test", "quant": "Q8_0",
            "size_bytes": {size}, "sha256": "{sha}", "license": "MIT",
            "source_revision": "0000000000000000000000000000000000000000",
            "sources": [{{"kind": "url", "repo_or_url": "file://{source}", "filename": "model.gguf"}}],
            "capabilities": {{"languages": ["en"], "streaming": false}}
        }}"#,
        id = id,
        family = family,
        size = payload.len(),
        sha = hash_str(payload),
        source = source_file.display(),
    );
    serde_json::from_str::<RegistryEntry>(&json).expect("test entry parses")
}

fn hash_str(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// https scheme passes the registry trust gate; the fake transport keys on
/// this exact string, so no network I/O ever happens.
const URL: &str = "https://mirror.fake.invalid/narrivy-test/test.gguf";

/// Entry served by the shared fake transport at `URL`.
fn fake_entry(payload_sha: &str, size: usize) -> RegistryEntry {
    let json = format!(
        r#"{{
            "id": "whisper-test-Q8_0", "family": "whisper", "variant": "test", "quant": "Q8_0",
            "size_bytes": {size}, "sha256": "{sha}", "license": "MIT",
            "source_revision": "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
            "sources": [{{"kind": "url", "repo_or_url": "{url}", "filename": "test.gguf"}}],
            "capabilities": {{"languages": ["en"], "streaming": false}}
        }}"#,
        sha = payload_sha,
        url = URL,
    );
    serde_json::from_str::<RegistryEntry>(&json).expect("test entry parses")
}

fn store_with_fake() -> (tempfile::TempDir, Arc<FakeTransport>, ModelStore) {
    let root = tempfile::tempdir().expect("tempdir");
    let transport = Arc::new(FakeTransport::default());
    let store = ModelStore::new(root.path().to_path_buf(), transport.clone(), test_policy());
    (root, transport, store)
}

fn staging_entries(root: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(root.join("staging"))
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).collect())
        .unwrap_or_default()
}

/// Scenario step 1: happy acquire publishes and re-observes verified bytes.
#[test]
fn happy_acquire_installs_verified_bytes() {
    let (_root, transport, store) = store_with_fake();
    let payload = gguf_like(4096);
    let entry = fake_entry(&hash_str(&payload), payload.len());
    transport.serve(URL, &payload);

    assert_eq!(store.state(&entry), InstallState::ColdMissing);

    let acquired = store.acquire(&entry).expect("happy acquire");
    let path = acquired.path().to_path_buf();
    assert!(path.is_file());
    assert_eq!(
        path,
        store
            .cache_root()
            .join("models")
            .join("whisper")
            .join("whisper-test-Q8_0")
            .join(&entry.sha256.as_ref().expect("pinned")[..12])
            .join("model.gguf")
    );
    assert_eq!(std::fs::read(&path).expect("read installed"), payload);
    assert_eq!(
        store.state(&entry),
        InstallState::PresentUnverified { path: path.clone() }
    );
    assert!(
        staging_entries(store.cache_root()).is_empty(),
        "staging must be empty after install"
    );
    let binding = store
        .verify_installed(&entry)
        .expect("exact installed binding");
    assert_eq!(binding.path(), path);
    assert_eq!(binding.id(), entry.id);
    assert_eq!(binding.family(), entry.family);
    assert_eq!(binding.source_revision(), entry.source_revision);
    assert_eq!(binding.sha256(), entry.sha256.as_deref().expect("pin"));
    assert_eq!(binding.size_bytes(), entry.size_bytes);
    let debug = format!("{binding:?}");
    assert!(!debug.contains(store.cache_root().to_string_lossy().as_ref()));
}

#[test]
fn installed_presence_never_upgrades_tampered_bytes_or_metadata() {
    let (_root, transport, store) = store_with_fake();
    let payload = gguf_like(4096);
    let entry = fake_entry(&hash_str(&payload), payload.len());
    transport.serve(URL, &payload);
    let acquired = store.acquire(&entry).expect("acquire");
    let path = acquired.path().to_path_buf();

    let mut changed = payload.clone();
    let last = changed.len() - 1;
    changed[last] ^= 0x55;
    std::fs::write(&path, changed).expect("same-size tamper");
    assert!(matches!(
        store.state(&entry),
        InstallState::PresentUnverified { .. }
    ));
    assert!(matches!(
        store.verify_installed(&entry),
        Err(ModelManagerError::InstalledDigestMismatch { .. })
    ));

    std::fs::write(&path, &payload).expect("restore exact bytes");
    let metadata = path.parent().expect("install dir").join("meta.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&metadata).expect("read installed metadata"))
            .expect("metadata JSON");
    value["source_revision"] = serde_json::Value::String("wrong-revision".into());
    std::fs::write(
        metadata,
        serde_json::to_vec(&value).expect("serialize metadata"),
    )
    .expect("tamper metadata");
    assert!(matches!(
        store.verify_installed(&entry),
        Err(ModelManagerError::InstalledMetadataMismatch { .. })
    ));
}

#[test]
fn oversized_installed_metadata_is_rejected_before_json_parsing() {
    let (_root, transport, store) = store_with_fake();
    let payload = gguf_like(1024);
    let entry = fake_entry(&hash_str(&payload), payload.len());
    transport.serve(URL, &payload);
    let acquired = store.acquire(&entry).expect("acquire");
    let path = acquired.path().to_path_buf();
    let metadata = path.parent().expect("install dir").join("meta.json");
    std::fs::write(metadata, vec![b' '; 16 * 1024 + 1]).expect("oversized metadata");

    assert!(matches!(
        store.verify_installed(&entry),
        Err(ModelManagerError::UnsafeInstalledModel { .. })
    ));
}

#[cfg(unix)]
#[test]
fn installed_symlink_is_rejected_even_when_target_bytes_match() {
    let (_root, transport, store) = store_with_fake();
    let payload = gguf_like(1024);
    let entry = fake_entry(&hash_str(&payload), payload.len());
    transport.serve(URL, &payload);
    let acquired = store.acquire(&entry).expect("acquire");
    let path = acquired.path().to_path_buf();
    let target = path.with_file_name("replacement.gguf");
    std::fs::rename(&path, &target).expect("move exact model");
    std::os::unix::fs::symlink(&target, &path).expect("symlink model");
    assert!(matches!(
        store.verify_installed(&entry),
        Err(ModelManagerError::UnsafeInstalledModel { .. })
    ));
}

#[test]
fn transport_size_mismatch_never_publishes() {
    let (_root, transport, store) = store_with_fake();
    let payload = gguf_like(1024);
    let mut entry = fake_entry(&hash_str(&payload), payload.len());
    entry.size_bytes += 1;
    transport.serve(URL, &payload);
    assert!(matches!(
        store.acquire(&entry),
        Err(ModelManagerError::InstalledSizeMismatch { .. })
    ));
    assert_eq!(store.state(&entry), InstallState::ColdMissing);
    assert!(staging_entries(store.cache_root()).is_empty());
}

#[test]
fn uppercase_registry_digest_is_canonicalized_for_storage_and_binding() {
    let (_root, transport, store) = store_with_fake();
    let payload = gguf_like(1024);
    let mut entry = fake_entry(&hash_str(&payload), payload.len());
    entry.sha256 = entry.sha256.map(|digest| digest.to_ascii_uppercase());
    transport.serve(URL, &payload);

    let binding = store.acquire(&entry).expect("uppercase pinned acquire");
    assert_eq!(binding.sha256(), hash_str(&payload));
    assert!(
        binding
            .path()
            .components()
            .any(|component| component.as_os_str() == &hash_str(&payload)[..12])
    );
}

#[test]
fn registry_and_direct_store_reject_unsafe_identity_components() {
    let payload = gguf_like(64);
    let source = tempfile::NamedTempFile::new().expect("source");
    let mut entry = file_entry("safe", "family", &payload, source.path());
    entry.id = "../escape".into();
    let registry = serde_json::json!({"version": 1, "entries": [entry.clone()]});
    assert!(matches!(
        ModelRegistry::from_json_str(&registry.to_string(), Path::new("test")),
        Err(ModelManagerError::InvalidRegistryEntry { .. })
    ));
    let root = tempfile::tempdir().expect("cache");
    let store = ModelStore::new(
        root.path().to_path_buf(),
        Arc::new(FileTransport),
        test_policy(),
    );
    assert!(matches!(
        store.install_dir(&entry),
        Err(ModelManagerError::InvalidRegistryEntry { .. })
    ));
}

#[cfg(unix)]
#[test]
fn cache_root_symlink_is_rejected_before_transport() {
    let parent = tempfile::tempdir().expect("parent");
    let real = parent.path().join("real-cache");
    std::fs::create_dir(&real).expect("real cache");
    let link = parent.path().join("cache-link");
    std::os::unix::fs::symlink(&real, &link).expect("cache symlink");
    let payload = gguf_like(128);
    let entry = fake_entry(&hash_str(&payload), payload.len());
    let transport = Arc::new(FakeTransport::default());
    transport.serve(URL, &payload);
    let store = ModelStore::new(link, transport, test_policy());
    assert!(matches!(
        store.acquire(&entry),
        Err(ModelManagerError::UnsafeCacheRoot)
    ));
    assert!(
        std::fs::read_dir(real)
            .expect("real cache remains")
            .next()
            .is_none()
    );
}

#[cfg(unix)]
#[test]
fn cache_descendant_symlinks_cannot_redirect_acquisition() {
    for descendant in ["staging", "models", "models/whisper"] {
        let root = tempfile::tempdir().expect("cache");
        let outside = tempfile::tempdir().expect("outside");
        let link = root.path().join(descendant);
        std::fs::create_dir_all(link.parent().expect("link parent")).expect("parents");
        std::os::unix::fs::symlink(outside.path(), &link).expect("descendant symlink");
        let payload = gguf_like(128);
        let entry = fake_entry(&hash_str(&payload), payload.len());
        let transport = Arc::new(FakeTransport::default());
        transport.serve(URL, &payload);
        let store = ModelStore::new(root.path().to_path_buf(), transport, test_policy());

        assert!(matches!(
            store.acquire(&entry),
            Err(ModelManagerError::UnsafeCacheRoot)
        ));
        assert!(
            std::fs::read_dir(outside.path())
                .expect("outside remains")
                .next()
                .is_none(),
            "{descendant} must not redirect a write"
        );
    }
}

#[cfg(unix)]
#[test]
fn cleanup_and_budget_reject_symlinked_cache_trees() {
    let root = tempfile::tempdir().expect("cache");
    let outside = tempfile::tempdir().expect("outside");
    let sentinel = outside.path().join("sentinel");
    std::fs::write(&sentinel, b"keep").expect("sentinel");
    std::os::unix::fs::symlink(outside.path(), root.path().join("staging"))
        .expect("staging symlink");
    let entry = fake_entry(&"0".repeat(64), 1);

    assert!(matches!(
        narrivy_model_manager::remove_unreferenced(
            std::slice::from_ref(&entry),
            root.path(),
            &[],
            false
        ),
        Err(ModelManagerError::UnsafeCacheRoot)
    ));
    assert!(sentinel.is_file());

    std::fs::remove_file(root.path().join("staging")).expect("remove test symlink");
    std::os::unix::fs::symlink(outside.path(), root.path().join("models")).expect("models symlink");
    let budget = narrivy_model_manager::CacheBudget {
        max_bytes: 0,
        fallback_id: entry.id.clone(),
    };
    assert!(matches!(
        narrivy_model_manager::enforce_cache_budget(root.path(), &[entry], &budget),
        Err(ModelManagerError::UnsafeCacheRoot)
    ));
    assert!(sentinel.is_file());
}

/// Scenario step 2: tampered transport bytes yield typed CorruptDownload,
/// staging is cleaned, nothing activates; retry after fix succeeds.
#[test]
fn tampered_transport_gives_corrupt_download_then_retry_after_fix() {
    let (_root, transport, store) = store_with_fake();
    let payload = gguf_like(2048);
    let entry = fake_entry(&hash_str(&payload), payload.len());
    transport.serve(URL, &payload);
    transport.set_corrupt(URL, true);

    let err = store.acquire(&entry).expect_err("tampered bytes must fail");
    match err {
        ModelManagerError::CorruptDownload {
            expected, actual, ..
        } => {
            assert_eq!(&expected, entry.sha256.as_ref().expect("pin"));
            assert_ne!(actual, *expected);
        }
        other => panic!("expected CorruptDownload, got {other:?}"),
    }

    assert!(
        staging_entries(store.cache_root()).is_empty(),
        ".part removed on corruption"
    );
    assert_eq!(
        store.state(&entry),
        InstallState::ColdMissing,
        "nothing activated"
    );

    transport.set_corrupt(URL, false);
    let acquired = store.acquire(&entry).expect("retry after fix");
    let path = acquired.path().to_path_buf();
    assert_eq!(std::fs::read(&path).expect("read"), payload);
    assert_eq!(
        store.state(&entry),
        InstallState::PresentUnverified { path }
    );
}

/// Scenario step 3: deleting an installed file returns Cold-missing and a
/// fresh acquire restores it.
#[test]
fn deleted_install_returns_cold_missing_and_reacquires() {
    let (_root, transport, store) = store_with_fake();
    let payload = gguf_like(1024);
    let entry = fake_entry(&hash_str(&payload), payload.len());
    transport.serve(URL, &payload);

    let acquired = store.acquire(&entry).expect("first acquire");
    let path = acquired.path().to_path_buf();
    std::fs::remove_file(&path).expect("delete installed file");

    assert_eq!(
        store.state(&entry),
        InstallState::ColdMissing,
        "deleted bytes => Cold-missing"
    );
    assert!(!path.exists());

    let reacquired = store.acquire(&entry).expect("re-acquire");
    let repath = reacquired.path().to_path_buf();
    assert_eq!(repath, path);
    assert_eq!(std::fs::read(&repath).expect("read"), payload);
    assert_eq!(
        store.state(&entry),
        InstallState::PresentUnverified { path: repath }
    );
}

/// SEC-002: entries without a checksum pin are never acquirable.
#[test]
fn unpinned_entry_is_refused() {
    let (_root, _transport, store) = store_with_fake();
    let mut entry = fake_entry(&"0".repeat(64), 16);
    entry.sha256 = None;
    match store.acquire(&entry) {
        Err(ModelManagerError::ChecksumRequired(id)) => assert_eq!(id, entry.id),
        other => panic!("expected ChecksumRequired, got {other:?}"),
    }
    assert_eq!(store.state(&entry), InstallState::ColdMissing);
}

/// SEC-007: arbitrary non-trusted URLs are refused before any I/O.
#[test]
fn untrusted_url_kinds_are_refused() {
    for bad in ["http://evil.example/m.gguf", "ftp://x/y", "/absolute/path"] {
        let json = format!(
            r#"{{ "id": "x", "family": "f", "variant": "v", "quant": "Q8_0",
                 "size_bytes": 1, "license": "MIT",
                 "source_revision": "r",
                 "sources": [{{"kind": "url", "repo_or_url": "{bad}", "filename": "m"}}],
                 "capabilities": {{"languages": [], "streaming": false}} }}"#,
            bad = bad.replace('"', ""),
        );
        let entry: RegistryEntry = serde_json::from_str(&json).expect("parses");
        match entry.resolve_url() {
            Err(
                ModelManagerError::UntrustedSourceUrl(_)
                | ModelManagerError::InvalidRegistryEntry { .. },
            ) => {}
            other => panic!("expected UntrustedSourceUrl for {bad}, got {other:?}"),
        }
    }
    // HF sources resolve through the immutable revision pin.
    let json = r#"{ "id": "h", "family": "f", "variant": "v", "quant": "Q8_0",
        "size_bytes": 1, "license": "MIT",
        "source_revision": "0123456789abcdef0123456789abcdef01234567",
        "sources": [{"kind": "hf", "repo_or_url": "org/repo", "filename": "m.gguf"}],
        "capabilities": {"languages": [], "streaming": false} }"#;
    let entry: RegistryEntry = serde_json::from_str(json).expect("parses");
    assert_eq!(
        entry.resolve_url().expect("hf resolves"),
        "https://huggingface.co/org/repo/resolve/0123456789abcdef0123456789abcdef01234567/m.gguf"
    );
}

#[test]
fn registry_ingestion_rejects_oversized_documents_before_parsing() {
    let oversized = " ".repeat(1024 * 1024 + 1);
    assert!(matches!(
        ModelRegistry::from_json_str(&oversized, Path::new("oversized.json")),
        Err(ModelManagerError::RegistryParse { .. })
    ));

    let file = tempfile::NamedTempFile::new().expect("registry temp file");
    std::fs::write(file.path(), vec![0xff; 1024 * 1024 + 1]).expect("oversized bytes");
    assert!(matches!(
        ModelRegistry::from_path(file.path()),
        Err(ModelManagerError::RegistryParse { .. })
    ));
}

#[test]
fn registry_ingestion_bounds_entries_sources_languages_and_source_fields() {
    let payload = gguf_like(64);
    let mut entry = fake_entry(&hash_str(&payload), payload.len());

    let too_many_entries = serde_json::json!({"version": 1, "entries": vec![entry.clone(); 257]});
    assert!(matches!(
        ModelRegistry::from_json_str(&too_many_entries.to_string(), Path::new("entries.json")),
        Err(ModelManagerError::RegistryParse { .. })
    ));

    entry.sources = vec![entry.sources[0].clone(); 9];
    let too_many_sources = serde_json::json!({"version": 1, "entries": [entry.clone()]});
    assert!(matches!(
        ModelRegistry::from_json_str(&too_many_sources.to_string(), Path::new("sources.json")),
        Err(ModelManagerError::InvalidRegistryEntry { .. })
    ));

    entry = fake_entry(&hash_str(&payload), payload.len());
    entry.capabilities.languages = vec!["en".to_string(); 65];
    let too_many_languages = serde_json::json!({"version": 1, "entries": [entry.clone()]});
    assert!(matches!(
        ModelRegistry::from_json_str(&too_many_languages.to_string(), Path::new("languages.json")),
        Err(ModelManagerError::InvalidRegistryEntry { .. })
    ));

    entry = fake_entry(&hash_str(&payload), payload.len());
    entry.sources[0].repo_or_url = format!("https://example.invalid/{}", "x".repeat(4096));
    let oversized_source = serde_json::json!({"version": 1, "entries": [entry]});
    assert!(matches!(
        ModelRegistry::from_json_str(
            &oversized_source.to_string(),
            Path::new("source-field.json")
        ),
        Err(ModelManagerError::InvalidRegistryEntry { .. })
    ));

    let mut entry = fake_entry(&hash_str(&payload), payload.len());
    entry.sources[0].filename = format!("{}.gguf", "x".repeat(256));
    let oversized_filename = serde_json::json!({"version": 1, "entries": [entry]});
    assert!(matches!(
        ModelRegistry::from_json_str(
            &oversized_filename.to_string(),
            Path::new("source-filename.json")
        ),
        Err(ModelManagerError::InvalidRegistryEntry { .. })
    ));

    let mut entry = fake_entry(&hash_str(&payload), payload.len());
    entry.capabilities.languages = vec!["x".repeat(65)];
    let oversized_language = serde_json::json!({"version": 1, "entries": [entry]});
    assert!(matches!(
        ModelRegistry::from_json_str(
            &oversized_language.to_string(),
            Path::new("language-field.json")
        ),
        Err(ModelManagerError::InvalidRegistryEntry { .. })
    ));
}

#[test]
fn hf_sources_require_exact_repo_shape_and_commit_revision() {
    let payload = gguf_like(64);
    let mut entry = fake_entry(&hash_str(&payload), payload.len());
    entry.sources[0].kind = SourceKind::Hf;
    entry.source_revision = "0123456789abcdef0123456789abcdef01234567".into();

    for bad_repo in [
        "org",
        "org/repo/extra",
        "org/../repo",
        "org//repo",
        "org/repo?download=1",
        "org/repo#fragment",
    ] {
        entry.sources[0].repo_or_url = bad_repo.into();
        assert!(
            matches!(
                entry.resolve_url(),
                Err(ModelManagerError::InvalidRegistryEntry { .. })
            ),
            "accepted malformed HF repo {bad_repo}"
        );
    }

    entry.sources[0].repo_or_url = "org/repo".into();
    for bad_revision in [
        "0123456789abcdef0123456789abcdef0123456",
        "0123456789abcdef0123456789abcdef012345678",
        "0123456789abcdef0123456789abcdef0123456g",
        "0123456789abcdef0123456789abcdef012345#7",
    ] {
        entry.source_revision = bad_revision.into();
        assert!(
            matches!(
                entry.resolve_url(),
                Err(ModelManagerError::InvalidRegistryEntry { .. })
            ),
            "accepted malformed HF revision {bad_revision}"
        );
    }
}

#[test]
fn direct_urls_require_structurally_safe_https_or_absolute_file_urls() {
    let payload = gguf_like(64);
    let mut entry = fake_entry(&hash_str(&payload), payload.len());
    for bad_url in [
        "https:///missing-authority/model.gguf",
        "https://bad host/model.gguf",
        "https://user@example.invalid/model.gguf",
        "https://example.invalid\\model.gguf",
        "https://example.invalid:/model.gguf",
        "https://example.invalid:65536/model.gguf",
        "https://[:::]/model.gguf",
        "file://relative/model.gguf",
        "file:///tmp/model.gguf?query=1",
        "file:///tmp/model.gguf#fragment",
        "file:///tmp/../model.gguf",
    ] {
        entry.sources[0].repo_or_url = bad_url.into();
        assert!(
            matches!(
                entry.resolve_url(),
                Err(ModelManagerError::InvalidRegistryEntry { .. })
            ),
            "accepted malformed direct URL {bad_url}"
        );
    }

    entry.sources[0].repo_or_url = "https://example.invalid/model.gguf?mirror=1#pin".into();
    assert!(entry.resolve_url().is_ok(), "valid HTTPS URL was rejected");
    entry.sources[0].repo_or_url = "file:///tmp/model.gguf".into();
    assert!(entry.resolve_url().is_ok(), "valid file URL was rejected");
}

/// PERF-007: residency Hot/Warm/Cold with versioned invalidation.
#[test]
fn residency_hot_warm_cold_with_invalidation() {
    let root = tempfile::tempdir().expect("tempdir");
    let path = root.path().join("residency.json");
    let key_a = InvalidationKey {
        model_sha256: "aaa".into(),
        runtime: "transcribe.cpp".into(),
        backend: "cpu".into(),
        tool_version: "0.2.1".into(),
    };
    let key_backend_b = InvalidationKey {
        backend: "vulkan".into(),
        ..key_a.clone()
    };

    {
        let mut tracker = ResidencyTracker::load(&path);
        tracker.promote_hot("m1", key_a.clone()).expect("hot");
        tracker.persist().expect("persist");
        assert_eq!(
            tracker.state_for("m1", key_a.clone()).expect("state"),
            (Residency::Hot, false)
        );
    } // drop: restart-reconcilable read-back below

    let mut tracker = ResidencyTracker::load(&path);
    assert_eq!(
        tracker.state_for("m1", key_a.clone()).expect("same key"),
        (Residency::Hot, false)
    );

    // Changing any invalidation-key component demotes Warm/Hot back to Cold.
    assert_eq!(
        tracker
            .state_for("m1", key_backend_b)
            .expect("changed backend"),
        (Residency::Cold, true),
        "backend change must invalidate"
    );
    assert_eq!(
        tracker
            .state_for("m1", key_a.clone())
            .expect("back to old key"),
        (Residency::Cold, true)
    );
    assert!(tracker.hot_ids().is_empty());

    tracker.promote_warm("m2", key_a.clone()).expect("warm");
    assert_eq!(tracker.state("m2"), Residency::Warm);
    tracker.cool("m2").expect("cool");
    assert_eq!(tracker.state("m2"), Residency::Cold);

    // A corrupt residency file degrades to empty, never blocks work.
    std::fs::write(&path, "not json{").expect("corrupt file");
    let tracker = ResidencyTracker::load(&path);
    assert_eq!(tracker.state("m1"), Residency::Cold);
}

/// PERF-008: cleanup removes staging leftovers and losing variants while the
/// protected set survives; dry-run touches nothing; re-runs are no-ops.
#[test]
fn cleanup_removes_losers_and_protects_selected_pinned_fallback() {
    let root = tempfile::tempdir().expect("tempdir");
    let cache = root.path();

    let mk = |id: &str, family: &str, pinned: bool| -> RegistryEntry {
        let sha = {
            use sha2::{Digest, Sha256};
            let mut h = Sha256::new();
            h.update(id.as_bytes());
            hex::encode(h.finalize())
        };
        serde_json::from_value(serde_json::json!({
            "id": id, "family": family, "variant": "v", "quant": "Q8_0",
            "size_bytes": 8, "sha256": sha, "license": "MIT", "source_revision": "r",
            "sources": [{"kind": "url", "repo_or_url": format!("https://x/{}", id), "filename": "m"}],
            "capabilities": {"languages": [], "streaming": false},
            "pinned": pinned
        }))
        .expect("entry")
    };

    let selected = mk("whisper-selected", "whisper", false); // source-of-selected
    let fallback = mk("whisper-cpu-fallback", "whisper", false); // CPU fallback
    let pinned = mk("parakeet-pinned", "parakeet", true); // user pin
    let loser = mk("whisper-loser-bench", "whisper", false); // benchmark loser

    let install = |store: &ModelStore, entry: &RegistryEntry| {
        let dir = store.install_dir(entry).expect("install dir");
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("model.gguf"), b"12345678").expect("bytes");
    };

    let transport: Arc<FakeTransport> = Arc::new(FakeTransport::default());
    let store = ModelStore::new(cache.to_path_buf(), transport, test_policy());
    for e in [&selected, &fallback, &pinned, &loser] {
        install(&store, e);
    }
    std::fs::create_dir_all(store.staging_dir()).expect("staging mkdir");
    std::fs::write(store.staging_dir().join("leftover.part"), b"x").expect("leftover");

    // Dry run reports but does not delete.
    let report = narrivy_model_manager::remove_unreferenced(
        &[
            selected.clone(),
            fallback.clone(),
            pinned.clone(),
            loser.clone(),
        ],
        cache,
        &["whisper-selected", "whisper-cpu-fallback"],
        true,
    )
    .expect("dry-run report");
    assert!(!report.removed.is_empty(), "dry-run lists what would go");
    assert!(
        loser_path(cache, "whisper").is_some(),
        "dry-run keeps files"
    );

    // Real run removes the loser + staging, protects the rest.
    let report = narrivy_model_manager::remove_unreferenced(
        &[selected.clone(), fallback.clone(), pinned.clone(), loser],
        cache,
        &["whisper-selected", "whisper-cpu-fallback"],
        false,
    )
    .expect("cleanup");
    assert!(
        loser_path(cache, "whisper").is_none(),
        "losing variant removed"
    );
    assert!(
        !cache.join("staging/leftover.part").exists(),
        "staging leftover removed"
    );
    for kept in [&selected, &fallback, &pinned] {
        assert!(
            store.model_path(kept).is_ok_and(|p| p.is_file()),
            "protected {} survives",
            kept.id
        );
    }
    assert_eq!(
        report
            .kept_protected
            .iter()
            .filter(|k| **k == "parakeet-pinned")
            .count(),
        1,
        "registry pin protected even though not listed in keep_ids"
    );

    // Idempotent: second pass removes nothing.
    let report = narrivy_model_manager::remove_unreferenced(
        &[selected, fallback, pinned],
        cache,
        &["whisper-selected", "whisper-cpu-fallback"],
        false,
    )
    .expect("second cleanup");
    assert!(
        report.removed.is_empty(),
        "restart-reconcilable: second run is a no-op"
    );
}

fn loser_path(cache: &Path, family: &str) -> Option<PathBuf> {
    let dir = cache
        .join("models")
        .join(family)
        .join("whisper-loser-bench");
    dir.exists().then_some(dir)
}

/// The bundled seed registry parses and matches its documented shape.
#[test]
fn seed_registry_is_valid_and_pinned() {
    let registry = ModelRegistry::seeded();
    let entries = registry.entries();
    assert!(
        entries.len() >= 4,
        "installed tiny + >=3 declared candidates"
    );
    let tiny = registry.get("whisper-tiny.en-Q8_0").expect("tiny present");
    assert_eq!(tiny.license, "Apache-2.0");
    assert_eq!(
        tiny.sha256.as_deref(),
        Some("e8c9b73c06344307d8b346e07fbe93dd88d894627854bcff31523f1ce44394fa"),
        "seed pins the sha256 of the existing local whisper-tiny.en-Q8_0.gguf"
    );
    for entry in entries {
        assert!(
            !entry.source_revision.is_empty(),
            "{} must pin a revision",
            entry.id
        );
        assert!(
            !entry.sources.is_empty(),
            "{} must declare sources",
            entry.id
        );
        if entry.unqualified {
            continue; // candidates may lack hashes until qualification
        }
        assert!(
            entry.sha256.is_some(),
            "{} qualified entries need hashes",
            entry.id
        );
    }
    let parakeet = registry
        .get("parakeet-tdt-0.6b-v3-Q8_0")
        .expect("parakeet placeholder");
    assert!(
        parakeet.unqualified,
        "parakeet placeholder stays marked unqualified"
    );
}

/// Offline `file://` acquisition through the real FileTransport.
#[test]
fn file_transport_offline_acquire_roundtrip() {
    let src_dir = tempfile::tempdir().expect("src tempdir");
    let payload = gguf_like(8192);
    let source = src_dir.path().join("model.gguf");
    std::fs::write(&source, &payload).expect("write source");

    let cache = tempfile::tempdir().expect("cache tempdir");
    let store = ModelStore::new(
        cache.path().to_path_buf(),
        Arc::new(FileTransport),
        test_policy(),
    );
    let entry = file_entry("offline-model", "whisper", &payload, &source);

    let acquired = store.acquire(&entry).expect("offline acquire");
    let path = acquired.path().to_path_buf();
    assert_eq!(std::fs::read(&path).expect("read"), payload);
    assert_eq!(
        store.state(&entry),
        InstallState::PresentUnverified { path }
    );

    // Tamper the source; acquire into a fresh cache must fail cleanly.
    let mut tampered = payload.clone();
    let last = tampered.len() - 1;
    tampered[last] ^= 0xFF;
    std::fs::write(&source, &tampered).expect("tamper source");
    let cache2 = tempfile::tempdir().expect("cache2");
    let store2 = ModelStore::new(
        cache2.path().to_path_buf(),
        Arc::new(FileTransport),
        test_policy(),
    );
    let entry2 = file_entry("offline-model", "whisper", &payload, &source);
    match store2.acquire(&entry2) {
        Err(ModelManagerError::CorruptDownload { part_path, .. }) => {
            assert!(!part_path.exists(), "corrupt part deleted");
        }
        other => panic!("expected CorruptDownload, got {other:?}"),
    }
}

#[test]
fn bundled_install_observation_distinguishes_absent_from_incomplete() {
    let cache = tempfile::tempdir().expect("cache tempdir");
    let store = ModelStore::new(
        cache.path().to_path_buf(),
        Arc::new(NoNetworkTransport),
        test_policy(),
    );
    let registry = BundledModelRegistry::load();
    let id = "whisper-tiny.en-Q8_0";

    assert!(matches!(
        store.verify_bundled_installed(&registry, id),
        Err(ModelManagerError::InstalledModelMissing { id: missing }) if missing == id
    ));

    let entry = registry
        .entries()
        .iter()
        .find(|entry| entry.id == id)
        .expect("bundled entry");
    std::fs::create_dir_all(store.install_dir(entry).expect("install dir"))
        .expect("create incomplete install dir");
    assert!(matches!(
        store.verify_bundled_installed(&registry, id),
        Err(ModelManagerError::UnsafeInstalledModel { id: invalid }) if invalid == id
    ));

    assert!(matches!(
        store.verify_bundled_installed(&registry, "not-a-bundled-model"),
        Err(ModelManagerError::UnknownBundledModel)
    ));
}

/// Every obligation the store imposed, in the order it imposed them.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Obligation {
    Restrict { path: PathBuf, kind: CacheEntryKind },
    CreatePrivate { path: PathBuf },
}

/// A scriptable policy that records obligations and can refuse any of them.
struct ScriptedPolicy {
    inner: RecordingPolicy,
    asked: Mutex<Vec<Obligation>>,
}

impl ScriptedPolicy {
    fn new(policy: RecordingPolicy) -> Self {
        Self {
            inner: policy,
            asked: Mutex::new(Vec::new()),
        }
    }

    fn asked(&self) -> Vec<Obligation> {
        self.asked
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    /// The paths the store required to be owner-only, in order.
    fn restricted(&self) -> Vec<(PathBuf, CacheEntryKind)> {
        self.asked()
            .into_iter()
            .filter_map(|obligation| match obligation {
                Obligation::Restrict { path, kind } => Some((path, kind)),
                _ => None,
            })
            .collect()
    }

    /// The files the store required to be created owner-only, in order.
    fn created_private(&self) -> Vec<PathBuf> {
        self.asked()
            .into_iter()
            .filter_map(|obligation| match obligation {
                Obligation::CreatePrivate { path } => Some(path),
                _ => None,
            })
            .collect()
    }
}

impl CacheFilePolicy for ScriptedPolicy {
    fn restrict_private(
        &self,
        path: &Path,
        kind: CacheEntryKind,
    ) -> narrivy_model_manager::Result<()> {
        self.asked
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(Obligation::Restrict {
                path: path.to_path_buf(),
                kind,
            });
        self.inner.restrict_private(path, kind)
    }

    fn create_private_file(&self, path: &Path) -> narrivy_model_manager::Result<std::fs::File> {
        self.asked
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(Obligation::CreatePrivate {
                path: path.to_path_buf(),
            });
        self.inner.create_private_file(path)
    }

    fn owner_of(&self, metadata: &std::fs::Metadata) -> narrivy_model_manager::Result<CacheOwner> {
        self.inner.owner_of(metadata)
    }

    fn same_object(&self, path: &std::fs::Metadata, opened: &std::fs::Metadata) -> bool {
        self.inner.same_object(path, opened)
    }
}

fn scripted_store(
    root: &Path,
    transport: Arc<FakeTransport>,
    policy: RecordingPolicy,
) -> (ModelStore, Arc<ScriptedPolicy>) {
    // The script records the obligations and delegates the actual answer, so
    // a refusal set here is a refusal the store really has to handle.
    let scripted = Arc::new(ScriptedPolicy::new(policy));
    let recording: Arc<dyn CacheFilePolicy> = scripted.clone();
    let store = ModelStore::new(root.to_path_buf(), transport, recording);
    (store, scripted)
}

#[test]
fn acquisition_demands_owner_only_for_the_cache_root_every_component_and_the_sidecar() {
    let root = tempfile::tempdir().expect("cache");
    let transport = Arc::new(FakeTransport::default());
    let payload = gguf_like(1024);
    let entry = fake_entry(&hash_str(&payload), payload.len());
    transport.serve(URL, &payload);
    let (store, policy) = scripted_store(root.path(), transport, RecordingPolicy::default());
    let binding = store.acquire(&entry).expect("verified acquire");

    let restricted = policy.restricted();
    let directories: Vec<PathBuf> = restricted
        .iter()
        .filter(|(_, kind)| *kind == CacheEntryKind::Directory)
        .map(|(path, _)| path.clone())
        .collect();
    let install_dir = binding.path().parent().expect("install dir").to_path_buf();
    for expected in [
        root.path().to_path_buf(),
        root.path().join("staging"),
        root.path().join("models"),
        root.path().join("models/whisper"),
        root.path().join("models/whisper/whisper-test-Q8_0"),
        install_dir.clone(),
    ] {
        assert!(
            directories.contains(&expected),
            "{} must be required to be owner-only, got {directories:?}",
            expected.display()
        );
    }

    let files: Vec<PathBuf> = restricted
        .iter()
        .filter(|(_, kind)| *kind == CacheEntryKind::File)
        .map(|(path, _)| path.clone())
        .collect();
    assert_eq!(
        files.len(),
        1,
        "only the staged download is a restricted file"
    );
    assert!(files[0].starts_with(root.path().join("staging")));

    assert_eq!(
        policy.created_private(),
        vec![install_dir.join("meta.json.part")],
        "the sidecar is created owner-only, not tightened after the fact"
    );
}

#[test]
fn a_platform_that_cannot_protect_the_staged_download_refuses_the_install() {
    let root = tempfile::tempdir().expect("cache");
    let transport = Arc::new(FakeTransport::default());
    let payload = gguf_like(1024);
    let entry = fake_entry(&hash_str(&payload), payload.len());
    transport.serve(URL, &payload);
    let (store, _policy) = scripted_store(
        root.path(),
        transport,
        RecordingPolicy::default()
            .refusing_to_restrict_files("this platform cannot restrict a file"),
    );
    assert!(matches!(
        store.acquire(&entry),
        Err(ModelManagerError::CacheProtectionUnavailable { kind, .. }) if kind == "file"
    ));
    assert_eq!(
        store.state(&entry),
        InstallState::ColdMissing,
        "a refused install must leave nothing behind for a later read to trust"
    );
    assert!(staging_entries(root.path()).is_empty());
}

#[test]
fn a_platform_that_cannot_create_a_private_sidecar_refuses_the_install() {
    let root = tempfile::tempdir().expect("cache");
    let transport = Arc::new(FakeTransport::default());
    let payload = gguf_like(1024);
    let entry = fake_entry(&hash_str(&payload), payload.len());
    transport.serve(URL, &payload);
    let (store, _policy) = scripted_store(
        root.path(),
        transport,
        RecordingPolicy::default().refusing_to_create("this platform has no private create"),
    );
    assert!(matches!(
        store.acquire(&entry),
        Err(ModelManagerError::CacheProtectionUnavailable { kind, .. }) if kind == "file"
    ));
}

#[test]
fn a_cache_directory_owned_by_another_user_is_refused_before_any_download() {
    let root = tempfile::tempdir().expect("cache");
    let transport = Arc::new(FakeTransport::default());
    let payload = gguf_like(1024);
    let entry = fake_entry(&hash_str(&payload), payload.len());
    transport.serve(URL, &payload);
    let (store, _policy) = scripted_store(
        root.path(),
        transport,
        RecordingPolicy::default().with_foreign_descendant(),
    );
    assert!(matches!(
        store.acquire(&entry),
        Err(ModelManagerError::UnsafeCacheRoot)
    ));
    assert!(
        staging_entries(root.path()).is_empty(),
        "a cache root this user does not own exclusively must be refused first"
    );
}

#[test]
fn a_platform_that_cannot_prove_object_identity_refuses_the_installed_bytes() {
    let root = tempfile::tempdir().expect("cache");
    let transport = Arc::new(FakeTransport::default());
    let payload = gguf_like(1024);
    let entry = fake_entry(&hash_str(&payload), payload.len());
    transport.serve(URL, &payload);
    let (store, _policy) = scripted_store(root.path(), transport, RecordingPolicy::default());
    store.acquire(&entry).expect("verified acquire");

    let substituted = Arc::new(ScriptedPolicy::new(
        RecordingPolicy::default().with_substituted_objects(),
    ));
    let strict: Arc<dyn CacheFilePolicy> = substituted;
    let recheck = ModelStore::new(
        root.path().to_path_buf(),
        Arc::new(NoNetworkTransport),
        strict,
    );
    assert!(
        matches!(
            recheck.verify_installed(&entry),
            Err(ModelManagerError::UnsafeInstalledModel { .. })
        ),
        "a stat-then-open race the platform cannot close must be refused, not assumed away"
    );
}

#[cfg(unix)]
#[test]
fn an_acquired_cache_is_owner_only_on_disk() {
    use std::os::unix::fs::MetadataExt;
    let (_root, transport, store) = store_with_fake();
    let payload = gguf_like(1024);
    let entry = fake_entry(&hash_str(&payload), payload.len());
    transport.serve(URL, &payload);
    let binding = store.acquire(&entry).expect("verified acquire");
    let install_dir = binding.path().parent().expect("install dir");

    for directory in [
        store.cache_root().to_path_buf(),
        store.staging_dir(),
        store.models_dir(),
        store.models_dir().join("whisper"),
        install_dir.to_path_buf(),
    ] {
        let mode = std::fs::metadata(&directory)
            .unwrap_or_else(|error| panic!("stat {}: {error}", directory.display()))
            .mode()
            & 0o777;
        assert_eq!(
            mode,
            0o700,
            "{} must be owner-only, got {mode:o}",
            directory.display()
        );
    }
    for file in [binding.path().to_path_buf(), install_dir.join("meta.json")] {
        let mode = std::fs::metadata(&file)
            .unwrap_or_else(|error| panic!("stat {}: {error}", file.display()))
            .mode()
            & 0o777;
        assert_eq!(
            mode,
            0o600,
            "{} must be owner-only, got {mode:o}",
            file.display()
        );
    }
}

#[test]
fn a_platform_that_cannot_protect_anything_refuses_before_any_download() {
    let root = tempfile::tempdir().expect("cache");
    let transport = Arc::new(FakeTransport::default());
    let payload = gguf_like(1024);
    let entry = fake_entry(&hash_str(&payload), payload.len());
    transport.serve(URL, &payload);
    let (store, _policy) = scripted_store(
        root.path(),
        transport,
        RecordingPolicy::default().refusing_to_restrict("this platform protects nothing"),
    );
    assert!(
        matches!(
            store.acquire(&entry),
            Err(ModelManagerError::CacheProtectionUnavailable { kind, .. })
                if kind == "directory"
        ),
        "the first obligation is the staging directory, so the refusal comes first"
    );
    assert!(staging_entries(root.path()).is_empty());
    assert!(
        !store.models_dir().exists(),
        "nothing is created below a cache this platform cannot protect"
    );
}

#[test]
fn a_refused_staged_download_is_destroyed_rather_than_left_at_the_umask() {
    let root = tempfile::tempdir().expect("cache");
    let transport = Arc::new(FakeTransport::default());
    let payload = gguf_like(1024);
    let entry = fake_entry(&hash_str(&payload), payload.len());
    transport.serve(URL, &payload);
    let (store, _policy) = scripted_store(
        root.path(),
        transport,
        RecordingPolicy::default()
            .refusing_to_restrict_files("this platform cannot restrict a file"),
    );
    assert!(matches!(
        store.acquire(&entry),
        Err(ModelManagerError::CacheProtectionUnavailable { kind, .. }) if kind == "file"
    ));
    assert!(
        staging_entries(root.path()).is_empty(),
        "downloaded bytes that cannot be made private must not survive the refusal"
    );
}
