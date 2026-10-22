//! narrivy-cli: thin client of narrivyd (PORT-003). Never owns domain state.
//!
//! P1: real request round-trips over the narrivy-ipc unix socket. Argument
//! surface is stable; parse_args behavior/tests are unchanged.
//!
//! `toggle` is the installed atomic activation command. It sends exactly one
//! `ToggleSession` wire request and never inspects status, starts, or stops
//! on its own: the daemon owns the atomic decision (ADR-driven minor 7). Its
//! installed path is `/usr/bin/narrivy-cli` and its output is typed
//! content-free identity only — no transcript or target content.
//!
//! `support-report` is the user-reachable support report (REL-008): it renders
//! the daemon's content-free advertised diagnostics facts into a redacted
//! `narrivy_diagnostics::SupportReport` and never reads a transcript, the
//! recovery journal, or the retained History.

mod support;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use narrivy_ipc::{
    ActivationSourceDto, Command, Event, RECOGNITION_SETTINGS_PROTOCOL_MINOR,
    RecognitionContextHintsDto, RecognitionSettingsDto, RecognitionTagDto, ResponsePayload,
    SpokenLanguageDto, ToggleResultDto,
};
use narrivy_ipc_unix::{IpcClient, default_socket_path};

use support::SUPPORT_REPORT_COMMAND;

const USAGE: &str = "usage: narrivy-cli <status|start|stop|toggle|cancel|history|models|use-model|download-model|cancel-download|pipeline-engine|pipelines|store|store-remove|meetings|meeting|meeting-start|meeting-stop|meeting-process|meeting-notes|echo-cancel|voice|noise|words|mic|hold|history-keep|numbers|meeting-import|delivery|auto-notes|support-report> [TXID|MODEL-ID|MODEL-ID,DEVICE|MEETING-ID|TITLE|on|off|train|forget] [--pipeline ID] [--socket PATH] [--output PATH]\n       start|toggle recognition: [--language TAG|auto] [--output-language TAG] [--script TAG] [--translate-to TAG] [--locale TAG] [--context-hint TEXT]...\n       toggle accepts only --pipeline raw|clean\n       support-report [--output PATH]: redacted, content-free diagnostics for a bug report";

/// Bounded wait for every request round-trip.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// Starting a dictation may first load the pipeline's own model, which the
/// daemon bounds at about a minute and a half.
const START_TIMEOUT: Duration = Duration::from_secs(100);

/// Bounded wait for the single post-start session event.
const EVENT_TIMEOUT: Duration = Duration::from_secs(2);

/// The exact command a user may bind to a COSMIC (or other desktop)
/// shortcut. Narrivy never installs or edits that shortcut itself.
pub const USER_MANAGED_SHORTCUT_COMMAND: &str = "/usr/bin/narrivy-cli toggle";

#[derive(Debug, Default, PartialEq, Eq)]
struct RecognitionArgs {
    language: Option<String>,
    output_language: Option<String>,
    script: Option<String>,
    translate_to: Option<String>,
    locale: Option<String>,
    context_hints: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct ParsedArgs {
    command: String,
    socket: Option<String>,
    pipeline: Option<String>,
    detail_txid: Option<String>,
    output: Option<String>,
    recognition: RecognitionArgs,
}

fn set_once(slot: &mut Option<String>, flag: &str, value: String) -> Result<(), String> {
    if slot.replace(value).is_some() {
        return Err(format!("{flag} may be provided only once"));
    }
    Ok(())
}

fn parse_args(args: &[String]) -> Result<ParsedArgs, String> {
    let mut command: Option<String> = None;
    let mut socket: Option<String> = None;
    let mut pipeline: Option<String> = None;
    let mut detail_txid: Option<String> = None;
    let mut output: Option<String> = None;
    let mut recognition = RecognitionArgs::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--socket" => {
                let value = args.get(i + 1).ok_or("--socket requires a value")?;
                socket = Some(value.clone());
                i += 1;
            }
            "--output" => {
                let value = args.get(i + 1).ok_or("--output requires a value")?;
                output = Some(value.clone());
                i += 1;
            }
            "--pipeline" => {
                let value = args.get(i + 1).ok_or("--pipeline requires a value")?;
                pipeline = Some(value.clone());
                i += 1;
            }
            flag @ ("--language" | "--output-language" | "--script" | "--translate-to"
            | "--locale" | "--context-hint") => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| format!("{flag} requires a value"))?;
                match flag {
                    "--language" => set_once(&mut recognition.language, flag, value.clone())?,
                    "--output-language" => {
                        set_once(&mut recognition.output_language, flag, value.clone())?
                    }
                    "--script" => set_once(&mut recognition.script, flag, value.clone())?,
                    "--translate-to" => {
                        set_once(&mut recognition.translate_to, flag, value.clone())?
                    }
                    "--locale" => set_once(&mut recognition.locale, flag, value.clone())?,
                    "--context-hint" => recognition.context_hints.push(value.clone()),
                    _ => unreachable!(),
                }
                i += 1;
            }
            other if other.starts_with("--") => return Err(format!("unknown flag {other}")),
            other if command.is_none() => command = Some(other.to_string()),
            other
                if matches!(
                    command.as_deref(),
                    Some(
                        "history"
                            | "use-model"
                            | "pipeline-engine"
                            | "download-model"
                            | "cancel-download"
                            | "store-remove"
                            | "meeting"
                            | "meeting-start"
                            | "meeting-process"
                            | "meeting-notes"
                            | "echo-cancel"
                            | "voice"
                            | "noise"
                            | "words"
                            | "mic"
                            | "hold"
                            | "history-keep"
                            | "numbers"
                            | "meeting-import"
                            | "delivery"
                            | "auto-notes"
                    )
                ) && detail_txid.is_none() =>
            {
                detail_txid = Some(other.to_owned());
            }
            other => return Err(format!("unexpected extra argument {other:?}")),
        }
        i += 1;
    }
    let command = command.ok_or(USAGE)?;
    Ok(ParsedArgs {
        command,
        socket,
        pipeline,
        detail_txid,
        output,
        recognition,
    })
}

fn recognition_settings(args: RecognitionArgs) -> Result<Option<RecognitionSettingsDto>, String> {
    let requested = args.language.is_some()
        || args.output_language.is_some()
        || args.script.is_some()
        || args.translate_to.is_some()
        || args.locale.is_some()
        || !args.context_hints.is_empty();
    if !requested {
        return Ok(None);
    }
    let tag = |value: String| RecognitionTagDto::parse(value).map_err(|error| error.to_string());
    let spoken_language = match args.language.as_deref() {
        None => SpokenLanguageDto::Unspecified,
        Some("auto") => SpokenLanguageDto::AutoDetect,
        Some(value) => SpokenLanguageDto::Explicit {
            tag: tag(value.to_owned())?,
        },
    };
    let context_hints = if args.context_hints.is_empty() {
        None
    } else {
        Some(
            RecognitionContextHintsDto::try_new(args.context_hints)
                .map_err(|error| error.to_string())?,
        )
    };
    Ok(Some(RecognitionSettingsDto {
        spoken_language,
        output_language: args.output_language.map(tag).transpose()?,
        output_script: args.script.map(tag).transpose()?,
        translation_target: args.translate_to.map(tag).transpose()?,
        locale: args.locale.map(tag).transpose()?,
        context_hints,
    }))
}

/// One atomic toggle may select only the canonical Raw/Clean pipeline modes.
///
/// Anything else is refused before a socket connection exists, so no capture
/// or admission side effect is possible. An absent value stays `None` and the
/// daemon's own configured default decides.
fn toggle_pipeline_id(pipeline: Option<&str>) -> Result<Option<String>, String> {
    match pipeline {
        None => Ok(None),
        Some("raw") => Ok(Some("raw".to_owned())),
        Some("clean") => Ok(Some("clean".to_owned())),
        Some(other) => Err(format!(
            "--pipeline {other:?} is not a selectable dictation mode (expected raw or clean)"
        )),
    }
}

/// Content-free rendering of one typed toggle outcome: the installed command
/// prints the outcome plus opaque identity, never transcript/target content.
fn format_toggle_outcome(result: &ToggleResultDto) -> String {
    let mut rendered = format!("outcome={}", result.outcome.wire_tag());
    if let Some(session_id) = result.session_id {
        rendered.push_str(&format!(" session_id={session_id}"));
    }
    if let Some(transaction_id) = &result.transaction_id {
        rendered.push_str(&format!(" tx={transaction_id}"));
    }
    if let Some(pipeline_id) = &result.pipeline_id {
        rendered.push_str(&format!(" pipeline={pipeline_id}"));
    }
    rendered
}

fn format_timings(timings: &narrivy_ipc::StageTimingsUs) -> String {
    timings
        .0
        .iter()
        .map(|(stage, micros)| format!("{stage}={micros}"))
        .collect::<Vec<_>>()
        .join(",")
}

fn connect(socket: &Option<String>) -> Result<IpcClient, ExitCode> {
    let path = match socket {
        Some(path) => std::path::PathBuf::from(path),
        None => default_socket_path(),
    };
    IpcClient::connect_as(&path, "cli").map_err(|err| {
        eprintln!("narrivy-cli: connect {} failed: {err}", path.display());
        ExitCode::from(1)
    })
}

/// One bounded round-trip; prints the typed payload or a rejection summary.
fn round_trip(client: &mut IpcClient, label: &str, command: &Command) -> ExitCode {
    let timeout = match command {
        Command::StartSession { .. } | Command::ToggleSession { .. } => START_TIMEOUT,
        _ => REQUEST_TIMEOUT,
    };
    match client.request(command, timeout) {
        Ok(payload) => print_payload(label, payload),
        Err(err) => {
            eprintln!("narrivy-cli: {label} request failed: {err}");
            ExitCode::from(1)
        }
    }
}

fn print_meeting(command: &str, meeting: &narrivy_ipc::MeetingDto) {
    println!(
        "narrivy-cli {command}: meeting id={} state={} progress={} stage={} model={} speakers={} duration_ms={}",
        meeting.id,
        meeting.state,
        meeting.progress,
        meeting.stage.as_deref().unwrap_or("-"),
        meeting.model,
        meeting.speakers,
        meeting.duration_ms
    );
}

fn print_payload(command: &str, payload: ResponsePayload) -> ExitCode {
    match payload {
        ResponsePayload::Status(status) => {
            println!(
                "narrivy-cli {command}: daemon_build={} protocol_minor={}",
                status.daemon_build, status.protocol_minor
            );
            println!(
                "narrivy-cli {command}: capabilities={}",
                status.capabilities.join(",")
            );
            match status.active_session {
                Some(active) => {
                    println!(
                        "narrivy-cli {command}: active session_id={} state={} pipeline={:?} tx={:?}",
                        active.session_id, active.state, active.pipeline_id, active.transaction_id
                    );
                    if let Some(timings) = active.stage_timings_us {
                        println!(
                            "narrivy-cli {command}: active_timings_us={}",
                            format_timings(&timings)
                        );
                    }
                }
                None => println!("narrivy-cli {command}: no active session"),
            }
            if let Some(timings) = status.last_session_timings_us {
                println!(
                    "narrivy-cli {command}: last_session_timings_us={}",
                    format_timings(&timings)
                );
            }
            if let Some(portal) = status.portal_keyboard {
                println!(
                    "narrivy-cli {command}: portal eligibility={} lifecycle={} can_request={} permission_active={} transport_connected={} sender_available={} delivery_qualified={} cleanup={}",
                    portal.eligibility.wire_tag(),
                    portal.lifecycle.wire_tag(),
                    portal.can_request,
                    portal.permission_active,
                    portal.transport_connected,
                    portal.sender_available,
                    portal.delivery_qualified,
                    portal.cleanup.wire_tag(),
                );
            }
            ExitCode::SUCCESS
        }
        ResponsePayload::Accepted => {
            println!("narrivy-cli {command}: accepted");
            ExitCode::SUCCESS
        }
        ResponsePayload::Meeting(meeting) => {
            print_meeting(command, &meeting);
            ExitCode::SUCCESS
        }
        ResponsePayload::Meetings(meetings) => {
            for meeting in &meetings {
                print_meeting(command, meeting);
            }
            ExitCode::SUCCESS
        }
        // Content-free: counts and the folder, never what was said.
        ResponsePayload::MeetingDetail(detail) => {
            print_meeting(command, &detail.meeting);
            println!(
                "narrivy-cli {command}: turns={} named={} models={} folder={}",
                detail.turns.len(),
                detail.names.len(),
                detail.models.join(","),
                detail.folder
            );
            ExitCode::SUCCESS
        }
        ResponsePayload::AudioOptions(options) => {
            println!(
                "narrivy-cli {command}: echo_cancel={} active={} voice_filter={} trained={} only_my_voice={} training={} noise={}",
                options.echo_cancel,
                options.echo_cancel_active,
                options.voice_filter_available,
                options.voice_trained,
                options.only_my_voice,
                options.voice_training,
                options.noise_suppression
            );
            if command == "mic" {
                let chosen = if options.microphone.is_empty() { "default" } else { &options.microphone };
                println!("narrivy-cli mic: chosen={chosen} missing={}", options.microphone_missing);
                for mic in &options.microphones {
                    println!("narrivy-cli mic: available={}", mic.name);
                }
            }
            ExitCode::SUCCESS
        }
        ResponsePayload::TextOptions(options) => {
            println!(
                "narrivy-cli {command}: numbers_as_digits={} delivery={}",
                options.numbers_as_digits, options.delivery_method
            );
            ExitCode::SUCCESS
        }
        ResponsePayload::MeetingOptions(options) => {
            println!(
                "narrivy-cli {command}: auto_notes={} microphone={} speaker={}",
                options.auto_notes, options.microphone_in_use, options.speaker_in_use
            );
            ExitCode::SUCCESS
        }
        // The library's content can be private: its size only.
        ResponsePayload::MeetingSlide(png) => {
            println!("narrivy-cli {command}: meeting_slide bytes={}", png.len());
            ExitCode::SUCCESS
        }
        ResponsePayload::AssistantLibrary(json) => {
            println!("narrivy-cli {command}: assistant_library bytes={}", json.len());
            ExitCode::SUCCESS
        }
        ResponsePayload::HoldKey(hold) => {
            let key = hold.key.map_or_else(|| "off".to_owned(), |k| k.to_string());
            println!("narrivy-cli {command}: key={key} state={}", hold.state);
            ExitCode::SUCCESS
        }
        // Counts only: words can be private.
        ResponsePayload::Vocabulary(vocabulary) => {
            println!(
                "narrivy-cli {command}: words={} replacements={}",
                vocabulary.words.len(),
                vocabulary.replacements.len()
            );
            ExitCode::SUCCESS
        }
        ResponsePayload::WritingFindings(findings) => {
            println!("narrivy-cli {command}: findings={}", findings.len());
            ExitCode::SUCCESS
        }
        ResponsePayload::WritingRules(rules) => {
            println!("narrivy-cli {command}: rules={}", rules.len());
            ExitCode::SUCCESS
        }
        ResponsePayload::Toggle(result) => {
            println!("narrivy-cli {command}: {}", format_toggle_outcome(&result));
            ExitCode::SUCCESS
        }
        ResponsePayload::History(entries) => {
            for e in entries {
                println!(
                    "narrivy-cli {command}: history tx={} state={} delivered={} updated={}",
                    e.txid, e.state, e.has_delivered_text, e.updated_unix_ms
                );
            }
            ExitCode::SUCCESS
        }
        ResponsePayload::Shortcuts(shortcuts) => {
            println!(
                "narrivy-cli {command}: shortcuts support={} count={}",
                shortcuts.support,
                shortcuts.bindings.len()
            );
            ExitCode::SUCCESS
        }
        ResponsePayload::Models(models) => {
            for model in &models {
                println!(
                    "narrivy-cli {command}: model id={} quant={} engine={} state={} size={} downloaded={}",
                    model.id,
                    model.quant,
                    model.engine,
                    model.state,
                    model.size_bytes,
                    model
                        .downloaded_bytes
                        .map_or_else(|| "-".to_owned(), |bytes| bytes.to_string()),
                );
            }
            ExitCode::SUCCESS
        }
        ResponsePayload::LanguageModels(models) => {
            for model in &models {
                println!(
                    "narrivy-cli {command}: language-model id={} roles={} in_use={} size={}",
                    model.id,
                    model.roles.join(","),
                    model.in_use_for.join(","),
                    model.size_bytes
                );
            }
            ExitCode::SUCCESS
        }
        ResponsePayload::Store(items) => {
            for item in &items {
                println!(
                    "narrivy-cli {command}: item id={} kind={} state={} fit={}:{} source={} size={} installed={}",
                    item.id,
                    item.kind,
                    item.state,
                    item.fit,
                    item.fit_reason.as_deref().unwrap_or("-"),
                    item.source,
                    item.size_bytes,
                    item.installed_bytes
                        .map_or_else(|| "-".to_owned(), |bytes| bytes.to_string()),
                );
            }
            ExitCode::SUCCESS
        }
        ResponsePayload::ShortcutSet(result) => {
            println!("narrivy-cli {command}: shortcut {}", result.outcome);
            ExitCode::SUCCESS
        }
        ResponsePayload::HistoryEntry(detail) => {
            println!(
                "narrivy-cli {command}: entry tx={} state={}",
                detail.txid, detail.state
            );
            if let Some(text) = &detail.delivered {
                println!("narrivy-cli {command}: delivered={text:?}");
            } else if let Some(text) = &detail.corrected {
                println!("narrivy-cli {command}: corrected={text:?}");
            }
            ExitCode::SUCCESS
        }
        ResponsePayload::RecoveryEntry(_) => {
            eprintln!("narrivy-cli {command}: unexpected recovery entry payload");
            ExitCode::from(1)
        }
        ResponsePayload::CapabilityCheck(result) => {
            println!(
                "narrivy-cli {command}: check {} available={} detail={}",
                result.id, result.available, result.detail
            );
            ExitCode::SUCCESS
        }
        ResponsePayload::Pipelines(pipelines) => {
            for pipeline in pipelines {
                println!(
                    "narrivy-cli {command}: pipeline id={} name={:?} v{} enabled={} model={} device={}",
                    pipeline.id,
                    pipeline.name,
                    pipeline.version,
                    pipeline.enabled,
                    pipeline.model_id.as_deref().unwrap_or("-"),
                    pipeline.compute.as_deref().unwrap_or("-"),
                );
            }
            ExitCode::SUCCESS
        }
        ResponsePayload::PortalKeyboardPermission(_) => {
            eprintln!("narrivy-cli {command}: unexpected portal permission payload");
            ExitCode::from(1)
        }
        ResponsePayload::OfflineModelProvision(_) => {
            eprintln!("narrivy-cli {command}: unexpected model provisioning payload");
            ExitCode::from(1)
        }
        ResponsePayload::Rejected { code, detail } => {
            eprintln!(
                "narrivy-cli {command}: rejected code={} detail={detail}",
                code.wire_tag()
            );
            ExitCode::from(1)
        }
    }
}

/// Print exactly one pushed SessionStateChanged after StartSession.
fn print_one_session_event(client: &mut IpcClient) {
    match client.next_event(EVENT_TIMEOUT) {
        Ok(Event::SessionStateChanged {
            session_id,
            state,
            seq,
            ..
        }) => println!("narrivy-cli start: event session_id={session_id} state={state} seq={seq}"),
        Ok(_) => println!("narrivy-cli start: unrelated event received"),
        Err(_) => println!("narrivy-cli start: no session event within {EVENT_TIMEOUT:?}"),
    }
}

/// Produce the user-reachable support report from the daemon's advertised
/// content-free diagnostics facts (REL-008).
///
/// The output is redacted by construction: the report is built only from
/// `GetStatus` capability facts and explicitly chosen client facts, the
/// serializer applies `narrivy_diagnostics::Redactor`, and the report must pass
/// its own content check before a single byte is written. No transcript, audio,
/// clipboard/selection, token, secret, or environment value is read or emitted,
/// and this command never asks the daemon for a transcript, a journal record or
/// a retained History row.
fn run_support_report(socket: &Option<String>, output: Option<String>) -> ExitCode {
    let mut client = match connect(socket) {
        Ok(client) => client,
        Err(code) => return code,
    };
    let status = match client.request(&Command::GetStatus, REQUEST_TIMEOUT) {
        Ok(ResponsePayload::Status(status)) => status,
        Ok(ResponsePayload::Rejected { code, detail }) => {
            eprintln!(
                "narrivy-cli {SUPPORT_REPORT_COMMAND}: rejected code={} detail={}",
                code.wire_tag(),
                detail
            );
            return ExitCode::from(1);
        }
        Ok(_) => {
            eprintln!("narrivy-cli {SUPPORT_REPORT_COMMAND}: unexpected status payload");
            return ExitCode::from(1);
        }
        Err(err) => {
            eprintln!("narrivy-cli {SUPPORT_REPORT_COMMAND}: status request failed: {err}");
            return ExitCode::from(1);
        }
    };
    let report = match support::report_from_status(&status, unix_now_ms()) {
        Ok(report) => report,
        Err(message) => {
            eprintln!("narrivy-cli {SUPPORT_REPORT_COMMAND}: {message}");
            return ExitCode::from(1);
        }
    };
    let (text, check) = match support::render_shareable(&report) {
        Ok(rendered) => rendered,
        Err(message) => {
            eprintln!("narrivy-cli {SUPPORT_REPORT_COMMAND}: {message}");
            return ExitCode::from(1);
        }
    };
    match support::emit(
        &text,
        &check,
        output.as_deref().map(PathBuf::from).as_deref(),
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("narrivy-cli {SUPPORT_REPORT_COMMAND}: {message}");
            ExitCode::from(1)
        }
    }
}

/// Generation time for the report, supplied by this process and never by a
/// daemon-supplied value.
fn unix_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Run one already-parsed command. Split out of `main` so the installed
/// toggle path can be exercised against a real local transport in tests.
fn run_command(
    command: &str,
    socket: &Option<String>,
    pipeline: Option<String>,
    detail_txid: Option<String>,
    recognition: Option<RecognitionSettingsDto>,
    output: Option<String>,
) -> ExitCode {
    if command == SUPPORT_REPORT_COMMAND {
        // The support report is a client-side rendering: it must not inherit
        // session controls, a pipeline selection, or recognition settings.
        if pipeline.is_some() || detail_txid.is_some() || recognition.is_some() {
            eprintln!("narrivy-cli: {SUPPORT_REPORT_COMMAND} accepts only --socket and --output");
            return ExitCode::from(2);
        }
        return run_support_report(socket, output);
    }
    if output.is_some() {
        eprintln!("narrivy-cli: --output is accepted only by {SUPPORT_REPORT_COMMAND}");
        return ExitCode::from(2);
    }
    // Validate the one selectable identity BEFORE connecting: an unsupported
    // mode must have no request and therefore no capture side effect.
    let toggle_pipeline = if command == "toggle" {
        match toggle_pipeline_id(pipeline.as_deref()) {
            Ok(pipeline) => pipeline,
            Err(message) => {
                eprintln!("narrivy-cli: {message}");
                return ExitCode::from(2);
            }
        }
    } else {
        None
    };
    if recognition.is_some() && !matches!(command, "start" | "toggle") {
        eprintln!("narrivy-cli: recognition settings are accepted only by start or toggle");
        return ExitCode::from(2);
    }
    let mut client = match connect(socket) {
        Ok(client) => client,
        Err(code) => return code,
    };
    if recognition.is_some() && client.negotiated_minor() < RECOGNITION_SETTINGS_PROTOCOL_MINOR {
        eprintln!("narrivy-cli: the daemon does not support recognition settings; update Narrivy");
        return ExitCode::from(2);
    }
    match command {
        "status" => round_trip(&mut client, "status", &Command::GetStatus),
        "stop" => round_trip(&mut client, "stop", &Command::StopSession),
        "cancel" => round_trip(&mut client, "cancel", &Command::CancelSession),
        "toggle" => round_trip(
            &mut client,
            "toggle",
            // Exactly one atomic request; never status-then-start/stop.
            &Command::ToggleSession {
                pipeline_id: toggle_pipeline,
                activation: ActivationSourceDto::InstalledCliCommand,
                recognition,
            },
        ),
        "models" => round_trip(&mut client, "models", &Command::ListModels),
        "store" => round_trip(&mut client, "store", &Command::ListStore),
        "store-remove" => match detail_txid {
            Some(id) => round_trip(
                &mut client,
                "store-remove",
                &Command::RemoveStoreItem { id },
            ),
            None => {
                eprintln!("{USAGE}");
                ExitCode::from(2)
            }
        },
        // The positional argument is the model id from `models`.
        "use-model" => match detail_txid {
            Some(id) => round_trip(&mut client, "use-model", &Command::SelectModel { id }),
            None => {
                eprintln!("{USAGE}");
                ExitCode::from(2)
            }
        },
        // The daemon's only network use: fetch a published model by id.
        "download-model" | "cancel-download" => match detail_txid {
            Some(id) if command == "download-model" => round_trip(
                &mut client,
                "download-model",
                &Command::DownloadModel { id },
            ),
            Some(id) => round_trip(
                &mut client,
                "cancel-download",
                &Command::CancelModelDownload { id },
            ),
            None => {
                eprintln!("{USAGE}");
                ExitCode::from(2)
            }
        },
        // `MODEL-ID,DEVICE` for one pipeline; `-` as the model means
        // "whatever model is loaded", and the device defaults to auto.
        "pipeline-engine" => match (detail_txid, pipeline) {
            (Some(choice), Some(pipeline_id)) => {
                let (model, compute) = choice.split_once(',').unwrap_or((choice.as_str(), "auto"));
                round_trip(
                    &mut client,
                    "pipeline-engine",
                    &Command::SetPipelineEngine {
                        pipeline_id,
                        model_id: (model != "-").then(|| model.to_owned()),
                        compute: compute.to_owned(),
                    },
                )
            }
            _ => {
                eprintln!("{USAGE}");
                ExitCode::from(2)
            }
        },
        "pipelines" => round_trip(&mut client, "pipelines", &Command::ListPipelines),
        // Meetings (protocol minor 25): the positional argument is the title
        // for meeting-start and the meeting id for meeting / meeting-process.
        "meeting-start" => round_trip(
            &mut client,
            "meeting-start",
            &Command::MeetingStart { title: detail_txid, watch: false, share: false, microphone: None, computer_sound: None },
        ),
        "meeting-stop" => round_trip(&mut client, "meeting-stop", &Command::MeetingStop),
        "meetings" => round_trip(&mut client, "meetings", &Command::ListMeetings),
        // Removing the computer's sound from the microphone (minor 27):
        // `echo-cancel` shows it, `echo-cancel on|off` sets it.
        "echo-cancel" => match detail_txid.as_deref() {
            None => round_trip(&mut client, "echo-cancel", &Command::GetAudioOptions),
            Some(value @ ("on" | "off")) => round_trip(
                &mut client,
                "echo-cancel",
                &Command::SetAudioOptions {
                    echo_cancel: Some(value == "on"),
                    only_my_voice: None,
                    noise_suppression: None,
                    microphone: None,
                    sounds: None,
                    voice_strictness: None,
                },
            ),
            Some(_) => {
                eprintln!("{USAGE}");
                ExitCode::from(2)
            }
        },
        // Import a recording as a meeting (minor 37): `meeting-import PATH`.
        "meeting-import" => match detail_txid {
            Some(path) => round_trip(
                &mut client,
                "meeting-import",
                &Command::ImportMeeting { path, title: None },
            ),
            None => {
                eprintln!("{USAGE}");
                ExitCode::from(2)
            }
        },
        // Meeting notes written automatically (minor 39): `auto-notes on|off`.
        "auto-notes" => match detail_txid.as_deref() {
            None => round_trip(&mut client, "auto-notes", &Command::GetMeetingOptions),
            Some(value @ ("on" | "off")) => round_trip(
                &mut client,
                "auto-notes",
                &Command::SetMeetingOptions {
                    auto_notes: Some(value == "on"),
                    microphone: None,
                    speaker: None,
                    call_audio: None,
                    room_audio: None,
                    screen: None,
                    process_on_stop: None,
                },
            ),
            Some(_) => {
                eprintln!("{USAGE}");
                ExitCode::from(2)
            }
        },
        // Delivery method (minor 38): `delivery paste|type`.
        "delivery" => match detail_txid {
            None => round_trip(&mut client, "delivery", &Command::GetTextOptions),
            Some(method) => round_trip(
                &mut client,
                "delivery",
                &Command::SetTextOptions {
                    numbers_as_digits: None,
                    delivery_method: Some(method),
                    cleanup_idle_minutes: None,
                    spoken_breaks: None,
                },
            ),
        },
        // Numbers as digits on Clean (minor 35): `numbers on|off`.
        "numbers" => match detail_txid.as_deref() {
            None => round_trip(&mut client, "numbers", &Command::GetTextOptions),
            Some(value @ ("on" | "off")) => round_trip(
                &mut client,
                "numbers",
                &Command::SetTextOptions {
                    numbers_as_digits: Some(value == "on"),
                    delivery_method: None,
                    cleanup_idle_minutes: None,
                    spoken_breaks: None,
                },
            ),
            Some(_) => {
                eprintln!("{USAGE}");
                ExitCode::from(2)
            }
        },
        // How long History is kept (minor 34): `history-keep off|7d|30d|forever`.
        "history-keep" => match detail_txid {
            Some(retention) => round_trip(
                &mut client,
                "history-keep",
                &Command::SetHistoryRetention { retention },
            ),
            None => {
                eprintln!("{USAGE}");
                ExitCode::from(2)
            }
        },
        // Press and hold (minor 32): `hold` shows it, `hold CODE|off`.
        "hold" => match detail_txid.as_deref() {
            None => round_trip(&mut client, "hold", &Command::GetHoldKey),
            Some("off") => round_trip(&mut client, "hold", &Command::SetHoldKey { key: None }),
            Some(code) => match code.parse::<u16>() {
                Ok(key) => round_trip(&mut client, "hold", &Command::SetHoldKey { key: Some(key) }),
                Err(_) => {
                    eprintln!("{USAGE}");
                    ExitCode::from(2)
                }
            },
        },
        // The microphone (minor 31): `mic` lists them, `mic NAME|default`.
        "mic" => match detail_txid {
            None => round_trip(&mut client, "mic", &Command::GetAudioOptions),
            Some(name) => round_trip(
                &mut client,
                "mic",
                &Command::SetAudioOptions {
                    echo_cancel: None,
                    only_my_voice: None,
                    noise_suppression: None,
                    microphone: Some(if name == "default" { String::new() } else { name }),
                    sounds: None,
                    voice_strictness: None,
                },
            ),
        },
        // The vocabulary (minor 30): `words` shows counts, `words add WORD`.
        "words" => match detail_txid {
            None => round_trip(&mut client, "words", &Command::GetVocabulary),
            Some(word) => {
                let current = match client.request(&Command::GetVocabulary, REQUEST_TIMEOUT) {
                    Ok(ResponsePayload::Vocabulary(v)) => v,
                    _ => {
                        eprintln!("narrivy-cli words: the vocabulary could not be read");
                        return ExitCode::from(1);
                    }
                };
                let mut vocabulary = current;
                vocabulary.words.push(word);
                round_trip(&mut client, "words", &Command::SetVocabulary { vocabulary })
            }
        },
        // Noise suppression level (minor 29): `noise off|standard|strong`.
        "noise" => match detail_txid {
            Some(level) => round_trip(
                &mut client,
                "noise",
                &Command::SetAudioOptions {
                    echo_cancel: None,
                    only_my_voice: None,
                    noise_suppression: Some(level),
                    microphone: None,
                    sounds: None,
                    voice_strictness: None,
                },
            ),
            None => round_trip(&mut client, "noise", &Command::GetAudioOptions),
        },
        // The voice filter (minor 28): `voice` shows it, `voice train` records
        // about 20 s of the user reading, `voice forget` removes the profile,
        // `voice on|off` switches "Only my voice".
        "voice" => match detail_txid.as_deref() {
            None => round_trip(&mut client, "voice", &Command::GetAudioOptions),
            Some("train") => round_trip(&mut client, "voice", &Command::TrainVoice),
            Some("forget") => round_trip(&mut client, "voice", &Command::ForgetVoice),
            Some(value @ ("on" | "off")) => round_trip(
                &mut client,
                "voice",
                &Command::SetAudioOptions {
                    echo_cancel: None,
                    only_my_voice: Some(value == "on"),
                    noise_suppression: None,
                    microphone: None,
                    sounds: None,
                    voice_strictness: None,
                },
            ),
            Some(_) => {
                eprintln!("{USAGE}");
                ExitCode::from(2)
            }
        },
        // Detailed notes; they are saved with the meeting as notes-notes.md.
        "meeting-notes" => match detail_txid {
            Some(id) => round_trip(
                &mut client,
                "meeting-notes",
                &Command::SummarizeMeeting {
                    turn_id: format!("cli-{}", std::process::id()),
                    id,
                    kind: "notes".to_owned(),
                    instruction: None,
                },
            ),
            None => {
                eprintln!("{USAGE}");
                ExitCode::from(2)
            }
        },
        "meeting" | "meeting-process" => match detail_txid {
            Some(id) if command == "meeting" => {
                round_trip(&mut client, "meeting", &Command::GetMeeting { id })
            }
            Some(id) => round_trip(
                &mut client,
                "meeting-process",
                &Command::ProcessMeeting {
                    id,
                    model: None,
                    room_speakers: None,
                    call_speakers: None,
                },
            ),
            None => {
                eprintln!("{USAGE}");
                ExitCode::from(2)
            }
        },
        "history" => match detail_txid {
            Some(txid) => round_trip(&mut client, "history", &Command::GetHistoryEntry { txid }),
            None => round_trip(&mut client, "history", &Command::GetHistory),
        },
        _ => {
            let start = Command::StartSession {
                // The daemon selects its configuration-appropriate default.
                // The CLI only forwards an explicit choice.
                pipeline_id: pipeline,
                activation: "manual".to_owned(),
                recognition,
            };
            let code = round_trip(&mut client, "start", &start);
            if code == ExitCode::SUCCESS {
                print_one_session_event(&mut client);
            }
            code
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse_args(&args) {
        Err(msg) => {
            eprintln!(
                "{msg}
{USAGE}"
            );
            ExitCode::from(2)
        }
        Ok(parsed)
            if matches!(
                parsed.command.as_str(),
                "status"
                    | "start"
                    | "stop"
                    | "toggle"
                    | "cancel"
                    | "history"
                    | "models"
                    | "pipelines"
                    | "use-model"
                    | "pipeline-engine"
                    | "download-model"
                    | "cancel-download"
                    | "store"
                    | "store-remove"
                    | "meetings"
                    | "meeting"
                    | "meeting-start"
                    | "meeting-stop"
                    | "meeting-process"
                    | "meeting-notes"
                    | "echo-cancel"
                    | "voice"
                    | "noise"
                    | "words"
                    | "mic"
                    | "hold"
                    | "history-keep"
                    | "numbers"
                    | "meeting-import"
                    | "delivery"
                    | "auto-notes"
                    | SUPPORT_REPORT_COMMAND
            ) =>
        {
            let recognition = match recognition_settings(parsed.recognition) {
                Ok(settings) => settings,
                Err(message) => {
                    eprintln!("narrivy-cli: invalid recognition settings: {message}");
                    return ExitCode::from(2);
                }
            };
            run_command(
                &parsed.command,
                &parsed.socket,
                parsed.pipeline,
                parsed.detail_txid,
                recognition,
                parsed.output,
            )
        }
        Ok(parsed) => {
            eprintln!(
                "unknown command {:?}
{USAGE}",
                parsed.command
            );
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use narrivy_ipc::{Envelope, ToggleOutcomeDto};
    use narrivy_ipc_unix::{PeerCredentials, UnixIpcServer};
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_string()).collect()
    }

    #[test]
    fn parses_command_and_socket() {
        let parsed = parse_args(&s(&["status", "--socket", "/tmp/x.sock"])).unwrap();
        assert_eq!(parsed.command, "status");
        assert_eq!(parsed.socket.as_deref(), Some("/tmp/x.sock"));
        assert!(parsed.pipeline.is_none());
        assert!(parsed.detail_txid.is_none());
        assert_eq!(parsed.recognition, RecognitionArgs::default());
    }

    #[test]
    fn rejects_unknown_and_missing() {
        assert!(parse_args(&s(&["explode"])).is_ok()); // validated by caller
        assert!(parse_args(&s(&["--bogus"])).is_err());
        assert!(parse_args(&[]).is_err());
        assert!(parse_args(&s(&["--socket"])).is_err());
    }

    #[test]
    fn parses_explicit_history_detail_and_pipeline() {
        let parsed = parse_args(&s(&["history", "tx-9", "--pipeline", "linux-mic-inbox"])).unwrap();
        assert_eq!(parsed.command, "history");
        assert_eq!(parsed.detail_txid.as_deref(), Some("tx-9"));
        assert_eq!(parsed.pipeline.as_deref(), Some("linux-mic-inbox"));
    }

    #[test]
    fn recognition_flags_stay_distinct_and_bounded() {
        let parsed = parse_args(&s(&[
            "start",
            "--language",
            "en-US",
            "--output-language",
            "ur",
            "--script",
            "Arab",
            "--translate-to",
            "en",
            "--locale",
            "ur-PK",
            "--context-hint",
            "Narrivy",
            "--context-hint",
            "dictation",
        ]))
        .unwrap();
        let settings = recognition_settings(parsed.recognition).unwrap().unwrap();
        assert!(matches!(
            settings.spoken_language,
            SpokenLanguageDto::Explicit { ref tag } if tag.as_str() == "en-US"
        ));
        assert_eq!(settings.output_language.unwrap().as_str(), "ur");
        assert_eq!(settings.output_script.unwrap().as_str(), "Arab");
        assert_eq!(settings.translation_target.unwrap().as_str(), "en");
        assert_eq!(settings.locale.unwrap().as_str(), "ur-PK");
        assert_eq!(
            settings.context_hints.unwrap().as_slice(),
            ["Narrivy", "dictation"]
        );
        assert!(parse_args(&s(&["start", "--language", "en", "--language", "ur"])).is_err());
        assert!(
            recognition_settings(RecognitionArgs {
                context_hints: vec!["x".repeat(129)],
                ..RecognitionArgs::default()
            })
            .is_err()
        );
    }

    #[test]
    fn toggle_pipeline_flag_accepts_only_canonical_modes() {
        assert_eq!(toggle_pipeline_id(None).unwrap(), None);
        assert_eq!(toggle_pipeline_id(Some("raw")).unwrap(), Some("raw".into()));
        assert_eq!(
            toggle_pipeline_id(Some("clean")).unwrap(),
            Some("clean".into())
        );
        for invalid in [
            "Raw",
            "CLEAN",
            "mock-dictation",
            "linux-mic-inbox",
            "linux-mic-routed-inbox",
            "default",
            "",
            "raw ",
        ] {
            assert!(
                toggle_pipeline_id(Some(invalid)).is_err(),
                "{invalid:?} must not be a selectable toggle mode"
            );
        }
    }

    #[test]
    fn toggle_output_is_content_free_identity_only() {
        let result = ToggleResultDto {
            outcome: ToggleOutcomeDto::FinalizationRequested,
            activation: ActivationSourceDto::InstalledCliCommand,
            session_id: Some(1u128 << 32 | 4),
            transaction_id: Some("tx-toggle".into()),
            pipeline_id: Some("clean".into()),
        };
        let rendered = format_toggle_outcome(&result);
        assert!(rendered.contains("outcome=finalization_requested"));
        assert!(rendered.contains("session_id=4294967300"));
        assert!(rendered.contains("tx=tx-toggle"));
        assert!(rendered.contains("pipeline=clean"));
        // The installed command cannot render transcript or target content:
        // the typed payload carries none.
        for leak in ["transcript", "delivered", "target", "clipboard"] {
            assert!(
                !rendered.contains(leak),
                "{leak} must never appear in installed toggle output"
            );
        }
        assert_eq!(USER_MANAGED_SHORTCUT_COMMAND, "/usr/bin/narrivy-cli toggle");
    }

    /// Recorded wire requests from one test server.
    #[derive(Default)]
    struct WireLog {
        commands: Mutex<Vec<Command>>,
    }

    /// Serve one recording response for exactly one command; assert the CLI
    /// sends one `ToggleSession` and nothing else.
    struct CliHarness {
        _dir: tempfile::TempDir,
        socket: String,
        server: Arc<UnixIpcServer>,
        log: Arc<WireLog>,
        serve: Option<std::thread::JoinHandle<()>>,
    }

    impl CliHarness {
        fn start() -> CliHarness {
            let dir = tempfile::tempdir().unwrap();
            let socket_path = dir.path().join("narrivy-cli-toggle.sock");
            let server = Arc::new(UnixIpcServer::bind(&socket_path).unwrap());
            let log = Arc::new(WireLog::default());
            let handler_log = Arc::clone(&log);
            let serve_server = Arc::clone(&server);
            let serve = std::thread::spawn(move || {
                let _ =
                    serve_server.serve(move |_credentials: PeerCredentials, envelope: Envelope| {
                        let request_id = envelope.request_id;
                        let command: Command = envelope.parse_payload().ok()?;
                        handler_log
                            .commands
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .push(command.clone());
                        let payload = match command {
                            Command::ToggleSession {
                                pipeline_id,
                                activation,
                                ..
                            } => ResponsePayload::Toggle(ToggleResultDto {
                                outcome: ToggleOutcomeDto::Started,
                                activation,
                                session_id: Some(7),
                                transaction_id: Some("tx-cli".into()),
                                pipeline_id: pipeline_id.or_else(|| Some("clean".into())),
                            }),
                            _ => ResponsePayload::Rejected {
                                code: narrivy_ipc::IpcErrorCode::UnknownCommand,
                                detail: "unexpected command in CLI test server".into(),
                            },
                        };
                        Envelope::new(narrivy_ipc::MessageKind::Response, request_id)
                            .with_payload(&payload)
                            .ok()
                    });
            });
            CliHarness {
                socket: socket_path.to_string_lossy().into_owned(),
                server,
                log,
                serve: Some(serve),
                _dir: dir,
            }
        }

        fn commands(&self) -> Vec<Command> {
            self.log
                .commands
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        }

        fn stop(&mut self) {
            self.server.request_shutdown();
            if let Some(serve) = self.serve.take() {
                let _ = serve.join();
            }
        }
    }

    #[test]
    fn toggle_sends_exactly_one_toggle_session_request() {
        let mut harness = CliHarness::start();
        let code = run_command(
            "toggle",
            &Some(harness.socket.clone()),
            Some("clean".to_owned()),
            None,
            None,
            None,
        );
        assert_eq!(code, ExitCode::SUCCESS);
        let commands = harness.commands();
        assert_eq!(commands.len(), 1, "exactly one wire request: {commands:?}");
        assert_eq!(
            commands[0],
            Command::ToggleSession {
                pipeline_id: Some("clean".to_owned()),
                activation: ActivationSourceDto::InstalledCliCommand,
                recognition: None,
            }
        );
        harness.stop();
    }

    #[test]
    fn toggle_default_pipeline_stays_daemon_owned() {
        let mut harness = CliHarness::start();
        let code = run_command(
            "toggle",
            &Some(harness.socket.clone()),
            None,
            None,
            None,
            None,
        );
        assert_eq!(code, ExitCode::SUCCESS);
        let commands = harness.commands();
        assert_eq!(commands.len(), 1);
        assert_eq!(
            commands[0],
            Command::ToggleSession {
                pipeline_id: None,
                activation: ActivationSourceDto::InstalledCliCommand,
                recognition: None,
            }
        );
        harness.stop();
    }

    #[test]
    fn toggle_sends_explicit_recognition_settings_in_the_atomic_request() {
        let mut harness = CliHarness::start();
        let recognition = recognition_settings(RecognitionArgs {
            language: Some("en".to_owned()),
            context_hints: vec!["Narrivy".to_owned()],
            ..RecognitionArgs::default()
        })
        .unwrap();
        let code = run_command(
            "toggle",
            &Some(harness.socket.clone()),
            Some("raw".to_owned()),
            None,
            recognition.clone(),
            None,
        );
        assert_eq!(code, ExitCode::SUCCESS);
        assert_eq!(
            harness.commands(),
            [Command::ToggleSession {
                pipeline_id: Some("raw".to_owned()),
                activation: ActivationSourceDto::InstalledCliCommand,
                recognition,
            }]
        );
        harness.stop();
    }

    #[test]
    fn unsupported_toggle_mode_never_reaches_the_wire() {
        let mut harness = CliHarness::start();
        let code = run_command(
            "toggle",
            &Some(harness.socket.clone()),
            Some("linux-mic-routed-inbox".to_owned()),
            None,
            None,
            None,
        );
        assert_eq!(code, ExitCode::from(2));
        assert!(
            harness.commands().is_empty(),
            "an unsupported mode must not open a request"
        );
        harness.stop();
    }

    #[test]
    fn installed_command_path_is_the_documented_shortcut_command() {
        // The user-managed shortcut must resolve to the installed binary and
        // the atomic subcommand; the CLI never configures the shortcut.
        assert_eq!(USER_MANAGED_SHORTCUT_COMMAND, "/usr/bin/narrivy-cli toggle");
        assert!(Path::new("/usr/bin").is_absolute());
        assert!(USAGE.contains("toggle"));
        assert!(USAGE.contains("raw|clean"));
    }

    /// Content canary that must never appear in support-report output.
    const REPORT_CANARY: &str = "the invoice goes to accounting on thursday";

    /// A daemon that answers `GetStatus` with a scripted capability set and
    /// records every request it receives.
    struct StatusHarness {
        _dir: tempfile::TempDir,
        socket: String,
        server: Arc<UnixIpcServer>,
        log: Arc<WireLog>,
        serve: Option<std::thread::JoinHandle<()>>,
    }

    impl StatusHarness {
        fn start(capabilities: Vec<String>) -> StatusHarness {
            let dir = tempfile::tempdir().unwrap();
            let socket_path = dir.path().join("narrivy-cli-support-report.sock");
            let server = Arc::new(UnixIpcServer::bind(&socket_path).unwrap());
            let log = Arc::new(WireLog::default());
            let handler_log = Arc::clone(&log);
            let serve_server = Arc::clone(&server);
            let capabilities = Arc::new(capabilities);
            let serve = std::thread::spawn(move || {
                let _ =
                    serve_server.serve(move |_credentials: PeerCredentials, envelope: Envelope| {
                        let request_id = envelope.request_id;
                        let command: Command = envelope.parse_payload().ok()?;
                        handler_log
                            .commands
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .push(command.clone());
                        let payload = match command {
                            Command::GetStatus => ResponsePayload::Status(narrivy_ipc::StatusDto {
                                daemon_build: "0.0.0".to_owned(),
                                protocol_minor: narrivy_ipc::PROTOCOL_MINOR,
                                active_session: None,
                                last_session_timings_us: None,
                                capabilities: capabilities.as_ref().clone(),
                                installed_model: None,
                                portal_keyboard: None,
                                model_provisioning: None,
                            }),
                            _ => ResponsePayload::Rejected {
                                code: narrivy_ipc::IpcErrorCode::UnknownCommand,
                                detail: "unexpected command in support-report test server".into(),
                            },
                        };
                        Envelope::new(narrivy_ipc::MessageKind::Response, request_id)
                            .with_payload(&payload)
                            .ok()
                    });
            });
            StatusHarness {
                socket: socket_path.to_string_lossy().into_owned(),
                server,
                log,
                serve: Some(serve),
                _dir: dir,
            }
        }

        fn commands(&self) -> Vec<Command> {
            self.log
                .commands
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        }

        fn stop(&mut self) {
            self.server.request_shutdown();
            if let Some(serve) = self.serve.take() {
                let _ = serve.join();
            }
        }
    }

    fn composed_capabilities() -> Vec<String> {
        s(&[
            "mock.asr",
            "null.delivery",
            "history.optional=enabled",
            "diagnostics.support-report=available",
            "diagnostics.health.overall=ok",
            "diagnostics.latency.sessions=2",
            "diagnostics.latency.p50_us=90000",
            "diagnostics.latency.p95_us=310000",
        ])
    }

    /// The shipped subcommand produces a report over a real socket, and it asks
    /// the daemon for nothing but status: never a transcript, a journal record
    /// or a retained History row.
    #[test]
    fn support_report_asks_only_for_status_and_writes_a_report() {
        let mut harness = StatusHarness::start(composed_capabilities());
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("support-report.json");
        let code = run_command(
            SUPPORT_REPORT_COMMAND,
            &Some(harness.socket.clone()),
            None,
            None,
            None,
            Some(output.to_string_lossy().into_owned()),
        );
        assert_eq!(code, ExitCode::SUCCESS);
        assert_eq!(
            harness.commands(),
            [Command::GetStatus],
            "the support report must never request session content"
        );
        let written = std::fs::read_to_string(&output).unwrap();
        assert!(written.contains("\"schema_version\": 1"), "{written}");
        assert!(written.contains("\"p95_micros\": 310000"), "{written}");
        assert!(!written.contains(REPORT_CANARY), "{written}");
        let report = narrivy_diagnostics::SupportReport::from_json_str(&written).unwrap();
        assert_eq!(report.latency.count, 2);
        assert!(!report.health.is_empty());
        harness.stop();
    }

    /// The subcommand fails closed when the daemon does not compose the
    /// support-report surface, and writes nothing at all.
    #[test]
    fn support_report_fails_closed_without_the_advertised_capability() {
        let mut harness = StatusHarness::start(s(&[
            "mock.asr",
            "null.delivery",
            "diagnostics.support-report=unavailable:not_composed",
        ]));
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("support-report.json");
        let code = run_command(
            SUPPORT_REPORT_COMMAND,
            &Some(harness.socket.clone()),
            None,
            None,
            None,
            Some(output.to_string_lossy().into_owned()),
        );
        assert_eq!(code, ExitCode::from(1));
        assert!(!output.exists(), "no partial report may be written");
        assert_eq!(harness.commands(), [Command::GetStatus]);
        harness.stop();
    }

    /// `--output` belongs to the support report only, and the report accepts no
    /// session controls at all.
    #[test]
    fn support_report_rejects_session_controls_and_output_is_its_own() {
        let mut harness = StatusHarness::start(composed_capabilities());
        assert_eq!(
            run_command(
                SUPPORT_REPORT_COMMAND,
                &Some(harness.socket.clone()),
                Some("clean".to_owned()),
                None,
                None,
                None,
            ),
            ExitCode::from(2)
        );
        assert_eq!(
            run_command(
                "status",
                &Some(harness.socket.clone()),
                None,
                None,
                None,
                Some("/tmp/never-written.json".to_owned()),
            ),
            ExitCode::from(2)
        );
        assert!(
            harness.commands().is_empty(),
            "a rejected argument must not open a request"
        );
        harness.stop();
    }

    #[test]
    fn parses_the_support_report_command_and_output_flag() {
        let parsed = parse_args(&s(&["support-report", "--socket", "/tmp/x.sock"])).unwrap();
        assert_eq!(parsed.command, SUPPORT_REPORT_COMMAND);
        assert_eq!(parsed.socket.as_deref(), Some("/tmp/x.sock"));
        assert!(parsed.output.is_none());
        let parsed = parse_args(&s(&["support-report", "--output", "/tmp/r.json"])).unwrap();
        assert_eq!(parsed.output.as_deref(), Some("/tmp/r.json"));
        assert!(parse_args(&s(&["support-report", "--output"])).is_err());
    }
}
