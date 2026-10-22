//! The OpenVINO add-on (D25, T2.16): where its runtime pack and model packs
//! live, whether they are installed and intact, and how its worker starts.
//!
//! The daemon never loads OpenVINO. It starts `narrivy-openvino-worker`
//! (bound by digest, like the transcribe.cpp worker) and tells it where the
//! runtime pack, the model pack and the compiled-model cache are. Packs sit
//! beside the models directory:
//!
//! ```text
//! <data>/models/<model id>/model-manifest.json …   OpenVINO model packs
//! <data>/runtimes/<runtime id>/runtime-manifest.json, lib/…
//! <data>/openvino-cache/<runtime version>/          compiled NPU/GPU blobs
//! ```

#![forbid(unsafe_code)]

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use narrivy_asr_openvino::{MODEL_MANIFEST, RUNTIME_MANIFEST, RUNTIME_PACK_ID, RUNTIME_VERSION};
use narrivy_model_manager::{CatalogModel, CatalogRuntime, model_catalog, runtime_catalog};
use narrivy_process_linux::VerifiedArtifactExecutable;

pub const WORKER_ENV: &str = "NARRIVY_OPENVINO_WORKER";
pub const WORKER_SHA256_ENV: &str = "NARRIVY_OPENVINO_WORKER_SHA256";
pub const WORKER_SIZE_ENV: &str = "NARRIVY_OPENVINO_WORKER_SIZE";
/// Developer source for Store installs: a directory laid out like the
/// `narrivy/store` repository (`models/openvino/<pack>/`, `runtimes/…`). Files from it are checked
/// against the same pins as downloads.
pub const LOCAL_STORE_ENV: &str = "NARRIVY_STORE_LOCAL_DIR";

/// Whether the runtime pack is ready to use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeState {
    Missing,
    Ready,
    /// Present but not what the catalog pins (damaged, partly removed).
    Invalid(&'static str),
}

impl RuntimeState {
    /// `runtime.openvino=installed`, `missing` or `invalid:<code>`.
    pub fn capability(self) -> String {
        match self {
            Self::Missing => "runtime.openvino=missing".to_owned(),
            Self::Ready => "runtime.openvino=installed".to_owned(),
            Self::Invalid(code) => format!("runtime.openvino=invalid:{code}"),
        }
    }
}

/// The catalog entry of the runtime pack this daemon's worker is built for.
pub fn catalog_runtime() -> Option<CatalogRuntime> {
    runtime_catalog()
        .into_iter()
        .find(|runtime| runtime.id == RUNTIME_PACK_ID)
}

/// Where installed runtime packs live, for a given models directory.
pub fn runtimes_dir(models_dir: &Path) -> Option<PathBuf> {
    models_dir.parent().map(|data| data.join("runtimes"))
}

pub fn runtime_root(models_dir: &Path) -> Option<PathBuf> {
    runtimes_dir(models_dir).map(|dir| dir.join(RUNTIME_PACK_ID))
}

/// The compiled-model cache for this runtime version.
pub fn cache_dir(models_dir: &Path) -> Option<PathBuf> {
    models_dir
        .parent()
        .map(|data| data.join("openvino-cache").join(RUNTIME_VERSION))
}

/// Check the installed runtime pack: its manifest must be the pinned one
/// and every listed file present at its listed size. Contents were checked
/// file by file when the pack was installed; a size check here is cheap
/// enough for every status report.
pub fn runtime_state(models_dir: &Path) -> RuntimeState {
    let (Some(root), Some(pin)) = (runtime_root(models_dir), catalog_runtime()) else {
        return RuntimeState::Missing;
    };
    let manifest_path = root.join(RUNTIME_MANIFEST);
    if std::fs::symlink_metadata(&manifest_path).is_err() {
        return RuntimeState::Missing;
    }
    if !digest_matches(&manifest_path, &pin.manifest_sha256, pin.manifest_size) {
        return RuntimeState::Invalid("manifest_mismatch");
    }
    let Ok(manifest) = narrivy_asr_openvino::read_manifest(&manifest_path) else {
        return RuntimeState::Invalid("manifest_invalid");
    };
    let complete = manifest.files.iter().all(|file| {
        std::fs::symlink_metadata(root.join(&file.path))
            .is_ok_and(|meta| meta.is_file() && meta.len() == file.size_bytes)
    });
    if complete {
        RuntimeState::Ready
    } else {
        RuntimeState::Invalid("files_missing")
    }
}

/// An installed OpenVINO model pack, as the worker binds it: the path,
/// pinned digest and size of its manifest.
pub fn installed_model(models_dir: &Path, model: &CatalogModel) -> Option<(PathBuf, String, u64)> {
    let (Some(sha256), Some(size)) = (&model.manifest_sha256, model.manifest_size) else {
        return None;
    };
    let manifest = models_dir.join(&model.id).join(MODEL_MANIFEST);
    digest_matches(&manifest, sha256, size).then(|| (manifest, sha256.to_ascii_lowercase(), size))
}

/// Whether an OpenVINO model pack directory is present at all, verified or
/// not (for listing it as installed before it is checked at load).
pub fn model_present(models_dir: &Path, id: &str) -> bool {
    std::fs::symlink_metadata(models_dir.join(id).join(MODEL_MANIFEST))
        .is_ok_and(|meta| meta.is_file())
}

/// The OpenVINO model packs in the catalog.
pub fn catalog_models() -> Vec<CatalogModel> {
    model_catalog()
        .into_iter()
        .filter(|model| model.engine == narrivy_model_manager::ENGINE_OPENVINO)
        .collect()
}

fn digest_matches(path: &Path, sha256: &str, size: u64) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file() && meta.len() == size)
        && narrivy_model_manager::store::sha256_file(path)
            .is_ok_and(|digest| digest.eq_ignore_ascii_case(sha256))
}

/// The worker executable, bound by the digest and size its environment
/// names (a controlled developer build, like the transcribe.cpp worker).
pub fn worker() -> Result<VerifiedArtifactExecutable, &'static str> {
    let path = std::env::var_os(WORKER_ENV).ok_or("openvino_worker_missing")?;
    let digest = std::env::var(WORKER_SHA256_ENV).map_err(|_| "openvino_worker_missing")?;
    let size = std::env::var(WORKER_SIZE_ENV)
        .ok()
        .and_then(|size| size.parse::<u64>().ok())
        .ok_or("openvino_worker_missing")?;
    VerifiedArtifactExecutable::bind_controlled(Path::new(&path), &digest, size)
        .map_err(|_| "openvino_worker_invalid")
}

/// Whether this daemon was given an OpenVINO worker at all.
pub fn worker_configured() -> bool {
    std::env::var_os(WORKER_ENV).is_some()
}

/// The arguments the worker takes before the common ones.
pub fn leading_args(models_dir: &Path, model_dir: &Path) -> Option<Vec<OsString>> {
    let runtime = runtime_root(models_dir)?;
    let cache = cache_dir(models_dir)?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .create(&cache)
        .ok()?;
    Some(vec![
        "--runtime".into(),
        runtime.into_os_string(),
        "--model-dir".into(),
        model_dir.as_os_str().to_owned(),
        "--cache-dir".into(),
        cache.into_os_string(),
    ])
}

/// The developer source for Store installs, when one is configured.
pub fn local_store() -> Option<PathBuf> {
    std::env::var_os(LOCAL_STORE_ENV)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute() && path.is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_sit_beside_the_models_directory() {
        let models = Path::new("/data/narrivy/models");
        assert_eq!(
            runtime_root(models),
            Some(PathBuf::from(format!(
                "/data/narrivy/runtimes/{RUNTIME_PACK_ID}"
            )))
        );
        assert_eq!(
            cache_dir(models),
            Some(PathBuf::from(format!(
                "/data/narrivy/openvino-cache/{RUNTIME_VERSION}"
            )))
        );
        assert!(catalog_runtime().is_some());
        assert!(!catalog_models().is_empty());
    }

    #[test]
    fn a_missing_or_altered_runtime_is_reported_as_such() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = tempfile::tempdir()?;
        let models = dir.path().join("models");
        std::fs::create_dir(&models)?;
        assert_eq!(runtime_state(&models), RuntimeState::Missing);
        let root = runtime_root(&models).ok_or("no root")?;
        std::fs::create_dir_all(&root)?;
        std::fs::write(root.join(RUNTIME_MANIFEST), b"{}")?;
        assert_eq!(
            runtime_state(&models),
            RuntimeState::Invalid("manifest_mismatch")
        );
        assert_eq!(
            RuntimeState::Invalid("manifest_mismatch").capability(),
            "runtime.openvino=invalid:manifest_mismatch"
        );
        Ok(())
    }
}
