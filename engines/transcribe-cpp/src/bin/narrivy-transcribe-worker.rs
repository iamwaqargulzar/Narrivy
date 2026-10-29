//! Process-isolated resident transcribe.cpp worker.
//!
//! stdin/stdout are the private bounded binary protocol. stderr is not used;
//! the native logger is disabled before model load. Transcript/audio content
//! is never written to disk or ordinary logs.

use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use sha2::{Digest, Sha256};

use narrivy_asr_api::{AsrError, AsrTag, MAX_ATTESTED_TAGS, SpokenLanguage};
use narrivy_asr_transcribe::resident_protocol::{
    self, ComputeRequest, MAX_RECOGNITION_OPTIONS_BYTES, MAX_SAMPLES, OP_DECODE, OP_SHUTDOWN,
    OP_STREAM_BEGIN, OP_STREAM_FEED, OP_STREAM_FINALIZE, OP_STREAM_RESET, PCM_TRANSFER_BYTES,
    PCM_TRANSFER_SAMPLES, RecognitionOptions, STATUS_BACKEND_MISMATCH,
    STATUS_CANCELLATION_UNSUPPORTED, STATUS_DECODE_FAILED, STATUS_INVALID_AUDIO,
    STATUS_INVALID_REQUEST, STATUS_MODEL_FORMAT_UNSUPPORTED, STATUS_MODEL_LOAD_FAILED,
    STATUS_NATIVE_BACKEND_FAILED, STATUS_NATIVE_STARTUP_OTHER, STATUS_OK, STATUS_OUT_OF_MEMORY,
    STATUS_READY, STATUS_SAMPLE_RATE_MISMATCH, STATUS_STREAM_FINAL, STATUS_STREAM_RESET,
    STATUS_STREAM_STARTED, STATUS_STREAM_UPDATE, STATUS_STREAMING_UNSUPPORTED,
    STATUS_UNSUPPORTED_REQUEST, STATUS_VERSION_MISMATCH, StreamSnapshot, WorkerCapabilities,
    WorkerCompute,
};
use transcribe_cpp::{
    Backend, Feature, Model, ModelOptions, RunExtension, RunOptions, StreamOptions, StreamUpdate,
    Task, TimestampKind, WhisperRunOptions,
};

fn main() -> std::process::ExitCode {
    // `execve` made this worker dumpable again; it holds audio and
    // transcripts (D37, F-H18).
    if narrivy_process_linux::forbid_core_dumps().is_err() {
        return std::process::ExitCode::from(1);
    }
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(()) => std::process::ExitCode::from(1),
    }
}

fn run() -> Result<(), ()> {
    let model_binding = parse_model_binding()?;
    let model_path = &model_binding.path;
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut reader = stdin.lock();
    let mut writer = stdout.lock();
    if validate_model_binding(&model_binding).is_err() {
        resident_protocol::write_response(
            &mut writer,
            resident_protocol::STATUS_MODEL_BINDING_FAILED,
            0,
            &[],
        )
        .map_err(|_| ())?;
        return Err(());
    }
    if narrivy_asr_transcribe::validate_gguf_model(model_path).is_err() {
        resident_protocol::write_response(&mut writer, STATUS_MODEL_LOAD_FAILED, 0, &[])
            .map_err(|_| ())?;
        return Err(());
    }
    transcribe_cpp::disable_logging();
    let (model, compute) = match load_model(model_path, model_binding.compute) {
        Ok(loaded) => loaded,
        Err(error) => {
            let status = match error {
                transcribe_cpp::Error::ModelFileNotFound(_) => STATUS_MODEL_LOAD_FAILED,
                transcribe_cpp::Error::ModelLoad(_) => STATUS_MODEL_FORMAT_UNSUPPORTED,
                transcribe_cpp::Error::VersionMismatch(_)
                | transcribe_cpp::Error::BadStructSize(_) => STATUS_VERSION_MISMATCH,
                transcribe_cpp::Error::OutOfMemory(_) => STATUS_OUT_OF_MEMORY,
                transcribe_cpp::Error::Backend(_) => STATUS_NATIVE_BACKEND_FAILED,
                _ => STATUS_NATIVE_STARTUP_OTHER,
            };
            resident_protocol::write_response(&mut writer, status, 0, &[]).map_err(|_| ())?;
            return Err(());
        }
    };
    let Ok(native_capabilities) = model.capabilities() else {
        resident_protocol::write_response(&mut writer, STATUS_NATIVE_STARTUP_OTHER, 0, &[])
            .map_err(|_| ())?;
        return Err(());
    };
    // ggml reports concrete device names (for the strict CPU request this is
    // currently `CPU`), not the lower-case request token used by the CLI.
    if compute == WorkerCompute::Cpu && !model.backend().eq_ignore_ascii_case("cpu") {
        resident_protocol::write_response(&mut writer, STATUS_BACKEND_MISMATCH, 0, &[])
            .map_err(|_| ())?;
        return Err(());
    }
    if native_capabilities.native_sample_rate != 16_000 {
        resident_protocol::write_response(&mut writer, STATUS_SAMPLE_RATE_MISMATCH, 0, &[])
            .map_err(|_| ())?;
        return Err(());
    }
    if !model.supports(Feature::Cancellation) {
        resident_protocol::write_response(&mut writer, STATUS_CANCELLATION_UNSUPPORTED, 0, &[])
            .map_err(|_| ())?;
        return Err(());
    }
    let capabilities = match worker_capabilities_from_model(&model, &native_capabilities, compute) {
        Ok(capabilities) => capabilities,
        Err(()) => {
            resident_protocol::write_response(&mut writer, STATUS_NATIVE_STARTUP_OTHER, 0, &[])
                .map_err(|_| ())?;
            return Err(());
        }
    };
    let capability_payload = match resident_protocol::encode_capabilities(&capabilities) {
        Ok(payload) => payload,
        Err(_) => {
            resident_protocol::write_response(&mut writer, STATUS_NATIVE_STARTUP_OTHER, 0, &[])
                .map_err(|_| ())?;
            return Err(());
        }
    };
    resident_protocol::write_response(&mut writer, STATUS_READY, 0, &capability_payload)
        .map_err(|_| ())?;

    loop {
        let Some(header) = resident_protocol::read_request(&mut reader).map_err(|_| ())? else {
            return Ok(());
        };
        match header.operation {
            OP_SHUTDOWN => return Ok(()),
            OP_DECODE => {
                let options_payload = match read_options_payload(&mut reader) {
                    Ok(payload) => payload,
                    Err(_) => return Err(()),
                };
                let sample_count = usize::try_from(header.sample_count).map_err(|_| ())?;
                let samples = match read_pcm(&mut reader, sample_count) {
                    Ok(samples) => samples,
                    Err(PcmReadError::Invalid) => {
                        resident_protocol::write_response(
                            &mut writer,
                            STATUS_INVALID_AUDIO,
                            header.request_id,
                            &[],
                        )
                        .map_err(|_| ())?;
                        continue;
                    }
                    Err(PcmReadError::Io) => return Err(()),
                };
                let options = match resident_protocol::decode_recognition_options(&options_payload)
                {
                    Ok(options) => options,
                    Err(_) => {
                        resident_protocol::write_response(
                            &mut writer,
                            STATUS_INVALID_REQUEST,
                            header.request_id,
                            &[],
                        )
                        .map_err(|_| ())?;
                        continue;
                    }
                };
                let run_options = match build_run_options(&options, &capabilities) {
                    Ok(run_options) => run_options,
                    Err(_) => {
                        resident_protocol::write_response(
                            &mut writer,
                            STATUS_UNSUPPORTED_REQUEST,
                            header.request_id,
                            &[],
                        )
                        .map_err(|_| ())?;
                        continue;
                    }
                };
                let result = model
                    .session()
                    .and_then(|mut session| session.run(&samples, &run_options))
                    .map(|transcript| transcript.text.into_bytes())
                    .or_else(|error| match error {
                        // The model began repeating itself and was stopped:
                        // what it said before, with one copy of the repeat,
                        // is the transcript (0.2.3 returned the loop itself).
                        transcribe_cpp::Error::OutputRepetition {
                            partial: Some(partial),
                            ..
                        } => Ok(partial.text.into_bytes()),
                        other => Err(other),
                    });
                match result {
                    Ok(text) if text.len() <= resident_protocol::MAX_TEXT_BYTES => {
                        resident_protocol::write_response(
                            &mut writer,
                            STATUS_OK,
                            header.request_id,
                            &text,
                        )
                        .map_err(|_| ())?;
                    }
                    Ok(_) | Err(_) => {
                        resident_protocol::write_response(
                            &mut writer,
                            STATUS_DECODE_FAILED,
                            header.request_id,
                            &[],
                        )
                        .map_err(|_| ())?;
                    }
                }
            }
            OP_STREAM_BEGIN => {
                if !capabilities.supports_streaming {
                    resident_protocol::write_response(
                        &mut writer,
                        STATUS_STREAMING_UNSUPPORTED,
                        header.request_id,
                        &[],
                    )
                    .map_err(|_| ())?;
                    continue;
                }
                match run_stream(&model, header.request_id, &mut reader, &mut writer)? {
                    StreamLoopOutcome::Continue => {}
                    StreamLoopOutcome::Shutdown => return Ok(()),
                }
            }
            _ => return Err(()),
        }
    }
}

enum PcmReadError {
    Invalid,
    Io,
}

/// The pinned 0.2.3 high-level API documents `Feature::InitialPrompt` as
/// accepting a free-text decode prompt ("whisper today"). It does not re-export
/// a stable Whisper run-extension kind on the idiomatic surface (`accepts_ext`
/// takes a raw `u32`; the named kind lives only in `sys`). Context bias is
/// therefore attested from that feature and applied only through
/// `RunExtension::Whisper`.
fn accepts_whisper_context_bias(model: &Model) -> bool {
    model.supports(Feature::InitialPrompt)
}

/// Load the model where the parent asked. `Auto` tries a GPU first and
/// falls back to strict CPU when there is none, the model does not load
/// there, or the self-test fails. `Gpu` does not fall back: without a GPU
/// that passes, the worker reports a backend failure. The attested device is
/// what was loaded.
fn load_model(
    path: &std::path::Path,
    request: ComputeRequest,
) -> Result<(Model, WorkerCompute), transcribe_cpp::Error> {
    if request == ComputeRequest::Npu {
        return Err(transcribe_cpp::Error::Backend(
            "this worker has no NPU backend".to_owned(),
        ));
    }
    #[cfg(feature = "vulkan")]
    if request != ComputeRequest::Cpu
        && let Some(loaded) = load_on_gpu(path)
    {
        return Ok(loaded);
    }
    if request == ComputeRequest::Gpu {
        return Err(transcribe_cpp::Error::Backend(
            "no GPU loaded the model and passed the self-test".to_owned(),
        ));
    }
    let model = Model::load_with(
        path,
        &ModelOptions {
            backend: Backend::Cpu,
            device: None,
            // The weights are served from the (hash-checked, never rewritten
            // in place) model file's page cache rather than copied: the
            // system can reclaim that memory between dictations.
            no_mmap: false,
        },
    )?;
    Ok((model, WorkerCompute::Cpu))
}

/// The recording the GPU self-test decodes.
#[cfg(feature = "vulkan")]
const SELF_TEST_WAV: &[u8] = include_bytes!("../../../../tests/fixtures/jfk16.wav");

/// Load on a GPU and prove it there: the known recording must decode to the
/// known phrase faster than real time. Any failure returns `None`, after the
/// GPU copy of the model has been released.
#[cfg(feature = "vulkan")]
fn load_on_gpu(path: &std::path::Path) -> Option<(Model, WorkerCompute)> {
    use narrivy_asr_transcribe::self_test;

    if !transcribe_cpp::backend_available(Backend::Vulkan) {
        return None;
    }
    let model = Model::load_with(
        path,
        &ModelOptions {
            backend: Backend::Vulkan,
            device: None,
            no_mmap: false,
        },
    )
    .ok()?;
    let compute = match model.device().ok()?.device_type {
        transcribe_cpp::DeviceType::Gpu => WorkerCompute::DiscreteGpu,
        transcribe_cpp::DeviceType::Igpu => WorkerCompute::IntegratedGpu,
        _ => return None,
    };
    if i64::from(model.capabilities().ok()?.native_sample_rate)
        != i64::from(self_test::SAMPLE_RATE_HZ)
    {
        return None;
    }
    let samples = self_test::fixture_samples(SELF_TEST_WAV)?;
    let started = std::time::Instant::now();
    let transcript = model
        .session()
        .and_then(|mut session| session.run(&samples, &RunOptions::default()))
        .ok()?;
    let passed = self_test::phrase_matches(&transcript.text)
        && self_test::faster_than_real_time(started.elapsed(), samples.len());
    passed.then_some((model, compute))
}

fn worker_capabilities_from_model(
    model: &Model,
    native: &transcribe_cpp::Capabilities,
    compute: WorkerCompute,
) -> Result<WorkerCapabilities, ()> {
    Ok(WorkerCapabilities {
        compute,
        supports_streaming: native.supports_streaming,
        supports_language_detect: native.supports_language_detect,
        supports_translate: native.supports_translate,
        accepts_whisper_run_options: accepts_whisper_context_bias(model),
        languages: bounded_tags(native.languages.iter())?,
        translate_target_languages: bounded_tags(native.translate_target_languages.iter())?,
    })
}

fn bounded_tags<T: AsRef<str>>(values: impl IntoIterator<Item = T>) -> Result<Vec<AsrTag>, ()> {
    let mut tags = Vec::new();
    for value in values {
        if tags.len() == MAX_ATTESTED_TAGS {
            return Err(());
        }
        let tag = AsrTag::parse(value.as_ref()).map_err(|_| ())?;
        tags.push(tag);
    }
    Ok(tags)
}

fn build_run_options(
    options: &RecognitionOptions,
    capabilities: &WorkerCapabilities,
) -> Result<RunOptions, AsrError> {
    capabilities
        .to_asr_capabilities()
        .validate(&options.to_asr_request())?;
    let mut run_options = RunOptions {
        timestamps: TimestampKind::None,
        ..RunOptions::default()
    };
    if let SpokenLanguage::Explicit(tag) = &options.spoken_language {
        run_options.language = Some(tag.as_str().to_owned());
    }
    if let Some(target) = &options.translation_target {
        run_options.task = Task::Translate;
        run_options.target_language = Some(target.as_str().to_owned());
    }
    if let Some(bias) = &options.context_bias {
        if !capabilities.accepts_whisper_run_options {
            return Err(AsrError::new(
                narrivy_asr_api::UNSUPPORTED_CONTEXT_BIAS,
                "context bias is not supported by the loaded model",
            ));
        }
        run_options.family = Some(RunExtension::Whisper(WhisperRunOptions {
            initial_prompt: Some(bias.hints().join(" ")),
            ..WhisperRunOptions::default()
        }));
    }
    Ok(run_options)
}

fn read_options_payload(reader: &mut impl Read) -> Result<Vec<u8>, PcmReadError> {
    let mut len_bytes = [0_u8; 4];
    reader
        .read_exact(&mut len_bytes)
        .map_err(|_| PcmReadError::Io)?;
    let len = usize::try_from(u32::from_le_bytes(len_bytes)).map_err(|_| PcmReadError::Invalid)?;
    if len > MAX_RECOGNITION_OPTIONS_BYTES {
        return Err(PcmReadError::Invalid);
    }
    let mut payload = vec![0_u8; len];
    if len > 0 {
        reader
            .read_exact(&mut payload)
            .map_err(|_| PcmReadError::Io)?;
    }
    Ok(payload)
}

fn read_pcm(reader: &mut impl Read, sample_count: usize) -> Result<Vec<f32>, PcmReadError> {
    if sample_count == 0 || sample_count > MAX_SAMPLES {
        return Err(PcmReadError::Invalid);
    }
    let mut samples = Vec::with_capacity(sample_count);
    let mut transfer = [0_u8; PCM_TRANSFER_BYTES];
    let mut invalid = false;
    while samples.len() < sample_count {
        let remaining = sample_count - samples.len();
        let transfer_samples = remaining.min(PCM_TRANSFER_SAMPLES);
        let transfer_bytes = transfer_samples
            .checked_mul(size_of::<f32>())
            .ok_or(PcmReadError::Invalid)?;
        reader
            .read_exact(&mut transfer[..transfer_bytes])
            .map_err(|_| PcmReadError::Io)?;
        for raw in transfer[..transfer_bytes].chunks_exact(size_of::<f32>()) {
            let sample = f32::from_le_bytes(raw.try_into().map_err(|_| PcmReadError::Invalid)?);
            invalid |= !sample.is_finite();
            samples.push(sample);
        }
    }
    if invalid {
        Err(PcmReadError::Invalid)
    } else {
        Ok(samples)
    }
}

enum StreamLoopOutcome {
    Continue,
    Shutdown,
}

fn run_stream(
    model: &Model,
    request_id: u128,
    reader: &mut impl Read,
    writer: &mut impl std::io::Write,
) -> Result<StreamLoopOutcome, ()> {
    let mut session = model.session().map_err(|_| ())?;
    let mut stream = session
        .stream(
            &RunOptions {
                timestamps: TimestampKind::None,
                ..RunOptions::default()
            },
            &StreamOptions::default(),
        )
        .map_err(|_| ())?;
    resident_protocol::write_response(writer, STATUS_STREAM_STARTED, request_id, &[])
        .map_err(|_| ())?;
    let mut total_samples = 0_usize;

    loop {
        let Some(header) = resident_protocol::read_request(reader).map_err(|_| ())? else {
            return Err(());
        };
        if header.request_id != request_id && header.operation != OP_SHUTDOWN {
            return Err(());
        }
        match header.operation {
            OP_STREAM_FEED => {
                let count = usize::try_from(header.sample_count).map_err(|_| ())?;
                let samples = match read_pcm(reader, count) {
                    Ok(samples) => samples,
                    Err(PcmReadError::Invalid) => {
                        resident_protocol::write_response(
                            writer,
                            STATUS_INVALID_AUDIO,
                            request_id,
                            &[],
                        )
                        .map_err(|_| ())?;
                        continue;
                    }
                    Err(PcmReadError::Io) => return Err(()),
                };
                total_samples = total_samples.checked_add(samples.len()).ok_or(())?;
                if total_samples > MAX_SAMPLES {
                    resident_protocol::write_response(
                        writer,
                        STATUS_INVALID_AUDIO,
                        request_id,
                        &[],
                    )
                    .map_err(|_| ())?;
                    continue;
                }
                let update = stream.feed(&samples).map_err(|_| ())?;
                write_stream_snapshot(writer, STATUS_STREAM_UPDATE, request_id, update, &stream)?;
            }
            OP_STREAM_FINALIZE => {
                let update = stream.finalize().map_err(|_| ())?;
                if !update.is_final {
                    return Err(());
                }
                write_stream_snapshot(writer, STATUS_STREAM_FINAL, request_id, update, &stream)?;
                return Ok(StreamLoopOutcome::Continue);
            }
            OP_STREAM_RESET => {
                stream.reset();
                resident_protocol::write_response(writer, STATUS_STREAM_RESET, request_id, &[])
                    .map_err(|_| ())?;
                return Ok(StreamLoopOutcome::Continue);
            }
            OP_SHUTDOWN => {
                stream.reset();
                return Ok(StreamLoopOutcome::Shutdown);
            }
            _ => return Err(()),
        }
    }
}

fn write_stream_snapshot(
    writer: &mut impl std::io::Write,
    status: u8,
    request_id: u128,
    update: StreamUpdate,
    stream: &transcribe_cpp::Stream<'_>,
) -> Result<(), ()> {
    let text = stream.text();
    let payload = resident_protocol::encode_stream_snapshot(&StreamSnapshot {
        revision: update.revision,
        result_changed: update.result_changed,
        is_final: update.is_final,
        committed_changed: update.committed_changed,
        tentative_changed: update.tentative_changed,
        input_received_ms: update.input_received_ms,
        audio_committed_ms: update.audio_committed_ms,
        buffered_ms: update.buffered_ms,
        full: text.full,
        committed: text.committed,
        tentative: text.tentative,
    })
    .map_err(|_| ())?;
    resident_protocol::write_response(writer, status, request_id, &payload).map_err(|_| ())
}

struct ModelBinding {
    compute: ComputeRequest,
    path: PathBuf,
    sha256: [u8; 32],
    size: u64,
}

fn parse_model_binding() -> Result<ModelBinding, ()> {
    let mut args = std::env::args_os();
    let _program = args.next();
    parse_model_binding_from(&mut args)
}

fn parse_model_binding_from(
    args: &mut impl Iterator<Item = std::ffi::OsString>,
) -> Result<ModelBinding, ()> {
    // `--compute <auto|cpu>` may lead; without it the worker is strict CPU.
    let mut first = args.next();
    let mut compute = ComputeRequest::Cpu;
    if first.as_deref() == Some(std::ffi::OsStr::new(ComputeRequest::ARGUMENT)) {
        compute = args
            .next()
            .and_then(|value| value.into_string().ok())
            .and_then(|value| ComputeRequest::from_argument(&value))
            .ok_or(())?;
        first = args.next();
    }
    if first.as_deref() != Some(std::ffi::OsStr::new("--model-sha256")) {
        return Err(());
    }
    let sha256 = args
        .next()
        .and_then(|value| value.into_string().ok())
        .ok_or(())?;
    let sha256 = parse_sha256(&sha256)?;
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--model-size")) {
        return Err(());
    }
    let size = args
        .next()
        .and_then(|value| value.into_string().ok())
        .ok_or(())?
        .parse::<u64>()
        .map_err(|_| ())?;
    if size == 0 {
        return Err(());
    }
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--model")) {
        return Err(());
    }
    let path = args.next().map(PathBuf::from).ok_or(())?;
    if args.next().is_some() {
        return Err(());
    }
    let text = path.to_str().ok_or(())?;
    let descriptor = text.strip_prefix("/proc/self/fd/").ok_or(())?;
    if descriptor.is_empty()
        || !descriptor.bytes().all(|byte| byte.is_ascii_digit())
        || descriptor.parse::<i32>().map_err(|_| ())? < 256
    {
        return Err(());
    }
    Ok(ModelBinding {
        compute,
        path,
        sha256,
        size,
    })
}

fn parse_sha256(value: &str) -> Result<[u8; 32], ()> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(());
    }
    let mut digest = [0_u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let text = std::str::from_utf8(pair).map_err(|_| ())?;
        digest[index] = u8::from_str_radix(text, 16).map_err(|_| ())?;
    }
    Ok(digest)
}

fn validate_model_binding(binding: &ModelBinding) -> Result<(), ()> {
    let mut file = std::fs::File::open(&binding.path).map_err(|_| ())?;
    let metadata = file.metadata().map_err(|_| ())?;
    if !metadata.is_file()
        || metadata.len() != binding.size
        || metadata.permissions().mode() & 0o7777 != 0o400
    {
        return Err(());
    }
    let required_seals = rustix::fs::SealFlags::WRITE
        | rustix::fs::SealFlags::GROW
        | rustix::fs::SealFlags::SHRINK
        | rustix::fs::SealFlags::FUTURE_WRITE
        | rustix::fs::SealFlags::EXEC
        | rustix::fs::SealFlags::SEAL;
    if rustix::fs::fcntl_get_seals(&file).map_err(|_| ())? != required_seals {
        return Err(());
    }
    let mut hasher = Sha256::new();
    let mut observed = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|_| ())?;
        if count == 0 {
            break;
        }
        observed = observed
            .checked_add(u64::try_from(count).map_err(|_| ())?)
            .ok_or(())?;
        if observed > binding.size {
            return Err(());
        }
        hasher.update(&buffer[..count]);
    }
    let digest: [u8; 32] = hasher.finalize().into();
    if observed != binding.size || digest != binding.sha256 {
        return Err(());
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use narrivy_asr_api::{
        ContextBias, SpokenLanguage, UNSUPPORTED_CONTEXT_BIAS, UNSUPPORTED_LANGUAGE_DETECT,
        UNSUPPORTED_TRANSLATION,
    };
    use std::io::Write;

    fn tag(raw: &str) -> AsrTag {
        AsrTag::parse(raw).expect("test tag")
    }

    fn tiny_en(accepts_whisper: bool) -> WorkerCapabilities {
        WorkerCapabilities {
            languages: vec![tag("en")],
            accepts_whisper_run_options: accepts_whisper,
            ..WorkerCapabilities::default()
        }
    }

    #[test]
    fn binding_arguments_require_exact_digest_size_and_inherited_fd_path() -> Result<(), String> {
        let digest = "ab".repeat(32);
        let parse = |values: &[&str]| {
            let mut args = values.iter().map(|value| std::ffi::OsString::from(*value));
            parse_model_binding_from(&mut args)
        };
        let binding = parse(&[
            "--model-sha256",
            &digest,
            "--model-size",
            "8",
            "--model",
            "/proc/self/fd/256",
        ])
        .map_err(|()| "exact binding arguments were rejected".to_owned())?;
        assert_eq!(binding.size, 8);
        assert_eq!(binding.sha256, [0xab; 32]);
        assert_eq!(binding.compute, ComputeRequest::Cpu);
        let tail = [
            "--model-sha256",
            digest.as_str(),
            "--model-size",
            "8",
            "--model",
            "/proc/self/fd/256",
        ];
        for (value, expected) in [("auto", ComputeRequest::Auto), ("cpu", ComputeRequest::Cpu)] {
            let mut arguments = vec!["--compute", value];
            arguments.extend(tail);
            assert_eq!(
                parse(&arguments).map(|binding| binding.compute),
                Ok(expected)
            );
        }
        {
            let mut arguments = vec!["--compute", "gpu"];
            arguments.extend(tail);
            assert_eq!(
                parse(&arguments).map(|binding| binding.compute),
                Ok(ComputeRequest::Gpu)
            );
        }
        for unknown in [vec!["--compute", "tpu"], vec!["--compute"]] {
            let mut arguments = unknown;
            arguments.extend(tail);
            assert!(parse(&arguments).is_err());
        }
        for invalid in [
            vec!["--model", "/tmp/model.gguf"],
            vec![
                "--model-sha256",
                &digest,
                "--model-size",
                "8",
                "--model",
                "/tmp/model.gguf",
            ],
            vec![
                "--model-sha256",
                &digest,
                "--model-size",
                "0",
                "--model",
                "/proc/self/fd/256",
            ],
            vec![
                "--model-sha256",
                "00",
                "--model-size",
                "8",
                "--model",
                "/proc/self/fd/256",
            ],
        ] {
            assert!(parse(&invalid).is_err());
        }
        Ok(())
    }

    #[test]
    fn binding_validation_requires_exact_sealed_read_only_bytes()
    -> Result<(), Box<dyn std::error::Error>> {
        let bytes = b"GGUF\x03\0\0\0worker-binding";
        let mut file = std::fs::File::from(rustix::fs::memfd_create(
            "worker-binding-test",
            rustix::fs::MemfdFlags::CLOEXEC | rustix::fs::MemfdFlags::ALLOW_SEALING,
        )?);
        file.write_all(bytes)?;
        let path = PathBuf::from(format!(
            "/proc/self/fd/{}",
            std::os::fd::AsRawFd::as_raw_fd(&file)
        ));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400))?;
        let seals = rustix::fs::SealFlags::WRITE
            | rustix::fs::SealFlags::GROW
            | rustix::fs::SealFlags::SHRINK
            | rustix::fs::SealFlags::FUTURE_WRITE
            | rustix::fs::SealFlags::EXEC
            | rustix::fs::SealFlags::SEAL;
        rustix::fs::fcntl_add_seals(&file, seals)?;
        let digest: [u8; 32] = Sha256::digest(bytes).into();
        assert!(
            validate_model_binding(&ModelBinding {
                compute: ComputeRequest::Cpu,
                path,
                sha256: digest,
                size: u64::try_from(bytes.len())?,
            })
            .is_ok()
        );
        Ok(())
    }

    #[test]
    fn default_and_explicit_english_build_run_options_without_guessing() {
        let caps = tiny_en(true);
        let default = build_run_options(&RecognitionOptions::default(), &caps).expect("default");
        assert_eq!(default.task, Task::Transcribe);
        assert_eq!(default.timestamps, TimestampKind::None);
        assert_eq!(default.language, None);
        assert_eq!(default.target_language, None);
        assert!(default.family.is_none());

        let explicit = RecognitionOptions {
            spoken_language: SpokenLanguage::Explicit(tag("en")),
            ..RecognitionOptions::default()
        };
        let run = build_run_options(&explicit, &caps).expect("explicit en");
        assert_eq!(run.language.as_deref(), Some("en"));
        assert_eq!(run.task, Task::Transcribe);
        assert!(run.family.is_none());

        let auto = RecognitionOptions {
            spoken_language: SpokenLanguage::AutoDetect,
            ..RecognitionOptions::default()
        };
        assert_eq!(
            build_run_options(&auto, &caps).unwrap_err().code,
            UNSUPPORTED_LANGUAGE_DETECT
        );
        let translate = RecognitionOptions {
            translation_target: Some(tag("en")),
            ..RecognitionOptions::default()
        };
        assert_eq!(
            build_run_options(&translate, &caps).unwrap_err().code,
            UNSUPPORTED_TRANSLATION
        );
    }

    #[test]
    fn translation_sets_task_only_with_an_attested_target() {
        let caps = WorkerCapabilities {
            supports_language_detect: true,
            supports_translate: true,
            languages: vec![tag("en"), tag("de")],
            translate_target_languages: vec![tag("en")],
            ..WorkerCapabilities::default()
        };
        let run = build_run_options(
            &RecognitionOptions {
                spoken_language: SpokenLanguage::Explicit(tag("de")),
                translation_target: Some(tag("en")),
                ..RecognitionOptions::default()
            },
            &caps,
        )
        .expect("translate");
        assert_eq!(run.task, Task::Translate);
        assert_eq!(run.target_language.as_deref(), Some("en"));
        assert_eq!(run.language.as_deref(), Some("de"));

        assert_eq!(
            build_run_options(
                &RecognitionOptions {
                    translation_target: Some(tag("ja")),
                    ..RecognitionOptions::default()
                },
                &caps,
            )
            .unwrap_err()
            .code,
            UNSUPPORTED_TRANSLATION
        );
    }

    #[test]
    fn whisper_run_extension_is_used_only_when_attested_and_omits_hint_text() {
        let hint = "secret-hotword";
        let options = RecognitionOptions {
            context_bias: Some(ContextBias::try_new([hint]).unwrap()),
            ..RecognitionOptions::default()
        };
        let accepted = build_run_options(&options, &tiny_en(true)).expect("bias");
        match accepted.family {
            Some(RunExtension::Whisper(whisper)) => {
                assert_eq!(whisper.initial_prompt.as_deref(), Some(hint));
            }
            other => panic!("expected whisper run options, got {other:?}"),
        }

        let error = build_run_options(&options, &tiny_en(false)).unwrap_err();
        assert_eq!(error.code, UNSUPPORTED_CONTEXT_BIAS);
        assert!(
            !error.message.contains(hint),
            "context text leaked into error message"
        );
        assert!(
            !format!("{error:?}").contains(hint),
            "context text leaked into error debug"
        );
        assert!(
            !format!("{options:?}").contains(hint),
            "context text leaked into options debug"
        );
    }

    #[test]
    fn bounded_tags_fail_closed_on_invalid_or_oversized_attestations() {
        let tags = bounded_tags(["en", "de", "en-US"]).expect("valid tags");
        assert_eq!(
            tags.iter().map(AsrTag::as_str).collect::<Vec<_>>(),
            ["en", "de", "en-us"]
        );
        assert!(bounded_tags(["en", "!"]).is_err());
        let too_many: Vec<String> = (0..=MAX_ATTESTED_TAGS)
            .map(|index| format!("en-x{index:02}"))
            .collect();
        assert!(bounded_tags(&too_many).is_err());
    }
}
