//! Process-isolated resident OpenVINO speech worker.
//!
//! stdin/stdout carry the private resident protocol shared with the
//! transcribe.cpp worker; stderr is not used. The OpenVINO runtime is loaded
//! from the installed runtime pack, never from the system. Transcript and
//! audio content is never written to disk or logs.
//!
//! Arguments, in order:
//! `--runtime DIR --model-dir DIR --cache-dir DIR [--compute auto|npu|gpu|cpu]
//!  --model-sha256 HEX --model-size N --model /proc/self/fd/N`
//! The last three bind the model pack's manifest, passed as a sealed
//! read-only descriptor; every file it lists is checked before loading.

use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::Instant;

use narrivy_asr_api::resident_protocol::{
    self, ComputeRequest, MAX_RECOGNITION_OPTIONS_BYTES, MAX_SAMPLES, OP_DECODE, OP_SHUTDOWN,
    OP_STREAM_BEGIN, PCM_TRANSFER_BYTES, PCM_TRANSFER_SAMPLES, STATUS_DECODE_FAILED,
    STATUS_INVALID_AUDIO, STATUS_INVALID_REQUEST, STATUS_MODEL_BINDING_FAILED,
    STATUS_MODEL_LOAD_FAILED, STATUS_NATIVE_BACKEND_FAILED, STATUS_NATIVE_STARTUP_OTHER, STATUS_OK,
    STATUS_READY, STATUS_STREAMING_UNSUPPORTED, STATUS_UNSUPPORTED_REQUEST, WorkerCapabilities,
    WorkerCompute,
};
use narrivy_asr_api::{AsrTag, MAX_ATTESTED_TAGS, SpokenLanguage, self_test};
use narrivy_asr_openvino::genai::{Runtime, Whisper};
use narrivy_asr_openvino::{PackManifest, runtime_lib_dir, whisper_languages};
use sha2::{Digest, Sha256};

/// The recording every accelerator must decode before it is used.
const SELF_TEST_WAV: &[u8] = include_bytes!("../../../../tests/fixtures/jfk16.wav");

struct Arguments {
    runtime: PathBuf,
    model_dir: PathBuf,
    cache_dir: PathBuf,
    compute: ComputeRequest,
    manifest: PathBuf,
    sha256: [u8; 32],
    size: u64,
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(()) => std::process::ExitCode::from(1),
    }
}

fn run() -> Result<(), ()> {
    let stdout = std::io::stdout();
    let mut writer = stdout.lock();
    let Ok(arguments) = parse_arguments(&mut std::env::args_os().skip(1)) else {
        return refuse(&mut writer, STATUS_MODEL_BINDING_FAILED);
    };
    let Ok(manifest_bytes) = read_bound_manifest(&arguments) else {
        return refuse(&mut writer, STATUS_MODEL_BINDING_FAILED);
    };
    let Ok(manifest) = PackManifest::parse(&manifest_bytes) else {
        return refuse(&mut writer, STATUS_MODEL_LOAD_FAILED);
    };
    if manifest.verify(&arguments.model_dir).is_err() {
        return refuse(&mut writer, STATUS_MODEL_LOAD_FAILED);
    }
    let wants_npu = matches!(
        arguments.compute,
        ComputeRequest::Auto | ComputeRequest::Npu
    );
    let Ok(runtime) = Runtime::load(&runtime_lib_dir(&arguments.runtime), wants_npu) else {
        return refuse(&mut writer, STATUS_NATIVE_STARTUP_OTHER);
    };
    let Some((mut whisper, compute)) = load(&runtime, &arguments) else {
        return refuse(&mut writer, STATUS_NATIVE_BACKEND_FAILED);
    };
    let Ok(capabilities) = capabilities(&arguments, compute, whisper.takes_prompt()) else {
        return refuse(&mut writer, STATUS_NATIVE_STARTUP_OTHER);
    };
    let Ok(payload) = resident_protocol::encode_capabilities(&capabilities) else {
        return refuse(&mut writer, STATUS_NATIVE_STARTUP_OTHER);
    };
    // Compiling leaves freed heap behind; hand it back before serving.
    narrivy_asr_openvino::genai::release_free_memory();
    resident_protocol::write_response(&mut writer, STATUS_READY, 0, &payload).map_err(|_| ())?;

    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    loop {
        let Some(header) = resident_protocol::read_request(&mut reader).map_err(|_| ())? else {
            return Ok(());
        };
        match header.operation {
            OP_SHUTDOWN => return Ok(()),
            OP_DECODE => {
                let options_payload = read_options_payload(&mut reader).map_err(|_| ())?;
                let sample_count = usize::try_from(header.sample_count).map_err(|_| ())?;
                let samples = match read_pcm(&mut reader, sample_count) {
                    Ok(samples) => samples,
                    Err(PcmReadError::Invalid) => {
                        respond(&mut writer, STATUS_INVALID_AUDIO, header.request_id, &[])?;
                        continue;
                    }
                    Err(PcmReadError::Io) => return Err(()),
                };
                let Ok(options) = resident_protocol::decode_recognition_options(&options_payload)
                else {
                    respond(&mut writer, STATUS_INVALID_REQUEST, header.request_id, &[])?;
                    continue;
                };
                if capabilities
                    .to_asr_capabilities()
                    .validate(&options.to_asr_request())
                    .is_err()
                {
                    respond(
                        &mut writer,
                        STATUS_UNSUPPORTED_REQUEST,
                        header.request_id,
                        &[],
                    )?;
                    continue;
                }
                let language = match &options.spoken_language {
                    SpokenLanguage::Explicit(tag) => Some(tag.as_str().to_owned()),
                    _ => None,
                };
                let terms: Vec<&str> = options
                    .context_bias
                    .as_ref()
                    .map(|bias| bias.hints().iter().map(String::as_str).collect())
                    .unwrap_or_default();
                let prompt = (!terms.is_empty()).then(|| format!("{}.", terms.join(", ")));
                let mut outcome =
                    whisper.transcribe_with_prompt(&samples, language.as_deref(), prompt.as_deref());
                // Whisper sometimes continues the prompt instead of hearing
                // the audio: recognise again without it.
                if prompt.is_some() && outcome.as_deref().is_ok_and(|text| echoes_prompt(text, &terms)) {
                    outcome = whisper.transcribe(&samples, language.as_deref());
                }
                match outcome {
                    Ok(text) if text.len() <= resident_protocol::MAX_TEXT_BYTES => {
                        respond(&mut writer, STATUS_OK, header.request_id, text.as_bytes())?;
                    }
                    Ok(_) | Err(_) => {
                        respond(&mut writer, STATUS_DECODE_FAILED, header.request_id, &[])?;
                    }
                }
            }
            // OpenVINO Whisper recognises whole recordings; the parent only
            // streams to workers that attest streaming.
            OP_STREAM_BEGIN => {
                respond(
                    &mut writer,
                    STATUS_STREAMING_UNSUPPORTED,
                    header.request_id,
                    &[],
                )?;
            }
            _ => return Err(()),
        }
    }
}

fn refuse(writer: &mut impl std::io::Write, status: u8) -> Result<(), ()> {
    resident_protocol::write_response(writer, status, 0, &[]).map_err(|_| ())?;
    Err(())
}

fn respond(
    writer: &mut impl std::io::Write,
    status: u8,
    request_id: u128,
    payload: &[u8],
) -> Result<(), ()> {
    resident_protocol::write_response(writer, status, request_id, payload).map_err(|_| ())
}

/// Load the model where the parent asked. `Auto` tries the NPU, then the
/// GPU, then the CPU; an accelerator is used only after it decodes the
/// known recording correctly and faster than real time. A strict request
/// does not fall back.
fn load<'runtime>(
    runtime: &'runtime Runtime,
    arguments: &Arguments,
) -> Option<(Whisper<'runtime>, WorkerCompute)> {
    let available = runtime.devices().ok()?;
    let order: &[(&str, WorkerCompute)] = match arguments.compute {
        ComputeRequest::Npu => &[("NPU", WorkerCompute::Npu)],
        ComputeRequest::Gpu => &[("GPU", WorkerCompute::IntegratedGpu)],
        ComputeRequest::Cpu => &[("CPU", WorkerCompute::Cpu)],
        ComputeRequest::Auto => &[
            ("NPU", WorkerCompute::Npu),
            ("GPU", WorkerCompute::IntegratedGpu),
            ("CPU", WorkerCompute::Cpu),
        ],
    };
    let cache = arguments.cache_dir.to_str()?;
    for (device, compute) in order {
        if !available
            .iter()
            .any(|name| name == device || name.starts_with(&format!("{device}.")))
        {
            continue;
        }
        let properties: &[(&str, &str)] = match *compute {
            WorkerCompute::Cpu => &[],
            // The plugin's own compiler: the NPU driver's compiler in the
            // pack can be older than this runtime accepts.
            WorkerCompute::Npu => &[("CACHE_DIR", cache), ("NPU_COMPILER_TYPE", "PLUGIN")],
            _ => &[("CACHE_DIR", cache)],
        };
        let Ok(mut whisper) = runtime.whisper(&arguments.model_dir, device, properties) else {
            continue;
        };
        if *compute == WorkerCompute::Cpu || passes_self_test(&mut whisper) {
            return Some((whisper, *compute));
        }
    }
    None
}

fn passes_self_test(whisper: &mut Whisper<'_>) -> bool {
    let Some(samples) = self_test::fixture_samples(SELF_TEST_WAV) else {
        return false;
    };
    // The first decode on a new device also builds its kernels; time the
    // second, which is what dictation will see.
    if whisper.transcribe(&samples, None).is_err() {
        return false;
    }
    let started = Instant::now();
    whisper.transcribe(&samples, None).is_ok_and(|text| {
        self_test::phrase_matches(&text)
            && self_test::faster_than_real_time(started.elapsed(), samples.len())
    })
}

fn capabilities(
    arguments: &Arguments,
    compute: WorkerCompute,
    takes_prompt: bool,
) -> Result<WorkerCapabilities, ()> {
    let config =
        std::fs::read(arguments.model_dir.join("generation_config.json")).unwrap_or_default();
    let codes = whisper_languages(&config);
    if codes.len() > MAX_ATTESTED_TAGS {
        return Err(());
    }
    let languages = codes
        .iter()
        .map(|code| AsrTag::parse(code).map_err(|_| ()))
        .collect::<Result<Vec<_>, ()>>()?;
    Ok(WorkerCapabilities {
        compute,
        supports_streaming: false,
        supports_language_detect: languages.len() > 1,
        supports_translate: false,
        // The user's words reach the model as its initial prompt.
        accepts_whisper_run_options: takes_prompt,
        languages,
        translate_target_languages: Vec::new(),
    })
}

/// Normalised words: lower case, letters and digits only.
fn plain_words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| c.is_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect::<String>()
        })
        .filter(|w| !w.is_empty())
        .collect()
}

/// Whether `text` repeats the prompt rather than the audio: three or more
/// prompt entries in the prompt's own order, the same entry three times, or
/// only prompt entries.
fn echoes_prompt(text: &str, terms: &[&str]) -> bool {
    let entries: Vec<String> = terms
        .iter()
        .flat_map(|hint| hint.split(','))
        .map(|entry| plain_words(entry).join(" "))
        .filter(|entry| !entry.is_empty())
        .collect();
    if entries.is_empty() {
        return false;
    }
    let spoken = plain_words(text).join(" ");
    if spoken.is_empty() {
        return false;
    }
    let found: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter_map(|(i, entry)| spoken.find(entry.as_str()).map(|_| i))
        .collect();
    // In order: entry i, i+1, i+2 appear one after another in the text.
    let in_order = entries.windows(3).any(|run| spoken.contains(&run.join(" ")));
    let looped = entries
        .iter()
        .any(|entry| spoken.matches(entry.as_str()).count() >= 3);
    let only_entries = !found.is_empty() && {
        let mut rest = spoken.clone();
        for entry in &entries {
            rest = rest.replace(entry.as_str(), " ");
        }
        rest.split_whitespace().next().is_none() && found.len() >= 2
    };
    in_order || looped || only_entries
}

fn parse_arguments(args: &mut impl Iterator<Item = std::ffi::OsString>) -> Result<Arguments, ()> {
    let mut path_after = |flag: &str| -> Result<PathBuf, ()> {
        if args.next().as_deref() != Some(std::ffi::OsStr::new(flag)) {
            return Err(());
        }
        let path = args.next().map(PathBuf::from).ok_or(())?;
        if path.is_absolute() {
            Ok(path)
        } else {
            Err(())
        }
    };
    let runtime = path_after("--runtime")?;
    let model_dir = path_after("--model-dir")?;
    let cache_dir = path_after("--cache-dir")?;
    let mut next = args.next().ok_or(())?;
    let mut compute = ComputeRequest::Cpu;
    if next == ComputeRequest::ARGUMENT {
        let value = args.next().ok_or(())?;
        compute = ComputeRequest::from_argument(value.to_str().ok_or(())?).ok_or(())?;
        next = args.next().ok_or(())?;
    }
    if next != "--model-sha256" {
        return Err(());
    }
    let sha256 =
        narrivy_asr_openvino::parse_sha256(args.next().ok_or(())?.to_str().ok_or(())?).ok_or(())?;
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--model-size")) {
        return Err(());
    }
    let size = args
        .next()
        .ok_or(())?
        .to_str()
        .ok_or(())?
        .parse::<u64>()
        .map_err(|_| ())?;
    if size == 0 || size > narrivy_asr_openvino::MAX_MANIFEST_BYTES {
        return Err(());
    }
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--model")) {
        return Err(());
    }
    let manifest = args.next().map(PathBuf::from).ok_or(())?;
    if args.next().is_some() {
        return Err(());
    }
    let descriptor = manifest
        .to_str()
        .and_then(|text| text.strip_prefix("/proc/self/fd/"))
        .ok_or(())?;
    if descriptor.is_empty()
        || !descriptor.bytes().all(|byte| byte.is_ascii_digit())
        || descriptor.parse::<i32>().map_err(|_| ())? < 256
    {
        return Err(());
    }
    Ok(Arguments {
        runtime,
        model_dir,
        cache_dir,
        compute,
        manifest,
        sha256,
        size,
    })
}

/// Read the manifest from the sealed descriptor the parent passed, and
/// check it is exactly the bound bytes.
fn read_bound_manifest(arguments: &Arguments) -> Result<Vec<u8>, ()> {
    let mut file = std::fs::File::open(&arguments.manifest).map_err(|_| ())?;
    let metadata = file.metadata().map_err(|_| ())?;
    if !metadata.is_file()
        || metadata.len() != arguments.size
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
    let mut bytes = Vec::new();
    (&mut file)
        .take(arguments.size + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ())?;
    let digest: [u8; 32] = Sha256::digest(&bytes).into();
    if u64::try_from(bytes.len()).map_err(|_| ())? != arguments.size || digest != arguments.sha256 {
        return Err(());
    }
    Ok(bytes)
}

enum PcmReadError {
    Invalid,
    Io,
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
        let transfer_samples = (sample_count - samples.len()).min(PCM_TRANSFER_SAMPLES);
        let transfer_bytes = transfer_samples * size_of::<f32>();
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

#[cfg(test)]
mod echo_tests {
    use super::echoes_prompt;

    #[test]
    fn a_continued_prompt_is_spotted() {
        let terms = ["Narrivy, OpenVINO, Waqar, Lahore"];
        assert!(echoes_prompt("Narrivy, OpenVINO, Waqar.", &terms));
        assert!(echoes_prompt("Lahore Lahore Lahore", &terms));
        assert!(echoes_prompt("Waqar, Lahore", &terms));
        assert!(!echoes_prompt("Send the Narrivy build to Waqar today.", &terms));
        assert!(!echoes_prompt("Thanks, see you tomorrow.", &terms));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(values: &[&str]) -> Result<Arguments, ()> {
        let mut args = values.iter().map(|value| std::ffi::OsString::from(*value));
        parse_arguments(&mut args)
    }

    #[test]
    fn arguments_bind_absolute_directories_and_a_high_descriptor() {
        let digest = "ab".repeat(32);
        let mut full = vec![
            "--runtime",
            "/r",
            "--model-dir",
            "/m",
            "--cache-dir",
            "/c",
            "--compute",
            "npu",
            "--model-sha256",
            &digest,
            "--model-size",
            "10",
            "--model",
            "/proc/self/fd/300",
        ];
        let parsed = parse(&full);
        assert!(
            parsed
                .as_ref()
                .is_ok_and(|a| a.compute == ComputeRequest::Npu && a.size == 10)
        );
        full[1] = "relative";
        assert!(parse(&full).is_err());
        full[1] = "/r";
        full[13] = "/proc/self/fd/3";
        assert!(parse(&full).is_err());
        full[13] = "/tmp/manifest";
        assert!(parse(&full).is_err());
        // Without --compute the request is strict CPU.
        let short = [
            "--runtime",
            "/r",
            "--model-dir",
            "/m",
            "--cache-dir",
            "/c",
            "--model-sha256",
            &digest,
            "--model-size",
            "10",
            "--model",
            "/proc/self/fd/300",
        ];
        assert!(parse(&short).is_ok_and(|a| a.compute == ComputeRequest::Cpu));
    }
}
