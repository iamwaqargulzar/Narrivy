//! Language models on this computer (D32): what each is, what it can do
//! (clean-up, the assistant), and which one each job uses.
//!
//! Language-model packs live in the `llm` directory beside the speech
//! models; each has a `model-manifest.json` written by
//! `scripts/build_llm_pack.py`. The user's choice for each job is kept in
//! `llm/selection.json`. Until a choice exists, the developer variables
//! (`NARRIVY_REFINE_MODEL_DIR`, `NARRIVY_ASSISTANT_MODEL_DIR`) decide.

use std::path::{Path, PathBuf};

use narrivy_ipc::LanguageModelDto;

const SELECTION_FILE: &str = "selection.json";

/// What a language model is used for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    CleanUp,
    Assistant,
}

impl Role {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "clean_up" => Some(Self::CleanUp),
            "assistant" => Some(Self::Assistant),
            _ => None,
        }
    }

    const fn wire(self) -> &'static str {
        match self {
            Self::CleanUp => "clean_up",
            Self::Assistant => "assistant",
        }
    }

    /// The manifest task a model needs for this role.
    const fn task(self) -> &'static str {
        match self {
            Self::CleanUp => "refine",
            Self::Assistant => "chat",
        }
    }

    const fn env(self) -> &'static str {
        match self {
            Self::CleanUp => super::refine::MODEL_DIR_ENV,
            Self::Assistant => super::assistant::MODEL_DIR_ENV,
        }
    }
}

/// The user's choice for each job: a pack id, `""` for "none", or absent
/// when never chosen.
#[derive(Debug, Default)]
struct Selection {
    clean_up: Option<String>,
    assistant: Option<String>,
}

/// One installed pack, from its manifest.
#[derive(Debug, Clone)]
struct Pack {
    id: String,
    dir: PathBuf,
    name: String,
    publisher: String,
    size_bytes: u64,
    license: String,
    notice: Option<String>,
    tasks: Vec<String>,
    languages: Vec<String>,
}

/// Where language-model packs live: `llm` beside the speech models.
pub fn root() -> Option<PathBuf> {
    let models = super::model_selection::models_root_from_env()?;
    Some(models.parent()?.join("llm"))
}

fn scan(root: &Path) -> Vec<Pack> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut packs: Vec<Pack> = entries
        .filter_map(Result::ok)
        // A download in progress (`<id>.partial`) is not installed yet.
        .filter(|entry| !entry.file_name().to_string_lossy().ends_with(".partial"))
        .filter_map(|entry| read_pack(&entry.path()))
        .collect();
    packs.sort_by(|a, b| a.name.cmp(&b.name));
    packs
}

fn read_pack(dir: &Path) -> Option<Pack> {
    let manifest_path = dir.join(narrivy_asr_openvino::MODEL_MANIFEST);
    let metadata = std::fs::symlink_metadata(&manifest_path).ok()?;
    if !metadata.is_file() || metadata.len() > narrivy_asr_openvino::MAX_MANIFEST_BYTES {
        return None;
    }
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).ok()?).ok()?;
    let text = |pointer: &str| {
        manifest
            .pointer(pointer)
            .and_then(|v| v.as_str())
            .map(str::to_owned)
    };
    let list = |key: &str| -> Vec<String> {
        manifest
            .get(key)
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    let tasks = list("tasks");
    if !tasks.iter().any(|task| task == "refine" || task == "chat") {
        return None;
    }
    let size_bytes = manifest
        .get("files")
        .and_then(|v| v.as_array())
        .map(|files| {
            files
                .iter()
                .filter_map(|file| file.get("size_bytes").and_then(serde_json::Value::as_u64))
                .sum()
        })
        .unwrap_or(0);
    Some(Pack {
        id: text("/id")?,
        dir: dir.to_path_buf(),
        name: text("/display_name")?,
        publisher: text("/source/publisher").unwrap_or_default(),
        size_bytes,
        license: text("/license/spdx_or_name").unwrap_or_default(),
        notice: text("/license/notice"),
        tasks,
        languages: list("languages"),
    })
}

fn read_selection(root: &Path) -> Selection {
    let value: serde_json::Value = std::fs::read(root.join(SELECTION_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let field = |key: &str| value.get(key).and_then(|v| v.as_str()).map(str::to_owned);
    Selection {
        clean_up: field("clean_up"),
        assistant: field("assistant"),
    }
}

/// The directory of the model selected for `role`, if any.
pub fn selected_dir(role: Role) -> Option<PathBuf> {
    let root = root()?;
    let selection = read_selection(&root);
    let chosen = match role {
        Role::CleanUp => selection.clean_up,
        Role::Assistant => selection.assistant,
    };
    match chosen {
        // Chosen: that pack, or nothing.
        Some(id) => scan(&root)
            .into_iter()
            .find(|pack| pack.id == id && pack.tasks.iter().any(|t| t == role.task()))
            .map(|pack| pack.dir),
        // Never chosen: the developer variable.
        None => std::env::var_os(role.env())
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute() && dir.is_dir()),
    }
}

/// The directory of the installed pack `id` under `root`, if any.
pub fn installed_dir_under(root: &Path, id: &str) -> Option<PathBuf> {
    scan(root)
        .into_iter()
        .find(|pack| pack.id == id)
        .map(|pack| pack.dir)
}

/// Whether clean-up or the assistant runs the pack in `dir` now.
pub fn in_use(dir: &Path) -> bool {
    super::refine::current_dir().as_deref() == Some(dir)
        || super::assistant::current_dir().as_deref() == Some(dir)
}

/// Recommended packs by id (D31: never shipped, suggested first).
fn recommended(id: &str) -> &'static [&'static str] {
    match id {
        "s1-mini-openvino" => &["clean_up"],
        "qwen3.5-4b-openvino" => &["assistant"],
        _ => &[],
    }
}

/// Every language model on this computer and what it does, then the
/// catalog's models that can be downloaded.
pub fn list(downloads: &super::download::ModelDownloads) -> Vec<LanguageModelDto> {
    let Some(root) = root() else {
        return Vec::new();
    };
    let clean_up = super::refine::current_dir();
    let assistant = super::assistant::current_dir();
    let assistant_state = super::assistant::current_state();
    let clean_up_state = super::refine::shared().map(|refiner| match refiner.state() {
        super::refine::RefineState::Loading => "loading".to_owned(),
        super::refine::RefineState::Ready(device) => format!("ready:{device}"),
        super::refine::RefineState::Failed(code) => format!("failed:{code}"),
    });
    let installed = scan(&root);
    let available: Vec<LanguageModelDto> = narrivy_model_manager::language_model_catalog()
        .into_iter()
        .filter(|model| installed.iter().all(|pack| pack.id != model.id))
        .map(|model| downloadable(model, downloads))
        .collect();
    installed
        .into_iter()
        .map(|pack| {
            let mut in_use_for = Vec::new();
            if clean_up.as_deref() == Some(pack.dir.as_path()) {
                in_use_for.push("clean_up".to_owned());
            }
            if assistant.as_deref() == Some(pack.dir.as_path()) {
                in_use_for.push("assistant".to_owned());
            }
            let roles = [Role::CleanUp, Role::Assistant]
                .into_iter()
                .filter(|role| pack.tasks.iter().any(|t| t == role.task()))
                .map(|role| role.wire().to_owned())
                .collect();
            LanguageModelDto {
                clean_up_state: in_use_for
                    .iter()
                    .any(|role| role == "clean_up")
                    .then(|| clean_up_state.clone())
                    .flatten(),
                assistant_state: in_use_for
                    .iter()
                    .any(|role| role == "assistant")
                    .then(|| assistant_state.clone())
                    .flatten(),
                id: pack.id.clone(),
                name: pack.name,
                publisher: pack.publisher,
                size_bytes: pack.size_bytes,
                license: pack.license,
                license_notice: pack.notice,
                roles,
                languages: pack.languages,
                recommended_for: recommended(&pack.id)
                    .iter()
                    .map(|r| (*r).to_owned())
                    .collect(),
                in_use_for,
                installed: true,
                download: None,
                installed_bytes: None,
            }
        })
        .chain(available)
        .collect()
}

/// A catalog language model that is not installed, with its download state.
fn downloadable(
    model: narrivy_model_manager::CatalogLanguageModel,
    downloads: &super::download::ModelDownloads,
) -> LanguageModelDto {
    let installing = downloads.progress().filter(|(id, _)| *id == model.id);
    let failure = downloads.failure().filter(|(id, _)| *id == model.id);
    let published = narrivy_model_manager::PackTarget::for_language_model(&model)
        .is_ok_and(|pack| super::store::installable(&pack));
    let download = match (&installing, failure, published) {
        (Some(_), _, _) => "installing".to_owned(),
        (None, Some((_, code)), _) => format!("failed:{code}"),
        (None, None, true) => "available".to_owned(),
        (None, None, false) => "unpublished".to_owned(),
    };
    let roles = [Role::CleanUp, Role::Assistant]
        .into_iter()
        .filter(|role| model.tasks.iter().any(|t| t == role.task()))
        .map(|role| role.wire().to_owned())
        .collect();
    LanguageModelDto {
        recommended_for: recommended(&model.id)
            .iter()
            .map(|r| (*r).to_owned())
            .collect(),
        id: model.id,
        name: model.name,
        publisher: model.publisher,
        size_bytes: model.size_bytes,
        license: model.license,
        license_notice: None,
        roles,
        languages: model.languages,
        in_use_for: Vec::new(),
        clean_up_state: None,
        assistant_state: None,
        installed: false,
        download: Some(download),
        installed_bytes: installing.map(|(_, bytes)| bytes),
    }
}

/// Use pack `id` for `role`, or nothing. Returns the directory now in use.
pub fn select(role: Role, id: Option<&str>) -> Result<Option<PathBuf>, &'static str> {
    let root = root().ok_or("language_models_unavailable")?;
    let dir = match id {
        Some(id) => Some(
            scan(&root)
                .into_iter()
                .find(|pack| pack.id == id)
                .ok_or("unknown_model")
                .and_then(|pack| {
                    if pack.tasks.iter().any(|t| t == role.task()) {
                        Ok(pack.dir)
                    } else {
                        Err("model_cannot_do_this")
                    }
                })?,
        ),
        None => None,
    };
    let mut selection = read_selection(&root);
    let value = Some(id.unwrap_or_default().to_owned());
    match role {
        Role::CleanUp => selection.clean_up = value,
        Role::Assistant => selection.assistant = value,
    }
    let mut saved = serde_json::Map::new();
    if let Some(id) = selection.clean_up {
        saved.insert("clean_up".to_owned(), id.into());
    }
    if let Some(id) = selection.assistant {
        saved.insert("assistant".to_owned(), id.into());
    }
    let bytes = serde_json::to_vec_pretty(&serde_json::Value::Object(saved))
        .map_err(|_| "selection_not_saved")?;
    let temporary = root.join(format!("{SELECTION_FILE}.partial"));
    std::fs::write(&temporary, bytes)
        .and_then(|()| std::fs::rename(&temporary, root.join(SELECTION_FILE)))
        .map_err(|_| "selection_not_saved")?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_pack(root: &Path, name: &str, id: &str, tasks: &[&str]) -> std::io::Result<()> {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir)?;
        let manifest = serde_json::json!({
            "schema_version": 1,
            "id": id,
            "display_name": format!("{id} name"),
            "source": {"publisher": "Someone"},
            "license": {"spdx_or_name": "Apache-2.0", "notice": "Keep the name."},
            "tasks": tasks,
            "languages": ["en"],
            "files": [{"path": "a.bin", "size_bytes": 10}, {"path": "b.bin", "size_bytes": 5}],
        });
        std::fs::write(dir.join("model-manifest.json"), manifest.to_string())
    }

    #[test]
    fn packs_are_read_from_their_manifests_and_speech_packs_are_skipped() -> std::io::Result<()> {
        let root = tempfile::tempdir()?;
        write_pack(root.path(), "a", "clean-model", &["refine"])?;
        write_pack(root.path(), "b", "chat-model", &["chat", "refine"])?;
        write_pack(root.path(), "c", "speech-model", &["transcribe"])?;
        // Still downloading: its manifest is there, its model files not yet.
        write_pack(root.path(), "d.partial", "arriving-model", &["chat"])?;
        let packs = scan(root.path());
        let ids: Vec<&str> = packs.iter().map(|pack| pack.id.as_str()).collect();
        assert_eq!(ids, ["chat-model", "clean-model"]);
        assert_eq!(packs[0].size_bytes, 15);
        assert_eq!(packs[0].notice.as_deref(), Some("Keep the name."));
        Ok(())
    }

    #[test]
    fn roles_follow_the_manifest_tasks() {
        assert_eq!(Role::parse("clean_up"), Some(Role::CleanUp));
        assert_eq!(Role::parse("assistant").map(Role::task), Some("chat"));
        assert_eq!(Role::parse("translate"), None);
        assert_eq!(recommended("s1-mini-openvino"), ["clean_up"]);
    }
}
