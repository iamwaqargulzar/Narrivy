//! Fixed-shape language models on the OpenVINO core C API.
//!
//! The NPU cannot compile some models through the GenAI pipeline: Qwen3.5's
//! linear-attention layers export to a step loop it cannot schedule.
//! `scripts/npu/qwen35_npu.py` exports such models as a fixed-shape block
//! piece (and optionally a one-token piece) whose memory between calls is
//! explicit (`npu-shape.json` lists it). This module runs them:
//!
//! - memory stays in tensors the device reads in place. Each call's state
//!   outputs are written straight into a second buffer, which becomes the
//!   next call's input; keys and values live in one buffer per layer and
//!   only the new rows are copied in. Nothing round-trips per token;
//! - the fixed system prompt is computed once and restored per request;
//! - text is encoded by the pack's OpenVINO tokenizer (on the CPU) and
//!   decoded from its `tokenizer.json` (byte-level BPE).
//!
//! Unsafe invariants, as in [`crate::genai`]:
//! - every function pointer comes from `libopenvino_c` held by the
//!   [`Runtime`] this model borrows, so the library outlives every call;
//! - signatures match `openvino/c/*.h` of OpenVINO 2026.4;
//! - every handle the API creates is freed exactly once, in `Drop`, in
//!   reverse order of creation (tensors, requests, contexts, models, core);
//! - every pointer passed in is valid for the call; data pointers are taken
//!   from a live tensor and used only while that tensor lives, within the
//!   tensor's byte size;
//! - the worker is single-threaded; no handle crosses threads.

use std::collections::HashMap;
use std::ffi::{CString, c_char, c_int, c_void};
use std::marker::PhantomData;
use std::path::Path;

use crate::genai::{GenAiError, Runtime};

/// The file a fixed-shape pack is recognised by.
pub const NPU_SHAPE: &str = "npu-shape.json";

type Status = c_int;
const OK: Status = 0;
// `ov_element_type_e`.
const ELEMENT_F16: c_int = 3;
const F16_BYTES: usize = 2;
/// The inputs every call sets, besides memory.
const FIXED_INPUTS: [&str; 6] = ["ids", "pos", "valid", "conv_idx", "last", "mask"];
/// Attention to a masked position: finite, so a row never becomes NaN.
const MASKED: f32 = -1.0e4;

#[repr(C)]
struct Shape {
    rank: i64,
    dims: *const i64,
}

type CoreCreate = unsafe extern "C" fn(*mut *mut c_void) -> Status;
type AddExtension = unsafe extern "C" fn(*const c_void, *const c_char) -> Status;
type Compile = unsafe extern "C" fn(
    *const c_void,
    *const c_char,
    *const c_char,
    usize,
    *mut *mut c_void,
    ...
) -> Status;
type Free = unsafe extern "C" fn(*mut c_void);
type MakeRequest = unsafe extern "C" fn(*const c_void, *mut *mut c_void) -> Status;
type HostTensor = unsafe extern "C" fn(*const c_void, c_int, Shape, *mut *mut c_void) -> Status;
type TensorCreate = unsafe extern "C" fn(c_int, Shape, *mut *mut c_void) -> Status;
type StringTensor =
    unsafe extern "C" fn(*const *const c_char, usize, Shape, *mut *mut c_void) -> Status;
type GetTensor = unsafe extern "C" fn(*const c_void, *const c_char, *mut *mut c_void) -> Status;
type SetTensor = unsafe extern "C" fn(*mut c_void, *const c_char, *const c_void) -> Status;
type SetInput = unsafe extern "C" fn(*mut c_void, usize, *const c_void) -> Status;
type Infer = unsafe extern "C" fn(*mut c_void) -> Status;
type TensorData = unsafe extern "C" fn(*const c_void, *mut *mut c_void) -> Status;
type TensorBytes = unsafe extern "C" fn(*const c_void, *mut usize) -> Status;

struct Api {
    core_create: CoreCreate,
    core_free: Free,
    add_extension: AddExtension,
    compile: Compile,
    compiled_free: Free,
    get_context: MakeRequest,
    context_free: Free,
    create_request: MakeRequest,
    request_free: Free,
    host_tensor: HostTensor,
    tensor_create: TensorCreate,
    string_tensor: StringTensor,
    tensor_free: Free,
    get_tensor: GetTensor,
    set_tensor: SetTensor,
    set_input: SetInput,
    infer: Infer,
    tensor_data: TensorData,
    tensor_bytes: TensorBytes,
}

impl Api {
    fn load(runtime: &Runtime) -> Result<Self, GenAiError> {
        let core = runtime
            .library(&runtime.versioned("libopenvino_c.so"))
            .ok_or(GenAiError::Runtime)?;
        // SAFETY: each symbol is read with the type its 2026.4 header
        // declares; the library stays loaded inside `Runtime`.
        unsafe {
            macro_rules! symbol {
                ($name:literal) => {
                    *core.get($name).map_err(|_| GenAiError::Runtime)?
                };
            }
            Ok(Self {
                core_create: symbol!(b"ov_core_create\0"),
                core_free: symbol!(b"ov_core_free\0"),
                add_extension: symbol!(b"ov_core_add_extension\0"),
                compile: symbol!(b"ov_core_compile_model_from_file\0"),
                compiled_free: symbol!(b"ov_compiled_model_free\0"),
                get_context: symbol!(b"ov_compiled_model_get_context\0"),
                context_free: symbol!(b"ov_remote_context_free\0"),
                create_request: symbol!(b"ov_compiled_model_create_infer_request\0"),
                request_free: symbol!(b"ov_infer_request_free\0"),
                host_tensor: symbol!(b"ov_remote_context_create_host_tensor\0"),
                tensor_create: symbol!(b"ov_tensor_create\0"),
                string_tensor: symbol!(b"ov_tensor_create_from_string_array\0"),
                tensor_free: symbol!(b"ov_tensor_free\0"),
                get_tensor: symbol!(b"ov_infer_request_get_tensor\0"),
                set_tensor: symbol!(b"ov_infer_request_set_tensor\0"),
                set_input: symbol!(b"ov_infer_request_set_input_tensor_by_index\0"),
                infer: symbol!(b"ov_infer_request_infer\0"),
                tensor_data: symbol!(b"ov_tensor_data\0"),
                tensor_bytes: symbol!(b"ov_tensor_get_byte_size\0"),
            })
        }
    }
}

/// What `npu-shape.json` declares.
struct Layout {
    prompt_block: usize,
    context: usize,
    kernel: usize,
    memory: Vec<(String, Vec<i64>)>,
    /// The fixed inputs the model kept (an export may drop unused ones).
    inputs: Vec<String>,
}

impl Layout {
    fn read(dir: &Path) -> Option<Self> {
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join(NPU_SHAPE)).ok()?).ok()?;
        let number = |key: &str| {
            value
                .get(key)?
                .as_u64()
                .and_then(|n| usize::try_from(n).ok())
        };
        let memory = value
            .get("memory")?
            .as_array()?
            .iter()
            .map(|slot| {
                let name = slot.get("name")?.as_str()?.to_owned();
                let dims = slot
                    .get("shape")?
                    .as_array()?
                    .iter()
                    .map(serde_json::Value::as_i64)
                    .collect::<Option<Vec<i64>>>()?;
                let plain = name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_');
                (plain && !dims.is_empty() && dims.iter().all(|d| (1..=1 << 20).contains(d)))
                    .then_some((name, dims))
            })
            .collect::<Option<Vec<_>>>()?;
        let layout = Self {
            prompt_block: number("prompt_block")?,
            context: number("context")?,
            kernel: number("kernel")?,
            memory,
            inputs: match value.get("inputs") {
                Some(list) => list
                    .as_array()?
                    .iter()
                    .map(|name| name.as_str().map(str::to_owned))
                    .collect::<Option<Vec<_>>>()?,
                None => FIXED_INPUTS.iter().map(|name| (*name).to_owned()).collect(),
            },
        };
        (layout.prompt_block > 1
            && layout.prompt_block <= 256
            && layout.context >= layout.prompt_block
            && layout.context <= 1 << 16
            && layout.kernel <= 16
            && !layout.memory.is_empty())
        .then_some(layout)
    }
}

/// Byte-level BPE decoding from a `tokenizer.json`.
struct Decoder {
    pieces: Vec<Option<Vec<u8>>>,
    stop: Vec<i64>,
}

impl Decoder {
    fn read(dir: &Path) -> Option<Self> {
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("tokenizer.json")).ok()?).ok()?;
        let to_byte = unicode_to_byte();
        let mut pieces: Vec<Option<Vec<u8>>> = Vec::new();
        let mut put = |id: usize, piece: Option<Vec<u8>>| {
            if id >= pieces.len() {
                pieces.resize(id + 1, None);
            }
            pieces[id] = piece;
        };
        for (token, id) in value.pointer("/model/vocab")?.as_object()? {
            let id = usize::try_from(id.as_u64()?).ok()?;
            if id > 1 << 22 {
                return None;
            }
            let bytes = token
                .chars()
                .map(|c| to_byte.get(&c).copied())
                .collect::<Option<Vec<u8>>>();
            put(id, bytes);
        }
        let mut stop = Vec::new();
        for added in value.get("added_tokens")?.as_array()? {
            let id = usize::try_from(added.get("id")?.as_u64()?).ok()?;
            if id > 1 << 22 {
                return None;
            }
            let content = added.get("content")?.as_str()?;
            if matches!(content, "<|im_end|>" | "<|endoftext|>") {
                stop.push(i64::try_from(id).ok()?);
            }
            let special = added.get("special").and_then(serde_json::Value::as_bool) == Some(true);
            put(id, (!special).then(|| content.as_bytes().to_vec()));
        }
        (!stop.is_empty()).then_some(Self { pieces, stop })
    }

    fn decode(&self, tokens: &[i64]) -> String {
        let mut bytes = Vec::new();
        for &token in tokens {
            if let Some(Some(piece)) = usize::try_from(token).ok().and_then(|t| self.pieces.get(t))
            {
                bytes.extend_from_slice(piece);
            }
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

/// The byte-level alphabet: every byte is shown as one printable character.
fn unicode_to_byte() -> HashMap<char, u8> {
    let printable = |b: u32| {
        (0x21..=0x7E).contains(&b) || (0xA1..=0xAC).contains(&b) || (0xAE..=0xFF).contains(&b)
    };
    let mut map = HashMap::with_capacity(256);
    let mut extra = 0u32;
    for b in 0u32..256 {
        let shown = if printable(b) {
            b
        } else {
            extra += 1;
            255 + extra
        };
        if let (Some(c), Ok(byte)) = (char::from_u32(shown), u8::try_from(b)) {
            map.insert(c, byte);
        }
    }
    map
}

/// One compiled piece and its request.
struct Piece {
    request: *mut c_void,
    block: usize,
}

/// One slot of memory between calls.
struct Slot {
    name: CString,
    output: CString,
    front: *mut c_void,
    /// Recurrent state is replaced whole each call: the call writes into
    /// `back`, which then becomes `front`. Keys and values have no `back`.
    back: Option<*mut c_void>,
    /// Keys and values: (heads, context, head size) for row copies.
    rows: Option<(usize, usize, usize)>,
}

/// A fixed-shape model loaded on one device.
pub struct StaticLlm<'runtime> {
    api: Api,
    core: *mut c_void,
    compiled: Vec<*mut c_void>,
    contexts: Vec<*mut c_void>,
    requests: Vec<*mut c_void>,
    tensors: Vec<*mut c_void>,
    prompt: Piece,
    /// The optional one-token piece; without it single tokens use the block
    /// piece, which costs little more and keeps one copy of the weights.
    step: Option<Piece>,
    encoder: *mut c_void,
    slots: Vec<Slot>,
    layout: Layout,
    decoder: Decoder,
    length: usize,
    /// The system prompt computed once: its text, length and memory.
    prefix: Option<(String, usize, Vec<Vec<u8>>)>,
    /// For chat: the text whose tokens the memory holds, so a follow-up
    /// question only feeds what is new.
    transcript: Option<String>,
    /// The model's choice after each position of the last call.
    choices: Vec<i64>,
    /// Whether the pack reports those choices, so tokens can be guessed.
    guesses: bool,
    /// The MTP layer's draft after each position of the last call, when the
    /// pack runs one (`drafts` output).
    drafts: Vec<i64>,
    drafting: bool,
    _runtime: PhantomData<&'runtime Runtime>,
}

impl<'runtime> StaticLlm<'runtime> {
    /// Whether `dir` holds a fixed-shape pack.
    pub fn is_pack(dir: &Path) -> bool {
        dir.join(NPU_SHAPE).is_file()
    }

    /// Compile the pack's pieces on `device` and its tokenizer on the CPU.
    pub fn load(
        runtime: &'runtime Runtime,
        dir: &Path,
        device: &str,
        cache_dir: &Path,
    ) -> Result<Self, GenAiError> {
        let layout = Layout::read(dir).ok_or(GenAiError::Load)?;
        let decoder = Decoder::read(dir).ok_or(GenAiError::Load)?;
        let api = Api::load(runtime)?;
        let mut core: *mut c_void = std::ptr::null_mut();
        // SAFETY: valid out-pointer; on success `core` is owned by `model`.
        if unsafe { (api.core_create)(&mut core) } != OK || core.is_null() {
            return Err(GenAiError::Runtime);
        }
        let mut model = Self {
            api,
            core,
            compiled: Vec::new(),
            contexts: Vec::new(),
            requests: Vec::new(),
            tensors: Vec::new(),
            prompt: Piece {
                request: std::ptr::null_mut(),
                block: layout.prompt_block,
            },
            step: None,
            encoder: std::ptr::null_mut(),
            slots: Vec::new(),
            layout,
            decoder,
            length: 0,
            prefix: None,
            transcript: None,
            choices: Vec::new(),
            guesses: true,
            drafts: Vec::new(),
            drafting: true,
            _runtime: PhantomData,
        };
        let extension = cstring(&runtime.lib_dir().join("libopenvino_tokenizers.so"))?;
        // SAFETY: `core` is live; the path is a NUL-terminated string.
        if unsafe { (model.api.add_extension)(model.core, extension.as_ptr()) } != OK {
            return Err(GenAiError::Runtime);
        }
        let cache = cache_dir.to_str().ok_or(GenAiError::Load)?;
        let mut properties = vec![("CACHE_DIR", cache)];
        if device == "NPU" {
            // The runtime's own compiler: the driver's can be older than it.
            properties.push(("NPU_COMPILER_TYPE", "PLUGIN"));
        }
        let prompt = model.compile(&dir.join("prompt.xml"), device, &properties)?;
        model.prompt.request = model.request(prompt)?;
        if dir.join("step.xml").is_file() {
            let step = model.compile(&dir.join("step.xml"), device, &properties)?;
            model.step = Some(Piece {
                request: model.request(step)?,
                block: 1,
            });
        }
        let encoder = model.compile(&dir.join("openvino_tokenizer.xml"), "CPU", &[])?;
        model.encoder = model.request(encoder)?;
        model.allocate(prompt)?;
        Ok(model)
    }

    fn compile(
        &mut self,
        path: &Path,
        device: &str,
        properties: &[(&str, &str)],
    ) -> Result<*mut c_void, GenAiError> {
        let path = cstring(path)?;
        let device = CString::new(device).map_err(|_| GenAiError::Load)?;
        let owned: Vec<(CString, CString)> = properties
            .iter()
            .map(|(k, v)| Ok((CString::new(*k)?, CString::new(*v)?)))
            .collect::<Result<_, std::ffi::NulError>>()
            .map_err(|_| GenAiError::Load)?;
        let mut compiled: *mut c_void = std::ptr::null_mut();
        let (p, d, out, compile) = (
            path.as_ptr(),
            device.as_ptr(),
            &mut compiled,
            self.api.compile,
        );
        // SAFETY: `core` is live; every string outlives the call; the
        // variadic arguments are `2 * pairs` NUL-terminated strings.
        let status = unsafe {
            match owned.as_slice() {
                [] => compile(self.core, p, d, 0, out),
                [(k1, v1)] => compile(self.core, p, d, 2, out, k1.as_ptr(), v1.as_ptr()),
                [(k1, v1), (k2, v2)] => compile(
                    self.core,
                    p,
                    d,
                    4,
                    out,
                    k1.as_ptr(),
                    v1.as_ptr(),
                    k2.as_ptr(),
                    v2.as_ptr(),
                ),
                _ => return Err(GenAiError::Load),
            }
        };
        if status != OK || compiled.is_null() {
            return Err(GenAiError::Load);
        }
        self.compiled.push(compiled);
        Ok(compiled)
    }

    fn request(&mut self, compiled: *mut c_void) -> Result<*mut c_void, GenAiError> {
        let mut request: *mut c_void = std::ptr::null_mut();
        // SAFETY: `compiled` is live; valid out-pointer.
        if unsafe { (self.api.create_request)(compiled, &mut request) } != OK || request.is_null() {
            return Err(GenAiError::Load);
        }
        self.requests.push(request);
        Ok(request)
    }

    /// Allocate the memory slots where the device can read them in place
    /// (host memory it shares, when it offers that), and bind them.
    fn allocate(&mut self, compiled: *mut c_void) -> Result<(), GenAiError> {
        let mut context: *mut c_void = std::ptr::null_mut();
        // SAFETY: `compiled` is live; valid out-pointer.
        let shared =
            unsafe { (self.api.get_context)(compiled, &mut context) } == OK && !context.is_null();
        if shared {
            self.contexts.push(context);
        }
        let slots: Vec<(String, Vec<i64>)> = self.layout.memory.clone();
        for (name, dims) in slots {
            let rolling = name.starts_with("conv.") || name.starts_with("state.");
            let front = self.tensor(shared.then_some(context), &dims)?;
            let back = if rolling {
                Some(self.tensor(shared.then_some(context), &dims)?)
            } else {
                None
            };
            let rows = (!rolling && dims.len() == 3).then(|| {
                let d = |i: usize| usize::try_from(dims[i]).unwrap_or(0);
                (d(0), d(1), d(2))
            });
            if !rolling && rows.is_none_or(|(_, context, _)| context != self.layout.context) {
                return Err(GenAiError::Load);
            }
            self.slots.push(Slot {
                output: CString::new(format!("new_{name}")).map_err(|_| GenAiError::Load)?,
                name: CString::new(name).map_err(|_| GenAiError::Load)?,
                front,
                back,
                rows,
            });
        }
        self.bind()?;
        self.clear();
        Ok(())
    }

    fn tensor(
        &mut self,
        context: Option<*mut c_void>,
        dims: &[i64],
    ) -> Result<*mut c_void, GenAiError> {
        let shape = Shape {
            rank: i64::try_from(dims.len()).map_err(|_| GenAiError::Load)?,
            dims: dims.as_ptr(),
        };
        let mut tensor: *mut c_void = std::ptr::null_mut();
        // SAFETY: `dims` outlives the call (the shape is copied); valid
        // out-pointer; the context, when given, is live.
        let status = unsafe {
            match context {
                Some(context) => (self.api.host_tensor)(context, ELEMENT_F16, shape, &mut tensor),
                None => (self.api.tensor_create)(ELEMENT_F16, shape, &mut tensor),
            }
        };
        if status != OK || tensor.is_null() {
            // A device without shared host memory gets ordinary tensors.
            if context.is_some() {
                return self.tensor(None, dims);
            }
            return Err(GenAiError::Load);
        }
        self.tensors.push(tensor);
        Ok(tensor)
    }

    /// Point both pieces at the current buffers.
    fn bind(&self) -> Result<(), GenAiError> {
        let requests =
            std::iter::once(self.prompt.request).chain(self.step.as_ref().map(|s| s.request));
        for request in requests {
            for slot in &self.slots {
                // SAFETY: request and tensors are live; names are C strings.
                unsafe {
                    if (self.api.set_tensor)(request, slot.name.as_ptr(), slot.front) != OK {
                        return Err(GenAiError::Generate);
                    }
                    if let Some(back) = slot.back
                        && (self.api.set_tensor)(request, slot.output.as_ptr(), back) != OK
                    {
                        return Err(GenAiError::Generate);
                    }
                }
            }
        }
        Ok(())
    }

    /// Work on the bytes of a live tensor.
    fn with_bytes<T>(
        &self,
        tensor: *mut c_void,
        use_it: impl FnOnce(&mut [u8]) -> Result<T, GenAiError>,
    ) -> Result<T, GenAiError> {
        let mut data: *mut c_void = std::ptr::null_mut();
        let mut size = 0usize;
        // SAFETY: `tensor` is live; valid out-pointers.
        let found = unsafe {
            (self.api.tensor_data)(tensor, &mut data) == OK
                && (self.api.tensor_bytes)(tensor, &mut size) == OK
        };
        if !found || data.is_null() {
            return Err(GenAiError::Generate);
        }
        // SAFETY: the tensor owns `size` bytes at `data` while it lives, and
        // the slice does not outlive this call. No other slice of the same
        // tensor is alive: callers nest only distinct tensors.
        use_it(unsafe { std::slice::from_raw_parts_mut(data.cast::<u8>(), size) })
    }

    /// Write one fixed input, when the model has it.
    fn put(
        &self,
        request: *mut c_void,
        name: &str,
        write: impl FnOnce(&mut [u8]) -> Result<(), GenAiError>,
    ) -> Result<(), GenAiError> {
        if self.layout.inputs.iter().any(|input| input == name) {
            self.with_tensor(request, name, write)
        } else {
            Ok(())
        }
    }

    /// A request's own tensor by name, freed after `use_it`.
    fn with_tensor<T>(
        &self,
        request: *mut c_void,
        name: &str,
        use_it: impl FnOnce(&mut [u8]) -> Result<T, GenAiError>,
    ) -> Result<T, GenAiError> {
        let name = CString::new(name).map_err(|_| GenAiError::Generate)?;
        let mut tensor: *mut c_void = std::ptr::null_mut();
        // SAFETY: request is live; the name is a C string; valid out-pointer.
        if unsafe { (self.api.get_tensor)(request, name.as_ptr(), &mut tensor) } != OK
            || tensor.is_null()
        {
            return Err(GenAiError::Generate);
        }
        let result = self.with_bytes(tensor, use_it);
        // SAFETY: `tensor` was returned above and is freed once.
        unsafe { (self.api.tensor_free)(tensor) };
        result
    }

    fn clear(&mut self) {
        for slot in &self.slots {
            for tensor in std::iter::once(slot.front).chain(slot.back) {
                let _ = self.with_bytes(tensor, |bytes| {
                    bytes.fill(0);
                    Ok(())
                });
            }
        }
        self.length = 0;
    }

    /// Run one piece over `tokens` (at most its block) and return the
    /// token it chose next.
    fn call(&mut self, tokens: &[i64]) -> Result<i64, GenAiError> {
        let n = tokens.len();
        let piece = match &self.step {
            Some(step) if n == 1 => step,
            _ => &self.prompt,
        };
        let (request, block) = (piece.request, piece.block);
        let context = self.layout.context;
        if n == 0 || n > block || self.length + n > context {
            return Err(GenAiError::Generate);
        }
        let length = self.length;
        let put_i64 = |bytes: &mut [u8], values: &mut dyn Iterator<Item = i64>| {
            for (chunk, value) in bytes.chunks_exact_mut(8).zip(values) {
                chunk.copy_from_slice(&value.to_ne_bytes());
            }
            Ok(())
        };
        let put_f32 = |bytes: &mut [u8], values: &mut dyn Iterator<Item = f32>| {
            for (chunk, value) in bytes.chunks_exact_mut(4).zip(values) {
                chunk.copy_from_slice(&value.to_ne_bytes());
            }
            Ok(())
        };
        let to_i64 = |v: usize| i64::try_from(v).unwrap_or(i64::MAX);
        self.put(request, "ids", |b| {
            put_i64(
                b,
                &mut (0..block).map(|t| tokens.get(t).copied().unwrap_or(0)),
            )
        })?;
        self.put(request, "pos", |b| {
            put_i64(b, &mut (0..block).map(|t| to_i64(length + t)))
        })?;
        self.put(request, "valid", |b| {
            put_f32(b, &mut (0..block).map(|t| if t < n { 1.0 } else { 0.0 }))
        })?;
        self.put(request, "conv_idx", |b| {
            put_i64(b, &mut (0..self.layout.kernel).map(|k| to_i64(n + k)))
        })?;
        self.put(request, "last", |b| {
            put_i64(b, &mut std::iter::once(to_i64(n - 1)))
        })?;
        let width = context + block;
        self.put(request, "mask", |b| {
            put_f32(
                b,
                &mut (0..block * width).map(|i| {
                    let (row, col) = (i / width, i % width);
                    let open = if col < context {
                        col < length
                    } else {
                        col - context <= row.min(n - 1)
                    };
                    if open { 0.0 } else { MASKED }
                }),
            )
        })?;
        // SAFETY: the request is live and every input is bound.
        if unsafe { (self.api.infer)(request) } != OK {
            return Err(GenAiError::Generate);
        }
        // New key and value rows go into the one buffer per layer.
        for slot in &self.slots {
            let Some((heads, context, size)) = slot.rows else {
                continue;
            };
            let output = slot.output.to_str().map_err(|_| GenAiError::Generate)?;
            let front = slot.front;
            self.with_tensor(request, output, |rows| {
                self.with_bytes(front, |target| {
                    let row = size * F16_BYTES;
                    for head in 0..heads {
                        let from = head * block * row;
                        let to = (head * context + length) * row;
                        let (Some(source), Some(dest)) = (
                            rows.get(from..from + n * row),
                            target.get_mut(to..to + n * row),
                        ) else {
                            return Err(GenAiError::Generate);
                        };
                        dest.copy_from_slice(source);
                    }
                    Ok(())
                })
            })?;
        }
        // The state just written becomes the next input.
        for slot in &mut self.slots {
            if let Some(back) = slot.back.as_mut() {
                std::mem::swap(&mut slot.front, back);
            }
        }
        self.bind()?;
        self.length += n;
        // The choice after every position, for checking guessed tokens. A
        // pack exported without it simply does not guess.
        self.choices.clear();
        if self.guesses {
            let mut choices = std::mem::take(&mut self.choices);
            let read = self.with_tensor(request, "tokens", |b| {
                choices.extend(
                    b.chunks_exact(8)
                        .filter_map(|c| c.try_into().ok().map(i64::from_ne_bytes)),
                );
                Ok(())
            });
            self.choices = choices;
            if read.is_err() {
                self.guesses = false;
            }
        }
        self.drafts.clear();
        if self.guesses && self.drafting {
            let mut drafts = std::mem::take(&mut self.drafts);
            let read = self.with_tensor(request, "drafts", |b| {
                drafts.extend(
                    b.chunks_exact(8)
                        .filter_map(|c| c.try_into().ok().map(i64::from_ne_bytes)),
                );
                Ok(())
            });
            self.drafts = drafts;
            if read.is_err() {
                self.drafting = false;
            }
        }
        self.with_tensor(request, "token", |b| {
            b.get(..8)
                .and_then(|b| b.try_into().ok())
                .map(i64::from_ne_bytes)
                .ok_or(GenAiError::Generate)
        })
    }

    fn feed(&mut self, tokens: &[i64]) -> Result<i64, GenAiError> {
        let mut next = -1;
        for chunk in tokens.chunks(self.layout.prompt_block) {
            next = self.call(chunk)?;
        }
        Ok(next)
    }

    fn encode(&self, text: &str) -> Result<Vec<i64>, GenAiError> {
        let text = CString::new(text).map_err(|_| GenAiError::Generate)?;
        let strings = [text.as_ptr()];
        let dims = [1i64];
        let shape = Shape {
            rank: 1,
            dims: dims.as_ptr(),
        };
        let mut input: *mut c_void = std::ptr::null_mut();
        // SAFETY: one live C string, a one-element shape, valid out-pointer.
        if unsafe { (self.api.string_tensor)(strings.as_ptr(), 1, shape, &mut input) } != OK
            || input.is_null()
        {
            return Err(GenAiError::Generate);
        }
        // SAFETY: request and tensor are live; the tensor is freed once.
        let ran = unsafe {
            let ok = (self.api.set_input)(self.encoder, 0, input) == OK
                && (self.api.infer)(self.encoder) == OK;
            (self.api.tensor_free)(input);
            ok
        };
        if !ran {
            return Err(GenAiError::Generate);
        }
        self.with_tensor(self.encoder, "input_ids", |bytes| {
            Ok(bytes
                .chunks_exact(8)
                .filter_map(|c| c.try_into().ok().map(i64::from_ne_bytes))
                .collect())
        })
    }

    /// Save the recurrent state. Keys and values need no saving: calls
    /// only write rows at or past the current length, so the rows before
    /// it stay as they were until the memory is cleared.
    fn snapshot(&self) -> Result<Vec<Vec<u8>>, GenAiError> {
        self.slots
            .iter()
            .filter(|slot| slot.back.is_some())
            .map(|slot| self.with_bytes(slot.front, |b| Ok(b.to_vec())))
            .collect()
    }

    fn restore(&mut self, length: usize, saved: &[Vec<u8>]) -> Result<(), GenAiError> {
        let rolling = self.slots.iter().filter(|slot| slot.back.is_some());
        for (slot, bytes) in rolling.zip(saved) {
            self.with_bytes(slot.front, |target| {
                if target.len() != bytes.len() {
                    return Err(GenAiError::Generate);
                }
                target.copy_from_slice(bytes);
                Ok(())
            })?;
        }
        self.length = length;
        Ok(())
    }

    /// Answer one system prompt and user message, greedily, with at most
    /// `max_new` tokens. `stop_now` is asked before each token.
    ///
    /// `source` is the dictated text a clean-up mostly repeats: tokens are
    /// guessed ahead from it (empty: no guessing).
    pub fn generate(
        &mut self,
        system: &str,
        user: &str,
        source: &str,
        max_new: usize,
        stop_now: &mut dyn FnMut() -> bool,
    ) -> Result<String, GenAiError> {
        let head = format!("<|im_start|>system\n{system}<|im_end|>\n");
        self.start_from(&head)?;
        let rest = format!(
            "<|im_start|>user\n{user}<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n"
        );
        let tokens = self.encode(&rest)?;
        if self.length + tokens.len() >= self.layout.context {
            return Err(GenAiError::Generate);
        }
        // Feed whole blocks; the rest of the prompt goes into the first
        // decoding call, which can already check a guess of the reply.
        let block = self.layout.prompt_block;
        let cut = tokens.len().saturating_sub(1) / block * block;
        self.feed(&tokens[..cut])?;
        let source = if self.guesses && !source.is_empty() {
            self.encode(source)?
        } else {
            Vec::new()
        };
        let out = self.decode_guessing(&tokens[cut..], &source, true, max_new, stop_now, None)?;
        Ok(self.decoder.decode(&out))
    }

    /// Turn guessing ahead on or off (on by default when the pack allows).
    pub fn set_guessing(&mut self, on: bool) {
        self.guesses = on;
    }

    /// Put the memory at the end of `head` (a system prompt), computing it
    /// once and restoring it afterwards; an empty head starts empty.
    fn start_from(&mut self, head: &str) -> Result<(), GenAiError> {
        self.transcript = None;
        if head.is_empty() {
            self.clear();
            return Ok(());
        }
        if self.prefix.as_ref().is_none_or(|(text, _, _)| text != head) {
            self.clear();
            let tokens = self.encode(head)?;
            self.feed(&tokens)?;
            self.prefix = Some((head.to_owned(), self.length, self.snapshot()?));
        }
        let Some((text, length, saved)) = self.prefix.take() else {
            return Err(GenAiError::Generate);
        };
        let restored = self.restore(length, &saved);
        self.prefix = Some((text, length, saved));
        restored
    }

    /// Answer a conversation (`role`, `content` pairs, an optional system
    /// message first) greedily, streaming text to `on_text` (which returns
    /// true to stop). Earlier turns stay in memory, so a follow-up only
    /// feeds what is new; the oldest turns are left out when the
    /// conversation outgrows the context.
    pub fn chat(
        &mut self,
        messages: &[(String, String)],
        max_new: usize,
        stop_now: &mut dyn FnMut() -> bool,
        on_text: &mut dyn FnMut(&str) -> bool,
    ) -> Result<String, GenAiError> {
        let context = self.layout.context;
        let reserve = max_new.min(context / 2);
        let (system, turns) = match messages.split_first() {
            Some(((role, content), rest)) if role == "system" => (Some(content.as_str()), rest),
            _ => (None, messages),
        };
        let head = system.map_or(String::new(), |s| {
            format!("<|im_start|>system\n{s}<|im_end|>\n")
        });
        let mut first = 0;
        let (prompt, length) = loop {
            let prompt = render(&head, turns.get(first..).unwrap_or_default());
            let length = self.encode(&prompt)?.len();
            if length + reserve <= context || first + 1 >= turns.len() {
                break (prompt, length);
            }
            first += 1;
        };
        if length >= context {
            return Err(GenAiError::Generate);
        }
        let new_tokens = match self.transcript.take() {
            Some(held) if prompt.starts_with(&held) => self.encode(&prompt[held.len()..])?,
            _ => {
                self.start_from(&head)?;
                self.encode(&prompt[head.len()..])?
            }
        };
        if new_tokens.is_empty() || self.length + new_tokens.len() >= context {
            return Err(GenAiError::Generate);
        }
        let max_new = max_new.min(context - self.length - new_tokens.len());
        let block = self.layout.prompt_block;
        let cut = new_tokens.len().saturating_sub(1) / block * block;
        self.feed(&new_tokens[..cut])?;
        // Guess from the conversation: replies often quote it.
        let source = self.encode(&prompt)?;
        let out = self.decode_guessing(
            &new_tokens[cut..],
            &source,
            false,
            max_new,
            stop_now,
            Some(on_text),
        )?;
        let reply = self.decoder.decode(&out);
        self.transcript = Some(format!("{prompt}{reply}"));
        Ok(reply)
    }
}

/// A conversation in the chat format: each turn, then an open assistant
/// turn with reasoning off. Earlier assistant turns are written the way
/// they were generated, so the memory of them can be reused.
fn render(head: &str, turns: &[(String, String)]) -> String {
    let mut text = head.to_owned();
    for (role, content) in turns {
        let opening = if role == "assistant" {
            "<think>\n\n</think>\n\n"
        } else {
            ""
        };
        text.push_str(&format!(
            "<|im_start|>{role}\n{opening}{content}<|im_end|>\n"
        ));
    }
    text.push_str("<|im_start|>assistant\n<think>\n\n</think>\n\n");
    text
}

impl StaticLlm<'_> {
    /// Greedy decoding that guesses ahead from `source` (the dictated text,
    /// which a clean-up mostly repeats). Each call feeds the next token and
    /// a guess of what follows; every guessed token the model agrees with is
    /// kept, so a call yields several tokens instead of one.
    ///
    /// Keys and values past the agreed tokens are simply not counted.
    /// Recurrent state cannot be rewound, so for models that have it a call
    /// starts from the last state known good, and tokens agreed since then
    /// ride at the front of the next call with its guess: no call is spent
    /// only on rolling back.
    fn decode_guessing(
        &mut self,
        tail: &[i64],
        source: &[i64],
        open_from_source: bool,
        max_new: usize,
        stop_now: &mut dyn FnMut() -> bool,
        mut on_text: Option<&mut dyn FnMut(&str) -> bool>,
    ) -> Result<Vec<i64>, GenAiError> {
        let mut shown = 0usize;
        let block = self.layout.prompt_block;
        let context = self.layout.context;
        let rolling = self.slots.iter().any(|slot| slot.back.is_some());
        let mut good = if rolling {
            Some((self.length, self.snapshot()?))
        } else {
            None
        };
        // The end of the prompt is fed with the first call: its last token
        // is `token`, the rest rides as pending. Nothing of it is output.
        let (&last, before) = tail.split_last().ok_or(GenAiError::Generate)?;
        let mut pending: Vec<i64> = before.to_vec();
        let mut out: Vec<i64> = Vec::new();
        let mut token = last;
        let mut in_prompt = true;
        // The MTP layer's draft for the token after `token`, when it runs.
        let mut draft: Option<i64> = None;
        while (in_prompt || out.len() < max_new && !self.decoder.stop.contains(&token))
            && !stop_now()
        {
            let produced = usize::from(!in_prompt);
            let room = (block - 1)
                .saturating_sub(pending.len())
                .min(max_new.saturating_sub(out.len() + produced))
                .min(context.saturating_sub(self.length + pending.len() + 1));
            if self.length + pending.len() + 1 > context {
                if !in_prompt {
                    out.push(token);
                }
                break;
            }
            let guess: Vec<i64> = if !self.guesses || room == 0 {
                Vec::new()
            } else if in_prompt {
                // A clean-up usually opens the way the dictation does.
                if open_from_source {
                    lookup(source, &[], room).to_vec()
                } else {
                    Vec::new()
                }
            } else {
                let mut recent = out.clone();
                recent.push(token);
                let mut guess = Vec::new();
                // The MTP draft first, then what followed it in the source.
                if let Some(draft) = draft {
                    guess.push(draft);
                    recent.push(draft);
                }
                guess.extend_from_slice(lookup(source, &recent, room - guess.len()));
                guess
            };
            let mut fed = pending.clone();
            fed.push(token);
            fed.extend_from_slice(&guess);
            let chosen = self.call(&fed)?;
            let choices: Vec<i64> = if guess.is_empty() {
                vec![chosen]
            } else {
                self.choices
                    .get(pending.len()..)
                    .unwrap_or_default()
                    .to_vec()
            };
            let agreed = guess
                .iter()
                .zip(&choices)
                .take_while(|(guessed, chose)| {
                    guessed == chose && !self.decoder.stop.contains(guessed)
                })
                .count();
            // Read before any settling call overwrites it.
            let next_draft = self.drafts.get(pending.len() + agreed).copied();
            if !in_prompt {
                out.push(token);
            }
            in_prompt = false;
            out.extend_from_slice(&guess[..agreed]);
            if agreed == guess.len() {
                pending.clear();
                if rolling {
                    good = Some((self.length, self.snapshot()?));
                }
            } else if let Some((length, saved)) = good.take() {
                pending.push(token);
                pending.extend_from_slice(&guess[..agreed]);
                self.restore(length, &saved)?;
                good = Some((length, saved));
                if pending.len() + 1 >= block {
                    // Too many to carry: settle them now.
                    self.feed(&pending)?;
                    pending.clear();
                    good = Some((self.length, self.snapshot()?));
                }
            } else {
                self.length -= guess.len() - agreed;
            }
            token = *choices.get(agreed).ok_or(GenAiError::Generate)?;
            draft = next_draft;
            // Stream what is new, once it ends on a whole character.
            if let Some(on_text) = on_text.as_mut() {
                let text = self.decoder.decode(&out);
                if text.len() > shown && !text.ends_with('\u{FFFD}') {
                    let stop = text.get(shown..).is_some_and(&mut **on_text);
                    shown = text.len();
                    if stop {
                        break;
                    }
                }
            }
        }
        if !pending.is_empty() {
            // Leave the state consistent for whoever reads it next.
            self.feed(&pending)?;
        }
        out.truncate(max_new);
        Ok(out)
    }
}

/// What followed the latest place `source` holds the end of `recent`; the
/// longest match of the last three, two or one tokens wins.
fn lookup<'a>(source: &'a [i64], recent: &[i64], most: usize) -> &'a [i64] {
    if recent.is_empty() {
        return &source[..most.min(source.len())];
    }
    for size in [3, 2, 1] {
        let Some(tail) = recent.len().checked_sub(size).map(|start| &recent[start..]) else {
            continue;
        };
        if let Some(start) = source.windows(size).rposition(|window| window == tail) {
            let from = start + size;
            return &source[from..(from + most).min(source.len())];
        }
    }
    &[]
}

impl Drop for StaticLlm<'_> {
    fn drop(&mut self) {
        // SAFETY: every handle was created by this model, is freed once,
        // and nothing uses it afterwards; order is the reverse of creation.
        unsafe {
            for tensor in self.tensors.drain(..) {
                (self.api.tensor_free)(tensor);
            }
            for request in self.requests.drain(..) {
                (self.api.request_free)(request);
            }
            for context in self.contexts.drain(..) {
                (self.api.context_free)(context);
            }
            for compiled in self.compiled.drain(..) {
                (self.api.compiled_free)(compiled);
            }
            (self.api.core_free)(self.core);
        }
    }
}

fn cstring(path: &Path) -> Result<CString, GenAiError> {
    CString::new(path.to_str().ok_or(GenAiError::Load)?).map_err(|_| GenAiError::Load)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_conversation_renders_so_its_earlier_turns_can_be_reused() {
        let head = "<|im_start|>system\nBe brief.<|im_end|>\n";
        let first = vec![("user".to_owned(), "Hi".to_owned())];
        let opened = render(head, &first);
        assert_eq!(
            opened,
            "<|im_start|>system\nBe brief.<|im_end|>\n<|im_start|>user\nHi<|im_end|>\n\
             <|im_start|>assistant\n<think>\n\n</think>\n\n"
        );
        // The next turn's prompt starts with what was fed for this one plus
        // the reply, so only the new part is fed.
        let mut next = first.clone();
        next.push(("assistant".to_owned(), "Hello.".to_owned()));
        next.push(("user".to_owned(), "Bye".to_owned()));
        assert!(render(head, &next).starts_with(&format!("{opened}Hello.")));
    }

    #[test]
    fn guesses_follow_the_latest_match_of_the_longest_tail() {
        let source = [1, 2, 3, 4, 5, 2, 3, 9];
        // "2 3" last appears before 9; the three-token tail "1 2 3" before 4.
        assert_eq!(lookup(&source, &[2, 3], 4), &[9]);
        assert_eq!(lookup(&source, &[1, 2, 3], 2), &[4, 5]);
        assert_eq!(lookup(&source, &[7, 5], 3), &[2, 3, 9]);
        assert_eq!(lookup(&source, &[7], 3), &[] as &[i64]);
        assert_eq!(lookup(&[], &[1], 3), &[] as &[i64]);
        // Before any output, the guess is the start of the dictation.
        assert_eq!(lookup(&source, &[], 2), &[1, 2]);
    }

    #[test]
    fn the_byte_alphabet_covers_every_byte_once() {
        let map = unicode_to_byte();
        assert_eq!(map.len(), 256);
        assert_eq!(map.get(&'Ġ'), Some(&b' '));
        assert_eq!(map.get(&'Ċ'), Some(&b'\n'));
        assert_eq!(map.get(&'a'), Some(&b'a'));
    }

    #[test]
    fn a_layout_must_be_complete_and_bounded() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        let write = |text: &str| std::fs::write(dir.path().join(NPU_SHAPE), text);
        write(
            r#"{"prompt_block":16,"context":512,"kernel":3,"memory":[{"name":"key.3","shape":[2,512,256]}]}"#,
        )?;
        assert!(Layout::read(dir.path()).is_some());
        write(
            r#"{"prompt_block":16,"context":512,"kernel":3,"memory":[{"name":"../x","shape":[2]}]}"#,
        )?;
        assert!(Layout::read(dir.path()).is_none());
        write(r#"{"prompt_block":16,"context":8,"kernel":3,"memory":[{"name":"a","shape":[2]}]}"#)?;
        assert!(Layout::read(dir.path()).is_none());
        Ok(())
    }

    #[test]
    fn decoding_skips_special_tokens_and_joins_bytes() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        std::fs::write(
            dir.path().join("tokenizer.json"),
            r#"{"model":{"vocab":{"Hi":0,"Ġthere":1,"Ċ":2}},
               "added_tokens":[{"id":3,"content":"<|im_end|>","special":true}]}"#,
        )?;
        let decoder =
            Decoder::read(dir.path()).ok_or_else(|| std::io::Error::other("no decoder"))?;
        assert_eq!(decoder.decode(&[0, 1, 2, 3]), "Hi there\n");
        assert_eq!(decoder.stop, vec![3]);
        Ok(())
    }
}
