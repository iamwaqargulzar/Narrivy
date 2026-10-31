// narrivyd entrypoint — thin wiring over `crate::daemon`.
// See `src/daemon/` for session orchestration, capture helpers and policy.

pub mod daemon;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use narrivy_core::{WorkerStop, WorkerSupervisor, recover_unsent_to_inbox};
use narrivy_ipc::PortalEligibilityDto;
use narrivy_ipc_unix::UnixIpcServer;
use narrivy_platform_linux::portal_eis::{
    PortalEisEligibility, SystemRemoteDesktopProbe, classify_portal_eis,
};
use narrivy_platform_linux::portal_eis_zbus::DirectZbusPortalDriver;

use narrivyd::instance;

#[cfg(feature = "asr-transcribe-resident")]
fn installed_model_is_missing(error: &narrivy_model_manager::ModelManagerError) -> bool {
    matches!(
        error,
        narrivy_model_manager::ModelManagerError::InstalledModelMissing { .. }
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShutdownAttempt {
    Succeeded,
    TimedOut,
    Failed,
}

/// Bound for waiting on one signal stream. The installed tokio feature set
/// (`net`, `rt`, `signal`, `time`) does not enable `macros`, so
/// `tokio::select!` is unavailable for a concurrent wait over both signals and
/// the explicit stop flag.
const SIGNAL_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// One bounded observation of the bridge's signal sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SignalEvent {
    /// SIGTERM was observed.
    Terminate,
    /// SIGINT was observed.
    Interrupt,
    /// Neither signal arrived before the poll bound elapsed.
    Pending,
    /// The named stream ended. The installed tokio `Signal::recv` is
    /// documented never to yield `None`, so this is an unexpected bridge
    /// failure that must stop the daemon rather than be ignored.
    Closed(&'static str),
}

/// Bounded source of signal events. The indirection keeps the bridge loop's
/// failure policy directly testable without delivering real OS signals.
trait SignalSource {
    fn next_event(&mut self, timeout: Duration) -> SignalEvent;
}

/// SIGTERM/SIGINT source backed by tokio's unix signal streams.
struct TokioSignalSource {
    runtime: tokio::runtime::Runtime,
    terminate: tokio::signal::unix::Signal,
    interrupt: tokio::signal::unix::Signal,
}

impl SignalSource for TokioSignalSource {
    fn next_event(&mut self, timeout: Duration) -> SignalEvent {
        let Self {
            runtime,
            terminate,
            interrupt,
        } = self;
        let terminated =
            runtime.block_on(async { tokio::time::timeout(timeout, terminate.recv()).await });
        match terminated {
            Ok(Some(())) => return SignalEvent::Terminate,
            // `Signal::recv` never yields `None` in the installed tokio API;
            // fail closed if that ever changes.
            Ok(None) => return SignalEvent::Closed("SIGTERM"),
            Err(_elapsed) => {}
        }
        let interrupted =
            runtime.block_on(async { tokio::time::timeout(timeout, interrupt.recv()).await });
        match interrupted {
            Ok(Some(())) => SignalEvent::Interrupt,
            Ok(None) => SignalEvent::Closed("SIGINT"),
            Err(_elapsed) => SignalEvent::Pending,
        }
    }
}

/// Owns the daemon's SIGTERM/SIGINT bridge on one dedicated thread.
struct TerminationSignalListener {
    cancel: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<Result<(), String>>>,
}

/// Fail-closed guard for the signal bridge.
///
/// `tokio::signal::unix::Signal::recv` is documented never to yield `None`,
/// and registration errors are fatal before `serve` starts. A listener thread
/// can still end unexpectedly (for example a panic while polling). Whenever
/// the listener ends for any reason, the serve loop must not keep running
/// without a way to observe SIGTERM/SIGINT, so ending the bridge always asks
/// the server to stop. On the ordinary explicit-stop path this is a redundant
/// no-op: `finish`/`Drop` only run after `serve` has already returned.
struct SignalBridgeExit<'a>(&'a UnixIpcServer);

impl Drop for SignalBridgeExit<'_> {
    fn drop(&mut self) {
        self.0.request_shutdown();
    }
}

/// Run the signal bridge until SIGTERM/SIGINT, an explicit stop request, or an
/// unexpected source failure.
///
/// The [`SignalBridgeExit`] guard makes one invariant hold on every exit path:
/// ending the bridge ends the serve loop. Before that guard, a source failure
/// returned `Err` while `serve` kept blocking forever, because
/// [`TerminationSignalListener::finish`] only runs after `serve` returns.
fn run_signal_bridge(
    server: &UnixIpcServer,
    cancel: &AtomicBool,
    source: &mut dyn SignalSource,
) -> Result<(), String> {
    let _bridge_exit = SignalBridgeExit(server);
    while !cancel.load(Ordering::Acquire) {
        match source.next_event(SIGNAL_POLL_INTERVAL) {
            SignalEvent::Terminate | SignalEvent::Interrupt => return Ok(()),
            SignalEvent::Pending => {}
            SignalEvent::Closed(name) => {
                return Err(format!("{name} listener closed unexpectedly"));
            }
        }
    }
    Ok(())
}

impl TerminationSignalListener {
    fn spawn(server: Arc<UnixIpcServer>) -> Result<Self, String> {
        let cancel = Arc::new(AtomicBool::new(false));
        let thread_cancel = Arc::clone(&cancel);
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let join = std::thread::Builder::new()
            .name("narrivy-signal".to_owned())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|error| format!("signal runtime creation failed: {error}"));
                let runtime = match runtime {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = ready_tx.send(Err(error.clone()));
                        return Err(error);
                    }
                };
                let (terminate, interrupt) = {
                    // `tokio::signal::unix::signal` panics without an entered
                    // runtime context; registration completes before the ready
                    // signal so no signal can be missed.
                    let _runtime_guard = runtime.enter();
                    let terminate =
                        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                            .map_err(|error| format!("SIGTERM registration failed: {error}"));
                    let interrupt =
                        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
                            .map_err(|error| format!("SIGINT registration failed: {error}"));
                    (terminate, interrupt)
                };
                let (terminate, interrupt) = match (terminate, interrupt) {
                    (Ok(terminate), Ok(interrupt)) => (terminate, interrupt),
                    (Err(error), _) | (_, Err(error)) => {
                        let _ = ready_tx.send(Err(error.clone()));
                        return Err(error);
                    }
                };
                ready_tx
                    .send(Ok(()))
                    .map_err(|_| "signal listener startup owner disappeared".to_owned())?;

                let mut source = TokioSignalSource {
                    runtime,
                    terminate,
                    interrupt,
                };
                run_signal_bridge(server.as_ref(), &thread_cancel, &mut source)
            })
            .map_err(|error| format!("signal listener thread creation failed: {error}"))?;

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
                // The thread sees `cancel` within one poll once its bridge
                // runs; one stuck before that is not waited for past the
                // bound. Startup fails either way and the process exits.
                let mut join = Some(join);
                let _ = join_thread_bounded(&mut join, Instant::now() + Duration::from_secs(2));
                Err(format!("signal listener startup timed out: {error}"))
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
                    .map_err(|_| "signal listener terminated unexpectedly".to_owned())?;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                // The handle stays owned by this listener; the caller reports
                // the failure instead of detaching the thread and claiming a
                // clean shutdown.
                return Err("signal listener did not stop within the shutdown budget".to_owned());
            }
            std::thread::sleep(remaining.min(SIGNAL_POLL_INTERVAL));
        }
    }
}

impl Drop for TerminationSignalListener {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        // Only a provably finished thread is joined here. Blocking here would
        // re-introduce an unbounded wait on a path where failure was already
        // reported by `finish_until`.
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
        // Any failure from an attempted final cleanup is authoritative even
        // when the first pass happened to report success.
        (_, Some(ShutdownAttempt::Failed | ShutdownAttempt::TimedOut)) => true,
        (ShutdownAttempt::Succeeded, None | Some(ShutdownAttempt::Succeeded)) => false,
        // An ordinary supervisor/resident error is sticky. A later cleanup
        // pass may reduce leaked resources but cannot erase that uncertainty.
        (ShutdownAttempt::Failed, _) => true,
        (ShutdownAttempt::TimedOut, Some(ShutdownAttempt::Succeeded)) => false,
        (ShutdownAttempt::TimedOut, None) => true,
    }
}

fn portal_eligibility() -> PortalEligibilityDto {
    portal_eligibility_from(classify_portal_eis(&SystemRemoteDesktopProbe::default()))
}

const fn portal_eligibility_from(eligibility: PortalEisEligibility) -> PortalEligibilityDto {
    match eligibility {
        PortalEisEligibility::ReadyToRequestPermission { .. } => {
            PortalEligibilityDto::ReadyToRequest
        }
        PortalEisEligibility::ProbeFailed(_) => PortalEligibilityDto::ProbeFailed,
        PortalEisEligibility::InterfaceAbsent
        | PortalEisEligibility::VersionTooOld { .. }
        | PortalEisEligibility::KeyboardUnavailable { .. } => PortalEligibilityDto::Unavailable,
    }
}

#[cfg(feature = "asr-transcribe-resident")]
fn model_cache_root() -> Result<std::path::PathBuf, &'static str> {
    let base = match std::env::var_os("XDG_CACHE_HOME") {
        Some(value) if !value.is_empty() => std::path::PathBuf::from(value),
        _ => std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(std::path::PathBuf::from)
            .map(|home| home.join(".cache"))
            .ok_or("model cache base is unavailable")?,
    };
    if !base.is_absolute() {
        return Err("model cache base must be absolute");
    }
    Ok(base.join("narrivy"))
}

// Re-exports for backward compatibility with `include!("../src/main.rs")` tests
// and for external `daemon_bin::` users. Publish the same public surface that
// the original monolithic `main.rs` exposed at crate root.
#[allow(unused_imports)]
pub use daemon::capture::{AudioSourceKind, SAMPLE_RATE_HZ, parse_wav_mono, sine_fixture_samples};
#[allow(unused_imports)]
pub use daemon::diagnostics::{
    DaemonDiagnostics, SessionOutcome, append_diagnostics_capabilities, content_safe,
    diagnostic_redactor,
};
#[allow(unused_imports)]
pub use daemon::policy::ActivationFacts;
#[allow(unused_imports)]
pub use daemon::policy::{
    AsrEngineKind, CLEAN_PIPELINE_ID, CorrectionStrategy, DEFAULT_PIPELINE_ENV,
    DeliveryBackendKind, EngineConfig, LINUX_MIC_INBOX_PIPELINE_ID, MOCK_TRANSCRIPT,
    RAW_PIPELINE_ID, builtin_pipeline_defs, collapse_only, default_pipeline_id,
    default_pipeline_id_from, open_journal, open_startup_journal, pipelines_dir, probe_pw_record,
    probe_theme_store_writability, run_capability_check, run_capability_check_with,
    seed_builtin_pipelines,
};
#[allow(unused_imports)]
pub use daemon::portal::{PORTAL_EVENT_POLL_INTERVAL, spawn_portal_event_pump};
#[allow(unused_imports)]
pub use daemon::session::{
    DaemonEvent, DaemonState, DeliveryReport, DeliverySink, EXIT_ALREADY_RUNNING, EXIT_FAILURE,
    PORTAL_PERMISSION_REQUEST_TIMEOUT, PORTAL_REVOKE_TIMEOUT, PUMP_WAKE_FALLBACK, SessionRequest,
    SessionSlot, build_handler, build_handler_with_portal, exit_code, spawn_event_pump, state_wire,
    stop_event_pump,
};
#[allow(unused_imports)]
pub use daemon::shutdown::{DAEMON_SHUTDOWN_TOTAL, ShutdownBudget, join_thread_bounded};

/// The durable, private Inbox directory below the state root (F-M3).
fn inbox_dir(state_dir: &std::path::Path) -> std::path::PathBuf {
    state_dir.join("inbox")
}

fn recover_startup_results(
    journal: &narrivy_core::Journal,
    inbox_dir: std::path::PathBuf,
) -> Result<narrivy_core::StartupRecoveryReport, narrivy_core::CoreError> {
    let mut inbox = narrivy_platform_linux::recovery::InboxOnlyDelivery::new(inbox_dir);
    recover_unsent_to_inbox(journal, &mut inbox)
}

/// Removes a bound IPC socket when startup fails before the serve loop.
///
/// Disarmed by [`disarm`](Self::disarm) once the daemon is serving, because a
/// socket left by a clean exit is the stale-endpoint state the next start
/// verifies and takes over. While startup is still in progress there is no
/// such contract, so the endpoint must not survive a refused start.
struct StartupSocketGuard {
    path: std::path::PathBuf,
    armed: bool,
}

impl StartupSocketGuard {
    fn new(path: std::path::PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for StartupSocketGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Cargo features compiled into this binary, for `--build-features`.
const BUILD_FEATURES: &[&str] = &[
    #[cfg(feature = "asr-transcribe-cpp")]
    "asr-transcribe-cpp",
    #[cfg(feature = "asr-transcribe-resident")]
    "asr-transcribe-resident",
];

fn main() -> std::process::ExitCode {
    // No core file and no same-user `/proc` reads of transcripts, audio or
    // the clipboard held in memory (D37, F-H18). Before anything is read.
    if let Err(err) = narrivy_process_linux::forbid_core_dumps() {
        eprintln!("narrivyd: cannot turn off core dumps: {err}");
        return exit_code(EXIT_FAILURE);
    }
    // The package builder checks what this binary was built with (F-M14):
    // a daemon without the resident engine must never be packaged.
    if std::env::args().nth(1).as_deref() == Some("--build-features") {
        println!("{}", BUILD_FEATURES.join(" "));
        return std::process::ExitCode::SUCCESS;
    }
    let socket_path = match std::env::var_os("NARRIVY_SOCKET") {
        Some(v) if !v.is_empty() => std::path::PathBuf::from(v),
        _ => narrivy_ipc_unix::default_socket_path(),
    };

    let _instance_lock = match instance::acquire_instance_lock() {
        Ok(lock) => lock,
        Err(instance::InstanceError::AlreadyRunning { lock_path }) => {
            eprintln!("narrivyd: another instance holds {lock_path:?}; refusing to start");
            return exit_code(EXIT_ALREADY_RUNNING);
        }
        Err(err) => {
            eprintln!("narrivyd: instance check failed: {err}");
            return exit_code(EXIT_FAILURE);
        }
    };

    let runtime_dir = match instance::runtime_dir() {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("narrivyd: runtime dir unavailable: {err}");
            return exit_code(EXIT_FAILURE);
        }
    };
    let state_dir = match instance::state_dir() {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("narrivyd: durable state unavailable: {err}");
            return exit_code(EXIT_FAILURE);
        }
    };
    let journal = match open_startup_journal(&state_dir) {
        Ok(journal) => Arc::new(journal),
        Err(err) => {
            eprintln!("narrivyd: {err}");
            return exit_code(EXIT_FAILURE);
        }
    };

    // Optional retained History (PRIV-002). Composition is best-effort by
    // contract: an unavailable store leaves every other command serving, keeps
    // `history.optional=disabled:not_composed` truthful, and turns the History
    // commands into typed rejections. Refusing to start instead would trade
    // every other capability for an optional store.
    let history =
        match daemon::retained_history::retention_preference(&state_dir).and_then(|retention| {
            daemon::retained_history::open_retained_history(&state_dir, retention)
        }) {
            Ok(service) => {
                // Composition is not logged. The store's presence, effective
                // retention and entry count are already advertised as content-free
                // `history.*` capability tokens, which is how a client discovers
                // them. An unconditional line here would also put a filesystem path
                // on the ordinary diagnostic channel, and would make a fail-closed
                // startup emit anything other than its single fatal error, which
                // the candidate builder's negative probe asserts.
                Some(Arc::new(service))
            }
            Err(reason) => {
                eprintln!("narrivyd: optional history unavailable: {reason}");
                None
            }
        };

    // One total stop budget for the whole daemon, fixed before any signal can
    // be handled. The IPC server arms it on the first shutdown request, so the
    // connection cleanup inside `serve` and every later owner share one
    // monotonic deadline (ADR-0069).
    let shutdown_budget =
        daemon::shutdown::ShutdownBudget::new(daemon::shutdown::DAEMON_SHUTDOWN_TOTAL);
    let server =
        match UnixIpcServer::bind_with_shutdown_budget(&socket_path, shutdown_budget.clone()) {
            Ok(server) => Arc::new(server),
            Err(err) => {
                eprintln!("narrivyd: bind {} failed: {err}", socket_path.display());
                if err.code == narrivy_ipc::IpcErrorCode::DaemonUnavailable {
                    return exit_code(EXIT_ALREADY_RUNNING);
                }
                return exit_code(EXIT_FAILURE);
            }
        };
    let mut signal_listener = match TerminationSignalListener::spawn(Arc::clone(&server)) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("narrivyd: {error}");
            return exit_code(EXIT_FAILURE);
        }
    };
    // Startup binds the endpoint before most configuration is resolved, so any
    // later refusal to start would otherwise leave a socket behind for a daemon
    // that is not running. A client can connect to it, and it reads as a live
    // instance until the next start takes it over. This guard removes the
    // socket on every early exit; it is disarmed once the daemon reaches its
    // serve loop, where a leftover socket is the documented stale-endpoint state
    // that the next bind verifies and takes over by flock.
    let mut startup_socket_guard = StartupSocketGuard::new(socket_path.clone());

    let mut engine_config = match EngineConfig::from_env() {
        Ok(cfg) => cfg,
        Err(reason) => {
            eprintln!("narrivyd: {reason}");
            return exit_code(EXIT_FAILURE);
        }
    };

    if let Err(err) = seed_builtin_pipelines(&pipelines_dir(&runtime_dir)) {
        eprintln!("narrivyd: builtin pipeline seeding failed: {err}");
        return exit_code(EXIT_FAILURE);
    }

    // Recovered text is durable, private user data (F-M3): it lives under the
    // 0700 state root so it survives logout and reboot, never on the runtime
    // tmpfs. Text a previous version left in the runtime-dir Inbox is moved.
    let inbox_dir = inbox_dir(&state_dir);
    let inbox_store = narrivy_platform_linux::delivery::InboxStore::new(inbox_dir.clone());
    if let Err(err) = inbox_store.ensure_private_directory() {
        eprintln!("narrivyd: inbox dir unavailable: {err}");
        return exit_code(EXIT_FAILURE);
    }
    let adopted = inbox_store.adopt_from(&runtime_dir.join("inbox"));
    if adopted > 0 {
        eprintln!("narrivyd: moved {adopted} recovered-text file(s) to the durable Inbox");
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
    // One bounded, direct probe of the exact GlobalShortcuts interface on the
    // canonical portal object. The observed fact is reported as its own
    // capability; no desktop name is consulted and no listener is implied.
    state.configure_activation_facts(ActivationFacts {
        atomic_command: true,
        portal_interface: Some(narrivy_platform_linux::trigger::probe_portal_interface()),
        portal_listener: false,
    });
    // Desktop shortcut management (owner decision D24): detected once from
    // the session this daemon runs in. An unknown desktop is reported as the
    // manual route rather than treated as a failure.
    state.configure_shortcuts(Arc::new(daemon::shortcuts::ShortcutService::compose(
        &state_dir,
    )));
    let supervisor = Arc::new(WorkerSupervisor::new());
    let portal_driver = match DirectZbusPortalDriver::new() {
        Ok(driver) => driver,
        Err(_) => {
            eprintln!("narrivyd: portal driver runtime unavailable");
            return exit_code(EXIT_FAILURE);
        }
    };
    let portal_service = match daemon::portal::PortalPermissionService::spawn(
        &supervisor,
        portal_driver,
        portal_eligibility(),
    ) {
        Ok(service) => Arc::new(service),
        Err(_) => {
            eprintln!("narrivyd: portal permission service failed to start");
            return exit_code(EXIT_FAILURE);
        }
    };
    #[cfg(feature = "asr-transcribe-resident")]
    let mut pending_model_provisioning = None;
    #[cfg(feature = "asr-transcribe-resident")]
    if engine_config.transcribe_resident {
        if engine_config.transcribe_installed {
            let Some(model_id) = engine_config.transcribe_model_id.as_deref() else {
                eprintln!("narrivyd: installed resident ASR requires a model ID");
                return exit_code(EXIT_FAILURE);
            };
            let cache_root = match model_cache_root() {
                Ok(path) => path,
                Err(reason) => {
                    eprintln!("narrivyd: installed model cache unavailable: {reason}");
                    return exit_code(EXIT_FAILURE);
                }
            };
            if narrivy_platform_linux::model_import::prepare_model_cache_root(&cache_root).is_err()
            {
                eprintln!("narrivyd: installed model cache root is unsafe");
                return exit_code(EXIT_FAILURE);
            }
            let registry = Arc::new(narrivy_model_manager::BundledModelRegistry::load());
            if registry.resolve_import(model_id).is_err() {
                eprintln!("narrivyd: selected installed model ID is not registered");
                return exit_code(EXIT_FAILURE);
            }
            let runtime =
                match narrivy_platform_linux::installed_runtime::resolve_installed_runtime() {
                    Ok(binding) => binding,
                    Err(_) => {
                        eprintln!("narrivyd: installed resident runtime failed verification");
                        return exit_code(EXIT_FAILURE);
                    }
                };
            let store = Arc::new(narrivy_model_manager::ModelStore::new(
                cache_root,
                Arc::new(narrivy_model_manager::NoNetworkTransport),
                Arc::new(narrivy_platform_linux::model_cache_policy::PosixCacheFilePolicy::new()),
            ));
            let mut runtime = Some(runtime);
            let model = match store.verify_bundled_installed(&registry, model_id) {
                Ok(binding) => {
                    match narrivy_platform_linux::installed_runtime::bind_installed_model(&binding)
                    {
                        Ok(binding) => Some(binding),
                        Err(_) => {
                            eprintln!(
                                "narrivyd: selected installed model immutable binding failed"
                            );
                            return exit_code(EXIT_FAILURE);
                        }
                    }
                }
                Err(error) => {
                    if !installed_model_is_missing(&error) {
                        eprintln!("narrivyd: selected installed model verification failed fatally");
                        return exit_code(EXIT_FAILURE);
                    }
                    let Some(runtime) = runtime.take() else {
                        eprintln!("narrivyd: installed runtime ownership was lost");
                        return exit_code(EXIT_FAILURE);
                    };
                    if let Err(error) = state.initialize_awaiting_installed_model(runtime) {
                        eprintln!(
                            "narrivyd: installed resident degraded startup failed ({})",
                            error.code
                        );
                        return exit_code(EXIT_FAILURE);
                    }
                    eprintln!(
                        "narrivyd: serving with installed resident ASR unavailable (model_missing)"
                    );
                    pending_model_provisioning = Some((
                        Arc::clone(&store),
                        Arc::clone(&registry),
                        model_id.to_owned(),
                    ));
                    // The daemon remains reachable for truthful status. No
                    // resident process or fallback ASR is started.
                    None
                }
            };
            if let Some(model) = model {
                let Some(runtime) = runtime.take() else {
                    eprintln!("narrivyd: installed runtime ownership was lost");
                    return exit_code(EXIT_FAILURE);
                };
                if let Err(error) = state.initialize_installed_resident_asr(runtime, model) {
                    eprintln!(
                        "narrivyd: installed resident ASR startup failed ({})",
                        error.code
                    );
                    return exit_code(EXIT_FAILURE);
                }
            }
        } else {
            let Some(model) = engine_config.transcribe_model_path.as_deref() else {
                eprintln!("narrivyd: resident ASR requires an explicit model");
                return exit_code(EXIT_FAILURE);
            };
            let Some(model_sha256) = engine_config.transcribe_model_sha256.as_deref() else {
                eprintln!("narrivyd: resident ASR requires an explicit model SHA-256");
                return exit_code(EXIT_FAILURE);
            };
            let Some(model_size) = engine_config.transcribe_model_size else {
                eprintln!("narrivyd: resident ASR requires an explicit model size");
                return exit_code(EXIT_FAILURE);
            };
            // The model chosen in the app before the last shutdown is loaded
            // in place of the configured one. If it no longer loads, the
            // configured model is used.
            state.model_selection().persist_under(&state_dir);
            // The engine loaded at start-up follows the configured device
            // request (automatic unless the operator set one).
            if let Some(compute) = std::env::var("NARRIVY_TRANSCRIBE_COMPUTE")
                .ok()
                .and_then(|value| daemon::model_selection::ComputeChoice::parse(&value))
            {
                state.model_selection().set_current_compute(compute);
            }
            // An OpenVINO model is not loaded at start-up: the configured model
            // starts at once, and a pipeline that wants the OpenVINO model
            // loads it when it starts.
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
                    if let Err(error) =
                        state.initialize_resident_asr(model, model_sha256, model_size)
                    {
                        eprintln!("narrivyd: resident ASR startup failed ({})", error.code);
                        return exit_code(EXIT_FAILURE);
                    }
                }
            }
        }
    }
    #[cfg(feature = "asr-transcribe-resident")]
    if let Some((store, registry, model_id)) = pending_model_provisioning {
        if let Err(error) = state.configure_offline_model_provisioning(
            store,
            registry,
            model_id,
            Arc::clone(&supervisor),
        ) {
            eprintln!(
                "narrivyd: offline model provisioning composition failed ({})",
                error.code
            );
            return exit_code(EXIT_FAILURE);
        }
    }
    let sink: DeliverySink = Arc::new(std::sync::Mutex::new(Vec::new()));

    let mut pump = Some(spawn_event_pump(server.clone(), state.clone()));
    // The default pipeline's model, when start-up loaded another one (an
    // OpenVINO model), loads now in the background.
    // The echo canceller starts now too, so dictation never waits for it.
    daemon::echo::warm();
    // The voice filter too: the first dictation should not wait for it.
    daemon::voice::warm();
    // Press and hold, when the user chose a key.
    daemon::hold::start(socket_path.clone());
    #[cfg(feature = "asr-transcribe-resident")]
    daemon::warm_default_pipeline(&state, &supervisor, &engine_config);
    // The clean-up model, when one is configured, loads in the background
    // too; dictation uses the deterministic clean-up until it is ready.
    let _ = daemon::refine::shared();
    let portal_pump_shutdown = Arc::new(AtomicBool::new(false));
    let mut portal_pump = Some(spawn_portal_event_pump(
        Arc::clone(&server),
        Arc::clone(&portal_service),
        Arc::clone(&portal_pump_shutdown),
    ));

    let worker_start_delay = std::env::var("NARRIVY_WORKER_START_DELAY_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_millis);
    // The content-free diagnostics surface (REL-008). It owns no thread, no
    // timer and no open connection: it only accumulates health facts and
    // per-session latency samples that a client renders a support report from,
    // so `diagnostics.support-report=available` is a composition fact here and
    // every session records into it on its own terminal path.
    let diagnostics = Arc::new(daemon::diagnostics::DaemonDiagnostics::new());
    // Compute planning (P5). Discovery is a bounded, one-shot observation of
    // this host, so it is taken once here and the plan is immutable for the
    // daemon's lifetime; a per-session re-derivation would make a capability
    // token depend on when a client happened to ask. A failed plan is not a
    // startup failure: it means no accelerator may be selected, which the
    // capability list already states truthfully.
    //
    // Discovery stays before `serve` (F-H7). The protocol has no daemon-wide
    // `starting` state: `StatusDto` carries no readiness field and the
    // capability list has only `compute.device=resolved` and
    // `compute.device=unavailable:no_qualified_plan`. Serving first would make
    // the first status either claim no plan that may still resolve, or need a
    // new wire token every client must learn. Clients are not shut out
    // meanwhile: the socket is bound and listening above, so `connect`
    // succeeds and the handshake is answered once `serve` starts. Each probe
    // is bounded by `narrivy_compute::discovery::PROBE_TIMEOUT` (10 s); a
    // probe that runs to that bound can outlast one client's 5 s handshake,
    // which that client then reports as a timeout.
    // `slow_compute_discovery_still_accepts_a_client` pins this.
    let compute_plan = Arc::new(daemon::compute::build_plan(
        &state_dir,
        std::env::var_os("NARRIVY_COMPUTE_SIDECAR")
            .map(std::path::PathBuf::from)
            .as_deref(),
    ));
    startup_socket_guard.disarm();
    // Printed only now: discovery above runs before `serve` accepts clients.
    eprintln!("narrivyd: serving on {}", socket_path.display());
    // The assistant extension's routines run on their schedule.
    daemon::assistant_library::start_routines(&state);
    let serve_result = server.serve(build_handler_with_portal(
        state.clone(),
        journal,
        history.clone(),
        sink,
        supervisor.clone(),
        worker_start_delay,
        Arc::new(engine_config),
        Some(Arc::clone(&portal_service)),
        Some(Arc::clone(&diagnostics)),
        Some(Arc::clone(&compute_plan)),
    ));
    // The retained History store owns no thread, timer, or open connection
    // between calls, so its lifetime ends with the serve window. The handler
    // holding the worker-facing reference is already dropped inside `serve`;
    // releasing the composition root's own reference here makes that ordering
    // explicit and before any bounded join below.
    drop(history);

    // One monotonic stop budget for every owner below (ADR-0069). The IPC
    // server armed it on the first shutdown request — the signal bridge, an
    // explicit stop, or a fatal serve error — so the connection cleanup that
    // already ran inside `serve` spent the same total as this sequence instead
    // of receiving a fresh window after `serve` returned.
    if !shutdown_budget.is_armed() {
        // `serve` can only return after a shutdown request armed the budget;
        // this keeps one fixed deadline on any unexpected return path.
        shutdown_budget.arm();
    }
    let shutdown_deadline = shutdown_budget.deadline();

    let signal_listener_failed = match signal_listener.finish_until(shutdown_deadline) {
        Ok(()) => false,
        Err(error) => {
            eprintln!("narrivyd: {error}");
            true
        }
    };
    // Remove the daemon lookup handle first. No status/request path may
    // schedule resident recovery once owned shutdown has begun.
    #[cfg(feature = "asr-transcribe-resident")]
    let resident_asr = state.take_resident_asr();

    // Close portal admission, hand the portal actor the shared deadline and
    // cancel it before any bounded join. Doing this first keeps portal cleanup
    // from becoming a sequential wait that consumes the budget later owners
    // need, while its join still happens below inside the same deadline.
    portal_service.begin_shutdown(shutdown_deadline);
    // Cancel every supervised worker now, without waiting. A session that is
    // waiting on the resident owner (or on any other dependency) must observe
    // cancellation before that owner's bounded cleanup starts, otherwise the
    // session's join can spend the whole shared deadline first.
    supervisor.cancel_all();

    // Resident cleanup is attempted before the session joins: it closes the
    // engine's admission and reaps owned process generations, which releases a
    // session blocked on the resident owner. An exhausted budget still signals
    // and retains every owned handle inside `shutdown_until`.
    #[cfg(feature = "asr-transcribe-resident")]
    let resident_shutdown_initial =
        resident_asr
            .as_ref()
            .map_or(ShutdownAttempt::Succeeded, |engine| {
                match engine.shutdown_until(shutdown_deadline) {
                    Ok(()) => ShutdownAttempt::Succeeded,
                    Err(error) => {
                        let attempt = if error.code == "resident_worker_shutdown_timeout" {
                            ShutdownAttempt::TimedOut
                        } else {
                            ShutdownAttempt::Failed
                        };
                        eprintln!("narrivyd: resident ASR shutdown failed ({})", error.code);
                        attempt
                    }
                }
            });
    #[cfg(not(feature = "asr-transcribe-resident"))]
    let resident_shutdown_initial = ShutdownAttempt::Succeeded;

    // The portal service join is bounded by the same shared deadline; a spent
    // budget still leaves admission closed and the owner cancelled above.
    let portal_shutdown_failed = match portal_service.shutdown_until(shutdown_deadline) {
        Ok(outcome) => {
            outcome.cleanup == narrivy_platform_linux::portal_eis::PortalCleanupStatus::Failed
        }
        Err(_) => true,
    };
    portal_pump_shutdown.store(true, Ordering::Release);
    let portal_pump_failed = join_thread_bounded(&mut portal_pump, shutdown_deadline).is_err();
    if portal_shutdown_failed {
        eprintln!("narrivyd: portal permission cleanup failed");
    }
    if portal_pump_failed {
        eprintln!("narrivyd: portal event pump did not stop within the shutdown budget");
    }

    // Every supervised session is joined inside the shared deadline; there is
    // no per-worker fresh window, and timed-out workers stay owned.
    let worker_shutdown_initial = match supervisor.shutdown_all_until(shutdown_deadline) {
        Ok(outcomes) => {
            let timed_out = outcomes
                .iter()
                .filter(|(_, outcome)| *outcome == WorkerStop::TimedOut)
                .count();
            if timed_out > 0 {
                eprintln!("narrivyd: {timed_out} worker(s) exceeded the shutdown deadline");
            }
            if timed_out > 0 {
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

    // Retries never establish a fresh window: each final pass spends whatever
    // remains of the same deadline. An initial non-timeout error stays sticky.
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

    // Closing resident admission on the first call may allow a supervised task
    // to release the resident lifecycle/worker lock during the worker retry. A
    // successful final call is the engine's guarantee that no owned generation
    // remains; it also spends only the shared remaining time.
    #[cfg(feature = "asr-transcribe-resident")]
    let resident_shutdown_retry = if resident_shutdown_initial != ShutdownAttempt::Succeeded {
        resident_asr
            .as_ref()
            .map(|engine| match engine.shutdown_until(shutdown_deadline) {
                Ok(()) => ShutdownAttempt::Succeeded,
                Err(error) => {
                    eprintln!("narrivyd: final resident ASR reap failed ({})", error.code);
                    if error.code == "resident_worker_shutdown_timeout" {
                        ShutdownAttempt::TimedOut
                    } else {
                        ShutdownAttempt::Failed
                    }
                }
            })
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
    if portal_shutdown_failed
        || portal_pump_failed
        || worker_shutdown_failed
        || resident_shutdown_failed
        || pump_failed
        || serve_failed
        || signal_listener_failed
    {
        return exit_code(EXIT_FAILURE);
    }
    std::process::ExitCode::SUCCESS
}

#[cfg(test)]
mod shutdown_attempt_tests {
    use super::{
        ShutdownAttempt, portal_eligibility_from, recover_startup_results, shutdown_attempt_failed,
    };

    #[test]
    fn startup_composition_recovers_a_committed_result_with_real_adapters()
    -> Result<(), Box<dyn std::error::Error>> {
        use narrivy_core::{JournalEntry, JournalEntryState, RecoveryJournal};
        use narrivy_platform_linux::delivery::InboxStore;
        use narrivy_types::{DeliveryIntent, TransactionId};

        let root = tempfile::tempdir()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
        }
        let journal = super::open_startup_journal(root.path())?;
        let txid = TransactionId::new("tx-startup-real-adapters");
        let now_ms = u64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_millis(),
        )?;
        let mut entry = JournalEntry::new_pending(txid.as_str(), now_ms);
        entry.state = JournalEntryState::Committed;
        entry.variants.raw = Some("private raw fixture".to_owned());
        entry.variants.corrected = Some("private corrected fixture".to_owned());
        entry.delivery_intent = Some(DeliveryIntent {
            transaction_id: txid.clone(),
            backend: "routed-then-inbox".to_owned(),
        });
        journal.write(&entry)?;
        let inbox = super::inbox_dir(root.path());

        let report = recover_startup_results(&journal, inbox.clone())?;

        assert_eq!(report.recovered, 1);
        assert_eq!(
            InboxStore::new(inbox).read_text(&txid)?,
            "private corrected fixture"
        );
        let recovered = journal
            .read(txid.as_str())?
            .ok_or("missing journal entry")?;
        assert_eq!(recovered.state, JournalEntryState::InboxRecovered);
        assert_eq!(recovered.variants.delivered, recovered.variants.corrected);
        Ok(())
    }

    #[test]
    fn portal_probe_mapping_enables_only_the_keyboard_capable_state() {
        use narrivy_ipc::PortalEligibilityDto;
        use narrivy_platform_linux::portal_eis::{PortalEisEligibility, PortalProbeFailure};

        assert_eq!(
            portal_eligibility_from(PortalEisEligibility::ReadyToRequestPermission { version: 2 }),
            PortalEligibilityDto::ReadyToRequest
        );
        assert_eq!(
            portal_eligibility_from(PortalEisEligibility::KeyboardUnavailable { version: 2 }),
            PortalEligibilityDto::Unavailable
        );
        assert_eq!(
            portal_eligibility_from(PortalEisEligibility::ProbeFailed(
                PortalProbeFailure::TimedOut
            )),
            PortalEligibilityDto::ProbeFailed
        );
    }

    #[cfg(feature = "asr-transcribe-resident")]
    #[test]
    fn installed_model_startup_degrades_only_for_explicit_absence() {
        use super::installed_model_is_missing;
        use narrivy_model_manager::ModelManagerError;

        assert!(installed_model_is_missing(
            &ModelManagerError::InstalledModelMissing {
                id: "bundled".to_owned(),
            }
        ));
        for error in [
            ModelManagerError::UnsafeInstalledModel {
                id: "bundled".to_owned(),
            },
            ModelManagerError::InstalledMetadataMismatch {
                id: "bundled".to_owned(),
            },
            ModelManagerError::InstalledSizeMismatch {
                id: "bundled".to_owned(),
            },
            ModelManagerError::InstalledDigestMismatch {
                id: "bundled".to_owned(),
            },
        ] {
            assert!(!installed_model_is_missing(&error));
        }
        assert!(!installed_model_is_missing(
            &ModelManagerError::UnknownBundledModel
        ));
        assert!(!installed_model_is_missing(
            &ModelManagerError::UnsafeCacheRoot
        ));
        assert!(!installed_model_is_missing(&ModelManagerError::Io(
            std::io::Error::new(std::io::ErrorKind::PermissionDenied, "controlled")
        )));
        let json_error = serde_json::Error::io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "controlled",
        ));
        assert!(!installed_model_is_missing(&ModelManagerError::Json(
            json_error
        )));
    }

    #[test]
    fn ordinary_shutdown_error_is_sticky_but_timeout_requires_final_certainty() {
        assert!(shutdown_attempt_failed(
            ShutdownAttempt::Failed,
            Some(ShutdownAttempt::Succeeded)
        ));
        assert!(!shutdown_attempt_failed(
            ShutdownAttempt::TimedOut,
            Some(ShutdownAttempt::Succeeded)
        ));
        assert!(shutdown_attempt_failed(
            ShutdownAttempt::TimedOut,
            Some(ShutdownAttempt::TimedOut)
        ));
        assert!(shutdown_attempt_failed(
            ShutdownAttempt::TimedOut,
            Some(ShutdownAttempt::Failed)
        ));
        assert!(shutdown_attempt_failed(ShutdownAttempt::TimedOut, None));
        assert!(!shutdown_attempt_failed(ShutdownAttempt::Succeeded, None));
        assert!(!shutdown_attempt_failed(
            ShutdownAttempt::Succeeded,
            Some(ShutdownAttempt::Succeeded)
        ));
        assert!(shutdown_attempt_failed(
            ShutdownAttempt::Succeeded,
            Some(ShutdownAttempt::TimedOut)
        ));
        assert!(shutdown_attempt_failed(
            ShutdownAttempt::Succeeded,
            Some(ShutdownAttempt::Failed)
        ));
    }
}

#[cfg(test)]
mod signal_bridge_tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;

    use narrivy_ipc::{Envelope, IpcError};
    use narrivy_ipc_unix::{PeerCredentials, UnixIpcServer};

    use super::{SignalEvent, SignalSource, run_signal_bridge};

    fn handler(_credentials: PeerCredentials, _envelope: Envelope) -> Option<Envelope> {
        None
    }

    /// A serve loop that is provably past its first accept iteration.
    struct LiveServeLoop {
        _dir: tempfile::TempDir,
        server: Arc<UnixIpcServer>,
        serve: std::thread::JoinHandle<Result<(), IpcError>>,
    }

    fn live_serve_loop() -> Result<LiveServeLoop, Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let socket = dir.path().join("narrivyd.sock");
        let server = Arc::new(UnixIpcServer::bind(&socket)?);
        let serve_server = Arc::clone(&server);
        let serve = std::thread::spawn(move || serve_server.serve(handler));
        let probe = std::os::unix::net::UnixStream::connect(&socket)?;
        drop(probe);
        std::thread::sleep(Duration::from_millis(50));
        Ok(LiveServeLoop {
            _dir: dir,
            server,
            serve,
        })
    }

    /// Join a serve loop with a bound; a timeout is the original defect.
    fn stop_and_join(
        serve: std::thread::JoinHandle<Result<(), IpcError>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let _ = done_tx.send(serve.join());
        });
        let joined = done_rx
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| "serve loop kept running after the signal bridge ended")?;
        waiter
            .join()
            .map_err(|_| "join helper thread panicked unexpectedly")?;
        joined
            .map_err(|_| "serve thread panicked unexpectedly")?
            .map_err(|error| format!("serve failed while stopping: {error}"))?;
        Ok(())
    }

    /// Returns the same scripted event on every poll.
    struct FixedSource(SignalEvent);

    impl SignalSource for FixedSource {
        fn next_event(&mut self, _timeout: Duration) -> SignalEvent {
            self.0
        }
    }

    #[test]
    fn observed_signal_stops_the_serve_loop_and_reports_success()
    -> Result<(), Box<dyn std::error::Error>> {
        let live = live_serve_loop()?;
        let cancel = AtomicBool::new(false);
        run_signal_bridge(
            live.server.as_ref(),
            &cancel,
            &mut FixedSource(SignalEvent::Interrupt),
        )
        .map_err(|error| format!("observed signal reported a bridge failure: {error}"))?;
        stop_and_join(live.serve)
    }

    /// Regression test for the original defect: an unexpected listener end
    /// returned `Err` while `serve` kept blocking forever, because `finish`
    /// only runs after `serve` returns.
    #[test]
    fn unexpected_source_end_stops_the_serve_loop_and_reports_failure()
    -> Result<(), Box<dyn std::error::Error>> {
        let live = live_serve_loop()?;
        let cancel = AtomicBool::new(false);
        let outcome = run_signal_bridge(
            live.server.as_ref(),
            &cancel,
            &mut FixedSource(SignalEvent::Closed("SIGTERM")),
        );
        if outcome.is_ok() {
            return Err("a closed signal source must be reported as a bridge failure".into());
        }
        stop_and_join(live.serve)
    }

    #[test]
    fn explicit_stop_ends_the_bridge_without_failure() -> Result<(), Box<dyn std::error::Error>> {
        let live = live_serve_loop()?;
        let cancel = AtomicBool::new(true);
        run_signal_bridge(
            live.server.as_ref(),
            &cancel,
            &mut FixedSource(SignalEvent::Pending),
        )
        .map_err(|error| format!("explicit stop reported a bridge failure: {error}"))?;
        stop_and_join(live.serve)
    }
}
