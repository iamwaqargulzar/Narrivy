//! The models Narrivy knows about, as one read-only list.
//!
//! Two sources feed it:
//! - the version-1 registry: single-file models the `transcribe.cpp` engine
//!   loads, each with its pinned source, size and (when verified) SHA-256;
//! - the published multi-file packs: models published to the project's
//!   Hugging Face organisation for an engine the registry cannot describe yet.
//!
//! Each comes compiled in, and, once the daemon has fetched it, as the live
//! copy published in the Store repository (`catalog/<file>`, see
//! [`install_live`]). A live entry replaces the built-in one with the same
//! id; built-in entries the live copy does not name stay listed, so a model
//! already installed never drops out of the list. Live entries carry the
//! same pins and pass the same checks as built-in ones: a download is
//! verified against them, never against anything fetched with the file.
//!
//! This module only describes models. Whether one is installed, active or
//! usable on this computer is the daemon's observation, not a catalog fact.

use std::path::Path;
use std::sync::RwLock;

use serde::Deserialize;

use crate::registry::ModelRegistry;

const PUBLISHED_PACKS_JSON: &str = include_str!("../seed/published-packs.json");
const PUBLISHED_RUNTIMES_JSON: &str = include_str!("../seed/published-runtimes.json");
const PUBLISHED_LANGUAGE_MODELS_JSON: &str = include_str!("../seed/published-language-models.json");

/// The catalog files the Store repository publishes under `catalog/`, in
/// the order [`install_live`] takes them.
pub const LIVE_FILES: [&str; 4] = [
    "registry.json",
    "published-packs.json",
    "published-language-models.json",
    "published-runtimes.json",
];

/// Largest live catalog file accepted, before it is parsed.
pub const MAX_LIVE_FILE_BYTES: usize = 1024 * 1024;
/// Most entries a live file may list.
const MAX_LIVE_ENTRIES: usize = 256;

/// The live copies in use: each file's text, once it passed its checks.
static LIVE: RwLock<[Option<String>; 4]> = RwLock::new([None, None, None, None]);

fn live(index: usize) -> Option<String> {
    LIVE.read().ok().and_then(|files| files[index].clone())
}

/// Check one live catalog file (`name` is one of [`LIVE_FILES`]) the way
/// the built-in one is read; `Ok` with its entry count.
///
/// # Errors
/// A content-free reason: unknown file, too large, unreadable, or too many
/// entries.
pub fn check_live(name: &str, text: &str) -> Result<usize, &'static str> {
    if text.len() > MAX_LIVE_FILE_BYTES {
        return Err("too_large");
    }
    let count = match name {
        "registry.json" => ModelRegistry::from_json_str(text, Path::new("<live>"))
            .map_err(|_| "unreadable")?
            .entries()
            .len(),
        "published-packs.json" => parse_packs(text).ok_or("unreadable")?.len(),
        "published-language-models.json" => parse_language_models(text).ok_or("unreadable")?.len(),
        "published-runtimes.json" => parse_runtimes(text).ok_or("unreadable")?.len(),
        _ => return Err("unknown_file"),
    };
    if count > MAX_LIVE_ENTRIES {
        return Err("too_many_entries");
    }
    Ok(count)
}

/// Use `files` (name, text) as the live catalog. Each file that passes
/// [`check_live`] replaces its live copy; one that fails leaves the copy in
/// use (or the built-in one) alone. Returns how many were taken.
pub fn install_live(files: &[(&str, String)]) -> usize {
    let Ok(mut live) = LIVE.write() else {
        return 0;
    };
    let mut taken = 0;
    for (name, text) in files {
        let Some(index) = LIVE_FILES.iter().position(|file| file == name) else {
            continue;
        };
        if check_live(name, text).is_ok() {
            live[index] = Some(text.clone());
            taken += 1;
        }
    }
    taken
}

/// Forget the live catalog (tests; a reset to the built-in list).
pub fn clear_live() {
    if let Ok(mut live) = LIVE.write() {
        *live = [None, None, None, None];
    }
}

/// `live` entries, then the `built_in` ones whose id `live` does not use.
fn merged<T>(live: Option<Vec<T>>, built_in: Vec<T>, id: impl Fn(&T) -> &str) -> Vec<T> {
    let Some(mut list) = live else {
        return built_in;
    };
    let extra: Vec<T> = built_in
        .into_iter()
        .filter(|entry| list.iter().all(|taken| id(taken) != id(entry)))
        .collect();
    list.extend(extra);
    list
}

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
    let live_registry = live(0)
        .and_then(|text| ModelRegistry::from_json_str(&text, Path::new("<live>")).ok())
        .map(|registry| registry_models(&registry));
    let mut models = merged(
        live_registry,
        registry_models(&ModelRegistry::seeded()),
        |model| model.id.as_str(),
    );
    let packs = merged(
        live(1).and_then(|text| parse_packs(&text)),
        parse_packs(PUBLISHED_PACKS_JSON).unwrap_or_default(),
        |pack| pack.id.as_str(),
    );
    models.extend(packs.into_iter().map(|pack| CatalogModel {
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
    // An id both sources use is listed once, the registry's.
    let mut seen = std::collections::HashSet::new();
    models.retain(|model| seen.insert(model.id.clone()));
    models
}

fn parse_packs(text: &str) -> Option<Vec<PublishedPack>> {
    serde_json::from_str::<PublishedPacksFile>(text)
        .ok()
        .filter(|file| file.version == 1)
        .map(|file| file.packs)
}

fn registry_models(registry: &ModelRegistry) -> Vec<CatalogModel> {
    registry
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
        .collect()
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
    merged(
        live(3).and_then(|text| parse_runtimes(&text)),
        parse_runtimes(PUBLISHED_RUNTIMES_JSON).unwrap_or_default(),
        |runtime| runtime.id.as_str(),
    )
}

fn parse_runtimes(text: &str) -> Option<Vec<CatalogRuntime>> {
    serde_json::from_str::<PublishedRuntimesFile>(text)
        .ok()
        .filter(|file| file.version == 1)
        .map(|file| file.runtimes)
}

/// A language model pack (D32): an OpenVINO GenAI model for clean-up or the
/// assistant, downloaded from Models → Language models and pinned by its
/// manifest like the speech packs.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogLanguageModel {
    pub id: String,
    pub name: String,
    pub publisher: String,
    /// Manifest tasks: `chat` (the assistant), `refine` (clean-up).
    pub tasks: Vec<String>,
    pub languages: Vec<String>,
    pub size_bytes: u64,
    pub license: String,
    pub source_repo: String,
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
struct PublishedLanguageModelsFile {
    version: u32,
    models: Vec<CatalogLanguageModel>,
}

/// The language models Models → Language models offers for download.
#[must_use]
pub fn language_model_catalog() -> Vec<CatalogLanguageModel> {
    merged(
        live(2).and_then(|text| parse_language_models(&text)),
        parse_language_models(PUBLISHED_LANGUAGE_MODELS_JSON).unwrap_or_default(),
        |model| model.id.as_str(),
    )
}

fn parse_language_models(text: &str) -> Option<Vec<CatalogLanguageModel>> {
    serde_json::from_str::<PublishedLanguageModelsFile>(text)
        .ok()
        .filter(|file| file.version == 1)
        .map(|file| file.models)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_models_are_pinned_store_packs() {
        let models = language_model_catalog();
        assert!(
            models
                .iter()
                .any(|model| model.tasks.iter().any(|t| t == "chat"))
        );
        assert!(
            models
                .iter()
                .any(|model| model.tasks.iter().any(|t| t == "refine"))
        );
        for model in &models {
            let pack = crate::PackTarget::for_language_model(model);
            assert!(pack.is_ok(), "{}", model.id);
            assert!(model.source_path.starts_with("models/llm/"), "{}", model.id);
        }
    }

    #[test]
    fn a_live_file_adds_and_replaces_entries_and_keeps_the_rest() -> Result<(), String> {
        // A live copy of the built-in language models with one more pack (a
        // renamed copy, so its pins are real) and one name changed.
        let mut file: serde_json::Value =
            serde_json::from_str(PUBLISHED_LANGUAGE_MODELS_JSON).map_err(|e| e.to_string())?;
        let models = file["models"].as_array_mut().ok_or("no models")?;
        let mut extra = models[0].clone();
        extra["id"] = "live-only-test-pack".into();
        models[0]["name"] = "Renamed live".into();
        let built_in_second = models.remove(1);
        models.push(extra);
        let text = file.to_string();
        assert_eq!(check_live("published-language-models.json", &text), Ok(2));
        assert_eq!(install_live(&[("published-language-models.json", text)]), 1);
        let listed = language_model_catalog();
        clear_live();
        let ids: Vec<&str> = listed.iter().map(|m| m.id.as_str()).collect();
        assert!(ids.contains(&"live-only-test-pack"));
        // The entry the live copy left out is still listed.
        assert!(ids.contains(&built_in_second["id"].as_str().ok_or("id")?));
        assert!(listed.iter().any(|m| m.name == "Renamed live"));
        assert_eq!(listed.len(), 3);
        Ok(())
    }

    #[test]
    fn the_store_additions_pass_the_live_checks_once_pinned() -> Result<(), String> {
        // What scripts/build_store_catalog.py publishes: the built-in list
        // plus the additions, pinned to the Store commit.
        let additions: serde_json::Value = serde_json::from_str(include_str!(
            "../../../packaging/store/catalog-additions.json"
        ))
        .map_err(|e| e.to_string())?;
        let mut file: serde_json::Value =
            serde_json::from_str(PUBLISHED_LANGUAGE_MODELS_JSON).map_err(|e| e.to_string())?;
        let models = file["models"].as_array_mut().ok_or("no models")?;
        for entry in additions["language_models"]
            .as_array()
            .ok_or("no additions")?
        {
            let mut entry = entry.clone();
            entry["source_revision"] = "a".repeat(40).into();
            models.push(entry);
        }
        let parsed = parse_language_models(&file.to_string()).ok_or("unreadable")?;
        assert_eq!(
            check_live("published-language-models.json", &file.to_string()),
            Ok(parsed.len())
        );
        for model in &parsed {
            assert!(
                crate::PackTarget::for_language_model(model).is_ok(),
                "{}",
                model.id
            );
        }
        Ok(())
    }

    #[test]
    fn a_live_file_that_fails_its_checks_is_not_used() {
        assert_eq!(check_live("published-packs.json", "{}"), Err("unreadable"));
        assert_eq!(check_live("other.json", "{}"), Err("unknown_file"));
        let huge = " ".repeat(MAX_LIVE_FILE_BYTES + 1);
        assert_eq!(check_live("registry.json", &huge), Err("too_large"));
        assert_eq!(
            install_live(&[("published-runtimes.json", "[]".to_owned())]),
            0
        );
        assert_eq!(
            runtime_catalog(),
            parse_runtimes(PUBLISHED_RUNTIMES_JSON).unwrap_or_default()
        );
    }

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
