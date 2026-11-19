//! Per-user daemon directories on Windows (the counterpart of
//! `narrivy_platform_linux::instance`).
//!
//! Everything lives under `%LOCALAPPDATA%\Narrivy`: the profile's local
//! (non-roaming) application data, whose inherited ACL already limits it to
//! the user, SYSTEM and administrators. Durable state goes in `state`, the
//! recovery Inbox and seeded pipelines in `run`. Single-instance ownership
//! is not a lock file here: the per-user pipe is created with
//! `FILE_FLAG_FIRST_PIPE_INSTANCE` (`narrivy-ipc-windows`), so the second
//! daemon fails to bind.

#![forbid(unsafe_code)]

use std::ffi::OsString;
use std::fmt;
use std::path::{Component, Path, PathBuf};

const APP_DIR: &str = "Narrivy";
const STATE_DIR: &str = "state";
const RUNTIME_DIR: &str = "run";

/// Why the per-user directories cannot be used.
#[derive(Debug)]
pub enum StateDirError {
    InvalidEnvironment(&'static str),
    Io(std::io::Error),
}

impl fmt::Display for StateDirError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEnvironment(name) => write!(f, "{name} is not a usable absolute path"),
            Self::Io(error) => write!(f, "state directory: {error}"),
        }
    }
}

impl std::error::Error for StateDirError {}

/// `%LOCALAPPDATA%\Narrivy\state`, created if missing.
pub fn state_dir() -> Result<PathBuf, StateDirError> {
    ensure(app_dir(std::env::var_os("LOCALAPPDATA"))?.join(STATE_DIR))
}

/// `%LOCALAPPDATA%\Narrivy\run`, created if missing: the recovery Inbox and
/// the seeded pipeline definitions (what `$XDG_RUNTIME_DIR/narrivy` holds on
/// Linux, but kept across logons, as the Inbox must be).
pub fn runtime_dir() -> std::io::Result<PathBuf> {
    let base = app_dir(std::env::var_os("LOCALAPPDATA")).map_err(|error| match error {
        StateDirError::Io(error) => error,
        other => std::io::Error::new(std::io::ErrorKind::NotFound, other.to_string()),
    })?;
    let dir = base.join(RUNTIME_DIR);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn app_dir(local_app_data: Option<OsString>) -> Result<PathBuf, StateDirError> {
    let Some(base) = local_app_data.filter(|value| !value.is_empty()) else {
        return Err(StateDirError::InvalidEnvironment("LOCALAPPDATA"));
    };
    let base = PathBuf::from(base);
    if !is_plain_absolute(&base) {
        return Err(StateDirError::InvalidEnvironment("LOCALAPPDATA"));
    }
    Ok(base.join(APP_DIR))
}

/// Absolute with no `.`/`..` parts. On Linux (where the unit tests run) a
/// Windows drive path is not absolute, so a `/`-rooted path stands in.
fn is_plain_absolute(path: &Path) -> bool {
    path.is_absolute()
        && !path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
}

fn ensure(dir: PathBuf) -> Result<PathBuf, StateDirError> {
    std::fs::create_dir_all(&dir).map_err(StateDirError::Io)?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_or_empty_local_app_data_is_refused() {
        assert!(matches!(
            app_dir(None),
            Err(StateDirError::InvalidEnvironment("LOCALAPPDATA"))
        ));
        assert!(matches!(
            app_dir(Some(OsString::new())),
            Err(StateDirError::InvalidEnvironment("LOCALAPPDATA"))
        ));
    }

    #[test]
    fn relative_or_dotted_local_app_data_is_refused() {
        for bad in ["relative\\dir", "relative/dir"] {
            assert!(app_dir(Some(bad.into())).is_err(), "{bad}");
        }
        let root = std::env::temp_dir();
        let dotted = root.join("a").join("..").join("b");
        assert!(app_dir(Some(dotted.into_os_string())).is_err());
    }

    #[test]
    fn app_dir_is_under_local_app_data() {
        let root = std::env::temp_dir();
        let dir = app_dir(Some(root.clone().into_os_string())).unwrap_or_default();
        assert_eq!(dir, root.join("Narrivy"));
    }
}
