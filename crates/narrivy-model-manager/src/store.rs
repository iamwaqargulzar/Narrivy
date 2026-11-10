//! Verified, staged model storage with explicitly unverified publication
//! state (SEC-002).
//!
//! Layout under the cache root:
//!
//! ```text
//! <cache_root>/staging/<id>-<rand>.part      # download lands here first
//! <cache_root>/models/<family>/<id>/<sha256-prefix>/model.gguf
//! ```

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use std::{fs::File, io::Read};

use narrivy_types::SessionCancellation;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{ModelManagerError, Result};
use crate::policy::{CacheEntryKind, CacheFilePolicy};
use crate::registry::{BundledModelIdentity, BundledModelRegistry, RegistryEntry};
use crate::transport::Transport;

/// Default bounded timeout for one transport fetch.
pub const DEFAULT_FETCH_TIMEOUT: Duration = Duration::from_secs(300);

/// Length of the sha256 prefix used in install paths.
const SHA_PREFIX_LEN: usize = 12;

/// Installed metadata is a small fixed-shape JSON record. Bound reads so a
/// same-user cache modification cannot turn verification into an unbounded
/// allocation.
const MAX_INSTALLED_METADATA_BYTES: u64 = 16 * 1024;

/// Install state derived from on-disk truth (restart-reconcilable: no
/// separate "installed" flag to drift out of sync).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallState {
    /// Nothing usable on disk (never acquired, or bytes deleted underneath).
    ColdMissing,
    /// A file exists at the derived path, but no integrity or metadata check
    /// has been performed for this observation.
    PresentUnverified { path: PathBuf },
}

/// Sidecar metadata written next to each verified artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InstalledMeta {
    schema_version: u32,
    id: String,
    family: String,
    source_revision: String,
    sha256: String,
    size_bytes: u64,
    acquired_at_unix: u64,
}

/// A portable, registry-derived observation of exact installed model bytes.
///
/// This does not close the final open/use race and is not package or release
/// provenance. Linux composition must still pass the path, digest and size to
/// the immutable descriptor binder before the model can become runnable.
#[derive(Clone, PartialEq, Eq)]
pub struct InstalledModelBinding {
    cache_root: PathBuf,
    path: PathBuf,
    id: String,
    family: String,
    source_revision: String,
    sha256: String,
    size_bytes: u64,
}

impl std::fmt::Debug for InstalledModelBinding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("InstalledModelBinding")
            .field("id", &self.id)
            .field("family", &self.family)
            .field("source_revision", &self.source_revision)
            .field("size_bytes", &self.size_bytes)
            .field("binding", &"registry-digest-observed")
            .finish_non_exhaustive()
    }
}

impl InstalledModelBinding {
    pub fn cache_root(&self) -> &Path {
        &self.cache_root
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn family(&self) -> &str {
        &self.family
    }

    pub fn source_revision(&self) -> &str {
        &self.source_revision
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    pub fn size_bytes(&self) -> u64 {
        self.size_bytes
    }
}

/// An installed observation whose identity came from this build's bundled
/// registry rather than caller-parsed registry data.
///
/// This remains a path-based observation, not an immutable descriptor,
/// package-membership proof, or release provenance. Linux runtime composition
/// must still bind the bytes independently.
#[derive(Clone)]
pub struct BundledInstalledModelBinding {
    observation: InstalledModelBinding,
}

impl std::fmt::Debug for BundledInstalledModelBinding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BundledInstalledModelBinding")
            .field("id", &self.observation.id)
            .field("family", &self.observation.family)
            .field("size_bytes", &self.observation.size_bytes)
            .field("binding", &"bundled-registry-observed")
            .finish_non_exhaustive()
    }
}

impl BundledInstalledModelBinding {
    pub fn cache_root(&self) -> &Path {
        self.observation.cache_root()
    }

    pub fn path(&self) -> &Path {
        self.observation.path()
    }

    pub fn id(&self) -> &str {
        self.observation.id()
    }

    pub fn family(&self) -> &str {
        self.observation.family()
    }

    pub fn source_revision(&self) -> &str {
        self.observation.source_revision()
    }

    pub fn sha256(&self) -> &str {
        self.observation.sha256()
    }

    pub fn size_bytes(&self) -> u64 {
        self.observation.size_bytes()
    }
}

impl BundledModelIdentity {
    /// Serialize the canonical installed sidecar for this bundled identity.
    pub fn installed_metadata_bytes(&self, acquired_at_unix: u64) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec_pretty(&InstalledMeta {
            schema_version: 1,
            id: self.id().to_owned(),
            family: self.family().to_owned(),
            source_revision: self.source_revision().to_owned(),
            sha256: self.sha256().to_owned(),
            size_bytes: self.size_bytes(),
            acquired_at_unix,
        })?)
    }

    /// Verify a bounded installed sidecar against this bundled identity.
    pub fn verify_installed_metadata(&self, bytes: &[u8]) -> Result<()> {
        if bytes.len() as u64 > MAX_INSTALLED_METADATA_BYTES {
            return Err(ModelManagerError::InstalledMetadataMismatch {
                id: self.id().to_owned(),
            });
        }
        let installed: InstalledMeta = serde_json::from_slice(bytes).map_err(|_| {
            ModelManagerError::InstalledMetadataMismatch {
                id: self.id().to_owned(),
            }
        })?;
        let expected = InstalledMeta {
            schema_version: 1,
            id: self.id().to_owned(),
            family: self.family().to_owned(),
            source_revision: self.source_revision().to_owned(),
            sha256: self.sha256().to_owned(),
            size_bytes: self.size_bytes(),
            acquired_at_unix: installed.acquired_at_unix,
        };
        if installed != expected {
            return Err(ModelManagerError::InstalledMetadataMismatch {
                id: self.id().to_owned(),
            });
        }
        Ok(())
    }

    /// Verify exact model bytes from an already-opened platform-owned reader.
    pub fn verify_model_reader(&self, reader: &mut File) -> Result<()> {
        let Some(actual) = sha256_reader_exact(reader, self.size_bytes())? else {
            return Err(ModelManagerError::InstalledSizeMismatch {
                id: self.id().to_owned(),
            });
        };
        if !actual.eq_ignore_ascii_case(self.sha256()) {
            return Err(ModelManagerError::OfflineImportDigestMismatch {
                id: self.id().to_owned(),
            });
        }
        Ok(())
    }
}

/// Facade over one model cache root.
pub struct ModelStore {
    cache_root: PathBuf,
    transport: Arc<dyn Transport>,
    timeout: Duration,
    policy: Arc<dyn CacheFilePolicy>,
}

impl ModelStore {
    /// Compose a store over `cache_root`.
    ///
    /// `policy` is mandatory and has no default: a model cache is private
    /// data, so "how does this host make a file owner-only" is a decision
    /// composition must make explicitly rather than inherit from a library
    /// that cannot know the answer.
    pub fn new(
        cache_root: impl Into<PathBuf>,
        transport: Arc<dyn Transport>,
        policy: Arc<dyn CacheFilePolicy>,
    ) -> Self {
        Self {
            cache_root: cache_root.into(),
            transport,
            timeout: DEFAULT_FETCH_TIMEOUT,
            policy,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn cache_root(&self) -> &Path {
        &self.cache_root
    }

    pub fn models_dir(&self) -> PathBuf {
        self.cache_root.join("models")
    }

    pub fn staging_dir(&self) -> PathBuf {
        self.cache_root.join("staging")
    }

    /// `models/<family>/<id>/<sha256-prefix>/` for a pinned entry.
    pub fn install_dir(&self, entry: &RegistryEntry) -> Result<PathBuf> {
        validate_cache_root(&self.cache_root)?;
        entry.validate()?;
        let sha = entry
            .sha256
            .as_deref()
            .ok_or_else(|| ModelManagerError::ChecksumRequired(entry.id.clone()))?;
        let normalized_sha = sha.to_ascii_lowercase();
        Ok(self
            .models_dir()
            .join(&entry.family)
            .join(&entry.id)
            .join(prefix(&normalized_sha)))
    }

    /// Final artifact path inside [`ModelStore::install_dir`].
    pub fn model_path(&self, entry: &RegistryEntry) -> Result<PathBuf> {
        Ok(self.install_dir(entry)?.join("model.gguf"))
    }

    /// Current presence state. This deliberately makes no integrity claim.
    pub fn state(&self, entry: &RegistryEntry) -> InstallState {
        match entry.sha256.as_deref() {
            Some(_) => {
                // state() must stay total even for entries missing a pin.
                match self.model_path(entry) {
                    Ok(path) if path.is_file() => InstallState::PresentUnverified { path },
                    _ => InstallState::ColdMissing,
                }
            }
            None => InstallState::ColdMissing,
        }
    }

    /// Resolve an installed model only after exact registry metadata, size and
    /// digest verification. The returned binding is an observation, not a
    /// retained descriptor and not package/release provenance.
    pub fn verify_installed(&self, entry: &RegistryEntry) -> Result<InstalledModelBinding> {
        self.verify_installed_with_control(entry, None)
    }

    /// Verify an installed observation with one cooperative cancellation and
    /// absolute-deadline budget. Filesystem syscalls themselves are not
    /// preemptible; the bounded model hash checks control between chunks.
    pub fn verify_installed_controlled(
        &self,
        entry: &RegistryEntry,
        cancellation: &SessionCancellation,
        deadline: Instant,
    ) -> Result<InstalledModelBinding> {
        self.verify_installed_with_control(entry, Some((cancellation, deadline)))
    }

    fn verify_installed_with_control(
        &self,
        entry: &RegistryEntry,
        control: Option<(&SessionCancellation, Instant)>,
    ) -> Result<InstalledModelBinding> {
        check_verification_control(control)?;
        let expected_sha256 = entry
            .sha256
            .as_deref()
            .ok_or_else(|| ModelManagerError::ChecksumRequired(entry.id.clone()))?;
        let path = self.model_path(entry)?;
        check_verification_control(control)?;
        let metadata_path = self.install_dir(entry)?.join("meta.json");
        check_verification_control(control)?;
        validate_cache_descendant(&self.cache_root, &path).map_err(|_| {
            ModelManagerError::UnsafeInstalledModel {
                id: entry.id.clone(),
            }
        })?;
        check_verification_control(control)?;
        validate_cache_descendant(&self.cache_root, &metadata_path).map_err(|_| {
            ModelManagerError::UnsafeInstalledModel {
                id: entry.id.clone(),
            }
        })?;
        check_verification_control(control)?;
        let path_metadata = std::fs::symlink_metadata(&path).map_err(|_| {
            ModelManagerError::UnsafeInstalledModel {
                id: entry.id.clone(),
            }
        })?;
        check_verification_control(control)?;
        let sidecar_path_metadata = std::fs::symlink_metadata(&metadata_path).map_err(|_| {
            ModelManagerError::UnsafeInstalledModel {
                id: entry.id.clone(),
            }
        })?;
        check_verification_control(control)?;
        if !path_metadata.file_type().is_file() || !sidecar_path_metadata.file_type().is_file() {
            return Err(ModelManagerError::UnsafeInstalledModel {
                id: entry.id.clone(),
            });
        }
        let mut model = File::open(&path)?;
        check_verification_control(control)?;
        let mut sidecar = File::open(&metadata_path)?;
        check_verification_control(control)?;
        let file_metadata = model.metadata()?;
        check_verification_control(control)?;
        let sidecar_metadata = sidecar.metadata()?;
        check_verification_control(control)?;
        if !file_metadata.is_file()
            || !sidecar_metadata.is_file()
            || !self.policy.same_object(&path_metadata, &file_metadata)
            || !self
                .policy
                .same_object(&sidecar_path_metadata, &sidecar_metadata)
        {
            return Err(ModelManagerError::UnsafeInstalledModel {
                id: entry.id.clone(),
            });
        }
        if sidecar_metadata.len() > MAX_INSTALLED_METADATA_BYTES {
            return Err(ModelManagerError::UnsafeInstalledModel {
                id: entry.id.clone(),
            });
        }
        if file_metadata.len() != entry.size_bytes {
            return Err(ModelManagerError::InstalledSizeMismatch {
                id: entry.id.clone(),
            });
        }
        let mut sidecar_bytes = vec![0_u8; sidecar_metadata.len() as usize];
        check_verification_control(control)?;
        sidecar.read_exact(&mut sidecar_bytes).map_err(|_| {
            ModelManagerError::UnsafeInstalledModel {
                id: entry.id.clone(),
            }
        })?;
        check_verification_control(control)?;
        let mut sidecar_extra = [0_u8; 1];
        let has_extra = sidecar.read(&mut sidecar_extra)? != 0;
        check_verification_control(control)?;
        let final_sidecar_metadata = sidecar.metadata()?;
        check_verification_control(control)?;
        if has_extra
            || !final_sidecar_metadata.is_file()
            || final_sidecar_metadata.len() != sidecar_metadata.len()
            || final_sidecar_metadata.modified().ok() != sidecar_metadata.modified().ok()
        {
            return Err(ModelManagerError::UnsafeInstalledModel {
                id: entry.id.clone(),
            });
        }
        let installed: InstalledMeta = serde_json::from_slice(&sidecar_bytes)?;
        check_verification_control(control)?;
        let expected_metadata = InstalledMeta {
            schema_version: 1,
            id: entry.id.clone(),
            family: entry.family.clone(),
            source_revision: entry.source_revision.clone(),
            sha256: expected_sha256.to_ascii_lowercase(),
            size_bytes: entry.size_bytes,
            acquired_at_unix: installed.acquired_at_unix,
        };
        if installed != expected_metadata {
            return Err(ModelManagerError::InstalledMetadataMismatch {
                id: entry.id.clone(),
            });
        }
        let Some(actual) = sha256_reader_exact_controlled(&mut model, entry.size_bytes, control)?
        else {
            return Err(ModelManagerError::InstalledSizeMismatch {
                id: entry.id.clone(),
            });
        };
        if !actual.eq_ignore_ascii_case(expected_sha256) {
            return Err(ModelManagerError::InstalledDigestMismatch {
                id: entry.id.clone(),
            });
        }
        check_verification_control(control)?;
        let final_metadata = model.metadata()?;
        check_verification_control(control)?;
        if !final_metadata.is_file()
            || final_metadata.len() != file_metadata.len()
            || final_metadata.modified().ok() != file_metadata.modified().ok()
        {
            return Err(ModelManagerError::UnsafeInstalledModel {
                id: entry.id.clone(),
            });
        }
        Ok(InstalledModelBinding {
            cache_root: self.cache_root.clone(),
            path,
            id: entry.id.clone(),
            family: entry.family.clone(),
            source_revision: entry.source_revision.clone(),
            sha256: expected_sha256.to_ascii_lowercase(),
            size_bytes: entry.size_bytes,
        })
    }

    /// Verify an installed path using only this build's bundled model
    /// authority, yielding a binding that cannot originate from caller JSON.
    pub fn verify_bundled_installed(
        &self,
        registry: &BundledModelRegistry,
        id: &str,
    ) -> Result<BundledInstalledModelBinding> {
        let identity = registry.resolve_import(id)?;
        let install_dir = self.install_dir(identity.entry())?;
        match std::fs::symlink_metadata(&install_dir) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(ModelManagerError::InstalledModelMissing {
                    id: identity.id().to_owned(),
                });
            }
            Err(error) => return Err(error.into()),
        }
        let observation = self.verify_installed(identity.entry())?;
        Ok(BundledInstalledModelBinding { observation })
    }

    /// Verify a bundled installed observation with one cooperative caller-owned
    /// cancellation/deadline budget. Blocking filesystem syscalls themselves
    /// cannot be preempted; bounded model reads check the budget per chunk.
    pub fn verify_bundled_installed_controlled(
        &self,
        registry: &BundledModelRegistry,
        id: &str,
        cancellation: &SessionCancellation,
        deadline: Instant,
    ) -> Result<BundledInstalledModelBinding> {
        check_verification_control(Some((cancellation, deadline)))?;
        let identity = registry.resolve_import(id)?;
        check_verification_control(Some((cancellation, deadline)))?;
        let install_dir = self.install_dir(identity.entry())?;
        check_verification_control(Some((cancellation, deadline)))?;
        match std::fs::symlink_metadata(&install_dir) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(ModelManagerError::InstalledModelMissing {
                    id: identity.id().to_owned(),
                });
            }
            Err(error) => return Err(error.into()),
        }
        check_verification_control(Some((cancellation, deadline)))?;
        let observation =
            self.verify_installed_with_control(identity.entry(), Some((cancellation, deadline)))?;
        Ok(BundledInstalledModelBinding { observation })
    }

    /// Acquire a model into a present-unverified layout, then return only after
    /// a fresh exact observation succeeds. Completed-directory publication is
    /// not yet atomic. Resume is not supported; timeouts are bounded.
    ///
    /// On checksum mismatch the staged part is deleted and a typed
    /// [`ModelManagerError::CorruptDownload`] is returned; nothing is ever
    /// activated from unverified bytes.
    pub fn acquire(&self, entry: &RegistryEntry) -> Result<InstalledModelBinding> {
        validate_cache_root(&self.cache_root)?;
        entry.validate()?;
        let expected = entry
            .sha256
            .clone()
            .ok_or_else(|| ModelManagerError::ChecksumRequired(entry.id.clone()))?;
        let url = entry.resolve_url()?;

        create_validated_dir_all(&self.cache_root, &self.staging_dir(), self.policy.as_ref())?;
        let part_path = self.staging_part_path(&entry.id);
        validate_cache_descendant(&self.cache_root, &part_path)?;
        if std::fs::symlink_metadata(&part_path).is_ok() {
            return Err(ModelManagerError::UnsafeCacheRoot);
        }

        if let Err(e) = self.transport.fetch(&url, &part_path, self.timeout) {
            // Best-effort cleanup; the transport error itself is authoritative.
            drop(std::fs::remove_file(&part_path));
            return Err(e);
        }

        let staged_metadata = std::fs::symlink_metadata(&part_path)?;
        if !staged_metadata.file_type().is_file() {
            drop(std::fs::remove_file(&part_path));
            return Err(ModelManagerError::UnsafeCacheRoot);
        }
        // Bytes that cannot be made owner-only are bytes that must not be left
        // on disk, so a refusal destroys the staged download rather than
        // leaving it at whatever the transport's umask produced.
        if let Err(error) = self
            .policy
            .restrict_private(&part_path, CacheEntryKind::File)
        {
            drop(std::fs::remove_file(&part_path));
            return Err(error);
        }

        let actual_size = std::fs::metadata(&part_path)?.len();
        if actual_size != entry.size_bytes {
            drop(std::fs::remove_file(&part_path));
            return Err(ModelManagerError::InstalledSizeMismatch {
                id: entry.id.clone(),
            });
        }
        let actual = sha256_file_exact(&part_path, entry.size_bytes)?.ok_or_else(|| {
            ModelManagerError::InstalledSizeMismatch {
                id: entry.id.clone(),
            }
        })?;
        if !actual.eq_ignore_ascii_case(&expected) {
            drop(std::fs::remove_file(&part_path));
            return Err(ModelManagerError::CorruptDownload {
                id: entry.id.clone(),
                expected,
                actual,
                part_path,
            });
        }

        let final_dir = self.install_dir(entry)?;
        create_validated_dir_all(&self.cache_root, &final_dir, self.policy.as_ref())?;
        let final_path = final_dir.join("model.gguf");
        let metadata_staging = final_dir.join("meta.json.part");
        match std::fs::symlink_metadata(&metadata_staging) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(ModelManagerError::UnsafeCacheRoot);
            }
            Ok(_) => std::fs::remove_file(&metadata_staging)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        // Atomic within the same filesystem: staging and models share the root.
        std::fs::rename(&part_path, &final_path)?;

        let meta = InstalledMeta {
            schema_version: 1,
            id: entry.id.clone(),
            family: entry.family.clone(),
            source_revision: entry.source_revision.clone(),
            sha256: expected.to_ascii_lowercase(),
            size_bytes: entry.size_bytes,
            acquired_at_unix: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        };
        let mut metadata_file = self.policy.create_private_file(&metadata_staging)?;
        std::io::Write::write_all(&mut metadata_file, &serde_json::to_vec_pretty(&meta)?)?;
        drop(metadata_file);
        std::fs::rename(metadata_staging, final_dir.join("meta.json"))?;

        self.verify_installed(entry)
    }

    /// Delete every leftover in staging; returns the number removed.
    pub fn clean_staging(&self) -> Result<usize> {
        validate_cache_root(&self.cache_root)?;
        validate_cache_tree(&self.cache_root, &self.staging_dir())?;
        remove_children(self.staging_dir())
    }

    fn staging_part_path(&self, id: &str) -> PathBuf {
        self.staging_dir()
            .join(format!("{id}-{:.0x}.part", part_suffix()))
    }
}

/// sha256 of a file, streamed in bounded chunks.
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let expected = file.metadata()?.len();
    sha256_reader_exact(&mut file, expected)?.ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "file size changed while hashing",
        )
        .into()
    })
}

fn sha256_file_exact(path: &Path, expected: u64) -> Result<Option<String>> {
    let mut file = File::open(path)?;
    sha256_reader_exact(&mut file, expected).map_err(Into::into)
}

fn sha256_reader_exact(file: &mut File, expected: u64) -> std::io::Result<Option<String>> {
    sha256_reader_exact_controlled(file, expected, None).map_err(|error| match error {
        ModelManagerError::Io(error) => error,
        _ => std::io::Error::other("unexpected controlled verification result"),
    })
}

fn sha256_reader_exact_controlled(
    file: &mut File,
    expected: u64,
    control: Option<(&SessionCancellation, Instant)>,
) -> Result<Option<String>> {
    sha256_reader_exact_controlled_with_hook(file, expected, control, &mut || {})
}

fn sha256_reader_exact_controlled_with_hook(
    file: &mut File,
    expected: u64,
    control: Option<(&SessionCancellation, Instant)>,
    on_chunk: &mut impl FnMut(),
) -> Result<Option<String>> {
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    let mut remaining = expected;
    while remaining > 0 {
        check_verification_control(control)?;
        let limit = usize::try_from(remaining.min(buf.len() as u64)).unwrap_or(buf.len());
        let n = file.read(&mut buf[..limit])?;
        check_verification_control(control)?;
        if n == 0 {
            return Ok(None);
        }
        hasher.update(&buf[..n]);
        remaining -= n as u64;
        on_chunk();
        check_verification_control(control)?;
    }
    check_verification_control(control)?;
    let mut extra = [0_u8; 1];
    if file.read(&mut extra)? != 0 {
        return Ok(None);
    }
    check_verification_control(control)?;
    Ok(Some(hex::encode(hasher.finalize())))
}

fn check_verification_control(control: Option<(&SessionCancellation, Instant)>) -> Result<()> {
    let Some((cancellation, deadline)) = control else {
        return Ok(());
    };
    if cancellation.is_cancelled() {
        Err(ModelManagerError::InstalledVerificationCancelled)
    } else if Instant::now() >= deadline {
        Err(ModelManagerError::InstalledVerificationTimedOut)
    } else {
        Ok(())
    }
}

fn prefix(sha: &str) -> &str {
    let end = usize::min(SHA_PREFIX_LEN, sha.len());
    &sha[..end]
}

fn part_suffix() -> u128 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    nanos ^ u128::from(COUNTER.fetch_add(1, Ordering::Relaxed)) << 32
}

fn remove_children(dir: PathBuf) -> Result<usize> {
    let mut removed = 0;
    let Ok(readings) = std::fs::read_dir(&dir) else {
        return Ok(0);
    };
    for child in readings {
        let child = child?.path();
        let meta = std::fs::symlink_metadata(&child)?;
        if meta.file_type().is_symlink() {
            return Err(ModelManagerError::UnsafeCacheRoot);
        }
        if meta.is_dir() {
            std::fs::remove_dir_all(&child)?;
        } else {
            std::fs::remove_file(&child)?;
        }
        removed += 1;
    }
    Ok(removed)
}

pub(crate) fn validate_cache_root(root: &Path) -> Result<()> {
    if !root.is_absolute()
        || root.components().any(|component| {
            matches!(
                component,
                std::path::Component::CurDir | std::path::Component::ParentDir
            )
        })
    {
        return Err(ModelManagerError::UnsafeCacheRoot);
    }
    for component in root.ancestors() {
        match std::fs::symlink_metadata(component) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(ModelManagerError::UnsafeCacheRoot);
            }
            Ok(metadata) if component == root && !metadata.is_dir() => {
                return Err(ModelManagerError::UnsafeCacheRoot);
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

pub(crate) fn validate_cache_descendant(root: &Path, target: &Path) -> Result<()> {
    validate_cache_root(root)?;
    let relative = target
        .strip_prefix(root)
        .map_err(|_| ModelManagerError::UnsafeCacheRoot)?;
    let mut current = root.to_path_buf();
    let component_count = relative.components().count();
    for (index, component) in relative.components().enumerate() {
        let std::path::Component::Normal(part) = component else {
            return Err(ModelManagerError::UnsafeCacheRoot);
        };
        current.push(part);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(ModelManagerError::UnsafeCacheRoot);
            }
            Ok(metadata) if index + 1 < component_count && !metadata.is_dir() => {
                return Err(ModelManagerError::UnsafeCacheRoot);
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

pub(crate) fn validate_cache_tree(root: &Path, tree: &Path) -> Result<()> {
    validate_cache_descendant(root, tree)?;
    let Ok(metadata) = std::fs::symlink_metadata(tree) else {
        return Ok(());
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ModelManagerError::UnsafeCacheRoot);
    }
    let mut pending = vec![tree.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory)? {
            let path = entry?.path();
            let metadata = std::fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                return Err(ModelManagerError::UnsafeCacheRoot);
            }
            if metadata.is_dir() {
                pending.push(path);
            } else if !metadata.is_file() {
                return Err(ModelManagerError::UnsafeCacheRoot);
            }
        }
    }
    Ok(())
}

/// Create `directory` below `root`, then prove that every component is a
/// directory this user owns and make each one owner-only.
///
/// Ownership is compared against the cache root rather than assumed, so a
/// directory another user could have prepared is refused instead of being
/// tightened on the way in.
fn create_validated_dir_all(
    root: &Path,
    directory: &Path,
    policy: &dyn CacheFilePolicy,
) -> Result<()> {
    validate_cache_descendant(root, directory)?;
    std::fs::create_dir_all(directory)?;
    let root_metadata = std::fs::symlink_metadata(root)?;
    let root_owner = policy.owner_of(&root_metadata)?;
    let relative = directory
        .strip_prefix(root)
        .map_err(|_| ModelManagerError::UnsafeCacheRoot)?;
    let mut current = root.to_path_buf();
    for component in std::iter::once(None).chain(relative.components().map(Some)) {
        if let Some(component) = component {
            let std::path::Component::Normal(part) = component else {
                return Err(ModelManagerError::UnsafeCacheRoot);
            };
            current.push(part);
        }
        let metadata = std::fs::symlink_metadata(&current)?;
        if !metadata.file_type().is_dir() || policy.owner_of(&metadata)? != root_owner {
            return Err(ModelManagerError::UnsafeCacheRoot);
        }
        policy.restrict_private(&current, CacheEntryKind::Directory)?;
    }
    validate_cache_descendant(root, directory)
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod controlled_verification_tests {
    use super::*;
    use std::io::{Seek, Write};
    use std::sync::mpsc::sync_channel;

    #[test]
    fn bundled_control_short_circuits_cancel_and_expiry_without_cache_access() {
        let cache = tempfile::tempdir().expect("cache");
        let store = ModelStore::new(
            cache.path(),
            Arc::new(crate::NoNetworkTransport),
            Arc::new(crate::policy::test_support::RecordingPolicy::default()),
        );
        let registry = BundledModelRegistry::load();
        let id = &registry.entries()[0].id;

        let cancelled = SessionCancellation::new();
        cancelled.cancel();
        assert!(matches!(
            store.verify_bundled_installed_controlled(&registry, id, &cancelled, Instant::now(),),
            Err(ModelManagerError::InstalledVerificationCancelled)
        ));

        assert!(matches!(
            store.verify_bundled_installed_controlled(
                &registry,
                id,
                &SessionCancellation::new(),
                Instant::now(),
            ),
            Err(ModelManagerError::InstalledVerificationTimedOut)
        ));
    }

    #[test]
    fn model_hash_observes_cancellation_between_bounded_chunks() {
        let mut file = tempfile::tempfile().expect("model file");
        let bytes = vec![0x35_u8; 64 * 1024 * 3];
        file.write_all(&bytes).expect("model bytes");
        file.rewind().expect("rewind");

        let cancellation = SessionCancellation::new();
        let worker_cancellation = cancellation.clone();
        let (reached_tx, reached_rx) = sync_channel(0);
        let (resume_tx, resume_rx) = sync_channel(0);
        let worker = std::thread::spawn(move || {
            let mut first = true;
            sha256_reader_exact_controlled_with_hook(
                &mut file,
                bytes.len() as u64,
                Some((
                    &worker_cancellation,
                    Instant::now() + Duration::from_secs(30),
                )),
                &mut || {
                    if first {
                        first = false;
                        reached_tx.send(()).expect("reached receiver");
                        resume_rx.recv().expect("resume sender");
                    }
                },
            )
        });
        reached_rx.recv().expect("first chunk");
        cancellation.cancel();
        resume_tx.send(()).expect("resume hash");
        assert!(matches!(
            worker.join().expect("hash thread"),
            Err(ModelManagerError::InstalledVerificationCancelled)
        ));
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod cache_protection_tests {
    use super::*;
    use crate::policy::test_support::{Obligation, RecordingPolicy};
    use crate::transport::FileTransport;

    /// The GGUF-shaped bytes every scenario in this module installs.
    const PAYLOAD: [u8; 8] = [b'G', b'G', b'U', b'F', 0x01, 0x02, 0x03, 0x04];

    /// A `file://` entry pinned to the true digest of the bytes at `source`.
    fn entry_with_source(source: &Path) -> RegistryEntry {
        let bytes = PAYLOAD;
        let json = format!(
            r#"{{
                "id": "whisper-test-Q8_0", "family": "whisper", "variant": "test",
                "quant": "Q8_0", "size_bytes": {size}, "sha256": "{sha}",
                "license": "MIT", "source_revision": "deadbeef",
                "sources": [{{"kind": "url", "repo_or_url": "file://{source}",
                             "filename": "test.gguf"}}],
                "capabilities": {{"languages": ["en"], "streaming": false}}
            }}"#,
            size = bytes.len(),
            sha = sha256_file(source).expect("hash source"),
            source = source.display(),
        );
        serde_json::from_str(&json).expect("entry parses")
    }

    /// A temp cache, a served source file, and the entry that names it. The
    /// source directory is returned so it outlives the store's reads.
    fn served() -> (tempfile::TempDir, tempfile::TempDir, RegistryEntry) {
        let source_dir = tempfile::tempdir().expect("source dir");
        let source = source_dir.path().join("test.gguf");
        std::fs::write(
            &source,
            vec![b'G', b'G', b'U', b'F', 0x01, 0x02, 0x03, 0x04],
        )
        .expect("write source");
        let entry = entry_with_source(&source);
        let cache = tempfile::tempdir().expect("cache");
        (source_dir, cache, entry)
    }

    fn store_with(cache: &tempfile::TempDir, policy: Arc<RecordingPolicy>) -> ModelStore {
        ModelStore::new(cache.path().to_path_buf(), Arc::new(FileTransport), policy)
    }

    #[test]
    fn acquisition_imposes_owner_only_on_every_object_it_creates() {
        let (_source, cache, entry) = served();
        let policy = Arc::new(RecordingPolicy::default());
        let store = store_with(&cache, policy.clone());
        let binding = store.acquire(&entry).expect("verified acquire");

        let asked = policy.asked();
        let restricted: Vec<(PathBuf, CacheEntryKind)> = asked
            .iter()
            .filter_map(|obligation| match obligation {
                Obligation::Restrict { path, kind } => Some((path.clone(), *kind)),
                _ => None,
            })
            .collect();
        let directories: Vec<PathBuf> = restricted
            .iter()
            .filter(|(_, kind)| *kind == CacheEntryKind::Directory)
            .map(|(path, _)| path.clone())
            .collect();
        assert_eq!(
            directories,
            vec![
                // The root is re-asserted for each created tree, so a
                // permission change made between the two creations is
                // repaired rather than assumed.
                cache.path().to_path_buf(),
                cache.path().join("staging"),
                cache.path().to_path_buf(),
                cache.path().join("models"),
                cache.path().join("models/whisper"),
                cache.path().join("models/whisper/whisper-test-Q8_0"),
                binding.path().parent().expect("install dir").to_path_buf(),
            ],
            "the root and every created component must be owner-only"
        );
        let files: Vec<PathBuf> = restricted
            .iter()
            .filter(|(_, kind)| *kind == CacheEntryKind::File)
            .map(|(path, _)| path.clone())
            .collect();
        assert_eq!(files.len(), 1, "exactly the staged download is restricted");
        assert!(
            files[0].starts_with(cache.path().join("staging")),
            "the staged part is the only file restricted before publication, got {}",
            files[0].display()
        );
        assert!(
            asked.iter().any(|obligation| matches!(
                obligation,
                Obligation::CreatePrivate { path }
                    if path.ends_with("meta.json.part")
            )),
            "the sidecar must be created owner-only, not tightened afterwards"
        );
    }

    #[test]
    fn a_platform_that_cannot_protect_a_file_refuses_the_whole_install() {
        let (_source, cache, entry) = served();
        let store = store_with(
            &cache,
            Arc::new(RecordingPolicy::default().refusing_to_restrict("no modes")),
        );
        assert!(matches!(
            store.acquire(&entry),
            Err(ModelManagerError::CacheProtectionUnavailable { .. })
        ));
        assert_eq!(
            store.state(&entry),
            InstallState::ColdMissing,
            "a refused install must leave nothing a later read could trust"
        );
    }

    #[test]
    fn a_platform_that_cannot_create_a_private_sidecar_refuses_the_install() {
        let (_source, cache, entry) = served();
        let store = store_with(
            &cache,
            Arc::new(RecordingPolicy::default().refusing_to_create("no private create")),
        );
        assert!(matches!(
            store.acquire(&entry),
            Err(ModelManagerError::CacheProtectionUnavailable { .. })
        ));
    }

    #[test]
    fn a_cache_directory_owned_by_another_user_is_refused() {
        let (_source, cache, entry) = served();
        let store = store_with(
            &cache,
            Arc::new(RecordingPolicy::default().with_foreign_descendant()),
        );
        assert!(
            matches!(
                store.acquire(&entry),
                Err(ModelManagerError::UnsafeCacheRoot)
            ),
            "a directory this user does not exclusively own is not a safe cache root"
        );
    }

    #[test]
    fn object_identity_comes_from_the_policy_not_from_the_store() {
        let (_source, cache, entry) = served();
        let strict = store_with(&cache, Arc::new(RecordingPolicy::default()));
        strict.acquire(&entry).expect("verified acquire");

        let substituted = store_with(
            &cache,
            Arc::new(RecordingPolicy::default().with_substituted_objects()),
        );
        assert!(
            matches!(
                substituted.verify_installed(&entry),
                Err(ModelManagerError::UnsafeInstalledModel { .. })
            ),
            "a platform that cannot prove the opened bytes are the observed path must refuse them"
        );
    }
}
