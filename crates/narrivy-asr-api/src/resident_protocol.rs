//! Private binary protocol between Narrivy and its process-isolated resident
//! speech workers (transcribe.cpp and OpenVINO). Engine-neutral: each
//! engine's worker speaks it, and one parent-side supervisor drives them.
//!
//! The protocol is deliberately tiny, length-bounded, and content-free except
//! for the request PCM and successful response text. Neither endpoint logs
//! frames. It is not IPC exposed to users or plugins.

use std::io::{Read, Write};

use crate::{
    AsrCapabilities, AsrRequest, AsrTag, ContextBias, MAX_ATTESTED_TAGS, MAX_CONTEXT_HINT_BYTES,
    MAX_CONTEXT_HINTS, MAX_TAG_BYTES, SpokenLanguage, WrittenOutput,
};

pub const MAGIC: [u8; 4] = *b"NRAS";
pub const VERSION: u16 = 5;
pub const HEADER_LEN: usize = 28;
/// One shared bound for every PCM frame accepted by this private protocol.
///
/// This is deliberately the portable documented dictation capture maximum
/// ([`narrivy_types::MAX_CAPTURE_SAMPLES`]), not a per-transport constant: the
/// daemon, the Linux capture adapter, this protocol and the resident worker all
/// account for the same finite frame budget. The wire format is unchanged (the
/// existing `u32` sample-count field); compatibility is a peer-capability
/// decision: a worker or parent built against a smaller bound rejects a larger
/// frame fail-closed (`invalid_audio` / request rejection) instead of
/// truncating it.
pub const MAX_SAMPLES: usize = narrivy_types::MAX_CAPTURE_SAMPLES;
pub const MAX_TEXT_BYTES: usize = 1024 * 1024;
/// Bounded PCM transfer unit shared by the parent and worker. Keeping this
/// independent of the utterance length avoids duplicate full-payload byte
/// allocations on both sides of the process boundary.
pub const PCM_TRANSFER_SAMPLES: usize = 4_096;
pub const PCM_TRANSFER_BYTES: usize = PCM_TRANSFER_SAMPLES * size_of::<f32>();
pub const CAPABILITY_HEADER_LEN: usize = 4;
pub const MAX_CAPABILITY_PAYLOAD_LEN: usize = CAPABILITY_HEADER_LEN
    + MAX_ATTESTED_TAGS * (1 + MAX_TAG_BYTES)
    + MAX_ATTESTED_TAGS * (1 + MAX_TAG_BYTES);
const CAPABILITY_STREAMING: u8 = 1 << 0;
const CAPABILITY_LANGUAGE_DETECT: u8 = 1 << 1;
const CAPABILITY_TRANSLATION: u8 = 1 << 2;
const CAPABILITY_WHISPER_RUN_OPTIONS: u8 = 1 << 3;
const KNOWN_CAPABILITY_BITS: u8 = CAPABILITY_STREAMING
    | CAPABILITY_LANGUAGE_DETECT
    | CAPABILITY_TRANSLATION
    | CAPABILITY_WHISPER_RUN_OPTIONS;
pub const RECOGNITION_OPTIONS_HEADER_LEN: usize = 5;
pub const MAX_RECOGNITION_OPTIONS_BYTES: usize = RECOGNITION_OPTIONS_HEADER_LEN
    + MAX_TAG_BYTES
    + MAX_TAG_BYTES
    + MAX_CONTEXT_HINTS * (1 + MAX_CONTEXT_HINT_BYTES);
const SPOKEN_UNSPECIFIED: u8 = 0;
const SPOKEN_AUTO_DETECT: u8 = 1;
const SPOKEN_EXPLICIT: u8 = 2;

pub const OP_DECODE: u8 = 1;
pub const OP_SHUTDOWN: u8 = 2;
pub const OP_STREAM_BEGIN: u8 = 3;
pub const OP_STREAM_FEED: u8 = 4;
pub const OP_STREAM_FINALIZE: u8 = 5;
pub const OP_STREAM_RESET: u8 = 6;

pub const STATUS_READY: u8 = 0;
pub const STATUS_OK: u8 = 1;
pub const STATUS_INVALID_AUDIO: u8 = 2;
pub const STATUS_DECODE_FAILED: u8 = 3;
pub const STATUS_MODEL_LOAD_FAILED: u8 = 4;
pub const STATUS_BACKEND_MISMATCH: u8 = 5;
pub const STATUS_SAMPLE_RATE_MISMATCH: u8 = 6;
pub const STATUS_CANCELLATION_UNSUPPORTED: u8 = 7;
pub const STATUS_MODEL_FORMAT_UNSUPPORTED: u8 = 8;
pub const STATUS_VERSION_MISMATCH: u8 = 9;
pub const STATUS_OUT_OF_MEMORY: u8 = 10;
pub const STATUS_NATIVE_BACKEND_FAILED: u8 = 11;
pub const STATUS_NATIVE_STARTUP_OTHER: u8 = 12;
pub const STATUS_STREAM_STARTED: u8 = 13;
pub const STATUS_STREAM_UPDATE: u8 = 14;
pub const STATUS_STREAM_FINAL: u8 = 15;
pub const STATUS_STREAM_RESET: u8 = 16;
pub const STATUS_STREAMING_UNSUPPORTED: u8 = 17;
pub const STATUS_MODEL_BINDING_FAILED: u8 = 18;
pub const STATUS_INVALID_REQUEST: u8 = 19;
pub const STATUS_UNSUPPORTED_REQUEST: u8 = 20;
pub const STREAM_SNAPSHOT_HEADER_LEN: usize = 44;

/// Native capabilities attested by the already-loaded model. These describe
/// the upstream model/runtime pair, not features composed or enabled by
/// Narrivy's daemon/UI pipeline. Written language, script, and locale are
/// omitted because this backend cannot apply them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkerCapabilities {
    /// The device the loaded model runs on.
    pub compute: WorkerCompute,
    pub supports_streaming: bool,
    pub supports_language_detect: bool,
    pub supports_translate: bool,
    pub accepts_whisper_run_options: bool,
    pub languages: Vec<AsrTag>,
    pub translate_target_languages: Vec<AsrTag>,
}

impl WorkerCapabilities {
    /// Whether two frames describe the same model, whatever device each
    /// worker loaded it on.
    #[must_use]
    pub fn same_model(&self, other: &Self) -> bool {
        Self {
            compute: other.compute,
            ..self.clone()
        } == *other
    }

    #[must_use]
    pub fn to_asr_capabilities(&self) -> AsrCapabilities {
        AsrCapabilities {
            streaming: self.supports_streaming,
            language_detection: self.supports_language_detect,
            translation: self.supports_translate,
            confidence: false,
            hotword_biasing: self.accepts_whisper_run_options,
            timestamps: false,
            languages: self.languages.clone(),
            translate_target_languages: self.translate_target_languages.clone(),
            scripts: Vec::new(),
            locales: Vec::new(),
        }
    }
}

/// What the parent asks the worker to run the model on.
///
/// The request is an optional leading worker argument. Without it the worker
/// is strict CPU and the capability frame's device byte is zero, exactly as
/// before the byte had a meaning, so the wire version is unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ComputeRequest {
    /// A GPU when this build has one that loads the model and passes the
    /// worker's self-test; the CPU otherwise.
    Auto,
    /// A GPU or nothing: the worker refuses to start when no GPU loads the
    /// model and passes the self-test.
    Gpu,
    /// An NPU or nothing, for workers whose runtime has one (OpenVINO). A
    /// worker without NPU support refuses to start.
    Npu,
    /// Strict CPU, the deterministic choice.
    #[default]
    Cpu,
}

impl ComputeRequest {
    pub const ARGUMENT: &'static str = "--compute";

    #[must_use]
    pub fn as_argument(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Gpu => "gpu",
            Self::Npu => "npu",
            Self::Cpu => "cpu",
        }
    }

    #[must_use]
    pub fn from_argument(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "gpu" => Some(Self::Gpu),
            "npu" => Some(Self::Npu),
            "cpu" => Some(Self::Cpu),
            _ => None,
        }
    }
}

/// The device class a ready worker attests its model runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WorkerCompute {
    #[default]
    Cpu,
    IntegratedGpu,
    DiscreteGpu,
    Npu,
}

impl WorkerCompute {
    /// Stable, content-free token for status reporting.
    #[must_use]
    pub fn as_token(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::IntegratedGpu => "igpu",
            Self::DiscreteGpu => "dgpu",
            Self::Npu => "npu",
        }
    }

    #[must_use]
    pub fn is_gpu(self) -> bool {
        matches!(self, Self::IntegratedGpu | Self::DiscreteGpu)
    }

    /// Any device other than the CPU. A failure on one drops the engine to
    /// the CPU.
    #[must_use]
    pub fn is_accelerated(self) -> bool {
        !matches!(self, Self::Cpu)
    }

    #[must_use]
    pub fn to_wire(self) -> u8 {
        match self {
            Self::Cpu => 0,
            Self::IntegratedGpu => 1,
            Self::DiscreteGpu => 2,
            Self::Npu => 3,
        }
    }

    #[must_use]
    pub fn from_wire(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Cpu),
            1 => Some(Self::IntegratedGpu),
            2 => Some(Self::DiscreteGpu),
            3 => Some(Self::Npu),
            _ => None,
        }
    }
}

/// One-shot decode settings this backend can apply. Output language, script,
/// and locale are intentionally absent: the worker cannot honor them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RecognitionOptions {
    pub spoken_language: SpokenLanguage,
    pub translation_target: Option<AsrTag>,
    pub context_bias: Option<ContextBias>,
}

impl RecognitionOptions {
    #[must_use]
    pub fn from_request(request: &AsrRequest) -> Self {
        Self {
            spoken_language: request.spoken_language.clone(),
            translation_target: request.translation_target.clone(),
            context_bias: request.context_bias.clone(),
        }
    }

    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    #[must_use]
    pub fn to_asr_request(&self) -> AsrRequest {
        AsrRequest {
            spoken_language: self.spoken_language.clone(),
            output: WrittenOutput::default(),
            translation_target: self.translation_target.clone(),
            locale: None,
            context_bias: self.context_bias.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamSnapshot {
    pub revision: i32,
    pub result_changed: bool,
    pub is_final: bool,
    pub committed_changed: bool,
    pub tentative_changed: bool,
    pub input_received_ms: i64,
    pub audio_committed_ms: i64,
    pub buffered_ms: i64,
    pub full: String,
    pub committed: String,
    pub tentative: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestHeader {
    pub operation: u8,
    pub request_id: u128,
    pub sample_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResponseHeader {
    pub status: u8,
    pub request_id: u128,
    pub payload_len: u32,
}

fn prefix(kind: u8) -> [u8; HEADER_LEN] {
    let mut bytes = [0_u8; HEADER_LEN];
    bytes[..4].copy_from_slice(&MAGIC);
    bytes[4..6].copy_from_slice(&VERSION.to_le_bytes());
    bytes[6] = kind;
    bytes
}

pub fn encode_request(header: RequestHeader) -> [u8; HEADER_LEN] {
    let mut bytes = prefix(header.operation);
    bytes[8..24].copy_from_slice(&header.request_id.to_le_bytes());
    bytes[24..28].copy_from_slice(&header.sample_count.to_le_bytes());
    bytes
}

pub fn decode_request(bytes: [u8; HEADER_LEN]) -> Result<RequestHeader, &'static str> {
    validate_prefix(&bytes)?;
    let request_id =
        u128::from_le_bytes(bytes[8..24].try_into().map_err(|_| "invalid_request_id")?);
    let sample_count = u32::from_le_bytes(
        bytes[24..28]
            .try_into()
            .map_err(|_| "invalid_sample_count")?,
    );
    let operation = bytes[6];
    if !matches!(
        operation,
        OP_DECODE
            | OP_SHUTDOWN
            | OP_STREAM_BEGIN
            | OP_STREAM_FEED
            | OP_STREAM_FINALIZE
            | OP_STREAM_RESET
    ) {
        return Err("invalid_operation");
    }
    let requires_no_audio = matches!(
        operation,
        OP_SHUTDOWN | OP_STREAM_BEGIN | OP_STREAM_FINALIZE | OP_STREAM_RESET
    );
    if requires_no_audio && sample_count != 0 {
        return Err("invalid_operation_payload");
    }
    if operation == OP_STREAM_FEED && sample_count == 0 {
        return Err("invalid_operation_payload");
    }
    if usize::try_from(sample_count).unwrap_or(usize::MAX) > MAX_SAMPLES {
        return Err("audio_too_large");
    }
    Ok(RequestHeader {
        operation,
        request_id,
        sample_count,
    })
}

pub fn encode_response(header: ResponseHeader) -> [u8; HEADER_LEN] {
    let mut bytes = prefix(header.status);
    bytes[8..24].copy_from_slice(&header.request_id.to_le_bytes());
    bytes[24..28].copy_from_slice(&header.payload_len.to_le_bytes());
    bytes
}

pub fn decode_response(bytes: [u8; HEADER_LEN]) -> Result<ResponseHeader, &'static str> {
    validate_prefix(&bytes)?;
    let status = bytes[6];
    if !matches!(
        status,
        STATUS_READY
            | STATUS_OK
            | STATUS_INVALID_AUDIO
            | STATUS_DECODE_FAILED
            | STATUS_MODEL_LOAD_FAILED
            | STATUS_BACKEND_MISMATCH
            | STATUS_SAMPLE_RATE_MISMATCH
            | STATUS_CANCELLATION_UNSUPPORTED
            | STATUS_MODEL_FORMAT_UNSUPPORTED
            | STATUS_VERSION_MISMATCH
            | STATUS_OUT_OF_MEMORY
            | STATUS_NATIVE_BACKEND_FAILED
            | STATUS_NATIVE_STARTUP_OTHER
            | STATUS_STREAM_STARTED
            | STATUS_STREAM_UPDATE
            | STATUS_STREAM_FINAL
            | STATUS_STREAM_RESET
            | STATUS_STREAMING_UNSUPPORTED
            | STATUS_MODEL_BINDING_FAILED
            | STATUS_INVALID_REQUEST
            | STATUS_UNSUPPORTED_REQUEST
    ) {
        return Err("invalid_status");
    }
    let request_id =
        u128::from_le_bytes(bytes[8..24].try_into().map_err(|_| "invalid_request_id")?);
    let payload_len = u32::from_le_bytes(
        bytes[24..28]
            .try_into()
            .map_err(|_| "invalid_payload_length")?,
    );
    if usize::try_from(payload_len).unwrap_or(usize::MAX) > MAX_TEXT_BYTES {
        return Err("response_too_large");
    }
    let valid_payload_shape = match status {
        STATUS_READY => usize::try_from(payload_len).ok().is_some_and(|length| {
            (CAPABILITY_HEADER_LEN..=MAX_CAPABILITY_PAYLOAD_LEN).contains(&length)
        }),
        STATUS_OK => true,
        STATUS_STREAM_UPDATE | STATUS_STREAM_FINAL => {
            usize::try_from(payload_len).is_ok_and(|length| length >= STREAM_SNAPSHOT_HEADER_LEN)
        }
        _ => payload_len == 0,
    };
    if !valid_payload_shape {
        return Err("invalid_response_payload_shape");
    }
    Ok(ResponseHeader {
        status,
        request_id,
        payload_len,
    })
}

pub fn encode_stream_snapshot(snapshot: &StreamSnapshot) -> Result<Vec<u8>, &'static str> {
    let full = snapshot.full.as_bytes();
    let committed = snapshot.committed.as_bytes();
    let tentative = snapshot.tentative.as_bytes();
    let total = STREAM_SNAPSHOT_HEADER_LEN
        .checked_add(full.len())
        .and_then(|length| length.checked_add(committed.len()))
        .and_then(|length| length.checked_add(tentative.len()))
        .ok_or("stream_snapshot_too_large")?;
    if total > MAX_TEXT_BYTES {
        return Err("stream_snapshot_too_large");
    }
    let full_len = u32::try_from(full.len()).map_err(|_| "stream_snapshot_too_large")?;
    let committed_len = u32::try_from(committed.len()).map_err(|_| "stream_snapshot_too_large")?;
    let tentative_len = u32::try_from(tentative.len()).map_err(|_| "stream_snapshot_too_large")?;
    let mut payload = Vec::with_capacity(total);
    payload.extend_from_slice(&snapshot.revision.to_le_bytes());
    let flags = u8::from(snapshot.result_changed)
        | (u8::from(snapshot.is_final) << 1)
        | (u8::from(snapshot.committed_changed) << 2)
        | (u8::from(snapshot.tentative_changed) << 3);
    payload.push(flags);
    payload.extend_from_slice(&[0_u8; 3]);
    payload.extend_from_slice(&snapshot.input_received_ms.to_le_bytes());
    payload.extend_from_slice(&snapshot.audio_committed_ms.to_le_bytes());
    payload.extend_from_slice(&snapshot.buffered_ms.to_le_bytes());
    payload.extend_from_slice(&full_len.to_le_bytes());
    payload.extend_from_slice(&committed_len.to_le_bytes());
    payload.extend_from_slice(&tentative_len.to_le_bytes());
    payload.extend_from_slice(full);
    payload.extend_from_slice(committed);
    payload.extend_from_slice(tentative);
    Ok(payload)
}

pub fn decode_stream_snapshot(payload: &[u8]) -> Result<StreamSnapshot, &'static str> {
    if payload.len() < STREAM_SNAPSHOT_HEADER_LEN {
        return Err("stream_snapshot_truncated");
    }
    let revision = i32::from_le_bytes(payload[0..4].try_into().map_err(|_| "snapshot_revision")?);
    let flags = payload[4];
    if flags & !0b0000_1111 != 0 || payload[5..8] != [0_u8; 3] {
        return Err("stream_snapshot_reserved_nonzero");
    }
    let read_i64 = |offset: usize| -> Result<i64, &'static str> {
        Ok(i64::from_le_bytes(
            payload[offset..offset + 8]
                .try_into()
                .map_err(|_| "snapshot_integer")?,
        ))
    };
    let read_u32 = |offset: usize| -> Result<u32, &'static str> {
        Ok(u32::from_le_bytes(
            payload[offset..offset + 4]
                .try_into()
                .map_err(|_| "snapshot_integer")?,
        ))
    };
    let full_len = usize::try_from(read_u32(32)?).map_err(|_| "snapshot_length")?;
    let committed_len = usize::try_from(read_u32(36)?).map_err(|_| "snapshot_length")?;
    let tentative_len = usize::try_from(read_u32(40)?).map_err(|_| "snapshot_length")?;
    let expected = STREAM_SNAPSHOT_HEADER_LEN
        .checked_add(full_len)
        .and_then(|length| length.checked_add(committed_len))
        .and_then(|length| length.checked_add(tentative_len))
        .ok_or("snapshot_length")?;
    if expected != payload.len() {
        return Err("snapshot_length");
    }
    let full_end = STREAM_SNAPSHOT_HEADER_LEN + full_len;
    let committed_end = full_end + committed_len;
    let decode = |bytes: &[u8]| {
        String::from_utf8(bytes.to_vec()).map_err(|_| "stream_snapshot_invalid_utf8")
    };
    Ok(StreamSnapshot {
        revision,
        result_changed: flags & 1 != 0,
        is_final: flags & 2 != 0,
        committed_changed: flags & 4 != 0,
        tentative_changed: flags & 8 != 0,
        input_received_ms: read_i64(8)?,
        audio_committed_ms: read_i64(16)?,
        buffered_ms: read_i64(24)?,
        full: decode(&payload[STREAM_SNAPSHOT_HEADER_LEN..full_end])?,
        committed: decode(&payload[full_end..committed_end])?,
        tentative: decode(&payload[committed_end..])?,
    })
}

pub fn encode_capabilities(capabilities: &WorkerCapabilities) -> Result<Vec<u8>, &'static str> {
    if capabilities.languages.len() > MAX_ATTESTED_TAGS
        || capabilities.translate_target_languages.len() > MAX_ATTESTED_TAGS
    {
        return Err("too_many_capability_tags");
    }
    let flags = (u8::from(capabilities.supports_streaming) * CAPABILITY_STREAMING)
        | (u8::from(capabilities.supports_language_detect) * CAPABILITY_LANGUAGE_DETECT)
        | (u8::from(capabilities.supports_translate) * CAPABILITY_TRANSLATION)
        | (u8::from(capabilities.accepts_whisper_run_options) * CAPABILITY_WHISPER_RUN_OPTIONS);
    let language_count =
        u8::try_from(capabilities.languages.len()).map_err(|_| "too_many_capability_tags")?;
    let target_count = u8::try_from(capabilities.translate_target_languages.len())
        .map_err(|_| "too_many_capability_tags")?;
    let mut payload = Vec::with_capacity(CAPABILITY_HEADER_LEN);
    payload.push(flags);
    payload.push(capabilities.compute.to_wire());
    payload.push(language_count);
    payload.push(target_count);
    encode_tag_list(&capabilities.languages, &mut payload)?;
    encode_tag_list(&capabilities.translate_target_languages, &mut payload)?;
    if payload.len() > MAX_CAPABILITY_PAYLOAD_LEN {
        return Err("capability_payload_too_large");
    }
    Ok(payload)
}

pub fn decode_capabilities(payload: &[u8]) -> Result<WorkerCapabilities, &'static str> {
    if payload.len() < CAPABILITY_HEADER_LEN {
        return Err("invalid_capability_length");
    }
    if payload.len() > MAX_CAPABILITY_PAYLOAD_LEN {
        return Err("capability_payload_too_large");
    }
    let flags = payload[0];
    if flags & !KNOWN_CAPABILITY_BITS != 0 {
        return Err("unknown_capability_bit");
    }
    let compute = WorkerCompute::from_wire(payload[1]).ok_or("unknown_compute_device")?;
    let language_count = usize::from(payload[2]);
    let target_count = usize::from(payload[3]);
    if language_count > MAX_ATTESTED_TAGS || target_count > MAX_ATTESTED_TAGS {
        return Err("too_many_capability_tags");
    }
    let mut offset = CAPABILITY_HEADER_LEN;
    let languages = decode_tag_list(payload, &mut offset, language_count)?;
    let translate_target_languages = decode_tag_list(payload, &mut offset, target_count)?;
    if offset != payload.len() {
        return Err("capability_payload_trailing");
    }
    Ok(WorkerCapabilities {
        compute,
        supports_streaming: flags & CAPABILITY_STREAMING != 0,
        supports_language_detect: flags & CAPABILITY_LANGUAGE_DETECT != 0,
        supports_translate: flags & CAPABILITY_TRANSLATION != 0,
        accepts_whisper_run_options: flags & CAPABILITY_WHISPER_RUN_OPTIONS != 0,
        languages,
        translate_target_languages,
    })
}

pub fn encode_recognition_options(options: &RecognitionOptions) -> Result<Vec<u8>, &'static str> {
    if options.is_default() {
        return Ok(Vec::new());
    }
    let (spoken_kind, spoken_bytes) = match &options.spoken_language {
        SpokenLanguage::Unspecified => (SPOKEN_UNSPECIFIED, &[] as &[u8]),
        SpokenLanguage::AutoDetect => (SPOKEN_AUTO_DETECT, &[] as &[u8]),
        SpokenLanguage::Explicit(tag) => (SPOKEN_EXPLICIT, tag.as_str().as_bytes()),
    };
    let target_bytes = options
        .translation_target
        .as_ref()
        .map_or(&[] as &[u8], |tag| tag.as_str().as_bytes());
    if spoken_bytes.len() > MAX_TAG_BYTES || target_bytes.len() > MAX_TAG_BYTES {
        return Err("recognition_options_tag_too_long");
    }
    let hints = options
        .context_bias
        .as_ref()
        .map_or(&[] as &[String], ContextBias::hints);
    if hints.len() > MAX_CONTEXT_HINTS {
        return Err("too_many_context_hints");
    }
    let spoken_len =
        u8::try_from(spoken_bytes.len()).map_err(|_| "recognition_options_tag_too_long")?;
    let target_len =
        u8::try_from(target_bytes.len()).map_err(|_| "recognition_options_tag_too_long")?;
    let hint_count = u8::try_from(hints.len()).map_err(|_| "too_many_context_hints")?;
    let mut payload = Vec::with_capacity(RECOGNITION_OPTIONS_HEADER_LEN);
    payload.push(spoken_kind);
    payload.push(0);
    payload.push(spoken_len);
    payload.push(target_len);
    payload.push(hint_count);
    payload.extend_from_slice(spoken_bytes);
    payload.extend_from_slice(target_bytes);
    for hint in hints {
        if hint.is_empty() || hint.len() > MAX_CONTEXT_HINT_BYTES || hint.as_bytes().contains(&0) {
            return Err("invalid_context_bias");
        }
        let hint_len = u8::try_from(hint.len()).map_err(|_| "recognition_options_hint_too_long")?;
        payload.push(hint_len);
        payload.extend_from_slice(hint.as_bytes());
    }
    if payload.len() > MAX_RECOGNITION_OPTIONS_BYTES {
        return Err("recognition_options_too_large");
    }
    Ok(payload)
}

pub fn encode_recognition_options_frame(
    options: &RecognitionOptions,
) -> Result<Vec<u8>, &'static str> {
    let payload = encode_recognition_options(options)?;
    if payload.len() > MAX_RECOGNITION_OPTIONS_BYTES {
        return Err("recognition_options_too_large");
    }
    let len = u32::try_from(payload.len()).map_err(|_| "recognition_options_too_large")?;
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&len.to_le_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

pub fn decode_recognition_options(payload: &[u8]) -> Result<RecognitionOptions, &'static str> {
    if payload.is_empty() {
        return Ok(RecognitionOptions::default());
    }
    if payload.len() < RECOGNITION_OPTIONS_HEADER_LEN {
        return Err("recognition_options_truncated");
    }
    if payload.len() > MAX_RECOGNITION_OPTIONS_BYTES {
        return Err("recognition_options_too_large");
    }
    let spoken_kind = payload[0];
    if payload[1] != 0 {
        return Err("recognition_options_reserved_nonzero");
    }
    let spoken_len = usize::from(payload[2]);
    let target_len = usize::from(payload[3]);
    let hint_count = usize::from(payload[4]);
    if spoken_len > MAX_TAG_BYTES || target_len > MAX_TAG_BYTES {
        return Err("recognition_options_tag_too_long");
    }
    if hint_count > MAX_CONTEXT_HINTS {
        return Err("too_many_context_hints");
    }
    let mut offset = RECOGNITION_OPTIONS_HEADER_LEN;
    let spoken_bytes = take_bytes(payload, &mut offset, spoken_len)?;
    let target_bytes = take_bytes(payload, &mut offset, target_len)?;
    let spoken_language = match spoken_kind {
        SPOKEN_UNSPECIFIED => {
            if spoken_len != 0 {
                return Err("invalid_spoken_language");
            }
            SpokenLanguage::Unspecified
        }
        SPOKEN_AUTO_DETECT => {
            if spoken_len != 0 {
                return Err("invalid_spoken_language");
            }
            SpokenLanguage::AutoDetect
        }
        SPOKEN_EXPLICIT => {
            if spoken_len == 0 {
                return Err("invalid_spoken_language");
            }
            SpokenLanguage::Explicit(parse_wire_tag(spoken_bytes)?)
        }
        _ => return Err("invalid_spoken_language"),
    };
    let translation_target = if target_len == 0 {
        None
    } else {
        Some(parse_wire_tag(target_bytes)?)
    };
    let mut hints = Vec::new();
    for _ in 0..hint_count {
        if offset >= payload.len() {
            return Err("recognition_options_truncated");
        }
        let hint_len = usize::from(payload[offset]);
        offset += 1;
        if hint_len == 0 || hint_len > MAX_CONTEXT_HINT_BYTES {
            return Err("invalid_context_bias");
        }
        let hint_bytes = take_bytes(payload, &mut offset, hint_len)?;
        if hint_bytes.contains(&0) {
            return Err("invalid_context_bias");
        }
        let hint = std::str::from_utf8(hint_bytes).map_err(|_| "invalid_context_bias")?;
        hints.push(hint.to_owned());
    }
    if offset != payload.len() {
        return Err("recognition_options_trailing");
    }
    let context_bias = if hints.is_empty() {
        None
    } else {
        Some(ContextBias::try_new(hints).map_err(|_| "invalid_context_bias")?)
    };
    Ok(RecognitionOptions {
        spoken_language,
        translation_target,
        context_bias,
    })
}

fn encode_tag_list(tags: &[AsrTag], payload: &mut Vec<u8>) -> Result<(), &'static str> {
    for tag in tags {
        let bytes = tag.as_str().as_bytes();
        if bytes.is_empty() || bytes.len() > MAX_TAG_BYTES {
            return Err("invalid_capability_tag");
        }
        let len = u8::try_from(bytes.len()).map_err(|_| "invalid_capability_tag")?;
        payload.push(len);
        payload.extend_from_slice(bytes);
    }
    Ok(())
}

fn decode_tag_list(
    payload: &[u8],
    offset: &mut usize,
    count: usize,
) -> Result<Vec<AsrTag>, &'static str> {
    let mut tags = Vec::with_capacity(count);
    for _ in 0..count {
        if *offset >= payload.len() {
            return Err("capability_tag_truncated");
        }
        let len = usize::from(payload[*offset]);
        *offset += 1;
        if len == 0 || len > MAX_TAG_BYTES {
            return Err("invalid_capability_tag");
        }
        let bytes = take_bytes(payload, offset, len)?;
        tags.push(parse_wire_tag(bytes)?);
    }
    Ok(tags)
}

fn take_bytes<'a>(
    payload: &'a [u8],
    offset: &mut usize,
    len: usize,
) -> Result<&'a [u8], &'static str> {
    let end = offset.checked_add(len).ok_or("payload_length")?;
    let bytes = payload.get(*offset..end).ok_or("payload_length")?;
    *offset = end;
    Ok(bytes)
}

fn parse_wire_tag(bytes: &[u8]) -> Result<AsrTag, &'static str> {
    let raw = std::str::from_utf8(bytes).map_err(|_| "invalid_capability_tag")?;
    AsrTag::parse(raw).map_err(|_| "invalid_capability_tag")
}

fn validate_prefix(bytes: &[u8; HEADER_LEN]) -> Result<(), &'static str> {
    if bytes[..4] != MAGIC {
        return Err("invalid_magic");
    }
    if u16::from_le_bytes([bytes[4], bytes[5]]) != VERSION {
        return Err("unsupported_version");
    }
    if bytes[7] != 0 {
        return Err("reserved_byte_nonzero");
    }
    Ok(())
}

pub fn read_request(reader: &mut impl Read) -> Result<Option<RequestHeader>, &'static str> {
    let mut header = [0_u8; HEADER_LEN];
    let mut offset = 0;
    while offset < header.len() {
        match reader.read(&mut header[offset..]) {
            Ok(0) if offset == 0 => return Ok(None),
            Ok(0) => return Err("truncated_request_header"),
            Ok(count) => offset += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return Err("request_read_failed"),
        }
    }
    decode_request(header).map(Some)
}

pub fn write_response(
    writer: &mut impl Write,
    status: u8,
    request_id: u128,
    payload: &[u8],
) -> Result<(), &'static str> {
    if payload.len() > MAX_TEXT_BYTES {
        return Err("response_too_large");
    }
    let payload_len = u32::try_from(payload.len()).map_err(|_| "response_too_large")?;
    let header = encode_response(ResponseHeader {
        status,
        request_id,
        payload_len,
    });
    writer
        .write_all(&header)
        .and_then(|_| writer.write_all(payload))
        .and_then(|_| writer.flush())
        .map_err(|_| "response_write_failed")
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    fn tag(raw: &str) -> AsrTag {
        AsrTag::parse(raw).expect("test tag")
    }

    #[test]
    fn request_and_response_round_trip() -> Result<(), &'static str> {
        let request = RequestHeader {
            operation: OP_DECODE,
            request_id: 42,
            sample_count: 160,
        };
        assert_eq!(decode_request(encode_request(request)), Ok(request));

        let response = ResponseHeader {
            status: STATUS_OK,
            request_id: 42,
            payload_len: 7,
        };
        assert_eq!(decode_response(encode_response(response)), Ok(response));

        for supports_streaming in [false, true] {
            let capabilities = WorkerCapabilities {
                supports_streaming,
                ..WorkerCapabilities::default()
            };
            assert_eq!(
                decode_capabilities(&encode_capabilities(&capabilities)?),
                Ok(capabilities)
            );
        }

        let attested = WorkerCapabilities {
            supports_streaming: true,
            supports_language_detect: true,
            supports_translate: true,
            accepts_whisper_run_options: true,
            languages: vec![tag("en"), tag("de")],
            translate_target_languages: vec![tag("en")],
            compute: WorkerCompute::DiscreteGpu,
        };
        assert_eq!(
            decode_capabilities(&encode_capabilities(&attested)?),
            Ok(attested)
        );

        let snapshot = StreamSnapshot {
            revision: 7,
            result_changed: true,
            is_final: false,
            committed_changed: true,
            tentative_changed: true,
            input_received_ms: 320,
            audio_committed_ms: 240,
            buffered_ms: 80,
            full: "controlled full".to_owned(),
            committed: "controlled ".to_owned(),
            tentative: "partial".to_owned(),
        };
        let encoded = encode_stream_snapshot(&snapshot)?;
        assert_eq!(decode_stream_snapshot(&encoded), Ok(snapshot));
        Ok(())
    }

    #[test]
    fn recognition_options_round_trip_preserves_default_and_applied_fields()
    -> Result<(), &'static str> {
        assert!(encode_recognition_options(&RecognitionOptions::default())?.is_empty());
        assert_eq!(
            decode_recognition_options(&[]),
            Ok(RecognitionOptions::default())
        );
        let frame = encode_recognition_options_frame(&RecognitionOptions::default())?;
        assert_eq!(frame, 0_u32.to_le_bytes());

        let explicit = RecognitionOptions {
            spoken_language: SpokenLanguage::Explicit(tag("en")),
            translation_target: Some(tag("de")),
            context_bias: Some(ContextBias::try_new(["secret-hotword", "narrivy"]).unwrap()),
        };
        let encoded = encode_recognition_options(&explicit)?;
        assert_eq!(decode_recognition_options(&encoded), Ok(explicit.clone()));
        assert!(
            !format!("{explicit:?}").contains("secret-hotword"),
            "context text leaked into options debug"
        );
        assert!(
            !encoded
                .windows(b"output".len())
                .any(|window| window == b"output")
        );
        Ok(())
    }

    #[test]
    fn sample_bound_is_the_shared_portable_capture_maximum() {
        assert_eq!(MAX_SAMPLES, narrivy_types::MAX_CAPTURE_SAMPLES);
        assert_eq!(MAX_SAMPLES, 4_800_000);
        let maximum = RequestHeader {
            operation: OP_STREAM_FEED,
            request_id: 7,
            sample_count: u32::try_from(MAX_SAMPLES).unwrap_or(u32::MAX),
        };
        assert_eq!(decode_request(encode_request(maximum)), Ok(maximum));
        let over = RequestHeader {
            operation: OP_STREAM_FEED,
            request_id: 7,
            sample_count: u32::try_from(MAX_SAMPLES + 1).unwrap_or(u32::MAX),
        };
        assert_eq!(decode_request(encode_request(over)), Err("audio_too_large"));
    }

    #[test]
    fn malformed_and_oversized_frames_fail_closed() {
        let mut wrong_magic = encode_request(RequestHeader {
            operation: OP_DECODE,
            request_id: 1,
            sample_count: 1,
        });
        wrong_magic[0] = b'X';
        assert_eq!(decode_request(wrong_magic), Err("invalid_magic"));

        let oversized = encode_response(ResponseHeader {
            status: STATUS_OK,
            request_id: 1,
            payload_len: u32::try_from(MAX_TEXT_BYTES + 1).unwrap_or(u32::MAX),
        });
        assert_eq!(decode_response(oversized), Err("response_too_large"));

        let error_with_text = encode_response(ResponseHeader {
            status: STATUS_DECODE_FAILED,
            request_id: 1,
            payload_len: 1,
        });
        assert_eq!(
            decode_response(error_with_text),
            Err("invalid_response_payload_shape")
        );

        let ready_without_capabilities = encode_response(ResponseHeader {
            status: STATUS_READY,
            request_id: 0,
            payload_len: 0,
        });
        assert_eq!(
            decode_response(ready_without_capabilities),
            Err("invalid_response_payload_shape")
        );
        let ready_too_large = encode_response(ResponseHeader {
            status: STATUS_READY,
            request_id: 0,
            payload_len: u32::try_from(MAX_CAPABILITY_PAYLOAD_LEN + 1).unwrap_or(u32::MAX),
        });
        assert_eq!(
            decode_response(ready_too_large),
            Err("invalid_response_payload_shape")
        );
        assert_eq!(decode_capabilities(&[1]), Err("invalid_capability_length"));
        assert_eq!(
            decode_capabilities(&[1 << 7, 0, 0, 0]),
            Err("unknown_capability_bit")
        );
        assert_eq!(
            decode_capabilities(&[0, 1, 0, 0]).map(|capabilities| capabilities.compute),
            Ok(WorkerCompute::IntegratedGpu)
        );
        assert_eq!(
            decode_capabilities(&[0, 2, 0, 0]).map(|capabilities| capabilities.compute),
            Ok(WorkerCompute::DiscreteGpu)
        );
        assert_eq!(
            decode_capabilities(&[0, 3, 0, 0]).map(|capabilities| capabilities.compute),
            Ok(WorkerCompute::Npu)
        );
        assert_eq!(
            decode_capabilities(&[0, 4, 0, 0]),
            Err("unknown_compute_device")
        );
        let on_gpu = WorkerCapabilities {
            compute: WorkerCompute::IntegratedGpu,
            ..WorkerCapabilities::default()
        };
        assert_eq!(
            encode_capabilities(&on_gpu)
                .ok()
                .and_then(|payload| decode_capabilities(&payload).ok()),
            Some(on_gpu)
        );
        assert_eq!(
            ComputeRequest::from_argument("auto"),
            Some(ComputeRequest::Auto)
        );
        assert_eq!(
            ComputeRequest::from_argument("cpu"),
            Some(ComputeRequest::Cpu)
        );
        assert_eq!(
            ComputeRequest::from_argument("gpu"),
            Some(ComputeRequest::Gpu)
        );
        assert_eq!(
            ComputeRequest::from_argument("npu"),
            Some(ComputeRequest::Npu)
        );
        assert_eq!(ComputeRequest::from_argument("tpu"), None);
        assert_eq!(
            decode_capabilities(&[0, 0, 1, 0]),
            Err("capability_tag_truncated")
        );
        let mut oversized_languages =
            vec![0_u8, 0, u8::try_from(MAX_ATTESTED_TAGS + 1).unwrap(), 0];
        oversized_languages.extend(std::iter::repeat_n(0, 8));
        assert_eq!(
            decode_capabilities(&oversized_languages),
            Err("too_many_capability_tags")
        );
        assert_eq!(
            encode_capabilities(&WorkerCapabilities {
                languages: vec![tag("en"); MAX_ATTESTED_TAGS + 1],
                ..WorkerCapabilities::default()
            }),
            Err("too_many_capability_tags")
        );

        assert_eq!(
            decode_recognition_options(&[3, 0, 0, 0, 0]),
            Err("invalid_spoken_language")
        );
        assert_eq!(
            decode_recognition_options(&[SPOKEN_EXPLICIT, 0, 2, 0, 0, b'e', b'n', 0]),
            Err("recognition_options_trailing")
        );
        let mut too_many_hints = vec![
            SPOKEN_UNSPECIFIED,
            0,
            0,
            0,
            u8::try_from(MAX_CONTEXT_HINTS + 1).unwrap(),
        ];
        assert_eq!(
            decode_recognition_options(&too_many_hints),
            Err("too_many_context_hints")
        );
        too_many_hints.resize(MAX_RECOGNITION_OPTIONS_BYTES + 1, 1);
        assert_eq!(
            decode_recognition_options(&too_many_hints),
            Err("recognition_options_too_large")
        );
        assert_eq!(
            decode_recognition_options(&[SPOKEN_UNSPECIFIED, 0, 0, 0, 1, 0]),
            Err("invalid_context_bias")
        );

        let mut malformed_snapshot = vec![0_u8; STREAM_SNAPSHOT_HEADER_LEN];
        malformed_snapshot[4] = 0b1000_0000;
        assert_eq!(
            decode_stream_snapshot(&malformed_snapshot),
            Err("stream_snapshot_reserved_nonzero")
        );
        let mut wrong_lengths = vec![0_u8; STREAM_SNAPSHOT_HEADER_LEN];
        wrong_lengths[32..36].copy_from_slice(&1_u32.to_le_bytes());
        assert_eq!(
            decode_stream_snapshot(&wrong_lengths),
            Err("snapshot_length")
        );

        assert_eq!(PCM_TRANSFER_BYTES, 16_384);
        assert_eq!(PCM_TRANSFER_BYTES % size_of::<f32>(), 0);
    }
}
