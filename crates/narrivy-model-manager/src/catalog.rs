//! The models Narrivy knows about, as one read-only list.
//!
//! Two sources feed it, both compiled in:
//! - the version-1 registry: single-file models the `transcribe.cpp` engine
//!   loads, each with its pinned source, size and (when verified) SHA-256;
//! - the published multi-file packs: models published to the project's
//!   Hugging Face organisation for an engine the registry cannot describe yet.
//!
//! This module only describes models. Whether one is installed, active or
//! usable on this computer is the daemon's observation, not a catalog fact.

use serde::Deserialize;

use crate::registry::ModelRegistry;

const PUBLISHED_PACKS_JSON: &str = include_str!("../seed/published-packs.json");
const PUBLISHED_RUNTIMES_JSON: &str = include_str!("../seed/published-runtimes.json");

/// Engine name for registry entries.
pub const ENGINE_TRANSCRIBE_CPP: &str = "transcribe.cpp";
/// Engine name for OpenVINO IR packs (the OpenVINO add-on, D25).
pub const ENGINE_OPENVINO: &str = "openvino";

/// Device classes a `transcribe.cpp` model may be qualified on: the CPU
/// path, and the GPU path of the same engine.
const TRANSCRIBE_CPP_DEVICES: &[&str] = &["CPU", "GPU"];

/// One model as the catalog describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogModel {
    pub id: String,
    pub family: String,
    pub variant: String,
    /// Quantization, as the publisher names it (for example `Q8_0`, `INT8`).
    pub quant: String,
    /// Total size of the model's files.
    pub size_bytes: u64,
    /// Whole-file SHA-256 for a single-file model whose bytes were verified.
    pub sha256: Option<String>,
    /// File name of a single-file model; `None` for a multi-file pack.
    pub filename: Option<String>,
    pub license: String,
    pub engine: String,
    pub source_repo: String,
    /// Where the model sits inside `source_repo`: the file's path for a
    /// single-file model, the pack's folder for a pack (empty for a pack at
    /// the repository root). The Store repository is laid out by kind and
    /// runtime, for example `models/gguf/<pack>/` and `models/openvino/<pack>/`.
    pub source_path: String,
    /// `None` until the model is published at a pinned revision; until then
    /// a pack can only be installed from a local developer source.
    pub source_revision: Option<String>,
    pub languages: Vec<String>,
    pub streaming: bool,
    /// Device classes this model may be qualified on (`CPU`, `GPU`, `NPU`).
    /// Eligibility, not a claim that it works on this computer.
    pub devices: Vec<String>,
    /// True once this exact variant passed qualification.
    pub qualified: bool,
    /// For a pack of several files: the pinned digest and size of its
    /// manifest, which in turn pins every file.
    pub manifest_sha256: Option<String>,
    pub manifest_size: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PublishedPack {
    id: String,
    family: String,
    variant: String,
    quant: String,
    size_bytes: u64,
    license: String,
    engine: String,
    source_repo: String,
    #[serde(default)]
    source_path: String,
    source_revision: Option<String>,
    languages: Vec<String>,
    streaming: bool,
    devices: Vec<String>,
    #[serde(default)]
    manifest_sha256: Option<String>,
    #[serde(default)]
    manifest_size: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PublishedPacksFile {
    version: u32,
    packs: Vec<PublishedPack>,
}

/// Every model the catalog describes: registry entries first, then the
/// published multi-file packs. A pack file this build cannot parse
/// contributes nothing rather than failing the listing.
pub fn model_catalog() -> Vec<CatalogModel> {
    let registry = ModelRegistry::seeded();
    let mut models: Vec<CatalogModel> = registry
        .entries()
        .iter()
        .map(|entry| {
            let source = entry.sources.first();
            CatalogModel {
                id: entry.id.clone(),
                family: entry.family.clone(),
                variant: entry.variant.clone(),
                quant: entry.quant.clone(),
                size_bytes: entry.size_bytes,
                sha256: entry.sha256.clone(),
                // The source names the file's path in its repository; the
                // file keeps only its own name once installed.
                filename: source.and_then(|s| s.filename.rsplit('/').next().map(str::to_owned)),
                license: entry.license.clone(),
                engine: ENGINE_TRANSCRIBE_CPP.to_owned(),
                source_repo: source.map(|s| s.repo_or_url.clone()).unwrap_or_default(),
                source_path: source.map(|s| s.filename.clone()).unwrap_or_default(),
                source_revision: Some(entry.source_revision.clone()),
                languages: entry.capabilities.languages.clone(),
                streaming: entry.capabilities.streaming,
                devices: TRANSCRIBE_CPP_DEVICES
                    .iter()
                    .map(|d| (*d).to_owned())
                    .collect(),
                qualified: !entry.unqualified,
                manifest_sha256: None,
                manifest_size: None,
            }
        })
        .collect();
    if let Ok(file) = serde_json::from_str::<PublishedPacksFile>(PUBLISHED_PACKS_JSON) {
        if file.version == 1 {
            models.extend(file.packs.into_iter().map(|pack| CatalogModel {
                id: pack.id,
                family: pack.family,
                variant: pack.variant,
                quant: pack.quant,
                size_bytes: pack.size_bytes,
                sha256: None,
                filename: None,
                license: pack.license,
                engine: pack.engine,
                source_repo: pack.source_repo,
                source_path: pack.source_path,
                source_revision: pack.source_revision,
                languages: pack.languages,
                streaming: pack.streaming,
                devices: pack.devices,
                qualified: false,
                manifest_sha256: pack.manifest_sha256,
                manifest_size: pack.manifest_size,
            }));
        }
    }
    models
}

/// An accelerator runtime the Store can add (D25): a pack of libraries
/// pinned by its manifest.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogRuntime {
    pub id: String,
    pub runtime: String,
    pub name: String,
    pub version: String,
    pub platform: String,
    pub size_bytes: u64,
    pub license: String,
    pub source_repo: String,
    /// The pack's folder inside `source_repo`, for example
    /// `runtimes/openvino/2026.4-linux-x86_64`.
    #[serde(default)]
    pub source_path: String,
    /// `None` until the pack is published; until then it can only be
    /// installed from a local developer source.
    pub source_revision: Option<String>,
    pub manifest_sha256: String,
    pub manifest_size: u64,
    pub devices: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PublishedRuntimesFile {
    version: u32,
    runtimes: Vec<CatalogRuntime>,
}

/// The accelerator runtimes the Store offers.
#[must_use]
pub fn runtime_catalog() -> Vec<CatalogRuntime> {
    serde_json::from_str::<PublishedRuntimesFile>(PUBLISHED_RUNTIMES_JSON)
        .ok()
        .filter(|file| file.version == 1)
        .map(|file| file.runtimes)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalog_lists_registry_models_and_published_packs() {
        let models = model_catalog();
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        for expected in [
            "whisper-tiny.en-Q8_0",
            "whisper-large-v3-turbo-Q8_0",
            "whisper-large-v3-Q5_K_M",
            "parakeet-unified-en-0.6b-Q8_0",
            "whisper-large-v3-turbo-openvino-int8",
            "whisper-large-v3-openvino-int8",
        ] {
            assert!(ids.contains(&expected), "missing {expected}");
        }
        // Ids are unique across both sources.
        let mut unique = ids.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), ids.len());
    }

    #[test]
    fn a_published_pack_names_its_own_engine_and_pinned_source() {
        let models = model_catalog();
        let pack = models
            .iter()
            .find(|m| m.id == "whisper-large-v3-openvino-int8");
        assert!(pack.is_some_and(|m| {
            m.engine == "openvino"
                && m.quant == "INT8"
                && m.filename.is_none()
                && m.devices == ["CPU", "GPU", "NPU"]
                && m.source_repo == "narrivy/store"
                && m.source_path == "models/openvino/whisper-large-v3-int8"
                && !m.qualified
        }));
        let gguf = models
            .iter()
            .find(|m| m.id == "parakeet-unified-en-0.6b-Q8_0");
        assert!(gguf.is_some_and(|m| {
            m.engine == ENGINE_TRANSCRIBE_CPP
                && m.devices == ["CPU", "GPU"]
                && m.streaming
                && m.sha256.as_deref().is_some_and(|s| s.len() == 64)
                && m.source_repo == "narrivy/store"
                && m.source_path
                    == "models/gguf/parakeet-unified-en-0.6b-q8-0/parakeet-unified-en-0.6b-Q8_0.gguf"
                && m.filename.as_deref() == Some("parakeet-unified-en-0.6b-Q8_0.gguf")
        }));
    }
}
