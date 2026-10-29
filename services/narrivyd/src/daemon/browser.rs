//! The assistant's own browser: an optional Store pack holding Chromium's
//! headless shell (Chrome for Testing), which the `view_page` tool opens
//! pages in. It is installed only when the user asks, from the pinned pack
//! (every file checked against its manifest), beside the other runtime
//! packs. Before each use its manifest is checked against the catalog's pin
//! and the executable against the manifest. Without it, `view_page` uses an
//! installed Firefox.

#![forbid(unsafe_code)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use narrivy_model_manager::{CatalogRuntime, runtime_catalog};

/// The catalog's name for this kind of pack.
const RUNTIME: &str = "browser";
const MANIFEST: &str = "runtime-manifest.json";
const EXECUTABLE: &str = "chrome-headless-shell";

/// The browser pack this daemon knows.
#[must_use]
pub fn catalog() -> Option<CatalogRuntime> {
    runtime_catalog()
        .into_iter()
        .find(|runtime| runtime.runtime == RUNTIME)
}

/// Where the pack is installed, for a given models directory.
#[must_use]
pub fn root(models_dir: &Path) -> Option<PathBuf> {
    let runtime = catalog()?;
    super::openvino::runtimes_dir(models_dir).map(|dir| dir.join(runtime.id))
}

/// The models directory, as the assistant's worker finds it.
fn models_dir() -> Option<PathBuf> {
    let speech_model = PathBuf::from(std::env::var_os("NARRIVY_TRANSCRIBE_MODEL")?);
    super::model_selection::models_directory(&speech_model)
}

/// The executable last checked, with what it was when checked: hashing it
/// again on every page is avoided while the file is unchanged.
static CHECKED: Mutex<Option<(PathBuf, u64, Option<SystemTime>)>> = Mutex::new(None);

/// The installed browser, checked, ready to start; `None` when it is not
/// installed or does not match its pins.
#[must_use]
pub fn executable() -> Option<PathBuf> {
    let runtime = catalog()?;
    let root = root(&models_dir()?)?;
    let manifest_path = root.join(MANIFEST);
    let manifest_digest = narrivy_model_manager::store::sha256_file(&manifest_path).ok()?;
    if !manifest_digest.eq_ignore_ascii_case(&runtime.manifest_sha256) {
        return None;
    }
    let manifest = narrivy_asr_openvino::read_manifest(&manifest_path).ok()?;
    let listed = manifest.files.iter().find(|file| file.path == EXECUTABLE)?;
    let path = root.join(EXECUTABLE);
    let meta = std::fs::symlink_metadata(&path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let seen = (path.clone(), meta.len(), meta.modified().ok());
    let already = CHECKED
        .lock()
        .is_ok_and(|checked| checked.as_ref() == Some(&seen));
    if !already {
        narrivy_asr_openvino::verify_file(&path, listed).ok()?;
        if let Ok(mut checked) = CHECKED.lock() {
            *checked = Some(seen);
        }
    }
    // Installed packs are private files; the executable must also run.
    if meta.permissions().mode() & 0o100 == 0 {
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).ok()?;
    }
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalog_pins_the_browser_pack() {
        let runtime = catalog();
        assert!(
            runtime
                .as_ref()
                .is_some_and(|r| r.manifest_sha256.len() == 64 && r.size_bytes > 0)
        );
        let models = Path::new("/data/narrivy/models");
        assert!(root(models).is_some_and(|dir| dir.starts_with("/data/narrivy/runtimes")));
    }
}
