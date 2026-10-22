//! Linux-only complete-directory import for bundled model identities.
//!
//! This module owns cache-root policy and every filesystem mutation. The
//! portable model manager owns registry identity and exact byte/metadata
//! verification.

use std::fs::File;
use std::io::{Read, Seek, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Component, Path};
use std::time::Instant;

use narrivy_model_manager::{
    BundledInstalledModelBinding, BundledModelIdentity, BundledModelRegistry, ModelManagerError,
    ModelStore,
};
use narrivy_types::SessionCancellation;
use rustix::fs::{AtFlags, Mode, OFlags};
use sha2::{Digest, Sha256};

const COPY_CHUNK_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfflineModelImportError {
    CancelledBeforePublication,
    TimedOutBeforePublication,
    UnknownModel,
    UnsafeCacheRoot,
    SourceUnavailable,
    SourceUnsafe,
    SourceChanged,
    BytesMismatch,
    ExistingInstallInvalid,
    Collision,
    CleanupFailedBeforePublication,
    DurabilityFailedBeforePublication,
    DurabilityFailedAfterPublication,
    VerificationFailedBeforePublication,
    VerificationFailedAfterPublication,
    Io,
}

/// Create or validate the dedicated per-user model cache root before an
/// installed daemon advertises offline provisioning. Existing insecure roots
/// are rejected without chmod; every missing component is created through an
/// already-open directory and the final root must be current-user-owned 0700.
pub fn prepare_model_cache_root(path: &Path) -> Result<(), OfflineModelImportError> {
    if !path.is_absolute() {
        return Err(OfflineModelImportError::UnsafeCacheRoot);
    }
    let relative = path
        .strip_prefix("/")
        .map_err(|_| OfflineModelImportError::UnsafeCacheRoot)?;
    let components = relative
        .components()
        .map(|component| match component {
            Component::Normal(value) => Ok(value),
            _ => Err(OfflineModelImportError::UnsafeCacheRoot),
        })
        .collect::<Result<Vec<_>, _>>()?;
    if components.is_empty() {
        return Err(OfflineModelImportError::UnsafeCacheRoot);
    }

    let owner = rustix::process::getuid().as_raw();
    let mut current = File::open("/").map_err(|_| OfflineModelImportError::UnsafeCacheRoot)?;
    for (index, component) in components.iter().enumerate() {
        let mut created = false;
        let next = match rustix::fs::openat(
            &current,
            *component,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(descriptor) => File::from(descriptor),
            Err(error) if error == rustix::io::Errno::NOENT => {
                match rustix::fs::mkdirat(&current, *component, Mode::RWXU) {
                    Ok(()) => created = true,
                    Err(rustix::io::Errno::EXIST) => {}
                    Err(_) => return Err(OfflineModelImportError::UnsafeCacheRoot),
                }
                rustix::fs::openat(
                    &current,
                    *component,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map(File::from)
                .map_err(|_| OfflineModelImportError::UnsafeCacheRoot)?
            }
            Err(_) => return Err(OfflineModelImportError::UnsafeCacheRoot),
        };
        if created {
            next.set_permissions(std::fs::Permissions::from_mode(0o700))
                .map_err(|_| OfflineModelImportError::UnsafeCacheRoot)?;
            next.sync_all()
                .map_err(|_| OfflineModelImportError::UnsafeCacheRoot)?;
            current
                .sync_all()
                .map_err(|_| OfflineModelImportError::UnsafeCacheRoot)?;
        }
        if index + 1 == components.len() {
            validate_private_directory(&next, owner)?;
        }
        current = next;
    }
    Ok(())
}

impl std::fmt::Display for OfflineModelImportError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::CancelledBeforePublication => {
                "offline model import was cancelled before publication"
            }
            Self::TimedOutBeforePublication => "offline model import timed out before publication",
            Self::UnknownModel => "model id is not present in the bundled registry",
            Self::UnsafeCacheRoot => "dedicated model cache root is unsafe",
            Self::SourceUnavailable => "offline model source is unavailable",
            Self::SourceUnsafe => "offline model source is unsafe",
            Self::SourceChanged => "offline model source changed during import",
            Self::BytesMismatch => "offline model bytes do not match bundled identity",
            Self::ExistingInstallInvalid => "existing model installation is invalid",
            Self::Collision => "offline model import staging collided repeatedly",
            Self::CleanupFailedBeforePublication => {
                "offline model import cleanup could not confirm removal before publication"
            }
            Self::DurabilityFailedBeforePublication => {
                "offline model import durability sync failed before publication"
            }
            Self::DurabilityFailedAfterPublication => {
                "offline model import was published but durability confirmation failed"
            }
            Self::VerificationFailedBeforePublication => {
                "offline model import verification failed before publication"
            }
            Self::VerificationFailedAfterPublication => {
                "offline model import was published but verification confirmation failed"
            }
            Self::Io => "offline model import filesystem operation failed",
        })
    }
}

impl std::error::Error for OfflineModelImportError {}

/// Import one local file under the exact model identity compiled into this
/// build. `deadline` is one absolute caller-owned budget and is never reset.
///
/// Cancellation is cooperative around bounded chunks and filesystem phases;
/// a blocking kernel filesystem operation can still outlive the deadline.
/// `RENAME_NOREPLACE` is the irreversible publication point. Once this call
/// publishes, it never reports cancellation or timeout and never removes the
/// destination; post-publication confirmation failures are typed separately.
/// No transport, CLI, or daemon policy is involved.
pub fn import_bundled_model(
    store: &ModelStore,
    registry: &BundledModelRegistry,
    id: &str,
    source_path: &Path,
    cancellation: &SessionCancellation,
    deadline: Instant,
) -> Result<BundledInstalledModelBinding, OfflineModelImportError> {
    let identity = registry.resolve_import(id).map_err(map_identity_error)?;
    check_control(cancellation, deadline)?;
    let disposition = import_identity_without_postpublication_rehash(
        store.cache_root(),
        &identity,
        source_path,
        cancellation,
        deadline,
        &mut ProductionHooks,
    )?;
    match disposition {
        ImportDisposition::Published => store
            .verify_bundled_installed_controlled(registry, id, cancellation, deadline)
            .map_err(|_| OfflineModelImportError::VerificationFailedAfterPublication),
        ImportDisposition::ExistingVerified => store
            .verify_bundled_installed_controlled(registry, id, cancellation, deadline)
            .map_err(map_existing_binding_error),
    }
}

trait ImportIdentity {
    fn id(&self) -> &str;
    fn family(&self) -> &str;
    fn sha256(&self) -> &str;
    fn size_bytes(&self) -> u64;
    fn metadata_bytes(&self, acquired_at_unix: u64) -> Result<Vec<u8>, ModelManagerError>;
    fn verify_metadata(&self, bytes: &[u8]) -> Result<(), ModelManagerError>;
}

impl ImportIdentity for BundledModelIdentity {
    fn id(&self) -> &str {
        self.id()
    }

    fn family(&self) -> &str {
        self.family()
    }

    fn sha256(&self) -> &str {
        self.sha256()
    }

    fn size_bytes(&self) -> u64 {
        self.size_bytes()
    }

    fn metadata_bytes(&self, acquired_at_unix: u64) -> Result<Vec<u8>, ModelManagerError> {
        self.installed_metadata_bytes(acquired_at_unix)
    }

    fn verify_metadata(&self, bytes: &[u8]) -> Result<(), ModelManagerError> {
        self.verify_installed_metadata(bytes)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Checkpoint {
    BeforeSourceIo,
    StageCreated,
    BeforeCopy,
    CopyChunk,
    VerifyChunk,
    Staged,
    BeforePublish,
    AfterPublish,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncPoint {
    CreatedDirectory,
    CreatedDirectoryParent,
    ModelLeaf,
    MetadataLeaf,
    StagedDirectory,
    StagingParentAfterRename,
    DestinationParent,
}

trait Hooks {
    fn checkpoint(&mut self, _point: Checkpoint) {}
    fn sync(&mut self, file: &File, _point: SyncPoint) -> std::io::Result<()> {
        file.sync_all()
    }
}

struct ProductionHooks;
impl Hooks for ProductionHooks {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImportDisposition {
    Published,
    ExistingVerified,
}

#[cfg(test)]
fn import_identity(
    cache_root: &Path,
    identity: &impl ImportIdentity,
    source_path: &Path,
    cancellation: &SessionCancellation,
    deadline: Instant,
    hooks: &mut impl Hooks,
) -> Result<ImportDisposition, OfflineModelImportError> {
    import_identity_with_postpublication_verification(
        cache_root,
        identity,
        source_path,
        cancellation,
        deadline,
        hooks,
        true,
    )
}

fn import_identity_without_postpublication_rehash(
    cache_root: &Path,
    identity: &impl ImportIdentity,
    source_path: &Path,
    cancellation: &SessionCancellation,
    deadline: Instant,
    hooks: &mut impl Hooks,
) -> Result<ImportDisposition, OfflineModelImportError> {
    import_identity_with_postpublication_verification(
        cache_root,
        identity,
        source_path,
        cancellation,
        deadline,
        hooks,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
fn import_identity_with_postpublication_verification(
    cache_root: &Path,
    identity: &impl ImportIdentity,
    source_path: &Path,
    cancellation: &SessionCancellation,
    deadline: Instant,
    hooks: &mut impl Hooks,
    verify_after_publication: bool,
) -> Result<ImportDisposition, OfflineModelImportError> {
    if !is_safe_component(identity.id()) || !is_safe_component(identity.family()) {
        return Err(OfflineModelImportError::UnknownModel);
    }
    hooks.checkpoint(Checkpoint::BeforeSourceIo);
    check_control(cancellation, deadline)?;
    let root = open_dedicated_root(cache_root)?;
    check_control(cancellation, deadline)?;
    let owner = rustix::process::getuid().as_raw();
    let mut source = open_source(source_path, identity.size_bytes(), owner)?;
    check_control(cancellation, deadline)?;
    let source_before = source
        .metadata()
        .map_err(|_| OfflineModelImportError::SourceUnsafe)?;
    check_control(cancellation, deadline)?;

    let staging_parent =
        open_or_create_private_chain(&root, &["staging"], owner, cancellation, deadline, hooks)?;
    let mut stage = OwnedStage::create(
        staging_parent,
        identity.id(),
        owner,
        cancellation,
        deadline,
        hooks,
    )?;
    let result = complete_staged_import(
        &root,
        &mut stage,
        &mut source,
        &source_before,
        identity,
        owner,
        cancellation,
        deadline,
        hooks,
        verify_after_publication,
    );
    match result {
        Err(error) if stage.active => Err(stage.abort_for_failure(error, hooks)),
        result => result,
    }
}

#[allow(clippy::too_many_arguments)]
fn complete_staged_import(
    root: &File,
    stage: &mut OwnedStage,
    source: &mut File,
    source_before: &std::fs::Metadata,
    identity: &impl ImportIdentity,
    owner: u32,
    cancellation: &SessionCancellation,
    deadline: Instant,
    hooks: &mut impl Hooks,
    verify_after_publication: bool,
) -> Result<ImportDisposition, OfflineModelImportError> {
    hooks.checkpoint(Checkpoint::BeforeCopy);
    check_control(cancellation, deadline)?;
    let mut model = stage.create_leaf("model.gguf", owner)?;
    check_control(cancellation, deadline)?;
    copy_and_verify_exact(source, &mut model, identity, cancellation, deadline, hooks)?;
    check_control(cancellation, deadline)?;
    hooks
        .sync(&model, SyncPoint::ModelLeaf)
        .map_err(|_| OfflineModelImportError::DurabilityFailedBeforePublication)?;
    check_control(cancellation, deadline)?;
    model
        .seek(std::io::SeekFrom::Start(0))
        .map_err(|_| OfflineModelImportError::Io)?;
    verify_file_exact_digest(&mut model, identity, Some((cancellation, deadline)), hooks).map_err(
        |error| {
            if is_control_error(error) {
                error
            } else {
                OfflineModelImportError::VerificationFailedBeforePublication
            }
        },
    )?;
    check_control(cancellation, deadline)?;

    let metadata = identity
        .metadata_bytes(unix_time())
        .map_err(|_| OfflineModelImportError::VerificationFailedBeforePublication)?;
    check_control(cancellation, deadline)?;
    let mut meta = stage.create_leaf("meta.json", owner)?;
    check_control(cancellation, deadline)?;
    meta.write_all(&metadata)
        .map_err(|_| OfflineModelImportError::Io)?;
    check_control(cancellation, deadline)?;
    hooks
        .sync(&meta, SyncPoint::MetadataLeaf)
        .map_err(|_| OfflineModelImportError::DurabilityFailedBeforePublication)?;
    check_control(cancellation, deadline)?;
    hooks
        .sync(&stage.directory, SyncPoint::StagedDirectory)
        .map_err(|_| OfflineModelImportError::DurabilityFailedBeforePublication)?;
    hooks.checkpoint(Checkpoint::Staged);
    check_control(cancellation, deadline)?;

    let source_after = source
        .metadata()
        .map_err(|_| OfflineModelImportError::SourceChanged)?;
    if !same_unchanged_file(source_before, &source_after) {
        return Err(OfflineModelImportError::SourceChanged);
    }

    let prefix = identity
        .sha256()
        .get(..12)
        .ok_or(OfflineModelImportError::VerificationFailedBeforePublication)?;
    let destination_parent = open_or_create_private_chain(
        root,
        &["models", identity.family(), identity.id()],
        owner,
        cancellation,
        deadline,
        hooks,
    )?;
    hooks.checkpoint(Checkpoint::BeforePublish);
    // This is the final control observation before the irreversible rename.
    // After publication, control expiry may stop confirmation work but must
    // be reported as publication uncertainty, never as clean cancellation.
    check_control(cancellation, deadline)?;

    match stage.publish(&destination_parent, prefix) {
        Ok(()) => {
            hooks.checkpoint(Checkpoint::AfterPublish);
            hooks
                .sync(&stage.parent, SyncPoint::StagingParentAfterRename)
                .map_err(|_| OfflineModelImportError::DurabilityFailedAfterPublication)?;
            hooks
                .sync(&destination_parent, SyncPoint::DestinationParent)
                .map_err(|_| OfflineModelImportError::DurabilityFailedAfterPublication)?;
            if verify_after_publication {
                verify_complete(
                    &destination_parent,
                    prefix,
                    identity,
                    owner,
                    Some((cancellation, deadline)),
                    hooks,
                )
                .map_err(|_| OfflineModelImportError::VerificationFailedAfterPublication)?;
            }
            Ok(ImportDisposition::Published)
        }
        Err(error) if error == rustix::io::Errno::EXIST => {
            stage
                .cleanup_and_sync(hooks)
                .map_err(|_| OfflineModelImportError::CleanupFailedBeforePublication)?;
            check_control(cancellation, deadline)?;
            hooks
                .sync(&destination_parent, SyncPoint::DestinationParent)
                .map_err(|_| OfflineModelImportError::DurabilityFailedBeforePublication)?;
            check_control(cancellation, deadline)?;
            verify_complete(
                &destination_parent,
                prefix,
                identity,
                owner,
                Some((cancellation, deadline)),
                hooks,
            )?;
            Ok(ImportDisposition::ExistingVerified)
        }
        Err(_) => Err(OfflineModelImportError::Io),
    }
}

fn check_control(
    cancellation: &SessionCancellation,
    deadline: Instant,
) -> Result<(), OfflineModelImportError> {
    // Explicit cancellation deterministically wins when both conditions are
    // observed at one checkpoint.
    if cancellation.is_cancelled() {
        Err(OfflineModelImportError::CancelledBeforePublication)
    } else if Instant::now() >= deadline {
        Err(OfflineModelImportError::TimedOutBeforePublication)
    } else {
        Ok(())
    }
}

fn is_control_error(error: OfflineModelImportError) -> bool {
    matches!(
        error,
        OfflineModelImportError::CancelledBeforePublication
            | OfflineModelImportError::TimedOutBeforePublication
    )
}

fn is_safe_component(value: &str) -> bool {
    !value.is_empty()
        && Path::new(value).components().count() == 1
        && matches!(
            Path::new(value).components().next(),
            Some(Component::Normal(_))
        )
}

fn open_dedicated_root(path: &Path) -> Result<File, OfflineModelImportError> {
    if !path.is_absolute() {
        return Err(OfflineModelImportError::UnsafeCacheRoot);
    }
    let relative = path
        .strip_prefix("/")
        .map_err(|_| OfflineModelImportError::UnsafeCacheRoot)?;
    if relative
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(OfflineModelImportError::UnsafeCacheRoot);
    }
    let slash = File::open("/").map_err(|_| OfflineModelImportError::UnsafeCacheRoot)?;
    let root = rustix::fs::openat2(
        &slash,
        relative,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        rustix::fs::ResolveFlags::BENEATH
            | rustix::fs::ResolveFlags::NO_SYMLINKS
            | rustix::fs::ResolveFlags::NO_MAGICLINKS,
    )
    .map(File::from)
    .map_err(|_| OfflineModelImportError::UnsafeCacheRoot)?;
    validate_private_directory(&root, rustix::process::getuid().as_raw())?;
    Ok(root)
}

fn open_source(
    path: &Path,
    expected_size: u64,
    owner: u32,
) -> Result<File, OfflineModelImportError> {
    if !path.is_absolute() {
        return Err(OfflineModelImportError::SourceUnsafe);
    }
    let relative = path
        .strip_prefix("/")
        .map_err(|_| OfflineModelImportError::SourceUnsafe)?;
    if relative
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(OfflineModelImportError::SourceUnsafe);
    }
    let slash = File::open("/").map_err(|_| OfflineModelImportError::SourceUnavailable)?;
    let file = rustix::fs::openat2(
        &slash,
        relative,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
        rustix::fs::ResolveFlags::BENEATH
            | rustix::fs::ResolveFlags::NO_SYMLINKS
            | rustix::fs::ResolveFlags::NO_MAGICLINKS,
    )
    .map(File::from)
    .map_err(|error| {
        if error == rustix::io::Errno::NOENT {
            OfflineModelImportError::SourceUnavailable
        } else {
            OfflineModelImportError::SourceUnsafe
        }
    })?;
    let metadata = file
        .metadata()
        .map_err(|_| OfflineModelImportError::SourceUnsafe)?;
    if !metadata.is_file()
        || metadata.uid() != owner
        || metadata.size() != expected_size
        || metadata.mode() & 0o6022 != 0
    {
        return Err(OfflineModelImportError::SourceUnsafe);
    }
    Ok(file)
}

fn open_or_create_private_chain(
    root: &File,
    components: &[&str],
    owner: u32,
    cancellation: &SessionCancellation,
    deadline: Instant,
    hooks: &mut impl Hooks,
) -> Result<File, OfflineModelImportError> {
    let mut current = root.try_clone().map_err(|_| OfflineModelImportError::Io)?;
    for component in components {
        check_control(cancellation, deadline)?;
        let next = match rustix::fs::openat(
            &current,
            *component,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => File::from(fd),
            Err(error) if error == rustix::io::Errno::NOENT => {
                let created = match rustix::fs::mkdirat(&current, *component, Mode::RWXU) {
                    Ok(()) => true,
                    Err(rustix::io::Errno::EXIST) => false,
                    Err(_) => return Err(OfflineModelImportError::Io),
                };
                let directory = rustix::fs::openat(
                    &current,
                    *component,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map(File::from)
                .map_err(|_| OfflineModelImportError::Io)?;
                if created {
                    check_control(cancellation, deadline)?;
                    directory
                        .set_permissions(std::fs::Permissions::from_mode(0o700))
                        .map_err(|_| OfflineModelImportError::Io)?;
                    check_control(cancellation, deadline)?;
                    hooks
                        .sync(&directory, SyncPoint::CreatedDirectory)
                        .map_err(|_| OfflineModelImportError::DurabilityFailedBeforePublication)?;
                    check_control(cancellation, deadline)?;
                    hooks
                        .sync(&current, SyncPoint::CreatedDirectoryParent)
                        .map_err(|_| OfflineModelImportError::DurabilityFailedBeforePublication)?;
                }
                directory
            }
            Err(_) => return Err(OfflineModelImportError::UnsafeCacheRoot),
        };
        validate_private_directory(&next, owner)?;
        check_control(cancellation, deadline)?;
        current = next;
    }
    Ok(current)
}

fn validate_private_directory(file: &File, owner: u32) -> Result<(), OfflineModelImportError> {
    let metadata = file
        .metadata()
        .map_err(|_| OfflineModelImportError::UnsafeCacheRoot)?;
    if !metadata.is_dir() || metadata.uid() != owner || metadata.mode() & 0o777 != 0o700 {
        return Err(OfflineModelImportError::UnsafeCacheRoot);
    }
    Ok(())
}

struct OwnedStage {
    parent: File,
    directory: File,
    name: String,
    active: bool,
}

impl OwnedStage {
    fn create(
        parent: File,
        id: &str,
        owner: u32,
        cancellation: &SessionCancellation,
        deadline: Instant,
        hooks: &mut impl Hooks,
    ) -> Result<Self, OfflineModelImportError> {
        for _ in 0..32 {
            check_control(cancellation, deadline)?;
            let name = format!("{id}-{:.0x}.install", nonce());
            match rustix::fs::mkdirat(&parent, name.as_str(), Mode::RWXU) {
                Ok(()) => {
                    hooks.checkpoint(Checkpoint::StageCreated);
                    let directory = match rustix::fs::openat(
                        &parent,
                        name.as_str(),
                        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                        Mode::empty(),
                    )
                    .map(File::from)
                    {
                        Ok(directory) => directory,
                        Err(_) => {
                            return Err(cleanup_new_stage_directory(
                                &parent,
                                name.as_str(),
                                OfflineModelImportError::Io,
                                hooks,
                            ));
                        }
                    };
                    let mut stage = Self {
                        parent,
                        directory,
                        name,
                        active: true,
                    };
                    let setup = (|| {
                        stage
                            .directory
                            .set_permissions(std::fs::Permissions::from_mode(0o700))
                            .map_err(|_| OfflineModelImportError::Io)?;
                        validate_private_directory(&stage.directory, owner)?;
                        check_control(cancellation, deadline)?;
                        hooks
                            .sync(&stage.directory, SyncPoint::CreatedDirectory)
                            .map_err(|_| {
                                OfflineModelImportError::DurabilityFailedBeforePublication
                            })?;
                        check_control(cancellation, deadline)?;
                        hooks
                            .sync(&stage.parent, SyncPoint::CreatedDirectoryParent)
                            .map_err(|_| {
                                OfflineModelImportError::DurabilityFailedBeforePublication
                            })?;
                        Ok(())
                    })();
                    return match setup {
                        Ok(()) => Ok(stage),
                        Err(error) => Err(stage.abort_for_failure(error, hooks)),
                    };
                }
                Err(error) if error == rustix::io::Errno::EXIST => continue,
                Err(_) => return Err(OfflineModelImportError::Io),
            }
        }
        Err(OfflineModelImportError::Collision)
    }

    fn create_leaf(&self, name: &str, owner: u32) -> Result<File, OfflineModelImportError> {
        let file = rustix::fs::openat(
            &self.directory,
            name,
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .map(File::from)
        .map_err(|_| OfflineModelImportError::Io)?;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|_| OfflineModelImportError::Io)?;
        let metadata = file.metadata().map_err(|_| OfflineModelImportError::Io)?;
        if !metadata.is_file() || metadata.uid() != owner || metadata.mode() & 0o777 != 0o600 {
            return Err(OfflineModelImportError::Io);
        }
        Ok(file)
    }

    fn publish(&mut self, destination: &File, name: &str) -> Result<(), rustix::io::Errno> {
        rustix::fs::renameat_with(
            &self.parent,
            self.name.as_str(),
            destination,
            name,
            rustix::fs::RenameFlags::NOREPLACE,
        )?;
        self.active = false;
        Ok(())
    }

    fn cleanup(&mut self) {
        if !self.active {
            return;
        }
        let _ = rustix::fs::unlinkat(&self.directory, "model.gguf", AtFlags::empty());
        let _ = rustix::fs::unlinkat(&self.directory, "meta.json", AtFlags::empty());
        let _ = rustix::fs::unlinkat(&self.parent, self.name.as_str(), AtFlags::REMOVEDIR);
        self.active = false;
    }

    fn cleanup_and_sync(&mut self, hooks: &mut impl Hooks) -> Result<(), ()> {
        if !self.active {
            return Ok(());
        }
        for name in ["model.gguf", "meta.json"] {
            if let Err(error) = rustix::fs::unlinkat(&self.directory, name, AtFlags::empty()) {
                if error != rustix::io::Errno::NOENT {
                    return Err(());
                }
            }
        }
        rustix::fs::unlinkat(&self.parent, self.name.as_str(), AtFlags::REMOVEDIR)
            .map_err(|_| ())?;
        self.active = false;
        hooks
            .sync(&self.parent, SyncPoint::StagingParentAfterRename)
            .map_err(|_| ())
    }

    fn abort_for_failure(
        &mut self,
        error: OfflineModelImportError,
        hooks: &mut impl Hooks,
    ) -> OfflineModelImportError {
        if self.cleanup_and_sync(hooks).is_ok() {
            error
        } else {
            OfflineModelImportError::CleanupFailedBeforePublication
        }
    }
}

fn cleanup_new_stage_directory(
    parent: &File,
    name: &str,
    error: OfflineModelImportError,
    hooks: &mut impl Hooks,
) -> OfflineModelImportError {
    if rustix::fs::unlinkat(parent, name, AtFlags::REMOVEDIR).is_ok()
        && hooks
            .sync(parent, SyncPoint::StagingParentAfterRename)
            .is_ok()
    {
        error
    } else {
        OfflineModelImportError::CleanupFailedBeforePublication
    }
}

impl Drop for OwnedStage {
    fn drop(&mut self) {
        let was_active = self.active;
        self.cleanup();
        if was_active {
            // Best effort only in Drop. Explicit concurrent-loser cleanup
            // reports sync failures through the normal result path.
            let _ = self.parent.sync_all();
        }
    }
}

fn verify_complete(
    parent: &File,
    name: &str,
    identity: &impl ImportIdentity,
    owner: u32,
    control: Option<(&SessionCancellation, Instant)>,
    hooks: &mut impl Hooks,
) -> Result<(), OfflineModelImportError> {
    check_optional_control(control)?;
    let directory = rustix::fs::openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|_| OfflineModelImportError::ExistingInstallInvalid)?;
    check_optional_control(control)?;
    validate_private_directory(&directory, owner)
        .map_err(|_| OfflineModelImportError::ExistingInstallInvalid)?;
    verify_exact_leaf_names(&directory, control)?;
    check_optional_control(control)?;
    let mut model = open_private_leaf(&directory, "model.gguf", owner)?;
    verify_file_exact_digest(&mut model, identity, control, hooks)
        .map_err(|error| map_verification_control(error, control))?;
    check_optional_control(control)?;
    let mut meta = open_private_leaf(&directory, "meta.json", owner)?;
    let bytes = read_bounded(&mut meta, 16 * 1024, control)?;
    identity
        .verify_metadata(&bytes)
        .map_err(|_| OfflineModelImportError::ExistingInstallInvalid)
}

fn verify_exact_leaf_names(
    directory: &File,
    control: Option<(&SessionCancellation, Instant)>,
) -> Result<(), OfflineModelImportError> {
    let mut buffer = [std::mem::MaybeUninit::<u8>::uninit(); 4096];
    let mut entries = rustix::fs::RawDir::new(directory, &mut buffer);
    let mut model = false;
    let mut metadata = false;
    while let Some(entry) = entries.next() {
        check_optional_control(control)?;
        let entry = entry.map_err(|_| OfflineModelImportError::ExistingInstallInvalid)?;
        match entry.file_name().to_bytes() {
            b"." | b".." => {}
            b"model.gguf" if !model => model = true,
            b"meta.json" if !metadata => metadata = true,
            _ => return Err(OfflineModelImportError::ExistingInstallInvalid),
        }
    }
    if !model || !metadata {
        return Err(OfflineModelImportError::ExistingInstallInvalid);
    }
    Ok(())
}

fn open_private_leaf(
    directory: &File,
    name: &str,
    owner: u32,
) -> Result<File, OfflineModelImportError> {
    let file = rustix::fs::openat(
        directory,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|_| OfflineModelImportError::ExistingInstallInvalid)?;
    let metadata = file
        .metadata()
        .map_err(|_| OfflineModelImportError::ExistingInstallInvalid)?;
    if !metadata.is_file() || metadata.uid() != owner || metadata.mode() & 0o777 != 0o600 {
        return Err(OfflineModelImportError::ExistingInstallInvalid);
    }
    Ok(file)
}

fn copy_and_verify_exact(
    source: &mut File,
    destination: &mut File,
    identity: &impl ImportIdentity,
    cancellation: &SessionCancellation,
    deadline: Instant,
    hooks: &mut impl Hooks,
) -> Result<(), OfflineModelImportError> {
    let mut digest = Sha256::new();
    let mut copied = 0_u64;
    let mut buffer = [0_u8; COPY_CHUNK_BYTES];
    loop {
        check_control(cancellation, deadline)?;
        let remaining_with_sentinel = identity
            .size_bytes()
            .saturating_add(1)
            .saturating_sub(copied);
        if remaining_with_sentinel == 0 {
            return Err(OfflineModelImportError::BytesMismatch);
        }
        let wanted = usize::try_from(remaining_with_sentinel)
            .unwrap_or(usize::MAX)
            .min(buffer.len());
        let read = source
            .read(&mut buffer[..wanted])
            .map_err(|_| OfflineModelImportError::Io)?;
        check_control(cancellation, deadline)?;
        if read == 0 {
            break;
        }
        let mut written = 0;
        while written < read {
            check_control(cancellation, deadline)?;
            let count = destination
                .write(&buffer[written..read])
                .map_err(|_| OfflineModelImportError::Io)?;
            if count == 0 {
                return Err(OfflineModelImportError::Io);
            }
            written += count;
            check_control(cancellation, deadline)?;
        }
        digest.update(&buffer[..read]);
        copied = copied.saturating_add(read as u64);
        hooks.checkpoint(Checkpoint::CopyChunk);
        check_control(cancellation, deadline)?;
    }
    if copied != identity.size_bytes()
        || lower_hex(digest.finalize().as_slice()) != identity.sha256()
    {
        return Err(OfflineModelImportError::BytesMismatch);
    }
    Ok(())
}

fn verify_file_exact_digest(
    file: &mut File,
    identity: &impl ImportIdentity,
    control: Option<(&SessionCancellation, Instant)>,
    hooks: &mut impl Hooks,
) -> Result<(), OfflineModelImportError> {
    let mut digest = Sha256::new();
    let mut read_total = 0_u64;
    let mut buffer = [0_u8; COPY_CHUNK_BYTES];
    loop {
        check_optional_control(control)?;
        let remaining_with_sentinel = identity
            .size_bytes()
            .saturating_add(1)
            .saturating_sub(read_total);
        if remaining_with_sentinel == 0 {
            return Err(OfflineModelImportError::ExistingInstallInvalid);
        }
        let wanted = usize::try_from(remaining_with_sentinel)
            .unwrap_or(usize::MAX)
            .min(buffer.len());
        let read = file
            .read(&mut buffer[..wanted])
            .map_err(|_| OfflineModelImportError::ExistingInstallInvalid)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
        read_total = read_total.saturating_add(read as u64);
        hooks.checkpoint(Checkpoint::VerifyChunk);
        check_optional_control(control)?;
    }
    if read_total != identity.size_bytes()
        || lower_hex(digest.finalize().as_slice()) != identity.sha256()
    {
        return Err(OfflineModelImportError::ExistingInstallInvalid);
    }
    Ok(())
}

fn read_bounded(
    file: &mut File,
    limit: usize,
    control: Option<(&SessionCancellation, Instant)>,
) -> Result<Vec<u8>, OfflineModelImportError> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        check_optional_control(control)?;
        let remaining = limit.saturating_add(1).saturating_sub(bytes.len());
        if remaining == 0 {
            return Err(OfflineModelImportError::ExistingInstallInvalid);
        }
        let wanted = remaining.min(buffer.len());
        let read = file
            .read(&mut buffer[..wanted])
            .map_err(|_| OfflineModelImportError::ExistingInstallInvalid)?;
        if read == 0 {
            return Ok(bytes);
        }
        bytes.extend_from_slice(&buffer[..read]);
        check_optional_control(control)?;
    }
}

fn lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len().saturating_mul(2));
    for &byte in bytes {
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    out
}

fn check_optional_control(
    control: Option<(&SessionCancellation, Instant)>,
) -> Result<(), OfflineModelImportError> {
    match control {
        Some((cancellation, deadline)) => check_control(cancellation, deadline),
        None => Ok(()),
    }
}

fn map_verification_control(
    error: OfflineModelImportError,
    control: Option<(&SessionCancellation, Instant)>,
) -> OfflineModelImportError {
    if control.is_some() && is_control_error(error) {
        error
    } else {
        OfflineModelImportError::ExistingInstallInvalid
    }
}

fn same_unchanged_file(before: &std::fs::Metadata, after: &std::fs::Metadata) -> bool {
    before.dev() == after.dev()
        && before.ino() == after.ino()
        && before.mode() == after.mode()
        && before.uid() == after.uid()
        && before.gid() == after.gid()
        && before.size() == after.size()
        && before.mtime() == after.mtime()
        && before.mtime_nsec() == after.mtime_nsec()
        && before.ctime() == after.ctime()
        && before.ctime_nsec() == after.ctime_nsec()
}

fn unix_time() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn nonce() -> u128 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    unix_time() as u128 ^ (u128::from(COUNTER.fetch_add(1, Ordering::Relaxed)) << 64)
}

fn map_identity_error(error: ModelManagerError) -> OfflineModelImportError {
    match error {
        ModelManagerError::UnknownBundledModel => OfflineModelImportError::UnknownModel,
        _ => OfflineModelImportError::VerificationFailedBeforePublication,
    }
}

fn map_existing_binding_error(error: ModelManagerError) -> OfflineModelImportError {
    match error {
        ModelManagerError::InstalledVerificationCancelled => {
            OfflineModelImportError::CancelledBeforePublication
        }
        ModelManagerError::InstalledVerificationTimedOut => {
            OfflineModelImportError::TimedOutBeforePublication
        }
        _ => OfflineModelImportError::ExistingInstallInvalid,
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use narrivy_model_manager::NoNetworkTransport;
    use std::sync::Arc;
    use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
    use std::time::Duration;

    type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

    #[test]
    fn prepare_cache_root_creates_private_idempotent_directory() -> TestResult {
        let parent = tempfile::tempdir()?;
        let root = parent.path().join("narrivy");
        prepare_model_cache_root(&root)?;
        let metadata = std::fs::symlink_metadata(&root)?;
        assert!(metadata.is_dir());
        assert_eq!(metadata.uid(), rustix::process::getuid().as_raw());
        assert_eq!(metadata.mode() & 0o777, 0o700);
        prepare_model_cache_root(&root)?;
        Ok(())
    }

    #[test]
    fn prepare_cache_root_rejects_existing_insecure_or_symlink_root() -> TestResult {
        let parent = tempfile::tempdir()?;
        let insecure = parent.path().join("insecure");
        std::fs::create_dir(&insecure)?;
        std::fs::set_permissions(&insecure, std::fs::Permissions::from_mode(0o755))?;
        assert_eq!(
            prepare_model_cache_root(&insecure),
            Err(OfflineModelImportError::UnsafeCacheRoot)
        );
        assert_eq!(std::fs::symlink_metadata(&insecure)?.mode() & 0o777, 0o755);

        let link = parent.path().join("linked");
        std::os::unix::fs::symlink(&insecure, &link)?;
        assert_eq!(
            prepare_model_cache_root(&link),
            Err(OfflineModelImportError::UnsafeCacheRoot)
        );
        Ok(())
    }

    #[derive(Clone)]
    struct FakeIdentity {
        bytes: Vec<u8>,
        digest: String,
    }

    impl FakeIdentity {
        fn new() -> Self {
            let mut bytes = vec![b'G', b'G', b'U', b'F'];
            bytes.resize(COPY_CHUNK_BYTES * 3, 0x35);
            let digest = Sha256::digest(&bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            Self { bytes, digest }
        }
    }

    impl ImportIdentity for FakeIdentity {
        fn id(&self) -> &str {
            "fixture-model"
        }

        fn family(&self) -> &str {
            "fixture-family"
        }

        fn sha256(&self) -> &str {
            &self.digest
        }

        fn size_bytes(&self) -> u64 {
            self.bytes.len() as u64
        }

        fn metadata_bytes(&self, acquired_at_unix: u64) -> Result<Vec<u8>, ModelManagerError> {
            Ok(serde_json::to_vec(&serde_json::json!({
                "id": self.id(),
                "sha256": self.sha256(),
                "size_bytes": self.size_bytes(),
                "acquired_at_unix": acquired_at_unix
            }))?)
        }

        fn verify_metadata(&self, bytes: &[u8]) -> Result<(), ModelManagerError> {
            let value: serde_json::Value = serde_json::from_slice(bytes)?;
            if value.get("id").and_then(serde_json::Value::as_str) == Some(self.id())
                && value.get("sha256").and_then(serde_json::Value::as_str) == Some(self.sha256())
                && value.get("size_bytes").and_then(serde_json::Value::as_u64)
                    == Some(self.size_bytes())
            {
                Ok(())
            } else {
                Err(ModelManagerError::InstalledMetadataMismatch {
                    id: self.id().to_owned(),
                })
            }
        }
    }

    #[derive(Default)]
    struct TestHooks {
        fail_sync: Option<SyncPoint>,
        syncs: Vec<SyncPoint>,
        before_source_io: Option<Box<dyn FnMut() + Send>>,
        stage_created: Option<Box<dyn FnMut() + Send>>,
        before_copy: Option<Box<dyn FnMut() + Send>>,
        copy_chunk: Option<Box<dyn FnMut() + Send>>,
        verify_chunk: Option<Box<dyn FnMut() + Send>>,
        staged: Option<Box<dyn FnMut() + Send>>,
        before_publish: Option<Box<dyn FnMut() + Send>>,
        after_publish: Option<Box<dyn FnMut() + Send>>,
    }

    impl Hooks for TestHooks {
        fn checkpoint(&mut self, point: Checkpoint) {
            match point {
                Checkpoint::BeforeSourceIo => {
                    if let Some(callback) = self.before_source_io.as_mut() {
                        callback();
                    }
                }
                Checkpoint::StageCreated => {
                    if let Some(callback) = self.stage_created.as_mut() {
                        callback();
                    }
                }
                Checkpoint::BeforeCopy => {
                    if let Some(callback) = self.before_copy.as_mut() {
                        callback();
                    }
                }
                Checkpoint::CopyChunk => {
                    if let Some(callback) = self.copy_chunk.as_mut() {
                        callback();
                    }
                }
                Checkpoint::VerifyChunk => {
                    if let Some(callback) = self.verify_chunk.as_mut() {
                        callback();
                    }
                }
                Checkpoint::Staged => {
                    if let Some(callback) = self.staged.as_mut() {
                        callback();
                    }
                }
                Checkpoint::BeforePublish => {
                    if let Some(callback) = self.before_publish.as_mut() {
                        callback();
                    }
                }
                Checkpoint::AfterPublish => {
                    if let Some(callback) = self.after_publish.as_mut() {
                        callback();
                    }
                }
            }
        }

        fn sync(&mut self, file: &File, point: SyncPoint) -> std::io::Result<()> {
            self.syncs.push(point);
            if self.fail_sync == Some(point) {
                return Err(std::io::Error::other("controlled sync failure"));
            }
            file.sync_all()
        }
    }

    fn fixture() -> TestResult<(
        tempfile::TempDir,
        tempfile::TempDir,
        std::path::PathBuf,
        FakeIdentity,
    )> {
        let cache = tempfile::tempdir()?;
        std::fs::set_permissions(cache.path(), std::fs::Permissions::from_mode(0o700))?;
        let source_root = tempfile::tempdir()?;
        let identity = FakeIdentity::new();
        let source = source_root.path().join("model.gguf");
        std::fs::write(&source, &identity.bytes)?;
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o600))?;
        Ok((cache, source_root, source, identity))
    }

    fn installed_dir(cache: &Path, identity: &FakeIdentity) -> std::path::PathBuf {
        cache
            .join("models")
            .join(identity.family())
            .join(identity.id())
            .join(&identity.sha256()[..12])
    }

    fn future_deadline() -> Instant {
        Instant::now() + Duration::from_secs(30)
    }

    fn import_test(
        cache_root: &Path,
        identity: &FakeIdentity,
        source: &Path,
        hooks: &mut TestHooks,
    ) -> Result<(), OfflineModelImportError> {
        import_identity(
            cache_root,
            identity,
            source,
            &SessionCancellation::new(),
            future_deadline(),
            hooks,
        )
        .map(|_| ())
    }

    struct TestPause {
        reached: Receiver<()>,
        resume: SyncSender<()>,
    }

    impl TestPause {
        fn wait(&self) -> TestResult {
            self.reached.recv_timeout(Duration::from_secs(5))?;
            Ok(())
        }

        fn release(&self) -> TestResult {
            self.resume.send(())?;
            Ok(())
        }
    }

    fn pause_callback() -> (Box<dyn FnMut() + Send>, TestPause) {
        let (reached_tx, reached_rx) = sync_channel(0);
        let (resume_tx, resume_rx) = sync_channel(0);
        let mut invoked = false;
        let callback = Box::new(move || {
            if invoked {
                return;
            }
            invoked = true;
            reached_tx.send(()).expect("pause reached receiver");
            resume_rx.recv().expect("pause resume sender");
        });
        (
            callback,
            TestPause {
                reached: reached_rx,
                resume: resume_tx,
            },
        )
    }

    fn staging_is_empty(cache: &Path) -> TestResult<bool> {
        let staging = cache.join("staging");
        if !staging.exists() {
            return Ok(true);
        }
        Ok(std::fs::read_dir(staging)?.next().is_none())
    }

    #[test]
    fn cancellation_and_expiry_before_mutation_leave_no_stage_or_destination() -> TestResult {
        let (cache, _source_root, source, identity) = fixture()?;
        let cancelled = SessionCancellation::new();
        cancelled.cancel();
        assert_eq!(
            import_identity(
                cache.path(),
                &identity,
                &source,
                &cancelled,
                future_deadline(),
                &mut TestHooks::default(),
            ),
            Err(OfflineModelImportError::CancelledBeforePublication)
        );
        assert!(staging_is_empty(cache.path())?);
        assert!(!installed_dir(cache.path(), &identity).exists());

        let active = SessionCancellation::new();
        assert_eq!(
            import_identity(
                cache.path(),
                &identity,
                &source,
                &active,
                Instant::now(),
                &mut TestHooks::default(),
            ),
            Err(OfflineModelImportError::TimedOutBeforePublication)
        );
        assert!(staging_is_empty(cache.path())?);
        assert!(!installed_dir(cache.path(), &identity).exists());
        Ok(())
    }

    #[test]
    fn cancellation_mid_copy_confirms_stage_cleanup() -> TestResult {
        let (cache, _source_root, source, identity) = fixture()?;
        let (callback, pause) = pause_callback();
        let cancellation = SessionCancellation::new();
        let thread_cancellation = cancellation.clone();
        let cache_path = cache.path().to_path_buf();
        let thread_cache = cache_path.clone();
        let thread_identity = identity.clone();
        let destination = installed_dir(cache.path(), &identity);
        let worker = std::thread::spawn(move || {
            let mut hooks = TestHooks {
                copy_chunk: Some(callback),
                ..TestHooks::default()
            };
            import_identity(
                &thread_cache,
                &thread_identity,
                &source,
                &thread_cancellation,
                future_deadline(),
                &mut hooks,
            )
        });
        pause.wait()?;
        cancellation.cancel();
        pause.release()?;
        assert_eq!(
            worker.join().expect("import thread"),
            Err(OfflineModelImportError::CancelledBeforePublication)
        );
        assert!(staging_is_empty(&cache_path)?);
        assert!(!destination.exists());
        Ok(())
    }

    #[test]
    fn deadline_expiry_mid_copy_confirms_stage_cleanup() -> TestResult {
        let (cache, _source_root, source, identity) = fixture()?;
        let (callback, pause) = pause_callback();
        let cancellation = SessionCancellation::new();
        let cache_path = cache.path().to_path_buf();
        let thread_cache = cache_path.clone();
        let thread_identity = identity.clone();
        let destination = installed_dir(cache.path(), &identity);
        let deadline = Instant::now() + Duration::from_millis(250);
        let worker = std::thread::spawn(move || {
            let mut hooks = TestHooks {
                copy_chunk: Some(callback),
                ..TestHooks::default()
            };
            import_identity(
                &thread_cache,
                &thread_identity,
                &source,
                &cancellation,
                deadline,
                &mut hooks,
            )
        });
        pause.wait()?;
        while Instant::now() < deadline {
            std::thread::yield_now();
        }
        pause.release()?;
        assert_eq!(
            worker.join().expect("import thread"),
            Err(OfflineModelImportError::TimedOutBeforePublication)
        );
        assert!(staging_is_empty(&cache_path)?);
        assert!(!destination.exists());
        Ok(())
    }

    #[test]
    fn cancellation_during_staged_reread_confirms_stage_cleanup() -> TestResult {
        let (cache, _source_root, source, identity) = fixture()?;
        let (callback, pause) = pause_callback();
        let cancellation = SessionCancellation::new();
        let thread_cancellation = cancellation.clone();
        let cache_path = cache.path().to_path_buf();
        let thread_cache = cache_path.clone();
        let thread_identity = identity.clone();
        let destination = installed_dir(cache.path(), &identity);
        let worker = std::thread::spawn(move || {
            let mut hooks = TestHooks {
                verify_chunk: Some(callback),
                ..TestHooks::default()
            };
            import_identity(
                &thread_cache,
                &thread_identity,
                &source,
                &thread_cancellation,
                future_deadline(),
                &mut hooks,
            )
        });
        pause.wait()?;
        cancellation.cancel();
        pause.release()?;
        assert_eq!(
            worker.join().expect("import thread"),
            Err(OfflineModelImportError::CancelledBeforePublication)
        );
        assert!(staging_is_empty(&cache_path)?);
        assert!(!destination.exists());
        Ok(())
    }

    #[test]
    fn final_prepublish_cancellation_cleans_and_never_commits() -> TestResult {
        let (cache, _source_root, source, identity) = fixture()?;
        let (callback, pause) = pause_callback();
        let cancellation = SessionCancellation::new();
        let thread_cancellation = cancellation.clone();
        let cache_path = cache.path().to_path_buf();
        let thread_cache = cache_path.clone();
        let thread_identity = identity.clone();
        let destination = installed_dir(cache.path(), &identity);
        let worker = std::thread::spawn(move || {
            let mut hooks = TestHooks {
                before_publish: Some(callback),
                ..TestHooks::default()
            };
            import_identity(
                &thread_cache,
                &thread_identity,
                &source,
                &thread_cancellation,
                future_deadline(),
                &mut hooks,
            )
        });
        pause.wait()?;
        cancellation.cancel();
        pause.release()?;
        assert_eq!(
            worker.join().expect("import thread"),
            Err(OfflineModelImportError::CancelledBeforePublication)
        );
        assert!(staging_is_empty(&cache_path)?);
        assert!(!destination.exists());
        Ok(())
    }

    #[test]
    fn cancellation_after_publication_retains_bytes_and_reports_unconfirmed() -> TestResult {
        let (cache, _source_root, source, identity) = fixture()?;
        let (callback, pause) = pause_callback();
        let cancellation = SessionCancellation::new();
        let thread_cancellation = cancellation.clone();
        let cache_path = cache.path().to_path_buf();
        let thread_cache = cache_path.clone();
        let thread_identity = identity.clone();
        let expected = identity.bytes.clone();
        let destination = installed_dir(cache.path(), &identity);
        let worker = std::thread::spawn(move || {
            let mut hooks = TestHooks {
                after_publish: Some(callback),
                ..TestHooks::default()
            };
            import_identity(
                &thread_cache,
                &thread_identity,
                &source,
                &thread_cancellation,
                future_deadline(),
                &mut hooks,
            )
        });
        pause.wait()?;
        cancellation.cancel();
        pause.release()?;
        assert_eq!(
            worker.join().expect("import thread"),
            Err(OfflineModelImportError::VerificationFailedAfterPublication)
        );
        assert_eq!(std::fs::read(destination.join("model.gguf"))?, expected);
        assert!(staging_is_empty(&cache_path)?);
        Ok(())
    }

    #[test]
    fn deadline_after_publication_retains_bytes_and_reports_unconfirmed() -> TestResult {
        let (cache, _source_root, source, identity) = fixture()?;
        let destination = installed_dir(cache.path(), &identity);
        let expected = identity.bytes.clone();
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut hooks = TestHooks {
            after_publish: Some(Box::new(move || {
                std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
            })),
            ..TestHooks::default()
        };
        assert_eq!(
            import_identity(
                cache.path(),
                &identity,
                &source,
                &SessionCancellation::new(),
                deadline,
                &mut hooks,
            ),
            Err(OfflineModelImportError::VerificationFailedAfterPublication)
        );
        assert_eq!(std::fs::read(destination.join("model.gguf"))?, expected);
        assert!(staging_is_empty(cache.path())?);
        Ok(())
    }

    #[test]
    fn failed_control_cleanup_is_not_reported_as_clean_cancellation() -> TestResult {
        let (cache, _source_root, source, identity) = fixture()?;
        let cancellation = SessionCancellation::new();
        let cancellation_at_publish = cancellation.clone();
        let mut hooks = TestHooks {
            fail_sync: Some(SyncPoint::StagingParentAfterRename),
            before_publish: Some(Box::new(move || cancellation_at_publish.cancel())),
            ..TestHooks::default()
        };
        assert_eq!(
            import_identity(
                cache.path(),
                &identity,
                &source,
                &cancellation,
                future_deadline(),
                &mut hooks,
            ),
            Err(OfflineModelImportError::CleanupFailedBeforePublication)
        );
        assert!(staging_is_empty(cache.path())?);
        assert!(!installed_dir(cache.path(), &identity).exists());
        Ok(())
    }

    #[test]
    fn failed_cleanup_after_deadline_is_not_reported_as_clean_timeout() -> TestResult {
        let (cache, _source_root, source, identity) = fixture()?;
        let deadline = Instant::now() + Duration::from_millis(250);
        let mut hooks = TestHooks {
            fail_sync: Some(SyncPoint::StagingParentAfterRename),
            before_publish: Some(Box::new(move || {
                while Instant::now() < deadline {
                    std::thread::yield_now();
                }
            })),
            ..TestHooks::default()
        };
        assert_eq!(
            import_identity(
                cache.path(),
                &identity,
                &source,
                &SessionCancellation::new(),
                deadline,
                &mut hooks,
            ),
            Err(OfflineModelImportError::CleanupFailedBeforePublication)
        );
        assert!(staging_is_empty(cache.path())?);
        assert!(!installed_dir(cache.path(), &identity).exists());
        Ok(())
    }

    #[test]
    fn cancellation_has_deterministic_priority_over_expiry() {
        let cancellation = SessionCancellation::new();
        cancellation.cancel();
        assert_eq!(
            check_control(&cancellation, Instant::now()),
            Err(OfflineModelImportError::CancelledBeforePublication)
        );
    }

    #[test]
    fn import_error_text_contains_no_caller_identity_or_path() {
        let canaries = ["MODEL_CANARY_7ca9", "/home/user/SOURCE_CANARY_21bd.gguf"];
        for error in [
            OfflineModelImportError::CancelledBeforePublication,
            OfflineModelImportError::TimedOutBeforePublication,
            OfflineModelImportError::CleanupFailedBeforePublication,
            OfflineModelImportError::DurabilityFailedAfterPublication,
            OfflineModelImportError::VerificationFailedAfterPublication,
        ] {
            let display = error.to_string();
            let debug = format!("{error:?}");
            for canary in canaries {
                assert!(!display.contains(canary));
                assert!(!debug.contains(canary));
            }
        }
    }

    #[test]
    fn fifo_source_is_rejected_without_waiting_for_a_writer() -> TestResult {
        let dir = tempfile::tempdir()?;
        let fifo = dir.path().join("source.fifo");
        let status = std::process::Command::new("mkfifo").arg(&fifo).status()?;
        assert!(
            status.success(),
            "mkfifo failed for controlled test fixture"
        );
        std::fs::set_permissions(&fifo, std::fs::Permissions::from_mode(0o600))?;

        let started = Instant::now();
        assert!(matches!(
            open_source(&fifo, 1, rustix::process::getuid().as_raw()),
            Err(OfflineModelImportError::SourceUnsafe)
        ));
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "FIFO source open blocked instead of failing closed"
        );
        Ok(())
    }

    #[test]
    fn complete_directory_import_is_private_and_durable() -> TestResult {
        let (cache, _source_root, source, identity) = fixture()?;
        let mut hooks = TestHooks::default();
        import_test(cache.path(), &identity, &source, &mut hooks)?;
        let directory = installed_dir(cache.path(), &identity);
        assert_eq!(std::fs::metadata(&directory)?.mode() & 0o777, 0o700);
        assert_eq!(
            std::fs::metadata(directory.join("model.gguf"))?.mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(directory.join("meta.json"))?.mode() & 0o777,
            0o600
        );
        assert!(hooks.syncs.contains(&SyncPoint::StagingParentAfterRename));
        assert!(hooks.syncs.contains(&SyncPoint::DestinationParent));
        assert_eq!(std::fs::read(directory.join("model.gguf"))?, identity.bytes);
        Ok(())
    }

    #[test]
    fn unsafe_existing_root_is_rejected_without_chmod_or_children() -> TestResult {
        let (cache, _source_root, source, identity) = fixture()?;
        std::fs::set_permissions(cache.path(), std::fs::Permissions::from_mode(0o755))?;
        let before = std::fs::metadata(cache.path())?.mode() & 0o777;
        assert_eq!(
            import_test(cache.path(), &identity, &source, &mut TestHooks::default()),
            Err(OfflineModelImportError::UnsafeCacheRoot)
        );
        assert_eq!(std::fs::metadata(cache.path())?.mode() & 0o777, before);
        assert!(!cache.path().join("staging").exists());
        Ok(())
    }

    #[test]
    fn digest_failure_removes_owned_staging_directory() -> TestResult {
        let (cache, _source_root, source, identity) = fixture()?;
        let mut changed = identity.bytes.clone();
        changed[7] ^= 0x55;
        std::fs::write(&source, changed)?;
        assert_eq!(
            import_test(cache.path(), &identity, &source, &mut TestHooks::default()),
            Err(OfflineModelImportError::BytesMismatch)
        );
        assert!(
            std::fs::read_dir(cache.path().join("staging"))?
                .next()
                .is_none()
        );
        Ok(())
    }

    #[test]
    fn cleanup_never_recursively_removes_an_adversarial_replacement() -> TestResult {
        let (cache, _source_root, source, identity) = fixture()?;
        let staging = cache.path().join("staging");
        let displaced = cache.path().join("displaced-stage");
        let staging_for_hook = staging.clone();
        let displaced_for_hook = displaced.clone();
        let source_for_hook = source.clone();
        let mut hooks = TestHooks {
            staged: Some(Box::new(move || {
                let stage = std::fs::read_dir(&staging_for_hook)
                    .expect("read staging")
                    .next()
                    .expect("owned stage")
                    .expect("stage entry")
                    .path();
                let stage_name = stage.file_name().expect("stage name").to_owned();
                std::fs::rename(&stage, &displaced_for_hook).expect("displace stage");
                let replacement = staging_for_hook.join(stage_name);
                std::fs::create_dir(&replacement).expect("replacement stage");
                std::fs::write(replacement.join("sentinel"), b"must remain").expect("sentinel");
                std::fs::OpenOptions::new()
                    .append(true)
                    .open(&source_for_hook)
                    .expect("open source")
                    .write_all(b"changed")
                    .expect("change source");
            })),
            ..TestHooks::default()
        };
        assert_eq!(
            import_test(cache.path(), &identity, &source, &mut hooks),
            Err(OfflineModelImportError::CleanupFailedBeforePublication)
        );
        let replacement = std::fs::read_dir(&staging)?
            .next()
            .expect("replacement remains")?
            .path();
        assert_eq!(std::fs::read(replacement.join("sentinel"))?, b"must remain");
        assert!(std::fs::read_dir(&displaced)?.next().is_none());
        Ok(())
    }

    #[test]
    fn destination_ancestor_swap_cannot_redirect_publication() -> TestResult {
        let (cache, _source_root, source, identity) = fixture()?;
        let id_parent = cache
            .path()
            .join("models")
            .join(identity.family())
            .join(identity.id());
        let moved = cache.path().join("descriptor-held-parent");
        let redirected = cache.path().join("redirected-parent");
        let moved_for_hook = moved.clone();
        let redirected_for_hook = redirected.clone();
        let id_parent_for_hook = id_parent.clone();
        let mut hooks = TestHooks {
            before_publish: Some(Box::new(move || {
                std::fs::rename(&id_parent_for_hook, &moved_for_hook).expect("move parent");
                std::fs::create_dir(&redirected_for_hook).expect("redirect target");
                std::fs::set_permissions(
                    &redirected_for_hook,
                    std::fs::Permissions::from_mode(0o700),
                )
                .expect("redirect mode");
                std::os::unix::fs::symlink(&redirected_for_hook, &id_parent_for_hook)
                    .expect("replacement symlink");
            })),
            ..TestHooks::default()
        };
        import_test(cache.path(), &identity, &source, &mut hooks)?;
        assert!(
            moved
                .join(&identity.sha256()[..12])
                .join("model.gguf")
                .is_file()
        );
        assert!(std::fs::read_dir(&redirected)?.next().is_none());
        Ok(())
    }

    #[test]
    fn destination_sync_failure_is_typed_and_never_reported_success() -> TestResult {
        let (cache, _source_root, source, identity) = fixture()?;
        let mut hooks = TestHooks {
            fail_sync: Some(SyncPoint::DestinationParent),
            ..TestHooks::default()
        };
        assert_eq!(
            import_test(cache.path(), &identity, &source, &mut hooks),
            Err(OfflineModelImportError::DurabilityFailedAfterPublication)
        );
        assert!(installed_dir(cache.path(), &identity).is_dir());
        assert!(staging_is_empty(cache.path())?);
        Ok(())
    }

    #[test]
    fn postpublication_verification_failure_retains_destination_and_ignores_cancellation()
    -> TestResult {
        let (cache, _source_root, source, identity) = fixture()?;
        let destination = installed_dir(cache.path(), &identity);
        let destination_for_hook = destination.clone();
        let cancellation = SessionCancellation::new();
        let cancel_after_publish = cancellation.clone();
        let mut hooks = TestHooks {
            after_publish: Some(Box::new(move || {
                cancel_after_publish.cancel();
                let model = destination_for_hook.join("model.gguf");
                let mut bytes = std::fs::read(&model).expect("published model");
                bytes[0] ^= 0x5a;
                std::fs::write(model, bytes).expect("controlled corruption");
            })),
            ..TestHooks::default()
        };
        assert_eq!(
            import_identity(
                cache.path(),
                &identity,
                &source,
                &cancellation,
                future_deadline(),
                &mut hooks,
            ),
            Err(OfflineModelImportError::VerificationFailedAfterPublication)
        );
        assert!(destination.is_dir());
        assert!(staging_is_empty(cache.path())?);
        Ok(())
    }

    #[test]
    fn stage_setup_sync_failure_is_explicitly_cleaned() -> TestResult {
        let (cache, _source_root, source, identity) = fixture()?;
        let mut hooks = TestHooks {
            fail_sync: Some(SyncPoint::CreatedDirectory),
            ..TestHooks::default()
        };
        assert_eq!(
            import_test(cache.path(), &identity, &source, &mut hooks),
            Err(OfflineModelImportError::DurabilityFailedBeforePublication)
        );
        assert!(staging_is_empty(cache.path())?);
        assert!(!installed_dir(cache.path(), &identity).exists());
        Ok(())
    }

    #[test]
    fn stage_open_failure_reports_unconfirmed_cleanup() -> TestResult {
        let (cache, _source_root, source, identity) = fixture()?;
        let staging = cache.path().join("staging");
        let staging_for_hook = staging.clone();
        let mut hooks = TestHooks {
            stage_created: Some(Box::new(move || {
                let stage = std::fs::read_dir(&staging_for_hook)
                    .expect("read staging")
                    .next()
                    .expect("created stage")
                    .expect("stage entry")
                    .path();
                std::fs::remove_dir(&stage).expect("remove empty stage");
                std::fs::write(&stage, b"controlled replacement").expect("replacement file");
            })),
            ..TestHooks::default()
        };
        assert_eq!(
            import_test(cache.path(), &identity, &source, &mut hooks),
            Err(OfflineModelImportError::CleanupFailedBeforePublication)
        );
        let replacement = std::fs::read_dir(&staging)?
            .next()
            .expect("replacement remains")?
            .path();
        assert_eq!(std::fs::read(replacement)?, b"controlled replacement");
        assert!(!installed_dir(cache.path(), &identity).exists());
        Ok(())
    }

    #[test]
    fn concurrent_imports_converge_only_after_sync_and_exact_verification() -> TestResult {
        let (cache, _source_root, source, identity) = fixture()?;
        let cache_left = cache.path().to_path_buf();
        let cache_right = cache.path().to_path_buf();
        let source_left = source.clone();
        let source_right = source;
        let left_identity = identity.clone();
        let right_identity = identity.clone();
        let left = std::thread::spawn(move || {
            import_identity(
                &cache_left,
                &left_identity,
                &source_left,
                &SessionCancellation::new(),
                future_deadline(),
                &mut ProductionHooks,
            )
        });
        let right = std::thread::spawn(move || {
            import_identity(
                &cache_right,
                &right_identity,
                &source_right,
                &SessionCancellation::new(),
                future_deadline(),
                &mut ProductionHooks,
            )
        });
        left.join().expect("left join")?;
        right.join().expect("right join")?;
        assert_eq!(
            std::fs::read(installed_dir(cache.path(), &identity).join("model.gguf"))?,
            identity.bytes
        );
        assert!(
            std::fs::read_dir(cache.path().join("staging"))?
                .next()
                .is_none()
        );
        Ok(())
    }

    #[test]
    fn unknown_bundled_id_is_redacted_before_source_io() -> TestResult {
        let cache = tempfile::tempdir()?;
        std::fs::set_permissions(cache.path(), std::fs::Permissions::from_mode(0o700))?;
        let store = ModelStore::new(
            cache.path(),
            Arc::new(NoNetworkTransport),
            Arc::new(crate::model_cache_policy::PosixCacheFilePolicy::new()),
        );
        let caller = "secret/caller/path";
        let error = import_bundled_model(
            &store,
            &BundledModelRegistry::load(),
            caller,
            Path::new("/missing/source"),
            &SessionCancellation::new(),
            future_deadline(),
        )
        .expect_err("unknown id");
        assert_eq!(error, OfflineModelImportError::UnknownModel);
        assert!(!error.to_string().contains(caller));
        assert!(!cache.path().join("staging").exists());
        Ok(())
    }
}
