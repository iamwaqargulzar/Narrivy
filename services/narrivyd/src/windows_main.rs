// Windows daemon composition (PORT-007, D36). Included by `main.rs` on
// Windows; the Linux composition is `linux_main.rs`, and this file follows
// its order and its shutdown contract (ADR-0069) with the Windows adapters
// chosen in `daemon::host`:
//
// - IPC: the per-user named pipe `\\.\pipe\narrivy-<SID>-narrivyd`, owner-only
//   DACL, remote clients rejected, every peer's token SID checked. Binding it
//   is also the single-instance guard (`FILE_FLAG_FIRST_PIPE_INSTANCE`), so it
//   happens before the journal is opened: a second daemon never touches the
//   journal or the Inbox.
// - State: `%LOCALAPPDATA%\Narrivy\state` (journal, history, preferences) and
//   `%LOCALAPPDATA%\Narrivy\run` (seeded pipelines, recovery Inbox).
// - Lifecycle: the daemon runs in the user's interactive session (a Windows
//   service would live in session 0, where capture, `SendInput`, the
//   clipboard and global shortcuts cannot reach the desktop). The CLI starts
//   it on demand (`narrivy-cli start|toggle`), detached and windowless, as the
//   same user. Ctrl+C, Ctrl+Break and closing the console stop it cleanly
//   when it has a console; a detached daemon ends with the logon session, and
//   the recovery journal covers that abrupt end as it covers a crash.
//
// Linux-only features (portals, PipeWire and wtype helpers, echo
// cancellation, desktop shortcut files, the evdev hold key, the Secret
// Service, OpenVINO) are composed through stand-ins that report them as
// unavailable capabilities. The live microphone lanes need a speech engine
// (`asr-transcribe-cpp`), which this composition does not build yet.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use daemon::host::IpcServer;
use narrivy_core::{WorkerStop, WorkerSupervisor, recover_unsent_to_inbox};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShutdownAttempt {
    Succeeded,
    TimedOut,
    Failed,
}

/// Bound for one wait on the console-event streams (no `tokio::select!`: the
/// workspace tokio has no `macros` feature).
const CONSOLE_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Cargo features compiled into this binary, for `--build-features`.
const BUILD_FEATURES: &[&str] = &[
    #[cfg(feature = "asr-transcribe-cpp")]
    "asr-transcribe-cpp",
    #[cfg(feature = "asr-transcribe-resident")]
    "asr-transcribe-resident",
];

/// Ends the serve loop whenever the console bridge ends, for any reason, so
/// the daemon never keeps serving without a way to be stopped.
struct ConsoleBridgeExit<'a>(&'a IpcServer);

impl Drop for ConsoleBridgeExit<'_> {
    fn drop(&mut self) {
        self.0.request_shutdown();
    }
}

/// Owns the Ctrl+C / Ctrl+Break / close bridge on one dedicated thread.
struct ConsoleStopListener {
    cancel: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<Result<(), String>>>,
}

impl ConsoleStopListener {
    fn spawn(server: Arc<IpcServer>) -> Result<Self, String> {
        let cancel = Arc::new(AtomicBool::new(false));
        let thread_cancel = Arc::clone(&cancel);
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let join = std::thread::Builder::new()
            .name("narrivy-console".to_owned())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let error = format!("console runtime creation failed: {error}");
                        let _ = ready_tx.send(Err(error.clone()));
                        return Err(error);
                    }
                };
                // Registration needs an entered runtime and completes before
                // the ready signal, so no event can be missed.
                let streams = {
                    let _runtime_guard = runtime.enter();
                    (
                        tokio::signal::windows::ctrl_c(),
                        tokio::signal::windows::ctrl_break(),
                        tokio::signal::windows::ctrl_close(),
                    )
                };
                let (mut ctrl_c, mut ctrl_break, mut ctrl_close) = match streams {
                    (Ok(c), Ok(b), Ok(close)) => (c, b, close),
                    (Err(error), _, _) | (_, Err(error), _) | (_, _, Err(error)) => {
                        let error = format!("console event registration failed: {error}");
                        let _ = ready_tx.send(Err(error.clone()));
                        return Err(error);
                    }
                };
                ready_tx
                    .send(Ok(()))
                    .map_err(|_| "console listener startup owner disappeared".to_owned())?;
                let _bridge_exit = ConsoleBridgeExit(server.as_ref());
                let wait = |event: Result<Option<()>, tokio::time::error::Elapsed>| match event {
                    Ok(Some(())) => Some(Ok(())),
                    Ok(None) => Some(Err("console event stream closed unexpectedly".to_owned())),
                    Err(_elapsed) => None,
                };
                while !thread_cancel.load(Ordering::Acquire) {
                    let step = CONSOLE_POLL_INTERVAL / 3;
                    // Each timer is created inside `block_on`, where the
                    // runtime's time driver is current.
                    let events = [
                        runtime.block_on(async { tokio::time::timeout(step, ctrl_c.recv()).await }),
                        runtime.block_on(async {
                            tokio::time::timeout(step, ctrl_break.recv()).await
                        }),
                        runtime.block_on(async {
                            tokio::time::timeout(step, ctrl_close.recv()).await
                        }),
                    ];
                    for event in events {
                        if let Some(result) = wait(event) {
                            return result;
                        }
                    }
                }
                Ok(())
            })
            .map_err(|error| format!("console listener thread creation failed: {error}"))?;

        match ready_rx.recv_timeout(Duration::from_secs(2)) {
            Ok(Ok(())) => Ok(Self {
                cancel,
                join: Some(join),
            }),
            Ok(Err(error)) => {
                let _ = join.join();
                Err(error)
            }
            Err(error) => {
                cancel.store(true, Ordering::Release);
                let _ = join.join();
                Err(format!("console listener startup timed out: {error}"))
            }
        }
    }

    fn finish_until(&mut self, deadline: Instant) -> Result<(), String> {
        self.cancel.store(true, Ordering::Release);
        loop {
            if self
                .join
                .as_ref()
                .is_some_and(std::thread::JoinHandle::is_finished)
            {
                let Some(join) = self.join.take() else {
                    return Ok(());
                };
                return join
                    .join()
                    .map_err(|_| "console listener terminated unexpectedly".to_owned())?;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err("console listener did not stop within the shutdown budget".to_owned());
            }
            std::thread::sleep(remaining.min(CONSOLE_POLL_INTERVAL));
        }
    }
}

impl Drop for ConsoleStopListener {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        if self
            .join
            .as_ref()
            .is_some_and(std::thread::JoinHandle::is_finished)
            && let Some(join) = self.join.take()
        {
            let _ = join.join();
        }
    }
}

fn shutdown_attempt_failed(initial: ShutdownAttempt, retry: Option<ShutdownAttempt>) -> bool {
    match (initial, retry) {
        (_, Some(ShutdownAttempt::Failed | ShutdownAttempt::TimedOut)) => true,
        (ShutdownAttempt::Succeeded, None | Some(ShutdownAttempt::Succeeded)) => false,
        (ShutdownAttempt::Failed, _) => true,
        (ShutdownAttempt::TimedOut, Some(ShutdownAttempt::Succeeded)) => false,
        (ShutdownAttempt::TimedOut, None) => true,
    }
}

fn recover_startup_results(
    journal: &narrivy_core::Journal,
    inbox_dir: std::path::PathBuf,
) -> Result<narrivy_core::StartupRecoveryReport, narrivy_core::CoreError> {
    let mut inbox = narrivy_platform_windows::delivery::InboxOnlyDelivery::new(inbox_dir);
    recover_unsent_to_inbox(journal, &mut inbox)
}

fn main() -> std::process::ExitCode {
    use daemon::session::{
        DaemonState, DeliverySink, EXIT_ALREADY_RUNNING, EXIT_FAILURE, build_handler_with_portal,
        exit_code, spawn_event_pump, stop_event_pump,
    };
    use daemon::shutdown::join_thread_bounded;

    // No Windows Error Reporting crash dialog or report for this process
    // (D37, F-H18), before anything is read. A machine-wide WER LocalDumps
    // policy stays outside its control (documented residual).
    if let Err(err) = daemon::host::process::forbid_core_dumps() {
        eprintln!("narrivyd: cannot turn off crash reports: {err}");
        return exit_code(EXIT_FAILURE);
    }
    if std::env::args().nth(1).as_deref() == Some("--build-features") {
        println!("{}", BUILD_FEATURES.join(" "));
        return std::process::ExitCode::SUCCESS;
    }
    let socket_path = match std::env::var_os("NARRIVY_SOCKET") {
        Some(v) if !v.is_empty() => std::path::PathBuf::from(v),
        _ => daemon::host::default_socket_path(),
    };

    // The pipe is the instance lock: bind before any durable state is read.
    let shutdown_budget =
        daemon::shutdown::ShutdownBudget::new(daemon::shutdown::DAEMON_SHUTDOWN_TOTAL);
    let server = match IpcServer::bind_with_shutdown_budget(&socket_path, shutdown_budget.clone()) {
        Ok(server) => Arc::new(server),
        Err(err) => {
            if err.code == narrivy_ipc::IpcErrorCode::DaemonUnavailable {
                eprintln!(
                    "narrivyd: another instance serves {}; refusing to start",
                    socket_path.display()
                );
                return exit_code(EXIT_ALREADY_RUNNING);
            }
            eprintln!("narrivyd: bind {} failed: {err}", socket_path.display());
            return exit_code(EXIT_FAILURE);
        }
    };

    let runtime_dir = match daemon::host::runtime_dir() {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("narrivyd: runtime dir unavailable: {err}");
            return exit_code(EXIT_FAILURE);
        }
    };
    let state_dir = match daemon::host::state_dir() {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("narrivyd: durable state unavailable: {err}");
            return exit_code(EXIT_FAILURE);
        }
    };
    let journal = match daemon::policy::open_startup_journal(&state_dir) {
        Ok(journal) => Arc::new(journal),
        Err(err) => {
            eprintln!("narrivyd: {err}");
            return exit_code(EXIT_FAILURE);
        }
    };
    // Optional retained History (PRIV-002), best-effort as on Linux.
    let history =
        match daemon::retained_history::retention_preference(&state_dir).and_then(|retention| {
            daemon::retained_history::open_retained_history(&state_dir, retention)
        }) {
            Ok(service) => Some(Arc::new(service)),
            Err(reason) => {
                eprintln!("narrivyd: optional history unavailable: {reason}");
                None
            }
        };

    let mut console_listener = match ConsoleStopListener::spawn(Arc::clone(&server)) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("narrivyd: {error}");
            return exit_code(EXIT_FAILURE);
        }
    };

    let mut engine_config = match daemon::policy::EngineConfig::from_env() {
        Ok(cfg) => cfg,
        Err(reason) => {
            eprintln!("narrivyd: {reason}");
            return exit_code(EXIT_FAILURE);
        }
    };
    // A resident configuration in a build without the engine would serve
    // without the engine it names; refuse instead.
    #[cfg(not(feature = "asr-transcribe-resident"))]
    if engine_config.transcribe_resident {
        eprintln!("narrivyd: this build has no resident ASR engine");
        return exit_code(EXIT_FAILURE);
    }
    if let Err(err) =
        daemon::policy::seed_builtin_pipelines(&daemon::policy::pipelines_dir(&runtime_dir))
    {
        eprintln!("narrivyd: builtin pipeline seeding failed: {err}");
        return exit_code(EXIT_FAILURE);
    }
    let inbox_dir = runtime_dir.join("inbox");
    if let Err(err) = std::fs::create_dir_all(&inbox_dir) {
        eprintln!("narrivyd: inbox dir unavailable: {err}");
        return exit_code(EXIT_FAILURE);
    }
    match recover_startup_results(&*journal, inbox_dir.clone()) {
        Ok(report) if report.recovered > 0 || report.delivery_failed > 0 => eprintln!(
            "narrivyd: startup recovery completed recovered={} incomplete={} failed={}",
            report.recovered, report.incomplete, report.delivery_failed
        ),
        Ok(_) => {}
        Err(error) => {
            eprintln!("narrivyd: startup recovery failed: {error}");
            return exit_code(EXIT_FAILURE);
        }
    }
    daemon::session::prune_inbox(inbox_dir.clone());
    engine_config.inbox_dir = Some(inbox_dir);

    let state = Arc::new(DaemonState::new());
    state.configure_activation_facts(daemon::policy::ActivationFacts {
        atomic_command: true,
        portal_interface: Some(daemon::host::linux_features::trigger::probe_portal_interface()),
        portal_listener: false,
    });
    state.configure_shortcuts(Arc::new(daemon::shortcuts::ShortcutService::compose(
        &state_dir,
    )));
    let supervisor = Arc::new(WorkerSupervisor::new());
    // The resident engine. Installed (MSI, per machine): the worker bound to
    // the installed manifest and the bundled model to this build's registry,
    // both under operating-system ownership in the install folder; anything
    // else is refused, never replaced by an unverified path. Developer runs:
    // an explicit, digest-pinned worker and model
    // (`NARRIVY_TRANSCRIBE_WORKER{,_SHA256,_SIZE}`,
    // `NARRIVY_TRANSCRIBE_MODEL{,_SHA256,_SIZE}`).
    #[cfg(feature = "asr-transcribe-resident")]
    if engine_config.transcribe_resident && engine_config.transcribe_installed {
        use daemon::host::linux_features::installed_runtime;
        let Some(model_id) = engine_config.transcribe_model_id.as_deref() else {
            eprintln!("narrivyd: installed resident ASR requires a model ID");
            return exit_code(EXIT_FAILURE);
        };
        let Ok(identity) = narrivy_model_manager::BundledModelRegistry::load().resolve_import(model_id)
        else {
            eprintln!("narrivyd: selected installed model ID is not registered");
            return exit_code(EXIT_FAILURE);
        };
        let runtime = match installed_runtime::resolve_installed_runtime() {
            Ok(binding) => binding,
            Err(error) => {
                eprintln!("narrivyd: installed resident runtime failed verification ({error})");
                return exit_code(EXIT_FAILURE);
            }
        };
        // The MSI ships no model (D40, as the Linux package): with none
        // installed the service waits for a download, with its verified
        // worker kept for it. A model that is present but fails
        // verification is still refused.
        match installed_runtime::bind_bundled_model(&identity) {
            Ok(model) => {
                if let Err(error) = state.initialize_installed_resident_asr(runtime, model) {
                    eprintln!("narrivyd: installed resident ASR startup failed ({})", error.code);
                    return exit_code(EXIT_FAILURE);
                }
                // Models the user downloads live beside the state directory,
                // in the user's own folder; the installed one stays where it
                // was installed.
                engine_config.installed_model =
                    installed_runtime::bundled_model_path(&identity).map(|path| {
                        daemon::policy::InstalledModelFile {
                            path,
                            sha256: identity.sha256().to_owned(),
                            size: identity.size_bytes(),
                        }
                    });
            }
            Err(daemon::host::process::DataArtifactError::Missing) => {
                if let Err(error) = state.initialize_awaiting_installed_model(runtime) {
                    eprintln!(
                        "narrivyd: installed resident degraded startup failed ({})",
                        error.code
                    );
                    return exit_code(EXIT_FAILURE);
                }
                eprintln!("narrivyd: serving with installed resident ASR unavailable (model_missing)");
            }
            Err(_) => {
                eprintln!("narrivyd: installed model failed verification");
                return exit_code(EXIT_FAILURE);
            }
        }
        state.model_selection().use_installed_worker();
        engine_config.models_dir = state_dir
            .parent()
            .map(|data| data.join("models"))
            .filter(|models| std::fs::create_dir_all(models).is_ok());
        state.model_selection().persist_under(&state_dir);
        if let Some(chosen) = daemon::model_selection::restore(&state_dir, &engine_config) {
            let compute = state.model_selection().current_compute();
            match state.replace_resident_model(&chosen, compute, &narrivy_core::CancelFlag::new()) {
                Ok(()) => state.model_selection().set_current(chosen),
                Err(code) => eprintln!("narrivyd: the remembered model did not load ({code})"),
            }
        }
    }
    #[cfg(feature = "asr-transcribe-resident")]
    if engine_config.transcribe_resident && !engine_config.transcribe_installed {
        let (Some(model), Some(model_sha256), Some(model_size)) = (
            engine_config.transcribe_model_path.as_deref(),
            engine_config.transcribe_model_sha256.as_deref(),
            engine_config.transcribe_model_size,
        ) else {
            eprintln!("narrivyd: resident ASR requires an explicit model, SHA-256 and size");
            return exit_code(EXIT_FAILURE);
        };
        state.model_selection().persist_under(&state_dir);
        if let Some(compute) = std::env::var("NARRIVY_TRANSCRIBE_COMPUTE")
            .ok()
            .and_then(|value| daemon::model_selection::ComputeChoice::parse(&value))
        {
            state.model_selection().set_current_compute(compute);
        }
        let remembered = daemon::model_selection::restore(&state_dir, &engine_config)
            .filter(|chosen| chosen.engine == daemon::model_selection::ModelEngine::TranscribeCpp)
            .filter(|chosen| {
                state
                    .initialize_resident_asr(&chosen.path, &chosen.sha256, chosen.size)
                    .map_err(|error| {
                        eprintln!(
                            "narrivyd: the remembered model did not load ({}); using the configured model",
                            error.code
                        );
                    })
                    .is_ok()
            });
        match remembered {
            Some(chosen) => state.model_selection().set_current(chosen),
            None => {
                if let Err(error) = state.initialize_resident_asr(model, model_sha256, model_size) {
                    eprintln!("narrivyd: resident ASR startup failed ({})", error.code);
                    return exit_code(EXIT_FAILURE);
                }
            }
        }
    }
    let sink: DeliverySink = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut pump = Some(spawn_event_pump(server.clone(), state.clone()));
    // The echo canceller (an in-process setup on Windows) is ready before
    // the first dictation, as on Linux.
    daemon::echo::warm();
    daemon::voice::warm();
    daemon::hold::start(socket_path.clone());
    // The toggle shortcut: Windows has no desktop store to bind it in.
    daemon::hotkey::start(socket_path.clone());
    #[cfg(feature = "asr-transcribe-resident")]
    daemon::warm_default_pipeline(&state, &supervisor, &engine_config);
    let _ = daemon::refine::shared();

    let worker_start_delay = std::env::var("NARRIVY_WORKER_START_DELAY_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_millis);
    let diagnostics = Arc::new(daemon::diagnostics::DaemonDiagnostics::new());
    // The last good live model list, if one was kept (no network here).
    daemon::catalog_feed::load_cached();
    let compute_plan = Arc::new(daemon::compute::build_plan(&state_dir, None));
    eprintln!("narrivyd: serving on {}", socket_path.display());
    daemon::assistant_library::start_routines(&state);
    let serve_result = server.serve(build_handler_with_portal(
        state.clone(),
        journal,
        history.clone(),
        sink,
        supervisor.clone(),
        worker_start_delay,
        Arc::new(engine_config),
        None,
        Some(Arc::clone(&diagnostics)),
        Some(Arc::clone(&compute_plan)),
    ));
    drop(history);

    // One monotonic stop budget for every owner below (ADR-0069).
    if !shutdown_budget.is_armed() {
        shutdown_budget.arm();
    }
    let shutdown_deadline = shutdown_budget.deadline();
    // Keep the meeting being recorded, untranscribed (D46), as on Linux.
    if !daemon::meetings::save_recording_at_shutdown(shutdown_deadline) {
        eprintln!("narrivyd: the meeting being recorded was not saved before the deadline");
    }
    let console_listener_failed = match console_listener.finish_until(shutdown_deadline) {
        Ok(()) => false,
        Err(error) => {
            eprintln!("narrivyd: {error}");
            true
        }
    };
    // Remove the resident lookup handle first, then cancel every worker,
    // then close the engine (as on Linux): a session waiting on the engine
    // sees cancellation before the engine's bounded cleanup starts.
    #[cfg(feature = "asr-transcribe-resident")]
    let resident_asr = state.take_resident_asr();
    daemon::hotkey::stop();
    supervisor.cancel_all();
    #[cfg(feature = "asr-transcribe-resident")]
    let resident_attempt = |engine: &narrivy_asr_transcribe::ResidentWorkerEngine| match engine
        .shutdown_until(shutdown_deadline)
    {
        Ok(()) => ShutdownAttempt::Succeeded,
        Err(error) => {
            eprintln!("narrivyd: resident ASR shutdown failed ({})", error.code);
            if error.code == "resident_worker_shutdown_timeout" {
                ShutdownAttempt::TimedOut
            } else {
                ShutdownAttempt::Failed
            }
        }
    };
    #[cfg(feature = "asr-transcribe-resident")]
    let resident_shutdown_initial = resident_asr
        .as_ref()
        .map_or(ShutdownAttempt::Succeeded, resident_attempt);
    #[cfg(not(feature = "asr-transcribe-resident"))]
    let resident_shutdown_initial = ShutdownAttempt::Succeeded;
    let worker_shutdown_initial = match supervisor.shutdown_all_until(shutdown_deadline) {
        Ok(outcomes) => {
            let timed_out = outcomes
                .iter()
                .filter(|(_, outcome)| *outcome == WorkerStop::TimedOut)
                .count();
            if timed_out > 0 {
                eprintln!("narrivyd: {timed_out} worker(s) exceeded the shutdown deadline");
                ShutdownAttempt::TimedOut
            } else {
                ShutdownAttempt::Succeeded
            }
        }
        Err(err) => {
            eprintln!("narrivyd: worker shutdown failed: {err}");
            ShutdownAttempt::Failed
        }
    };
    let worker_shutdown_retry = if worker_shutdown_initial != ShutdownAttempt::Succeeded
        || supervisor.live_worker_count_until(shutdown_deadline) != 0
    {
        Some(match supervisor.shutdown_all_until(shutdown_deadline) {
            Ok(outcomes) => {
                if outcomes
                    .iter()
                    .any(|(_, outcome)| *outcome == WorkerStop::TimedOut)
                    || supervisor.live_worker_count_until(shutdown_deadline) != 0
                {
                    ShutdownAttempt::TimedOut
                } else {
                    ShutdownAttempt::Succeeded
                }
            }
            Err(err) => {
                eprintln!("narrivyd: final worker reap failed: {err}");
                ShutdownAttempt::Failed
            }
        })
    } else {
        None
    };
    #[cfg(feature = "asr-transcribe-resident")]
    let resident_shutdown_retry = if resident_shutdown_initial != ShutdownAttempt::Succeeded {
        resident_asr.as_ref().map(resident_attempt)
    } else {
        None
    };
    #[cfg(not(feature = "asr-transcribe-resident"))]
    let resident_shutdown_retry = None;
    let worker_shutdown_failed =
        shutdown_attempt_failed(worker_shutdown_initial, worker_shutdown_retry)
            || supervisor.live_worker_count_until(shutdown_deadline) != 0;
    let resident_shutdown_failed =
        shutdown_attempt_failed(resident_shutdown_initial, resident_shutdown_retry);
    stop_event_pump(&state);
    let pump_failed = join_thread_bounded(&mut pump, shutdown_deadline).is_err();
    if pump_failed {
        eprintln!("narrivyd: event pump did not stop within the shutdown budget");
    }
    let serve_failed = match serve_result {
        Ok(()) => false,
        Err(err) => {
            eprintln!("narrivyd: serve failed: {err}");
            true
        }
    };
    if worker_shutdown_failed
        || resident_shutdown_failed
        || pump_failed
        || serve_failed
        || console_listener_failed
    {
        return exit_code(EXIT_FAILURE);
    }
    std::process::ExitCode::SUCCESS
}
