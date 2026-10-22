//! Daemon-facing owner for the process-isolated resident ASR worker.
//!
//! One worker keeps one CPU model loaded. Requests are serialized with
//! fail-fast admission; Narrivy never queues an unbounded number of audio
//! buffers behind the upstream model's single-compute contract. The parent
//! owns every pipe and the child, uses nonblocking I/O, and can kill/reap the
//! native boundary on cancellation or timeout.

use std::collections::VecDeque;
use std::io::{Read, Write};
#[cfg(test)]
use std::os::unix::fs::PermissionsExt;
#[cfg(test)]
use std::os::unix::process::CommandExt;
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
#[cfg(test)]
use std::process::Command;
use std::process::{Child, ChildStdin, ChildStdout, Stdio};
use std::sync::{
    Arc, Condvar, Mutex, OnceLock, TryLockError,
    atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

use narrivy_asr_api::{
    AsrCapabilities, AsrEngine, AsrError, AsrEvent, AsrRequest, AsrStream, CancelToken,
    OneShotAsrPort, UNSUPPORTED_LOCALE, UNSUPPORTED_OUTPUT_LANGUAGE, UNSUPPORTED_SCRIPT,
};
use narrivy_process_linux::{
    ArtifactError, ArtifactLaunchError, ArtifactProvenance, DataArtifactError, HelperEnvironment,
    ProcessError, VerifiedArtifactData, VerifiedArtifactExecutable, terminate_group_and_reap,
    try_wait_contained,
};
use narrivy_types::{AudioChunk, SessionCancellation, SessionId, Transcript};

use crate::resident_protocol::{
    self, ComputeRequest, HEADER_LEN, MAX_SAMPLES, OP_DECODE, OP_SHUTDOWN, OP_STREAM_BEGIN,
    OP_STREAM_FEED, OP_STREAM_FINALIZE, OP_STREAM_RESET, PCM_TRANSFER_BYTES, PCM_TRANSFER_SAMPLES,
    RecognitionOptions, RequestHeader, STATUS_BACKEND_MISMATCH, STATUS_CANCELLATION_UNSUPPORTED,
    STATUS_DECODE_FAILED, STATUS_INVALID_AUDIO, STATUS_INVALID_REQUEST,
    STATUS_MODEL_BINDING_FAILED, STATUS_MODEL_FORMAT_UNSUPPORTED, STATUS_MODEL_LOAD_FAILED,
    STATUS_NATIVE_BACKEND_FAILED, STATUS_NATIVE_STARTUP_OTHER, STATUS_OK, STATUS_OUT_OF_MEMORY,
    STATUS_READY, STATUS_SAMPLE_RATE_MISMATCH, STATUS_STREAM_FINAL, STATUS_STREAM_RESET,
    STATUS_STREAM_STARTED, STATUS_STREAM_UPDATE, STATUS_UNSUPPORTED_REQUEST,
    STATUS_VERSION_MISMATCH, WorkerCapabilities, WorkerCompute,
};

pub const WORKER_BIN_ENV: &str = "NARRIVY_TRANSCRIBE_WORKER";
pub const WORKER_SHA256_ENV: &str = "NARRIVY_TRANSCRIBE_WORKER_SHA256";
pub const WORKER_SIZE_ENV: &str = "NARRIVY_TRANSCRIBE_WORKER_SIZE";
pub const MODEL_SHA256_ENV: &str = "NARRIVY_TRANSCRIBE_MODEL_SHA256";
pub const MODEL_SIZE_ENV: &str = "NARRIVY_TRANSCRIBE_MODEL_SIZE";
/// `auto` (the default) lets the worker use a GPU that passes its self-test;
/// `gpu` requires one; `cpu` keeps recognition on the CPU.
pub const COMPUTE_ENV: &str = "NARRIVY_TRANSCRIBE_COMPUTE";
pub const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
/// Floor for one resident decode round trip.
///
/// A CPU batch model's work is proportional to the submitted audio, so the
/// effective deadline is `decode_budget`; this floor keeps short requests
/// bounded exactly as before.
pub const DECODE_TIMEOUT: Duration = Duration::from_secs(120);
/// Additional decode budget granted per second of submitted 16 kHz audio.
const DECODE_TIMEOUT_PER_AUDIO_SECOND: Duration = Duration::from_secs(1);
/// Absolute ceiling for one decode round trip. A wedged worker therefore can
/// never hold a session indefinitely; cancellation remains responsive because
/// every write/read loop polls the shared cancellation signal.
pub const MAX_DECODE_TIMEOUT: Duration = Duration::from_secs(600);
pub const STREAM_BEGIN_TIMEOUT: Duration = Duration::from_secs(2);
pub const TERMINATION_TIMEOUT: Duration = Duration::from_secs(2);
const GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(250);
const POLL_INTERVAL: Duration = Duration::from_millis(2);

/// Decode budget for one request carrying `sample_count` frames at the
/// protocol's fixed 16 kHz rate: the floor plus one second per audio second,
/// capped at [`MAX_DECODE_TIMEOUT`].
fn decode_budget(sample_count: usize) -> Duration {
    let audio_seconds = sample_count / narrivy_types::CAPTURE_SAMPLE_RATE_HZ as usize;
    let scaled = u32::try_from(audio_seconds)
        .ok()
        .and_then(|seconds| DECODE_TIMEOUT.checked_add(DECODE_TIMEOUT_PER_AUDIO_SECOND * seconds))
        .unwrap_or(MAX_DECODE_TIMEOUT);
    scaled.min(MAX_DECODE_TIMEOUT)
}

#[cfg(test)]
static STARTUP_TERMINATION_FAILURES: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Test-only simulated resource failure: while this is above zero, the next
/// `SpawnAnchor::start` reports the typed failure a refused thread creation
/// would, before any thread exists and before any resident child can be
/// admitted. Production builds cannot reach it (`cfg(test)` only), so it never
/// slows or fails a real engine.
#[cfg(test)]
static FAILED_HANDOFF_OWNER_INITIALIZATIONS: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone)]
struct WorkerConfig {
    executable: WorkerExecutable,
    model: WorkerModel,
    worker_origin: WorkerBindingOrigin,
    model_origin: ModelBindingOrigin,
    compute: Arc<ComputePolicy>,
    /// Arguments a worker other than transcribe.cpp needs ahead of the
    /// common ones, such as where its runtime and model directory are.
    leading_args: Arc<[std::ffi::OsString]>,
}

#[cfg(test)]
thread_local! {
    /// What controlled test workers are asked for. A test opts into `Auto`.
    static CONTROLLED_COMPUTE: std::cell::Cell<ComputeRequest> =
        const { std::cell::Cell::new(ComputeRequest::Cpu) };
}

/// What the next worker is asked to run on. It starts from the
/// configuration (or the caller's explicit choice) and drops to strict CPU,
/// for the life of the engine, the first time a worker that was allowed a
/// GPU fails.
struct ComputePolicy {
    request: AtomicU8,
}

const REQUEST_AUTO: u8 = 0;
const REQUEST_GPU: u8 = 1;
const REQUEST_CPU: u8 = 2;
const REQUEST_NPU: u8 = 3;

impl ComputePolicy {
    fn new(request: ComputeRequest) -> Self {
        Self {
            request: AtomicU8::new(match request {
                ComputeRequest::Auto => REQUEST_AUTO,
                ComputeRequest::Gpu => REQUEST_GPU,
                ComputeRequest::Npu => REQUEST_NPU,
                ComputeRequest::Cpu => REQUEST_CPU,
            }),
        }
    }

    fn from_env() -> Result<Self, AsrError> {
        match std::env::var(COMPUTE_ENV) {
            Err(std::env::VarError::NotPresent) => Ok(Self::new(ComputeRequest::Auto)),
            Ok(value) => ComputeRequest::from_argument(&value)
                .map(Self::new)
                .ok_or_else(compute_configuration_error),
            Err(std::env::VarError::NotUnicode(_)) => Err(compute_configuration_error()),
        }
    }

    fn request(&self) -> ComputeRequest {
        match self.request.load(Ordering::Acquire) {
            REQUEST_AUTO => ComputeRequest::Auto,
            REQUEST_GPU => ComputeRequest::Gpu,
            REQUEST_NPU => ComputeRequest::Npu,
            _ => ComputeRequest::Cpu,
        }
    }

    fn allows_gpu(&self) -> bool {
        self.request() != ComputeRequest::Cpu
    }

    fn withdraw_gpu(&self) {
        self.request.store(REQUEST_CPU, Ordering::Release);
    }
}

fn compute_configuration_error() -> AsrError {
    AsrError::new(
        "resident_compute_configuration_invalid",
        "the resident ASR compute setting must be auto, gpu, npu or cpu",
    )
}

/// Failures that mean the worker itself, not the request, went wrong.
fn is_worker_failure(code: &str) -> bool {
    matches!(
        code,
        "decode_timeout"
            | "decode_failed"
            | "resident_worker_io"
            | "resident_worker_protocol"
            | "resident_worker_unavailable"
    )
}

#[derive(Clone, Copy)]
enum WorkerBindingOrigin {
    Controlled,
    TrustAnchor,
}

#[derive(Clone, Copy)]
enum ModelBindingOrigin {
    Controlled,
    TrustAnchor,
}

#[derive(Clone)]
enum WorkerExecutable {
    DigestBound(VerifiedArtifactExecutable),
    #[cfg(test)]
    Controlled(PathBuf),
}

#[derive(Clone)]
enum WorkerModel {
    DigestBound(VerifiedArtifactData),
    #[cfg(test)]
    Controlled(PathBuf),
}

struct SharedWorker {
    config: WorkerConfig,
    /// What the model can do. Its device field is the first worker's; the
    /// live worker's device is `compute`.
    capabilities: WorkerCapabilities,
    /// The device the live worker attested, as [`WorkerCompute`] wire value.
    compute: AtomicU8,
    readiness: AtomicU8,
    admission_closed: AtomicBool,
    lifecycle: Mutex<()>,
    recovery_in_flight: AtomicUsize,
    worker: Mutex<WorkerSlot>,
    spawn_anchor: SpawnAnchor,
    #[cfg(test)]
    recovery_post_handshake_hook: Mutex<Option<RecoveryPostHandshakeHook>>,
}

/// One long-lived thread that performs every process creation for this engine.
///
/// Child lifetime hardening installs `PR_SET_PDEATHSIG(SIGKILL)`, and Linux
/// binds that relationship to the thread that calls `clone`/`exec` — not to
/// the process or to whichever daemon thread happened to request startup. A
/// resident worker created by a short-lived provisioning or recovery worker
/// thread is therefore killed as soon as that thread returns. Routing every
/// spawn through this anchor (whose lifetime is the engine's `SharedWorker`)
/// keeps the kernel lifetime relationship truthful: no owner thread outlives
/// the process it created, and no process outlives its owner thread.
///
/// Explicit ownership:
/// - `stop` closes the request channel and retires the owner thread inside a
///   bounded budget, so teardown never claims completion while a spawn or an
///   abandoned-child reap could still be running; `in_flight` lets shutdown
///   prove the same quiescence.
/// - A child whose reply races caller cancellation, timeout, or abandonment is
///   owned by the anchor until it has been terminated and reaped.
/// - A request that cannot be handed off or answered inside the caller's
///   absolute deadline surfaces as the caller's own bounded control error
///   instead of blocking past it.
struct SpawnAnchor {
    requests: Option<std::sync::mpsc::SyncSender<SpawnRequest>>,
    retirement: Arc<Retirement>,
    owner: Option<std::thread::JoinHandle<()>>,
    in_flight: Arc<AtomicUsize>,
    /// Children abandoned by a cancelled, timed-out, or vanished caller whose
    /// bounded termination could not be confirmed. Ownership stays here so a
    /// later bounded reap can still resolve them; nothing is silently dropped.
    retained: Arc<Mutex<VecDeque<Child>>>,
    /// Retained fallback hand-off owner, created before any child can be
    /// admitted. `Drop` submits to this already-owned mechanism instead of
    /// spawning a thread or waiting on the contended process-wide registry.
    handoffs: FallbackHandoffOwner,
    /// Earliest deadline any caller already established for this anchor. A
    /// later drop may shorten its opportunistic window but never start a fresh
    /// full one past a service shutdown deadline.
    retire_deadline: Option<Instant>,
    /// Test-only switch that leaves the next abandoned child retained without a
    /// first termination attempt, so the retention/reap path is observable.
    #[cfg(test)]
    retain_abandoned_once: Arc<AtomicBool>,
    /// Test-only switch that makes the next retained reap report an
    /// unconfirmed termination without releasing ownership, so the forced
    /// reap-failure path is deterministic.
    #[cfg(test)]
    retained_reap_failures: Arc<AtomicUsize>,
}

/// One-shot latch flipped by the owner thread just before it returns, so
/// `stop` can prove retirement without an unbounded `join`.
#[derive(Default)]
struct Retirement {
    retired: Mutex<bool>,
    signal: Condvar,
}

impl Retirement {
    fn mark(&self) {
        let mut retired = self
            .retired
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *retired = true;
        self.signal.notify_all();
    }

    fn is_set(&self) -> bool {
        *self
            .retired
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn wait(&self, deadline: Instant) -> bool {
        let mut retired = self
            .retired
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while !*retired {
            let wait = SpawnAnchor::wait_slice(deadline);
            if wait.is_zero() {
                return false;
            }
            let (guard, _) = self
                .signal
                .wait_timeout(retired, wait)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            retired = guard;
        }
        true
    }
}

enum SpawnRequest {
    Spawn {
        config: WorkerConfig,
        cancellation: SessionCancellation,
        deadline: Instant,
        reply: std::sync::mpsc::SyncSender<Result<Child, AsrError>>,
    },
}

/// Counts one in-flight owner operation (spawn plus any abandoned-child reap).
struct SpawnActivity(Arc<AtomicUsize>);

impl SpawnActivity {
    fn begin(counter: &Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::AcqRel);
        Self(Arc::clone(counter))
    }
}

impl Drop for SpawnActivity {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl SpawnAnchor {
    fn start() -> Result<Self, AsrError> {
        // The retained hand-off owner exists before any resident child can be
        // admitted. If the process cannot create it, startup fails closed here,
        // while no child ownership exists yet, instead of discovering a missing
        // hand-off owner during `Drop`.
        #[cfg(test)]
        if FAILED_HANDOFF_OWNER_INITIALIZATIONS
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            return Err(FallbackHandoffOwner::unavailable());
        }
        let handoffs = FallbackHandoffOwner::start()?;
        let (requests, pending) = std::sync::mpsc::sync_channel::<SpawnRequest>(0);
        let retirement = Arc::new(Retirement::default());
        let owner_retirement = Arc::clone(&retirement);
        let in_flight = Arc::new(AtomicUsize::new(0));
        let owner_in_flight = Arc::clone(&in_flight);
        let retained = Arc::new(Mutex::new(VecDeque::new()));
        let owner_retained = Arc::clone(&retained);
        #[cfg(test)]
        let retain_abandoned_once = Arc::new(AtomicBool::new(false));
        #[cfg(test)]
        let owner_retain_abandoned = Arc::clone(&retain_abandoned_once);
        let owner = std::thread::Builder::new()
            .name("narrivy-resident-spawn-owner".to_owned())
            .spawn(move || {
                while let Ok(request) = pending.recv() {
                    let SpawnRequest::Spawn {
                        config,
                        cancellation,
                        deadline,
                        reply,
                    } = request;
                    let _activity = SpawnActivity::begin(&owner_in_flight);
                    let result = spawn_worker_process(&config, &cancellation, deadline);
                    if let Err(std::sync::mpsc::SendError(Ok(mut child))) = reply.send(result) {
                        // The caller timed out, was cancelled, or vanished
                        // between request and completion.
                        #[cfg(test)]
                        let skip_first_reap = owner_retain_abandoned.swap(false, Ordering::AcqRel);
                        #[cfg(not(test))]
                        let skip_first_reap = false;
                        let reaped = !skip_first_reap
                            && terminate_group_and_reap(
                                &mut child,
                                Instant::now() + TERMINATION_TIMEOUT,
                            )
                            .is_ok();
                        if !reaped {
                            // Ownership is retained for a later bounded reap. If
                            // it never succeeds, this owner thread's exit still
                            // delivers the parent-death signal as the final net.
                            owner_retained
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .push_back(child);
                        }
                    }
                }
                // Retire only after the request channel closed. `stop` waits on
                // this latch, so retirement is observable without racing a
                // still-running spawn.
                owner_retirement.mark();
            })
            .map_err(|_| {
                AsrError::new(
                    "resident_worker_spawn_owner_unavailable",
                    "resident ASR spawn owner could not be created",
                )
            })?;
        Ok(Self {
            requests: Some(requests),
            retirement,
            owner: Some(owner),
            in_flight,
            retained,
            handoffs,
            retire_deadline: None,
            #[cfg(test)]
            retain_abandoned_once,
            #[cfg(test)]
            retained_reap_failures: Arc::new(AtomicUsize::new(0)),
        })
    }

    /// Create one child on the long-lived anchor thread and return its owner.
    ///
    /// Both the rendezvous hand-off and the reply wait are bounded by the
    /// caller's absolute deadline and poll cancellation, so a busy or failed
    /// owner can only surface as the caller's typed control error.
    fn spawn(
        &self,
        config: &WorkerConfig,
        cancellation: &SessionCancellation,
        deadline: Instant,
    ) -> Result<Child, AsrError> {
        let requests = self.requests.as_ref().ok_or_else(Self::unavailable)?;
        let (reply, pending) = std::sync::mpsc::sync_channel::<Result<Child, AsrError>>(0);
        let mut request = SpawnRequest::Spawn {
            config: config.clone(),
            cancellation: cancellation.clone(),
            deadline,
            reply,
        };
        loop {
            check_startup_control(deadline, Some(cancellation))?;
            match requests.try_send(request) {
                Ok(()) => break,
                Err(std::sync::mpsc::TrySendError::Full(retained)) => {
                    request = retained;
                    let wait = Self::wait_slice(deadline);
                    if wait.is_zero() {
                        return Err(startup_timeout_error());
                    }
                    std::thread::sleep(wait);
                }
                Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                    return Err(Self::unavailable());
                }
            }
        }
        loop {
            check_startup_control(deadline, Some(cancellation))?;
            let wait = Self::wait_slice(deadline);
            if wait.is_zero() {
                return Err(startup_timeout_error());
            }
            match pending.recv_timeout(wait) {
                Ok(result) => return result,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(Self::unavailable());
                }
            }
        }
    }

    /// True when no spawn or abandoned-child reap is running on the owner.
    fn is_idle(&self) -> bool {
        self.in_flight.load(Ordering::Acquire) == 0
    }

    /// Retry bounded termination for every retained abandoned child. Ownership
    /// is only released for a child whose death and reap were confirmed; an
    /// unresolved retention returns a narrow typed failure.
    ///
    /// The registry lock is acquired with `try_lock` inside this caller's
    /// deadline: a blocking acquisition could otherwise wait out another
    /// caller's whole reap window and only then notice this caller's budget was
    /// already spent. Ownership still stays in the registry, so a caller that
    /// cannot acquire in time reports an unconfirmed reap instead of mutating
    /// or dropping anything.
    fn reap_retained_until(&self, deadline: Instant) -> Result<(), AsrError> {
        let mut retained = loop {
            match self.retained.try_lock() {
                Ok(retained) => break retained,
                Err(TryLockError::Poisoned(poisoned)) => break poisoned.into_inner(),
                Err(TryLockError::WouldBlock) => {
                    let wait = Self::wait_slice(deadline);
                    if wait.is_zero() {
                        // One final non-blocking observation: a concurrent
                        // caller may have confirmed the reap at the deadline.
                        match self.retained.try_lock() {
                            Ok(retained) if retained.is_empty() => return Ok(()),
                            _ => return Err(Self::reap_unconfirmed()),
                        }
                    }
                    std::thread::sleep(wait);
                }
            }
        };
        #[cfg(test)]
        if self
            .retained_reap_failures
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            // Deterministic forced failure: ownership is untouched and the
            // caller cannot mistake an unconfirmed termination for a reap.
            return Err(Self::reap_unconfirmed());
        }
        while let Some(mut child) = retained.pop_front() {
            let attempt = deadline.min(Instant::now() + TERMINATION_TIMEOUT);
            if terminate_group_and_reap(&mut child, attempt).is_err() {
                retained.push_back(child);
                return Err(Self::reap_unconfirmed());
            }
        }
        Ok(())
    }

    fn reap_unconfirmed() -> AsrError {
        AsrError::new(
            "resident_worker_reap_unconfirmed",
            "resident ASR worker termination could not be confirmed",
        )
    }

    /// Acquire the retained registry without blocking past `deadline`, so a
    /// contended drop cannot silently extend an exhausted service deadline.
    /// The caller keeps ownership when this returns an error.
    fn try_lock_retained_until(
        &self,
        deadline: Instant,
    ) -> Result<std::sync::MutexGuard<'_, VecDeque<Child>>, AsrError> {
        loop {
            match self.retained.try_lock() {
                Ok(retained) => return Ok(retained),
                Err(TryLockError::Poisoned(poisoned)) => return Ok(poisoned.into_inner()),
                Err(TryLockError::WouldBlock) => {
                    let wait = Self::wait_slice(deadline);
                    if wait.is_zero() {
                        return Err(Self::reap_unconfirmed());
                    }
                    std::thread::sleep(wait);
                }
            }
        }
    }

    #[cfg(test)]
    fn retain_next_abandoned_for_test(&self) {
        self.retain_abandoned_once.store(true, Ordering::Release);
    }

    #[cfg(test)]
    fn fail_retained_reaps_for_test(&self, attempts: usize) {
        self.retained_reap_failures
            .store(attempts, Ordering::Release);
    }

    #[cfg(test)]
    fn retained_count(&self) -> usize {
        self.retained
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }

    /// Bounded retirement within the caller's own teardown budget.
    #[cfg(test)]
    fn stop(&mut self) -> Result<(), AsrError> {
        self.stop_until(Instant::now() + TERMINATION_TIMEOUT)
    }

    /// Close the request channel and retire the owner thread before `deadline`.
    /// Success proves the owner is gone and no spawn remains in flight, so no
    /// child created through this anchor can be left unreaped by teardown.
    /// Retirement never extends `deadline`: the latch is only marked just
    /// before the closure returns, and `JoinHandle::is_finished` observes that
    /// the owner's main function returned — which is what stops further spawn
    /// and publication work. The final `join` is therefore a retirement join
    /// of a thread that runs no user-supplied code or destructors, not a wait
    /// on worker completion.
    fn stop_until(&mut self, deadline: Instant) -> Result<(), AsrError> {
        self.requests.take();
        // Remember the tightest deadline any caller established for this
        // anchor so a later `Drop` cannot open a fresh full window.
        self.retire_deadline = Some(match self.retire_deadline {
            Some(existing) => existing.min(deadline),
            None => deadline,
        });
        loop {
            // Either the owner marked its latch, or it ended without marking
            // one (a panic); both are observed inside the same budget.
            if self.retirement.is_set() || self.owner_finished() {
                break;
            }
            let wait = Self::wait_slice(deadline);
            if wait.is_zero() {
                return Err(Self::owner_still_active());
            }
            let _ = self.retirement.wait(Instant::now() + wait);
        }
        // Wait for the owner's closure to return before joining it, so the
        // join below cannot wait on any spawn or abandoned-child work.
        while !self.owner_finished() {
            let wait = Self::wait_slice(deadline);
            if wait.is_zero() {
                return Err(Self::owner_still_active());
            }
            std::thread::sleep(wait);
        }
        if let Some(owner) = self.owner.take() {
            if owner.join().is_err() {
                return Err(AsrError::new(
                    "resident_worker_spawn_owner_failed",
                    "resident ASR spawn owner terminated unexpectedly",
                ));
            }
        }
        Ok(())
    }

    fn owner_finished(&self) -> bool {
        self.owner
            .as_ref()
            .is_some_and(std::thread::JoinHandle::is_finished)
    }

    fn owner_still_active() -> AsrError {
        AsrError::new(
            "resident_worker_spawn_owner_still_active",
            "resident ASR spawn owner did not retire within its bound",
        )
    }

    fn wait_slice(deadline: Instant) -> Duration {
        deadline
            .saturating_duration_since(Instant::now())
            .min(POLL_INTERVAL)
    }

    fn unavailable() -> AsrError {
        AsrError::new(
            "resident_worker_spawn_owner_unavailable",
            "resident ASR spawn owner is not running",
        )
    }

    /// Deadline a `Drop` may still spend: the tightest caller deadline this
    /// anchor already knows, capped by the ordinary opportunistic window.
    fn drop_deadline(&self) -> Instant {
        let opportunistic = Instant::now() + TERMINATION_TIMEOUT;
        match self.retire_deadline {
            Some(existing) => existing.min(opportunistic),
            None => opportunistic,
        }
    }
}

/// Ownership a dropped owner hands to the process-wide fallback owner.
///
/// `registries` carries the *shared* abandoned-child publication queues, not a
/// snapshot: the spawning owner thread may still publish a failed-reap child
/// into one after the drop, so the fallback keeps the queue itself until the
/// writer has retired.
struct FallbackHandoff {
    workers: VecDeque<RetiringWorker>,
    owner_threads: Vec<std::thread::JoinHandle<()>>,
    registries: Vec<Arc<Mutex<VecDeque<Child>>>>,
}

impl FallbackHandoff {
    fn is_empty(&self) -> bool {
        self.workers.is_empty() && self.owner_threads.is_empty() && self.registries.is_empty()
    }
}

/// Hand-offs the retained fallback hand-off owner still has to install because
/// a contended process-wide lock could not be taken inside a drop budget.
/// Bounded cleanup must not report success while one is in flight.
static PENDING_HANDOFFS: AtomicU64 = AtomicU64::new(0);

/// Coordinator retirements still in flight: incremented by the owning engine
/// immediately before it drops its only hand-off sender, decremented by the
/// coordinator after it published its own join handle to the process-wide
/// fallback owner. A bounded cleanup may not report quiescence while this is
/// above zero, so a coordinator cannot retire between a clean report and its
/// registration.
static COORDINATOR_RETIREMENTS: AtomicU64 = AtomicU64::new(0);

fn install_fallback_handoff(state: &mut InitialRetiringState, handoff: FallbackHandoff) {
    state.queue.extend(handoff.workers);
    state.owner_threads.extend(handoff.owner_threads);
    state.registries.extend(handoff.registries);
    state.unconfirmed = state.unconfirmed.saturating_add(1);
}

/// Install a hand-off as soon as the fallback registry lock frees. Ownership is
/// never released here; this only waits for the lock.
fn install_fallback_handoff_blocking(handoff: FallbackHandoff) {
    loop {
        match initial_retiring().try_lock() {
            Ok(mut state) => {
                install_fallback_handoff(&mut state, handoff);
                return;
            }
            Err(TryLockError::Poisoned(poisoned)) => {
                let mut state = poisoned.into_inner();
                install_fallback_handoff(&mut state, handoff);
                return;
            }
            Err(TryLockError::WouldBlock) => std::thread::sleep(POLL_INTERVAL),
        }
    }
}

/// Retained fallback hand-off owner: one parked coordinator thread per engine,
/// created by `SpawnAnchor::start` *before* any resident child can be admitted.
///
/// Startup: if this thread cannot be created, `SpawnAnchor::start` fails with
/// `resident_worker_handoff_owner_unavailable` before the spawn-owner thread or
/// any child exists, so the failure is closed before child ownership.
///
/// Lifetime: the coordinator parks on its queue for as long as its engine
/// lives. Its join handle is never detached: the coordinator receives its own
/// handle at startup and publishes it to the process-wide fallback owner when
/// its engine drops the only sender, so a later bounded cleanup can prove
/// completion before joining it.
///
/// Shutdown: `Drop` submits through an unbounded channel, which never blocks on
/// the receiver or on the contended cleanup registry. A hand-off whose deadline
/// was exhausted is installed by this non-deadline-bound coordinator as soon as
/// the registry lock frees, and `PENDING_HANDOFFS` keeps cleanup from reporting
/// quiescence first. The engine never spawns a thread during `Drop`.
///
/// Failure: the only way a submission fails is a coordinator that panicked
/// before processing it. The caller then keeps ownership and installs under the
/// lock itself; nothing is detached, dropped, or reported clean while pending.
struct FallbackHandoffOwner {
    sender: Option<std::sync::mpsc::Sender<FallbackHandoff>>,
}

impl FallbackHandoffOwner {
    /// Park one coordinator for this engine and retain its join handle.
    fn start() -> Result<Self, AsrError> {
        let (handoffs, pending) = std::sync::mpsc::channel::<FallbackHandoff>();
        let (own_handle_tx, own_handle_rx) =
            std::sync::mpsc::channel::<std::thread::JoinHandle<()>>();
        let coordinator = std::thread::Builder::new()
            .name("narrivy-resident-fallback-handoff-owner".to_owned())
            .spawn(move || {
                // The engine hands this thread its own join handle before any
                // hand-off can be submitted, so the handle is never detached.
                let own_handle = own_handle_rx.recv().ok();
                while let Ok(handoff) = pending.recv() {
                    // Not deadline-bound: waiting for the contended registry
                    // lock here cannot extend any caller's stop budget.
                    install_fallback_handoff_blocking(handoff);
                    PENDING_HANDOFFS.fetch_sub(1, Ordering::AcqRel);
                }
                // The engine dropped its only sender: this coordinator is now
                // process-wide fallback ownership. Publish its own handle so a
                // later bounded cleanup joins it, then mark the retirement.
                if let Some(handle) = own_handle {
                    initial_retiring()
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .coordinator_threads
                        .push(handle);
                }
                COORDINATOR_RETIREMENTS.fetch_sub(1, Ordering::AcqRel);
            })
            .map_err(|_| Self::unavailable())?;
        if let Err(std::sync::mpsc::SendError(handle)) = own_handle_tx.send(coordinator) {
            // Defensive: the coordinator cannot exit before it received its own
            // handle (it blocks on that rendezvous), so this is only reachable
            // if that thread panicked. Retain the handle with the process-wide
            // owner instead of detaching it.
            initial_retiring()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .coordinator_threads
                .push(handle);
        }
        Ok(Self {
            sender: Some(handoffs),
        })
    }

    /// Submit a hand-off without waiting on the contended cleanup registry.
    /// The channel is unbounded, so this never blocks while the coordinator
    /// runs. A returned hand-off means the coordinator is gone and the caller
    /// still owns the hand-off.
    fn submit(&self, handoff: FallbackHandoff) -> Result<(), FallbackHandoff> {
        let Some(sender) = self.sender.as_ref() else {
            return Err(handoff);
        };
        PENDING_HANDOFFS.fetch_add(1, Ordering::AcqRel);
        sender
            .send(handoff)
            .map_err(|std::sync::mpsc::SendError(handoff)| {
                PENDING_HANDOFFS.fetch_sub(1, Ordering::AcqRel);
                handoff
            })
    }

    fn unavailable() -> AsrError {
        AsrError::new(
            "resident_worker_handoff_owner_unavailable",
            "resident ASR fallback hand-off owner could not be created",
        )
    }
}

impl Drop for FallbackHandoffOwner {
    fn drop(&mut self) {
        if let Some(sender) = self.sender.take() {
            // The engine is dropping its only sender. Mark the retirement
            // before the coordinator can wake up and register, so a concurrent
            // bounded cleanup cannot report quiescence first.
            COORDINATOR_RETIREMENTS.fetch_add(1, Ordering::AcqRel);
            drop(sender);
        }
    }
}

impl SpawnAnchor {
    /// Hand every still-live resident handle to the process-wide fallback owner.
    ///
    /// Called only from `Drop`, which cannot return an error. The handles stay
    /// owned (never detached) and `unconfirmed` stays above zero until a later
    /// bounded cleanup proves termination and rejoins the owner threads. The
    /// direct acquisition is bounded by `deadline`; when the registry stays
    /// contended past it, the hand-off is submitted to the already-owned
    /// coordinator instead of blocking the drop or spawning a new thread.
    fn hand_off_to_fallback_owner(&self, handoff: FallbackHandoff, deadline: Instant) {
        if handoff.is_empty() {
            return;
        }
        let pending = handoff;
        loop {
            match initial_retiring().try_lock() {
                Ok(mut state) => {
                    install_fallback_handoff(&mut state, pending);
                    return;
                }
                Err(TryLockError::Poisoned(poisoned)) => {
                    let mut state = poisoned.into_inner();
                    install_fallback_handoff(&mut state, pending);
                    return;
                }
                Err(TryLockError::WouldBlock) => {
                    let wait =
                        POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now()));
                    if !wait.is_zero() {
                        std::thread::sleep(wait);
                        continue;
                    }
                    // Deadline spent: the retained coordinator takes over. It
                    // never blocks this drop, and the pending count keeps
                    // cleanup from reporting the hand-off clean.
                    if let Err(handoff) = self.handoffs.submit(pending) {
                        // Unreachable unless the retained coordinator panicked.
                        // Ownership preservation takes priority over the
                        // exhausted budget: install under the lock directly.
                        install_fallback_handoff_blocking(handoff);
                    }
                    return;
                }
            }
        }
    }
}

impl Drop for SpawnAnchor {
    fn drop(&mut self) {
        // A drop cannot report failure. It therefore spends at most the
        // tightest deadline a service caller already established (never a
        // fresh full window past it), then hands every still-live handle to
        // the process-wide fallback owner instead of detaching it.
        let deadline = self.drop_deadline();
        // Reap retained children first: the owner thread may be what keeps a
        // request channel alive, and a confirmed child reap is what lets the
        // fallback counter return to zero.
        let _ = self.reap_retained_until(deadline);
        let _ = self.stop_until(deadline);

        let owner_thread = self.owner.take();
        // Drain what is already published without blocking past `deadline`. A
        // still-running owner thread can publish a failed-reap child *after*
        // this snapshot, so its shared registry travels with it to the
        // fallback owner instead of being released here.
        let mut hand_off_registry = owner_thread.is_some();
        let mut retained_children = VecDeque::new();
        match self.try_lock_retained_until(deadline) {
            Ok(mut retained) => {
                while let Some(child) = retained.pop_front() {
                    retained_children.push_back(RetiringWorker::Child(child));
                }
            }
            // A contended lock must not extend the exhausted deadline; the
            // registry below keeps ownership of anything the drain could not
            // observe.
            Err(_) => hand_off_registry = true,
        }
        let registries = if hand_off_registry {
            vec![Arc::clone(&self.retained)]
        } else {
            Vec::new()
        };
        self.hand_off_to_fallback_owner(
            FallbackHandoff {
                workers: retained_children,
                owner_threads: owner_thread.into_iter().collect(),
                registries,
            },
            deadline,
        );
    }
}

struct RecoveryAdmission<'a> {
    counter: &'a AtomicUsize,
}

impl Drop for RecoveryAdmission<'_> {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::AcqRel);
    }
}

struct WorkerSlot {
    client: Option<WorkerClient>,
    retiring: VecDeque<RetiringWorker>,
    active_stream: Option<u128>,
    generation: u64,
}

#[cfg(test)]
struct RecoveryPostHandshakeHook {
    entered: std::sync::mpsc::SyncSender<()>,
    release: std::sync::mpsc::Receiver<()>,
    fail_termination_once: bool,
}

#[cfg(test)]
struct InitialCleanupHook {
    entered: std::sync::mpsc::SyncSender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

/// Observable lifecycle of the process-isolated resident model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ResidentReadiness {
    Ready = 0,
    Active = 1,
    Warming = 2,
    RestartRequired = 3,
    FailedClosed = 4,
}

impl ResidentReadiness {
    fn from_atomic(value: u8) -> Self {
        match value {
            0 => Self::Ready,
            1 => Self::Active,
            2 => Self::Warming,
            3 => Self::RestartRequired,
            _ => Self::FailedClosed,
        }
    }
}

/// Result of an explicit resident-worker recovery request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoverOutcome {
    AlreadyReady,
    Recovered,
    AlreadyWarming,
}

/// Content-free status of the configured resident worker artifact. A
/// controlled digest-bound build never implies installed-package provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResidentWorkerArtifactStatus {
    ControlledDigestBound,
    TrustAnchorDigestBound,
    BindingCancelled,
    BindingTimedOut,
    PackageBindingUnavailable,
    MissingPath,
    MissingDigest,
    MissingSize,
    InvalidDigest,
    InvalidSize,
    MissingArtifact,
    NonExecutable,
    Untrusted,
    NotNativeElf,
    SizeMismatch,
    DigestMismatch,
    IdentityChanged,
    SnapshotUnavailable,
    IsolationUnavailable,
}

/// Content-free provenance of the model bytes retained by the resident.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResidentModelArtifactStatus {
    ControlledDigestBound,
    TrustAnchorDigestBound,
    PackageBindingUnavailable,
    StoredBindingUnavailable,
}

/// Resolve and immutably bind the current controlled worker configuration.
/// The returned handle is reused for every generation owned by an engine.
fn bind_worker_from_env() -> Result<VerifiedArtifactExecutable, ResidentWorkerArtifactStatus> {
    let path = std::env::var_os(WORKER_BIN_ENV)
        .map(PathBuf::from)
        .ok_or(ResidentWorkerArtifactStatus::MissingPath)?;
    let digest = std::env::var(WORKER_SHA256_ENV)
        .map_err(|_| ResidentWorkerArtifactStatus::MissingDigest)?;
    let size = std::env::var(WORKER_SIZE_ENV)
        .map_err(|_| ResidentWorkerArtifactStatus::MissingSize)?
        .parse::<u64>()
        .map_err(|_| ResidentWorkerArtifactStatus::InvalidSize)?;
    let executable = VerifiedArtifactExecutable::bind_controlled(&path, &digest, size)
        .map_err(resident_artifact_status)?;
    if executable.provenance() != ArtifactProvenance::ControlledBuild {
        return Err(ResidentWorkerArtifactStatus::PackageBindingUnavailable);
    }
    Ok(executable)
}

/// Resolve the same controlled artifact contract used by engine startup.
#[must_use]
pub fn resident_worker_artifact_status() -> ResidentWorkerArtifactStatus {
    match bind_worker_from_env() {
        Ok(_) => ResidentWorkerArtifactStatus::ControlledDigestBound,
        Err(status) => status,
    }
}

fn resident_artifact_status(error: ArtifactError) -> ResidentWorkerArtifactStatus {
    match error {
        ArtifactError::Cancelled => ResidentWorkerArtifactStatus::BindingCancelled,
        ArtifactError::TimedOut => ResidentWorkerArtifactStatus::BindingTimedOut,
        ArtifactError::Missing => ResidentWorkerArtifactStatus::MissingArtifact,
        ArtifactError::InvalidDigest => ResidentWorkerArtifactStatus::InvalidDigest,
        ArtifactError::InvalidSize => ResidentWorkerArtifactStatus::InvalidSize,
        ArtifactError::Untrusted => ResidentWorkerArtifactStatus::Untrusted,
        ArtifactError::NonExecutable => ResidentWorkerArtifactStatus::NonExecutable,
        ArtifactError::NotNativeElf => ResidentWorkerArtifactStatus::NotNativeElf,
        ArtifactError::SizeMismatch => ResidentWorkerArtifactStatus::SizeMismatch,
        ArtifactError::DigestMismatch => ResidentWorkerArtifactStatus::DigestMismatch,
        ArtifactError::IdentityChanged => ResidentWorkerArtifactStatus::IdentityChanged,
        ArtifactError::SnapshotUnavailable => ResidentWorkerArtifactStatus::SnapshotUnavailable,
        ArtifactError::UnsupportedPlatform => ResidentWorkerArtifactStatus::IsolationUnavailable,
    }
}

fn artifact_status_error(status: ResidentWorkerArtifactStatus) -> AsrError {
    let code = match status {
        ResidentWorkerArtifactStatus::ControlledDigestBound => "resident_worker_unavailable",
        ResidentWorkerArtifactStatus::TrustAnchorDigestBound => "resident_worker_unavailable",
        ResidentWorkerArtifactStatus::BindingCancelled => "resident_worker_binding_cancelled",
        ResidentWorkerArtifactStatus::BindingTimedOut => "resident_worker_binding_timeout",
        ResidentWorkerArtifactStatus::PackageBindingUnavailable => {
            "resident_worker_package_binding_unavailable"
        }
        ResidentWorkerArtifactStatus::MissingPath => "resident_worker_missing",
        ResidentWorkerArtifactStatus::MissingDigest => "resident_worker_digest_missing",
        ResidentWorkerArtifactStatus::MissingSize => "resident_worker_size_missing",
        ResidentWorkerArtifactStatus::InvalidDigest => "resident_worker_digest_invalid",
        ResidentWorkerArtifactStatus::InvalidSize => "resident_worker_size_invalid",
        ResidentWorkerArtifactStatus::MissingArtifact => "resident_worker_missing",
        ResidentWorkerArtifactStatus::NonExecutable => "resident_worker_nonexecutable",
        ResidentWorkerArtifactStatus::Untrusted => "resident_worker_untrusted",
        ResidentWorkerArtifactStatus::NotNativeElf => "resident_worker_not_native_elf",
        ResidentWorkerArtifactStatus::SizeMismatch => "resident_worker_size_mismatch",
        ResidentWorkerArtifactStatus::DigestMismatch => "resident_worker_digest_mismatch",
        ResidentWorkerArtifactStatus::IdentityChanged => "resident_worker_identity_changed",
        ResidentWorkerArtifactStatus::SnapshotUnavailable => "resident_worker_snapshot_unavailable",
        ResidentWorkerArtifactStatus::IsolationUnavailable => {
            "resident_worker_isolation_unavailable"
        }
    };
    AsrError::new(code, "resident ASR worker artifact failed closed")
}

fn resident_launch_artifact_error(error: ArtifactError) -> AsrError {
    match error {
        ArtifactError::Cancelled => AsrError::new("cancelled", "ASR cancellation requested"),
        ArtifactError::TimedOut => startup_timeout_error(),
        other => artifact_status_error(resident_artifact_status(other)),
    }
}

fn model_binding_from_env() -> Result<(String, u64), AsrError> {
    let digest = std::env::var(MODEL_SHA256_ENV).map_err(|_| {
        AsrError::new(
            "resident_model_digest_missing",
            "resident ASR model digest binding is missing",
        )
    })?;
    let size = std::env::var(MODEL_SIZE_ENV)
        .map_err(|_| {
            AsrError::new(
                "resident_model_size_missing",
                "resident ASR model size binding is missing",
            )
        })?
        .parse::<u64>()
        .map_err(|_| {
            AsrError::new(
                "resident_model_size_invalid",
                "resident ASR model size binding is invalid",
            )
        })?;
    Ok((digest, size))
}

fn model_artifact_error(error: DataArtifactError) -> AsrError {
    let code = match error {
        DataArtifactError::Cancelled => "resident_model_binding_cancelled",
        DataArtifactError::TimedOut => "resident_model_binding_timeout",
        DataArtifactError::Missing => "resident_model_missing",
        DataArtifactError::InvalidDigest => "resident_model_digest_invalid",
        DataArtifactError::InvalidSize => "resident_model_size_invalid",
        DataArtifactError::Untrusted => "resident_model_untrusted",
        DataArtifactError::SizeMismatch => "resident_model_size_mismatch",
        DataArtifactError::DigestMismatch => "resident_model_digest_mismatch",
        DataArtifactError::IdentityChanged => "resident_model_identity_changed",
        DataArtifactError::SnapshotUnavailable => "resident_model_snapshot_unavailable",
        DataArtifactError::UnsupportedPlatform => "resident_model_isolation_unavailable",
    };
    AsrError::new(code, "resident ASR model artifact failed closed")
}

impl SharedWorker {
    fn compute(&self) -> WorkerCompute {
        WorkerCompute::from_wire(self.compute.load(Ordering::Acquire)).unwrap_or_default()
    }

    fn set_compute(&self, compute: WorkerCompute) {
        self.compute.store(compute.to_wire(), Ordering::Release);
    }

    /// A worker on a GPU went wrong: every later worker is strict CPU.
    fn note_failure(&self, code: &str) {
        if self.compute().is_accelerated() && is_worker_failure(code) {
            self.config.compute.withdraw_gpu();
        }
    }

    fn readiness(&self) -> ResidentReadiness {
        ResidentReadiness::from_atomic(self.readiness.load(Ordering::Acquire))
    }

    fn set_readiness(&self, readiness: ResidentReadiness) {
        self.readiness.store(readiness as u8, Ordering::Release);
    }

    fn admission_error() -> AsrError {
        AsrError::new(
            "resident_worker_shutdown",
            "resident ASR worker admission is closed",
        )
    }

    fn ensure_admission_open(&self) -> Result<(), AsrError> {
        if self.admission_closed.load(Ordering::Acquire) {
            Err(Self::admission_error())
        } else {
            Ok(())
        }
    }

    fn begin_recovery(&self) -> Result<RecoveryAdmission<'_>, AsrError> {
        let _lifecycle = self.lifecycle.lock().map_err(|_| {
            self.set_readiness(ResidentReadiness::FailedClosed);
            AsrError::new(
                "resident_worker_unhealthy",
                "resident ASR worker lifecycle owner is unavailable",
            )
        })?;
        self.ensure_admission_open()?;
        self.recovery_in_flight
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                count.checked_add(1)
            })
            .map_err(|_| {
                self.set_readiness(ResidentReadiness::FailedClosed);
                AsrError::new(
                    "resident_worker_admission_exhausted",
                    "resident ASR worker admission counter was exhausted",
                )
            })?;
        // Shutdown closes admission without waiting for this lock, so re-check
        // after taking the permit: a caller that passed the first check just
        // before closure must not start a new generation. The permit is
        // released by the guard on this failure path.
        let admission = RecoveryAdmission {
            counter: &self.recovery_in_flight,
        };
        if self.admission_closed.load(Ordering::Acquire) {
            drop(admission);
            return Err(Self::admission_error());
        }
        Ok(admission)
    }

    fn set_ready_if_open(&self) {
        if self.admission_closed.load(Ordering::Acquire) {
            self.set_readiness(ResidentReadiness::FailedClosed);
        } else {
            self.set_readiness(ResidentReadiness::Ready);
        }
    }

    #[cfg(test)]
    fn run_recovery_post_handshake_hook(&self) -> bool {
        let hook = self
            .recovery_post_handshake_hook
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        let Some(hook) = hook else {
            return false;
        };
        let _ = hook.entered.send(());
        let _ = hook.release.recv();
        hook.fail_termination_once
    }
}

impl Drop for SharedWorker {
    fn drop(&mut self) {
        // Drop cannot report failure and must not open a fresh full window: the
        // teardown deadline is capped by whatever service shutdown deadline the
        // anchor already observed.
        let teardown_deadline = self.spawn_anchor.drop_deadline();
        let slot = self
            .worker
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut still_owned = VecDeque::new();
        if let Some(mut worker) = slot.client.take()
            && worker.shutdown_until(teardown_deadline).is_err()
        {
            still_owned.push_back(RetiringWorker::Client(worker));
        }
        for mut worker in slot.retiring.drain(..) {
            if worker.terminate_until(teardown_deadline).is_err() {
                still_owned.push_back(worker);
            }
        }
        // Every unconfirmed worker goes to the process-wide fallback owner,
        // which keeps the sticky unconfirmed count until a later bounded
        // cleanup proves termination. Terminate owned children before retiring
        // their spawn owner, then hand any retention to the same fallback.
        self.spawn_anchor.hand_off_to_fallback_owner(
            FallbackHandoff {
                workers: still_owned,
                owner_threads: Vec::new(),
                registries: Vec::new(),
            },
            teardown_deadline,
        );
        let _ = self.spawn_anchor.reap_retained_until(teardown_deadline);
        let _ = self.spawn_anchor.stop_until(teardown_deadline);
    }
}

/// Cheaply clonable handle to one resident, process-isolated CPU model.
///
/// Clones share fail-fast single-request admission. A cancelled or unhealthy
/// child is removed and remains unavailable until explicit recovery from the
/// same executable/model pair; there is never a mock or sidecar substitution.
#[derive(Clone)]
pub struct ResidentWorkerEngine {
    shared: Arc<SharedWorker>,
}

impl std::fmt::Debug for ResidentWorkerEngine {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ResidentWorkerEngine")
            .field("model_loaded", &self.is_worker_ready())
            .field(
                "model_supports_streaming",
                &self.model_capabilities().supports_streaming,
            )
            .finish_non_exhaustive()
    }
}

impl ResidentWorkerEngine {
    /// Spawn and fully handshake the configured resident worker. No request is
    /// accepted and no readiness claim is possible until the native model has
    /// loaded (strict CPU, or a self-tested GPU when `auto` is requested) and
    /// emitted the protocol-ready frame.
    pub fn with_model_path(model: impl Into<PathBuf>) -> Result<Self, AsrError> {
        let model = model.into();
        let (digest, size) = model_binding_from_env()?;
        Self::with_model_binding(model, &digest, size)
    }

    /// Bind explicit model identity supplied by the composing service. The
    /// path is consumed only while creating the immutable snapshot.
    pub fn with_model_binding(
        model: impl Into<PathBuf>,
        expected_sha256: &str,
        expected_size: u64,
    ) -> Result<Self, AsrError> {
        let model = model.into();
        let executable = bind_worker_from_env().map_err(artifact_status_error)?;
        let model = VerifiedArtifactData::bind_controlled(&model, expected_sha256, expected_size)
            .map_err(model_artifact_error)?;
        Self::with_config(
            WorkerModel::DigestBound(model),
            WorkerExecutable::DigestBound(executable),
            ModelBindingOrigin::Controlled,
            WorkerBindingOrigin::Controlled,
            Instant::now() + STARTUP_TIMEOUT + TERMINATION_TIMEOUT,
            None,
            None,
        )
    }

    /// As [`Self::with_model_binding`], with the caller's device request,
    /// bounding and cooperatively cancelling the native model start-up. Used
    /// to load a model while the service is running, where shutdown must be
    /// able to stop the load.
    pub fn with_model_binding_until(
        model: impl Into<PathBuf>,
        expected_sha256: &str,
        expected_size: u64,
        compute: ComputeRequest,
        deadline: Instant,
        cancellation: &SessionCancellation,
    ) -> Result<Self, AsrError> {
        let model = model.into();
        let executable = bind_worker_from_env().map_err(artifact_status_error)?;
        let model = VerifiedArtifactData::bind_controlled(&model, expected_sha256, expected_size)
            .map_err(model_artifact_error)?;
        Self::with_config(
            WorkerModel::DigestBound(model),
            WorkerExecutable::DigestBound(executable),
            ModelBindingOrigin::Controlled,
            WorkerBindingOrigin::Controlled,
            deadline,
            Some(cancellation),
            Some(compute),
        )
    }

    /// Compose immutable artifacts bound beneath an independently validated
    /// trust anchor. This engine intentionally cannot attribute those anchors
    /// to an installed manifest, model registry, package, or release; the
    /// composing service owns any stronger authority-specific statement.
    pub fn with_trust_anchored_bindings(
        executable: VerifiedArtifactExecutable,
        model: VerifiedArtifactData,
    ) -> Result<Self, AsrError> {
        let cancellation = SessionCancellation::new();
        Self::with_trust_anchored_bindings_until(
            executable,
            model,
            Instant::now() + STARTUP_TIMEOUT + TERMINATION_TIMEOUT,
            &cancellation,
        )
    }

    /// Compose trust-anchored artifacts while bounding and cooperatively
    /// cancelling initial native model startup. `deadline` is absolute and is
    /// checked before process creation and throughout the readiness handshake.
    /// A startup error is returned only after the child has been reaped or its
    /// ownership has been retained for bounded cleanup by the next owner.
    pub fn with_trust_anchored_bindings_until(
        executable: VerifiedArtifactExecutable,
        model: VerifiedArtifactData,
        deadline: Instant,
        cancellation: &SessionCancellation,
    ) -> Result<Self, AsrError> {
        if executable.provenance() != ArtifactProvenance::TrustAnchorDigestBound {
            return Err(AsrError::new(
                "resident_worker_manifest_binding_required",
                "resident ASR installed worker binding is unavailable",
            ));
        }
        if model.provenance() != ArtifactProvenance::TrustAnchorDigestBound {
            return Err(AsrError::new(
                "resident_model_registry_binding_required",
                "resident ASR installed model binding is unavailable",
            ));
        }
        Self::with_config(
            WorkerModel::DigestBound(model),
            WorkerExecutable::DigestBound(executable),
            ModelBindingOrigin::TrustAnchor,
            WorkerBindingOrigin::TrustAnchor,
            deadline,
            Some(cancellation),
            None,
        )
    }

    #[cfg(test)]
    pub fn with_worker_path(
        model: impl Into<PathBuf>,
        executable: impl Into<PathBuf>,
    ) -> Result<Self, AsrError> {
        let cancellation = SessionCancellation::new();
        Self::with_worker_path_until(
            model,
            executable,
            Instant::now() + STARTUP_TIMEOUT + TERMINATION_TIMEOUT,
            &cancellation,
        )
    }

    #[cfg(test)]
    fn with_worker_path_until(
        model: impl Into<PathBuf>,
        executable: impl Into<PathBuf>,
        deadline: Instant,
        cancellation: &SessionCancellation,
    ) -> Result<Self, AsrError> {
        let executable = executable.into();
        validate_worker_executable(&executable)?;
        Self::with_config(
            WorkerModel::Controlled(model.into()),
            WorkerExecutable::Controlled(executable),
            ModelBindingOrigin::Controlled,
            WorkerBindingOrigin::Controlled,
            deadline,
            Some(cancellation),
            None,
        )
    }

    fn with_config(
        model: WorkerModel,
        executable: WorkerExecutable,
        model_origin: ModelBindingOrigin,
        worker_origin: WorkerBindingOrigin,
        deadline: Instant,
        cancellation: Option<&SessionCancellation>,
        compute: Option<ComputeRequest>,
    ) -> Result<Self, AsrError> {
        Self::with_launch(
            model,
            executable,
            model_origin,
            worker_origin,
            deadline,
            cancellation,
            compute,
            Vec::new(),
        )
    }

    /// Start a different resident worker that speaks the same protocol,
    /// such as the OpenVINO worker. `executable` is bound by its digest,
    /// `model` is the file the worker's model binding names (for an
    /// OpenVINO pack, its manifest), and `leading_args` come before the
    /// common `--compute` and model-binding arguments.
    #[allow(clippy::too_many_arguments)]
    pub fn with_worker_binding_until(
        executable: VerifiedArtifactExecutable,
        leading_args: Vec<std::ffi::OsString>,
        model: impl Into<PathBuf>,
        expected_sha256: &str,
        expected_size: u64,
        compute: ComputeRequest,
        deadline: Instant,
        cancellation: &SessionCancellation,
    ) -> Result<Self, AsrError> {
        let model = model.into();
        let model = VerifiedArtifactData::bind_controlled(&model, expected_sha256, expected_size)
            .map_err(model_artifact_error)?;
        Self::with_launch(
            WorkerModel::DigestBound(model),
            WorkerExecutable::DigestBound(executable),
            ModelBindingOrigin::Controlled,
            WorkerBindingOrigin::Controlled,
            deadline,
            Some(cancellation),
            Some(compute),
            leading_args,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn with_launch(
        model: WorkerModel,
        executable: WorkerExecutable,
        model_origin: ModelBindingOrigin,
        worker_origin: WorkerBindingOrigin,
        deadline: Instant,
        cancellation: Option<&SessionCancellation>,
        compute: Option<ComputeRequest>,
        leading_args: Vec<std::ffi::OsString>,
    ) -> Result<Self, AsrError> {
        #[cfg(test)]
        if let WorkerModel::Controlled(path) = &model {
            crate::validate_gguf_model(path)?;
        }
        let compute = match (&executable, compute) {
            (_, Some(request)) => ComputePolicy::new(request),
            (WorkerExecutable::DigestBound(_), None) => ComputePolicy::from_env()?,
            // Controlled test workers are strict CPU unless a test opts in.
            #[cfg(test)]
            (WorkerExecutable::Controlled(_), None) => {
                ComputePolicy::new(CONTROLLED_COMPUTE.with(std::cell::Cell::get))
            }
        };
        let config = WorkerConfig {
            executable,
            model,
            worker_origin,
            model_origin,
            compute: Arc::new(compute),
            leading_args: Arc::from(leading_args),
        };
        check_startup_control(deadline, cancellation)?;
        terminate_initial_retiring_until(deadline, cancellation)?;
        check_startup_control(deadline, cancellation)?;
        let operation_deadline = startup_operation_deadline(deadline)?;
        let spawn_anchor = SpawnAnchor::start()?;
        let spawn = || {
            WorkerClient::spawn_until(
                &config,
                operation_deadline,
                deadline,
                cancellation,
                &spawn_anchor,
            )
            .map_err(|failure| {
                let WorkerStartupFailure { error, owner } = failure;
                if let Some(owner) = owner {
                    initial_retiring()
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .queue
                        .push_back(*owner);
                }
                error
            })
        };
        let may_fall_back = config.compute.request() == ComputeRequest::Auto;
        let worker = match spawn() {
            Ok(worker) => worker,
            // The first worker could choose a GPU and did not start. That
            // withdrew the GPU, so one more attempt starts on the CPU. A
            // caller that required a GPU gets the failure instead.
            Err(error)
                if may_fall_back
                    && config.compute.request() == ComputeRequest::Cpu
                    && check_startup_control(operation_deadline, cancellation).is_ok() =>
            {
                drop(error);
                terminate_initial_retiring_until(deadline, cancellation)?;
                spawn()?
            }
            Err(error) => return Err(error),
        };
        let capabilities = worker.capabilities.clone();
        Ok(Self {
            shared: Arc::new(SharedWorker {
                config,
                compute: AtomicU8::new(capabilities.compute.to_wire()),
                capabilities,
                readiness: AtomicU8::new(ResidentReadiness::Ready as u8),
                admission_closed: AtomicBool::new(false),
                lifecycle: Mutex::new(()),
                recovery_in_flight: AtomicUsize::new(0),
                worker: Mutex::new(WorkerSlot {
                    client: Some(worker),
                    retiring: VecDeque::new(),
                    active_stream: None,
                    generation: 0,
                }),
                spawn_anchor,
                #[cfg(test)]
                recovery_post_handshake_hook: Mutex::new(None),
            }),
        })
    }

    /// Return the current resident lifecycle without waiting for model work.
    #[must_use]
    pub fn readiness(&self) -> ResidentReadiness {
        if self.shared.admission_closed.load(Ordering::Acquire) {
            return ResidentReadiness::FailedClosed;
        }
        let readiness = self.shared.readiness();
        if !matches!(
            readiness,
            ResidentReadiness::Ready | ResidentReadiness::Active
        ) {
            return readiness;
        }
        let mut slot = match self.shared.worker.try_lock() {
            Ok(slot) => slot,
            Err(TryLockError::WouldBlock) => return self.shared.readiness(),
            Err(TryLockError::Poisoned(poisoned)) => {
                let mut slot = poisoned.into_inner();
                let _ = terminate_slot(&self.shared, &mut slot);
                return ResidentReadiness::RestartRequired;
            }
        };
        let readiness = self.shared.readiness();
        if !matches!(
            readiness,
            ResidentReadiness::Ready | ResidentReadiness::Active
        ) {
            return readiness;
        }
        let deadline = Instant::now() + TERMINATION_TIMEOUT;
        match slot
            .client
            .as_mut()
            .map(|worker| try_wait_contained(&mut worker.child, deadline))
        {
            Some(Ok(None)) => readiness,
            Some(Ok(Some(_))) | None => {
                slot.client = None;
                slot.active_stream = None;
                self.shared
                    .set_readiness(ResidentReadiness::RestartRequired);
                ResidentReadiness::RestartRequired
            }
            Some(Err(ProcessError::UnexpectedDescendant)) => {
                slot.client = None;
                slot.active_stream = None;
                self.shared.set_readiness(ResidentReadiness::FailedClosed);
                ResidentReadiness::FailedClosed
            }
            Some(Err(_)) => {
                let _ = terminate_slot(&self.shared, &mut slot);
                ResidentReadiness::RestartRequired
            }
        }
    }

    #[must_use]
    pub fn is_worker_ready(&self) -> bool {
        matches!(
            self.readiness(),
            ResidentReadiness::Ready | ResidentReadiness::Active
        )
    }

    /// Native capability metadata attested by the loaded worker model during
    /// the startup handshake. This does not imply that Narrivy has composed or
    /// enabled a product path for the capability.
    #[must_use]
    pub fn model_capabilities(&self) -> WorkerCapabilities {
        WorkerCapabilities {
            compute: self.shared.compute(),
            ..self.shared.capabilities.clone()
        }
    }

    /// The device the live worker attested its model runs on. After a GPU
    /// worker fails, its replacement is strict CPU and this follows it.
    #[must_use]
    pub fn compute(&self) -> WorkerCompute {
        self.shared.compute()
    }

    /// Content-free provenance of the executable snapshot retained by this
    /// engine. This is a pure getter: it never reopens, recopies, or rehashes
    /// the configured path.
    #[must_use]
    pub fn artifact_status(&self) -> ResidentWorkerArtifactStatus {
        match (
            &self.shared.config.executable,
            self.shared.config.worker_origin,
        ) {
            (WorkerExecutable::DigestBound(executable), WorkerBindingOrigin::Controlled)
                if executable.provenance() == ArtifactProvenance::ControlledBuild =>
            {
                ResidentWorkerArtifactStatus::ControlledDigestBound
            }
            (WorkerExecutable::DigestBound(executable), WorkerBindingOrigin::TrustAnchor)
                if executable.provenance() == ArtifactProvenance::TrustAnchorDigestBound =>
            {
                ResidentWorkerArtifactStatus::TrustAnchorDigestBound
            }
            (WorkerExecutable::DigestBound(_), _) => {
                ResidentWorkerArtifactStatus::PackageBindingUnavailable
            }
            #[cfg(test)]
            (WorkerExecutable::Controlled(_), _) => ResidentWorkerArtifactStatus::Untrusted,
        }
    }

    /// Content-free provenance of the immutable model snapshot retained by
    /// this engine. This getter never reopens the configured source path.
    #[must_use]
    pub fn model_artifact_status(&self) -> ResidentModelArtifactStatus {
        match (&self.shared.config.model, self.shared.config.model_origin) {
            (WorkerModel::DigestBound(model), ModelBindingOrigin::Controlled)
                if model.provenance() == ArtifactProvenance::ControlledBuild =>
            {
                ResidentModelArtifactStatus::ControlledDigestBound
            }
            (WorkerModel::DigestBound(model), ModelBindingOrigin::TrustAnchor)
                if model.provenance() == ArtifactProvenance::TrustAnchorDigestBound =>
            {
                ResidentModelArtifactStatus::TrustAnchorDigestBound
            }
            (WorkerModel::DigestBound(_), _) => {
                ResidentModelArtifactStatus::PackageBindingUnavailable
            }
            #[cfg(test)]
            (WorkerModel::Controlled(_), _) => {
                ResidentModelArtifactStatus::StoredBindingUnavailable
            }
        }
    }

    /// Close admission and boundedly terminate every worker generation owned
    /// by this engine. Success means there is no active, warming, or retiring
    /// resident process left under this owner. The closure is permanent and
    /// shared by every clone.
    pub fn shutdown(&self, timeout: Duration) -> Result<(), AsrError> {
        let deadline = Instant::now().checked_add(timeout).ok_or_else(|| {
            AsrError::new(
                "invalid_timeout",
                "resident ASR shutdown timeout exceeds the platform clock range",
            )
        })?;
        self.shutdown_until(deadline)
    }

    /// Close admission and terminate every owned generation before `deadline`.
    ///
    /// The service composes one monotonic shutdown budget, so this entry point
    /// spends the shared remaining time instead of a fresh per-call window.
    /// Admission closure and owner cancellation are attempted even when the
    /// budget is already exhausted: a spent deadline still signals and retains
    /// owned handles and reports the unconfirmed reap truthfully rather than
    /// skipping cleanup.
    pub fn shutdown_until(&self, deadline: Instant) -> Result<(), AsrError> {
        // Admission closure never waits on a contended lock: a concurrent
        // `begin_recovery` re-checks this flag after incrementing its permit,
        // so closure is permanent even without the lifecycle lock.
        self.shared.admission_closed.store(true, Ordering::Release);
        self.shared.set_readiness(ResidentReadiness::FailedClosed);

        let lifecycle_poisoned = loop {
            match self.shared.lifecycle.try_lock() {
                Ok(_lifecycle) => break false,
                Err(TryLockError::Poisoned(poisoned)) => {
                    let _lifecycle = poisoned.into_inner();
                    break true;
                }
                Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                    std::thread::sleep(
                        POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())),
                    );
                }
                Err(TryLockError::WouldBlock) => {
                    self.cancel_owned_work();
                    return Err(shutdown_timeout_error());
                }
            }
        };

        // A recovery generation or an owner-thread reap may still be finishing
        // even after its admission permit was released. Shutdown is not
        // complete while either could still own a live process.
        while self.shared.recovery_in_flight.load(Ordering::Acquire) != 0
            || !self.shared.spawn_anchor.is_idle()
        {
            if Instant::now() >= deadline {
                self.cancel_owned_work();
                return Err(shutdown_timeout_error());
            }
            std::thread::sleep(
                POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())),
            );
        }

        // A child abandoned by a cancelled or timed-out spawn whose bounded
        // reap failed is still owned by the anchor. Shutdown may not report
        // success while that ownership is unresolved.
        loop {
            match self.shared.spawn_anchor.reap_retained_until(deadline) {
                Ok(()) => break,
                Err(_) if Instant::now() < deadline => {
                    std::thread::sleep(
                        POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())),
                    );
                }
                Err(_) => {
                    self.cancel_owned_work();
                    return Err(shutdown_timeout_error());
                }
            }
        }

        let mut slot = loop {
            match self.shared.worker.try_lock() {
                Ok(slot) => break slot,
                Err(TryLockError::Poisoned(poisoned)) => break poisoned.into_inner(),
                Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                    std::thread::sleep(
                        POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())),
                    );
                }
                Err(TryLockError::WouldBlock) => {
                    self.cancel_owned_work();
                    return Err(shutdown_timeout_error());
                }
            }
        };
        slot.active_stream = None;
        if let Some(worker) = slot.client.as_mut() {
            if let Err(error) = worker.shutdown_until(deadline) {
                if let Some(worker) = slot.client.take() {
                    slot.retiring.push_back(RetiringWorker::Client(worker));
                }
                // Exhausted budget: signal every retained handle and keep it
                // owned instead of reporting it as reaped.
                let _ = terminate_retiring_until(&mut slot, Instant::now());
                let _ = self.shared.spawn_anchor.reap_retained_until(Instant::now());
                return Err(error);
            }
            slot.client = None;
        }
        if let Err(error) = terminate_retiring_until(&mut slot, deadline) {
            let _ = terminate_retiring_until(&mut slot, Instant::now());
            let _ = self.shared.spawn_anchor.reap_retained_until(Instant::now());
            return Err(error);
        }
        self.shared.set_readiness(ResidentReadiness::FailedClosed);
        if lifecycle_poisoned {
            return Err(AsrError::new(
                "resident_worker_unhealthy",
                "resident ASR worker lifecycle owner was unavailable during shutdown",
            ));
        }
        Ok(())
    }

    /// Last-resort bounded cancellation when the shared budget is exhausted.
    ///
    /// Every still-owned handle is signalled with a zero-width deadline (the
    /// process adapter sends SIGKILL before it observes the deadline) and kept
    /// owned: children stay in the retiring/retained queues and the spawn owner
    /// stays under the anchor, so a later bounded reap or the drop fallback can
    /// still confirm termination. Nothing is reported as reaped.
    fn cancel_owned_work(&self) {
        let now = Instant::now();
        if let Ok(mut slot) = self.shared.worker.try_lock() {
            let _ = terminate_slot_until(&self.shared, &mut slot, now);
            let _ = terminate_retiring_until(&mut slot, now);
        }
        let _ = self.shared.spawn_anchor.reap_retained_until(now);
    }

    /// Explicitly replace a worker whose termination or protocol state became
    /// uncertain. Exactly one caller owns a warming generation. Native startup
    /// happens outside the owner mutex so readiness remains observable and a
    /// concurrent caller can fail fast with [`RecoverOutcome::AlreadyWarming`].
    pub fn recover(&self, cancellation: &CancelToken) -> Result<RecoverOutcome, AsrError> {
        if cancellation.is_cancelled() {
            return Err(AsrError::new(
                "cancelled",
                "resident ASR recovery was cancelled before startup",
            ));
        }
        let _recovery_admission = self.shared.begin_recovery()?;
        match self.shared.readiness() {
            ResidentReadiness::Ready | ResidentReadiness::Active => {
                return Ok(RecoverOutcome::AlreadyReady);
            }
            ResidentReadiness::Warming => return Ok(RecoverOutcome::AlreadyWarming),
            ResidentReadiness::FailedClosed | ResidentReadiness::RestartRequired => {}
        }

        let generation = {
            let mut slot = match self.shared.worker.lock() {
                Ok(slot) => slot,
                Err(poisoned) => {
                    self.shared.set_readiness(ResidentReadiness::FailedClosed);
                    poisoned.into_inner()
                }
            };
            match self.shared.readiness() {
                ResidentReadiness::Ready | ResidentReadiness::Active => {
                    return Ok(RecoverOutcome::AlreadyReady);
                }
                ResidentReadiness::Warming => return Ok(RecoverOutcome::AlreadyWarming),
                ResidentReadiness::FailedClosed => {
                    terminate_retiring_until(&mut slot, Instant::now() + TERMINATION_TIMEOUT)?;
                    return Err(readiness_error(ResidentReadiness::FailedClosed));
                }
                ResidentReadiness::RestartRequired => {
                    terminate_retiring_until(&mut slot, Instant::now() + TERMINATION_TIMEOUT)?;
                }
            }
            if slot.client.is_some() || slot.active_stream.is_some() {
                self.shared.set_readiness(ResidentReadiness::FailedClosed);
                return Err(AsrError::new(
                    "resident_worker_unhealthy",
                    "resident ASR restart state retained an owned worker",
                ));
            }
            slot.generation = slot.generation.checked_add(1).ok_or_else(|| {
                self.shared.set_readiness(ResidentReadiness::FailedClosed);
                AsrError::new(
                    "resident_worker_generation_exhausted",
                    "resident ASR recovery generation was exhausted",
                )
            })?;
            self.shared.set_readiness(ResidentReadiness::Warming);
            slot.generation
        };

        let startup_started = Instant::now();
        let spawned = WorkerClient::spawn_until(
            &self.shared.config,
            startup_started + STARTUP_TIMEOUT,
            startup_started + STARTUP_TIMEOUT + TERMINATION_TIMEOUT,
            Some(cancellation),
            &self.shared.spawn_anchor,
        );
        let replacement = match spawned {
            Ok(worker) => worker,
            Err(failure) => {
                let WorkerStartupFailure { error, owner } = failure;
                if let Some(owner) = owner {
                    self.shared
                        .worker
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .retiring
                        .push_back(*owner);
                }
                self.finish_failed_recovery(generation, ResidentReadiness::RestartRequired);
                return Err(error);
            }
        };

        #[cfg(test)]
        let mut replacement = replacement;
        #[cfg(test)]
        if self.shared.run_recovery_post_handshake_hook() {
            replacement.termination_failures_remaining = 1;
        }

        if cancellation.is_cancelled() {
            return Err(self.reject_recovery_candidate(
                generation,
                replacement,
                ResidentReadiness::RestartRequired,
                AsrError::new(
                    "cancelled",
                    "resident ASR recovery was cancelled after startup",
                ),
            ));
        }

        if self.shared.admission_closed.load(Ordering::Acquire) {
            return Err(self.reject_recovery_candidate(
                generation,
                replacement,
                ResidentReadiness::FailedClosed,
                SharedWorker::admission_error(),
            ));
        }

        if !replacement
            .capabilities
            .same_model(&self.shared.capabilities)
        {
            return Err(self.reject_recovery_candidate(
                generation,
                replacement,
                ResidentReadiness::FailedClosed,
                AsrError::new(
                    "resident_capability_changed",
                    "resident ASR model capabilities changed across worker restart",
                ),
            ));
        }

        let (mut slot, poisoned) = match self.shared.worker.lock() {
            Ok(slot) => (slot, false),
            Err(error) => {
                self.shared.set_readiness(ResidentReadiness::FailedClosed);
                (error.into_inner(), true)
            }
        };
        if poisoned {
            drop(slot);
            return Err(self.reject_recovery_candidate(
                generation,
                replacement,
                ResidentReadiness::FailedClosed,
                AsrError::new(
                    "resident_worker_unhealthy",
                    "the resident ASR worker owner is unavailable",
                ),
            ));
        }
        if slot.generation != generation || self.shared.readiness() != ResidentReadiness::Warming {
            drop(slot);
            return Err(self.reject_recovery_candidate(
                generation,
                replacement,
                ResidentReadiness::RestartRequired,
                AsrError::new(
                    "resident_worker_generation_changed",
                    "resident ASR recovery generation changed before installation",
                ),
            ));
        }
        if cancellation.is_cancelled() {
            drop(slot);
            return Err(self.reject_recovery_candidate(
                generation,
                replacement,
                ResidentReadiness::RestartRequired,
                AsrError::new(
                    "cancelled",
                    "resident ASR recovery was cancelled before installation",
                ),
            ));
        }
        if self.shared.admission_closed.load(Ordering::Acquire) {
            drop(slot);
            return Err(self.reject_recovery_candidate(
                generation,
                replacement,
                ResidentReadiness::FailedClosed,
                SharedWorker::admission_error(),
            ));
        }
        self.shared.set_compute(replacement.capabilities.compute);
        slot.client = Some(replacement);
        self.shared.set_ready_if_open();
        Ok(RecoverOutcome::Recovered)
    }

    fn finish_failed_recovery(&self, generation: u64, readiness: ResidentReadiness) {
        let slot = self
            .shared
            .worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if slot.generation == generation && self.shared.readiness() == ResidentReadiness::Warming {
            self.shared.set_readiness(readiness);
        }
    }

    fn reject_recovery_candidate(
        &self,
        generation: u64,
        mut candidate: WorkerClient,
        readiness: ResidentReadiness,
        primary: AsrError,
    ) -> AsrError {
        let cleanup = candidate.terminate_bounded();
        let mut slot = self
            .shared
            .worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if cleanup.is_err() {
            slot.retiring.push_back(RetiringWorker::Client(candidate));
        }
        if slot.generation == generation && self.shared.readiness() == ResidentReadiness::Warming {
            self.shared.set_readiness(readiness);
        }
        cleanup.err().unwrap_or(primary)
    }
}

fn reject_unapplied_written_settings(request: &AsrRequest) -> Result<(), AsrError> {
    if request.output.language.is_some() {
        return Err(AsrError::new(
            UNSUPPORTED_OUTPUT_LANGUAGE,
            "the resident backend cannot apply a written output language",
        ));
    }
    if request.output.script.is_some() {
        return Err(AsrError::new(
            UNSUPPORTED_SCRIPT,
            "the resident backend cannot apply an output script",
        ));
    }
    if request.locale.is_some() {
        return Err(AsrError::new(
            UNSUPPORTED_LOCALE,
            "the resident backend cannot apply a formatting locale",
        ));
    }
    Ok(())
}

impl OneShotAsrPort for ResidentWorkerEngine {
    fn recognize(
        &mut self,
        session: SessionId,
        audio: &AudioChunk,
        cancellation: &SessionCancellation,
    ) -> Result<Transcript, AsrError> {
        self.recognize_dispatch(session, audio, cancellation, &RecognitionOptions::default())
    }

    fn recognize_with(
        &mut self,
        session: SessionId,
        audio: &AudioChunk,
        cancellation: &SessionCancellation,
        request: &AsrRequest,
    ) -> Result<Transcript, AsrError> {
        self.capabilities().validate(request)?;
        reject_unapplied_written_settings(request)?;
        self.recognize_dispatch(
            session,
            audio,
            cancellation,
            &RecognitionOptions::from_request(request),
        )
    }
}

impl ResidentWorkerEngine {
    /// Recognise once. When the worker was on a GPU and failed, the audio is
    /// still in hand: start a CPU worker and finish this dictation there.
    fn recognize_dispatch(
        &mut self,
        session: SessionId,
        audio: &AudioChunk,
        cancellation: &SessionCancellation,
        options: &RecognitionOptions,
    ) -> Result<Transcript, AsrError> {
        let on_gpu = self.shared.compute().is_accelerated();
        let first = self.recognize_once(session, audio, cancellation, options);
        let Err(error) = &first else {
            return first;
        };
        if !on_gpu || !is_worker_failure(error.code) {
            return first;
        }
        if error.code == "decode_failed" {
            // The worker survived a failed decode; retire it so its
            // replacement loads on the CPU.
            let mut slot = self
                .shared
                .worker
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            terminate_slot(&self.shared, &mut slot)?;
        }
        match self.recover(cancellation) {
            Ok(_) => self.recognize_once(session, audio, cancellation, options),
            Err(_) => first,
        }
    }

    fn recognize_once(
        &mut self,
        session: SessionId,
        audio: &AudioChunk,
        cancellation: &SessionCancellation,
        options: &RecognitionOptions,
    ) -> Result<Transcript, AsrError> {
        validate_audio(audio)?;
        self.shared.ensure_admission_open()?;
        if cancellation.is_cancelled() {
            return Err(AsrError::new(
                "cancelled",
                "ASR cancellation requested before dispatch",
            ));
        }

        let mut slot = match self.shared.worker.try_lock() {
            Ok(slot) => slot,
            Err(TryLockError::WouldBlock) => {
                return Err(AsrError::new(
                    "asr_busy",
                    "the resident CPU model is processing another request",
                ));
            }
            Err(TryLockError::Poisoned(_)) => {
                return Err(AsrError::new(
                    "resident_worker_unhealthy",
                    "the resident ASR worker owner is unavailable",
                ));
            }
        };

        self.shared.ensure_admission_open()?;

        if slot.active_stream.is_some() {
            return Err(AsrError::new(
                "asr_busy",
                "the resident CPU model has an active stream",
            ));
        }
        if self.shared.readiness() != ResidentReadiness::Ready {
            return Err(readiness_error(self.shared.readiness()));
        }
        if slot.client.is_none() {
            self.shared
                .set_readiness(ResidentReadiness::RestartRequired);
            return Err(readiness_error(ResidentReadiness::RestartRequired));
        }
        self.shared.set_readiness(ResidentReadiness::Active);
        let result = slot
            .client
            .as_mut()
            .ok_or_else(|| {
                AsrError::new(
                    "resident_worker_unavailable",
                    "the resident ASR worker is unavailable",
                )
            })?
            .decode(session.0, &audio.samples, cancellation, options);

        if let Err(error) = &result {
            self.shared.note_failure(error.code);
            if matches!(
                error.code,
                "cancelled"
                    | "decode_timeout"
                    | "resident_worker_io"
                    | "resident_worker_protocol"
                    | "resident_worker_unavailable"
                    | "resident_worker_termination_failed"
                    | "resident_worker_termination_timeout"
            ) {
                terminate_slot(&self.shared, &mut slot)?;
            } else {
                self.shared.set_ready_if_open();
            }
        } else {
            self.shared.set_ready_if_open();
        }
        result.map(|text| Transcript {
            text,
            is_final: true,
        })
    }
}

impl AsrEngine for ResidentWorkerEngine {
    fn capabilities(&self) -> AsrCapabilities {
        self.shared.capabilities.to_asr_capabilities()
    }

    fn open_stream(
        &mut self,
        session: SessionId,
        cancellation: &CancelToken,
    ) -> Result<Box<dyn AsrStream>, AsrError> {
        self.shared.ensure_admission_open()?;
        if !self.shared.capabilities.supports_streaming {
            return Err(AsrError::new(
                "streaming_model_unsupported",
                "the loaded resident ASR model does not support native streaming",
            ));
        }
        if cancellation.is_cancelled() {
            return Err(AsrError::new(
                "cancelled",
                "ASR stream admission was cancelled before dispatch",
            ));
        }
        let mut slot = self.shared.worker.try_lock().map_err(|error| match error {
            TryLockError::WouldBlock => AsrError::new(
                "asr_busy",
                "the resident CPU model is processing another request",
            ),
            TryLockError::Poisoned(_) => AsrError::new(
                "resident_worker_unhealthy",
                "the resident ASR worker owner is unavailable",
            ),
        })?;
        self.shared.ensure_admission_open()?;
        if slot.active_stream.is_some() {
            return Err(AsrError::new(
                "asr_busy",
                "the resident CPU model already has an active stream",
            ));
        }
        if self.shared.readiness() != ResidentReadiness::Ready {
            return Err(readiness_error(self.shared.readiness()));
        }
        if slot.client.is_none() {
            self.shared
                .set_readiness(ResidentReadiness::RestartRequired);
            return Err(readiness_error(ResidentReadiness::RestartRequired));
        }
        self.shared.set_readiness(ResidentReadiness::Active);
        let begin_result = slot
            .client
            .as_mut()
            .ok_or_else(|| {
                AsrError::new(
                    "resident_worker_unavailable",
                    "the resident ASR worker is unavailable",
                )
            })?
            .stream_begin(session.0, cancellation);
        let response = match begin_result {
            Ok(response) => response,
            Err(error) => {
                return match terminate_slot(&self.shared, &mut slot) {
                    Ok(()) => Err(error),
                    Err(cleanup) => Err(cleanup),
                };
            }
        };
        if response.request_id != session.0
            || response.status != STATUS_STREAM_STARTED
            || !response.payload.is_empty()
        {
            terminate_slot(&self.shared, &mut slot)?;
            return Err(AsrError::new(
                "resident_worker_protocol",
                "resident ASR worker rejected the stream-begin contract",
            ));
        }
        slot.active_stream = Some(session.0);
        drop(slot);
        let mut pending = VecDeque::new();
        pending.push_back(AsrEvent::Started { session });
        Ok(Box::new(ResidentStream {
            shared: Arc::clone(&self.shared),
            session,
            cancellation: cancellation.clone(),
            pending,
            next_seq: 0,
            last_revision: None,
            last_partial: None,
            total_samples: 0,
            finalized: false,
            closed: false,
        }))
    }
}

fn readiness_error(readiness: ResidentReadiness) -> AsrError {
    match readiness {
        ResidentReadiness::Ready => AsrError::new(
            "resident_worker_unavailable",
            "the resident ASR worker is unavailable",
        ),
        ResidentReadiness::Active => AsrError::new(
            "asr_busy",
            "the resident CPU model is processing another request",
        ),
        ResidentReadiness::Warming => AsrError::new(
            "resident_worker_warming",
            "the resident ASR worker is warming",
        ),
        ResidentReadiness::RestartRequired => AsrError::new(
            "resident_worker_restart_required",
            "the resident ASR worker requires explicit recovery",
        ),
        ResidentReadiness::FailedClosed => AsrError::new(
            "resident_worker_failed_closed",
            "the resident ASR worker failed closed",
        ),
    }
}

fn shutdown_timeout_error() -> AsrError {
    AsrError::new(
        "resident_worker_shutdown_timeout",
        "resident ASR worker shutdown exceeded its bound",
    )
}

fn terminate_slot(shared: &SharedWorker, slot: &mut WorkerSlot) -> Result<(), AsrError> {
    terminate_slot_until(shared, slot, Instant::now() + TERMINATION_TIMEOUT)
}

fn terminate_slot_until(
    shared: &SharedWorker,
    slot: &mut WorkerSlot,
    deadline: Instant,
) -> Result<(), AsrError> {
    slot.active_stream = None;
    shared.set_readiness(ResidentReadiness::RestartRequired);
    if let Some(worker) = slot.client.as_mut() {
        if let Err(error) = worker.terminate_until(deadline) {
            if let Some(worker) = slot.client.take() {
                slot.retiring.push_back(RetiringWorker::Client(worker));
            }
            return Err(error);
        }
        slot.client = None;
    }
    Ok(())
}

fn terminate_retiring_until(slot: &mut WorkerSlot, deadline: Instant) -> Result<(), AsrError> {
    terminate_retiring_queue_until(&mut slot.retiring, deadline)
}

fn terminate_retiring_queue_until(
    retiring: &mut VecDeque<RetiringWorker>,
    deadline: Instant,
) -> Result<(), AsrError> {
    while !retiring.is_empty() {
        if let Some(worker) = retiring.front_mut() {
            worker.terminate_until(deadline)?;
        }
        retiring.pop_front();
    }
    Ok(())
}

struct ResidentStream {
    shared: Arc<SharedWorker>,
    session: SessionId,
    cancellation: CancelToken,
    pending: VecDeque<AsrEvent>,
    next_seq: u64,
    last_revision: Option<i32>,
    last_partial: Option<String>,
    total_samples: usize,
    finalized: bool,
    closed: bool,
}

impl ResidentStream {
    fn operate(&mut self, operation: u8, samples: &[f32]) -> Result<WorkerResponse, AsrError> {
        if self.cancellation.is_cancelled() {
            self.abort_bounded()?;
            return Err(AsrError::new("cancelled", "ASR stream was cancelled"));
        }
        let mut slot = self.shared.worker.lock().map_err(|_| {
            AsrError::new(
                "resident_worker_unhealthy",
                "the resident ASR worker owner is unavailable",
            )
        })?;
        if slot.active_stream != Some(self.session.0) {
            self.closed = true;
            return Err(AsrError::new(
                "resident_stream_not_active",
                "the resident ASR stream no longer owns the worker",
            ));
        }
        let result = slot
            .client
            .as_mut()
            .ok_or_else(|| {
                AsrError::new(
                    "resident_worker_unavailable",
                    "the resident ASR worker is unavailable",
                )
            })?
            .stream_operation(operation, self.session.0, samples, &self.cancellation);
        if let Err(error) = &result {
            self.shared.note_failure(error.code);
            if matches!(
                error.code,
                "cancelled"
                    | "decode_timeout"
                    | "resident_worker_io"
                    | "resident_worker_protocol"
                    | "resident_worker_unavailable"
                    | "invalid_audio_sample"
            ) {
                terminate_slot(&self.shared, &mut slot)?;
                self.closed = true;
            }
        }
        result
    }

    fn consume_snapshot(
        &mut self,
        response: WorkerResponse,
        final_expected: bool,
    ) -> Result<(), AsrError> {
        let expected_status = if final_expected {
            STATUS_STREAM_FINAL
        } else {
            STATUS_STREAM_UPDATE
        };
        if response.request_id != self.session.0 || response.status != expected_status {
            self.abort_bounded()?;
            return Err(AsrError::new(
                "resident_worker_protocol",
                "resident ASR worker returned an invalid stream response",
            ));
        }
        let snapshot = match resident_protocol::decode_stream_snapshot(&response.payload) {
            Ok(snapshot) => snapshot,
            Err(_) => {
                self.abort_bounded()?;
                return Err(AsrError::new(
                    "resident_worker_protocol",
                    "resident ASR worker returned a malformed stream snapshot",
                ));
            }
        };
        if snapshot.revision < 0
            || snapshot.is_final != final_expected
            || self
                .last_revision
                .is_some_and(|previous| snapshot.revision < previous)
            || (snapshot.result_changed
                && self
                    .last_revision
                    .is_some_and(|previous| snapshot.revision <= previous))
        {
            self.abort_bounded()?;
            return Err(AsrError::new(
                "resident_worker_protocol",
                "resident ASR worker violated stream revision/finality ordering",
            ));
        }
        self.last_revision = Some(snapshot.revision);
        if final_expected {
            self.next_seq = self.next_seq.saturating_add(1);
            self.pending.push_back(AsrEvent::Final {
                seq: self.next_seq,
                transcript: Transcript {
                    text: snapshot.full,
                    is_final: true,
                },
                confidence: None,
                first_audio_offset_ms: None,
            });
            self.finalized = true;
            self.release_stream();
        } else if snapshot.result_changed {
            let display = format!("{}{}", snapshot.committed, snapshot.tentative);
            if !display.is_empty() && self.last_partial.as_deref() != Some(display.as_str()) {
                self.next_seq = self.next_seq.saturating_add(1);
                self.last_partial = Some(display.clone());
                self.pending.push_back(AsrEvent::Partial {
                    seq: self.next_seq,
                    transcript: Transcript {
                        text: display,
                        is_final: false,
                    },
                });
            }
        }
        Ok(())
    }

    fn release_stream(&mut self) {
        if let Ok(mut slot) = self.shared.worker.lock() {
            if slot.active_stream == Some(self.session.0) {
                slot.active_stream = None;
                self.shared.set_ready_if_open();
            }
        } else {
            self.shared
                .set_readiness(ResidentReadiness::RestartRequired);
        }
        self.closed = true;
    }

    fn abort_bounded(&mut self) -> Result<(), AsrError> {
        self.abort_until(Instant::now() + TERMINATION_TIMEOUT)
    }

    fn abort_until(&mut self, deadline: Instant) -> Result<(), AsrError> {
        if self.closed {
            return Ok(());
        }
        let mut slot = self.shared.worker.lock().map_err(|_| {
            AsrError::new(
                "resident_worker_unhealthy",
                "the resident ASR worker owner is unavailable",
            )
        })?;
        if slot.active_stream == Some(self.session.0) {
            terminate_slot_until(&self.shared, &mut slot, deadline)?;
        }
        self.closed = true;
        Ok(())
    }
}

impl Drop for ResidentStream {
    fn drop(&mut self) {
        if self.closed {
            return;
        }
        if let Ok(mut slot) = self.shared.worker.lock() {
            if slot.active_stream == Some(self.session.0) {
                let deadline = Instant::now() + TERMINATION_TIMEOUT;
                let reset_ok = slot
                    .client
                    .as_mut()
                    .and_then(|client| {
                        client
                            .stream_operation_until(
                                OP_STREAM_RESET,
                                self.session.0,
                                &[],
                                &SessionCancellation::new(),
                                deadline,
                            )
                            .ok()
                    })
                    .is_some_and(|response| {
                        response.status == STATUS_STREAM_RESET
                            && response.request_id == self.session.0
                            && response.payload.is_empty()
                    });
                if reset_ok {
                    slot.active_stream = None;
                    self.shared.set_ready_if_open();
                } else {
                    let _ = terminate_slot_until(&self.shared, &mut slot, deadline);
                }
            }
        } else {
            self.shared
                .set_readiness(ResidentReadiness::RestartRequired);
        }
    }
}

impl AsrStream for ResidentStream {
    fn session(&self) -> SessionId {
        self.session
    }

    fn cancellation(&self) -> CancelToken {
        self.cancellation.clone()
    }

    fn push_audio(&mut self, chunk: &AudioChunk) -> Result<(), AsrError> {
        if self.finalized || self.closed {
            return Err(AsrError::new(
                "stream_finalized",
                "cannot push audio after the resident stream ended",
            ));
        }
        validate_audio(chunk)?;
        self.total_samples = self
            .total_samples
            .checked_add(chunk.samples.len())
            .ok_or_else(|| AsrError::new("invalid_audio_length", "stream audio overflowed"))?;
        if self.total_samples > MAX_SAMPLES {
            return Err(AsrError::new(
                "invalid_audio_length",
                format!(
                    "resident ASR stream exceeds the {} second bound",
                    narrivy_types::MAX_CAPTURE_SECONDS
                ),
            ));
        }
        let response = self.operate(OP_STREAM_FEED, &chunk.samples)?;
        self.consume_snapshot(response, false)
    }

    fn poll_event(&mut self) -> Option<AsrEvent> {
        self.pending.pop_front()
    }

    fn finalize(&mut self) -> Result<(), AsrError> {
        if self.finalized {
            return Ok(());
        }
        if self.closed {
            return Err(AsrError::new(
                "resident_stream_not_active",
                "the resident ASR stream is not active",
            ));
        }
        if self.total_samples == 0 {
            return Err(AsrError::new("no_audio", "no audio was fed to the stream"));
        }
        let response = self.operate(OP_STREAM_FINALIZE, &[])?;
        self.consume_snapshot(response, true)
    }

    fn finish(&mut self, timeout: Duration) -> Result<(), AsrError> {
        if self.closed || self.finalized {
            return Ok(());
        }
        if !self.cancellation.is_cancelled() {
            return Err(AsrError::new(
                "stream_not_terminal",
                "resident ASR stream has not finalized or been cancelled",
            ));
        }
        let deadline = Instant::now().checked_add(timeout).ok_or_else(|| {
            AsrError::new(
                "invalid_timeout",
                "resident ASR finish timeout exceeds the platform clock range",
            )
        })?;
        self.abort_until(deadline)
    }
}

#[cfg(test)]
fn validate_worker_executable(path: &Path) -> Result<(), AsrError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| {
        AsrError::new(
            "resident_worker_missing",
            "the resident ASR worker executable is unavailable",
        )
    })?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.permissions().mode() & 0o111 == 0
    {
        return Err(AsrError::new(
            "resident_worker_invalid",
            "the resident ASR worker must be a non-symlink executable file",
        ));
    }
    Ok(())
}

fn validate_audio(audio: &AudioChunk) -> Result<(), AsrError> {
    if audio.sample_rate_hz != crate::TARGET_SAMPLE_RATE_HZ || audio.channels != 1 {
        return Err(AsrError::new(
            "invalid_audio_layout",
            "resident ASR requires 16 kHz mono f32 PCM",
        ));
    }
    if audio.samples.is_empty() || audio.samples.len() > MAX_SAMPLES {
        return Err(AsrError::new(
            "invalid_audio_length",
            "resident ASR audio length is outside the bounded contract",
        ));
    }
    if audio.samples.iter().any(|sample| !sample.is_finite()) {
        return Err(AsrError::new(
            "invalid_audio_sample",
            "resident ASR rejects non-finite PCM",
        ));
    }
    Ok(())
}

enum RetiringWorker {
    Client(WorkerClient),
    Child(Child),
}

impl RetiringWorker {
    fn terminate_until(&mut self, deadline: Instant) -> Result<(), AsrError> {
        match self {
            Self::Client(worker) => worker.terminate_until(deadline),
            Self::Child(child) => terminate(child, deadline),
        }
    }

    #[cfg(test)]
    fn pid(&self) -> u32 {
        match self {
            Self::Client(worker) => worker.child.id(),
            Self::Child(child) => child.id(),
        }
    }
}

struct WorkerStartupFailure {
    error: AsrError,
    owner: Option<Box<RetiringWorker>>,
}

impl WorkerStartupFailure {
    fn without_owner(error: AsrError) -> Self {
        Self { error, owner: None }
    }

    fn from_client_until(mut client: WorkerClient, primary: AsrError, deadline: Instant) -> Self {
        match client.terminate_until(deadline) {
            Ok(()) => Self::without_owner(primary),
            Err(error) => Self {
                error,
                owner: Some(Box::new(RetiringWorker::Client(client))),
            },
        }
    }

    fn from_child_until(mut child: Child, primary: AsrError, deadline: Instant) -> Self {
        match terminate(&mut child, deadline) {
            Ok(()) => Self::without_owner(primary),
            Err(error) => Self {
                error,
                owner: Some(Box::new(RetiringWorker::Child(child))),
            },
        }
    }
}

/// Process-wide fallback owner for resident ownership that a dropped engine
/// could not confirm reaped.
///
/// A `Drop` implementation cannot report failure, so its only honest options
/// are to keep every live child/thread handle under an explicit owner and
/// leave a sticky unconfirmed count that the next bounded cleanup must clear.
/// Detaching a handle (dropping it) would hide an unreaped process behind the
/// parent-death signal, which is a containment net, not a cleanup proof.
#[derive(Default)]
struct InitialRetiringState {
    queue: VecDeque<RetiringWorker>,
    /// Spawn-owner threads whose retirement could not be confirmed before a
    /// drop deadline expired. Each handle is retained (not detached) so a
    /// later bounded cleanup proves thread completion before joining.
    owner_threads: Vec<std::thread::JoinHandle<()>>,
    /// Retained hand-off coordinators whose engine already dropped. Each
    /// published its own join handle here instead of detaching it, so a bounded
    /// cleanup can prove completion before joining it.
    coordinator_threads: Vec<std::thread::JoinHandle<()>>,
    /// Shared abandoned-child publication registries handed off with their
    /// spawning owner. A failed-reap child published *after* the drop still
    /// lands here, so each registry stays owned until every handed-off owner
    /// thread has retired and the queue was observed empty.
    registries: Vec<Arc<Mutex<VecDeque<Child>>>>,
    /// Count of ownership hand-offs that have not yet been confirmed. It only
    /// returns to zero once every fallback child is reaped, every handed-off
    /// owner or coordinator thread has been observed finished and joined, every
    /// publication registry is closed, every coordinator retirement has
    /// registered its handle and no hand-off is still pending.
    unconfirmed: u64,
    cleanup_in_progress: bool,
}

fn initial_retiring() -> &'static Mutex<InitialRetiringState> {
    static RETIRING: OnceLock<Mutex<InitialRetiringState>> = OnceLock::new();
    RETIRING.get_or_init(|| Mutex::new(InitialRetiringState::default()))
}

#[cfg(test)]
fn initial_cleanup_hook() -> &'static Mutex<Option<InitialCleanupHook>> {
    static HOOK: OnceLock<Mutex<Option<InitialCleanupHook>>> = OnceLock::new();
    HOOK.get_or_init(|| Mutex::new(None))
}

fn terminate_initial_retiring_until(
    deadline: Instant,
    cancellation: Option<&SessionCancellation>,
) -> Result<(), AsrError> {
    loop {
        check_startup_control(deadline, cancellation)?;
        let mut owned = {
            let mut state = match initial_retiring().try_lock() {
                Ok(state) => state,
                Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
                Err(TryLockError::WouldBlock) => {
                    std::thread::sleep(
                        POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())),
                    );
                    continue;
                }
            };
            if state.cleanup_in_progress {
                drop(state);
                std::thread::sleep(
                    POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())),
                );
                continue;
            }
            // Retire handed-off owner threads that really finished, and keep
            // the still-running handles under this owner. `is_finished`
            // observes that the owner's main function returned — which is what
            // stops further spawn and publication work — and the join then
            // only waits for that thread's native teardown.
            let handed_off = std::mem::take(&mut state.owner_threads);
            let mut still_running = Vec::new();
            for handle in handed_off {
                if handle.is_finished() {
                    let _ = handle.join();
                } else {
                    still_running.push(handle);
                }
            }
            state.owner_threads = still_running;

            // Hand-off coordinators publish their own handle once their engine
            // dropped. Joining follows the same rule as owner threads: only
            // after the thread's main function returned, so the join never
            // waits on live ownership work.
            let handed_off_coordinators = std::mem::take(&mut state.coordinator_threads);
            let mut still_retiring = Vec::new();
            for handle in handed_off_coordinators {
                if handle.is_finished() {
                    let _ = handle.join();
                } else {
                    still_retiring.push(handle);
                }
            }
            state.coordinator_threads = still_retiring;

            // Observe the shared publication registries. A still-running owner
            // thread may publish a failed-reap child after the drop snapshot,
            // so a registry keeps its place until every handed-off owner has
            // retired; only then is a drained, empty registry released.
            if !state.registries.is_empty() {
                let mut still_open = Vec::new();
                for registry in std::mem::take(&mut state.registries) {
                    let keep = {
                        let locked = match registry.try_lock() {
                            Ok(children) => Some(children),
                            Err(TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
                            Err(TryLockError::WouldBlock) => None,
                        };
                        match locked {
                            Some(mut children) => {
                                while let Some(child) = children.pop_front() {
                                    state.queue.push_back(RetiringWorker::Child(child));
                                }
                                !state.owner_threads.is_empty()
                            }
                            None => true,
                        }
                    };
                    if keep {
                        still_open.push(registry);
                    }
                }
                state.registries = still_open;
            }

            if state.queue.is_empty()
                && state.owner_threads.is_empty()
                && state.coordinator_threads.is_empty()
                && state.registries.is_empty()
                && PENDING_HANDOFFS.load(Ordering::Acquire) == 0
                && COORDINATOR_RETIREMENTS.load(Ordering::Acquire) == 0
            {
                state.unconfirmed = 0;
                return Ok(());
            }
            if state.queue.is_empty() {
                // Only unretired owner/coordinator threads, open publication
                // registries or an in-flight coordinator retirement remain.
                // Wait without holding the registry lock; the loop re-checks
                // this deadline each pass and returns a typed error when the
                // ownership stays unconfirmed.
                drop(state);
                let wait = POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now()));
                if wait.is_zero() {
                    return Err(AsrError::new(
                        "resident_worker_reap_unconfirmed",
                        "resident ASR ownership hand-off was not confirmed before its bound",
                    ));
                }
                std::thread::sleep(wait);
                continue;
            }
            state.cleanup_in_progress = true;
            std::mem::take(&mut state.queue)
        };

        #[cfg(test)]
        if let Some(hook) = initial_cleanup_hook()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            let _ = hook.entered.send(());
            let _ = hook.release.recv();
        }

        let cleanup = terminate_retiring_queue_until(&mut owned, deadline);
        {
            let mut state = initial_retiring()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !owned.is_empty() {
                owned.append(&mut state.queue);
                state.queue = owned;
            }
            state.cleanup_in_progress = false;
        }
        cleanup?;
    }
}

struct WorkerClient {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
    capabilities: WorkerCapabilities,
    #[cfg(test)]
    termination_failures_remaining: usize,
}

impl WorkerClient {
    /// Start a worker. A worker that was allowed a GPU and did not reach
    /// readiness (for any reason but the caller cancelling) withdraws the
    /// GPU, so the next worker is strict CPU.
    fn spawn_until(
        config: &WorkerConfig,
        operation_deadline: Instant,
        cleanup_deadline: Instant,
        cancellation: Option<&SessionCancellation>,
        anchor: &SpawnAnchor,
    ) -> Result<Self, WorkerStartupFailure> {
        let gpu_allowed = config.compute.allows_gpu();
        let spawned = Self::spawn_requested_until(
            config,
            operation_deadline,
            cleanup_deadline,
            cancellation,
            anchor,
        );
        if gpu_allowed
            && let Err(failure) = &spawned
            && failure.error.code != "cancelled"
        {
            config.compute.withdraw_gpu();
        }
        spawned
    }

    fn spawn_requested_until(
        config: &WorkerConfig,
        operation_deadline: Instant,
        cleanup_deadline: Instant,
        cancellation: Option<&SessionCancellation>,
        anchor: &SpawnAnchor,
    ) -> Result<Self, WorkerStartupFailure> {
        check_startup_control(operation_deadline, cancellation)
            .map_err(WorkerStartupFailure::without_owner)?;
        let fallback_cancellation = SessionCancellation::new();
        let launch_cancellation = cancellation.unwrap_or(&fallback_cancellation);
        let mut child = anchor
            .spawn(config, launch_cancellation, operation_deadline)
            .map_err(WorkerStartupFailure::without_owner)?;
        let Some(stdin) = child.stdin.take() else {
            let spawn_error = AsrError::new(
                "resident_worker_spawn_failed",
                "resident ASR worker stdin was unavailable",
            );
            return Err(WorkerStartupFailure::from_child_until(
                child,
                spawn_error,
                cleanup_deadline,
            ));
        };
        let Some(stdout) = child.stdout.take() else {
            let spawn_error = AsrError::new(
                "resident_worker_spawn_failed",
                "resident ASR worker stdout was unavailable",
            );
            return Err(WorkerStartupFailure::from_child_until(
                child,
                spawn_error,
                cleanup_deadline,
            ));
        };
        let mut client = Self {
            child,
            stdin,
            stdout,
            capabilities: WorkerCapabilities::default(),
            #[cfg(test)]
            termination_failures_remaining: STARTUP_TERMINATION_FAILURES.swap(0, Ordering::AcqRel),
        };
        if let Err(error) =
            set_nonblocking(&client.stdin).and_then(|()| set_nonblocking(&client.stdout))
        {
            return Err(WorkerStartupFailure::from_client_until(
                client,
                error,
                cleanup_deadline,
            ));
        }
        let response = match client.read_response(operation_deadline, cancellation) {
            Ok(response) => response,
            Err(error) => {
                return Err(WorkerStartupFailure::from_client_until(
                    client,
                    startup_control_error(error),
                    cleanup_deadline,
                ));
            }
        };
        if response.request_id != 0 {
            let protocol_error = AsrError::new(
                "resident_worker_protocol",
                "resident ASR worker sent an invalid readiness frame",
            );
            return Err(WorkerStartupFailure::from_client_until(
                client,
                protocol_error,
                cleanup_deadline,
            ));
        }
        if response.status != STATUS_READY {
            let code = match response.status {
                STATUS_MODEL_LOAD_FAILED => "resident_model_load_failed",
                STATUS_MODEL_BINDING_FAILED => "resident_model_binding_failed",
                STATUS_BACKEND_MISMATCH => "resident_backend_mismatch",
                STATUS_SAMPLE_RATE_MISMATCH => "resident_sample_rate_mismatch",
                STATUS_CANCELLATION_UNSUPPORTED => "resident_cancellation_unsupported",
                STATUS_MODEL_FORMAT_UNSUPPORTED => "resident_model_format_unsupported",
                STATUS_VERSION_MISMATCH => "resident_version_mismatch",
                STATUS_OUT_OF_MEMORY => "resident_out_of_memory",
                STATUS_NATIVE_BACKEND_FAILED => "resident_native_backend_failed",
                STATUS_NATIVE_STARTUP_OTHER => "resident_native_startup_failed",
                _ => "resident_worker_protocol",
            };
            let startup_error =
                AsrError::new(code, "resident ASR worker failed its startup contract");
            return Err(WorkerStartupFailure::from_client_until(
                client,
                startup_error,
                cleanup_deadline,
            ));
        }
        let capabilities = match resident_protocol::decode_capabilities(&response.payload) {
            Ok(capabilities) => capabilities,
            Err(_) => {
                let protocol_error = AsrError::new(
                    "resident_worker_protocol",
                    "resident ASR worker sent invalid model capabilities",
                );
                return Err(WorkerStartupFailure::from_client_until(
                    client,
                    protocol_error,
                    cleanup_deadline,
                ));
            }
        };
        if let Err(error) = check_startup_control(operation_deadline, cancellation) {
            return Err(WorkerStartupFailure::from_client_until(
                client,
                error,
                cleanup_deadline,
            ));
        }
        client.capabilities = capabilities;
        Ok(client)
    }

    fn decode(
        &mut self,
        request_id: u128,
        samples: &[f32],
        cancellation: &SessionCancellation,
        options: &RecognitionOptions,
    ) -> Result<String, AsrError> {
        match try_wait_contained(&mut self.child, Instant::now() + TERMINATION_TIMEOUT) {
            Ok(Some(_)) => {
                return Err(AsrError::new(
                    "resident_worker_unavailable",
                    "resident ASR worker exited before the request",
                ));
            }
            Ok(None) => {}
            Err(error) => return Err(resident_process_error(error)),
        }

        let sample_count = u32::try_from(samples.len()).map_err(|_| {
            AsrError::new(
                "invalid_audio_length",
                "resident ASR audio length is outside the bounded contract",
            )
        })?;
        let request_header = resident_protocol::encode_request(RequestHeader {
            operation: OP_DECODE,
            request_id,
            sample_count,
        });

        let deadline = Instant::now() + decode_budget(samples.len());
        self.write_all_bounded(&request_header, deadline, Some(cancellation))?;
        let options_frame =
            resident_protocol::encode_recognition_options_frame(options).map_err(|_| {
                AsrError::new(
                    "resident_worker_protocol",
                    "resident ASR recognition options exceeded the bounded contract",
                )
            })?;
        self.write_all_bounded(&options_frame, deadline, Some(cancellation))?;
        self.write_pcm_bounded(samples, deadline, cancellation)?;
        let response = self.read_response(deadline, Some(cancellation))?;
        if response.request_id != request_id {
            return Err(AsrError::new(
                "resident_worker_protocol",
                "resident ASR worker response identity mismatch",
            ));
        }
        match response.status {
            STATUS_OK => String::from_utf8(response.payload).map_err(|_| {
                AsrError::new(
                    "resident_worker_protocol",
                    "resident ASR worker returned invalid UTF-8",
                )
            }),
            STATUS_INVALID_AUDIO => Err(AsrError::new(
                "invalid_audio_sample",
                "resident ASR worker rejected the PCM payload",
            )),
            STATUS_DECODE_FAILED => Err(AsrError::new(
                "decode_failed",
                "resident transcribe.cpp decode failed",
            )),
            STATUS_UNSUPPORTED_REQUEST => Err(AsrError::new(
                narrivy_asr_api::UNSUPPORTED_REQUEST_APPLY,
                "the resident ASR worker could not apply the requested settings",
            )),
            STATUS_INVALID_REQUEST => Err(AsrError::new(
                "resident_worker_protocol",
                "resident ASR worker rejected a malformed recognition request",
            )),
            _ => Err(AsrError::new(
                "resident_worker_protocol",
                "resident ASR worker returned an invalid status",
            )),
        }
    }

    fn stream_begin(
        &mut self,
        request_id: u128,
        cancellation: &SessionCancellation,
    ) -> Result<WorkerResponse, AsrError> {
        let deadline = Instant::now() + STREAM_BEGIN_TIMEOUT;
        let header = resident_protocol::encode_request(RequestHeader {
            operation: OP_STREAM_BEGIN,
            request_id,
            sample_count: 0,
        });
        self.write_all_bounded(&header, deadline, Some(cancellation))?;
        self.read_response(deadline, Some(cancellation))
    }

    fn stream_operation(
        &mut self,
        operation: u8,
        request_id: u128,
        samples: &[f32],
        cancellation: &SessionCancellation,
    ) -> Result<WorkerResponse, AsrError> {
        self.stream_operation_until(
            operation,
            request_id,
            samples,
            cancellation,
            Instant::now() + DECODE_TIMEOUT,
        )
    }

    fn stream_operation_until(
        &mut self,
        operation: u8,
        request_id: u128,
        samples: &[f32],
        cancellation: &SessionCancellation,
        deadline: Instant,
    ) -> Result<WorkerResponse, AsrError> {
        let sample_count = u32::try_from(samples.len()).map_err(|_| {
            AsrError::new(
                "invalid_audio_length",
                "resident ASR stream chunk is outside the bounded contract",
            )
        })?;
        let header = resident_protocol::encode_request(RequestHeader {
            operation,
            request_id,
            sample_count,
        });
        self.write_all_bounded(&header, deadline, Some(cancellation))?;
        if operation == OP_STREAM_FEED {
            self.write_pcm_bounded(samples, deadline, cancellation)?;
        }
        let response = self.read_response(deadline, Some(cancellation))?;
        if response.status == STATUS_INVALID_AUDIO {
            return Err(AsrError::new(
                "invalid_audio_sample",
                "resident ASR worker rejected the streaming PCM payload",
            ));
        }
        Ok(response)
    }

    fn write_pcm_bounded(
        &mut self,
        samples: &[f32],
        deadline: Instant,
        cancellation: &SessionCancellation,
    ) -> Result<(), AsrError> {
        let mut transfer = [0_u8; PCM_TRANSFER_BYTES];
        for chunk in samples.chunks(PCM_TRANSFER_SAMPLES) {
            for (sample, encoded) in chunk.iter().zip(transfer.chunks_exact_mut(4)) {
                encoded.copy_from_slice(&sample.to_le_bytes());
            }
            self.write_all_bounded(
                &transfer[..size_of_val(chunk)],
                deadline,
                Some(cancellation),
            )?;
        }
        Ok(())
    }

    fn write_all_bounded(
        &mut self,
        bytes: &[u8],
        deadline: Instant,
        cancellation: Option<&SessionCancellation>,
    ) -> Result<(), AsrError> {
        let mut offset = 0;
        while offset < bytes.len() {
            check_control(deadline, cancellation)?;
            match self.stdin.write(&bytes[offset..]) {
                Ok(0) => return Err(worker_io_error()),
                Ok(count) => offset += count,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(POLL_INTERVAL);
                }
                Err(_) => return Err(worker_io_error()),
            }
        }
        Ok(())
    }

    fn read_response(
        &mut self,
        deadline: Instant,
        cancellation: Option<&SessionCancellation>,
    ) -> Result<WorkerResponse, AsrError> {
        let mut header = [0_u8; HEADER_LEN];
        self.read_exact_bounded(&mut header, deadline, cancellation)?;
        let decoded = resident_protocol::decode_response(header).map_err(|_| {
            AsrError::new(
                "resident_worker_protocol",
                "resident ASR worker returned a malformed frame",
            )
        })?;
        let payload_len = usize::try_from(decoded.payload_len).map_err(|_| {
            AsrError::new(
                "resident_worker_protocol",
                "resident ASR worker response length overflowed",
            )
        })?;
        let mut payload = vec![0_u8; payload_len];
        self.read_exact_bounded(&mut payload, deadline, cancellation)?;
        Ok(WorkerResponse {
            status: decoded.status,
            request_id: decoded.request_id,
            payload,
        })
    }

    fn read_exact_bounded(
        &mut self,
        bytes: &mut [u8],
        deadline: Instant,
        cancellation: Option<&SessionCancellation>,
    ) -> Result<(), AsrError> {
        let mut offset = 0;
        while offset < bytes.len() {
            check_control(deadline, cancellation)?;
            match self.stdout.read(&mut bytes[offset..]) {
                Ok(0) => return Err(worker_io_error()),
                Ok(count) => offset += count,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(POLL_INTERVAL);
                }
                Err(_) => return Err(worker_io_error()),
            }
        }
        Ok(())
    }

    fn shutdown_until(&mut self, deadline: Instant) -> Result<(), AsrError> {
        match try_wait_contained(&mut self.child, deadline) {
            Ok(Some(_)) => return Ok(()),
            Ok(None) => {}
            Err(error) => return Err(resident_process_error(error)),
        }
        let graceful_deadline = deadline.min(Instant::now() + GRACEFUL_SHUTDOWN_TIMEOUT);
        let shutdown = resident_protocol::encode_request(RequestHeader {
            operation: OP_SHUTDOWN,
            request_id: 0,
            sample_count: 0,
        });
        if self
            .write_all_bounded(&shutdown, graceful_deadline, None)
            .is_ok()
            && wait_reaped(&mut self.child, graceful_deadline).is_ok()
        {
            Ok(())
        } else {
            self.terminate_until(deadline)
        }
    }

    fn terminate_bounded(&mut self) -> Result<(), AsrError> {
        self.terminate_until(Instant::now() + TERMINATION_TIMEOUT)
    }

    fn terminate_until(&mut self, deadline: Instant) -> Result<(), AsrError> {
        #[cfg(test)]
        if self.termination_failures_remaining > 0 {
            self.termination_failures_remaining -= 1;
            return Err(AsrError::new(
                "resident_worker_termination_timeout",
                "resident ASR worker did not terminate within its cleanup bound",
            ));
        }
        terminate(&mut self.child, deadline)
    }
}

fn spawn_worker_process(
    config: &WorkerConfig,
    cancellation: &SessionCancellation,
    deadline: Instant,
) -> Result<Child, AsrError> {
    match (&config.executable, &config.model) {
        (WorkerExecutable::DigestBound(executable), WorkerModel::DigestBound(model)) => {
            let mut command = executable
                .command_with_control(HelperEnvironment::ResidentAsr, cancellation, deadline)
                .map_err(resident_launch_artifact_error)?;
            command
                .args(config.leading_args.iter())
                .arg(ComputeRequest::ARGUMENT)
                .arg(config.compute.request().as_argument())
                .arg("--model-sha256")
                .arg(hex_digest(model.sha256()))
                .arg("--model-size")
                .arg(model.size().to_string())
                .arg("--model");
            command
                .inherited_read_only_arg(model)
                .map_err(model_artifact_error)?
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
            command
                .spawn_with_control(cancellation, deadline)
                .map_err(|error| match error {
                    ArtifactLaunchError::Artifact(error) => resident_launch_artifact_error(error),
                    ArtifactLaunchError::Data(error) => model_artifact_error(error),
                    ArtifactLaunchError::Spawn(_) => AsrError::new(
                        "resident_worker_spawn_failed",
                        "unable to start the resident ASR worker",
                    ),
                })
        }
        #[cfg(test)]
        (WorkerExecutable::Controlled(path), WorkerModel::Controlled(model)) => {
            check_startup_control(deadline, Some(cancellation))?;
            let mut command = Command::new(path);
            command.env_clear().current_dir("/").process_group(0);
            // Only a test that opts into GPU selection sees the argument.
            if config.compute.allows_gpu() {
                command
                    .arg(ComputeRequest::ARGUMENT)
                    .arg(config.compute.request().as_argument());
            }
            command
                .arg("--model")
                .arg(model)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
            // Controlled test workers must reproduce the production kernel
            // lifetime relationship, otherwise an ownership regression cannot
            // be observed from an engine test.
            narrivy_process_linux::install_resident_worker_lifetime_hardening(&mut command);
            check_startup_control(deadline, Some(cancellation))?;
            command.spawn().map_err(|_| {
                AsrError::new(
                    "resident_worker_spawn_failed",
                    "unable to start the controlled resident ASR worker",
                )
            })
        }
        #[cfg(test)]
        _ => Err(AsrError::new(
            "resident_worker_configuration_invalid",
            "resident ASR worker and model bindings are incompatible",
        )),
    }
}

fn hex_digest(digest: [u8; 32]) -> String {
    use std::fmt::Write as _;

    let mut output = String::with_capacity(64);
    for byte in digest {
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

struct WorkerResponse {
    status: u8,
    request_id: u128,
    payload: Vec<u8>,
}

fn check_control(
    deadline: Instant,
    cancellation: Option<&SessionCancellation>,
) -> Result<(), AsrError> {
    if cancellation.is_some_and(SessionCancellation::is_cancelled) {
        return Err(AsrError::new("cancelled", "ASR cancellation requested"));
    }
    if Instant::now() >= deadline {
        return Err(AsrError::new(
            "decode_timeout",
            "resident transcribe.cpp exceeded its decode bound",
        ));
    }
    Ok(())
}

fn check_startup_control(
    deadline: Instant,
    cancellation: Option<&SessionCancellation>,
) -> Result<(), AsrError> {
    check_control(deadline, cancellation).map_err(startup_control_error)
}

fn startup_operation_deadline(cleanup_deadline: Instant) -> Result<Instant, AsrError> {
    let operation_deadline = cleanup_deadline
        .checked_sub(TERMINATION_TIMEOUT)
        .ok_or_else(startup_timeout_error)?;
    if Instant::now() >= operation_deadline {
        return Err(startup_timeout_error());
    }
    Ok(operation_deadline)
}

fn startup_timeout_error() -> AsrError {
    AsrError::new(
        "resident_worker_startup_timeout",
        "resident ASR worker exceeded its startup bound",
    )
}

fn startup_control_error(error: AsrError) -> AsrError {
    if error.code == "decode_timeout" {
        startup_timeout_error()
    } else {
        error
    }
}

fn worker_io_error() -> AsrError {
    AsrError::new(
        "resident_worker_io",
        "resident ASR worker pipe closed unexpectedly",
    )
}

fn set_nonblocking(fd: &impl std::os::fd::AsFd) -> Result<(), AsrError> {
    let current = rustix::fs::fcntl_getfl(fd).map_err(|_| worker_io_error())?;
    rustix::fs::fcntl_setfl(fd, current | rustix::fs::OFlags::NONBLOCK)
        .map_err(|_| worker_io_error())
}

fn terminate(child: &mut Child, deadline: Instant) -> Result<(), AsrError> {
    terminate_group_and_reap(child, deadline).map_err(resident_process_error)
}

fn wait_reaped(child: &mut Child, deadline: Instant) -> Result<(), AsrError> {
    loop {
        match try_wait_contained(child, deadline).map_err(resident_process_error)? {
            Some(_) => return Ok(()),
            None if Instant::now() < deadline => std::thread::sleep(POLL_INTERVAL),
            None => return Err(resident_process_error(ProcessError::TerminationTimeout)),
        }
    }
}

fn resident_process_error(error: ProcessError) -> AsrError {
    let code = match error {
        ProcessError::Wait(_) => "resident_worker_wait_failed",
        ProcessError::Signal(_) => "resident_worker_termination_failed",
        ProcessError::UnexpectedDescendant => "resident_worker_unexpected_descendant",
        ProcessError::TerminationTimeout => "resident_worker_termination_timeout",
    };
    AsrError::new(code, "resident ASR worker lifecycle failed closed")
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::PermissionsExt;
    use std::sync::mpsc;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn resident_test_guard() -> std::sync::MutexGuard<'static, ()> {
        static GUARD: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        GUARD
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn as_test_result<T>(result: Result<T, AsrError>) -> Result<T, std::io::Error> {
        result.map_err(|error| std::io::Error::other(error.code))
    }

    fn expected_asr_error<T>(result: Result<T, AsrError>) -> Result<AsrError, std::io::Error> {
        result.map_or_else(Ok, |_| Err(std::io::Error::other("expected ASR error")))
    }

    fn fake_model(dir: &Path) -> Result<PathBuf, std::io::Error> {
        let path = dir.join("model.gguf");
        std::fs::write(&path, b"GGUF\x03\0\0\0")?;
        Ok(path)
    }

    fn file_digest(path: &Path) -> Result<(String, u64), std::io::Error> {
        let bytes = std::fs::read(path)?;
        let size = u64::try_from(bytes.len()).map_err(std::io::Error::other)?;
        let digest: [u8; 32] = Sha256::digest(bytes).into();
        Ok((hex_digest(digest), size))
    }

    #[test]
    fn resident_rejects_written_settings_it_cannot_apply() -> TestResult {
        let output_language = AsrRequest {
            output: narrivy_asr_api::WrittenOutput {
                language: Some(as_test_result(narrivy_asr_api::AsrTag::parse("en"))?),
                script: None,
            },
            ..AsrRequest::default()
        };
        assert_eq!(
            expected_asr_error(reject_unapplied_written_settings(&output_language))?.code,
            UNSUPPORTED_OUTPUT_LANGUAGE
        );

        let script = AsrRequest {
            output: narrivy_asr_api::WrittenOutput {
                language: None,
                script: Some(as_test_result(narrivy_asr_api::AsrTag::parse("Latn"))?),
            },
            ..AsrRequest::default()
        };
        assert_eq!(
            expected_asr_error(reject_unapplied_written_settings(&script))?.code,
            UNSUPPORTED_SCRIPT
        );

        let locale = AsrRequest {
            locale: Some(as_test_result(narrivy_asr_api::AsrTag::parse("en-US"))?),
            ..AsrRequest::default()
        };
        assert_eq!(
            expected_asr_error(reject_unapplied_written_settings(&locale))?.code,
            UNSUPPORTED_LOCALE
        );
        Ok(())
    }

    fn fake_worker(dir: &Path, delay_ms: u64) -> Result<PathBuf, std::io::Error> {
        fake_worker_with_capability(dir, delay_ms, false)
    }

    fn fake_worker_with_capability(
        dir: &Path,
        delay_ms: u64,
        supports_streaming: bool,
    ) -> Result<PathBuf, std::io::Error> {
        fake_worker_with_capability_flag(dir, delay_ms, u8::from(supports_streaming))
    }

    fn fake_worker_with_capability_flag(
        dir: &Path,
        delay_ms: u64,
        capability_flag: u8,
    ) -> Result<PathBuf, std::io::Error> {
        fake_worker_with_behavior(dir, delay_ms, capability_flag, false)
    }

    fn fake_worker_with_behavior(
        dir: &Path,
        delay_ms: u64,
        capability_flag: u8,
        malformed_snapshot: bool,
    ) -> Result<PathBuf, std::io::Error> {
        let path = dir.join("resident-worker.py");
        let malformed_flag = u8::from(malformed_snapshot);
        let source = format!(
            r#"#!/usr/bin/env python3
import os, struct, sys, time
MAGIC=b'NRAS'; VERSION=5
def response(status, request_id, payload=b''):
    sys.stdout.buffer.write(MAGIC + struct.pack('<HBB', VERSION, status, 0) + request_id.to_bytes(16, 'little') + struct.pack('<I', len(payload)) + payload)
    sys.stdout.buffer.flush()
def snapshot(revision, final, full, committed, tentative):
    flags=1 | (2 if final else 0) | 4 | (0 if final else 8)
    return struct.pack('<iB3xqqqIII', revision, flags, 0, 0, 0, len(full), len(committed), len(tentative)) + full + committed + tentative
response(0, 0, bytes([{capability_flag}, 0, 0, 0]))
active=None; revision=0
while True:
    header=sys.stdin.buffer.read(28)
    if not header: break
    if len(header) != 28 or header[:4] != MAGIC: sys.exit(3)
    op=header[6]; request_id=int.from_bytes(header[8:24], 'little'); count=struct.unpack('<I', header[24:28])[0]
    if op == 2: break
    if op == 1:
        options_len_raw=sys.stdin.buffer.read(4)
        if len(options_len_raw) != 4: sys.exit(4)
        options_len=struct.unpack('<I', options_len_raw)[0]
        options=sys.stdin.buffer.read(options_len)
        if len(options) != options_len: sys.exit(4)
        payload=sys.stdin.buffer.read(count * 4)
        if len(payload) != count * 4: sys.exit(4)
        time.sleep({delay_ms} / 1000.0)
        response(1, request_id, b'controlled resident text')
    elif op == 3:
        if not {capability_flag} & 1: response(17, request_id)
        elif active is not None: sys.exit(5)
        else: active=request_id; revision=0; response(13, request_id)
    elif op == 4 and active == request_id:
        payload=sys.stdin.buffer.read(count * 4)
        if len(payload) != count * 4: sys.exit(6)
        time.sleep({delay_ms} / 1000.0)
        revision += 1
        if {malformed_flag}: response(14, request_id, b'\0\0\0\0\x80' + b'\0' * 39)
        else: response(14, request_id, snapshot(revision, False, b'controlled partial', b'controlled ', b'partial'))
    elif op == 5 and active == request_id:
        revision += 1
        response(15, request_id, snapshot(revision, True, b'controlled resident text', b'controlled resident text', b''))
        active=None
    elif op == 6 and active == request_id:
        response(16, request_id); active=None
    else: sys.exit(7)
"#
        );
        std::fs::write(&path, source)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
        Ok(path)
    }

    fn controlled_worker(dir: &Path, mode: &str) -> Result<PathBuf, std::io::Error> {
        let path = dir.join("resident-worker.py");
        let control_dir = format!("{:?}", dir);
        let mode = format!("{mode:?}");
        let source = format!(
            r#"#!/usr/bin/env python3
import os, struct, sys, time
MAGIC=b'NRAS'; VERSION=5; CONTROL_DIR={control_dir}; MODE={mode}
def response(status, request_id, payload=b''):
    sys.stdout.buffer.write(MAGIC + struct.pack('<HBB', VERSION, status, 0) + request_id.to_bytes(16, 'little') + struct.pack('<I', len(payload)) + payload)
    sys.stdout.buffer.flush()
def mark(name):
    with open(os.path.join(CONTROL_DIR, name), 'w', encoding='ascii') as handle: handle.write('1')
def snapshot(revision, final, full, committed, tentative):
    flags=1 | (2 if final else 0) | 4 | (0 if final else 8)
    return struct.pack('<iB3xqqqIII', revision, flags, 0, 0, 0, len(full), len(committed), len(tentative)) + full + committed + tentative
with open(os.path.join(CONTROL_DIR, 'worker.pid'), 'w', encoding='ascii') as handle: handle.write(str(os.getpid()))
with open(os.path.join(CONTROL_DIR, 'spawn.count'), 'a', encoding='ascii') as handle: handle.write('1\n')
ON_GPU = sys.argv[1:2] == ['--compute'] and sys.argv[2] in ('auto', 'gpu')
if ON_GPU and MODE == 'gpu_startup_dies': sys.exit(9)
if MODE == 'startup_block':
    mark('startup.entered')
    time.sleep(10)
if MODE == 'startup_malformed':
    sys.stdout.buffer.write(b'BAD!' + b'\0' * 24); sys.stdout.buffer.flush(); time.sleep(10)
else:
    response(0, 0, bytes([1, 1 if ON_GPU else 0, 0, 0]))
active=None; revision=0
while True:
    header=sys.stdin.buffer.read(28)
    if not header: break
    if len(header) != 28 or header[:4] != MAGIC: sys.exit(3)
    op=header[6]; request_id=int.from_bytes(header[8:24], 'little'); count=struct.unpack('<I', header[24:28])[0]
    if op == 2:
        mark('shutdown.received')
        if MODE == 'shutdown_block': time.sleep(10)
        break
    if op == 1:
        options_len_raw=sys.stdin.buffer.read(4)
        if len(options_len_raw) != 4: sys.exit(4)
        options_len=struct.unpack('<I', options_len_raw)[0]
        options=sys.stdin.buffer.read(options_len)
        if len(options) != options_len: sys.exit(4)
        payload=sys.stdin.buffer.read(count * 4)
        if len(payload) != count * 4: sys.exit(4)
        if ON_GPU and MODE == 'gpu_decode_dies': sys.exit(9)
        if ON_GPU and MODE == 'gpu_decode_fails': response(3, request_id)
        else: response(1, request_id, b'controlled resident text')
    elif op == 3:
        mark('begin.entered')
        if MODE == 'begin_eof': sys.exit(8)
        if MODE == 'begin_malformed':
            bad=bytearray(MAGIC + struct.pack('<HBB', VERSION, 13, 1) + request_id.to_bytes(16, 'little') + struct.pack('<I', 0))
            sys.stdout.buffer.write(bad); sys.stdout.buffer.flush(); time.sleep(10)
        elif MODE == 'begin_block': time.sleep(10)
        else: active=request_id; revision=0; response(13, request_id)
    elif op == 4 and active == request_id:
        payload=sys.stdin.buffer.read(count * 4)
        if len(payload) != count * 4: sys.exit(6)
        mark('feed.entered')
        if ON_GPU and MODE == 'gpu_feed_dies': sys.exit(9)
        if MODE == 'feed_block': time.sleep(10)
        revision += 1
        response(14, request_id, snapshot(revision, False, b'controlled partial', b'controlled ', b'partial'))
    elif op == 5 and active == request_id:
        revision += 1
        response(15, request_id, snapshot(revision, True, b'controlled resident text', b'controlled resident text', b'')); active=None
    elif op == 6 and active == request_id:
        response(16, request_id); active=None
    else: sys.exit(7)
"#
        );
        std::fs::write(&path, source)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
        Ok(path)
    }

    fn wait_for_marker(path: &Path) -> Result<(), std::io::Error> {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !path.exists() {
            if Instant::now() >= deadline {
                return Err(std::io::Error::other("controlled worker marker timed out"));
            }
            std::thread::sleep(POLL_INTERVAL);
        }
        Ok(())
    }

    fn controlled_pid(dir: &Path) -> Result<u32, std::io::Error> {
        // The fixture creates the PID file before writing it, so existence is
        // not readiness; poll for non-empty parseable content.
        let path = dir.join("worker.pid");
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Ok(text) = std::fs::read_to_string(&path)
                && let Ok(pid) = text.trim().parse()
            {
                return Ok(pid);
            }
            if Instant::now() >= deadline {
                return Err(std::io::Error::other(
                    "controlled worker did not record a PID",
                ));
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    fn assert_process_reaped(pid: u32) -> Result<(), std::io::Error> {
        if Path::new("/proc").join(pid.to_string()).exists() {
            return Err(std::io::Error::other(format!(
                "controlled worker PID {pid} was not reaped"
            )));
        }
        Ok(())
    }

    fn current_worker_pid(engine: &ResidentWorkerEngine) -> Result<u32, std::io::Error> {
        engine
            .shared
            .worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .client
            .as_ref()
            .map(|worker| worker.child.id())
            .ok_or_else(|| std::io::Error::other("resident worker was missing"))
    }

    fn controlled_config(model: PathBuf, executable: PathBuf) -> WorkerConfig {
        WorkerConfig {
            executable: WorkerExecutable::Controlled(executable),
            model: WorkerModel::Controlled(model),
            worker_origin: WorkerBindingOrigin::Controlled,
            model_origin: ModelBindingOrigin::Controlled,
            compute: Arc::new(ComputePolicy::new(ComputeRequest::Cpu)),
            leading_args: Arc::from(Vec::new()),
        }
    }

    fn process_state(pid: u32) -> Option<String> {
        std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|stat| {
                stat.rsplit(')')
                    .next()
                    .map(|tail| tail.split_whitespace().next().unwrap_or("?").to_owned())
            })
    }

    /// A worker created for a thread that has since exited must still serve a
    /// bounded real protocol operation. `/proc` presence and cached readiness
    /// race the asynchronous parent-death delivery, so neither is evidence
    /// that a worker survived; the settle window makes the direct-spawn case
    /// fail deterministically instead of racing the kernel signal.
    fn assert_worker_answers_after_requesting_thread_exit(
        engine: &ResidentWorkerEngine,
        worker_pid: u32,
    ) -> Result<(), Box<dyn std::error::Error>> {
        std::thread::sleep(OWNER_EXIT_SETTLE);
        let state = process_state(worker_pid);
        assert!(
            matches!(state.as_deref(), Some("R" | "S" | "D")),
            "resident worker was not running when its protocol operation ran: {state:?}"
        );
        let mut engine = engine.clone();
        let transcript =
            as_test_result(engine.recognize(SessionId(93), &audio(), &SessionCancellation::new()))?;
        assert_eq!(transcript.text, "controlled resident text");
        Ok(())
    }

    /// Teardown must retire the persistent owner thread and leave no worker
    /// behind; the latch is observable through the engine's own anchor.
    fn assert_engine_teardown_retires_owner(
        engine: ResidentWorkerEngine,
        worker_pid: u32,
    ) -> Result<(), std::io::Error> {
        let retirement = Arc::clone(&engine.shared.spawn_anchor.retirement);
        drop(engine);
        if !retirement.wait(Instant::now() + Duration::from_secs(2)) {
            return Err(std::io::Error::other(
                "resident spawn owner did not retire during teardown",
            ));
        }
        assert_process_reaped(worker_pid)
    }

    const OWNER_EXIT_SETTLE: Duration = Duration::from_millis(250);

    fn install_recovery_hook(
        engine: &ResidentWorkerEngine,
        fail_termination_once: bool,
    ) -> (mpsc::Receiver<()>, std::sync::mpsc::SyncSender<()>) {
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        *engine
            .shared
            .recovery_post_handshake_hook
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(RecoveryPostHandshakeHook {
            entered: entered_tx,
            release: release_rx,
            fail_termination_once,
        });
        (entered_rx, release_tx)
    }

    fn make_restart_required(
        engine: &ResidentWorkerEngine,
        session: SessionId,
    ) -> Result<u32, Box<dyn std::error::Error>> {
        let old_pid = {
            let slot = engine
                .shared
                .worker
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            slot.client
                .as_ref()
                .map(|worker| worker.child.id())
                .ok_or_else(|| std::io::Error::other("resident worker was missing"))?
        };
        let cancellation = SessionCancellation::new();
        let mut active = engine.clone();
        let mut stream = as_test_result(active.open_stream(session, &cancellation))?;
        cancellation.cancel();
        as_test_result(stream.finish(Duration::from_secs(1)))?;
        assert_process_reaped(old_pid)?;
        if engine.readiness() != ResidentReadiness::RestartRequired {
            return Err(std::io::Error::other("resident did not require restart").into());
        }
        Ok(old_pid)
    }

    fn audio() -> AudioChunk {
        AudioChunk {
            sample_rate_hz: 16_000,
            channels: 1,
            samples: vec![0.0; 160],
        }
    }

    /// Lets controlled workers be asked for a GPU for the rest of a test.
    struct GpuAllowed;

    impl GpuAllowed {
        fn for_this_test() -> Self {
            CONTROLLED_COMPUTE.with(|compute| compute.set(ComputeRequest::Auto));
            Self
        }
    }

    impl Drop for GpuAllowed {
        fn drop(&mut self) {
            CONTROLLED_COMPUTE.with(|compute| compute.set(ComputeRequest::Cpu));
        }
    }

    fn spawn_count(dir: &Path) -> Result<usize, std::io::Error> {
        Ok(std::fs::read_to_string(dir.join("spawn.count"))?
            .lines()
            .count())
    }

    #[test]
    fn worker_that_attests_a_gpu_is_reported_on_it() -> TestResult {
        let _guard = resident_test_guard();
        let _gpu = GpuAllowed::for_this_test();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = controlled_worker(dir.path(), "normal")?;
        let mut engine = as_test_result(ResidentWorkerEngine::with_worker_path(model, worker))?;
        assert_eq!(engine.compute(), WorkerCompute::IntegratedGpu);
        assert_eq!(
            engine.model_capabilities().compute,
            WorkerCompute::IntegratedGpu
        );
        let transcript =
            as_test_result(engine.recognize(SessionId(31), &audio(), &SessionCancellation::new()))?;
        assert_eq!(transcript.text, "controlled resident text");
        assert_eq!(spawn_count(dir.path())?, 1);
        Ok(())
    }

    #[test]
    fn worker_is_strict_cpu_unless_a_gpu_is_allowed() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = controlled_worker(dir.path(), "normal")?;
        let engine = as_test_result(ResidentWorkerEngine::with_worker_path(model, worker))?;
        assert_eq!(engine.compute(), WorkerCompute::Cpu);
        Ok(())
    }

    #[test]
    fn gpu_worker_that_cannot_start_is_replaced_by_a_cpu_worker() -> TestResult {
        let _guard = resident_test_guard();
        let _gpu = GpuAllowed::for_this_test();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = controlled_worker(dir.path(), "gpu_startup_dies")?;
        let mut engine = as_test_result(ResidentWorkerEngine::with_worker_path(model, worker))?;
        assert_eq!(engine.readiness(), ResidentReadiness::Ready);
        assert_eq!(engine.compute(), WorkerCompute::Cpu);
        assert_eq!(spawn_count(dir.path())?, 2);
        let transcript =
            as_test_result(engine.recognize(SessionId(32), &audio(), &SessionCancellation::new()))?;
        assert_eq!(transcript.text, "controlled resident text");
        Ok(())
    }

    #[test]
    fn required_gpu_that_cannot_start_is_an_error_not_a_cpu_worker() -> TestResult {
        let _guard = resident_test_guard();
        CONTROLLED_COMPUTE.with(|compute| compute.set(ComputeRequest::Gpu));
        // Restores strict CPU for later tests on this thread.
        let _restore = GpuAllowed;
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = controlled_worker(dir.path(), "gpu_startup_dies")?;
        assert!(ResidentWorkerEngine::with_worker_path(model, worker).is_err());
        assert_eq!(spawn_count(dir.path())?, 1);
        Ok(())
    }

    #[test]
    fn gpu_failure_mid_dictation_finishes_on_the_cpu() -> TestResult {
        for mode in ["gpu_decode_dies", "gpu_decode_fails"] {
            let _guard = resident_test_guard();
            let _gpu = GpuAllowed::for_this_test();
            let dir = tempfile::tempdir()?;
            let model = fake_model(dir.path())?;
            let worker = controlled_worker(dir.path(), mode)?;
            let mut engine = as_test_result(ResidentWorkerEngine::with_worker_path(model, worker))?;
            assert_eq!(engine.compute(), WorkerCompute::IntegratedGpu);
            // The same audio is recognised again on a CPU worker: the
            // dictation is not lost.
            let transcript = as_test_result(engine.recognize(
                SessionId(33),
                &audio(),
                &SessionCancellation::new(),
            ))?;
            assert_eq!(transcript.text, "controlled resident text", "{mode}");
            assert_eq!(engine.compute(), WorkerCompute::Cpu, "{mode}");
            assert_eq!(engine.readiness(), ResidentReadiness::Ready, "{mode}");
            assert_eq!(spawn_count(dir.path())?, 2, "{mode}");
            // The GPU stays withdrawn for later workers too.
            let again = as_test_result(engine.recognize(
                SessionId(34),
                &audio(),
                &SessionCancellation::new(),
            ))?;
            assert_eq!(again.text, "controlled resident text", "{mode}");
            assert_eq!(spawn_count(dir.path())?, 2, "{mode}");
        }
        Ok(())
    }

    #[test]
    fn gpu_failure_during_a_stream_moves_the_next_worker_to_the_cpu() -> TestResult {
        let _guard = resident_test_guard();
        let _gpu = GpuAllowed::for_this_test();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = controlled_worker(dir.path(), "gpu_feed_dies")?;
        let mut engine = as_test_result(ResidentWorkerEngine::with_worker_path(model, worker))?;
        assert_eq!(engine.compute(), WorkerCompute::IntegratedGpu);
        let mut stream =
            as_test_result(engine.open_stream(SessionId(35), &SessionCancellation::new()))?;
        assert!(stream.push_audio(&audio()).is_err());
        drop(stream);
        assert_eq!(engine.readiness(), ResidentReadiness::RestartRequired);
        assert_eq!(
            as_test_result(engine.recover(&SessionCancellation::new()))?,
            RecoverOutcome::Recovered
        );
        assert_eq!(engine.compute(), WorkerCompute::Cpu);
        let mut stream =
            as_test_result(engine.open_stream(SessionId(36), &SessionCancellation::new()))?;
        as_test_result(stream.push_audio(&audio()))?;
        Ok(())
    }

    #[test]
    fn one_worker_serves_repeated_requests_without_payload_files() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = fake_worker(dir.path(), 0)?;
        let mut engine = as_test_result(ResidentWorkerEngine::with_worker_path(model, worker))?;
        assert_eq!(engine.readiness(), ResidentReadiness::Ready);
        assert_eq!(
            as_test_result(engine.recover(&SessionCancellation::new()))?,
            RecoverOutcome::AlreadyReady
        );
        assert!(!engine.model_capabilities().supports_streaming);
        let first =
            as_test_result(engine.recognize(SessionId(11), &audio(), &SessionCancellation::new()))?;
        let second =
            as_test_result(engine.recognize(SessionId(12), &audio(), &SessionCancellation::new()))?;
        assert_eq!(first.text, "controlled resident text");
        assert_eq!(second.text, first.text);
        let names: Vec<_> = std::fs::read_dir(dir.path())?
            .filter_map(Result::ok)
            .map(|entry| entry.file_name())
            .collect();
        assert_eq!(names.len(), 2, "resident path created a payload file");
        Ok(())
    }

    #[test]
    fn worker_spawned_for_a_short_lived_owner_answers_after_that_owner_exits() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = fake_worker(dir.path(), 0)?;
        // Offline provisioning asks the engine to start the resident process
        // from a bounded supervisor thread that exits as soon as it commits
        // readiness. Production launches carry a kernel parent-death signal
        // that follows the thread that calls spawn, so the live process must
        // not be created on that short-lived thread.
        let owner_model = model.clone();
        let owner_worker = worker.clone();
        let spawned = std::thread::spawn(move || {
            let engine = as_test_result(ResidentWorkerEngine::with_worker_path(
                owner_model,
                owner_worker,
            ))?;
            let pid = current_worker_pid(&engine)?;
            Ok::<_, std::io::Error>((engine, pid))
        })
        .join()
        .map_err(|_| std::io::Error::other("resident spawn owner thread panicked"))?;
        let (engine, worker_pid) = spawned?;

        assert_eq!(engine.readiness(), ResidentReadiness::Ready);
        assert!(engine.is_worker_ready());
        assert_worker_answers_after_requesting_thread_exit(&engine, worker_pid)?;
        as_test_result(engine.shutdown(Duration::from_secs(2)))?;
        assert!(
            engine.shared.spawn_anchor.is_idle(),
            "spawn owner was still working when shutdown completed"
        );
        assert_process_reaped(worker_pid)?;
        assert_engine_teardown_retires_owner(engine, worker_pid)?;
        Ok(())
    }

    #[test]
    fn recovery_replacement_answers_after_its_temporary_recovery_thread_exits() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = fake_worker_with_capability(dir.path(), 0, true)?;
        let engine = as_test_result(ResidentWorkerEngine::with_worker_path(model, worker))?;
        make_restart_required(&engine, SessionId(30))?;

        // Recovery runs on a bounded supervised task that exits after it
        // installs the replacement generation.
        let recovery_engine = engine.clone();
        let recovered = std::thread::spawn(move || {
            as_test_result(recovery_engine.recover(&SessionCancellation::new()))
        })
        .join()
        .map_err(|_| std::io::Error::other("resident recovery thread panicked"))?;
        assert_eq!(recovered?, RecoverOutcome::Recovered);

        let replacement_pid = current_worker_pid(&engine)?;
        assert_worker_answers_after_requesting_thread_exit(&engine, replacement_pid)?;
        as_test_result(engine.shutdown(Duration::from_secs(2)))?;
        assert!(
            engine.shared.spawn_anchor.is_idle(),
            "spawn owner was still working when shutdown completed"
        );
        assert_process_reaped(replacement_pid)?;
        assert_engine_teardown_retires_owner(engine, replacement_pid)?;
        Ok(())
    }

    #[test]
    fn abandoned_spawn_request_is_reaped_by_the_persistent_owner() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = controlled_worker(dir.path(), "normal")?;
        let mut anchor = as_test_result(SpawnAnchor::start())?;
        let (reply, pending) = mpsc::sync_channel(0);
        anchor
            .requests
            .as_ref()
            .ok_or_else(|| std::io::Error::other("spawn owner was not running"))?
            .send(SpawnRequest::Spawn {
                config: controlled_config(model, worker),
                cancellation: SessionCancellation::new(),
                deadline: Instant::now() + Duration::from_secs(5),
                reply,
            })?;
        // The requesting thread abandons the reply only after the owner thread
        // actually created a live child; the owner must then still terminate
        // and reap that child.
        let pid = controlled_pid(dir.path())?;
        drop(pending);
        let deadline = Instant::now() + Duration::from_secs(2);
        while Path::new("/proc").join(pid.to_string()).exists() {
            if Instant::now() >= deadline {
                return Err(std::io::Error::other(format!(
                    "abandoned resident worker PID {pid} was not reaped by the owner"
                ))
                .into());
            }
            std::thread::sleep(POLL_INTERVAL);
        }
        as_test_result(anchor.stop())?;
        assert!(
            anchor.retirement.wait(Instant::now()),
            "owner retirement latch was not set"
        );
        assert!(anchor.owner.is_none(), "owner thread was not joined");
        Ok(())
    }

    #[test]
    fn unconfirmed_abandoned_worker_stays_owned_until_a_later_reap() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = controlled_worker(dir.path(), "normal")?;
        let mut anchor = as_test_result(SpawnAnchor::start())?;
        anchor.retain_next_abandoned_for_test();
        let (reply, pending) = mpsc::sync_channel(0);
        anchor
            .requests
            .as_ref()
            .ok_or_else(|| std::io::Error::other("spawn owner was not running"))?
            .send(SpawnRequest::Spawn {
                config: controlled_config(model, worker),
                cancellation: SessionCancellation::new(),
                deadline: Instant::now() + Duration::from_secs(5),
                reply,
            })?;
        let pid = controlled_pid(dir.path())?;
        drop(pending);
        let deadline = Instant::now() + Duration::from_secs(2);
        while anchor.retained_count() == 0 {
            if Instant::now() >= deadline {
                return Err(std::io::Error::other("abandoned worker was not retained").into());
            }
            std::thread::sleep(POLL_INTERVAL);
        }
        // Retention means the unconfirmed child is still owned, not lost.
        assert!(
            Path::new("/proc").join(pid.to_string()).exists(),
            "retained worker PID {pid} was dropped instead of owned"
        );
        as_test_result(anchor.reap_retained_until(Instant::now() + Duration::from_secs(2)))?;
        assert_eq!(anchor.retained_count(), 0);
        assert_process_reaped(pid)?;
        as_test_result(anchor.stop())?;
        Ok(())
    }

    /// Force-retain one real abandoned child through the owner thread and
    /// return the live PID, so retention tests do not reimplement the fixture.
    fn retain_one_abandoned_child(
        anchor: &SpawnAnchor,
        dir: &Path,
        model: PathBuf,
        worker: PathBuf,
    ) -> Result<u32, Box<dyn std::error::Error>> {
        anchor.retain_next_abandoned_for_test();
        let (reply, pending) = mpsc::sync_channel(0);
        anchor
            .requests
            .as_ref()
            .ok_or_else(|| std::io::Error::other("spawn owner was not running"))?
            .send(SpawnRequest::Spawn {
                config: controlled_config(model, worker),
                cancellation: SessionCancellation::new(),
                deadline: Instant::now() + Duration::from_secs(5),
                reply,
            })?;
        let pid = controlled_pid(dir)?;
        drop(pending);
        let deadline = Instant::now() + Duration::from_secs(2);
        while anchor.retained_count() == 0 {
            if Instant::now() >= deadline {
                return Err(std::io::Error::other("abandoned worker was not retained").into());
            }
            std::thread::sleep(POLL_INTERVAL);
        }
        Ok(pid)
    }

    /// A blocking mutex acquisition in `reap_retained_until` could wait out
    /// another caller's whole reap window and only then discover this caller's
    /// deadline was already spent. The contended acquisition must return
    /// inside this caller's own budget and leave ownership untouched.
    #[test]
    fn concurrent_retained_reap_honors_this_callers_absolute_deadline() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = controlled_worker(dir.path(), "normal")?;
        let mut anchor = as_test_result(SpawnAnchor::start())?;
        let pid = retain_one_abandoned_child(&anchor, dir.path(), model, worker)?;

        let retained = Arc::clone(&anchor.retained);
        let (locked_tx, locked_rx) = mpsc::sync_channel(0);
        let holder = std::thread::spawn(move || {
            let _guard = retained
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let _ = locked_tx.send(());
            std::thread::sleep(Duration::from_millis(400));
        });
        locked_rx.recv_timeout(Duration::from_secs(2))?;

        let started = Instant::now();
        let error = anchor
            .reap_retained_until(started + Duration::from_millis(100))
            .err()
            .ok_or_else(|| std::io::Error::other("contended reap reported success"))?;
        let elapsed = started.elapsed();
        assert_eq!(error.code, "resident_worker_reap_unconfirmed");
        assert!(
            elapsed < Duration::from_millis(400),
            "contended reap waited past this caller's deadline: {elapsed:?}"
        );

        holder
            .join()
            .map_err(|_| std::io::Error::other("retained-lock holder panicked"))?;
        assert_eq!(
            anchor.retained_count(),
            1,
            "a contended reap must not release unconfirmed ownership"
        );
        assert!(
            Path::new("/proc").join(pid.to_string()).exists(),
            "a contended reap must not drop the live child"
        );

        as_test_result(anchor.reap_retained_until(Instant::now() + Duration::from_secs(2)))?;
        assert_eq!(anchor.retained_count(), 0);
        assert_process_reaped(pid)?;
        as_test_result(anchor.stop())?;
        Ok(())
    }

    /// A forced unconfirmed reap must keep the child owned with the same sticky
    /// typed failure on every attempt, until a bounded attempt really confirms
    /// termination.
    #[test]
    fn forced_retained_reap_failure_keeps_ownership_until_confirmed() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = controlled_worker(dir.path(), "normal")?;
        let mut anchor = as_test_result(SpawnAnchor::start())?;
        let pid = retain_one_abandoned_child(&anchor, dir.path(), model, worker)?;

        anchor.fail_retained_reaps_for_test(2);
        for attempt in 0..2 {
            let error =
                match anchor.reap_retained_until(Instant::now() + Duration::from_millis(500)) {
                    Err(error) => error,
                    Ok(()) => {
                        return Err(std::io::Error::other(format!(
                            "forced reap attempt {attempt} succeeded"
                        ))
                        .into());
                    }
                };
            assert_eq!(error.code, "resident_worker_reap_unconfirmed");
            assert_eq!(
                anchor.retained_count(),
                1,
                "failed reap attempt {attempt} lost ownership"
            );
            assert!(
                Path::new("/proc").join(pid.to_string()).exists(),
                "failed reap attempt {attempt} dropped the live child"
            );
        }

        // The sticky failure clears only once a bounded attempt confirms the
        // reap; the child is then gone and the registry is empty.
        as_test_result(anchor.reap_retained_until(Instant::now() + Duration::from_secs(2)))?;
        assert_eq!(anchor.retained_count(), 0);
        assert_process_reaped(pid)?;
        as_test_result(anchor.stop())?;
        Ok(())
    }

    /// Exhausted retirement must keep the join handle under an explicit owner;
    /// a later bounded cleanup proves completion before joining and clears the
    /// sticky unconfirmed count.
    #[test]
    fn exhausted_retirement_hands_owner_thread_to_a_fallback_owner() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = controlled_worker(dir.path(), "normal")?;
        let mut anchor = as_test_result(SpawnAnchor::start())?;
        let (reply, pending) = mpsc::sync_channel(0);
        anchor
            .requests
            .as_ref()
            .ok_or_else(|| std::io::Error::other("spawn owner was not running"))?
            .send(SpawnRequest::Spawn {
                config: controlled_config(model, worker),
                cancellation: SessionCancellation::new(),
                deadline: Instant::now() + Duration::from_secs(5),
                reply,
            })?;
        let pid = controlled_pid(dir.path())?;
        // The rendezvous reply is deliberately never received, so the owner
        // stays inside this spawn activity while the retirement deadline
        // expires.
        let deadline = Instant::now() + Duration::from_secs(2);
        while anchor.is_idle() {
            if Instant::now() >= deadline {
                return Err(std::io::Error::other("owner never began the spawn").into());
            }
            std::thread::sleep(POLL_INTERVAL);
        }

        let error = anchor
            .stop_until(Instant::now() + Duration::from_millis(50))
            .err()
            .ok_or_else(|| std::io::Error::other("busy owner reported retirement"))?;
        assert_eq!(error.code, "resident_worker_spawn_owner_still_active");
        assert!(
            anchor.owner.is_some(),
            "deadline exhaustion detached the join handle"
        );

        let before = initial_retiring()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .unconfirmed;
        drop(anchor);
        {
            let state = initial_retiring()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            assert!(
                state.unconfirmed > before,
                "drop concealed an unresolved spawn owner"
            );
            assert!(
                !state.owner_threads.is_empty(),
                "drop detached the unfinished owner thread"
            );
            assert!(
                !state.registries.is_empty(),
                "drop lost the shared abandoned-child publication registry"
            );
        }

        // Releasing the abandoned reply lets the owned owner thread finish its
        // bounded abandoned-child reap and retire.
        drop(pending);
        as_test_result(terminate_initial_retiring_until(
            Instant::now() + Duration::from_secs(6),
            Some(&SessionCancellation::new()),
        ))?;
        {
            let state = initial_retiring()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            assert!(state.owner_threads.is_empty());
            assert!(state.queue.is_empty());
            assert!(state.registries.is_empty());
            assert_eq!(state.unconfirmed, 0);
        }
        assert_process_reaped(pid)?;
        Ok(())
    }

    /// A dropped anchor snapshots the children it can drain, but its still
    /// unfinished spawn owner can publish a failed-reap child *afterwards* into
    /// the registry the anchor no longer holds. The shared registry must travel
    /// to the fallback owner, and the late child must stay owned with a sticky
    /// unconfirmed failure until a bounded cleanup really reaps it.
    #[test]
    fn late_failed_child_publication_after_drop_stays_owned_until_reap() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = controlled_worker(dir.path(), "normal")?;
        let mut anchor = as_test_result(SpawnAnchor::start())?;
        // Force the owner's first abandoned child to be retained instead of
        // reaped, which is the publication that can race the drop.
        anchor.retain_next_abandoned_for_test();
        let (reply, pending) = mpsc::sync_channel(0);
        anchor
            .requests
            .as_ref()
            .ok_or_else(|| std::io::Error::other("spawn owner was not running"))?
            .send(SpawnRequest::Spawn {
                config: controlled_config(model, worker),
                cancellation: SessionCancellation::new(),
                deadline: Instant::now() + Duration::from_secs(5),
                reply,
            })?;
        let pid = controlled_pid(dir.path())?;
        // The owner is blocked in the rendezvous reply, so no child has been
        // published yet when the anchor is dropped.
        let deadline = Instant::now() + Duration::from_secs(2);
        while anchor.is_idle() {
            if Instant::now() >= deadline {
                return Err(std::io::Error::other("owner never began the spawn").into());
            }
            std::thread::sleep(POLL_INTERVAL);
        }
        let error = anchor
            .stop_until(Instant::now() + Duration::from_millis(50))
            .err()
            .ok_or_else(|| std::io::Error::other("busy owner reported retirement"))?;
        assert_eq!(error.code, "resident_worker_spawn_owner_still_active");

        let before = initial_retiring()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .unconfirmed;
        drop(anchor);

        // The drop hands the still-live registry over; it cannot contain the
        // late child yet.
        let registries = {
            let state = initial_retiring()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            assert!(state.unconfirmed > before);
            assert!(
                !state.registries.is_empty(),
                "drop lost the shared publication registry"
            );
            state.registries.clone()
        };
        assert!(shared_registry_is_empty(&registries));

        // Release the abandoned reply: the owner fails to publish the result to
        // the caller and its forced retained path publishes the failed-reap
        // child into the shared registry after the drop.
        drop(pending);
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let registries = initial_retiring()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .registries
                .clone();
            if !shared_registry_is_empty(&registries) {
                break;
            }
            if Instant::now() >= deadline {
                return Err(std::io::Error::other("late failed child was never published").into());
            }
            std::thread::sleep(POLL_INTERVAL);
        }
        assert!(
            Path::new("/proc").join(pid.to_string()).exists(),
            "the late failed child was dropped instead of owned"
        );

        // An exhausted cleanup must not report success or release the child.
        let error =
            terminate_initial_retiring_until(Instant::now(), Some(&SessionCancellation::new()))
                .err()
                .ok_or_else(|| std::io::Error::other("exhausted cleanup reported success"))?;
        assert_eq!(error.code, "resident_worker_startup_timeout");
        {
            let state = initial_retiring()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            assert!(
                state.unconfirmed > before,
                "unconfirmed late-child ownership was silently cleared"
            );
        }
        assert!(
            Path::new("/proc").join(pid.to_string()).exists(),
            "unconfirmed cleanup dropped the late child"
        );

        // A later bounded cleanup confirms the reap and clears the sticky
        // unconfirmed count.
        as_test_result(terminate_initial_retiring_until(
            Instant::now() + Duration::from_secs(6),
            Some(&SessionCancellation::new()),
        ))?;
        {
            let state = initial_retiring()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            assert!(state.owner_threads.is_empty());
            assert!(state.queue.is_empty());
            assert!(state.registries.is_empty());
            assert_eq!(state.unconfirmed, 0);
        }
        assert_process_reaped(pid)?;
        Ok(())
    }

    /// A contended fallback registry lock must not extend an exhausted drop
    /// deadline: the hand-off moves to the already-owned retained coordinator
    /// and is installed once the lock frees, with the sticky count kept until
    /// then.
    #[test]
    fn contended_fallback_lock_does_not_extend_the_drop_deadline() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = controlled_worker(dir.path(), "normal")?;
        let mut anchor = as_test_result(SpawnAnchor::start())?;
        let (reply, pending) = mpsc::sync_channel(0);
        anchor
            .requests
            .as_ref()
            .ok_or_else(|| std::io::Error::other("spawn owner was not running"))?
            .send(SpawnRequest::Spawn {
                config: controlled_config(model, worker),
                cancellation: SessionCancellation::new(),
                deadline: Instant::now() + Duration::from_secs(5),
                reply,
            })?;
        let pid = controlled_pid(dir.path())?;
        let deadline = Instant::now() + Duration::from_secs(2);
        while anchor.is_idle() {
            if Instant::now() >= deadline {
                return Err(std::io::Error::other("owner never began the spawn").into());
            }
            std::thread::sleep(POLL_INTERVAL);
        }
        let error = anchor
            .stop_until(Instant::now() + Duration::from_millis(50))
            .err()
            .ok_or_else(|| std::io::Error::other("busy owner reported retirement"))?;
        assert_eq!(error.code, "resident_worker_spawn_owner_still_active");

        // Hold the process-wide fallback registry past the drop budget.
        let (locked_tx, locked_rx) = mpsc::sync_channel(0);
        let holder = std::thread::spawn(move || {
            let _guard = initial_retiring()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let _ = locked_tx.send(());
            std::thread::sleep(Duration::from_millis(300));
        });
        locked_rx
            .recv_timeout(Duration::from_secs(2))
            .map_err(|error| std::io::Error::other(error.to_string()))?;

        let started = Instant::now();
        drop(anchor);
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_millis(250),
            "contended drop waited past its exhausted budget: {elapsed:?}"
        );

        holder
            .join()
            .map_err(|_| std::io::Error::other("fallback-lock holder panicked"))?;
        // The retained coordinator installs the hand-off once the lock frees.
        let deadline = Instant::now() + Duration::from_secs(2);
        while PENDING_HANDOFFS.load(Ordering::Acquire) != 0 {
            if Instant::now() >= deadline {
                return Err(std::io::Error::other(
                    "the retained coordinator never installed the hand-off",
                )
                .into());
            }
            std::thread::sleep(POLL_INTERVAL);
        }
        {
            let state = initial_retiring()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            assert!(
                !state.owner_threads.is_empty(),
                "the retained coordinator lost the unfinished owner thread"
            );
            assert!(
                !state.registries.is_empty(),
                "the retained coordinator lost the shared publication registry"
            );
        }

        drop(pending);
        as_test_result(terminate_initial_retiring_until(
            Instant::now() + Duration::from_secs(6),
            Some(&SessionCancellation::new()),
        ))?;
        {
            let state = initial_retiring()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            assert!(state.owner_threads.is_empty());
            assert!(state.queue.is_empty());
            assert!(state.registries.is_empty());
            assert_eq!(state.unconfirmed, 0);
        }
        assert_process_reaped(pid)?;
        Ok(())
    }

    /// The retained fallback hand-off owner must exist before any resident
    /// child is admitted. A forced initialization failure (test-only hook,
    /// indistinguishable from a real refused thread creation) fails the engine
    /// constructor closed: no worker process is executed and no anchor owns a
    /// child.
    #[test]
    fn failed_handoff_owner_initialization_prevents_child_admission() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = controlled_worker(dir.path(), "normal")?;
        FAILED_HANDOFF_OWNER_INITIALIZATIONS.store(1, Ordering::Release);

        let error = ResidentWorkerEngine::with_worker_path_until(
            model,
            worker,
            Instant::now() + Duration::from_secs(5),
            &SessionCancellation::new(),
        )
        .err()
        .ok_or_else(|| std::io::Error::other("startup succeeded without its hand-off owner"))?;

        assert_eq!(error.code, "resident_worker_handoff_owner_unavailable");
        assert_eq!(
            FAILED_HANDOFF_OWNER_INITIALIZATIONS.load(Ordering::Acquire),
            0,
            "the forced failure hook was not consumed by the engine constructor"
        );
        assert!(
            !dir.path().join("worker.pid").exists(),
            "a child was admitted before the retained hand-off owner existed"
        );
        assert!(
            !dir.path().join("spawn.count").exists(),
            "the controlled worker executed without a retained hand-off owner"
        );
        Ok(())
    }

    /// The hand-off owner belongs to its engine while the engine lives and is
    /// not exposed to the process-wide fallback owner. Once the engine drops,
    /// the coordinator publishes its own join handle and a bounded cleanup must
    /// prove completion and join it before it can report quiescence.
    #[test]
    fn dropped_anchor_retains_its_handoff_owner_until_a_bounded_cleanup_reaps_it() -> TestResult {
        let _guard = resident_test_guard();
        // Settle earlier coordinators so the registration below is the only
        // change the assertions can observe.
        let settle = Instant::now() + Duration::from_secs(2);
        while COORDINATOR_RETIREMENTS.load(Ordering::Acquire) != 0 {
            if Instant::now() >= settle {
                return Err(std::io::Error::other("a coordinator retirement never settled").into());
            }
            std::thread::sleep(POLL_INTERVAL);
        }
        let before = initial_retiring()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .coordinator_threads
            .len();

        let anchor = as_test_result(SpawnAnchor::start())?;
        {
            let state = initial_retiring()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            assert_eq!(
                state.coordinator_threads.len(),
                before,
                "a live engine exposed its hand-off owner to the process-wide fallback"
            );
        }
        drop(anchor);

        // The coordinator owns its retirement: it publishes its own handle
        // before exiting, and the sticky retirement count keeps a concurrent
        // cleanup from reporting clean first.
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let observed = initial_retiring()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .coordinator_threads
                .len();
            if observed > before {
                break;
            }
            if Instant::now() >= deadline {
                return Err(std::io::Error::other(
                    "the dropped hand-off owner was never registered",
                )
                .into());
            }
            std::thread::sleep(POLL_INTERVAL);
        }

        as_test_result(terminate_initial_retiring_until(
            Instant::now() + Duration::from_secs(2),
            Some(&SessionCancellation::new()),
        ))?;
        {
            let state = initial_retiring()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            assert!(state.coordinator_threads.is_empty());
            assert!(state.owner_threads.is_empty());
            assert!(state.queue.is_empty());
            assert!(state.registries.is_empty());
            assert_eq!(state.unconfirmed, 0);
        }
        assert_eq!(COORDINATOR_RETIREMENTS.load(Ordering::Acquire), 0);
        Ok(())
    }

    /// Snapshot helper: true when every handed-off publication registry is
    /// currently empty.
    fn shared_registry_is_empty(registries: &[Arc<Mutex<VecDeque<Child>>>]) -> bool {
        registries.iter().all(|registry| {
            registry
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_empty()
        })
    }

    #[test]
    fn anchor_rendezvous_respects_callers_absolute_deadline_and_cancellation() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = fake_worker(dir.path(), 0)?;
        let mut anchor = as_test_result(SpawnAnchor::start())?;
        let config = controlled_config(model, worker);

        let expired = anchor
            .spawn(&config, &SessionCancellation::new(), Instant::now())
            .err()
            .ok_or_else(|| std::io::Error::other("expired spawn request was accepted"))?;
        assert_eq!(expired.code, "resident_worker_startup_timeout");

        let cancellation = SessionCancellation::new();
        cancellation.cancel();
        let cancelled = anchor
            .spawn(
                &config,
                &cancellation,
                Instant::now() + Duration::from_secs(1),
            )
            .err()
            .ok_or_else(|| std::io::Error::other("cancelled spawn request was accepted"))?;
        assert_eq!(cancelled.code, "cancelled");
        as_test_result(anchor.stop())?;
        Ok(())
    }

    #[test]
    fn worker_attests_streaming_capability_without_enabling_a_stream_path() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = fake_worker_with_capability(dir.path(), 0, true)?;
        let mut engine = as_test_result(ResidentWorkerEngine::with_worker_path(model, worker))?;

        assert!(engine.model_capabilities().supports_streaming);
        let result =
            as_test_result(engine.recognize(SessionId(14), &audio(), &SessionCancellation::new()))?;
        assert_eq!(result.text, "controlled resident text");
        Ok(())
    }

    #[test]
    fn final_only_model_rejects_stream_before_worker_operation() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = fake_worker(dir.path(), 0)?;
        let mut engine = as_test_result(ResidentWorkerEngine::with_worker_path(model, worker))?;

        assert!(!engine.capabilities().streaming);
        let error = engine
            .open_stream(SessionId(17), &SessionCancellation::new())
            .err()
            .ok_or_else(|| std::io::Error::other("final-only model opened a stream"))?;
        assert_eq!(error.code, "streaming_model_unsupported");
        Ok(())
    }

    #[test]
    fn capable_worker_emits_started_partial_final_in_order() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = fake_worker_with_capability(dir.path(), 0, true)?;
        let mut engine = as_test_result(ResidentWorkerEngine::with_worker_path(model, worker))?;
        let mut stream =
            as_test_result(engine.open_stream(SessionId(18), &SessionCancellation::new()))?;
        assert_eq!(engine.readiness(), ResidentReadiness::Active);

        assert!(matches!(
            stream.poll_event(),
            Some(AsrEvent::Started { .. })
        ));
        as_test_result(stream.push_audio(&audio()))?;
        let Some(AsrEvent::Partial { seq, transcript }) = stream.poll_event() else {
            return Err(std::io::Error::other("missing controlled partial").into());
        };
        assert_eq!(seq, 1);
        assert_eq!(transcript.text, "controlled partial");
        assert!(!transcript.is_final);
        as_test_result(stream.push_audio(&audio()))?;
        assert!(
            stream.poll_event().is_none(),
            "duplicate partial was emitted"
        );
        as_test_result(stream.finalize())?;
        let Some(AsrEvent::Final {
            seq, transcript, ..
        }) = stream.poll_event()
        else {
            return Err(std::io::Error::other("missing controlled final").into());
        };
        assert_eq!(seq, 2);
        assert_eq!(transcript.text, "controlled resident text");
        assert!(transcript.is_final);
        as_test_result(stream.finish(Duration::from_millis(100)))?;
        assert_eq!(engine.readiness(), ResidentReadiness::Ready);
        Ok(())
    }

    #[test]
    fn active_readiness_detects_an_exited_worker_and_clears_the_stream() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = fake_worker_with_capability(dir.path(), 0, true)?;
        let mut engine = as_test_result(ResidentWorkerEngine::with_worker_path(model, worker))?;
        let stream =
            as_test_result(engine.open_stream(SessionId(181), &SessionCancellation::new()))?;
        assert_eq!(engine.readiness(), ResidentReadiness::Active);

        {
            let mut slot = engine
                .shared
                .worker
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            slot.client
                .as_mut()
                .ok_or_else(|| std::io::Error::other("resident worker was missing"))?
                .child
                .kill()?;
        }

        let deadline = Instant::now() + Duration::from_secs(1);
        while engine.readiness() == ResidentReadiness::Active && Instant::now() < deadline {
            std::thread::sleep(POLL_INTERVAL);
        }
        assert_eq!(engine.readiness(), ResidentReadiness::RestartRequired);
        {
            let slot = engine
                .shared
                .worker
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            assert!(slot.client.is_none());
            assert!(slot.active_stream.is_none());
        }
        drop(stream);
        Ok(())
    }

    #[test]
    fn malformed_stream_snapshot_terminates_worker_and_restarts_cleanly() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = fake_worker_with_behavior(dir.path(), 0, 1, true)?;
        let mut engine = as_test_result(ResidentWorkerEngine::with_worker_path(model, worker))?;
        let mut stream =
            as_test_result(engine.open_stream(SessionId(24), &SessionCancellation::new()))?;
        let error = stream
            .push_audio(&audio())
            .err()
            .ok_or_else(|| std::io::Error::other("malformed stream snapshot was accepted"))?;
        assert_eq!(error.code, "resident_worker_protocol");
        drop(stream);
        assert_eq!(engine.readiness(), ResidentReadiness::RestartRequired);

        fake_worker_with_capability(dir.path(), 0, true)?;
        let not_lazy = engine
            .recognize(SessionId(25), &audio(), &SessionCancellation::new())
            .err()
            .ok_or_else(|| std::io::Error::other("one-shot lazily restarted the worker"))?;
        assert_eq!(not_lazy.code, "resident_worker_restart_required");
        assert_eq!(
            as_test_result(engine.recover(&SessionCancellation::new()))?,
            RecoverOutcome::Recovered
        );
        let transcript =
            as_test_result(engine.recognize(SessionId(26), &audio(), &SessionCancellation::new()))?;
        assert_eq!(transcript.text, "controlled resident text");
        Ok(())
    }

    #[test]
    fn active_stream_excludes_one_shot_and_drop_resets_worker() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = fake_worker_with_capability(dir.path(), 0, true)?;
        let mut engine = as_test_result(ResidentWorkerEngine::with_worker_path(model, worker))?;
        let stream =
            as_test_result(engine.open_stream(SessionId(19), &SessionCancellation::new()))?;
        assert_eq!(engine.readiness(), ResidentReadiness::Active);
        assert_eq!(
            as_test_result(engine.recover(&SessionCancellation::new()))?,
            RecoverOutcome::AlreadyReady
        );
        let busy = engine
            .recognize(SessionId(20), &audio(), &SessionCancellation::new())
            .err()
            .ok_or_else(|| std::io::Error::other("one-shot overlapped an active stream"))?;
        assert_eq!(busy.code, "asr_busy");
        drop(stream);
        assert_eq!(engine.readiness(), ResidentReadiness::Ready);
        let transcript =
            as_test_result(engine.recognize(SessionId(21), &audio(), &SessionCancellation::new()))?;
        assert_eq!(transcript.text, "controlled resident text");
        Ok(())
    }

    #[test]
    fn cloned_stream_cancellation_kills_blocked_feed_and_allows_restart() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = controlled_worker(dir.path(), "feed_block")?;
        let mut engine = as_test_result(ResidentWorkerEngine::with_worker_path(model, worker))?;
        let old_pid = controlled_pid(dir.path())?;
        let cancellation = SessionCancellation::new();
        let mut stream = as_test_result(engine.open_stream(SessionId(22), &cancellation))?;
        let cancellation = stream.cancellation();
        let handle = std::thread::spawn(move || stream.push_audio(&audio()));
        wait_for_marker(&dir.path().join("feed.entered"))?;
        cancellation.cancel();
        let error = handle
            .join()
            .map_err(|_| std::io::Error::other("stream feed panicked"))?
            .err()
            .ok_or_else(|| std::io::Error::other("cancelled stream feed succeeded"))?;
        assert_eq!(error.code, "cancelled");
        assert_process_reaped(old_pid)?;
        assert_eq!(engine.readiness(), ResidentReadiness::RestartRequired);

        controlled_worker(dir.path(), "normal")?;
        assert_eq!(
            as_test_result(engine.recover(&SessionCancellation::new()))?,
            RecoverOutcome::Recovered
        );
        let transcript =
            as_test_result(engine.recognize(SessionId(23), &audio(), &SessionCancellation::new()))?;
        assert_eq!(transcript.text, "controlled resident text");
        Ok(())
    }

    #[test]
    fn cancelled_stream_begin_reaps_unknown_worker_and_restarts_cleanly() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = controlled_worker(dir.path(), "begin_block")?;
        let mut engine = as_test_result(ResidentWorkerEngine::with_worker_path(model, worker))?;
        let old_pid = controlled_pid(dir.path())?;
        let cancellation = SessionCancellation::new();
        let trigger = cancellation.clone();
        let mut opening = engine.clone();
        let handle = std::thread::spawn(move || {
            opening
                .open_stream(SessionId(50), &cancellation)
                .map(|_| ())
        });
        wait_for_marker(&dir.path().join("begin.entered"))?;
        trigger.cancel();
        let error = handle
            .join()
            .map_err(|_| std::io::Error::other("stream begin panicked"))?
            .err()
            .ok_or_else(|| std::io::Error::other("cancelled stream begin succeeded"))?;
        assert_eq!(error.code, "cancelled");
        assert_process_reaped(old_pid)?;
        assert_eq!(engine.readiness(), ResidentReadiness::RestartRequired);

        controlled_worker(dir.path(), "normal")?;
        let not_lazy = engine
            .open_stream(SessionId(51), &SessionCancellation::new())
            .err()
            .ok_or_else(|| std::io::Error::other("stream admission lazily restarted the worker"))?;
        assert_eq!(not_lazy.code, "resident_worker_restart_required");
        assert_eq!(
            as_test_result(engine.recover(&SessionCancellation::new()))?,
            RecoverOutcome::Recovered
        );
        let mut restarted = engine;
        let stream =
            as_test_result(restarted.open_stream(SessionId(52), &SessionCancellation::new()))?;
        drop(stream);
        Ok(())
    }

    #[test]
    fn timed_out_stream_begin_reaps_unknown_worker_and_restarts_cleanly() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = controlled_worker(dir.path(), "begin_block")?;
        let mut engine = as_test_result(ResidentWorkerEngine::with_worker_path(model, worker))?;
        let old_pid = controlled_pid(dir.path())?;
        let error = engine
            .open_stream(SessionId(52), &SessionCancellation::new())
            .err()
            .ok_or_else(|| std::io::Error::other("timed-out stream begin succeeded"))?;
        assert_eq!(error.code, "decode_timeout");
        assert_process_reaped(old_pid)?;
        assert_eq!(engine.readiness(), ResidentReadiness::RestartRequired);

        controlled_worker(dir.path(), "normal")?;
        assert_eq!(
            as_test_result(engine.recover(&SessionCancellation::new()))?,
            RecoverOutcome::Recovered
        );
        let stream =
            as_test_result(engine.open_stream(SessionId(53), &SessionCancellation::new()))?;
        drop(stream);
        Ok(())
    }

    #[test]
    fn eof_and_malformed_stream_begin_reap_workers() -> TestResult {
        let _guard = resident_test_guard();
        for (offset, mode, expected) in [
            (0_u128, "begin_eof", "resident_worker_io"),
            (10_u128, "begin_malformed", "resident_worker_protocol"),
        ] {
            let dir = tempfile::tempdir()?;
            let model = fake_model(dir.path())?;
            let worker = controlled_worker(dir.path(), mode)?;
            let mut engine = as_test_result(ResidentWorkerEngine::with_worker_path(model, worker))?;
            let old_pid = controlled_pid(dir.path())?;
            let error = engine
                .open_stream(SessionId(60 + offset), &SessionCancellation::new())
                .err()
                .ok_or_else(|| std::io::Error::other("invalid stream begin succeeded"))?;
            assert_eq!(error.code, expected);
            assert_process_reaped(old_pid)?;
            assert_eq!(engine.readiness(), ResidentReadiness::RestartRequired);

            controlled_worker(dir.path(), "normal")?;
            assert_eq!(
                as_test_result(engine.recover(&SessionCancellation::new()))?,
                RecoverOutcome::Recovered
            );
            let stream = as_test_result(
                engine.open_stream(SessionId(61 + offset), &SessionCancellation::new()),
            )?;
            drop(stream);
        }
        Ok(())
    }

    #[test]
    fn oversized_finish_timeout_is_typed_and_does_not_panic() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = fake_worker_with_capability(dir.path(), 0, true)?;
        let mut engine = as_test_result(ResidentWorkerEngine::with_worker_path(model, worker))?;
        let cancellation = SessionCancellation::new();
        let mut stream = as_test_result(engine.open_stream(SessionId(70), &cancellation))?;
        cancellation.cancel();
        let error = stream
            .finish(Duration::MAX)
            .err()
            .ok_or_else(|| std::io::Error::other("oversized timeout was accepted"))?;
        assert_eq!(error.code, "invalid_timeout");
        drop(stream);

        let transcript =
            as_test_result(engine.recognize(SessionId(71), &audio(), &SessionCancellation::new()))?;
        assert_eq!(transcript.text, "controlled resident text");
        Ok(())
    }

    #[test]
    fn worker_with_unknown_capability_fails_startup_closed() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = fake_worker_with_capability_flag(dir.path(), 0, 0b1000_0000)?;

        let error = ResidentWorkerEngine::with_worker_path(model, worker)
            .err()
            .ok_or_else(|| std::io::Error::other("unknown capability was accepted"))?;
        assert_eq!(error.code, "resident_worker_protocol");
        Ok(())
    }

    #[test]
    fn worker_restart_rejects_changed_model_capabilities() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = fake_worker_with_capability(dir.path(), 500, false)?;
        let engine = as_test_result(ResidentWorkerEngine::with_worker_path(&model, &worker))?;
        let cancellation = SessionCancellation::new();
        let trigger = cancellation.clone();
        let mut running = engine.clone();
        let handle =
            std::thread::spawn(move || running.recognize(SessionId(15), &audio(), &cancellation));
        std::thread::sleep(Duration::from_millis(30));
        trigger.cancel();
        let cancelled = handle
            .join()
            .map_err(|_| std::io::Error::other("cancelled request panicked"))?
            .err()
            .ok_or_else(|| std::io::Error::other("cancelled request succeeded"))?;
        assert_eq!(cancelled.code, "cancelled");

        fake_worker_with_capability(dir.path(), 0, true)?;
        let error = engine
            .recover(&SessionCancellation::new())
            .err()
            .ok_or_else(|| std::io::Error::other("changed capability was accepted"))?;
        assert_eq!(error.code, "resident_capability_changed");
        assert_eq!(engine.readiness(), ResidentReadiness::FailedClosed);
        Ok(())
    }

    #[test]
    fn decode_budget_scales_with_audio_and_stays_bounded() {
        assert_eq!(decode_budget(0), DECODE_TIMEOUT);
        assert_eq!(decode_budget(16_000), Duration::from_secs(121));
        assert_eq!(
            decode_budget(narrivy_types::MAX_CAPTURE_SAMPLES),
            Duration::from_secs(420)
        );
        assert_eq!(
            decode_budget(narrivy_types::MAX_CAPTURE_SAMPLES * 100),
            MAX_DECODE_TIMEOUT
        );
        assert!(decode_budget(narrivy_types::MAX_CAPTURE_SAMPLES) <= MAX_DECODE_TIMEOUT);
    }

    #[test]
    fn portable_capture_maximum_is_the_resident_audio_bound() {
        assert_eq!(MAX_SAMPLES, narrivy_types::MAX_CAPTURE_SAMPLES);
        let maximum = AudioChunk {
            sample_rate_hz: 16_000,
            channels: 1,
            samples: vec![0.0; MAX_SAMPLES],
        };
        assert!(validate_audio(&maximum).is_ok());
        let over = AudioChunk {
            sample_rate_hz: 16_000,
            channels: 1,
            samples: vec![0.0; MAX_SAMPLES + 1],
        };
        assert_eq!(
            validate_audio(&over).err().map(|error| error.code),
            Some("invalid_audio_length")
        );
    }

    #[test]
    fn maximum_audio_contract_crosses_bounded_transfer_chunks() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = fake_worker(dir.path(), 0)?;
        let mut engine = as_test_result(ResidentWorkerEngine::with_worker_path(model, worker))?;
        let maximum = AudioChunk {
            sample_rate_hz: 16_000,
            channels: 1,
            samples: vec![0.0; MAX_SAMPLES],
        };

        let result =
            as_test_result(engine.recognize(SessionId(13), &maximum, &SessionCancellation::new()))?;

        assert_eq!(result.text, "controlled resident text");
        let names: Vec<_> = std::fs::read_dir(dir.path())?
            .filter_map(Result::ok)
            .map(|entry| entry.file_name())
            .collect();
        assert_eq!(names.len(), 2, "bounded transfer created a payload file");
        Ok(())
    }

    #[test]
    fn concurrent_request_fails_fast_instead_of_queueing() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = fake_worker(dir.path(), 150)?;
        let engine = as_test_result(ResidentWorkerEngine::with_worker_path(model, worker))?;
        let (started_tx, started_rx) = mpsc::channel();
        let mut first = engine.clone();
        let handle = std::thread::spawn(move || {
            let _ = started_tx.send(());
            first.recognize(SessionId(21), &audio(), &SessionCancellation::new())
        });
        started_rx.recv_timeout(Duration::from_secs(1))?;
        std::thread::sleep(Duration::from_millis(20));
        let mut second = engine.clone();
        let error = second
            .recognize(SessionId(22), &audio(), &SessionCancellation::new())
            .err()
            .ok_or_else(|| std::io::Error::other("concurrent request queued"))?;
        assert_eq!(error.code, "asr_busy");
        let first_result = handle
            .join()
            .map_err(|_| std::io::Error::other("first request panicked"))?;
        assert!(first_result.is_ok());
        Ok(())
    }

    #[test]
    fn cancellation_kills_worker_and_next_request_restarts_it() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = fake_worker(dir.path(), 500)?;
        let engine = as_test_result(ResidentWorkerEngine::with_worker_path(model, worker))?;
        let cancellation = SessionCancellation::new();
        let trigger = cancellation.clone();
        let mut running = engine.clone();
        let handle =
            std::thread::spawn(move || running.recognize(SessionId(31), &audio(), &cancellation));
        std::thread::sleep(Duration::from_millis(30));
        trigger.cancel();
        let error = handle
            .join()
            .map_err(|_| std::io::Error::other("cancelled request panicked"))?
            .err()
            .ok_or_else(|| std::io::Error::other("cancelled request succeeded"))?;
        assert_eq!(error.code, "cancelled");
        assert_eq!(engine.readiness(), ResidentReadiness::RestartRequired);

        assert_eq!(
            as_test_result(engine.recover(&SessionCancellation::new()))?,
            RecoverOutcome::Recovered
        );
        let mut restarted = engine;
        let result = as_test_result(restarted.recognize(
            SessionId(32),
            &audio(),
            &SessionCancellation::new(),
        ))?;
        assert_eq!(result.text, "controlled resident text");
        Ok(())
    }

    #[test]
    fn recovery_exposes_one_warming_generation_and_cancellation_reaps_it() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = controlled_worker(dir.path(), "normal")?;
        let engine = as_test_result(ResidentWorkerEngine::with_worker_path(model, worker))?;
        let old_pid = controlled_pid(dir.path())?;
        let cancellation = SessionCancellation::new();
        let mut active = engine.clone();
        let mut stream = as_test_result(active.open_stream(SessionId(80), &cancellation))?;
        assert_eq!(engine.readiness(), ResidentReadiness::Active);
        cancellation.cancel();
        as_test_result(stream.finish(Duration::from_secs(1)))?;
        drop(stream);
        assert_process_reaped(old_pid)?;
        assert_eq!(engine.readiness(), ResidentReadiness::RestartRequired);

        controlled_worker(dir.path(), "startup_block")?;
        let recovery_cancellation = SessionCancellation::new();
        let trigger = recovery_cancellation.clone();
        let recovering = engine.clone();
        let handle = std::thread::spawn(move || recovering.recover(&recovery_cancellation));
        wait_for_marker(&dir.path().join("startup.entered"))?;
        let warming_pid = controlled_pid(dir.path())?;
        assert_eq!(engine.readiness(), ResidentReadiness::Warming);
        assert_eq!(
            as_test_result(engine.recover(&SessionCancellation::new()))?,
            RecoverOutcome::AlreadyWarming
        );
        let spawn_count = std::fs::read_to_string(dir.path().join("spawn.count"))?;
        assert_eq!(
            spawn_count.lines().count(),
            2,
            "concurrent recovery spawned twice"
        );

        trigger.cancel();
        let error = handle
            .join()
            .map_err(|_| std::io::Error::other("recovery panicked"))?
            .err()
            .ok_or_else(|| std::io::Error::other("cancelled recovery succeeded"))?;
        assert_eq!(error.code, "cancelled");
        assert_process_reaped(warming_pid)?;
        assert_eq!(engine.readiness(), ResidentReadiness::RestartRequired);

        controlled_worker(dir.path(), "normal")?;
        assert_eq!(
            as_test_result(engine.recover(&SessionCancellation::new()))?,
            RecoverOutcome::Recovered
        );
        assert_eq!(engine.readiness(), ResidentReadiness::Ready);
        Ok(())
    }

    #[test]
    fn bounded_initial_startup_rejects_pre_cancel_without_spawning() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = controlled_worker(dir.path(), "normal")?;
        let cancellation = SessionCancellation::new();
        cancellation.cancel();

        let error = ResidentWorkerEngine::with_worker_path_until(
            model,
            worker,
            Instant::now() + Duration::from_secs(1),
            &cancellation,
        )
        .err()
        .ok_or_else(|| std::io::Error::other("pre-cancelled startup succeeded"))?;

        assert_eq!(error.code, "cancelled");
        assert_eq!(error.message, "ASR cancellation requested");
        assert!(!dir.path().join("worker.pid").exists());
        assert!(!dir.path().join("spawn.count").exists());
        Ok(())
    }

    #[test]
    fn digest_bound_production_launch_maps_control_before_process_creation() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
        let owner = rustix::process::geteuid().as_raw();
        let worker_path = dir.path().join("resident-worker");
        std::fs::copy("/bin/true", &worker_path)?;
        std::fs::set_permissions(&worker_path, std::fs::Permissions::from_mode(0o755))?;
        let (worker_digest, worker_size) = file_digest(&worker_path)?;
        let worker = VerifiedArtifactExecutable::bind_trust_anchored(
            &worker_path,
            &worker_digest,
            worker_size,
            dir.path(),
            owner,
        )?;
        let model_path = fake_model(dir.path())?;
        std::fs::set_permissions(&model_path, std::fs::Permissions::from_mode(0o600))?;
        let (model_digest, model_size) = file_digest(&model_path)?;
        let model = VerifiedArtifactData::bind_trust_anchored(
            &model_path,
            &model_digest,
            model_size,
            dir.path(),
            owner,
        )?;
        let config = WorkerConfig {
            executable: WorkerExecutable::DigestBound(worker),
            model: WorkerModel::DigestBound(model),
            worker_origin: WorkerBindingOrigin::TrustAnchor,
            model_origin: ModelBindingOrigin::TrustAnchor,
            compute: Arc::new(ComputePolicy::new(ComputeRequest::Cpu)),
            leading_args: Arc::from(Vec::new()),
        };

        let cancellation = SessionCancellation::new();
        cancellation.cancel();
        let cancelled = spawn_worker_process(&config, &cancellation, Instant::now())
            .err()
            .ok_or_else(|| std::io::Error::other("pre-cancelled digest-bound launch spawned"))?;
        assert_eq!(cancelled.code, "cancelled");

        let timed_out = spawn_worker_process(&config, &SessionCancellation::new(), Instant::now())
            .err()
            .ok_or_else(|| std::io::Error::other("expired digest-bound launch spawned"))?;
        assert_eq!(timed_out.code, "resident_worker_startup_timeout");
        Ok(())
    }

    #[test]
    fn bounded_initial_startup_cancellation_reaps_controlled_worker() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = controlled_worker(dir.path(), "startup_block")?;
        let cancellation = SessionCancellation::new();
        let trigger = cancellation.clone();
        let handle = std::thread::spawn(move || {
            ResidentWorkerEngine::with_worker_path_until(
                model,
                worker,
                Instant::now() + Duration::from_secs(5),
                &cancellation,
            )
        });

        wait_for_marker(&dir.path().join("startup.entered"))?;
        let pid = controlled_pid(dir.path())?;
        trigger.cancel();
        let error = handle
            .join()
            .map_err(|_| std::io::Error::other("cancelled startup panicked"))?
            .err()
            .ok_or_else(|| std::io::Error::other("cancelled startup succeeded"))?;

        assert_eq!(error.code, "cancelled");
        assert_eq!(error.message, "ASR cancellation requested");
        assert_process_reaped(pid)?;
        assert!(
            initial_retiring()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .queue
                .is_empty()
        );
        Ok(())
    }

    #[test]
    fn bounded_initial_startup_timeout_reaps_controlled_worker() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = controlled_worker(dir.path(), "startup_block")?;

        let error = ResidentWorkerEngine::with_worker_path_until(
            model,
            worker,
            Instant::now() + TERMINATION_TIMEOUT + Duration::from_millis(250),
            &SessionCancellation::new(),
        )
        .err()
        .ok_or_else(|| std::io::Error::other("timed-out startup succeeded"))?;
        let pid = controlled_pid(dir.path())?;

        assert_eq!(error.code, "resident_worker_startup_timeout");
        assert_eq!(
            error.message,
            "resident ASR worker exceeded its startup bound"
        );
        assert_process_reaped(pid)?;
        assert!(
            initial_retiring()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .queue
                .is_empty()
        );
        Ok(())
    }

    #[test]
    fn recovery_startup_failure_and_pre_cancel_remain_restart_required() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = controlled_worker(dir.path(), "normal")?;
        let engine = as_test_result(ResidentWorkerEngine::with_worker_path(model, &worker))?;
        let old_pid = controlled_pid(dir.path())?;
        let cancellation = SessionCancellation::new();
        let mut active = engine.clone();
        let mut stream = as_test_result(active.open_stream(SessionId(81), &cancellation))?;
        cancellation.cancel();
        as_test_result(stream.finish(Duration::from_secs(1)))?;
        assert_process_reaped(old_pid)?;
        assert_eq!(engine.readiness(), ResidentReadiness::RestartRequired);

        let pre_cancelled = SessionCancellation::new();
        pre_cancelled.cancel();
        let cancelled = engine
            .recover(&pre_cancelled)
            .err()
            .ok_or_else(|| std::io::Error::other("pre-cancelled recovery succeeded"))?;
        assert_eq!(cancelled.code, "cancelled");
        assert_eq!(engine.readiness(), ResidentReadiness::RestartRequired);

        std::fs::remove_file(worker)?;
        let failed = engine
            .recover(&SessionCancellation::new())
            .err()
            .ok_or_else(|| std::io::Error::other("missing worker recovery succeeded"))?;
        assert_eq!(failed.code, "resident_worker_spawn_failed");
        assert_eq!(engine.readiness(), ResidentReadiness::RestartRequired);
        Ok(())
    }

    #[test]
    fn cancelled_pre_handshake_startup_retains_failed_cleanup_for_next_recovery() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = controlled_worker(dir.path(), "normal")?;
        let engine = as_test_result(ResidentWorkerEngine::with_worker_path(model, worker))?;
        make_restart_required(&engine, SessionId(85))?;

        controlled_worker(dir.path(), "startup_block")?;
        STARTUP_TERMINATION_FAILURES.store(1, Ordering::Release);
        let cancellation = SessionCancellation::new();
        let trigger = cancellation.clone();
        let recovering = engine.clone();
        let handle = std::thread::spawn(move || recovering.recover(&cancellation));
        wait_for_marker(&dir.path().join("startup.entered"))?;
        let candidate_pid = controlled_pid(dir.path())?;
        trigger.cancel();
        let error = handle
            .join()
            .map_err(|_| std::io::Error::other("pre-handshake recovery panicked"))?
            .err()
            .ok_or_else(|| std::io::Error::other("pre-handshake cancellation succeeded"))?;
        assert_eq!(error.code, "resident_worker_termination_timeout");
        assert!(Path::new("/proc").join(candidate_pid.to_string()).exists());
        assert_eq!(engine.readiness(), ResidentReadiness::RestartRequired);
        {
            let slot = engine
                .shared
                .worker
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            assert_eq!(slot.retiring.len(), 1);
            assert_eq!(slot.retiring[0].pid(), candidate_pid);
        }

        controlled_worker(dir.path(), "normal")?;
        assert_eq!(
            as_test_result(engine.recover(&SessionCancellation::new()))?,
            RecoverOutcome::Recovered
        );
        assert_process_reaped(candidate_pid)?;
        Ok(())
    }

    #[test]
    fn initial_startup_failure_retains_child_until_next_constructor() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = controlled_worker(dir.path(), "startup_malformed")?;
        STARTUP_TERMINATION_FAILURES.store(1, Ordering::Release);
        let error = ResidentWorkerEngine::with_worker_path(&model, &worker)
            .err()
            .ok_or_else(|| std::io::Error::other("malformed startup unexpectedly succeeded"))?;
        assert_eq!(error.code, "resident_worker_termination_timeout");
        let candidate_pid = controlled_pid(dir.path())?;
        assert!(Path::new("/proc").join(candidate_pid.to_string()).exists());
        {
            let retiring = initial_retiring()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            assert_eq!(retiring.queue.len(), 1);
            assert_eq!(retiring.queue[0].pid(), candidate_pid);
        }

        controlled_worker(dir.path(), "normal")?;
        let engine = as_test_result(ResidentWorkerEngine::with_worker_path(model, worker))?;
        assert_process_reaped(candidate_pid)?;
        assert_eq!(engine.readiness(), ResidentReadiness::Ready);
        assert!(
            initial_retiring()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .queue
                .is_empty()
        );
        Ok(())
    }

    #[test]
    fn overlapping_initial_cleanup_honors_the_earlier_caller_deadline() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = controlled_worker(dir.path(), "startup_malformed")?;
        STARTUP_TERMINATION_FAILURES.store(1, Ordering::Release);
        let retained = ResidentWorkerEngine::with_worker_path(&model, &worker)
            .err()
            .ok_or_else(|| std::io::Error::other("malformed startup unexpectedly succeeded"))?;
        assert_eq!(retained.code, "resident_worker_termination_timeout");
        let retained_pid = controlled_pid(dir.path())?;

        controlled_worker(dir.path(), "normal")?;
        let (entered_tx, entered_rx) = mpsc::sync_channel(0);
        let (release_tx, release_rx) = mpsc::sync_channel(0);
        *initial_cleanup_hook()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(InitialCleanupHook {
            entered: entered_tx,
            release: release_rx,
        });

        let first_model = model.clone();
        let first_worker = worker.clone();
        let first = std::thread::spawn(move || {
            ResidentWorkerEngine::with_worker_path(first_model, first_worker)
        });
        entered_rx.recv_timeout(Duration::from_secs(2))?;
        {
            let state = initial_retiring()
                .try_lock()
                .map_err(|_| std::io::Error::other("cleanup retained the ownership mutex"))?;
            assert!(state.cleanup_in_progress);
            assert!(state.queue.is_empty());
        }

        let started = Instant::now();
        let second = terminate_initial_retiring_until(
            started + Duration::from_millis(50),
            Some(&SessionCancellation::new()),
        )
        .err()
        .ok_or_else(|| std::io::Error::other("overlapping cleanup ignored lifecycle ownership"))?;
        assert_eq!(second.code, "resident_worker_startup_timeout");
        assert!(started.elapsed() < Duration::from_millis(500));

        release_tx.send(())?;
        let replacement = first
            .join()
            .map_err(|_| std::io::Error::other("initial cleanup constructor panicked"))?;
        let replacement = as_test_result(replacement)?;
        assert_process_reaped(retained_pid)?;
        assert_eq!(replacement.readiness(), ResidentReadiness::Ready);
        Ok(())
    }

    #[test]
    fn final_engine_drop_retains_unreaped_child_until_next_constructor() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = controlled_worker(dir.path(), "shutdown_block")?;
        let engine = as_test_result(ResidentWorkerEngine::with_worker_path(&model, &worker))?;
        let worker_pid = controlled_pid(dir.path())?;
        {
            let mut slot = engine
                .shared
                .worker
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let client = slot
                .client
                .as_mut()
                .ok_or_else(|| std::io::Error::other("resident worker was missing"))?;
            client.termination_failures_remaining = 1;
        }
        drop(engine);
        assert!(Path::new("/proc").join(worker_pid.to_string()).exists());
        {
            let retiring = initial_retiring()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            assert_eq!(retiring.queue.len(), 1);
            assert_eq!(retiring.queue[0].pid(), worker_pid);
            assert_eq!(
                retiring.unconfirmed, 1,
                "drop must leave a sticky unconfirmed ownership count"
            );
            assert!(
                retiring.owner_threads.is_empty(),
                "a completed owner thread must have been joined by drop"
            );
        }

        controlled_worker(dir.path(), "normal")?;
        let replacement = as_test_result(ResidentWorkerEngine::with_worker_path(model, worker))?;
        assert_process_reaped(worker_pid)?;
        assert_eq!(replacement.readiness(), ResidentReadiness::Ready);
        let retiring = initial_retiring()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(
            retiring.unconfirmed, 0,
            "a confirmed reap must clear the sticky ownership count"
        );
        Ok(())
    }

    #[test]
    fn post_handshake_cancellation_reaps_candidate_before_restoring_restart() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = controlled_worker(dir.path(), "normal")?;
        let engine = as_test_result(ResidentWorkerEngine::with_worker_path(model, worker))?;
        make_restart_required(&engine, SessionId(82))?;

        controlled_worker(dir.path(), "normal")?;
        let (entered, release) = install_recovery_hook(&engine, false);
        let cancellation = SessionCancellation::new();
        let trigger = cancellation.clone();
        let recovering = engine.clone();
        let handle = std::thread::spawn(move || recovering.recover(&cancellation));
        entered.recv_timeout(Duration::from_secs(2))?;
        let candidate_pid = controlled_pid(dir.path())?;
        assert_eq!(engine.readiness(), ResidentReadiness::Warming);
        trigger.cancel();
        release.send(())?;

        let error = handle
            .join()
            .map_err(|_| std::io::Error::other("post-handshake recovery panicked"))?
            .err()
            .ok_or_else(|| std::io::Error::other("post-handshake cancellation succeeded"))?;
        assert_eq!(error.code, "cancelled");
        assert_process_reaped(candidate_pid)?;
        assert_eq!(engine.readiness(), ResidentReadiness::RestartRequired);
        let slot = engine
            .shared
            .worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert!(slot.client.is_none());
        assert!(slot.retiring.is_empty());
        Ok(())
    }

    #[test]
    fn failed_candidate_reap_remains_owned_until_next_recovery() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = controlled_worker(dir.path(), "normal")?;
        let engine = as_test_result(ResidentWorkerEngine::with_worker_path(model, worker))?;
        make_restart_required(&engine, SessionId(83))?;

        controlled_worker(dir.path(), "normal")?;
        let (entered, release) = install_recovery_hook(&engine, true);
        let cancellation = SessionCancellation::new();
        let trigger = cancellation.clone();
        let recovering = engine.clone();
        let handle = std::thread::spawn(move || recovering.recover(&cancellation));
        entered.recv_timeout(Duration::from_secs(2))?;
        let candidate_pid = controlled_pid(dir.path())?;
        trigger.cancel();
        release.send(())?;

        let error = handle
            .join()
            .map_err(|_| std::io::Error::other("candidate cleanup panicked"))?
            .err()
            .ok_or_else(|| std::io::Error::other("forced candidate cleanup succeeded"))?;
        assert_eq!(error.code, "resident_worker_termination_timeout");
        assert!(Path::new("/proc").join(candidate_pid.to_string()).exists());
        assert_eq!(engine.readiness(), ResidentReadiness::RestartRequired);
        {
            let slot = engine
                .shared
                .worker
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            assert!(slot.client.is_none());
            assert_eq!(slot.retiring.len(), 1);
            assert_eq!(slot.retiring[0].pid(), candidate_pid);
        }

        assert_eq!(
            as_test_result(engine.recover(&SessionCancellation::new()))?,
            RecoverOutcome::Recovered
        );
        assert_process_reaped(candidate_pid)?;
        let slot = engine
            .shared
            .worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert!(slot.retiring.is_empty());
        assert!(slot.client.is_some());
        Ok(())
    }

    #[test]
    fn failed_active_worker_reap_moves_to_owned_retiring_state() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = controlled_worker(dir.path(), "normal")?;
        let engine = as_test_result(ResidentWorkerEngine::with_worker_path(model, worker))?;
        let worker_pid = controlled_pid(dir.path())?;
        {
            let mut slot = engine
                .shared
                .worker
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let client = slot
                .client
                .as_mut()
                .ok_or_else(|| std::io::Error::other("resident worker was missing"))?;
            client.termination_failures_remaining = 1;
        }

        let cancellation = SessionCancellation::new();
        let mut active = engine.clone();
        let mut stream = as_test_result(active.open_stream(SessionId(84), &cancellation))?;
        cancellation.cancel();
        let error = stream
            .finish(Duration::from_secs(1))
            .err()
            .ok_or_else(|| std::io::Error::other("forced active cleanup succeeded"))?;
        assert_eq!(error.code, "resident_worker_termination_timeout");
        drop(stream);
        assert!(Path::new("/proc").join(worker_pid.to_string()).exists());
        assert_eq!(engine.readiness(), ResidentReadiness::RestartRequired);
        {
            let slot = engine
                .shared
                .worker
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            assert!(slot.client.is_none());
            assert_eq!(slot.retiring.len(), 1);
            assert_eq!(slot.retiring[0].pid(), worker_pid);
        }

        controlled_worker(dir.path(), "normal")?;
        assert_eq!(
            as_test_result(engine.recover(&SessionCancellation::new()))?,
            RecoverOutcome::Recovered
        );
        assert_process_reaped(worker_pid)?;
        Ok(())
    }

    #[test]
    fn explicit_shutdown_reaps_worker_and_permanently_closes_admission() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = controlled_worker(dir.path(), "normal")?;
        let engine = as_test_result(ResidentWorkerEngine::with_worker_path(model, worker))?;
        let worker_pid = controlled_pid(dir.path())?;

        as_test_result(engine.shutdown(Duration::from_secs(1)))?;
        assert_process_reaped(worker_pid)?;
        assert!(dir.path().join("shutdown.received").exists());
        assert_eq!(engine.readiness(), ResidentReadiness::FailedClosed);
        as_test_result(engine.shutdown(Duration::from_millis(50)))?;

        let recovery = engine
            .recover(&SessionCancellation::new())
            .err()
            .ok_or_else(|| std::io::Error::other("shutdown engine recovered"))?;
        assert_eq!(recovery.code, "resident_worker_shutdown");

        let mut one_shot = engine.clone();
        let recognition = one_shot
            .recognize(SessionId(90), &audio(), &SessionCancellation::new())
            .err()
            .ok_or_else(|| std::io::Error::other("shutdown engine recognized audio"))?;
        assert_eq!(recognition.code, "resident_worker_shutdown");

        let mut streaming = engine;
        let stream = streaming
            .open_stream(SessionId(91), &SessionCancellation::new())
            .err()
            .ok_or_else(|| std::io::Error::other("shutdown engine opened a stream"))?;
        assert_eq!(stream.code, "resident_worker_shutdown");
        Ok(())
    }

    #[test]
    fn shutdown_closes_recovery_admission_and_candidate_is_reaped() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = controlled_worker(dir.path(), "normal")?;
        let engine = as_test_result(ResidentWorkerEngine::with_worker_path(model, worker))?;
        make_restart_required(&engine, SessionId(92))?;

        controlled_worker(dir.path(), "normal")?;
        let (entered, release) = install_recovery_hook(&engine, false);
        let recovering = engine.clone();
        let recovery = std::thread::spawn(move || recovering.recover(&SessionCancellation::new()));
        entered.recv_timeout(Duration::from_secs(2))?;
        let candidate_pid = controlled_pid(dir.path())?;

        let timeout = engine
            .shutdown(Duration::from_millis(20))
            .err()
            .ok_or_else(|| std::io::Error::other("shutdown ignored in-flight recovery"))?;
        assert_eq!(timeout.code, "resident_worker_shutdown_timeout");
        release.send(())?;
        let recovery_error = recovery
            .join()
            .map_err(|_| std::io::Error::other("recovery panicked"))?
            .err()
            .ok_or_else(|| std::io::Error::other("recovery crossed closed admission"))?;
        assert_eq!(recovery_error.code, "resident_worker_shutdown");
        assert_process_reaped(candidate_pid)?;

        as_test_result(engine.shutdown(Duration::from_secs(1)))?;
        assert_eq!(engine.readiness(), ResidentReadiness::FailedClosed);
        Ok(())
    }

    /// The shared service budget can already be spent when resident cleanup
    /// starts. Even then, admission must close, the owned worker must receive a
    /// cancel signal, ownership must survive, and the failure must stay
    /// truthful instead of pretending the generation was reaped.
    #[test]
    fn exhausted_shutdown_deadline_still_closes_admission_and_signals_owner() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = controlled_worker(dir.path(), "normal")?;
        let engine = as_test_result(ResidentWorkerEngine::with_worker_path(model, worker))?;
        let worker_pid = controlled_pid(dir.path())?;
        {
            let mut slot = engine
                .shared
                .worker
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            slot.client
                .as_mut()
                .ok_or_else(|| std::io::Error::other("resident worker was missing"))?
                .termination_failures_remaining = 1;
        }

        let error = match engine.shutdown_until(Instant::now()) {
            Err(error) => error,
            Ok(()) => {
                return Err(std::io::Error::other("an exhausted deadline reported success").into());
            }
        };
        assert!(
            matches!(
                error.code,
                "resident_worker_shutdown_timeout"
                    | "resident_worker_termination_timeout"
                    | "resident_worker_reap_unconfirmed"
            ),
            "unexpected exhausted-shutdown error: {}",
            error.code
        );
        assert_eq!(engine.readiness(), ResidentReadiness::FailedClosed);
        let recovery = engine
            .recover(&SessionCancellation::new())
            .err()
            .ok_or_else(|| std::io::Error::other("shutdown engine recovered"))?;
        assert_eq!(recovery.code, "resident_worker_shutdown");

        // A live worker may not stay owned only by the kernel parent-death net;
        // it must remain in an explicit registry, and a later bounded shutdown
        // must confirm the reap.
        if Path::new("/proc").join(worker_pid.to_string()).exists() {
            let slot = engine
                .shared
                .worker
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let owned = slot.client.is_some() || !slot.retiring.is_empty();
            drop(slot);
            let retained = engine.shared.spawn_anchor.retained_count() > 0;
            assert!(
                owned || retained,
                "live worker PID {worker_pid} lost explicit ownership"
            );
        }
        as_test_result(engine.shutdown_until(Instant::now() + Duration::from_secs(3)))?;
        assert_process_reaped(worker_pid)?;
        Ok(())
    }

    #[test]
    fn malformed_audio_and_symlink_worker_are_rejected() -> TestResult {
        let _guard = resident_test_guard();
        let dir = tempfile::tempdir()?;
        let model = fake_model(dir.path())?;
        let worker = fake_worker(dir.path(), 0)?;
        let link = dir.path().join("worker-link");
        std::os::unix::fs::symlink(&worker, &link)?;
        let error = ResidentWorkerEngine::with_worker_path(&model, link)
            .err()
            .ok_or_else(|| std::io::Error::other("symlink worker accepted"))?;
        assert_eq!(error.code, "resident_worker_invalid");

        let mut engine = as_test_result(ResidentWorkerEngine::with_worker_path(model, worker))?;
        let mut invalid = audio();
        invalid.samples[0] = f32::NAN;
        let error = engine
            .recognize(SessionId(40), &invalid, &SessionCancellation::new())
            .err()
            .ok_or_else(|| std::io::Error::other("non-finite audio accepted"))?;
        assert_eq!(error.code, "invalid_audio_sample");
        Ok(())
    }

    #[test]
    fn generic_trust_anchor_status_cannot_name_manifest_or_registry_authority() {
        let worker = format!("{:?}", ResidentWorkerArtifactStatus::TrustAnchorDigestBound);
        let model = format!("{:?}", ResidentModelArtifactStatus::TrustAnchorDigestBound);
        assert!(!worker.contains("Installed"));
        assert!(!worker.contains("Manifest"));
        assert!(!model.contains("Registry"));
        assert!(!model.contains("Package"));
    }

    #[test]
    #[ignore = "controlled content-free diagnostic; requires explicit real model and worker bindings"]
    fn real_model_snapshot_survives_source_removal_and_worker_restart() -> TestResult {
        let _guard = resident_test_guard();
        let source = std::env::var_os("NARRIVY_TRANSCRIBE_MODEL")
            .map(PathBuf::from)
            .ok_or_else(|| std::io::Error::other("NARRIVY_TRANSCRIBE_MODEL is required"))?;
        let digest = std::env::var(MODEL_SHA256_ENV)?;
        let size = std::env::var(MODEL_SIZE_ENV)?.parse::<u64>()?;
        let dir = tempfile::tempdir()?;
        let model = dir.path().join("controlled-model.gguf");
        std::fs::copy(&source, &model)?;
        std::fs::set_permissions(&model, std::fs::Permissions::from_mode(0o600))?;

        let engine = as_test_result(ResidentWorkerEngine::with_model_binding(
            &model, &digest, size,
        ))?;
        assert_eq!(
            engine.model_artifact_status(),
            ResidentModelArtifactStatus::ControlledDigestBound
        );
        std::fs::remove_file(&model)?;
        assert_eq!(engine.readiness(), ResidentReadiness::Ready);

        {
            let mut slot = engine
                .shared
                .worker
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            slot.client
                .as_mut()
                .ok_or_else(|| std::io::Error::other("resident worker missing"))?
                .child
                .kill()?;
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while engine.readiness() == ResidentReadiness::Ready && Instant::now() < deadline {
            std::thread::sleep(POLL_INTERVAL);
        }
        assert_eq!(engine.readiness(), ResidentReadiness::RestartRequired);
        assert_eq!(
            as_test_result(engine.recover(&SessionCancellation::new()))?,
            RecoverOutcome::Recovered
        );
        assert_eq!(engine.readiness(), ResidentReadiness::Ready);
        as_test_result(engine.shutdown(Duration::from_secs(2)))?;
        Ok(())
    }
}
