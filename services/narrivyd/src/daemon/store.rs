//! The Store (D23, D25, T2.19): what can be added, whether it fits this
//! computer, and installing and removing it (`ListStore`, `DownloadModel`,
//! `RemoveStoreItem`).
//!
//! The Store lists accelerator runtimes (owner direction 2026-10-01): speech
//! models are found and installed on the Models screen only.
//! - Vulkan is built in. It is listed so the user sees it is already there
//!   (`source=built_in`), never installed or removed.
//! - The OpenVINO add-on installs from its pinned runtime pack.
//! - CUDA is planned (`source=planned`): listed with whether this computer
//!   has an NVIDIA GPU, not installable yet.
//!
//! `DownloadModel` and `RemoveStoreItem` still accept the OpenVINO model
//! packs, which the Models screen installs and removes. Everything is pinned
//! by the catalog and checked file by file on install (ADR-0017); the
//! daemon, not the client, decides every state and verdict shown.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

use narrivy_ipc::StoreItemDto;
use narrivy_model_manager::{CatalogRuntime, PackTarget};

use super::download::ModelDownloads;
use super::model_selection::{
    ModelEngine, ModelSelection, loaded_path, models_directory, switchable,
};
use super::openvino::{self, RuntimeState};
use super::policy::EngineConfig;

/// Where Store items are installed: the directory beside the loaded model.
pub fn models_dir(config: &EngineConfig, selection: &ModelSelection) -> Option<PathBuf> {
    if !switchable(config) {
        return None;
    }
    let current = selection.current();
    loaded_path(config, current.as_ref()).and_then(models_directory)
}

/// Where an item comes from, if anywhere.
pub enum Source {
    Published,
    Local(PathBuf),
}

fn source_of(pack: &PackTarget) -> Option<Source> {
    if let Some(local) = openvino::local_store()
        && local
            .join(&pack.source_path)
            .join(pack.manifest_name)
            .is_file()
    {
        return Some(Source::Local(local));
    }
    pack.base_url.is_some().then_some(Source::Published)
}

/// Whether a pack can be installed from somewhere right now.
pub fn installable(pack: &PackTarget) -> bool {
    source_of(pack).is_some()
}

fn source_token(pack: Option<&PackTarget>) -> &'static str {
    match pack.and_then(source_of) {
        Some(Source::Published) => "published",
        Some(Source::Local(_)) => "local",
        None => "unavailable",
    }
}

/// Whether this computer has an NPU the kernel published. Enumeration
/// only: the runtime's self-test decides whether it is used.
fn npu_present() -> bool {
    std::fs::read_dir("/dev/accel")
        .map(|entries| {
            entries
                .flatten()
                .any(|entry| entry.file_name().to_string_lossy().starts_with("accel"))
        })
        .unwrap_or(false)
}

/// Whether this computer has an NVIDIA GPU with its kernel driver loaded.
fn nvidia_present() -> bool {
    std::path::Path::new("/proc/driver/nvidia/version").is_file()
}

fn hardware_fit() -> (&'static str, &'static str) {
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        return ("does_not_fit", "platform_unsupported");
    }
    if npu_present() {
        ("fits", "npu_found")
    } else {
        ("fits", "cpu_only")
    }
}

/// Store ids of the runtimes that are not installed from a pack.
const VULKAN_ID: &str = "vulkan";
const CUDA_ID: &str = "cuda";

/// `gpu_in_use`: the loaded speech model runs on a GPU through Vulkan.
pub fn listing(
    config: &EngineConfig,
    selection: &ModelSelection,
    downloads: &ModelDownloads,
    gpu_in_use: bool,
) -> Vec<StoreItemDto> {
    let Some(models) = models_dir(config, selection) else {
        return Vec::new();
    };
    let installing = downloads.progress();
    let failure = downloads.failure();
    let runtime_state = openvino::runtime_state(&models);
    let current = selection.current();
    let openvino_in_use = current
        .as_ref()
        .is_some_and(|model| model.engine == ModelEngine::OpenVino);
    let (fit, fit_reason) = hardware_fit();
    // Built in: listed so the user sees it is already there.
    let mut items = vec![item(
        ItemFacts {
            id: VULKAN_ID,
            kind: "runtime",
            category: "vulkan",
            name: "Vulkan GPU",
            size_bytes: 0,
            license: "",
            devices: &["GPU".to_owned()],
            requires: None,
            fit: ("fits", "built_in"),
            source: "built_in",
        },
        if gpu_in_use { "in_use" } else { "installed" },
        None,
        None,
    )];

    if let Some(runtime) = openvino::catalog_runtime() {
        let pack = PackTarget::for_runtime(&runtime).ok();
        let state = match runtime_state {
            RuntimeState::Ready if openvino_in_use => "in_use",
            RuntimeState::Ready => "installed",
            RuntimeState::Missing | RuntimeState::Invalid(_) => "available",
        };
        items.push(item(
            ItemFacts {
                id: &runtime.id,
                kind: "runtime",
                category: &runtime.runtime,
                name: &runtime.name,
                size_bytes: runtime.size_bytes,
                license: &runtime.license,
                devices: &runtime.devices,
                requires: None,
                fit: (fit, fit_reason),
                source: source_token(pack.as_ref()),
            },
            state,
            installing.as_ref(),
            failure.as_ref(),
        ));
    }

    // The assistant's own browser (optional; Firefox is used without it).
    if let Some(runtime) = super::browser::catalog() {
        let pack = PackTarget::for_runtime(&runtime).ok();
        let installed = super::browser::root(&models)
            .is_some_and(|dir| dir.join("runtime-manifest.json").is_file());
        items.push(item(
            ItemFacts {
                id: &runtime.id,
                kind: "module",
                category: "agent_browser",
                name: &runtime.name,
                size_bytes: runtime.size_bytes,
                license: &runtime.license,
                devices: &runtime.devices,
                requires: None,
                fit: ("fits", "any_computer"),
                source: source_token(pack.as_ref()),
            },
            if installed { "installed" } else { "available" },
            installing.as_ref(),
            failure.as_ref(),
        ));
    }

    items.push(item(
        ItemFacts {
            id: CUDA_ID,
            kind: "runtime",
            category: "cuda",
            name: "NVIDIA CUDA",
            size_bytes: 0,
            license: "",
            devices: &["GPU".to_owned()],
            requires: None,
            fit: if nvidia_present() {
                ("fits", "nvidia_found")
            } else {
                ("does_not_fit", "no_nvidia")
            },
            source: "planned",
        },
        "available",
        None,
        None,
    ));
    let planned = PLANNED_MODULES
        .iter()
        .map(|module| ("module", module))
        .chain(
            PLANNED_EXTENSIONS
                .iter()
                .map(|extension| ("extension", extension)),
        );
    for (kind, module) in planned {
        items.push(item(
            ItemFacts {
                id: module.id,
                kind,
                category: module.category,
                name: module.name,
                size_bytes: 0,
                license: "",
                devices: &module
                    .devices
                    .iter()
                    .map(|device| (*device).to_owned())
                    .collect::<Vec<_>>(),
                requires: module.requires,
                fit: ("unknown", "planned"),
                source: "planned",
            },
            "available",
            None,
            None,
        ));
    }
    items
}

/// A module or extension the Store will offer, shown so the user knows it is
/// coming.
struct PlannedModule {
    id: &'static str,
    category: &'static str,
    name: &'static str,
    devices: &'static [&'static str],
    requires: Option<&'static str>,
}

/// The planned modules, from the module catalog (`spec/catalogs/modules.json`):
/// small language-model specialists that refine dictated text (D23, D26: one
/// shared model in the OpenVINO worker), and noise suppression after 1.0.
const PLANNED_MODULES: &[PlannedModule] = &[
    PlannedModule {
        id: "refine-llm-openvino",
        category: "refine",
        name: "Clean-up",
        devices: &["NPU", "GPU", "CPU"],
        requires: Some(narrivy_asr_openvino::RUNTIME_PACK_ID),
    },
    PlannedModule {
        id: "grammar-languagetool",
        category: "refine",
        name: "Grammar and style check",
        devices: &["CPU"],
        requires: None,
    },
    PlannedModule {
        id: "punctuation-neural",
        category: "refine",
        name: "Neural punctuation",
        devices: &["CPU"],
        requires: None,
    },
    PlannedModule {
        id: "translation-offline",
        category: "refine",
        name: "Offline translation",
        devices: &["NPU", "GPU", "CPU"],
        requires: Some(narrivy_asr_openvino::RUNTIME_PACK_ID),
    },
    PlannedModule {
        id: "denoise-openvino",
        category: "denoise",
        name: "Noise suppression on the NPU",
        devices: &["NPU", "GPU"],
        requires: Some(narrivy_asr_openvino::RUNTIME_PACK_ID),
    },
    PlannedModule {
        id: "denoise-deepfilternet",
        category: "denoise",
        name: "DeepFilterNet noise suppression",
        devices: &["CPU"],
        requires: None,
    },
];

/// The planned extensions (D33): what brings Narrivy into other apps or adds
/// a whole area, each turned on from the Store.
const PLANNED_EXTENSIONS: &[PlannedModule] = &[
    PlannedModule {
        id: "writing-assistant-browser",
        category: "browser",
        name: "Writing assistant for Chrome and Brave",
        devices: &["NPU", "GPU", "CPU"],
        requires: None,
    },
    PlannedModule {
        id: "ai-assistant",
        category: "assistant",
        name: "AI assistant",
        devices: &["NPU", "GPU", "CPU"],
        requires: Some(narrivy_asr_openvino::RUNTIME_PACK_ID),
    },
    PlannedModule {
        id: "meeting-transcription",
        category: "meetings",
        name: "Meeting transcription",
        devices: &["NPU", "GPU", "CPU"],
        requires: None,
    },
    PlannedModule {
        id: "voice-changer",
        category: "voice",
        name: "Voice changer",
        devices: &["NPU"],
        requires: Some(narrivy_asr_openvino::RUNTIME_PACK_ID),
    },
];

struct ItemFacts<'a> {
    id: &'a str,
    kind: &'a str,
    category: &'a str,
    name: &'a str,
    size_bytes: u64,
    license: &'a str,
    devices: &'a [String],
    requires: Option<&'a str>,
    fit: (&'a str, &'a str),
    source: &'a str,
}

fn item(
    facts: ItemFacts<'_>,
    state: &str,
    installing: Option<&(String, u64)>,
    failure: Option<&(String, &'static str)>,
) -> StoreItemDto {
    let (state, installed_bytes) = match installing {
        Some((id, bytes)) if id == facts.id => ("installing", Some(*bytes)),
        _ => (state, None),
    };
    let failure = failure
        .filter(|(id, _)| id == facts.id && state == "available")
        .map(|(_, code)| (*code).to_owned());
    StoreItemDto {
        id: facts.id.to_owned(),
        kind: facts.kind.to_owned(),
        category: facts.category.to_owned(),
        name: facts.name.to_owned(),
        size_bytes: facts.size_bytes,
        license: facts.license.to_owned(),
        devices: facts.devices.to_vec(),
        state: if failure.is_some() { "failed" } else { state }.to_owned(),
        installed_bytes,
        failure,
        fit: facts.fit.0.to_owned(),
        fit_reason: Some(facts.fit.1.to_owned()),
        source: facts.source.to_owned(),
        requires: facts.requires.map(str::to_owned),
    }
}

/// What installing `id` means: which pack, from where, into which
/// directory.
pub struct Install {
    pub pack: PackTarget,
    pub parent: PathBuf,
    pub local: Option<PathBuf>,
}

/// Resolve a Store id to its install, or a content-free refusal.
pub fn plan_install(models: &Path, id: &str) -> Result<Install, &'static str> {
    if let Some(runtime) = openvino::catalog_runtime().filter(|runtime| runtime.id == id) {
        return install_for(
            PackTarget::for_runtime(&runtime).map_err(|_| "not_installable")?,
            openvino::runtimes_dir(models).ok_or("not_available")?,
        );
    }
    if let Some(runtime) = super::browser::catalog().filter(|runtime| runtime.id == id) {
        return install_for(
            PackTarget::for_runtime(&runtime).map_err(|_| "not_installable")?,
            openvino::runtimes_dir(models).ok_or("not_available")?,
        );
    }
    let model = openvino::catalog_models()
        .into_iter()
        .find(|model| model.id == id)
        .ok_or("unknown_item")?;
    install_for(
        PackTarget::for_model(&model).map_err(|_| "not_installable")?,
        models.to_path_buf(),
    )
}

fn install_for(pack: PackTarget, parent: PathBuf) -> Result<Install, &'static str> {
    let local = match source_of(&pack) {
        Some(Source::Local(dir)) => Some(dir),
        Some(Source::Published) => None,
        None => return Err("not_published_yet"),
    };
    Ok(Install {
        pack,
        parent,
        local,
    })
}

/// Remove an installed item. Refused while it is in use.
pub fn remove(models: &Path, selection: &ModelSelection, id: &str) -> Result<(), &'static str> {
    let current = selection.current();
    let target =
        if openvino::catalog_runtime().is_some_and(|runtime: CatalogRuntime| runtime.id == id) {
            if current
                .as_ref()
                .is_some_and(|model| model.engine == ModelEngine::OpenVino)
            {
                return Err("in_use");
            }
            openvino::runtime_root(models).ok_or("not_available")?
        } else if super::browser::catalog().is_some_and(|runtime| runtime.id == id) {
            super::browser::root(models).ok_or("not_available")?
        } else if openvino::catalog_models()
            .iter()
            .any(|model| model.id == id)
        {
            if current.as_ref().is_some_and(|model| model.id == id) {
                return Err("in_use");
            }
            models.join(id)
        } else {
            return Err("unknown_item");
        };
    match std::fs::symlink_metadata(&target) {
        Ok(meta) if meta.is_dir() => {
            std::fs::remove_dir_all(&target).map_err(|_| "remove_failed")?;
            // A half-finished install of the same item goes too.
            let staging = PathBuf::from(format!("{}.partial", target.display()));
            if std::fs::symlink_metadata(&staging).is_ok_and(|meta| meta.is_dir()) {
                let _ = std::fs::remove_dir_all(staging);
            }
            Ok(())
        }
        Ok(_) => Err("remove_failed"),
        Err(_) => Err("not_installed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_and_planned_items_cannot_be_installed_or_removed() {
        let dir = std::path::Path::new("/nonexistent/narrivy/models");
        let ids = [VULKAN_ID, CUDA_ID]
            .into_iter()
            .chain(PLANNED_MODULES.iter().map(|module| module.id))
            .chain(PLANNED_EXTENSIONS.iter().map(|extension| extension.id));
        for id in ids {
            assert_eq!(plan_install(dir, id).err(), Some("unknown_item"), "{id}");
            assert!(
                narrivy_model_manager::model_catalog()
                    .iter()
                    .all(|model| model.id != id),
                "{id} collides with a model"
            );
        }
    }

    #[test]
    fn unknown_items_are_refused_and_published_ones_have_a_source() {
        let dir = std::path::Path::new("/nonexistent/narrivy/models");
        assert_eq!(plan_install(dir, "nope").err(), Some("unknown_item"));
        // The runtime is published, so it can be installed without a
        // developer source.
        let runtime = openvino::catalog_runtime().and_then(|r| PackTarget::for_runtime(&r).ok());
        assert!(runtime.as_ref().is_some_and(installable));
    }

    #[test]
    fn a_failure_is_shown_only_on_its_own_item() {
        let facts = |id| ItemFacts {
            id,
            kind: "model",
            category: "speech",
            name: "x",
            size_bytes: 1,
            license: "MIT",
            devices: &[],
            requires: None,
            fit: ("fits", "cpu_only"),
            source: "published",
        };
        let failed = ("a".to_owned(), "integrity_mismatch");
        assert_eq!(
            item(facts("a"), "available", None, Some(&failed)).state,
            "failed"
        );
        assert_eq!(
            item(facts("b"), "available", None, Some(&failed)).state,
            "available"
        );
        let installing = ("b".to_owned(), 42);
        let shown = item(facts("b"), "available", Some(&installing), None);
        assert_eq!(
            (shown.state.as_str(), shown.installed_bytes),
            ("installing", Some(42))
        );
    }
}
