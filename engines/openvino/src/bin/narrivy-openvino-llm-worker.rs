//! Process-isolated OpenVINO language-model worker: clean-up (Refine) and
//! chat for the AI assistant.
//!
//! stdin/stdout carry [`narrivy_asr_openvino::refine_protocol`]; stderr is
//! not used. The runtime is loaded from the installed runtime pack. Text is
//! never written to disk or logs.
//!
//! Arguments, in order:
//! `--runtime DIR --model-dir DIR --cache-dir DIR [--compute auto|npu|gpu|cpu]`
//! The model directory's `model-manifest.json` is verified (every file's
//! size and SHA-256) before anything is loaded.

use std::path::PathBuf;

use narrivy_asr_api::resident_protocol::ComputeRequest;
use narrivy_asr_openvino::genai::{GenAiError, Llm, Picture, Runtime};
use narrivy_asr_openvino::refine_protocol::{
    self, Frame, MAX_TEXT_BYTES, OP_CANCEL, OP_CHAT, OP_CLEAN, OP_SHUTDOWN, REFINE_PROFILE,
    RefineProfile, STATUS_CANCELLED, STATUS_FAILED, STATUS_INVALID, STATUS_LOAD_FAILED, STATUS_OK,
    STATUS_PARTIAL, STATUS_READY,
};
use narrivy_asr_openvino::static_llm::StaticLlm;
use narrivy_asr_openvino::{MAX_MANIFEST_BYTES, MODEL_MANIFEST, PackManifest, runtime_lib_dir};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;

/// Longest prompt the NPU pipeline is compiled for (tokens). A dictation of
/// several minutes fits; longer text is cleaned in pieces by the daemon.
const NPU_MAX_PROMPT_TOKENS: &str = "2048";
/// The NPU reserves room for at least this many generated tokens.
const NPU_MIN_RESPONSE_TOKENS: &str = "1024";
/// Clean-up output is never much longer than its input.
const MAX_NEW_TOKENS_CLEAN: usize = 1536;
/// A chat reply on the NPU (its compiled shape bounds it).
const MAX_NEW_TOKENS_CHAT: usize = 2048;
/// A chat reply elsewhere: room for a whole file written in one tool call.
const MAX_NEW_TOKENS_CHAT_PIPELINE: usize = 8192;

struct Arguments {
    runtime: PathBuf,
    model_dir: PathBuf,
    cache_dir: PathBuf,
    compute: ComputeRequest,
}

fn main() -> std::process::ExitCode {
    // `execve` made this worker dumpable again; it holds audio, transcripts
    // or text being rewritten (D37, F-H18). Same call as
    // `narrivy_process_linux::forbid_core_dumps`.
    if rustix::process::set_dumpable_behavior(rustix::process::DumpableBehavior::NotDumpable)
        .is_err()
    {
        return std::process::ExitCode::from(1);
    }
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(()) => std::process::ExitCode::from(1),
    }
}

fn run() -> Result<(), ()> {
    let stdout = std::io::stdout();
    let mut writer = stdout.lock();
    let refuse = |writer: &mut std::io::StdoutLock<'_>| {
        let _ = refine_protocol::write_frame(writer, STATUS_LOAD_FAILED, 0, &[]);
        Err(())
    };
    let Ok(arguments) = parse_arguments(&mut std::env::args_os().skip(1)) else {
        return refuse(&mut writer);
    };
    let Some(profile) = verified_profile(&arguments) else {
        return refuse(&mut writer);
    };
    let wants_npu = matches!(
        arguments.compute,
        ComputeRequest::Auto | ComputeRequest::Npu
    );
    let Ok(runtime) = Runtime::load(&runtime_lib_dir(&arguments.runtime), wants_npu) else {
        return refuse(&mut writer);
    };
    let loaded = if StaticLlm::is_pack(&arguments.model_dir) {
        load_fixed(&runtime, &arguments, &profile)
            .map(|(llm, device)| (Model::Fixed(Box::new(llm)), device))
    } else {
        load(&runtime, &arguments).map(|(llm, device)| (Model::Pipeline(llm), device))
    };
    let Some((mut model, device)) = loaded else {
        return refuse(&mut writer);
    };
    // Compiling leaves freed heap behind; hand it back before serving.
    narrivy_asr_openvino::genai::release_free_memory();
    refine_protocol::write_frame(&mut writer, STATUS_READY, 0, device.as_bytes())
        .map_err(|_| ())?;

    let extra_context = profile.extra_context.as_ref().map(ToString::to_string);
    // A chat turn asked to think first: the template's thinking switched on.
    let thinking_context = {
        let mut context = profile
            .extra_context
            .clone()
            .filter(serde_json::Value::is_object)
            .unwrap_or_else(|| serde_json::json!({}));
        context["enable_thinking"] = serde_json::Value::Bool(true);
        Some(context.to_string())
    };
    // Requests are read on their own thread, so a cancel can arrive while a
    // chat turn is generating.
    let cancelled = Arc::new(AtomicU64::new(0));
    let (requests, incoming) = mpsc::channel::<Frame>();
    let reader_cancelled = Arc::clone(&cancelled);
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut reader = stdin.lock();
        while let Ok(Some(frame)) = refine_protocol::read_frame(&mut reader) {
            if frame.code == OP_CANCEL {
                reader_cancelled.store(frame.id, Ordering::Release);
                continue;
            }
            if requests.send(frame).is_err() {
                break;
            }
        }
    });
    while let Ok(frame) = incoming.recv() {
        let Ok(text) = std::str::from_utf8(&frame.payload) else {
            refine_protocol::write_frame(&mut writer, STATUS_INVALID, frame.id, &[])
                .map_err(|_| ())?;
            continue;
        };
        let reply = match frame.code {
            OP_SHUTDOWN => return Ok(()),
            OP_CLEAN => match &mut model {
                Model::Pipeline(llm) => llm.chat(
                    &profile.clean_messages(text),
                    extra_context.as_deref(),
                    clean_token_budget(text),
                ),
                Model::Fixed(llm) => llm.generate(
                    &profile.system,
                    &format!("{}{text}", profile.user_prefix),
                    text,
                    clean_token_budget(text),
                    &mut || false,
                ),
            },
            OP_CHAT => {
                let id = frame.id;
                let mut write_failed = false;
                // A thinking turn's template may open the thinking in the
                // prompt: the stream then starts inside it. Clients are
                // shown where it starts.
                let mut opened = !matches!(&model, Model::Pipeline(_))
                    || !request_parts(text).is_some_and(|(_, thinking)| thinking);
                let mut on_text = |piece: &str| {
                    if !opened && !piece.trim().is_empty() {
                        opened = true;
                        if !piece.trim_start().starts_with("<think>")
                            && refine_protocol::write_frame(
                                &mut writer,
                                STATUS_PARTIAL,
                                id,
                                b"<think>\n",
                            )
                            .is_err()
                        {
                            write_failed = true;
                        }
                    }
                    if refine_protocol::write_frame(
                        &mut writer,
                        STATUS_PARTIAL,
                        id,
                        piece.as_bytes(),
                    )
                    .is_err()
                    {
                        write_failed = true;
                    }
                    write_failed || cancelled.load(Ordering::Acquire) == id
                };
                let reply = match &mut model {
                    Model::Pipeline(llm) => match with_pictures(text) {
                        Some((messages, pictures, thinking)) => llm.chat_with_pictures(
                            &messages,
                            if thinking {
                                thinking_context.as_deref()
                            } else {
                                extra_context.as_deref()
                            },
                            MAX_NEW_TOKENS_CHAT_PIPELINE,
                            Some(&mut on_text),
                            &pictures,
                        ),
                        None => Err(GenAiError::Generate),
                    },
                    Model::Fixed(llm) => match chat_messages(text) {
                        Some(messages) => {
                            llm.chat(&messages, MAX_NEW_TOKENS_CHAT, &mut || false, &mut on_text)
                        }
                        None => Err(GenAiError::Generate),
                    },
                };
                if write_failed {
                    return Err(());
                }
                if cancelled.load(Ordering::Acquire) == id {
                    let partial = reply.unwrap_or_default();
                    let partial = strip_thinking(&partial);
                    refine_protocol::write_frame(
                        &mut writer,
                        STATUS_CANCELLED,
                        id,
                        &partial.as_bytes()[..partial.len().min(MAX_TEXT_BYTES)],
                    )
                    .map_err(|_| ())?;
                    continue;
                }
                reply
            }
            _ => {
                refine_protocol::write_frame(&mut writer, STATUS_INVALID, frame.id, &[])
                    .map_err(|_| ())?;
                continue;
            }
        };
        match reply {
            Ok(reply) if reply.len() <= MAX_TEXT_BYTES => {
                let reply = strip_thinking(&reply);
                refine_protocol::write_frame(&mut writer, STATUS_OK, frame.id, reply.as_bytes())
                    .map_err(|_| ())?;
            }
            Ok(_) | Err(_) => {
                refine_protocol::write_frame(&mut writer, STATUS_FAILED, frame.id, &[])
                    .map_err(|_| ())?;
            }
        }
    }
    Ok(())
}

/// At most this many pictures go with one question.
const MAX_PICTURES: usize = 4;
/// Pictures are scaled down to at most this many pixels: enough to read a
/// slide, and few enough image tokens to answer quickly.
const PICTURE_PIXELS: u32 = 1024 * 768;
/// A picture file larger than this is refused.
const MAX_PICTURE_BYTES: u64 = 24 * 1024 * 1024;

/// A chat request: a list of messages, or `{"messages": [...], "thinking":
/// true}` for a turn that thinks before answering.
fn request_parts(text: &str) -> Option<(serde_json::Value, bool)> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    match value {
        serde_json::Value::Array(_) => Some((value, false)),
        serde_json::Value::Object(mut object) => {
            let thinking = object
                .get("thinking")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let messages = object
                .remove("messages")
                .filter(serde_json::Value::is_array)?;
            Some((messages, thinking))
        }
        _ => None,
    }
}

/// A chat request's messages without their `images` lists, the pictures
/// the last user message names (PNG files the daemon chose), and whether it
/// thinks first.
fn with_pictures(text: &str) -> Option<(String, Vec<Picture>, bool)> {
    let (mut value, thinking) = request_parts(text)?;
    let list = value.as_array_mut()?;
    let mut paths = Vec::new();
    let last_user = list
        .iter()
        .rposition(|message| message.get("role").and_then(|r| r.as_str()) == Some("user"));
    for (index, message) in list.iter_mut().enumerate() {
        let object = message.as_object_mut()?;
        if let Some(images) = object.remove("images")
            && Some(index) == last_user
        {
            for path in images.as_array()?.iter().take(MAX_PICTURES) {
                paths.push(PathBuf::from(path.as_str()?));
            }
        }
    }
    let pictures = paths
        .iter()
        .map(|path| picture(path))
        .collect::<Option<Vec<_>>>()?;
    Some((value.to_string(), pictures, thinking))
}

/// Read a PNG and scale it to at most [`PICTURE_PIXELS`].
fn picture(path: &std::path::Path) -> Option<Picture> {
    if !path.is_absolute() || std::fs::metadata(path).ok()?.len() > MAX_PICTURE_BYTES {
        return None;
    }
    let mut reader = image::ImageReader::open(path).ok()?;
    reader.set_format(image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(512 * 1024 * 1024);
    reader.limits(limits);
    let mut rgb = reader.decode().ok()?.to_rgb8();
    let (width, height) = rgb.dimensions();
    let pixels = u64::from(width) * u64::from(height);
    if pixels > u64::from(PICTURE_PIXELS) {
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )]
        let scale = (f64::from(PICTURE_PIXELS) / pixels as f64).sqrt();
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let (w, h) = (
            ((f64::from(width) * scale) as u32).max(32),
            ((f64::from(height) * scale) as u32).max(32),
        );
        rgb = image::imageops::resize(&rgb, w, h, image::imageops::FilterType::Triangle);
    }
    let (width, height) = rgb.dimensions();
    Some(Picture {
        width: width as usize,
        height: height as usize,
        rgb: rgb.into_raw(),
    })
}

/// The `[{"role", "content"}, ...]` list a chat request carries.
fn chat_messages(text: &str) -> Option<Vec<(String, String)>> {
    // The fixed-shape (NPU) runner does not think.
    let (value, _) = request_parts(text)?;
    value
        .as_array()?
        .iter()
        .map(|message| {
            let role = message.get("role")?.as_str()?;
            matches!(role, "system" | "user" | "assistant").then_some(())?;
            Some((
                role.to_owned(),
                message.get("content")?.as_str()?.to_owned(),
            ))
        })
        .collect()
}

/// The loaded model: OpenVINO GenAI's pipeline, or Narrivy's runner for a
/// fixed-shape pack (a model the NPU cannot compile otherwise).
enum Model<'runtime> {
    Pipeline(Llm<'runtime>),
    Fixed(Box<StaticLlm<'runtime>>),
}

/// Load a fixed-shape pack. `Auto` tries the NPU, then the GPU, then the
/// CPU; the trial clean-up also computes the system prompt once.
fn load_fixed<'runtime>(
    runtime: &'runtime Runtime,
    arguments: &Arguments,
    profile: &RefineProfile,
) -> Option<(StaticLlm<'runtime>, &'static str)> {
    let available = runtime.devices().ok()?;
    let order: &[&'static str] = match arguments.compute {
        ComputeRequest::Npu => &["NPU"],
        ComputeRequest::Gpu => &["GPU"],
        ComputeRequest::Cpu => &["CPU"],
        ComputeRequest::Auto => &["NPU", "GPU", "CPU"],
    };
    for device in order {
        if !available
            .iter()
            .any(|name| name == device || name.starts_with(&format!("{device}.")))
        {
            continue;
        }
        let Ok(mut llm) =
            StaticLlm::load(runtime, &arguments.model_dir, device, &arguments.cache_dir)
        else {
            continue;
        };
        let trial: Result<String, GenAiError> = llm.generate(
            &profile.system,
            &format!("{}ok", profile.user_prefix),
            "ok",
            4,
            &mut || false,
        );
        if trial.is_ok() {
            return Some((llm, device));
        }
    }
    None
}

/// Verify the model pack and read its prompt profile.
fn verified_profile(arguments: &Arguments) -> Option<RefineProfile> {
    let manifest_path = arguments.model_dir.join(MODEL_MANIFEST);
    let metadata = std::fs::symlink_metadata(&manifest_path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_MANIFEST_BYTES {
        return None;
    }
    let manifest = PackManifest::parse(&std::fs::read(&manifest_path).ok()?).ok()?;
    if !manifest
        .files
        .iter()
        .any(|file| file.path == REFINE_PROFILE)
    {
        return None;
    }
    manifest.verify(&arguments.model_dir).ok()?;
    RefineProfile::parse(&std::fs::read(arguments.model_dir.join(REFINE_PROFILE)).ok()?)
}

/// Load where the parent asked. `Auto` tries the NPU, then the GPU, then
/// the CPU, keeping the first that loads and answers a trial prompt.
fn load<'runtime>(
    runtime: &'runtime Runtime,
    arguments: &Arguments,
) -> Option<(Llm<'runtime>, &'static str)> {
    let available = runtime.devices().ok()?;
    let order: &[&'static str] = match arguments.compute {
        ComputeRequest::Npu => &["NPU"],
        ComputeRequest::Gpu => &["GPU"],
        ComputeRequest::Cpu => &["CPU"],
        // Qwen3.5 models (a vision-language export, or any model with linear
        // attention layers) do not compile for the NPU; they run on the
        // GPU, else the CPU.
        ComputeRequest::Auto
            if is_vision_export(&arguments.model_dir)
                || has_linear_attention(&arguments.model_dir) =>
        {
            &["GPU", "CPU"]
        }
        ComputeRequest::Auto => &["NPU", "GPU", "CPU"],
    };
    let cache = arguments.cache_dir.to_str()?;
    for device in order {
        if !available
            .iter()
            .any(|name| name == device || name.starts_with(&format!("{device}.")))
        {
            continue;
        }
        let properties: &[(&str, &str)] = match *device {
            // The plugin's own compiler: the NPU driver's compiler in the
            // pack can be older than this runtime accepts.
            "NPU" => &[
                ("CACHE_DIR", cache),
                ("MAX_PROMPT_LEN", NPU_MAX_PROMPT_TOKENS),
                ("MIN_RESPONSE_LEN", NPU_MIN_RESPONSE_TOKENS),
                ("NPU_COMPILER_TYPE", "PLUGIN"),
            ],
            // Every device keeps its compiled model, so a model compiles
            // once per driver and runtime version, never on every load.
            _ => &[("CACHE_DIR", cache)],
        };
        let Ok(mut llm) = runtime.llm(&arguments.model_dir, device, properties) else {
            continue;
        };
        // A trial turn: the first generation on a device also builds its
        // kernels, so dictation does not pay for it.
        let trial = serde_json::json!([{"role": "user", "content": "ok"}]).to_string();
        if llm.chat(&trial, None, 4).is_ok() {
            return Some((llm, device));
        }
    }
    None
}

/// Whether the model is a vision-language export (language model split from
/// its vision encoder).
fn is_vision_export(model_dir: &std::path::Path) -> bool {
    model_dir.join("openvino_language_model.xml").is_file()
}

/// Whether the model mixes in linear-attention layers (Qwen3.5's Gated
/// DeltaNet): the NPU compiler cannot schedule them.
fn has_linear_attention(model_dir: &std::path::Path) -> bool {
    std::fs::read(model_dir.join("config.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|config| {
            let layers = config
                .get("layer_types")
                .or_else(|| config.pointer("/text_config/layer_types"))?
                .as_array()?
                .iter()
                .any(|kind| kind.as_str() == Some("linear_attention"));
            Some(layers)
        })
        .unwrap_or(false)
}

/// A clean-up is never much longer than what was said: about one token
/// per four bytes of text, with room to spare. The bound stops a model that
/// has started repeating itself.
fn clean_token_budget(text: &str) -> usize {
    (text.len() / 2 + 32).min(MAX_NEW_TOKENS_CLEAN)
}

/// Drop a reasoning block a model may emit before its answer.
fn strip_thinking(text: &str) -> &str {
    match text.rfind("</think>") {
        Some(end) => text[end + "</think>".len()..].trim(),
        None => text.trim(),
    }
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
    let compute = match args.next() {
        None => ComputeRequest::Cpu,
        Some(flag) if flag == ComputeRequest::ARGUMENT => {
            let value = args.next().ok_or(())?;
            ComputeRequest::from_argument(value.to_str().ok_or(())?).ok_or(())?
        }
        Some(_) => return Err(()),
    };
    if args.next().is_some() {
        return Err(());
    }
    Ok(Arguments {
        runtime,
        model_dir,
        cache_dir,
        compute,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_can_ask_to_think() {
        let (_, thinking) =
            request_parts(r#"{"messages":[{"role":"user","content":"q"}],"thinking":true}"#)
                .unwrap_or((serde_json::Value::Null, false));
        assert!(thinking);
        let (_, thinking) = request_parts(r#"[{"role":"user","content":"q"}]"#)
            .unwrap_or((serde_json::Value::Null, true));
        assert!(!thinking);
        assert!(request_parts(r#"{"thinking":true}"#).is_none());
        assert_eq!(
            chat_messages(r#"{"messages":[{"role":"user","content":"q"}],"thinking":true}"#)
                .map(|m| m.len()),
            Some(1)
        );
    }

    #[test]
    fn chat_messages_take_known_roles_only() {
        let ok =
            chat_messages(r#"[{"role":"system","content":"s"},{"role":"user","content":"q"}]"#);
        assert_eq!(ok.map(|m| m.len()), Some(2));
        assert!(chat_messages(r#"[{"role":"tool","content":"x"}]"#).is_none());
        assert!(chat_messages(r#"{"role":"user"}"#).is_none());
    }

    #[test]
    fn linear_attention_models_are_recognised_from_their_config() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        assert!(!has_linear_attention(dir.path()));
        std::fs::write(
            dir.path().join("config.json"),
            r#"{"layer_types": ["linear_attention", "full_attention"]}"#,
        )?;
        assert!(has_linear_attention(dir.path()));
        std::fs::write(
            dir.path().join("config.json"),
            r#"{"num_hidden_layers": 28}"#,
        )?;
        assert!(!has_linear_attention(dir.path()));
        Ok(())
    }

    #[test]
    fn arguments_need_absolute_directories() {
        let parse = |values: &[&str]| {
            parse_arguments(&mut values.iter().map(|value| std::ffi::OsString::from(*value)))
        };
        let full = [
            "--runtime",
            "/r",
            "--model-dir",
            "/m",
            "--cache-dir",
            "/c",
            "--compute",
            "npu",
        ];
        assert!(parse(&full).is_ok_and(|a| a.compute == ComputeRequest::Npu));
        assert!(parse(&full[..6]).is_ok_and(|a| a.compute == ComputeRequest::Cpu));
        assert!(parse(&["--runtime", "r", "--model-dir", "/m", "--cache-dir", "/c"]).is_err());
        assert!(parse(&[&full[..], &["extra"]].concat()).is_err());
    }

    #[test]
    fn the_clean_up_budget_follows_the_text() {
        assert_eq!(clean_token_budget("hello there"), 37);
        assert_eq!(
            clean_token_budget(&"a".repeat(10_000)),
            MAX_NEW_TOKENS_CLEAN
        );
    }

    #[test]
    fn reasoning_is_dropped_from_the_reply() {
        assert_eq!(strip_thinking("<think>\nhmm\n</think>\n\nHello."), "Hello.");
        assert_eq!(strip_thinking(" Hello. "), "Hello.");
    }
}
