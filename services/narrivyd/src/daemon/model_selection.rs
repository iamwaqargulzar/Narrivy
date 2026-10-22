//! Which model and device each pipeline recognises with, and which model
//! the daemon has loaded (`SetPipelineEngine`, `SelectModel`).
//!
//! One model is loaded at a time. Each pipeline may name the model it wants
//! and the device it should run on; a pipeline that names none uses whatever
//! is loaded. When a dictation starts, the daemon compares what the pipeline
//! wants with what is loaded and reloads only when they differ ([`plan`]).
//!
//! A model can be chosen when it is in the catalog with a pinned SHA-256
//! and its file sits beside the model the daemon was started with. A
//! built-in (transcribe.cpp) model is one file; an OpenVINO model is a pack
//! directory pinned by its manifest, and needs the OpenVINO runtime pack.
//! The bytes are verified against the pins when the model is loaded;
//! nothing here trusts a file by name.
//!
//! The new model is loaded first and the old one released afterwards, so a
//! failed load leaves the old model in use. The loaded model's id and every
//! pipeline's choice are kept in the private state directory and used again
//! at the next start.
//!
//! Only ids and content-free reason codes leave this module: no path does.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

#[cfg(feature = "asr-transcribe-resident")]
use narrivy_asr_transcribe::resident_protocol::ComputeRequest;

use narrivy_asr_api::resident_protocol::WorkerCompute;
use narrivy_model_manager::{ENGINE_OPENVINO, ENGINE_TRANSCRIBE_CPP, model_catalog};

use super::policy::{AsrEngineKind, EngineConfig};

const STORE_FILE: &str = "selected-model";
const PIPELINES_FILE: &str = "pipeline-engines";
/// A pipeline id in the store carries at most this many bytes.
const MAX_PIPELINE_ID_BYTES: usize = 64;

/// What one pipeline wants to recognise with.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PipelineEngine {
    /// A catalog model id; `None` uses whatever model is loaded.
    pub model: Option<String>,
    pub compute: ComputeChoice,
}

/// The device a pipeline asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ComputeChoice {
    /// A GPU that passes the self-test, else the CPU.
    #[default]
    Auto,
    Gpu,
    /// An NPU, through the OpenVINO add-on.
    Npu,
    Cpu,
}

impl ComputeChoice {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "gpu" => Some(Self::Gpu),
            "npu" => Some(Self::Npu),
            "cpu" => Some(Self::Cpu),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Gpu => "gpu",
            Self::Npu => "npu",
            Self::Cpu => "cpu",
        }
    }

    #[cfg(feature = "asr-transcribe-resident")]
    pub fn request(self) -> ComputeRequest {
        match self {
            Self::Auto => ComputeRequest::Auto,
            Self::Gpu => ComputeRequest::Gpu,
            Self::Npu => ComputeRequest::Npu,
            Self::Cpu => ComputeRequest::Cpu,
        }
    }

    /// Whether an engine that was asked for `loaded` and runs on `device`
    /// already satisfies this choice, so no reload is needed.
    fn satisfied_by(self, loaded: Self, device: LoadedDevice) -> bool {
        match self {
            // Automatic is satisfied by an automatic load wherever it
            // landed, and by anything already on an accelerator.
            Self::Auto => loaded == Self::Auto || device != LoadedDevice::Cpu,
            Self::Gpu => device == LoadedDevice::Gpu,
            Self::Npu => device == LoadedDevice::Npu,
            Self::Cpu => device == LoadedDevice::Cpu,
        }
    }
}

/// Where the loaded engine runs, as its worker attests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadedDevice {
    Cpu,
    Gpu,
    Npu,
}

impl From<bool> for LoadedDevice {
    /// `true` for an engine on a GPU.
    fn from(on_gpu: bool) -> Self {
        if on_gpu { Self::Gpu } else { Self::Cpu }
    }
}

impl From<WorkerCompute> for LoadedDevice {
    fn from(compute: WorkerCompute) -> Self {
        match compute {
            WorkerCompute::Cpu => Self::Cpu,
            WorkerCompute::IntegratedGpu | WorkerCompute::DiscreteGpu => Self::Gpu,
            WorkerCompute::Npu => Self::Npu,
        }
    }
}

/// Which engine runs a model.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ModelEngine {
    #[default]
    TranscribeCpp,
    /// `path` is the pack's manifest; the pack is its directory.
    OpenVino,
}

/// What starting a dictation with a pipeline requires.
#[derive(Debug, PartialEq, Eq)]
pub enum Plan {
    /// The loaded engine already is what the pipeline wants.
    Keep,
    Load {
        model: SelectedModel,
        compute: ComputeChoice,
    },
}

/// A catalog model resolved to a file on this computer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectedModel {
    pub id: String,
    pub path: PathBuf,
    pub sha256: String,
    pub size: u64,
    pub engine: ModelEngine,
}

#[derive(Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    Loading(String),
    Failed(&'static str),
}

/// Why a model cannot be chosen. Each maps to a fixed, content-free detail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectError {
    /// This daemon was not started with an explicit model beside which
    /// others can sit (for example the packaged composition).
    NotAvailable,
    UnknownModel,
    NeedsEngine,
    /// The catalog entry has no single file or no pinned hash to verify.
    NotVerifiable,
    NotInstalled,
}

impl SelectError {
    pub fn detail(self) -> &'static str {
        match self {
            Self::NotAvailable => "model selection is not available in this installation yet",
            Self::UnknownModel => "no such model",
            Self::NeedsEngine => "this model needs an engine that is not installed",
            Self::NotVerifiable => "this model cannot be verified before loading",
            Self::NotInstalled => "this model is not downloaded",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Resolution {
    AlreadyActive,
    Load(SelectedModel),
}

pub struct ModelSelection {
    /// The model in use when it is not the one from configuration.
    current: Mutex<Option<SelectedModel>>,
    /// The device the loaded engine was asked for.
    current_compute: Mutex<ComputeChoice>,
    pipelines: Mutex<BTreeMap<String, PipelineEngine>>,
    phase: Mutex<Phase>,
    store: OnceLock<PathBuf>,
}

impl Default for ModelSelection {
    fn default() -> Self {
        Self {
            current: Mutex::new(None),
            current_compute: Mutex::new(ComputeChoice::Auto),
            pipelines: Mutex::new(BTreeMap::new()),
            phase: Mutex::new(Phase::Idle),
            store: OnceLock::new(),
        }
    }
}

impl ModelSelection {
    /// Keep the loaded model's id and the pipelines' choices under
    /// `state_dir` from now on, and read back the pipelines' choices.
    pub fn persist_under(&self, state_dir: &Path) {
        let _ = self.store.set(state_dir.to_path_buf());
        if let Ok(text) = std::fs::read_to_string(state_dir.join(PIPELINES_FILE)) {
            *self
                .pipelines
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = parse_pipelines(&text);
        }
    }

    /// What `pipeline_id` wants; the default when it has chosen nothing.
    pub fn pipeline_engine(&self, pipeline_id: &str) -> PipelineEngine {
        self.pipelines
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(pipeline_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn set_pipeline_engine(&self, pipeline_id: &str, engine: PipelineEngine) {
        let mut pipelines = self
            .pipelines
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        pipelines.insert(pipeline_id.to_owned(), engine);
        if let Some(state_dir) = self.store.get() {
            write_store(
                &state_dir.join(PIPELINES_FILE),
                &render_pipelines(&pipelines),
            );
        }
    }

    /// The device the loaded engine was asked for.
    pub fn current_compute(&self) -> ComputeChoice {
        *self
            .current_compute
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Record the device the engine loaded at start-up was asked for.
    pub fn set_current_compute(&self, compute: ComputeChoice) {
        *self
            .current_compute
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = compute;
    }

    pub fn current(&self) -> Option<SelectedModel> {
        self.current
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Record the model restored at start-up as the one in use.
    pub fn set_current(&self, model: SelectedModel) {
        *self
            .current
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(model);
    }

    pub fn loading_id(&self) -> Option<String> {
        match &*self
            .phase
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
        {
            Phase::Loading(id) => Some(id.clone()),
            Phase::Idle | Phase::Failed(_) => None,
        }
    }

    /// Start loading `id`. Refused while another load is running.
    pub fn begin(&self, id: &str) -> bool {
        let mut phase = self
            .phase
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if matches!(*phase, Phase::Loading(_)) {
            return false;
        }
        *phase = Phase::Loading(id.to_owned());
        true
    }

    /// Finish the running load. Success makes the model current and keeps
    /// its id for the next start; failure keeps the previous model.
    pub fn finish(&self, outcome: Result<(SelectedModel, ComputeChoice), &'static str>) {
        let next = match outcome {
            Ok((model, compute)) => {
                if let Some(state_dir) = self.store.get() {
                    write_store(&state_dir.join(STORE_FILE), &model.id);
                }
                self.set_current(model);
                self.set_current_compute(compute);
                Phase::Idle
            }
            Err(code) => Phase::Failed(code),
        };
        *self
            .phase
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = next;
    }

    /// `model.selection=idle`, `loading` or `failed:<code>`.
    pub fn capability(&self) -> String {
        match &*self
            .phase
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
        {
            Phase::Idle => "model.selection=idle".to_owned(),
            Phase::Loading(_) => "model.selection=loading".to_owned(),
            Phase::Failed(code) => format!("model.selection=failed:{code}"),
        }
    }
}

/// The model the daemon has loaded: the chosen one, else the configured one.
pub fn loaded_path<'a>(
    config: &'a EngineConfig,
    current: Option<&'a SelectedModel>,
) -> Option<&'a Path> {
    current
        .map(|model| model.path.as_path())
        .or(config.transcribe_model_path.as_deref())
}

/// Whether this composition can switch models at all.
pub fn switchable(config: &EngineConfig) -> bool {
    config.asr_engine == AsrEngineKind::TranscribeCpp
        && config.transcribe_resident
        && !config.transcribe_installed
        && config.transcribe_model_path.is_some()
}

/// Resolve a catalog id to a verified-on-load file beside the loaded model.
pub fn resolve(
    config: &EngineConfig,
    current: Option<&SelectedModel>,
    id: &str,
) -> Result<Resolution, SelectError> {
    if !switchable(config) {
        return Err(SelectError::NotAvailable);
    }
    let model = model_catalog()
        .into_iter()
        .find(|model| model.id == id)
        .ok_or(SelectError::UnknownModel)?;
    if model.engine == ENGINE_OPENVINO {
        return resolve_openvino(config, current, model);
    }
    if model.engine != ENGINE_TRANSCRIBE_CPP {
        return Err(SelectError::NeedsEngine);
    }
    let (Some(filename), Some(sha256)) = (model.filename, model.sha256) else {
        return Err(SelectError::NotVerifiable);
    };
    let active = match current {
        Some(current) => current.id == id,
        None => config
            .transcribe_model_sha256
            .as_deref()
            .is_some_and(|loaded| loaded.eq_ignore_ascii_case(&sha256)),
    };
    if active {
        return Ok(Resolution::AlreadyActive);
    }
    let directory = loaded_path(config, current)
        .and_then(models_directory)
        .ok_or(SelectError::NotAvailable)?;
    let path = directory.join(filename);
    let present =
        std::fs::metadata(&path).is_ok_and(|meta| meta.is_file() && meta.len() == model.size_bytes);
    if !present {
        return Err(SelectError::NotInstalled);
    }
    Ok(Resolution::Load(SelectedModel {
        id: model.id,
        path,
        sha256,
        size: model.size_bytes,
        engine: ModelEngine::TranscribeCpp,
    }))
}

/// An OpenVINO model pack: needs the runtime pack and the worker, and the
/// pack's manifest must be the pinned one.
fn resolve_openvino(
    config: &EngineConfig,
    current: Option<&SelectedModel>,
    model: narrivy_model_manager::CatalogModel,
) -> Result<Resolution, SelectError> {
    use super::openvino;

    if current.is_some_and(|current| current.id == model.id) {
        return Ok(Resolution::AlreadyActive);
    }
    let directory = loaded_path(config, current)
        .and_then(models_directory)
        .ok_or(SelectError::NotAvailable)?;
    if !openvino::worker_configured()
        || openvino::runtime_state(&directory) != openvino::RuntimeState::Ready
    {
        return Err(SelectError::NeedsEngine);
    }
    let (path, sha256, size) =
        openvino::installed_model(&directory, &model).ok_or(SelectError::NotInstalled)?;
    Ok(Resolution::Load(SelectedModel {
        id: model.id,
        path,
        sha256,
        size,
        engine: ModelEngine::OpenVino,
    }))
}

/// The models directory for a loaded model's path: its parent, or for an
/// OpenVINO pack (whose path is the manifest inside the pack) the parent of
/// the pack.
pub fn models_directory(loaded: &Path) -> Option<PathBuf> {
    let parent = loaded.parent()?;
    if loaded.file_name() == Some(std::ffi::OsStr::new(narrivy_asr_openvino::MODEL_MANIFEST)) {
        parent.parent().map(Path::to_path_buf)
    } else {
        Some(parent.to_path_buf())
    }
}

/// The model the daemon has loaded, as a catalog entry when it is one.
fn loaded_model(config: &EngineConfig, current: Option<&SelectedModel>) -> Option<SelectedModel> {
    if let Some(current) = current {
        return Some(current.clone());
    }
    let sha256 = config.transcribe_model_sha256.clone()?;
    let id = model_catalog()
        .into_iter()
        .find(|model| {
            model
                .sha256
                .as_deref()
                .is_some_and(|pin| pin.eq_ignore_ascii_case(&sha256))
        })
        .map_or_else(|| "unlisted".to_owned(), |model| model.id);
    Some(SelectedModel {
        id,
        path: config.transcribe_model_path.clone()?,
        sha256,
        size: config.transcribe_model_size?,
        engine: ModelEngine::TranscribeCpp,
    })
}

/// Decide whether starting `pipeline_id` needs another engine than the one
/// loaded. `device` is where the loaded engine runs. A pipeline whose
/// model cannot be resolved any more (deleted, renamed) keeps the loaded
/// model rather than refusing to dictate.
pub fn plan(
    config: &EngineConfig,
    selection: &ModelSelection,
    pipeline_id: &str,
    device: impl Into<LoadedDevice>,
) -> Plan {
    let device = device.into();
    if !switchable(config) {
        return Plan::Keep;
    }
    let wanted = selection.pipeline_engine(pipeline_id);
    let current = selection.current();
    let other_model =
        wanted
            .model
            .as_deref()
            .and_then(|id| match resolve(config, current.as_ref(), id) {
                Ok(Resolution::Load(model)) => Some(model),
                Ok(Resolution::AlreadyActive) | Err(_) => None,
            });
    if let Some(model) = other_model {
        return Plan::Load {
            model,
            compute: wanted.compute,
        };
    }
    if wanted
        .compute
        .satisfied_by(selection.current_compute(), device)
    {
        return Plan::Keep;
    }
    match loaded_model(config, current.as_ref()) {
        Some(model) => Plan::Load {
            model,
            compute: wanted.compute,
        },
        None => Plan::Keep,
    }
}

/// `pipeline<TAB>model-or-dash<TAB>compute`, one line per pipeline.
fn render_pipelines(pipelines: &BTreeMap<String, PipelineEngine>) -> String {
    pipelines
        .iter()
        .map(|(id, engine)| {
            format!(
                "{id}\t{}\t{}\n",
                engine.model.as_deref().unwrap_or("-"),
                engine.compute.as_str()
            )
        })
        .collect()
}

/// Read the store back. A malformed line is skipped: that pipeline then
/// uses the default, which is always safe.
fn parse_pipelines(text: &str) -> BTreeMap<String, PipelineEngine> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let (id, model, compute) = (fields.next()?, fields.next()?, fields.next()?);
            if fields.next().is_some()
                || id.is_empty()
                || id.len() > MAX_PIPELINE_ID_BYTES
                || model.is_empty()
                || model.len() > narrivy_ipc::MAX_MODEL_ID_BYTES
            {
                return None;
            }
            Some((
                id.to_owned(),
                PipelineEngine {
                    model: (model != "-").then(|| model.to_owned()),
                    compute: ComputeChoice::parse(compute)?,
                },
            ))
        })
        .collect()
}

/// The model chosen before the last shutdown, if it can still be chosen.
pub fn restore(state_dir: &Path, config: &EngineConfig) -> Option<SelectedModel> {
    let id = std::fs::read_to_string(state_dir.join(STORE_FILE)).ok()?;
    match resolve(config, None, id.trim()) {
        Ok(Resolution::Load(model)) => Some(model),
        Ok(Resolution::AlreadyActive) | Err(_) => None,
    }
}

/// Replace the stored id atomically. A failure only means the choice is not
/// remembered across a restart; the model in use is unaffected.
fn write_store(store: &Path, id: &str) {
    let temporary = store.with_extension("tmp");
    if std::fs::write(&temporary, id).is_ok() && std::fs::rename(&temporary, store).is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    const PARAKEET: &str = "parakeet-unified-en-0.6b-Q8_0";
    const TINY: &str = "whisper-tiny.en-Q8_0";

    struct Fixture {
        dir: tempfile::TempDir,
        config: EngineConfig,
    }

    fn catalog(id: &str) -> Result<narrivy_model_manager::CatalogModel, String> {
        model_catalog()
            .into_iter()
            .find(|model| model.id == id)
            .ok_or_else(|| format!("{id} is not in the catalog"))
    }

    /// A sparse file of the catalog's exact size: the name and size checks
    /// here never read the bytes.
    fn place(dir: &Path, id: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let model = catalog(id)?;
        let path = dir.join(model.filename.ok_or("no file name")?);
        std::fs::File::create(&path)?.set_len(model.size_bytes)?;
        Ok(path)
    }

    /// A daemon started with the tiny model in a private directory.
    fn started_with_tiny() -> Result<Fixture, Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let tiny = catalog(TINY)?;
        let path = place(dir.path(), TINY)?;
        let config = EngineConfig {
            asr_engine: AsrEngineKind::TranscribeCpp,
            transcribe_resident: true,
            transcribe_model_path: Some(path),
            transcribe_model_sha256: tiny.sha256,
            transcribe_model_size: Some(tiny.size_bytes),
            ..EngineConfig::default()
        };
        Ok(Fixture { dir, config })
    }

    #[test]
    fn installed_catalog_model_beside_the_loaded_one_resolves_with_its_pin() -> TestResult {
        let fixture = started_with_tiny()?;
        assert_eq!(
            resolve(&fixture.config, None, PARAKEET),
            Err(SelectError::NotInstalled)
        );
        let path = place(fixture.dir.path(), PARAKEET)?;
        let parakeet = catalog(PARAKEET)?;
        assert_eq!(
            resolve(&fixture.config, None, PARAKEET),
            Ok(Resolution::Load(SelectedModel {
                id: PARAKEET.to_owned(),
                path,
                sha256: parakeet.sha256.ok_or("no pin")?,
                size: parakeet.size_bytes,
                engine: ModelEngine::TranscribeCpp,
            }))
        );
        Ok(())
    }

    #[test]
    fn the_model_in_use_and_unknown_or_foreign_models_are_not_loaded() -> TestResult {
        let fixture = started_with_tiny()?;
        assert_eq!(
            resolve(&fixture.config, None, TINY),
            Ok(Resolution::AlreadyActive)
        );
        assert_eq!(
            resolve(&fixture.config, None, "no-such-model"),
            Err(SelectError::UnknownModel)
        );
        let foreign = model_catalog()
            .into_iter()
            .find(|model| model.engine != ENGINE_TRANSCRIBE_CPP)
            .ok_or("catalog has no add-on model")?;
        assert_eq!(
            resolve(&fixture.config, None, &foreign.id),
            Err(SelectError::NeedsEngine)
        );
        // A wrong-sized file with the right name is not "installed".
        let parakeet = catalog(PARAKEET)?;
        std::fs::write(
            fixture
                .dir
                .path()
                .join(parakeet.filename.ok_or("no file name")?),
            b"short",
        )?;
        assert_eq!(
            resolve(&fixture.config, None, PARAKEET),
            Err(SelectError::NotInstalled)
        );
        Ok(())
    }

    #[test]
    fn other_compositions_cannot_switch() -> TestResult {
        let fixture = started_with_tiny()?;
        place(fixture.dir.path(), PARAKEET)?;
        let installed = EngineConfig {
            transcribe_installed: true,
            ..fixture.config.clone()
        };
        let not_resident = EngineConfig {
            transcribe_resident: false,
            ..fixture.config.clone()
        };
        for config in [installed, not_resident, EngineConfig::default()] {
            assert_eq!(
                resolve(&config, None, PARAKEET),
                Err(SelectError::NotAvailable)
            );
        }
        Ok(())
    }

    #[test]
    fn a_finished_load_becomes_current_and_is_restored_at_the_next_start() -> TestResult {
        let fixture = started_with_tiny()?;
        place(fixture.dir.path(), PARAKEET)?;
        let state = tempfile::tempdir()?;
        let selection = ModelSelection::default();
        selection.persist_under(state.path());
        assert_eq!(selection.capability(), "model.selection=idle");
        assert_eq!(restore(state.path(), &fixture.config), None);

        let resolved = resolve(&fixture.config, None, PARAKEET).map_err(SelectError::detail)?;
        let Resolution::Load(target) = resolved else {
            return Err("parakeet did not resolve to a load".into());
        };
        assert!(selection.begin(PARAKEET));
        assert!(!selection.begin(TINY), "a second load started");
        assert_eq!(selection.loading_id().as_deref(), Some(PARAKEET));
        assert_eq!(selection.capability(), "model.selection=loading");
        selection.finish(Ok((target.clone(), ComputeChoice::Auto)));
        assert_eq!(selection.capability(), "model.selection=idle");
        assert_eq!(selection.current(), Some(target.clone()));

        // With Parakeet current, it is the active one and tiny can be chosen.
        let current = selection.current();
        assert_eq!(
            resolve(&fixture.config, current.as_ref(), PARAKEET),
            Ok(Resolution::AlreadyActive)
        );
        assert!(matches!(
            resolve(&fixture.config, current.as_ref(), TINY),
            Ok(Resolution::Load(_))
        ));
        // The next start loads the remembered choice.
        assert_eq!(restore(state.path(), &fixture.config), Some(target));
        Ok(())
    }

    #[test]
    fn a_pipeline_reloads_only_when_its_model_or_device_differs() -> TestResult {
        let fixture = started_with_tiny()?;
        place(fixture.dir.path(), PARAKEET)?;
        let selection = ModelSelection::default();
        let plan_for =
            |pipeline: &str, on_gpu: bool| plan(&fixture.config, &selection, pipeline, on_gpu);

        // A pipeline that chose nothing uses what is loaded.
        assert_eq!(plan_for("clean", true), Plan::Keep);
        assert_eq!(plan_for("clean", false), Plan::Keep);

        // The same model as loaded: no reload, whichever pipeline asks.
        selection.set_pipeline_engine(
            "clean",
            PipelineEngine {
                model: Some(TINY.to_owned()),
                compute: ComputeChoice::Auto,
            },
        );
        assert_eq!(plan_for("clean", true), Plan::Keep);

        // Another model: load it, with that pipeline's device.
        selection.set_pipeline_engine(
            "raw",
            PipelineEngine {
                model: Some(PARAKEET.to_owned()),
                compute: ComputeChoice::Cpu,
            },
        );
        let Plan::Load { model, compute } = plan_for("raw", true) else {
            return Err("another model did not plan a load".into());
        };
        assert_eq!((model.id.as_str(), compute), (PARAKEET, ComputeChoice::Cpu));

        // Once it is loaded on the CPU, "raw" keeps it and "clean" goes back.
        selection.finish(Ok((model, ComputeChoice::Cpu)));
        assert_eq!(plan_for("raw", false), Plan::Keep);
        let Plan::Load { model, compute } = plan_for("clean", false) else {
            return Err("returning to the first model did not plan a load".into());
        };
        assert_eq!((model.id.as_str(), compute), (TINY, ComputeChoice::Auto));
        Ok(())
    }

    #[test]
    fn the_same_model_reloads_only_for_a_device_it_is_not_on() -> TestResult {
        let fixture = started_with_tiny()?;
        let selection = ModelSelection::default();
        let want = |compute| {
            selection.set_pipeline_engine(
                "clean",
                PipelineEngine {
                    model: None,
                    compute,
                },
            );
        };
        let needs_load = |on_gpu| plan(&fixture.config, &selection, "clean", on_gpu) != Plan::Keep;

        // Loaded automatically: on the GPU it satisfies Auto and GPU, on the
        // CPU it satisfies Auto and CPU.
        want(ComputeChoice::Auto);
        assert!(!needs_load(true) && !needs_load(false));
        want(ComputeChoice::Gpu);
        assert!(!needs_load(true) && needs_load(false));
        want(ComputeChoice::Cpu);
        assert!(needs_load(true) && !needs_load(false));

        // Loaded for the CPU on request: Auto reloads to try the GPU again.
        selection.set_current_compute(ComputeChoice::Cpu);
        want(ComputeChoice::Auto);
        assert!(needs_load(false));
        let Plan::Load { model, compute } = plan(&fixture.config, &selection, "clean", false)
        else {
            return Err("a device change did not plan a load".into());
        };
        assert_eq!((model.id.as_str(), compute), (TINY, ComputeChoice::Auto));
        Ok(())
    }

    #[test]
    fn a_pipeline_whose_model_is_gone_keeps_the_loaded_model() -> TestResult {
        let fixture = started_with_tiny()?;
        let selection = ModelSelection::default();
        selection.set_pipeline_engine(
            "clean",
            PipelineEngine {
                model: Some(PARAKEET.to_owned()),
                compute: ComputeChoice::Auto,
            },
        );
        // Parakeet is not beside the loaded model: dictation still works.
        assert_eq!(plan(&fixture.config, &selection, "clean", true), Plan::Keep);
        Ok(())
    }

    #[test]
    fn pipeline_choices_survive_a_restart_and_bad_lines_are_skipped() -> TestResult {
        let state = tempfile::tempdir()?;
        let selection = ModelSelection::default();
        selection.persist_under(state.path());
        selection.set_pipeline_engine(
            "clean",
            PipelineEngine {
                model: Some(PARAKEET.to_owned()),
                compute: ComputeChoice::Gpu,
            },
        );
        selection.set_pipeline_engine(
            "raw",
            PipelineEngine {
                model: None,
                compute: ComputeChoice::Cpu,
            },
        );
        let restarted = ModelSelection::default();
        restarted.persist_under(state.path());
        assert_eq!(
            restarted.pipeline_engine("clean"),
            selection.pipeline_engine("clean")
        );
        assert_eq!(
            restarted.pipeline_engine("raw"),
            selection.pipeline_engine("raw")
        );
        assert_eq!(
            restarted.pipeline_engine("other"),
            PipelineEngine::default()
        );

        let parsed = parse_pipelines(
            "clean\t-\tcpu\nbroken line\nraw\tm\ttpu\n\t-\tcpu\nx\ty\tgpu\textra\n",
        );
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed["clean"].compute, ComputeChoice::Cpu);
        Ok(())
    }

    #[test]
    fn a_failed_load_keeps_the_previous_model_and_reports_the_reason() -> TestResult {
        let state = tempfile::tempdir()?;
        let selection = ModelSelection::default();
        selection.persist_under(state.path());
        assert!(selection.begin(PARAKEET));
        selection.finish(Err("resident_model_load_failed"));
        assert_eq!(
            selection.capability(),
            "model.selection=failed:resident_model_load_failed"
        );
        assert_eq!(selection.current(), None);
        assert!(!state.path().join(STORE_FILE).exists());
        // A later attempt may start.
        assert!(selection.begin(PARAKEET));
        Ok(())
    }
}
