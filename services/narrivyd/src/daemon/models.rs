//! The model listing served by `ListModels`: the catalog's models, each with
//! what this daemon observes about it on this computer.
//!
//! - `active`: the model this daemon loaded. In an installed composition that
//!   is the selected registry id; otherwise it is the loaded file, matched by
//!   the SHA-256 the daemon verified at load (or, for an entry with no pinned
//!   hash, by file name and size).
//! - `installed`: a file with the entry's name and size sits beside the
//!   loaded model. Its bytes are verified when it is loaded, not here.
//! - `loading`: the model is being loaded in place of the active one
//!   (`SelectModel`); the active one stays in use until that finishes.
//! - `downloading`: the model is being downloaded (`DownloadModel`), with
//!   the bytes on disk so far.
//! - `needs_engine`: the model's engine is not part of this daemon, or (for
//!   an OpenVINO pack) the OpenVINO add-on is not installed.
//! - `available`: published, not on this computer.
//!
//! Only content-free metadata is reported: no path leaves this module.

#![forbid(unsafe_code)]

use std::path::Path;

use narrivy_ipc::ModelDto;
use narrivy_model_manager::{CatalogModel, ENGINE_OPENVINO, ENGINE_TRANSCRIBE_CPP, model_catalog};

use super::download::ModelDownloads;
use super::model_selection::{ModelSelection, SelectedModel, models_directory, switchable};
use super::policy::{AsrEngineKind, EngineConfig};

/// Id reported for a loaded model the catalog does not describe.
const UNLISTED_ID: &str = "unlisted";

pub fn model_listing(
    config: &EngineConfig,
    selection: &ModelSelection,
    downloads: &ModelDownloads,
) -> Vec<ModelDto> {
    let current = selection.current();
    let loading = selection.loading_id();
    let downloading = downloads.progress();
    let loaded = LoadedModel::from_config(config, current.as_ref());
    // A model can be downloaded only where it could then be chosen.
    let directory = switchable(config)
        .then(|| {
            loaded
                .as_ref()
                .and_then(|loaded| models_directory(loaded.path?))
        })
        .flatten();
    let openvino_ready = directory.as_deref().is_some_and(|dir| {
        super::openvino::worker_configured()
            && super::openvino::runtime_state(dir) == super::openvino::RuntimeState::Ready
    });
    let mut any_active = false;
    let mut listing: Vec<ModelDto> = model_catalog()
        .iter()
        .map(|model| {
            let mut state = if model.engine == ENGINE_OPENVINO {
                openvino_state(
                    model,
                    current.as_ref(),
                    directory.as_deref(),
                    openvino_ready,
                )
            } else {
                state_of(model, loaded.as_ref())
            };
            any_active |= state == "active";
            if state == "installed" && loading.as_deref() == Some(model.id.as_str()) {
                state = "loading";
            }
            let mut listed = dto(model, state);
            // An installed add-on pack can be removed here (the Models
            // screen); the loaded one cannot.
            listed.removable = model.engine == ENGINE_OPENVINO && state == "installed";
            if state == "available"
                && directory.is_some()
                && let Ok(pack) = narrivy_model_manager::PackTarget::for_model(model)
                && super::store::installable(&pack)
            {
                listed.downloadable = true;
                if let Some((id, received)) = &downloading
                    && *id == model.id
                {
                    listed.state = "downloading".to_owned();
                    listed.downloaded_bytes = Some(*received);
                }
            } else if state == "available"
                && let Some(directory) = directory.as_deref()
                && let Ok(target) = narrivy_model_manager::DownloadTarget::for_model(model)
            {
                listed.downloadable = true;
                match &downloading {
                    Some((id, received)) if *id == model.id => {
                        listed.state = "downloading".to_owned();
                        listed.downloaded_bytes = Some(*received);
                    }
                    _ => {
                        // What an earlier, unfinished download left to resume.
                        let (_, part) = super::download::destination(directory, &target);
                        listed.downloaded_bytes = std::fs::metadata(part)
                            .ok()
                            .map(|metadata| metadata.len())
                            .filter(|bytes| *bytes > 0 && *bytes <= model.size_bytes);
                    }
                }
            }
            listed
        })
        .collect();
    if let (false, Some(loaded)) = (any_active, &loaded) {
        listing.insert(0, loaded.unlisted());
    }
    listing
}

struct LoadedModel<'a> {
    /// Selected registry id in an installed composition.
    registry_id: Option<&'a str>,
    path: Option<&'a Path>,
    sha256: Option<&'a str>,
    size: Option<u64>,
}

impl<'a> LoadedModel<'a> {
    /// The model in use: the one chosen with `SelectModel`, else the one
    /// the daemon was started with.
    fn from_config(config: &'a EngineConfig, current: Option<&'a SelectedModel>) -> Option<Self> {
        if config.asr_engine != AsrEngineKind::TranscribeCpp {
            return None;
        }
        if let Some(current) = current {
            return Some(Self {
                registry_id: None,
                path: Some(current.path.as_path()),
                sha256: Some(current.sha256.as_str()),
                size: Some(current.size),
            });
        }
        let loaded = Self {
            registry_id: config.transcribe_model_id.as_deref(),
            path: config.transcribe_model_path.as_deref(),
            sha256: config.transcribe_model_sha256.as_deref(),
            size: config.transcribe_model_size,
        };
        (loaded.registry_id.is_some() || loaded.path.is_some()).then_some(loaded)
    }

    fn file_name(&self) -> Option<&str> {
        self.path?.file_name()?.to_str()
    }

    fn is(&self, model: &CatalogModel) -> bool {
        if model.engine != ENGINE_TRANSCRIBE_CPP {
            return false;
        }
        if let Some(id) = self.registry_id {
            return id == model.id;
        }
        match (&model.sha256, self.sha256) {
            (Some(pinned), Some(loaded)) => pinned.eq_ignore_ascii_case(loaded),
            // No pinned hash for this entry: fall back to name and size.
            _ => {
                model.filename.as_deref() == self.file_name() && self.size == Some(model.size_bytes)
            }
        }
    }

    /// A loaded model outside the catalog: only its size is known.
    fn unlisted(&self) -> ModelDto {
        ModelDto {
            id: UNLISTED_ID.to_owned(),
            family: String::new(),
            variant: String::new(),
            quant: String::new(),
            size_bytes: self.size.unwrap_or_default(),
            license: String::new(),
            engine: ENGINE_TRANSCRIBE_CPP.to_owned(),
            languages: Vec::new(),
            streaming: false,
            devices: Vec::new(),
            source_repo: String::new(),
            source_revision: String::new(),
            qualified: false,
            state: "active".to_owned(),
            downloadable: false,
            removable: false,
            downloaded_bytes: None,
        }
    }
}

/// An OpenVINO pack: in use, installed, or available once the add-on is
/// installed.
fn openvino_state(
    model: &CatalogModel,
    current: Option<&SelectedModel>,
    directory: Option<&Path>,
    openvino_ready: bool,
) -> &'static str {
    if current.is_some_and(|current| current.id == model.id) {
        return "active";
    }
    if !openvino_ready {
        return "needs_engine";
    }
    if directory.is_some_and(|dir| super::openvino::model_present(dir, &model.id)) {
        "installed"
    } else {
        "available"
    }
}

fn state_of(model: &CatalogModel, loaded: Option<&LoadedModel<'_>>) -> &'static str {
    if model.engine != ENGINE_TRANSCRIBE_CPP {
        return "needs_engine";
    }
    let Some(loaded) = loaded else {
        return "available";
    };
    if loaded.is(model) {
        return "active";
    }
    let beside = loaded
        .path
        .and_then(models_directory)
        .zip(model.filename.as_deref());
    let present = beside.is_some_and(|(dir, name)| {
        std::fs::metadata(dir.join(name))
            .is_ok_and(|meta| meta.is_file() && meta.len() == model.size_bytes)
    });
    if present { "installed" } else { "available" }
}

fn dto(model: &CatalogModel, state: &str) -> ModelDto {
    ModelDto {
        id: model.id.clone(),
        family: model.family.clone(),
        variant: model.variant.clone(),
        quant: model.quant.clone(),
        size_bytes: model.size_bytes,
        license: model.license.clone(),
        engine: model.engine.clone(),
        languages: model.languages.clone(),
        streaming: model.streaming,
        devices: model.devices.clone(),
        source_repo: model.source_repo.clone(),
        source_revision: model.source_revision.clone().unwrap_or_default(),
        qualified: model.qualified,
        state: state.to_owned(),
        downloadable: false,
        removable: false,
        downloaded_bytes: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    const PARAKEET: &str = "parakeet-unified-en-0.6b-Q8_0";
    const TINY: &str = "whisper-tiny.en-Q8_0";

    fn state<'a>(listing: &'a [ModelDto], id: &str) -> Option<&'a str> {
        listing
            .iter()
            .find(|m| m.id == id)
            .map(|m| m.state.as_str())
    }

    fn catalog_model(id: &str) -> Result<CatalogModel, String> {
        model_catalog()
            .into_iter()
            .find(|m| m.id == id)
            .ok_or_else(|| format!("{id} is not in the catalog"))
    }

    fn controlled(path: &Path, sha256: &str, size: u64) -> EngineConfig {
        EngineConfig {
            asr_engine: AsrEngineKind::TranscribeCpp,
            transcribe_model_path: Some(path.to_path_buf()),
            transcribe_model_sha256: Some(sha256.to_owned()),
            transcribe_model_size: Some(size),
            ..EngineConfig::default()
        }
    }

    #[test]
    fn without_a_loaded_model_nothing_is_active_or_installed() {
        let listing = model_listing(
            &EngineConfig::default(),
            &ModelSelection::default(),
            &ModelDownloads::default(),
        );
        assert!(
            listing
                .iter()
                .all(|m| m.state == "available" || m.state == "needs_engine")
        );
        assert_eq!(
            state(&listing, "whisper-large-v3-openvino-int8"),
            Some("needs_engine")
        );
        assert_eq!(state(&listing, PARAKEET), Some("available"));
    }

    #[test]
    fn the_loaded_model_is_active_and_a_file_beside_it_is_installed() -> TestResult {
        let parakeet = catalog_model(PARAKEET)?;
        let tiny = catalog_model(TINY)?;
        let dir = tempfile::tempdir()?;
        // A sparse file with the catalog's name and size stands in for the
        // installed model; its bytes are not what this listing checks.
        let tiny_file =
            std::fs::File::create(dir.path().join(tiny.filename.as_deref().ok_or("name")?))?;
        tiny_file.set_len(tiny.size_bytes)?;
        let loaded = dir.path().join(parakeet.filename.as_deref().ok_or("name")?);
        let config = controlled(
            &loaded,
            parakeet.sha256.as_deref().ok_or("sha")?,
            parakeet.size_bytes,
        );

        let listing = model_listing(
            &config,
            &ModelSelection::default(),
            &ModelDownloads::default(),
        );
        assert_eq!(state(&listing, PARAKEET), Some("active"));
        assert_eq!(state(&listing, TINY), Some("installed"));
        assert_eq!(
            state(&listing, "whisper-large-v3-turbo-Q8_0"),
            Some("available")
        );
        assert_eq!(listing.iter().filter(|m| m.state == "active").count(), 1);
        assert!(listing.iter().all(|m| m.id != UNLISTED_ID));
        Ok(())
    }

    #[test]
    fn a_loaded_model_outside_the_catalog_is_listed_as_unlisted() -> TestResult {
        let dir = tempfile::tempdir()?;
        let config = controlled(&dir.path().join("my-own.gguf"), &"ab".repeat(32), 1234);
        let listing = model_listing(
            &config,
            &ModelSelection::default(),
            &ModelDownloads::default(),
        );
        assert_eq!(listing[0].id, UNLISTED_ID);
        assert_eq!(listing[0].state, "active");
        assert_eq!(listing[0].size_bytes, 1234);
        assert_eq!(listing.iter().filter(|m| m.state == "active").count(), 1);
        Ok(())
    }

    #[test]
    fn an_installed_composition_is_matched_by_its_registry_id() {
        let config = EngineConfig {
            asr_engine: AsrEngineKind::TranscribeCpp,
            transcribe_model_id: Some(TINY.to_owned()),
            ..EngineConfig::default()
        };
        let listing = model_listing(
            &config,
            &ModelSelection::default(),
            &ModelDownloads::default(),
        );
        assert_eq!(state(&listing, TINY), Some("active"));
    }
}
