//! The voices voice mode speaks with: Store packs (`runtime: "voice"` in
//! the runtime catalog), installed only when the user asks, from the pinned
//! pack (every file checked against its manifest on install), into
//! `<data>/voices/<id>`. Before each voice-mode start the pack's manifest is
//! checked against the catalog's pin and every file against the manifest.
//!
//! Only Kokoro (English) is offered: sherpa-onnx 2 reads words from the
//! voice's word lists and no longer contains eSpeak NG (GPL-3.0), which the
//! Urdu Piper voice needs to turn letters into sounds.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use narrivy_model_manager::{CatalogRuntime, runtime_catalog};

/// The catalog's name for this kind of pack.
pub const RUNTIME: &str = "voice";
const MANIFEST: &str = "runtime-manifest.json";

/// The voice packs this daemon knows, from the live or built-in catalog.
#[must_use]
pub fn catalog() -> Vec<CatalogRuntime> {
    runtime_catalog()
        .into_iter()
        .filter(|runtime| runtime.runtime == RUNTIME)
        .collect()
}

/// Where voice packs are installed, for a given models directory.
#[must_use]
pub fn root(models_dir: &Path) -> Option<PathBuf> {
    models_dir.parent().map(|data| data.join("voices"))
}

/// Where `id` is installed (whether or not it is there).
#[must_use]
pub fn dir(models_dir: &Path, id: &str) -> Option<PathBuf> {
    root(models_dir).map(|root| root.join(id))
}

/// Whether `id`'s pack is on this computer (not yet checked).
#[must_use]
pub fn installed(models_dir: &Path, id: &str) -> bool {
    dir(models_dir, id).is_some_and(|dir| dir.join(MANIFEST).is_file())
}

/// The packs checked in full, with what their manifests were: checking
/// 300 MB again on every start is avoided while nothing changed.
static CHECKED: Mutex<Vec<(PathBuf, u64, Option<SystemTime>)>> = Mutex::new(Vec::new());

/// The Urdu voices' ids start with this (Piper's Urdu voice, which the
/// separate GPL Urdu speaker reads, D51).
pub const URDU_PREFIX: &str = "voice-piper-ur";

/// The installed English voice, checked against its pins; `None` when none
/// is installed or it does not match.
#[must_use]
pub fn english(models_dir: &Path) -> Option<PathBuf> {
    catalog()
        .into_iter()
        .filter(|runtime| runtime.id.contains("kokoro"))
        .find_map(|runtime| verified(models_dir, &runtime))
}

/// The installed Urdu voice, checked against its pins; `None` when none is
/// installed or it does not match.
#[must_use]
pub fn urdu(models_dir: &Path) -> Option<PathBuf> {
    catalog()
        .into_iter()
        .filter(|runtime| runtime.id.starts_with(URDU_PREFIX))
        .find_map(|runtime| verified(models_dir, &runtime))
}

fn verified(models_dir: &Path, runtime: &CatalogRuntime) -> Option<PathBuf> {
    let root = dir(models_dir, &runtime.id)?;
    let manifest_path = root.join(MANIFEST);
    let meta = std::fs::symlink_metadata(&manifest_path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let digest = narrivy_model_manager::store::sha256_file(&manifest_path).ok()?;
    if !digest.eq_ignore_ascii_case(&runtime.manifest_sha256) {
        return None;
    }
    let seen = (root.clone(), meta.len(), meta.modified().ok());
    if CHECKED.lock().is_ok_and(|checked| checked.contains(&seen)) {
        return Some(root);
    }
    let manifest = narrivy_asr_openvino::read_manifest(&manifest_path).ok()?;
    manifest.verify(&root).ok()?;
    if let Ok(mut checked) = CHECKED.lock() {
        checked.retain(|(path, _, _)| *path != seen.0);
        checked.push(seen);
    }
    Some(root)
}

/// Whether an English voice is installed (not yet checked): for the
/// voice-mode switch.
#[must_use]
pub fn any_installed(models_dir: &Path) -> bool {
    // The Urdu voice only adds Urdu: an English voice is what voice mode
    // needs.
    catalog()
        .iter()
        .filter(|runtime| !runtime.id.starts_with(URDU_PREFIX))
        .any(|runtime| installed(models_dir, &runtime.id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voices_install_beside_the_models_and_unknown_ones_are_not_found() {
        let models = Path::new("/data/narrivy/models");
        assert_eq!(
            dir(models, "voice-x"),
            Some(PathBuf::from("/data/narrivy/voices/voice-x"))
        );
        assert!(!installed(models, "voice-x"));
        assert!(english(Path::new("/nonexistent/narrivy/models")).is_none());
    }

    #[test]
    fn a_pack_whose_manifest_does_not_match_its_pin_is_not_used() -> std::io::Result<()> {
        let data = tempfile::tempdir()?;
        let models = data.path().join("models");
        let runtime = CatalogRuntime {
            id: "voice-kokoro-test".to_owned(),
            runtime: RUNTIME.to_owned(),
            name: "Kokoro".to_owned(),
            version: "1".to_owned(),
            platform: "any".to_owned(),
            size_bytes: 1,
            license: "Apache-2.0".to_owned(),
            source_repo: "narrivy/store".to_owned(),
            source_path: "voices/kokoro".to_owned(),
            source_revision: None,
            manifest_sha256: "0".repeat(64),
            manifest_size: 2,
            devices: vec!["CPU".to_owned()],
        };
        let pack = data.path().join("voices").join(&runtime.id);
        std::fs::create_dir_all(&pack)?;
        std::fs::write(pack.join(MANIFEST), b"{}")?;
        assert!(installed(&models, &runtime.id));
        assert!(verified(&models, &runtime).is_none());
        Ok(())
    }
}
