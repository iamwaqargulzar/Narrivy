//! Daemon process-wide single-instance ownership (ADR-0012).
//!
//! Unix socket validation, locking, stale-endpoint cleanup, and endpoint
//! lifetime belong exclusively to `narrivy-ipc-unix`. This module retains the
//! distinct daemon-lifetime instance lock and shared runtime-directory lookup.

use std::ffi::OsString;
use std::fs::{DirBuilder, File, OpenOptions};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

const PRIVATE_DIR_MODE: u32 = 0o700;

/// Resolve an explicitly isolated Narrivy runtime directory when supplied,
/// otherwise `$XDG_RUNTIME_DIR/narrivy` (creating it 0700), or fall back to
/// `/run/user/<uid>/narrivy`. `NARRIVY_RUNTIME_DIR` changes only Narrivy's
/// private socket/transient-runtime area; PipeWire remains on the user session
/// runtime. Durable recovery state is resolved separately by [`state_dir`].
pub fn runtime_dir() -> std::io::Result<PathBuf> {
    let dir = match std::env::var("NARRIVY_RUNTIME_DIR") {
        Ok(v) if !v.is_empty() => PathBuf::from(v),
        _ => {
            let base = match std::env::var("XDG_RUNTIME_DIR") {
                Ok(v) if !v.is_empty() => PathBuf::from(v),
                _ => {
                    let uid = current_uid();
                    PathBuf::from(format!("/run/user/{uid}"))
                }
            };
            base.join("narrivy")
        }
    };
    std::fs::create_dir_all(&dir)?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(PRIVATE_DIR_MODE))?;
    Ok(dir)
}

/// Failure to resolve or create Narrivy's durable state root.
#[derive(Debug)]
pub enum StateDirError {
    InvalidEnvironment(&'static str),
    Unsafe(&'static str),
    Io(std::io::Error),
}

impl std::fmt::Display for StateDirError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidEnvironment(name) => {
                write!(formatter, "{name} must name an absolute directory")
            }
            Self::Unsafe(reason) => write!(formatter, "unsafe state directory: {reason}"),
            Self::Io(error) => write!(formatter, "state directory I/O failed: {error}"),
        }
    }
}

impl std::error::Error for StateDirError {}

impl From<std::io::Error> for StateDirError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// Resolve and create the private durable state root used by the recovery
/// journal. The precedence follows ADR-0013: XDG state, then XDG data, then
/// the XDG data default below HOME.
pub fn state_dir() -> Result<PathBuf, StateDirError> {
    let base = resolve_state_base(
        std::env::var_os("XDG_STATE_HOME"),
        std::env::var_os("XDG_DATA_HOME"),
        std::env::var_os("HOME"),
    )?;
    ensure_state_dir(&base)
}

fn resolve_state_base(
    state_home: Option<OsString>,
    data_home: Option<OsString>,
    home: Option<OsString>,
) -> Result<PathBuf, StateDirError> {
    for (name, candidate, suffix) in [
        ("XDG_STATE_HOME", state_home, None),
        ("XDG_DATA_HOME", data_home, None),
        ("HOME", home, Some(Path::new(".local/share"))),
    ] {
        let Some(candidate) = candidate.filter(|value| !value.is_empty()) else {
            continue;
        };
        let path = PathBuf::from(candidate);
        if !path.is_absolute() {
            return Err(StateDirError::InvalidEnvironment(name));
        }
        if path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
        {
            return Err(StateDirError::InvalidEnvironment(name));
        }
        return Ok(match suffix {
            Some(suffix) => path.join(suffix),
            None => path,
        });
    }
    Err(StateDirError::InvalidEnvironment(
        "XDG_STATE_HOME, XDG_DATA_HOME, or HOME",
    ))
}

fn ensure_state_dir(base: &Path) -> Result<PathBuf, StateDirError> {
    if !base.is_absolute() {
        return Err(StateDirError::InvalidEnvironment("state base"));
    }
    ensure_owned_base(base)?;
    let narrivy = base.join("narrivy");
    match std::fs::symlink_metadata(&narrivy) {
        Ok(metadata) => validate_private_state_root(&metadata)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = DirBuilder::new();
            builder.mode(PRIVATE_DIR_MODE);
            builder.create(&narrivy)?;
            validate_private_state_root(&std::fs::symlink_metadata(&narrivy)?)?;
            sync_directory(base)?;
        }
        Err(error) => return Err(error.into()),
    }
    reject_symlink_components(&narrivy)?;
    Ok(narrivy)
}

fn ensure_owned_base(base: &Path) -> Result<(), StateDirError> {
    let mut components: Vec<_> = base.ancestors().collect();
    components.reverse();
    for component in components {
        match std::fs::symlink_metadata(component) {
            Ok(metadata) => validate_state_component(component, base, &metadata)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let parent = component
                    .parent()
                    .ok_or(StateDirError::Unsafe("state base has no parent"))?;
                let parent_metadata = std::fs::symlink_metadata(parent)?;
                if parent_metadata.uid() != current_uid() || parent_metadata.mode() & 0o022 != 0 {
                    return Err(StateDirError::Unsafe(
                        "missing state component has an unsafe parent",
                    ));
                }
                let mut builder = DirBuilder::new();
                builder.mode(PRIVATE_DIR_MODE);
                builder.create(component)?;
                validate_state_component(component, base, &std::fs::symlink_metadata(component)?)?;
                sync_directory(parent)?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn validate_state_component(
    component: &Path,
    base: &Path,
    metadata: &std::fs::Metadata,
) -> Result<(), StateDirError> {
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(StateDirError::Unsafe(
            "state path contains a non-directory or symlink",
        ));
    }
    if component == base {
        if metadata.uid() != current_uid() {
            return Err(StateDirError::Unsafe("state base owner mismatch"));
        }
        if metadata.mode() & 0o022 != 0 {
            return Err(StateDirError::Unsafe(
                "state base is writable by another user",
            ));
        }
    } else if metadata.mode() & 0o022 != 0 && metadata.mode() & 0o1000 == 0 {
        return Err(StateDirError::Unsafe("state ancestor permits replacement"));
    }
    Ok(())
}

fn validate_private_state_root(metadata: &std::fs::Metadata) -> Result<(), StateDirError> {
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(StateDirError::Unsafe(
            "Narrivy state root is not a directory",
        ));
    }
    if metadata.uid() != current_uid() {
        return Err(StateDirError::Unsafe("Narrivy state root owner mismatch"));
    }
    if metadata.mode() & 0o777 != PRIVATE_DIR_MODE {
        return Err(StateDirError::Unsafe("Narrivy state root mode is not 0700"));
    }
    Ok(())
}

fn reject_symlink_components(path: &Path) -> Result<(), StateDirError> {
    for component in path.ancestors() {
        let metadata = std::fs::symlink_metadata(component)?;
        if metadata.file_type().is_symlink() {
            return Err(StateDirError::Unsafe(
                "state path contains a symlink component",
            ));
        }
    }
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), StateDirError> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn current_uid() -> u32 {
    // SAFETY: getuid is async-signal-safe and cannot fail; no pointers involved.
    unsafe { libc::getuid() }
}

#[cfg(not(target_os = "linux"))]
fn current_uid() -> u32 {
    // Portable fallback for non-Linux unixes until platform adapters land.
    1000
}

/// Held for the daemon lifetime; dropping releases the lock.
pub struct InstanceLock {
    _file: File,
    path: PathBuf,
}

impl InstanceLock {
    /// Lock file location, for diagnostics (content-free).
    pub fn lock_path(&self) -> &Path {
        &self.path
    }
}

#[derive(Debug)]
pub enum InstanceError {
    AlreadyRunning { lock_path: PathBuf },
    Io(std::io::Error),
}

impl std::fmt::Display for InstanceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstanceError::AlreadyRunning { lock_path } => {
                write!(f, "another narrivyd holds {lock_path:?}")
            }
            InstanceError::Io(e) => write!(f, "instance lock io error: {e}"),
        }
    }
}

impl std::error::Error for InstanceError {}

#[cfg(target_os = "linux")]
mod ffi {
    use std::fs::File;
    use std::os::unix::io::AsRawFd;

    /// Try LOCK_EX|LOCK_NB on an open file. Returns Ok(true) when acquired.
    ///
    /// SAFETY: `fd` is a valid open file descriptor owned by the caller;
    /// flock does not move or retain it beyond the call.
    pub fn try_flock(fd: &File) -> bool {
        let rc = unsafe { libc::flock(fd.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        rc == 0
    }
}

#[cfg(not(target_os = "linux"))]
mod ffi {
    use std::fs::File;
    use std::os::unix::io::AsRawFd;
    pub fn try_flock(_fd: &File) -> bool {
        let _ = AsRawFd::as_raw_fd; // silence unused on stub platforms
        true
    }
}

/// Acquire the singleton lock or fail with `AlreadyRunning`.
pub fn acquire_instance_lock() -> Result<InstanceLock, InstanceError> {
    let dir = runtime_dir().map_err(InstanceError::Io)?;
    // Keep the daemon-lifetime ownership lock distinct from narrivy-ipc's
    // socket-adjacent `narrivyd.lock` lease.
    let path = dir.join("narrivyd.instance.lock");
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(InstanceError::Io)?;
    if ffi::try_flock(&file) {
        Ok(InstanceLock { _file: file, path })
    } else {
        Err(InstanceError::AlreadyRunning { lock_path: path })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn runtime_dir_is_created_with_safe_mode() -> Result<(), Box<dyn std::error::Error>> {
        let dir = runtime_dir()?;
        assert!(dir.is_dir());
        let md = std::fs::metadata(&dir)?;
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(md.permissions().mode() & 0o777, 0o700);
        Ok(())
    }

    #[test]
    fn second_lock_attempt_fails_while_first_held() {
        // Same process holds one lock via fd; acquiring again must conflict.
        let first = acquire_instance_lock();
        if first.is_err() {
            // An external narrivyd may be running on this host; the invariant
            // under test still holds for the success path below.
            return;
        }
        let second = acquire_instance_lock();
        assert!(matches!(second, Err(InstanceError::AlreadyRunning { .. })));
        drop(first);
    }

    #[test]
    fn state_base_resolution_obeys_xdg_precedence() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            resolve_state_base(
                Some("/state".into()),
                Some("/data".into()),
                Some("/home/u".into())
            )?,
            PathBuf::from("/state")
        );
        assert_eq!(
            resolve_state_base(None, Some("/data".into()), Some("/home/u".into()))?,
            PathBuf::from("/data")
        );
        assert_eq!(
            resolve_state_base(None, None, Some("/home/u".into()))?,
            PathBuf::from("/home/u/.local/share")
        );
        assert!(resolve_state_base(Some("relative".into()), None, None).is_err());
        assert!(resolve_state_base(Some("/safe/../escape".into()), None, None).is_err());
        assert!(resolve_state_base(None, None, None).is_err());
        Ok(())
    }

    #[test]
    fn state_root_is_private_and_existing_insecure_root_fails_closed()
    -> Result<(), Box<dyn std::error::Error>> {
        let outer = tempfile::tempdir()?;
        std::fs::set_permissions(outer.path(), std::fs::Permissions::from_mode(0o700))?;
        let base = outer.path().join("state");
        let root = ensure_state_dir(&base)?;
        assert_eq!(root, base.join("narrivy"));
        assert_eq!(
            std::fs::symlink_metadata(&root)?.permissions().mode() & 0o777,
            PRIVATE_DIR_MODE
        );

        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o750))?;
        assert!(matches!(
            ensure_state_dir(&base),
            Err(StateDirError::Unsafe("Narrivy state root mode is not 0700"))
        ));
        Ok(())
    }

    #[test]
    fn state_root_rejects_symlinked_base() -> Result<(), Box<dyn std::error::Error>> {
        let outer = tempfile::tempdir()?;
        std::fs::set_permissions(outer.path(), std::fs::Permissions::from_mode(0o700))?;
        let real = outer.path().join("real");
        std::fs::create_dir(&real)?;
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o700))?;
        let linked = outer.path().join("linked");
        symlink(&real, &linked)?;
        assert!(matches!(
            ensure_state_dir(&linked),
            Err(StateDirError::Unsafe(
                "state path contains a non-directory or symlink"
            ))
        ));
        Ok(())
    }
}
