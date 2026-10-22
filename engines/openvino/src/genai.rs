//! The OpenVINO GenAI C API, loaded at run time from a runtime pack.
//!
//! This is the crate's only `unsafe` code. Invariants:
//! - Every library is loaded by absolute path from the runtime pack, in
//!   dependency order, and kept loaded (`Runtime::_libraries`) for as long
//!   as any function pointer taken from it can be called. `Runtime` owns
//!   both, and every handle below borrows the `Runtime`.
//! - Function signatures match the headers of OpenVINO 2026.4
//!   (`openvino/genai/c/whisper_pipeline.h`, `llm_pipeline.h`, `vlm_pipeline.h`,
//!   `generation_config.h`, `chat_history.h`, `json_container.h`,
//!   `openvino/c/ov_core.h`); the runtime pack is pinned to that release.
//! - Every pointer passed in is valid for the call: strings are
//!   NUL-terminated `CString`s that outlive the call, audio is a live
//!   slice. Every object the API creates is freed exactly once, by the
//!   `Drop` of the wrapper that owns it.
//! - The worker is single-threaded; no handle is shared across threads.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::path::Path;

use libloading::Library;

type Status = c_int;
const OK: Status = 0;

#[repr(C)]
struct AvailableDevices {
    devices: *mut *mut c_char,
    size: usize,
}

type CoreCreate = unsafe extern "C" fn(*mut *mut c_void) -> Status;
type CoreFree = unsafe extern "C" fn(*mut c_void);
type CoreDevices = unsafe extern "C" fn(*const c_void, *mut AvailableDevices) -> Status;
type DevicesFree = unsafe extern "C" fn(*mut AvailableDevices);
type PipelineCreate =
    unsafe extern "C" fn(*const c_char, *const c_char, usize, *mut *mut c_void, ...) -> Status;
type PipelineFree = unsafe extern "C" fn(*mut c_void);
type PipelineGenerate =
    unsafe extern "C" fn(*mut c_void, *const f32, usize, *const c_void, *mut *mut c_void) -> Status;
type PipelineConfig = unsafe extern "C" fn(*const c_void, *mut *mut c_void) -> Status;
type ConfigSetText = unsafe extern "C" fn(*mut c_void, *const c_char) -> Status;
type ConfigFree = unsafe extern "C" fn(*mut c_void);
type ResultsString = unsafe extern "C" fn(*const c_void, *mut c_char, *mut usize) -> Status;
type ResultsFree = unsafe extern "C" fn(*mut c_void);
type LlmGenerate = unsafe extern "C" fn(
    *mut c_void,
    *const c_void,
    *const c_void,
    *const c_void,
    *mut *mut c_void,
) -> Status;
type Create = unsafe extern "C" fn(*mut *mut c_void) -> Status;
type SetSize = unsafe extern "C" fn(*mut c_void, usize) -> Status;
type JsonFromString = unsafe extern "C" fn(*mut *mut c_void, *const c_char) -> Status;
type HistoryFromJson = unsafe extern "C" fn(*mut *mut c_void, *const c_void) -> Status;
type HistorySetJson = unsafe extern "C" fn(*mut c_void, *const c_void) -> Status;
type VlmGenerate = unsafe extern "C" fn(
    *mut c_void,
    *const c_void,
    *const *const c_void,
    usize,
    *const c_void,
    *const Streamer,
    *mut *mut c_void,
) -> Status;

#[repr(C)]
struct Shape {
    rank: i64,
    dims: *const i64,
}
type TensorCreate = unsafe extern "C" fn(c_int, Shape, *mut *mut c_void) -> Status;
type TensorData = unsafe extern "C" fn(*const c_void, *mut *mut c_void) -> Status;
/// `ov_element_type_e::U8`.
const ELEMENT_U8: c_int = 16;

/// A picture for a vision-language model: `width` × `height` RGB bytes.
pub struct Picture {
    pub width: usize,
    pub height: usize,
    pub rgb: Vec<u8>,
}

/// `streamer_callback` from `llm_pipeline.h`: called with each decoded piece
/// of text; returns 0 to continue, 1 to stop.
#[repr(C)]
struct Streamer {
    callback: unsafe extern "C" fn(*const c_char, *mut c_void) -> c_int,
    args: *mut c_void,
}

const STREAM_RUNNING: c_int = 0;
const STREAM_STOP: c_int = 1;

/// The Rust side of a streamer: receives each piece and says whether to
/// stop.
type OnText<'a> = &'a mut dyn FnMut(&str) -> bool;

/// Called by the runtime on its generation thread, synchronously within
/// `generate`. Never unwinds into C.
unsafe extern "C" fn stream_trampoline(text: *const c_char, args: *mut c_void) -> c_int {
    if text.is_null() || args.is_null() {
        return STREAM_RUNNING;
    }
    // SAFETY: `args` is the `&mut OnText` passed for this call only, alive
    // for its whole duration; `text` is a NUL-terminated string the runtime
    // owns for the duration of the callback.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        let on_text = &mut *args.cast::<OnText<'_>>();
        on_text(&CStr::from_ptr(text).to_string_lossy())
    }));
    match outcome {
        Ok(false) => STREAM_RUNNING,
        _ => STREAM_STOP,
    }
}

/// The language-model half of the GenAI C API.
struct LlmApi {
    create: PipelineCreate,
    free: PipelineFree,
    generate_with_history: LlmGenerate,
    config_create: Create,
    config_free: ConfigFree,
    config_max_new_tokens: SetSize,
    results_string: ResultsString,
    results_free: ResultsFree,
    json_from_string: JsonFromString,
    json_free: ConfigFree,
    history_from_json: HistoryFromJson,
    history_set_extra_context: HistorySetJson,
    history_free: ConfigFree,
    vlm_create: PipelineCreate,
    vlm_free: PipelineFree,
    vlm_generate_with_history: VlmGenerate,
    vlm_results_string: ResultsString,
    vlm_results_free: ResultsFree,
    tensor_create: TensorCreate,
    tensor_data: TensorData,
    tensor_free: ConfigFree,
}

/// Why the runtime or a pipeline could not be used. Content-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenAiError {
    /// A library or symbol is missing from the runtime pack.
    Runtime,
    /// The pipeline could not be created on the requested device.
    Load,
    /// Recognition failed.
    Generate,
}

struct Api {
    core_create: CoreCreate,
    core_free: CoreFree,
    core_devices: CoreDevices,
    devices_free: DevicesFree,
    pipeline_create: PipelineCreate,
    pipeline_free: PipelineFree,
    pipeline_generate: PipelineGenerate,
    pipeline_config: PipelineConfig,
    config_language: ConfigSetText,
    config_task: ConfigSetText,
    /// Absent from older runtimes; recognition then ignores prompts.
    config_initial_prompt: Option<ConfigSetText>,
    config_free: ConfigFree,
    results_string: ResultsString,
    results_free: ResultsFree,
}

/// Give memory the allocator holds but no longer uses back to the system.
///
/// Compiling a model for a device leaves hundreds of megabytes of freed
/// heap that the C library keeps for reuse; a worker calls this once its
/// model is loaded so its footprint is what the model needs. A no-op where
/// the C library is not glibc.
pub fn release_free_memory() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        unsafe extern "C" {
            fn malloc_trim(pad: usize) -> c_int;
        }
        // SAFETY: glibc's `malloc_trim` takes a byte count, touches only the
        // allocator's own free lists, and is safe to call at any time.
        unsafe { malloc_trim(0) };
    }
}

/// The loaded runtime. Dropping it unloads the libraries, so it must
/// outlive every [`Whisper`] made from it, which the borrow enforces.
pub struct Runtime {
    api: Api,
    llm: LlmApi,
    libraries: Vec<(&'static str, Library)>,
    lib_dir: std::path::PathBuf,
}

impl Runtime {
    /// Load the runtime from a pack's `lib` directory. `with_npu` also loads
    /// the Level Zero loader and NPU driver, when the pack has them.
    pub fn load(lib_dir: &Path, with_npu: bool) -> Result<Self, GenAiError> {
        let mut names: Vec<&'static str> = vec![crate::RUNTIME_LIBRARIES[0]];
        if with_npu {
            names.extend(crate::NPU_DRIVER_LIBRARIES);
        }
        names.extend(&crate::RUNTIME_LIBRARIES[1..]);
        // The Level Zero loader finds drivers by searching library paths,
        // not by what is already loaded, so name the pack's NPU driver.
        let npu_driver = lib_dir.join(crate::NPU_DRIVER_LIBRARIES[1]);
        if with_npu && npu_driver.is_file() {
            // SAFETY: the worker sets this before any library is loaded and
            // before any other thread exists, so nothing reads the
            // environment concurrently.
            unsafe { std::env::set_var("ZE_ENABLE_ALT_DRIVERS", &npu_driver) };
        }
        let mut libraries: Vec<(&'static str, Library)> = Vec::new();
        for name in names {
            let path = lib_dir.join(name);
            if crate::NPU_DRIVER_LIBRARIES.contains(&name) && !path.is_file() {
                continue;
            }
            // SAFETY: loading a pinned OpenVINO 2026.4 library by absolute
            // path from the installed runtime pack runs its initialisers,
            // which have no preconditions on this process.
            let library = unsafe { Library::new(&path) }.map_err(|_| GenAiError::Runtime)?;
            libraries.push((name, library));
        }
        let find = |wanted: &str| {
            libraries
                .iter()
                .find(|(name, _)| *name == wanted)
                .map(|(_, library)| library)
                .ok_or(GenAiError::Runtime)
        };
        let core = find("libopenvino_c.so.2640")?;
        let genai = find("libopenvino_genai_c.so.2640")?;
        // SAFETY: each symbol is read with the type its 2026.4 header
        // declares, and the libraries stay loaded inside `Runtime`.
        let api = unsafe {
            Api {
                core_create: *core
                    .get::<CoreCreate>(b"ov_core_create\0")
                    .map_err(|_| GenAiError::Runtime)?,
                core_free: *core
                    .get::<CoreFree>(b"ov_core_free\0")
                    .map_err(|_| GenAiError::Runtime)?,
                core_devices: *core
                    .get::<CoreDevices>(b"ov_core_get_available_devices\0")
                    .map_err(|_| GenAiError::Runtime)?,
                devices_free: *core
                    .get::<DevicesFree>(b"ov_available_devices_free\0")
                    .map_err(|_| GenAiError::Runtime)?,
                pipeline_create: *genai
                    .get::<PipelineCreate>(b"ov_genai_whisper_pipeline_create\0")
                    .map_err(|_| GenAiError::Runtime)?,
                pipeline_free: *genai
                    .get::<PipelineFree>(b"ov_genai_whisper_pipeline_free\0")
                    .map_err(|_| GenAiError::Runtime)?,
                pipeline_generate: *genai
                    .get::<PipelineGenerate>(b"ov_genai_whisper_pipeline_generate\0")
                    .map_err(|_| GenAiError::Runtime)?,
                pipeline_config: *genai
                    .get::<PipelineConfig>(b"ov_genai_whisper_pipeline_get_generation_config\0")
                    .map_err(|_| GenAiError::Runtime)?,
                config_language: *genai
                    .get::<ConfigSetText>(b"ov_genai_whisper_generation_config_set_language\0")
                    .map_err(|_| GenAiError::Runtime)?,
                config_task: *genai
                    .get::<ConfigSetText>(b"ov_genai_whisper_generation_config_set_task\0")
                    .map_err(|_| GenAiError::Runtime)?,
                config_initial_prompt: genai
                    .get::<ConfigSetText>(b"ov_genai_whisper_generation_config_set_initial_prompt\0")
                    .ok()
                    .map(|symbol| *symbol),
                config_free: *genai
                    .get::<ConfigFree>(b"ov_genai_whisper_generation_config_free\0")
                    .map_err(|_| GenAiError::Runtime)?,
                results_string: *genai
                    .get::<ResultsString>(b"ov_genai_whisper_decoded_results_get_string\0")
                    .map_err(|_| GenAiError::Runtime)?,
                results_free: *genai
                    .get::<ResultsFree>(b"ov_genai_whisper_decoded_results_free\0")
                    .map_err(|_| GenAiError::Runtime)?,
            }
        };
        // SAFETY: as above, each symbol has its 2026.4 header type.
        let llm = unsafe {
            LlmApi {
                create: *genai
                    .get::<PipelineCreate>(b"ov_genai_llm_pipeline_create\0")
                    .map_err(|_| GenAiError::Runtime)?,
                free: *genai
                    .get::<PipelineFree>(b"ov_genai_llm_pipeline_free\0")
                    .map_err(|_| GenAiError::Runtime)?,
                generate_with_history: *genai
                    .get::<LlmGenerate>(b"ov_genai_llm_pipeline_generate_with_history\0")
                    .map_err(|_| GenAiError::Runtime)?,
                config_create: *genai
                    .get::<Create>(b"ov_genai_generation_config_create\0")
                    .map_err(|_| GenAiError::Runtime)?,
                config_free: *genai
                    .get::<ConfigFree>(b"ov_genai_generation_config_free\0")
                    .map_err(|_| GenAiError::Runtime)?,
                config_max_new_tokens: *genai
                    .get::<SetSize>(b"ov_genai_generation_config_set_max_new_tokens\0")
                    .map_err(|_| GenAiError::Runtime)?,
                results_string: *genai
                    .get::<ResultsString>(b"ov_genai_decoded_results_get_string\0")
                    .map_err(|_| GenAiError::Runtime)?,
                results_free: *genai
                    .get::<ResultsFree>(b"ov_genai_decoded_results_free\0")
                    .map_err(|_| GenAiError::Runtime)?,
                json_from_string: *genai
                    .get::<JsonFromString>(b"ov_genai_json_container_create_from_json_string\0")
                    .map_err(|_| GenAiError::Runtime)?,
                json_free: *genai
                    .get::<ConfigFree>(b"ov_genai_json_container_free\0")
                    .map_err(|_| GenAiError::Runtime)?,
                history_from_json: *genai
                    .get::<HistoryFromJson>(b"ov_genai_chat_history_create_from_json_container\0")
                    .map_err(|_| GenAiError::Runtime)?,
                history_set_extra_context: *genai
                    .get::<HistorySetJson>(b"ov_genai_chat_history_set_extra_context\0")
                    .map_err(|_| GenAiError::Runtime)?,
                history_free: *genai
                    .get::<ConfigFree>(b"ov_genai_chat_history_free\0")
                    .map_err(|_| GenAiError::Runtime)?,
                vlm_create: *genai
                    .get::<PipelineCreate>(b"ov_genai_vlm_pipeline_create\0")
                    .map_err(|_| GenAiError::Runtime)?,
                vlm_free: *genai
                    .get::<PipelineFree>(b"ov_genai_vlm_pipeline_free\0")
                    .map_err(|_| GenAiError::Runtime)?,
                vlm_generate_with_history: *genai
                    .get::<VlmGenerate>(b"ov_genai_vlm_pipeline_generate_with_history\0")
                    .map_err(|_| GenAiError::Runtime)?,
                vlm_results_string: *genai
                    .get::<ResultsString>(b"ov_genai_vlm_decoded_results_get_string\0")
                    .map_err(|_| GenAiError::Runtime)?,
                vlm_results_free: *genai
                    .get::<ResultsFree>(b"ov_genai_vlm_decoded_results_free\0")
                    .map_err(|_| GenAiError::Runtime)?,
                tensor_create: *core
                    .get::<TensorCreate>(b"ov_tensor_create\0")
                    .map_err(|_| GenAiError::Runtime)?,
                tensor_data: *core
                    .get::<TensorData>(b"ov_tensor_data\0")
                    .map_err(|_| GenAiError::Runtime)?,
                tensor_free: *core
                    .get::<ConfigFree>(b"ov_tensor_free\0")
                    .map_err(|_| GenAiError::Runtime)?,
            }
        };
        Ok(Self {
            api,
            llm,
            libraries,
            lib_dir: lib_dir.to_path_buf(),
        })
    }

    /// A loaded runtime library, for the other C API users in this crate.
    pub(crate) fn library(&self, name: &str) -> Option<&Library> {
        self.libraries
            .iter()
            .find(|(loaded, _)| *loaded == name)
            .map(|(_, library)| library)
    }

    /// The runtime pack's library directory.
    pub(crate) fn lib_dir(&self) -> &Path {
        &self.lib_dir
    }

    /// The devices OpenVINO can run on here, such as `CPU` and `NPU`.
    pub fn devices(&self) -> Result<Vec<String>, GenAiError> {
        let mut core: *mut c_void = std::ptr::null_mut();
        // SAFETY: `core` is a valid out-pointer; on success it owns a core
        // freed below.
        if unsafe { (self.api.core_create)(&mut core) } != OK || core.is_null() {
            return Err(GenAiError::Runtime);
        }
        let mut list = AvailableDevices {
            devices: std::ptr::null_mut(),
            size: 0,
        };
        // SAFETY: `core` is live; `list` is a valid out-structure that the
        // runtime fills and `devices_free` releases.
        let status = unsafe { (self.api.core_devices)(core, &mut list) };
        let mut names = Vec::new();
        if status == OK && !list.devices.is_null() {
            for index in 0..list.size {
                // SAFETY: the runtime returned `size` valid C strings.
                let name = unsafe { CStr::from_ptr(*list.devices.add(index)) };
                names.push(name.to_string_lossy().into_owned());
            }
        }
        // SAFETY: each object is freed once, after its last use.
        unsafe {
            if status == OK {
                (self.api.devices_free)(&mut list);
            }
            (self.api.core_free)(core);
        }
        if status == OK {
            Ok(names)
        } else {
            Err(GenAiError::Runtime)
        }
    }

    /// Load a Whisper model directory on `device` (`CPU`, `GPU` or `NPU`),
    /// with string properties such as `CACHE_DIR`.
    pub fn whisper(
        &self,
        model_dir: &Path,
        device: &str,
        properties: &[(&str, &str)],
    ) -> Result<Whisper<'_>, GenAiError> {
        let pipeline = create_pipeline(self.api.pipeline_create, model_dir, device, properties)?;
        Ok(Whisper {
            runtime: self,
            pipeline,
        })
    }
}

impl Runtime {
    /// Load a language model directory (OpenVINO IR with its tokenizer) on
    /// `device`, with string properties such as `CACHE_DIR` and, for the
    /// NPU, `MAX_PROMPT_LEN`.
    pub fn llm(
        &self,
        model_dir: &Path,
        device: &str,
        properties: &[(&str, &str)],
    ) -> Result<Llm<'_>, GenAiError> {
        // A vision-language export (such as Qwen3.5) splits the language
        // model from the vision encoder; it runs in the VLM pipeline, used
        // here for text only.
        let vision = model_dir.join("openvino_language_model.xml").is_file();
        let create = if vision {
            self.llm.vlm_create
        } else {
            self.llm.create
        };
        let pipeline = create_pipeline(create, model_dir, device, properties)?;
        Ok(Llm {
            runtime: self,
            pipeline,
            vision,
        })
    }
}

/// One loaded language model.
pub struct Llm<'runtime> {
    runtime: &'runtime Runtime,
    pipeline: *mut c_void,
    /// Loaded through the VLM pipeline.
    vision: bool,
}

impl Llm<'_> {
    /// Generate a reply to `messages`, a JSON array of `{role, content}`
    /// objects, through the model's own chat template. `extra_context` is a
    /// JSON object of template variables (for example
    /// `{"enable_thinking": false}`). Greedy decoding, at most
    /// `max_new_tokens`.
    pub fn chat(
        &mut self,
        messages: &str,
        extra_context: Option<&str>,
        max_new_tokens: usize,
    ) -> Result<String, GenAiError> {
        self.chat_streaming(messages, extra_context, max_new_tokens, None)
    }

    /// [`Self::chat`], handing each decoded piece to `on_text` as it is
    /// produced; `on_text` returns `true` to stop early. The full reply is
    /// still returned.
    pub fn chat_streaming(
        &mut self,
        messages: &str,
        extra_context: Option<&str>,
        max_new_tokens: usize,
        on_text: Option<&mut dyn FnMut(&str) -> bool>,
    ) -> Result<String, GenAiError> {
        self.chat_with_pictures(messages, extra_context, max_new_tokens, on_text, &[])
    }

    /// Whether the model can look at pictures.
    #[must_use]
    pub fn sees(&self) -> bool {
        self.vision
    }

    /// [`Self::chat_streaming`] with pictures for the last user message
    /// (a vision-language model only; others refuse pictures).
    pub fn chat_with_pictures(
        &mut self,
        messages: &str,
        extra_context: Option<&str>,
        max_new_tokens: usize,
        on_text: Option<&mut dyn FnMut(&str) -> bool>,
        pictures: &[Picture],
    ) -> Result<String, GenAiError> {
        if !pictures.is_empty() && !self.vision {
            return Err(GenAiError::Generate);
        }
        let api = &self.runtime.llm;
        let tensors = Tensors::of(api, pictures)?;
        let messages = Json::parse(api, messages)?;
        let extra = extra_context
            .map(|json| Json::parse(api, json))
            .transpose()?;
        let mut history: *mut c_void = std::ptr::null_mut();
        // SAFETY: `messages` is a live container; `history` is freed below.
        if unsafe { (api.history_from_json)(&mut history, messages.0) } != OK || history.is_null() {
            return Err(GenAiError::Generate);
        }
        let outcome = self.generate(history, extra.as_ref(), max_new_tokens, on_text, &tensors.0);
        // SAFETY: freed once, after its last use.
        unsafe { (api.history_free)(history) };
        outcome
    }

    fn generate(
        &mut self,
        history: *mut c_void,
        extra: Option<&Json<'_>>,
        max_new_tokens: usize,
        on_text: Option<&mut dyn FnMut(&str) -> bool>,
        pictures: &[*mut c_void],
    ) -> Result<String, GenAiError> {
        let api = &self.runtime.llm;
        if let Some(extra) = extra {
            // SAFETY: both objects are live; the history copies the context.
            if unsafe { (api.history_set_extra_context)(history, extra.0) } != OK {
                return Err(GenAiError::Generate);
            }
        }
        let mut config: *mut c_void = std::ptr::null_mut();
        // SAFETY: `config` receives a new config, freed below.
        if unsafe { (api.config_create)(&mut config) } != OK || config.is_null() {
            return Err(GenAiError::Generate);
        }
        let mut results: *mut c_void = std::ptr::null_mut();
        let mut handler: Option<OnText<'_>> = on_text;
        let streamer = handler.as_mut().map(|handler| Streamer {
            callback: stream_trampoline,
            args: std::ptr::from_mut::<OnText<'_>>(handler).cast(),
        });
        let streamer_ptr = streamer
            .as_ref()
            .map_or(std::ptr::null(), std::ptr::from_ref);
        // SAFETY: pipeline, history and config are live; the streamer and
        // the handler it points to outlive the call, which invokes it only
        // synchronously; `results` is freed below.
        let status = unsafe {
            if (api.config_max_new_tokens)(config, max_new_tokens) != OK {
                -1
            } else if self.vision {
                (api.vlm_generate_with_history)(
                    self.pipeline,
                    history,
                    if pictures.is_empty() {
                        std::ptr::null()
                    } else {
                        pictures.as_ptr().cast()
                    },
                    pictures.len(),
                    config,
                    streamer_ptr,
                    &mut results,
                )
            } else {
                (api.generate_with_history)(
                    self.pipeline,
                    history,
                    config,
                    streamer_ptr.cast(),
                    &mut results,
                )
            }
        };
        // SAFETY: freed once, after the call that read it.
        unsafe { (api.config_free)(config) };
        if status != OK || results.is_null() {
            return Err(GenAiError::Generate);
        }
        let (read, free) = if self.vision {
            (api.vlm_results_string, api.vlm_results_free)
        } else {
            (api.results_string, api.results_free)
        };
        let text = read_string(read, results);
        // SAFETY: freed once, after `read_string` read it.
        unsafe { free(results) };
        text
    }
}

impl Drop for Llm<'_> {
    fn drop(&mut self) {
        let free = if self.vision {
            self.runtime.llm.vlm_free
        } else {
            self.runtime.llm.free
        };
        // SAFETY: the pipeline was created by this runtime and is freed once.
        unsafe { free(self.pipeline) };
    }
}

/// Picture tensors (`u8`, `[1, height, width, 3]`), freed on drop.
struct Tensors<'api>(Vec<*mut c_void>, &'api LlmApi);

impl<'api> Tensors<'api> {
    fn of(api: &'api LlmApi, pictures: &[Picture]) -> Result<Self, GenAiError> {
        let mut tensors = Self(Vec::with_capacity(pictures.len()), api);
        for picture in pictures {
            let bytes = picture.width * picture.height * 3;
            if picture.width == 0 || picture.height == 0 || picture.rgb.len() != bytes {
                return Err(GenAiError::Generate);
            }
            let dims = [
                1,
                i64::try_from(picture.height).map_err(|_| GenAiError::Generate)?,
                i64::try_from(picture.width).map_err(|_| GenAiError::Generate)?,
                3,
            ];
            let shape = Shape {
                rank: 4,
                dims: dims.as_ptr(),
            };
            let mut tensor: *mut c_void = std::ptr::null_mut();
            // SAFETY: `dims` outlives the call (the shape is copied); valid
            // out-pointer; the tensor is freed on drop.
            if unsafe { (api.tensor_create)(ELEMENT_U8, shape, &mut tensor) } != OK
                || tensor.is_null()
            {
                return Err(GenAiError::Generate);
            }
            tensors.0.push(tensor);
            let mut data: *mut c_void = std::ptr::null_mut();
            // SAFETY: the tensor is live and owns `bytes` bytes of u8 data
            // (its shape above); the copy stays within both buffers.
            unsafe {
                if (api.tensor_data)(tensor, &mut data) != OK || data.is_null() {
                    return Err(GenAiError::Generate);
                }
                std::ptr::copy_nonoverlapping(picture.rgb.as_ptr(), data.cast::<u8>(), bytes);
            }
        }
        Ok(tensors)
    }
}

impl Drop for Tensors<'_> {
    fn drop(&mut self) {
        for tensor in &self.0 {
            // SAFETY: each tensor was created by `of` and is freed once.
            unsafe { (self.1.tensor_free)(*tensor) };
        }
    }
}

/// A GenAI JSON container, freed on drop.
struct Json<'api>(*mut c_void, &'api LlmApi);

impl<'api> Json<'api> {
    fn parse(api: &'api LlmApi, json: &str) -> Result<Self, GenAiError> {
        let text = CString::new(json).map_err(|_| GenAiError::Generate)?;
        let mut container: *mut c_void = std::ptr::null_mut();
        // SAFETY: `text` outlives the call; the container is freed on drop.
        if unsafe { (api.json_from_string)(&mut container, text.as_ptr()) } != OK
            || container.is_null()
        {
            return Err(GenAiError::Generate);
        }
        Ok(Self(container, api))
    }
}

impl Drop for Json<'_> {
    fn drop(&mut self) {
        // SAFETY: created by `parse` and freed once.
        unsafe { (self.1.json_free)(self.0) };
    }
}

/// Create a pipeline with `create` (Whisper or LLM: the same variadic
/// signature), passing up to four string properties.
fn create_pipeline(
    create: PipelineCreate,
    model_dir: &Path,
    device: &str,
    properties: &[(&str, &str)],
) -> Result<*mut c_void, GenAiError> {
    let path =
        CString::new(model_dir.as_os_str().as_encoded_bytes()).map_err(|_| GenAiError::Load)?;
    let device = CString::new(device).map_err(|_| GenAiError::Load)?;
    let owned: Vec<(CString, CString)> = properties
        .iter()
        .map(|(key, value)| Ok((CString::new(*key)?, CString::new(*value)?)))
        .collect::<Result<_, std::ffi::NulError>>()
        .map_err(|_| GenAiError::Load)?;
    let mut pipeline: *mut c_void = std::ptr::null_mut();
    let (p, d, out) = (path.as_ptr(), device.as_ptr(), &mut pipeline);
    // SAFETY: strings outlive the call; the variadic arguments are key/value
    // `const char*` pairs, `2 * pairs` of them, as the C API reads them.
    let status = unsafe {
        match owned.as_slice() {
            [] => create(p, d, 0, out),
            [(k1, v1)] => create(p, d, 2, out, k1.as_ptr(), v1.as_ptr()),
            [(k1, v1), (k2, v2)] => create(
                p,
                d,
                4,
                out,
                k1.as_ptr(),
                v1.as_ptr(),
                k2.as_ptr(),
                v2.as_ptr(),
            ),
            [(k1, v1), (k2, v2), (k3, v3)] => create(
                p,
                d,
                6,
                out,
                k1.as_ptr(),
                v1.as_ptr(),
                k2.as_ptr(),
                v2.as_ptr(),
                k3.as_ptr(),
                v3.as_ptr(),
            ),
            [(k1, v1), (k2, v2), (k3, v3), (k4, v4)] => create(
                p,
                d,
                8,
                out,
                k1.as_ptr(),
                v1.as_ptr(),
                k2.as_ptr(),
                v2.as_ptr(),
                k3.as_ptr(),
                v3.as_ptr(),
                k4.as_ptr(),
                v4.as_ptr(),
            ),
            _ => return Err(GenAiError::Load),
        }
    };
    if status != OK || pipeline.is_null() {
        return Err(GenAiError::Load);
    }
    Ok(pipeline)
}

/// One loaded Whisper pipeline.
pub struct Whisper<'runtime> {
    runtime: &'runtime Runtime,
    pipeline: *mut c_void,
}

impl Whisper<'_> {
    /// Recognise 16 kHz mono audio. `language` is a code such as `en`, or
    /// `None` to let the model detect it.
    pub fn transcribe(
        &mut self,
        samples: &[f32],
        language: Option<&str>,
    ) -> Result<String, GenAiError> {
        self.transcribe_with_prompt(samples, language, None)
    }

    /// Whether this runtime can pass an initial prompt to the model.
    #[must_use]
    pub fn takes_prompt(&self) -> bool {
        self.runtime.api.config_initial_prompt.is_some()
    }

    /// Recognise with `prompt` as the text the model takes to come before
    /// the audio: names and terms in it are heard more readily.
    pub fn transcribe_with_prompt(
        &mut self,
        samples: &[f32],
        language: Option<&str>,
        prompt: Option<&str>,
    ) -> Result<String, GenAiError> {
        let api = &self.runtime.api;
        let mut config: *mut c_void = std::ptr::null_mut();
        // SAFETY: the pipeline is live; `config` receives a copy of its
        // generation config, freed below.
        if unsafe { (api.pipeline_config)(self.pipeline, &mut config) } != OK || config.is_null() {
            return Err(GenAiError::Generate);
        }
        let outcome = self.generate_with(config, samples, language, prompt);
        // SAFETY: freed once, after the last use above.
        unsafe { (api.config_free)(config) };
        outcome
    }

    fn generate_with(
        &mut self,
        config: *mut c_void,
        samples: &[f32],
        language: Option<&str>,
        prompt: Option<&str>,
    ) -> Result<String, GenAiError> {
        let api = &self.runtime.api;
        let task = CString::new("transcribe").map_err(|_| GenAiError::Generate)?;
        // SAFETY: `config` is live and the strings outlive the calls.
        unsafe {
            if (api.config_task)(config, task.as_ptr()) != OK {
                return Err(GenAiError::Generate);
            }
            if let Some(code) = language {
                let tag = CString::new(format!("<|{code}|>")).map_err(|_| GenAiError::Generate)?;
                if (api.config_language)(config, tag.as_ptr()) != OK {
                    return Err(GenAiError::Generate);
                }
            }
            if let (Some(set_prompt), Some(text)) = (api.config_initial_prompt, prompt) {
                let text = CString::new(text).map_err(|_| GenAiError::Generate)?;
                if set_prompt(config, text.as_ptr()) != OK {
                    return Err(GenAiError::Generate);
                }
            }
        }
        let mut results: *mut c_void = std::ptr::null_mut();
        // SAFETY: the pipeline and config are live, `samples` is a live
        // slice of `len` floats, and `results` is freed below.
        let status = unsafe {
            (api.pipeline_generate)(
                self.pipeline,
                samples.as_ptr(),
                samples.len(),
                config,
                &mut results,
            )
        };
        if status != OK || results.is_null() {
            return Err(GenAiError::Generate);
        }
        let text = results_text(api, results);
        // SAFETY: freed once, after `results_text` read it.
        unsafe { (api.results_free)(results) };
        text
    }
}

fn results_text(api: &Api, results: *mut c_void) -> Result<String, GenAiError> {
    read_string(api.results_string, results)
}

fn read_string(results_string: ResultsString, results: *mut c_void) -> Result<String, GenAiError> {
    let mut size = 0_usize;
    // SAFETY: a null buffer asks for the size including the NUL.
    if unsafe { results_string(results, std::ptr::null_mut(), &mut size) } != OK
        || size == 0
        || size > narrivy_asr_api::resident_protocol::MAX_TEXT_BYTES + 1
    {
        return Err(GenAiError::Generate);
    }
    let mut buffer = vec![0_u8; size];
    // SAFETY: `buffer` holds `size` bytes, as the runtime asked for.
    if unsafe { results_string(results, buffer.as_mut_ptr().cast(), &mut size) } != OK {
        return Err(GenAiError::Generate);
    }
    let text = CStr::from_bytes_until_nul(&buffer).map_err(|_| GenAiError::Generate)?;
    Ok(text.to_string_lossy().trim().to_owned())
}

impl Drop for Whisper<'_> {
    fn drop(&mut self) {
        // SAFETY: the pipeline was created by this runtime and is freed once.
        unsafe { (self.runtime.api.pipeline_free)(self.pipeline) };
    }
}
