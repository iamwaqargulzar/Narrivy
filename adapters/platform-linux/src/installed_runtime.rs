//! Fail-closed observation of Narrivy's fixed installed resident runtime.
//!
//! A successful result proves only that a trusted fixed-root manifest named
//! the exact bytes retained in the returned immutable worker snapshot. It is
//! not evidence of dpkg membership, package authenticity, SBOM membership, or
//! release provenance.

use std::fs::{Metadata, OpenOptions};
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::time::Instant;

use narrivy_model_manager::BundledInstalledModelBinding;
use narrivy_process_linux::{
    ArtifactError, ArtifactProvenance, DataArtifactError, VerifiedArtifactData,
    VerifiedArtifactExecutable,
};
use narrivy_types::SessionCancellation;
use serde::Deserialize;

pub const INSTALLED_RUNTIME_MANIFEST_PATH: &str =
    "/usr/share/narrivy/installed-runtime-manifest.json";
pub const INSTALLED_RUNTIME_WORKER_PATH: &str = "/usr/lib/narrivy/narrivy-transcribe-worker";

const MANIFEST_RELATIVE_PATH: &str = "usr/share/narrivy/installed-runtime-manifest.json";
const WORKER_RELATIVE_PATH: &str = "usr/lib/narrivy/narrivy-transcribe-worker";
const MAX_MANIFEST_BYTES: u64 = 16 * 1024;
const MAX_WORKER_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstalledRuntimeError {
    ManifestMissing,
    ManifestUntrusted,
    ManifestTooLarge,
    ManifestInvalid,
    UnsupportedRuntime,
    Worker(ArtifactError),
}

impl std::fmt::Display for InstalledRuntimeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::ManifestMissing => "installed runtime manifest is missing",
            Self::ManifestUntrusted => "installed runtime manifest is untrusted",
            Self::ManifestTooLarge => "installed runtime manifest exceeds its size limit",
            Self::ManifestInvalid => "installed runtime manifest is invalid",
            Self::UnsupportedRuntime => "installed runtime identity is unsupported",
            Self::Worker(_) => "installed runtime worker binding failed",
        })
    }
}

impl std::error::Error for InstalledRuntimeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Worker(error) => Some(error),
            _ => None,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestV1 {
    schema_version: u32,
    runtime_id: String,
    worker: WorkerV1,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerV1 {
    sha256: String,
    size_bytes: u64,
}

/// Immutable worker bytes bound to a trusted fixed-root manifest observation.
#[derive(Clone)]
pub struct InstalledRuntimeBinding {
    worker: VerifiedArtifactExecutable,
}

/// C1's registry observation upgraded to one immutable Linux descriptor
/// snapshot. The selected model remains independent of the root runtime
/// manifest and is not package or release provenance.
#[derive(Clone)]
pub struct InstalledModelRuntimeBinding {
    model: VerifiedArtifactData,
    id: String,
    family: String,
    source_revision: String,
}

impl std::fmt::Debug for InstalledModelRuntimeBinding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("InstalledModelRuntimeBinding")
            .field("model_size_bytes", &self.model.size())
            .field("binding", &"registry-observed-immutable-digest-bound")
            .finish_non_exhaustive()
    }
}

impl InstalledModelRuntimeBinding {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub fn family(&self) -> &str {
        &self.family
    }

    #[must_use]
    pub fn source_revision(&self) -> &str {
        &self.source_revision
    }

    #[must_use]
    pub fn model(&self) -> &VerifiedArtifactData {
        &self.model
    }

    #[must_use]
    pub fn into_model(self) -> VerifiedArtifactData {
        self.model
    }
}

impl std::fmt::Debug for InstalledRuntimeBinding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("InstalledRuntimeBinding")
            .field("runtime", &"transcribe.cpp")
            .field("worker_size_bytes", &self.worker.size())
            .field("binding", &"installed-runtime-manifest-bound")
            .finish_non_exhaustive()
    }
}

impl InstalledRuntimeBinding {
    #[must_use]
    pub fn worker(&self) -> &VerifiedArtifactExecutable {
        &self.worker
    }

    #[must_use]
    pub fn into_worker(self) -> VerifiedArtifactExecutable {
        self.worker
    }
}

/// Resolve the only production installed-runtime location.
pub fn resolve_installed_runtime() -> Result<InstalledRuntimeBinding, InstalledRuntimeError> {
    resolve_under(Path::new("/"), 0)
}

/// Upgrade a C1 verified installed-model observation to an immutable Linux
/// descriptor binding. Raw paths cannot enter this installed composition API.
pub fn bind_installed_model(
    binding: &BundledInstalledModelBinding,
) -> Result<InstalledModelRuntimeBinding, DataArtifactError> {
    bind_observed_model(binding)
}

/// Upgrade a bundled installed-model observation to an immutable Linux
/// descriptor binding under one cooperative cancellation signal and absolute
/// deadline. No raw path or caller-selected digest enters this boundary.
pub fn bind_installed_model_controlled(
    binding: &BundledInstalledModelBinding,
    cancellation: &SessionCancellation,
    deadline: Instant,
) -> Result<InstalledModelRuntimeBinding, DataArtifactError> {
    bind_observed_model_controlled(binding, cancellation, deadline)
}

fn bind_observed_model_controlled(
    binding: &impl InstalledBindingView,
    cancellation: &SessionCancellation,
    deadline: Instant,
) -> Result<InstalledModelRuntimeBinding, DataArtifactError> {
    if !binding.cache_root().is_absolute() {
        return Err(DataArtifactError::Untrusted);
    }
    let real_owner = rustix::process::getuid();
    if real_owner != rustix::process::geteuid() {
        return Err(DataArtifactError::Untrusted);
    }
    let owner = real_owner.as_raw();
    let model = VerifiedArtifactData::bind_trust_anchored_with_control(
        binding.path(),
        binding.sha256(),
        binding.size_bytes(),
        binding.cache_root(),
        owner,
        cancellation,
        deadline,
    )?;
    if model.provenance() != ArtifactProvenance::TrustAnchorDigestBound {
        return Err(DataArtifactError::Untrusted);
    }
    Ok(InstalledModelRuntimeBinding {
        model,
        id: binding.id().to_owned(),
        family: binding.family().to_owned(),
        source_revision: binding.source_revision().to_owned(),
    })
}

trait InstalledBindingView {
    fn cache_root(&self) -> &Path;
    fn path(&self) -> &Path;
    fn sha256(&self) -> &str;
    fn size_bytes(&self) -> u64;
    fn id(&self) -> &str;
    fn family(&self) -> &str;
    fn source_revision(&self) -> &str;
}

impl InstalledBindingView for BundledInstalledModelBinding {
    fn cache_root(&self) -> &Path {
        self.cache_root()
    }
    fn path(&self) -> &Path {
        self.path()
    }
    fn sha256(&self) -> &str {
        self.sha256()
    }
    fn size_bytes(&self) -> u64 {
        self.size_bytes()
    }
    fn id(&self) -> &str {
        self.id()
    }
    fn family(&self) -> &str {
        self.family()
    }
    fn source_revision(&self) -> &str {
        self.source_revision()
    }
}

fn bind_observed_model(
    binding: &impl InstalledBindingView,
) -> Result<InstalledModelRuntimeBinding, DataArtifactError> {
    if !binding.cache_root().is_absolute() {
        return Err(DataArtifactError::Untrusted);
    }
    let real_owner = rustix::process::getuid();
    if real_owner != rustix::process::geteuid() {
        return Err(DataArtifactError::Untrusted);
    }
    let owner = real_owner.as_raw();
    let model = VerifiedArtifactData::bind_trust_anchored(
        binding.path(),
        binding.sha256(),
        binding.size_bytes(),
        binding.cache_root(),
        owner,
    )?;
    if model.provenance() != ArtifactProvenance::TrustAnchorDigestBound {
        return Err(DataArtifactError::Untrusted);
    }
    Ok(InstalledModelRuntimeBinding {
        model,
        id: binding.id().to_owned(),
        family: binding.family().to_owned(),
        source_revision: binding.source_revision().to_owned(),
    })
}

fn resolve_under(
    root: &Path,
    owner: u32,
) -> Result<InstalledRuntimeBinding, InstalledRuntimeError> {
    if !root.is_absolute() {
        return Err(InstalledRuntimeError::ManifestUntrusted);
    }
    let manifest_path = root.join(MANIFEST_RELATIVE_PATH);
    let worker_path = root.join(WORKER_RELATIVE_PATH);
    let manifest_bytes = read_trusted_manifest(&manifest_path, root, owner)?;
    let manifest: ManifestV1 = serde_json::from_slice(&manifest_bytes)
        .map_err(|_| InstalledRuntimeError::ManifestInvalid)?;
    if manifest.schema_version != 1 || manifest.runtime_id != "transcribe.cpp" {
        return Err(InstalledRuntimeError::UnsupportedRuntime);
    }
    if manifest.worker.size_bytes == 0
        || manifest.worker.size_bytes > MAX_WORKER_BYTES
        || manifest.worker.sha256.len() != 64
        || !manifest
            .worker
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(InstalledRuntimeError::ManifestInvalid);
    }
    let worker = VerifiedArtifactExecutable::bind_trust_anchored(
        &worker_path,
        &manifest.worker.sha256,
        manifest.worker.size_bytes,
        root,
        owner,
    )
    .map_err(InstalledRuntimeError::Worker)?;
    if worker.provenance() != ArtifactProvenance::TrustAnchorDigestBound {
        return Err(InstalledRuntimeError::Worker(ArtifactError::Untrusted));
    }
    Ok(InstalledRuntimeBinding { worker })
}

fn read_trusted_manifest(
    path: &Path,
    anchor: &Path,
    owner: u32,
) -> Result<Vec<u8>, InstalledRuntimeError> {
    validate_manifest_ancestry(path, anchor, owner)?;
    let anchor_path_metadata =
        std::fs::symlink_metadata(anchor).map_err(|_| InstalledRuntimeError::ManifestUntrusted)?;
    let anchor_file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(anchor)
        .map_err(|_| InstalledRuntimeError::ManifestUntrusted)?;
    let anchor_metadata = anchor_file
        .metadata()
        .map_err(|_| InstalledRuntimeError::ManifestUntrusted)?;
    if FileIdentity::from(&anchor_metadata) != FileIdentity::from(&anchor_path_metadata) {
        return Err(InstalledRuntimeError::ManifestUntrusted);
    }
    let relative = path
        .strip_prefix(anchor)
        .map_err(|_| InstalledRuntimeError::ManifestUntrusted)?;
    let mut file = rustix::fs::openat2(
        &anchor_file,
        relative,
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
        rustix::fs::ResolveFlags::BENEATH
            | rustix::fs::ResolveFlags::NO_SYMLINKS
            | rustix::fs::ResolveFlags::NO_MAGICLINKS,
    )
    .map(std::fs::File::from)
    .map_err(|error| match error {
        rustix::io::Errno::NOENT => InstalledRuntimeError::ManifestMissing,
        _ => InstalledRuntimeError::ManifestUntrusted,
    })?;
    let opened_metadata = file
        .metadata()
        .map_err(|_| InstalledRuntimeError::ManifestUntrusted)?;
    validate_manifest_leaf(&opened_metadata, owner)?;
    let length = opened_metadata.len();
    if length == 0 || length > MAX_MANIFEST_BYTES {
        return Err(InstalledRuntimeError::ManifestTooLarge);
    }
    let allocation =
        usize::try_from(length).map_err(|_| InstalledRuntimeError::ManifestTooLarge)?;
    let mut bytes = vec![0_u8; allocation];
    file.read_exact(&mut bytes)
        .map_err(|_| InstalledRuntimeError::ManifestUntrusted)?;
    let mut extra = [0_u8; 1];
    if file
        .read(&mut extra)
        .map_err(|_| InstalledRuntimeError::ManifestUntrusted)?
        != 0
    {
        return Err(InstalledRuntimeError::ManifestUntrusted);
    }
    let final_metadata = file
        .metadata()
        .map_err(|_| InstalledRuntimeError::ManifestUntrusted)?;
    if FileIdentity::from(&final_metadata) != FileIdentity::from(&opened_metadata) {
        return Err(InstalledRuntimeError::ManifestUntrusted);
    }
    Ok(bytes)
}

fn validate_manifest_leaf(metadata: &Metadata, owner: u32) -> Result<(), InstalledRuntimeError> {
    if !metadata.file_type().is_file()
        || metadata.uid() != owner
        || metadata.permissions().mode() & 0o7777 != 0o644
    {
        return Err(InstalledRuntimeError::ManifestUntrusted);
    }
    Ok(())
}

fn validate_manifest_ancestry(
    path: &Path,
    anchor: &Path,
    owner: u32,
) -> Result<(), InstalledRuntimeError> {
    if !path.is_absolute() || !anchor.is_absolute() || !path.starts_with(anchor) {
        return Err(InstalledRuntimeError::ManifestUntrusted);
    }
    let mut current = path
        .parent()
        .ok_or(InstalledRuntimeError::ManifestUntrusted)?;
    loop {
        let metadata = std::fs::symlink_metadata(current)
            .map_err(|_| InstalledRuntimeError::ManifestUntrusted)?;
        if !metadata.file_type().is_dir()
            || metadata.uid() != owner
            || metadata.permissions().mode() & 0o022 != 0
        {
            return Err(InstalledRuntimeError::ManifestUntrusted);
        }
        if current == anchor {
            return Ok(());
        }
        current = current
            .parent()
            .ok_or(InstalledRuntimeError::ManifestUntrusted)?;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
    mode: u32,
    owner: u32,
    group: u32,
    size: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

impl FileIdentity {
    fn from(metadata: &Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            mode: metadata.mode(),
            owner: metadata.uid(),
            group: metadata.gid(),
            size: metadata.size(),
            modified_seconds: metadata.mtime(),
            modified_nanoseconds: metadata.mtime_nsec(),
            changed_seconds: metadata.ctime(),
            changed_nanoseconds: metadata.ctime_nsec(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use narrivy_model_manager::{
        Capabilities, FileTransport, InstalledModelBinding, ModelStore, RegistryEntry, SourceKind,
        SourceSpec,
    };
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    impl InstalledBindingView for InstalledModelBinding {
        fn cache_root(&self) -> &Path {
            self.cache_root()
        }
        fn path(&self) -> &Path {
            self.path()
        }
        fn sha256(&self) -> &str {
            self.sha256()
        }
        fn size_bytes(&self) -> u64 {
            self.size_bytes()
        }
        fn id(&self) -> &str {
            self.id()
        }
        fn family(&self) -> &str {
            self.family()
        }
        fn source_revision(&self) -> &str {
            self.source_revision()
        }
    }

    fn owner() -> u32 {
        unsafe { libc::geteuid() }
    }

    fn install_fixture(root: &Path) -> TestResult {
        let manifest_parent = root.join("usr/share/narrivy");
        let worker_parent = root.join("usr/lib/narrivy");
        std::fs::create_dir_all(&manifest_parent)?;
        std::fs::create_dir_all(&worker_parent)?;
        for directory in [
            root,
            &root.join("usr"),
            &root.join("usr/share"),
            &manifest_parent,
            &root.join("usr/lib"),
            &worker_parent,
        ] {
            std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o755))?;
        }
        let worker = root.join(WORKER_RELATIVE_PATH);
        let bytes = std::fs::read("/bin/true")?;
        std::fs::write(&worker, &bytes)?;
        std::fs::set_permissions(&worker, std::fs::Permissions::from_mode(0o755))?;
        let digest = Sha256::digest(&bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let manifest = format!(
            "{{\"schema_version\":1,\"runtime_id\":\"transcribe.cpp\",\"worker\":{{\"sha256\":\"{digest}\",\"size_bytes\":{}}}}}",
            bytes.len()
        );
        let manifest_path = root.join(MANIFEST_RELATIVE_PATH);
        std::fs::write(&manifest_path, manifest)?;
        std::fs::set_permissions(&manifest_path, std::fs::Permissions::from_mode(0o644))?;
        Ok(())
    }

    #[test]
    fn valid_fixed_tree_returns_redacted_immutable_binding() -> TestResult {
        let root = tempfile::tempdir()?;
        install_fixture(root.path())?;
        let binding = resolve_under(root.path(), owner())?;
        assert_eq!(
            binding.worker().provenance(),
            ArtifactProvenance::TrustAnchorDigestBound
        );
        let debug = format!("{binding:?}");
        assert!(!debug.contains(&root.path().display().to_string()));
        assert!(!debug.contains(&format!("{:02x}", binding.worker().sha256()[0])));
        std::fs::write(root.path().join(WORKER_RELATIVE_PATH), b"replacement")?;
        binding.worker().revalidate()?;
        Ok(())
    }

    #[test]
    fn manifest_schema_and_semantics_fail_closed() -> TestResult {
        let invalid = [
            "{}",
            r#"{"schema_version":2,"runtime_id":"transcribe.cpp","worker":{"sha256":"00","size_bytes":1}}"#,
            r#"{"schema_version":1,"runtime_id":"other","worker":{"sha256":"00","size_bytes":1}}"#,
            r#"{"schema_version":1,"runtime_id":"transcribe.cpp","worker":{"sha256":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA","size_bytes":1}}"#,
            r#"{"schema_version":1,"runtime_id":"transcribe.cpp","worker":{"sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","size_bytes":0}}"#,
            r#"{"schema_version":1,"schema_version":1,"runtime_id":"transcribe.cpp","worker":{"sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","size_bytes":1}}"#,
            r#"{"schema_version":1,"runtime_id":"transcribe.cpp","worker":{"sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","size_bytes":1,"extra":true}}"#,
        ];
        for document in invalid {
            let root = tempfile::tempdir()?;
            install_fixture(root.path())?;
            let manifest = root.path().join(MANIFEST_RELATIVE_PATH);
            std::fs::write(&manifest, document)?;
            std::fs::set_permissions(&manifest, std::fs::Permissions::from_mode(0o644))?;
            assert!(
                resolve_under(root.path(), owner()).is_err(),
                "accepted {document}"
            );
        }
        Ok(())
    }

    #[test]
    fn manifest_and_worker_trust_fail_closed() -> TestResult {
        let root = tempfile::tempdir()?;
        install_fixture(root.path())?;
        let manifest = root.path().join(MANIFEST_RELATIVE_PATH);
        std::fs::set_permissions(&manifest, std::fs::Permissions::from_mode(0o664))?;
        assert!(matches!(
            resolve_under(root.path(), owner()),
            Err(InstalledRuntimeError::ManifestUntrusted)
        ));

        install_fixture(root.path())?;
        let worker = root.path().join(WORKER_RELATIVE_PATH);
        std::fs::set_permissions(&worker, std::fs::Permissions::from_mode(0o775))?;
        assert!(matches!(
            resolve_under(root.path(), owner()),
            Err(InstalledRuntimeError::Worker(ArtifactError::Untrusted))
        ));

        install_fixture(root.path())?;
        let share = root.path().join("usr/share");
        let real_share = root.path().join("usr/share-real");
        std::fs::rename(&share, &real_share)?;
        std::os::unix::fs::symlink(&real_share, &share)?;
        assert!(matches!(
            resolve_under(root.path(), owner()),
            Err(InstalledRuntimeError::ManifestUntrusted)
        ));
        Ok(())
    }

    #[test]
    fn oversized_manifest_is_rejected_before_parsing() -> TestResult {
        let root = tempfile::tempdir()?;
        install_fixture(root.path())?;
        let manifest = root.path().join(MANIFEST_RELATIVE_PATH);
        std::fs::write(&manifest, vec![b' '; (MAX_MANIFEST_BYTES + 1) as usize])?;
        std::fs::set_permissions(&manifest, std::fs::Permissions::from_mode(0o644))?;
        assert!(matches!(
            resolve_under(root.path(), owner()),
            Err(InstalledRuntimeError::ManifestTooLarge)
        ));
        Ok(())
    }

    #[test]
    fn verified_model_observation_becomes_redacted_immutable_binding() -> TestResult {
        let dir = tempfile::tempdir()?;
        let source = dir.path().join("source.gguf");
        let bytes = b"GGUF\x03\0\0\0installed-model";
        std::fs::write(&source, bytes)?;
        let digest = Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let entry = RegistryEntry {
            id: "test-model".into(),
            family: "whisper".into(),
            variant: "tiny".into(),
            quant: "q8_0".into(),
            size_bytes: u64::try_from(bytes.len())?,
            sha256: Some(digest),
            license: "MIT".into(),
            source_revision: "0123456789abcdef0123456789abcdef01234567".into(),
            sources: vec![SourceSpec {
                kind: SourceKind::Url,
                repo_or_url: format!("file://{}", source.display()),
                filename: "model.gguf".into(),
            }],
            capabilities: Capabilities {
                languages: vec!["en".into()],
                streaming: true,
            },
            pinned: false,
            unqualified: true,
        };
        let cache_root = dir.path().join("cache");
        let store = ModelStore::new(
            &cache_root,
            Arc::new(FileTransport),
            Arc::new(crate::model_cache_policy::PosixCacheFilePolicy::new()),
        );
        let observed = store.acquire(&entry)?;
        let binding = bind_observed_model(&observed)?;
        assert_eq!(binding.id(), entry.id);
        assert_eq!(binding.family(), entry.family);
        assert_eq!(binding.source_revision(), entry.source_revision);
        assert_eq!(
            binding.model().provenance(),
            ArtifactProvenance::TrustAnchorDigestBound
        );
        let debug = format!("{binding:?}");
        assert!(!debug.contains(&entry.id));
        assert!(!debug.contains(&cache_root.display().to_string()));
        std::fs::write(observed.path(), b"replacement")?;
        std::fs::remove_file(observed.path())?;
        binding.model().revalidate()?;
        Ok(())
    }

    #[test]
    fn verified_model_observation_supports_one_controlled_binding_budget() -> TestResult {
        let dir = tempfile::tempdir()?;
        let source = dir.path().join("source.gguf");
        let bytes = b"GGUF\x03\0\0\0controlled-installed-model";
        std::fs::write(&source, bytes)?;
        let digest = Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let entry = RegistryEntry {
            id: "controlled-test-model".into(),
            family: "whisper".into(),
            variant: "tiny".into(),
            quant: "q8_0".into(),
            size_bytes: u64::try_from(bytes.len())?,
            sha256: Some(digest),
            license: "MIT".into(),
            source_revision: "0123456789abcdef0123456789abcdef01234567".into(),
            sources: vec![SourceSpec {
                kind: SourceKind::Url,
                repo_or_url: format!("file://{}", source.display()),
                filename: "model.gguf".into(),
            }],
            capabilities: Capabilities {
                languages: vec!["en".into()],
                streaming: true,
            },
            pinned: false,
            unqualified: true,
        };
        let cache_root = dir.path().join("cache");
        let store = ModelStore::new(
            &cache_root,
            Arc::new(FileTransport),
            Arc::new(crate::model_cache_policy::PosixCacheFilePolicy::new()),
        );
        let observed = store.acquire(&entry)?;

        let cancellation = SessionCancellation::new();
        let binding = bind_observed_model_controlled(
            &observed,
            &cancellation,
            Instant::now() + std::time::Duration::from_secs(1),
        )?;
        assert_eq!(
            binding.model().provenance(),
            ArtifactProvenance::TrustAnchorDigestBound
        );
        binding.model().revalidate()?;

        let cancellation = SessionCancellation::new();
        cancellation.cancel();
        assert!(matches!(
            bind_observed_model_controlled(
                &observed,
                &cancellation,
                Instant::now() + std::time::Duration::from_secs(1),
            ),
            Err(DataArtifactError::Cancelled)
        ));

        let cancellation = SessionCancellation::new();
        assert!(matches!(
            bind_observed_model_controlled(&observed, &cancellation, Instant::now()),
            Err(DataArtifactError::TimedOut)
        ));
        Ok(())
    }
}
