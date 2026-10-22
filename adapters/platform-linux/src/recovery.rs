//! Concrete Linux-only ports for the strict Recovery Phase 2A Inbox-primary
//! and Phase 2B stdin-only-wtype-plus-Inbox-recovery slices. Neither slice
//! uses the general router, clipboard, uinput, or fixture paths.

use std::io::{self, Read};
#[cfg(test)]
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
#[cfg(test)]
use std::process::Command;
use std::process::{Child, ChildStdout, ExitStatus, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
    mpsc::{Receiver, SyncSender, TryRecvError, TrySendError, sync_channel},
};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[cfg(test)]
use std::sync::Mutex;

use narrivy_platform_api::{
    AudioCapturePort, AudioCaptureSession, CaptureEnd, CaptureStreamChunk, CaptureStreamEvent,
    CaptureStreamInfo, CaptureStreamSummary, CapturedAudio, DeliveryPort, PlatformError,
    StreamingAudioCapturePort, StreamingAudioCaptureSession,
};
use narrivy_process_linux::{
    HelperEnvironment, LaunchError, ProcessError, ResolveError, TrustedCommand, TrustedExecutable,
    terminate_group_and_reap, try_wait_contained,
};
use narrivy_types::{
    AudioChunk, DeliveryAttempt, DeliveryIntent, DeliveryOutcome, DeliveryReport, FallbackReason,
    SessionCancellation, SessionControl, SessionFinalizationReason,
};

use crate::delivery::{
    InboxStore,
    command::{OwnedStdinPump, StdinPumpState},
};
use crate::portal_eis::{PortalEisTextFailure, PortalEisTextPayload, PortalEisTextSubmission};

pub const CAPTURE_RATE_HZ: u32 = narrivy_types::CAPTURE_SAMPLE_RATE_HZ;
pub const CAPTURE_CHANNELS: u16 = 1;
/// Request a low-latency PipeWire quantum for interactive dictation. The
/// helper default is 100 ms, which alone exceeds the warm capture-ready goal.
const CAPTURE_LATENCY: &str = "10ms";
pub const CAPTURE_CANCEL_BOUND: Duration = Duration::from_secs(2);
pub const WTYPE_SUBMIT_BOUND: Duration = Duration::from_secs(2);
const PIPEWIRE_TARGET: &str = "@DEFAULT_AUDIO_SOURCE@";
const CAPTURE_READ_POLL: Duration = Duration::from_millis(2);
const CAPTURE_READ_BUFFER_BYTES: usize = 16 * 1024;
/// The streaming cadence is 80 ms at 16 kHz. The queue holds 64 chunks:
/// 5.12 s of audio, 320 KiB of payload. A larger model on the CPU can take
/// longer than one chunk period for a step (the first partial, a busy
/// machine); the queue absorbs such a stall instead of ending the dictation.
/// It stays bounded: an engine that is persistently slower than real time
/// still saturates it, and saturation fails the session. It never silently
/// substitutes a shorter stream for the captured input.
///
/// The original bound was eight chunks (640 ms), sized for the tiny streaming
/// canary. With the first larger streaming model on the reference machine a
/// third consecutive dictation failed as `capture_backpressure`.
pub const STREAM_CAPTURE_CHUNK_SAMPLES: usize = 1_280;
pub const STREAM_CAPTURE_QUEUE_CHUNKS: usize = 64;

#[cfg(test)]
static WTYPE_SYSTEM_TEST_LOCK: Mutex<()> = Mutex::new(());

/// Content-free availability result for the configured PipeWire recorder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PipeWireRecorderStatus {
    Available(PathBuf),
    Missing,
    NonExecutable,
    Untrusted,
    IdentityChanged,
    UnsupportedPlatform,
}

#[derive(Clone)]
enum StrictHelper {
    Trusted(TrustedExecutable),
    #[cfg(test)]
    Controlled(PathBuf),
}

impl std::fmt::Debug for StrictHelper {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Trusted(executable) => executable.fmt(formatter),
            #[cfg(test)]
            Self::Controlled(_) => formatter.write_str("ControlledHelper([redacted])"),
        }
    }
}

enum StrictCommand {
    Trusted(TrustedCommand),
    #[cfg(test)]
    Controlled(Command),
}

#[derive(Debug)]
enum StrictSpawnError {
    Resolve(ResolveError),
    Io,
}

impl StrictHelper {
    fn command(&self, environment: HelperEnvironment) -> Result<StrictCommand, ResolveError> {
        match self {
            Self::Trusted(executable) => {
                Ok(StrictCommand::Trusted(executable.command(environment)?))
            }
            #[cfg(test)]
            Self::Controlled(path) => {
                let mut command = Command::new(path);
                command
                    .env_clear()
                    .env("PATH", "/usr/bin:/bin")
                    .current_dir("/")
                    .process_group(0);
                Ok(StrictCommand::Controlled(command))
            }
        }
    }
}

impl StrictCommand {
    fn args<I, S>(&mut self, arguments: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        match self {
            Self::Trusted(command) => {
                command.args(arguments);
            }
            #[cfg(test)]
            Self::Controlled(command) => {
                command.args(arguments);
            }
        }
        self
    }

    fn arg(&mut self, argument: impl AsRef<std::ffi::OsStr>) -> &mut Self {
        match self {
            Self::Trusted(command) => {
                command.arg(argument);
            }
            #[cfg(test)]
            Self::Controlled(command) => {
                command.arg(argument);
            }
        }
        self
    }

    fn stdin(&mut self, configuration: Stdio) -> &mut Self {
        match self {
            Self::Trusted(command) => {
                command.stdin(configuration);
            }
            #[cfg(test)]
            Self::Controlled(command) => {
                command.stdin(configuration);
            }
        }
        self
    }

    fn stdout(&mut self, configuration: Stdio) -> &mut Self {
        match self {
            Self::Trusted(command) => {
                command.stdout(configuration);
            }
            #[cfg(test)]
            Self::Controlled(command) => {
                command.stdout(configuration);
            }
        }
        self
    }

    fn stderr(&mut self, configuration: Stdio) -> &mut Self {
        match self {
            Self::Trusted(command) => {
                command.stderr(configuration);
            }
            #[cfg(test)]
            Self::Controlled(command) => {
                command.stderr(configuration);
            }
        }
        self
    }

    fn spawn(self) -> Result<Child, StrictSpawnError> {
        match self {
            Self::Trusted(command) => command.spawn().map_err(|failure| match failure {
                LaunchError::Resolve(error) => StrictSpawnError::Resolve(error),
                LaunchError::Spawn(_) => StrictSpawnError::Io,
            }),
            #[cfg(test)]
            Self::Controlled(mut command) => spawn_bounded(&mut command),
        }
    }
}

/// Bounded retry for a test-controlled child launch.
///
/// `fork`/`exec` can fail transiently with `EAGAIN` when the machine is already
/// running many test binaries in parallel, which is not a defect in the capture
/// path under test. Retrying briefly keeps a load spike from reading as a
/// product failure, while a genuinely unlaunchable program still fails fast:
/// the budget is small and the error is unchanged.
#[cfg(test)]
fn spawn_bounded(command: &mut std::process::Command) -> Result<Child, StrictSpawnError> {
    const ATTEMPTS: u32 = 20;
    const RETRY_BOUND: Duration = Duration::from_secs(5);
    let started = Instant::now();
    let mut last = Err(StrictSpawnError::Io);
    for _ in 0..ATTEMPTS {
        match command.spawn() {
            Ok(child) => return Ok(child),
            Err(_) if started.elapsed() < RETRY_BOUND => {
                last = Err(StrictSpawnError::Io);
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => last = Err(StrictSpawnError::Io),
        }
    }
    last
}

/// Explicit, bounded invocation of PipeWire's `pw-record` for the selected
/// default microphone source. The caller owns this object for exactly one
/// session.
#[derive(Debug, Clone)]
pub struct PipeWireCapture {
    seconds: f32,
    helper: StrictHelper,
    /// The PipeWire source recorded: the default microphone unless
    /// [`PipeWireCapture::with_target`] names another.
    target: String,
    #[cfg(test)]
    stream_control_boundary: Option<StreamControlBoundary>,
    #[cfg(test)]
    producer_after_first_send: Option<DispatchPause>,
}

impl PipeWireCapture {
    /// Open one owned capture policy. `seconds` is the effective bounded
    /// window for this session: ordinary dictation passes the documented
    /// production maximum and stops early through `SessionControl`, while
    /// controlled developer probes may pass a short window. Values outside
    /// `[1, MAX_CAPTURE_SECONDS]` are rejected before any process side effect.
    pub fn new(seconds: f32) -> Result<Self, PlatformError> {
        validate_capture_window(seconds)?;
        let helper = StrictHelper::Trusted(
            resolve_pipewire_recorder().map_err(pipewire_resolve_platform_error)?,
        );
        Ok(Self {
            seconds,
            helper,
            target: PIPEWIRE_TARGET.to_owned(),
            #[cfg(test)]
            stream_control_boundary: None,
            #[cfg(test)]
            producer_after_first_send: None,
        })
    }

    /// Record `target` (a PipeWire source's node name) instead of the
    /// default microphone, such as the echo-cancelled microphone.
    #[must_use]
    pub fn with_target(mut self, target: impl Into<String>) -> Self {
        self.target = target.into();
        self
    }

    #[must_use]
    pub fn is_available() -> bool {
        matches!(
            pipewire_recorder_status(),
            PipeWireRecorderStatus::Available(_)
        )
    }
}

impl AudioCapturePort for PipeWireCapture {
    fn start(&mut self) -> Result<Box<dyn AudioCaptureSession>, PlatformError> {
        let frames = capture_frame_budget(self.seconds)?;
        let expected_samples =
            usize::try_from(frames).map_err(|_| error("capture_output_invalid"))?;
        let mut child = spawn_pipewire(&self.helper, frames, &self.target)?;
        let Some(stdout) = child.stdout.take() else {
            let _ = terminate_group_and_reap(&mut child, checked_deadline(CAPTURE_CANCEL_BOUND)?);
            return Err(error("pipewire_stdout_unavailable"));
        };
        if set_nonblocking(&stdout).is_err() {
            let _ = terminate_group_and_reap(&mut child, checked_deadline(CAPTURE_CANCEL_BOUND)?);
            return Err(error("pipewire_stdout_setup_failed"));
        }
        let started_at = Instant::now();
        Ok(Box::new(PipeWireCaptureSession {
            child: Some(child),
            stdout: Some(stdout),
            info: CaptureStreamInfo {
                source_selection: self.target.clone(),
                sample_rate_hz: CAPTURE_RATE_HZ,
                channels: CAPTURE_CHANNELS,
            },
            expected_samples,
            samples: Vec::with_capacity(expected_samples),
            carry: [0; 4],
            carry_len: 0,
            started_at,
            first_frame_at: None,
            safety_deadline: started_at
                + Duration::from_secs_f32(self.seconds)
                + Duration::from_secs(5),
            cleanup_deadline: None,
            completed: false,
        }))
    }
}

fn spawn_pipewire(helper: &StrictHelper, frames: u32, target: &str) -> Result<Child, PlatformError> {
    let mut command = helper
        .command(HelperEnvironment::PipeWireCapture)
        .map_err(pipewire_resolve_platform_error)?;
    command
        .args([
            "--raw",
            "--rate",
            "16000",
            "--channels",
            "1",
            "--format",
            "f32",
            "--target",
            target,
            "--latency",
            CAPTURE_LATENCY,
            "-n",
        ])
        .arg(frames.to_string())
        .arg("-")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    command.spawn().map_err(|failure| match failure {
        StrictSpawnError::Resolve(resolve) => pipewire_resolve_platform_error(resolve),
        StrictSpawnError::Io => error("pipewire_spawn_failed"),
    })
}

/// The documented production maximum for one owned capture window, expressed in
/// whole seconds. Both the batch and the producer-owned streaming session use
/// the same bound so a caller cannot obtain a longer capture through one API.
pub fn max_capture_seconds() -> f32 {
    narrivy_types::MAX_CAPTURE_SECONDS as f32
}

fn validate_capture_window(seconds: f32) -> Result<(), PlatformError> {
    if !seconds.is_finite() || !(1.0..=max_capture_seconds()).contains(&seconds) {
        return Err(error("capture_duration_invalid"));
    }
    Ok(())
}

fn capture_frame_budget(seconds: f32) -> Result<u32, PlatformError> {
    validate_capture_window(seconds)?;
    let frames = (f64::from(seconds) * f64::from(CAPTURE_RATE_HZ)).round();
    if !(1.0..=f64::from(u32::MAX)).contains(&frames)
        || frames > narrivy_types::MAX_CAPTURE_SAMPLES as f64
    {
        return Err(error("capture_duration_invalid"));
    }
    Ok(frames as u32)
}

struct PipeWireCaptureSession {
    child: Option<Child>,
    stdout: Option<ChildStdout>,
    info: CaptureStreamInfo,
    expected_samples: usize,
    samples: Vec<f32>,
    carry: [u8; 4],
    carry_len: usize,
    started_at: Instant,
    first_frame_at: Option<Instant>,
    safety_deadline: Instant,
    cleanup_deadline: Option<Instant>,
    completed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureRead {
    Data,
    Pending,
    Eof,
}

impl PipeWireCaptureSession {
    fn cleanup_deadline(&mut self) -> Instant {
        *self
            .cleanup_deadline
            .get_or_insert_with(|| Instant::now() + CAPTURE_CANCEL_BOUND)
    }

    fn read_available(&mut self) -> Result<CaptureRead, PlatformError> {
        let Some(stdout) = self.stdout.as_mut() else {
            return Err(error("capture_already_collected"));
        };
        let mut buffer = [0_u8; CAPTURE_READ_BUFFER_BYTES];
        match stdout.read(&mut buffer) {
            Ok(0) => Ok(CaptureRead::Eof),
            Ok(count) => {
                let had_no_frames = self.samples.is_empty();
                decode_raw_f32(
                    &buffer[..count],
                    &mut self.carry,
                    &mut self.carry_len,
                    &mut self.samples,
                    self.expected_samples,
                )?;
                if had_no_frames && !self.samples.is_empty() {
                    self.first_frame_at = Some(Instant::now());
                }
                Ok(CaptureRead::Data)
            }
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => Ok(CaptureRead::Pending),
            Err(_) => Err(error("capture_output_unreadable")),
        }
    }

    fn stop_child(&mut self, deadline: Instant) -> Result<(), PlatformError> {
        let Some(child) = self.child.as_mut() else {
            return Err(error("capture_already_collected"));
        };
        terminate_group_and_reap(child, deadline).map_err(capture_process_error)
    }

    fn reap_completed_child(&mut self) -> Result<Option<ExitStatus>, PlatformError> {
        // A normal nonblocking status poll must not start the session's one
        // cleanup window. The deadline is used only if a just-exited leader
        // left a residual process-group descendant to terminate.
        let deadline = checked_deadline(CAPTURE_CANCEL_BOUND)?;
        let Some(child) = self.child.as_mut() else {
            return Err(error("capture_already_collected"));
        };
        try_wait_contained(child, deadline).map_err(capture_process_error)
    }

    fn drain_after_reap(&mut self, deadline: Instant) -> Result<(), PlatformError> {
        loop {
            match self.read_available()? {
                CaptureRead::Data => {}
                CaptureRead::Eof => return Ok(()),
                CaptureRead::Pending if Instant::now() < deadline => {
                    std::thread::sleep(CAPTURE_READ_POLL);
                }
                CaptureRead::Pending => return Err(error("capture_output_drain_timeout")),
            }
        }
    }

    fn finish(
        &mut self,
        end: CaptureEnd,
        status: Option<ExitStatus>,
    ) -> Result<CapturedAudio, PlatformError> {
        if self.carry_len != 0 || self.samples.is_empty() {
            return Err(error("capture_output_invalid"));
        }
        if let Some(status) = status {
            let expected_budget_exit =
                expected_budget_exit(status.code(), self.samples.len(), self.expected_samples);
            if !(status.success() || expected_budget_exit) {
                return Err(error("pipewire_exit_failed"));
            }
        }
        // `SourceEnded` is dispatched as typed transaction truth instead of an
        // adapter error: the portable slice owns the durable `capture_end` and
        // fails closed with `capture_source_ended` before ASR. The captured
        // frames stay in the result only so the slice can journal the reason
        // first; the failed transaction never publishes partial audio.
        self.completed = true;
        self.child.take();
        self.stdout.take();
        let samples = std::mem::take(&mut self.samples);
        Ok(CapturedAudio {
            audio: AudioChunk {
                sample_rate_hz: CAPTURE_RATE_HZ,
                channels: CAPTURE_CHANNELS,
                samples,
            },
            end,
            first_frame_after_start_us: self.first_frame_at.map(|at| {
                u64::try_from(at.duration_since(self.started_at).as_micros()).unwrap_or(u64::MAX)
            }),
            capture_duration_us: u64::try_from(self.started_at.elapsed().as_micros())
                .unwrap_or(u64::MAX),
        })
    }

    fn terminate_for_error(&mut self) -> Result<(), PlatformError> {
        let deadline = self.cleanup_deadline();
        self.stop_child(deadline)?;
        self.drain_after_reap(deadline)
    }
}

impl AudioCaptureSession for PipeWireCaptureSession {
    fn info(&self) -> &CaptureStreamInfo {
        &self.info
    }

    fn wait_ready(&mut self, control: &SessionControl) -> Result<(), PlatformError> {
        if self.completed {
            return Err(error("capture_already_collected"));
        }
        loop {
            if control.is_cancelled() {
                self.terminate_for_error()?;
                return Err(error("cancelled"));
            }
            if control.finalization().is_requested() {
                self.terminate_for_error()?;
                return Err(error("capture_finalized_before_audio"));
            }
            let read = match self.read_available() {
                Ok(read) => read,
                Err(err) => {
                    self.terminate_for_error()?;
                    return Err(err);
                }
            };
            if !self.samples.is_empty() {
                return Ok(());
            }
            if read == CaptureRead::Eof || self.reap_completed_child()?.is_some() {
                let deadline = self.cleanup_deadline();
                self.drain_after_reap(deadline)?;
                return Err(error("capture_output_invalid"));
            }
            if Instant::now() >= self.safety_deadline {
                self.terminate_for_error()?;
                return Err(error("capture_timeout"));
            }
            std::thread::sleep(CAPTURE_READ_POLL);
        }
    }

    fn collect(&mut self, control: &SessionControl) -> Result<CapturedAudio, PlatformError> {
        self.collect_observed(control, &mut |_| {})
    }

    fn collect_observed(
        &mut self,
        control: &SessionControl,
        observe: &mut dyn FnMut(&[f32]),
    ) -> Result<CapturedAudio, PlatformError> {
        if self.completed {
            return Err(error("capture_already_collected"));
        }
        if self.samples.is_empty() {
            self.wait_ready(control)?;
        }
        let mut observed = 0;
        loop {
            if self.samples.len() != observed {
                observed = self.samples.len();
                observe(&self.samples);
            }
            // Cancellation is checked both before and after finalization so a
            // concurrent cancel always wins the observable control race.
            if control.is_cancelled() {
                self.terminate_for_error()?;
                return Err(error("cancelled"));
            }
            if control.finalization().is_requested() {
                if control.is_cancelled() {
                    self.terminate_for_error()?;
                    return Err(error("cancelled"));
                }
                let deadline = self.cleanup_deadline();
                self.stop_child(deadline)?;
                self.drain_after_reap(deadline)?;
                if control.is_cancelled() {
                    return Err(error("cancelled"));
                }
                let end = if control.finalization().reason()
                    == Some(SessionFinalizationReason::EndpointDetected)
                {
                    CaptureEnd::EndpointDetected
                } else {
                    CaptureEnd::FinalizationRequested
                };
                return self.finish(end, None);
            }
            let read = match self.read_available() {
                Ok(read) => read,
                Err(err) => {
                    self.terminate_for_error()?;
                    return Err(err);
                }
            };
            if read == CaptureRead::Eof {
                let status = loop {
                    if let Some(status) = self.reap_completed_child()? {
                        break status;
                    }
                    if Instant::now() >= self.safety_deadline {
                        self.terminate_for_error()?;
                        return Err(error("capture_timeout"));
                    }
                    std::thread::sleep(CAPTURE_READ_POLL);
                };
                let deadline = self.cleanup_deadline();
                self.drain_after_reap(deadline)?;
                let end = if self.samples.len() == self.expected_samples {
                    CaptureEnd::SafetyLimit
                } else {
                    CaptureEnd::SourceEnded
                };
                return self.finish(end, Some(status));
            }
            if let Some(status) = self.reap_completed_child()? {
                let deadline = self.cleanup_deadline();
                self.drain_after_reap(deadline)?;
                let end = if self.samples.len() == self.expected_samples {
                    CaptureEnd::SafetyLimit
                } else {
                    CaptureEnd::SourceEnded
                };
                return self.finish(end, Some(status));
            }
            if Instant::now() >= self.safety_deadline {
                self.terminate_for_error()?;
                return Err(error("capture_timeout"));
            }
            std::thread::sleep(CAPTURE_READ_POLL);
        }
    }
}

impl Drop for PipeWireCaptureSession {
    fn drop(&mut self) {
        if !self.completed {
            let deadline = self
                .cleanup_deadline
                .unwrap_or_else(|| Instant::now() + CAPTURE_CANCEL_BOUND);
            if let Some(child) = self.child.as_mut() {
                let _ = terminate_group_and_reap(child, deadline);
            }
            self.child.take();
            self.stdout.take();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum StreamProducerTerminal {
    Running = 0,
    Eof = 1,
    OutputUnreadable = 2,
    OutputInvalid = 3,
    Backpressure = 4,
    ConsumerGone = 5,
    Cancelled = 6,
    Finalized = 7,
    TerminationFailed = 8,
}

impl StreamProducerTerminal {
    fn load(value: &AtomicU8) -> Self {
        match value.load(Ordering::Acquire) {
            1 => Self::Eof,
            2 => Self::OutputUnreadable,
            3 => Self::OutputInvalid,
            4 => Self::Backpressure,
            5 => Self::ConsumerGone,
            6 => Self::Cancelled,
            7 => Self::Finalized,
            8 => Self::TerminationFailed,
            _ => Self::Running,
        }
    }

    fn error_code(self) -> Option<&'static str> {
        match self {
            Self::Running | Self::Eof | Self::Finalized => None,
            Self::OutputUnreadable => Some("capture_output_unreadable"),
            Self::OutputInvalid => Some("capture_output_invalid"),
            Self::Backpressure => Some("capture_backpressure"),
            Self::ConsumerGone => Some("capture_consumer_gone"),
            Self::Cancelled => Some("cancelled"),
            Self::TerminationFailed => Some("capture_termination_failed"),
        }
    }
}

struct ProducedCaptureChunk {
    sequence: u64,
    audio: AudioChunk,
    first_frame_after_start_us: Option<u64>,
}

/// Live bounded counterpart to [`AudioCapturePort`]. The batch and streaming
/// entry points deliberately share the exact recorder selection and duration
/// budget, while retaining independent session contracts.
impl PipeWireCapture {
    fn start_stream_owned(
        &mut self,
        control: &SessionControl,
    ) -> Result<PipeWireStreamingCaptureSession, PlatformError> {
        let frames = capture_frame_budget(self.seconds)?;
        let expected_samples =
            usize::try_from(frames).map_err(|_| error("capture_output_invalid"))?;
        let mut child = spawn_pipewire(&self.helper, frames, &self.target)?;
        let Some(stdout) = child.stdout.take() else {
            let _ = terminate_group_and_reap(&mut child, checked_deadline(CAPTURE_CANCEL_BOUND)?);
            return Err(error("pipewire_stdout_unavailable"));
        };
        if set_nonblocking(&stdout).is_err() {
            let _ = terminate_group_and_reap(&mut child, checked_deadline(CAPTURE_CANCEL_BOUND)?);
            return Err(error("pipewire_stdout_setup_failed"));
        }

        let started_at = Instant::now();
        let producer_control = control.clone();
        #[cfg(test)]
        let producer_after_first_send = self.producer_after_first_send.take();
        let terminal = Arc::new(AtomicU8::new(StreamProducerTerminal::Running as u8));
        let producer_terminal = Arc::clone(&terminal);
        let (sender, receiver) = sync_channel(STREAM_CAPTURE_QUEUE_CHUNKS);
        let parser = match std::thread::Builder::new()
            .name("narrivy-pipewire-stream".to_owned())
            .spawn(move || {
                stream_capture_producer(
                    stdout,
                    sender,
                    producer_terminal,
                    producer_control,
                    #[cfg(test)]
                    producer_after_first_send,
                    started_at,
                );
            }) {
            Ok(parser) => parser,
            Err(_) => {
                let _ =
                    terminate_group_and_reap(&mut child, checked_deadline(CAPTURE_CANCEL_BOUND)?);
                return Err(error("capture_thread_spawn_failed"));
            }
        };

        let safety_deadline = started_at
            .checked_add(Duration::from_secs_f32(self.seconds))
            .and_then(|deadline| deadline.checked_add(Duration::from_secs(5)))
            .ok_or_else(|| error("capture_duration_invalid"))?;

        Ok(PipeWireStreamingCaptureSession {
            child: Some(child),
            receiver: Some(receiver),
            parser: Some(parser),
            terminal,
            info: CaptureStreamInfo {
                source_selection: self.target.clone(),
                sample_rate_hz: CAPTURE_RATE_HZ,
                channels: CAPTURE_CHANNELS,
            },
            expected_samples,
            delivered_samples: 0,
            expected_sequence: 0,
            first_frame_after_start_us: None,
            pending: None,
            started_at,
            safety_deadline,
            stop_reason: None,
            completed: false,
            #[cfg(test)]
            control_boundary: self.stream_control_boundary.take(),
            #[cfg(test)]
            force_child_cleanup_timeout: false,
        })
    }
}

impl StreamingAudioCapturePort for PipeWireCapture {
    fn start_stream(
        &mut self,
        control: &SessionControl,
    ) -> Result<Box<dyn StreamingAudioCaptureSession>, PlatformError> {
        self.start_stream_owned(control)
            .map(|session| Box::new(session) as Box<dyn StreamingAudioCaptureSession>)
    }
}

fn stream_capture_producer(
    mut stdout: ChildStdout,
    sender: SyncSender<ProducedCaptureChunk>,
    terminal: Arc<AtomicU8>,
    control: SessionControl,
    #[cfg(test)] mut after_first_send: Option<DispatchPause>,
    started_at: Instant,
) {
    let mut input = [0_u8; CAPTURE_READ_BUFFER_BYTES];
    let mut carry = [0_u8; 4];
    let mut carry_len = 0;
    let mut samples = Vec::with_capacity(STREAM_CAPTURE_CHUNK_SAMPLES);
    let mut sequence = 0_u64;
    // The first-frame time is measured once per capture and travels on the
    // first chunk only. `first_frame_seen` keeps it from being measured again
    // after that chunk took it, which would stamp a later chunk and make the
    // core reject the stream.
    let mut first_frame_after_start_us = None;
    let mut first_frame_seen = false;

    loop {
        if producer_control_stop(
            &control,
            &sender,
            &terminal,
            sequence,
            &mut samples,
            &mut first_frame_after_start_us,
        ) {
            return;
        }
        match stdout.read(&mut input) {
            Ok(0) => {
                if carry_len != 0 {
                    set_signalled_terminal(&terminal, StreamProducerTerminal::OutputInvalid);
                    return;
                }
                if !samples.is_empty()
                    && !send_stream_chunk(
                        &sender,
                        &terminal,
                        sequence,
                        std::mem::take(&mut samples),
                        first_frame_after_start_us.take(),
                    )
                {
                    return;
                }
                terminal.store(StreamProducerTerminal::Eof as u8, Ordering::Release);
                return;
            }
            Ok(count) => {
                if control.is_cancelled() {
                    set_signalled_terminal(&terminal, StreamProducerTerminal::Cancelled);
                    return;
                }
                for byte in &input[..count] {
                    carry[carry_len] = *byte;
                    carry_len += 1;
                    if carry_len != 4 {
                        continue;
                    }
                    carry_len = 0;
                    let sample = f32::from_le_bytes(carry);
                    if !sample.is_finite() {
                        set_signalled_terminal(&terminal, StreamProducerTerminal::OutputInvalid);
                        return;
                    }
                    if !first_frame_seen {
                        first_frame_seen = true;
                        first_frame_after_start_us = Some(
                            u64::try_from(started_at.elapsed().as_micros()).unwrap_or(u64::MAX),
                        );
                    }
                    samples.push(sample);
                    if samples.len() == STREAM_CAPTURE_CHUNK_SAMPLES {
                        let complete = std::mem::replace(
                            &mut samples,
                            Vec::with_capacity(STREAM_CAPTURE_CHUNK_SAMPLES),
                        );
                        if !send_stream_chunk(
                            &sender,
                            &terminal,
                            sequence,
                            complete,
                            first_frame_after_start_us.take(),
                        ) {
                            return;
                        }
                        #[cfg(test)]
                        if sequence == 0 {
                            if let Some(pause) = after_first_send.take() {
                                pause.pause_submitter();
                            }
                        }
                        sequence = match sequence.checked_add(1) {
                            Some(sequence) => sequence,
                            None => {
                                set_signalled_terminal(
                                    &terminal,
                                    StreamProducerTerminal::OutputInvalid,
                                );
                                return;
                            }
                        };
                    }
                }
            }
            Err(read_error) if read_error.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(CAPTURE_READ_POLL);
            }
            Err(_) => {
                set_signalled_terminal(&terminal, StreamProducerTerminal::OutputUnreadable);
                return;
            }
        }
    }
}

fn producer_control_stop(
    control: &SessionControl,
    sender: &SyncSender<ProducedCaptureChunk>,
    terminal: &AtomicU8,
    sequence: u64,
    samples: &mut Vec<f32>,
    first_frame_after_start_us: &mut Option<u64>,
) -> bool {
    if control.is_cancelled() {
        set_signalled_terminal(terminal, StreamProducerTerminal::Cancelled);
        return true;
    }
    if !control.finalization().is_requested() {
        return false;
    }
    if control.is_cancelled() {
        set_signalled_terminal(terminal, StreamProducerTerminal::Cancelled);
        return true;
    }
    if !samples.is_empty()
        && !send_stream_chunk(
            sender,
            terminal,
            sequence,
            std::mem::take(samples),
            first_frame_after_start_us.take(),
        )
    {
        return true;
    }
    set_signalled_terminal(terminal, StreamProducerTerminal::Finalized);
    true
}

fn set_signalled_terminal(terminal: &AtomicU8, requested: StreamProducerTerminal) {
    terminal.store(requested as u8, Ordering::Release);
}

fn send_stream_chunk(
    sender: &SyncSender<ProducedCaptureChunk>,
    terminal: &AtomicU8,
    sequence: u64,
    samples: Vec<f32>,
    first_frame_after_start_us: Option<u64>,
) -> bool {
    let message = ProducedCaptureChunk {
        sequence,
        audio: AudioChunk {
            sample_rate_hz: CAPTURE_RATE_HZ,
            channels: CAPTURE_CHANNELS,
            samples,
        },
        first_frame_after_start_us,
    };
    match sender.try_send(message) {
        Ok(()) => true,
        Err(TrySendError::Full(_)) => {
            set_signalled_terminal(terminal, StreamProducerTerminal::Backpressure);
            false
        }
        Err(TrySendError::Disconnected(_)) => {
            terminal.store(
                StreamProducerTerminal::ConsumerGone as u8,
                Ordering::Release,
            );
            false
        }
    }
}

struct PipeWireStreamingCaptureSession {
    child: Option<Child>,
    receiver: Option<Receiver<ProducedCaptureChunk>>,
    parser: Option<JoinHandle<()>>,
    terminal: Arc<AtomicU8>,
    info: CaptureStreamInfo,
    expected_samples: usize,
    delivered_samples: usize,
    expected_sequence: u64,
    first_frame_after_start_us: Option<u64>,
    pending: Option<CaptureStreamChunk>,
    started_at: Instant,
    safety_deadline: Instant,
    stop_reason: Option<CaptureEnd>,
    completed: bool,
    #[cfg(test)]
    control_boundary: Option<StreamControlBoundary>,
    #[cfg(test)]
    force_child_cleanup_timeout: bool,
}

#[cfg(test)]
#[derive(Debug, Clone)]
enum StreamControlBoundary {
    AfterReceive(DispatchPause),
}

impl PipeWireStreamingCaptureSession {
    fn request_finalization_stop(&mut self) -> Result<(), PlatformError> {
        if self.stop_reason.is_some() {
            return Ok(());
        }
        let Some(child) = self.child.as_mut() else {
            return Err(error("capture_already_collected"));
        };
        let stopped = terminate_group_and_reap(child, checked_deadline(CAPTURE_CANCEL_BOUND)?)
            .map_err(capture_process_error);
        if stopped.is_ok() {
            self.child.take();
            self.stop_reason = Some(CaptureEnd::FinalizationRequested);
        }
        stopped
    }

    fn terminate_owned_until(&mut self, deadline: Instant) -> Result<(), PlatformError> {
        self.receiver.take();
        #[cfg(test)]
        let force_child_cleanup_timeout = self.force_child_cleanup_timeout;
        #[cfg(not(test))]
        let force_child_cleanup_timeout = false;
        let child_result = if force_child_cleanup_timeout {
            Err(error("capture_termination_timeout"))
        } else if let Some(child) = self.child.as_mut() {
            let result = terminate_group_and_reap(child, deadline).map_err(capture_process_error);
            if result.is_ok() {
                self.child.take();
            }
            result
        } else {
            Ok(())
        };
        let parser_result = self.join_parser(deadline);
        match (child_result, parser_result) {
            (Err(child_error), _) => Err(child_error),
            (Ok(()), Err(parser_error)) => Err(parser_error),
            (Ok(()), Ok(())) => Ok(()),
        }
    }

    fn terminate_owned(&mut self) -> Result<(), PlatformError> {
        self.terminate_owned_until(checked_deadline(CAPTURE_CANCEL_BOUND)?)
    }

    fn join_parser(&mut self, deadline: Instant) -> Result<(), PlatformError> {
        while self
            .parser
            .as_ref()
            .is_some_and(|parser| !parser.is_finished())
        {
            if Instant::now() >= deadline {
                return Err(error("capture_thread_termination_timeout"));
            }
            std::thread::sleep(CAPTURE_READ_POLL);
        }
        if let Some(parser) = self.parser.take() {
            parser
                .join()
                .map_err(|_| error("capture_thread_panicked"))?;
        }
        Ok(())
    }

    fn terminal_error_until(&mut self, deadline: Instant) -> Result<(), PlatformError> {
        let terminal = StreamProducerTerminal::load(&self.terminal);
        if let Some(code) = terminal.error_code() {
            self.terminate_owned_until(deadline)?;
            self.completed = true;
            return Err(error(code));
        }
        Ok(())
    }

    fn terminal_error(&mut self) -> Result<(), PlatformError> {
        self.terminal_error_until(checked_deadline(CAPTURE_CANCEL_BOUND)?)
    }

    fn accept_chunk(
        &mut self,
        chunk: ProducedCaptureChunk,
    ) -> Result<CaptureStreamChunk, PlatformError> {
        if chunk.sequence != self.expected_sequence
            || chunk.audio.sample_rate_hz != CAPTURE_RATE_HZ
            || chunk.audio.channels != CAPTURE_CHANNELS
            || chunk.audio.samples.is_empty()
        {
            self.terminate_owned()?;
            self.completed = true;
            return Err(error("capture_stream_sequence_invalid"));
        }
        let Some(total) = self
            .delivered_samples
            .checked_add(chunk.audio.samples.len())
        else {
            self.terminate_owned()?;
            self.completed = true;
            return Err(error("capture_output_overflow"));
        };
        if total > self.expected_samples {
            self.terminate_owned()?;
            self.completed = true;
            return Err(error("capture_output_overflow"));
        }
        self.delivered_samples = total;
        self.expected_sequence = self
            .expected_sequence
            .checked_add(1)
            .ok_or_else(|| error("capture_stream_sequence_invalid"))?;
        if self.first_frame_after_start_us.is_none() {
            self.first_frame_after_start_us = chunk.first_frame_after_start_us;
        }
        Ok(CaptureStreamChunk {
            sequence: chunk.sequence,
            audio: chunk.audio,
            first_frame_after_start_us: chunk.first_frame_after_start_us,
        })
    }

    fn finish_stream(
        &mut self,
        control: &SessionControl,
    ) -> Result<CaptureStreamEvent, PlatformError> {
        let deadline = checked_deadline(CAPTURE_CANCEL_BOUND)?;
        self.join_parser(deadline)?;
        self.terminal_error_until(deadline)?;
        let terminal = StreamProducerTerminal::load(&self.terminal);
        let status = if let Some(child) = self.child.as_mut() {
            if self.stop_reason.is_some() || terminal == StreamProducerTerminal::Finalized {
                terminate_group_and_reap(child, deadline)
                    .map(|()| None)
                    .map_err(capture_process_error)
            } else {
                reap_after_stream_eof(child, deadline)
                    .map(Some)
                    .map_err(capture_process_error)
            }?
        } else if self.stop_reason.is_some() {
            None
        } else {
            return Err(error("capture_already_collected"));
        };
        self.child.take();
        self.receiver.take();

        if self.delivered_samples == 0 {
            self.completed = true;
            return Err(error(
                if self.stop_reason == Some(CaptureEnd::FinalizationRequested)
                    || terminal == StreamProducerTerminal::Finalized
                {
                    "capture_finalized_before_audio"
                } else {
                    "capture_output_invalid"
                },
            ));
        }
        let mut end = self.stop_reason.unwrap_or({
            if self.delivered_samples == self.expected_samples {
                CaptureEnd::SafetyLimit
            } else {
                CaptureEnd::SourceEnded
            }
        });
        if end == CaptureEnd::FinalizationRequested
            && control.finalization().reason() == Some(SessionFinalizationReason::EndpointDetected)
        {
            end = CaptureEnd::EndpointDetected;
        }
        if end == CaptureEnd::SafetyLimit
            && !status.is_some_and(|status| {
                status.success()
                    || expected_budget_exit(
                        status.code(),
                        self.delivered_samples,
                        self.expected_samples,
                    )
            })
        {
            self.completed = true;
            return Err(error("pipewire_exit_failed"));
        }
        // A short EOF is typed transaction truth, not an adapter error: the
        // portable slice journals `capture_end` and then fails terminal with
        // `capture_source_ended` through its owned cleanup path. Cancellation
        // still wins this control race before any summary escapes.
        self.completed = true;
        if control.is_cancelled() {
            return Err(error("cancelled"));
        }
        Ok(CaptureStreamEvent::End(CaptureStreamSummary {
            end,
            total_samples: u64::try_from(self.delivered_samples).unwrap_or(u64::MAX),
            first_frame_after_start_us: self.first_frame_after_start_us,
            capture_duration_us: u64::try_from(self.started_at.elapsed().as_micros())
                .unwrap_or(u64::MAX),
        }))
    }

    fn poll_next(&mut self, control: &SessionControl) -> Result<CaptureStreamEvent, PlatformError> {
        loop {
            if control.is_cancelled() {
                self.terminate_owned()?;
                self.completed = true;
                return Err(error("cancelled"));
            }
            self.terminal_error()?;
            if control.finalization().is_requested() && self.stop_reason.is_none() {
                self.request_finalization_stop()?;
                if control.is_cancelled() {
                    self.terminate_owned()?;
                    self.completed = true;
                    return Err(error("cancelled"));
                }
            }

            let received = self
                .receiver
                .as_ref()
                .ok_or_else(|| error("capture_already_collected"))?
                .try_recv();
            match received {
                Ok(chunk) => {
                    #[cfg(test)]
                    if let Some(StreamControlBoundary::AfterReceive(pause)) =
                        self.control_boundary.take()
                    {
                        pause.pause_submitter();
                    }
                    if control.is_cancelled() {
                        self.terminate_owned()?;
                        self.completed = true;
                        return Err(error("cancelled"));
                    }
                    return self.accept_chunk(chunk).map(CaptureStreamEvent::Audio);
                }
                Err(TryRecvError::Disconnected) => return self.finish_stream(control),
                Err(TryRecvError::Empty) => {}
            }
            if Instant::now() >= self.safety_deadline {
                self.terminate_owned()?;
                self.completed = true;
                return Err(error("capture_timeout"));
            }
            std::thread::sleep(CAPTURE_READ_POLL);
        }
    }
}

impl StreamingAudioCaptureSession for PipeWireStreamingCaptureSession {
    fn info(&self) -> &CaptureStreamInfo {
        &self.info
    }

    fn wait_ready(&mut self, control: &SessionControl) -> Result<(), PlatformError> {
        if self.completed || self.pending.is_some() {
            return Err(error(if self.completed {
                "capture_already_collected"
            } else {
                "capture_already_ready"
            }));
        }
        match self.poll_next(control)? {
            CaptureStreamEvent::Audio(chunk) => {
                self.pending = Some(chunk);
                Ok(())
            }
            CaptureStreamEvent::End(_) => Err(error("capture_output_invalid")),
        }
    }

    fn next_event(
        &mut self,
        control: &SessionControl,
    ) -> Result<CaptureStreamEvent, PlatformError> {
        if self.completed {
            return Err(error("capture_already_collected"));
        }
        if control.is_cancelled() {
            self.terminate_owned()?;
            self.completed = true;
            return Err(error("cancelled"));
        }
        // A producer terminal error means the bounded stream is no longer
        // contiguous. It must preempt even a readiness-prefetched chunk: once
        // loss is known, no audio from that transaction may escape to ASR.
        self.terminal_error()?;
        if let Some(chunk) = self.pending.take() {
            return Ok(CaptureStreamEvent::Audio(chunk));
        }
        self.poll_next(control)
    }

    fn abort(&mut self, deadline: Instant) -> Result<(), PlatformError> {
        if self.completed {
            return Ok(());
        }
        self.terminate_owned_until(deadline)?;
        self.completed = true;
        Ok(())
    }
}

impl Drop for PipeWireStreamingCaptureSession {
    fn drop(&mut self) {
        if !self.completed {
            let _ = self.terminate_owned();
        }
    }
}

fn checked_deadline(duration: Duration) -> Result<Instant, PlatformError> {
    Instant::now()
        .checked_add(duration)
        .ok_or_else(|| error("capture_timeout_invalid"))
}

fn reap_after_stream_eof(child: &mut Child, deadline: Instant) -> Result<ExitStatus, ProcessError> {
    const TERMINATION_RESERVE: Duration = Duration::from_millis(50);
    let natural_deadline = deadline
        .checked_sub(TERMINATION_RESERVE)
        .unwrap_or(deadline);
    loop {
        match try_wait_contained(child, deadline)? {
            Some(status) => return Ok(status),
            None if Instant::now() < natural_deadline => {
                std::thread::sleep(CAPTURE_READ_POLL);
            }
            None => {
                terminate_group_and_reap(child, deadline)?;
                return Err(ProcessError::TerminationTimeout);
            }
        }
    }
}

/// Inbox-only delivery adapter. It never invokes the general delivery router.
#[derive(Debug, Clone)]
pub struct InboxOnlyDelivery {
    store: InboxStore,
}

impl InboxOnlyDelivery {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            store: InboxStore::new(dir),
        }
    }
}

impl DeliveryPort for InboxOnlyDelivery {
    fn deliver(
        &mut self,
        intent: &DeliveryIntent,
        text: &str,
        cancellation: &SessionCancellation,
    ) -> Result<DeliveryReport, PlatformError> {
        if intent.backend != "inbox" || cancellation.is_cancelled() {
            return Err(error(if cancellation.is_cancelled() {
                "cancelled"
            } else {
                "delivery_intent_invalid"
            }));
        }
        self.store
            .store_text_if(&intent.transaction_id, text, || {
                !cancellation.is_cancelled()
            })
            .map_err(|err| {
                if cancellation.is_cancelled() {
                    error("cancelled")
                } else {
                    error(match err {
                        crate::delivery::InboxError::InvalidTxid => "inbox_txid_invalid",
                        crate::delivery::InboxError::Io(_) => "inbox_write_failed",
                    })
                }
            })?;
        Ok(DeliveryReport {
            transaction_id: intent.transaction_id.clone(),
            backend: "inbox".to_owned(),
            attempts: vec![DeliveryAttempt {
                backend: "inbox".to_owned(),
                outcome: DeliveryOutcome::Delivered,
                fallback_reason: None,
                at_unix_ms: now_ms(),
            }],
            outcome: DeliveryOutcome::Delivered,
            failure_reason: None,
        })
    }
}

/// Content-free availability result for the strict Phase 2B wtype submitter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WtypeSubmitterStatus {
    Available(PathBuf),
    Missing,
    NonExecutable,
    Untrusted,
    IdentityChanged,
    UnsupportedPlatform,
}

/// Fact returned by the direct-submission boundary. `SubmittedUnverified`
/// deliberately says nothing about the target application retaining text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WtypeSubmission {
    CancelledBeforeDispatch,
    SubmittedUnverified,
    Failed(FallbackReason),
}

/// Testable wtype submission boundary. Implementations must use the payload
/// only as stdin and must bound/reap their child process.
pub trait WtypeSubmitter: Send {
    fn submit(&mut self, payload: &[u8], cancellation: &SessionCancellation) -> WtypeSubmission;
}

/// Real `wtype -` submitter for the strict Phase 2B lane.
#[derive(Debug, Clone)]
pub struct SystemWtypeSubmitter {
    helper: StrictHelper,
    #[cfg(test)]
    next_stdin_fault: Option<ControlledStdinFault>,
    #[cfg(test)]
    dispatch_boundary: Option<DispatchBoundary>,
}

/// Test-only fault injection at the former writer boundary. Production has no
/// stdin writer thread: the submitter itself exclusively owns the pipe and
/// child, so a detached writer panic or spawn failure is structurally
/// impossible.
#[cfg(test)]
#[derive(Debug, Clone)]
enum ControlledStdinFault {
    SetupAfter(std::sync::Arc<std::sync::Barrier>),
    PanicAfter(std::sync::Arc<std::sync::Barrier>),
}

/// A deterministic test-only pause around a real parent-owned boundary. The
/// paused side signals arrival and then blocks until the observing test
/// releases it, so the test never uses scheduler timing as evidence of the
/// dispatch boundary. The handoff uses two bounded channels instead of
/// barriers: the observer holds no arrival sender, so a setup/spawn failure on
/// the paused side disconnects the channel and reports an error immediately,
/// and both sides have a documented backstop so a failed peer can never strand
/// a worker thread or its owned recorder.
#[cfg(test)]
#[derive(Debug, Clone)]
struct DispatchPause {
    arrived: std::sync::mpsc::Sender<()>,
    release: Arc<Mutex<std::sync::mpsc::Receiver<()>>>,
}

/// Bound within which a test-controlled child must report readiness. The child
/// is a real process, so this covers interpreter start and scheduling delay
/// under a loaded machine rather than an in-memory signal.
#[cfg(test)]
const TEST_STREAM_READY_BOUND: Duration = Duration::from_secs(30);

/// Bound within which the observing test must see the paused side reach the
/// named boundary. This only guards a live-but-stuck peer; a peer that fails
/// before the boundary disconnects the arrival channel and is reported at once.
#[cfg(test)]
const DISPATCH_BOUNDARY_OBSERVE_BOUND: Duration = Duration::from_secs(10);

/// Backstop for the paused side when the observing test is gone or stuck. The
/// release channel disconnects as soon as the observer drops its handle, so
/// this bound only guards a live-but-silent observer.
#[cfg(test)]
const DISPATCH_BOUNDARY_RELEASE_BOUND: Duration = Duration::from_secs(60);

#[cfg(test)]
impl DispatchPause {
    fn pause_submitter(&self) {
        // A disconnected observer is an immediate release; the timeout only
        // covers an observer that is alive but never releases the pause.
        let _ = self.arrived.send(());
        let release = self
            .release
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _ = release.recv_timeout(DISPATCH_BOUNDARY_RELEASE_BOUND);
    }
}

#[cfg(test)]
#[derive(Debug, Clone)]
enum DispatchBoundary {
    BeforeFirstWrite(DispatchPause),
    AfterPositiveWrite(DispatchPause),
}

impl SystemWtypeSubmitter {
    pub fn new() -> Result<Self, PlatformError> {
        let helper =
            StrictHelper::Trusted(resolve_wtype_submitter().map_err(wtype_resolve_platform_error)?);
        Ok(Self {
            helper,
            #[cfg(test)]
            next_stdin_fault: None,
            #[cfg(test)]
            dispatch_boundary: None,
        })
    }

    #[cfg(test)]
    fn with_program_for_test(program: PathBuf) -> Self {
        Self {
            helper: StrictHelper::Controlled(program),
            next_stdin_fault: None,
            dispatch_boundary: None,
        }
    }

    #[cfg(test)]
    fn fail_next_stdin_for_test(&mut self, fault: ControlledStdinFault) {
        self.next_stdin_fault = Some(fault);
    }

    #[cfg(test)]
    fn pause_at_dispatch_boundary_for_test(&mut self, boundary: DispatchBoundary) {
        self.dispatch_boundary = Some(boundary);
    }
}

impl WtypeSubmitter for SystemWtypeSubmitter {
    fn submit(&mut self, payload: &[u8], cancellation: &SessionCancellation) -> WtypeSubmission {
        if cancellation.is_cancelled() {
            return WtypeSubmission::CancelledBeforeDispatch;
        }
        // This single absolute deadline spans child creation, parent-owned
        // stdin pumping, cancellation/timeout, kill, and reap. Cleanup never
        // grants a second WTYPE_SUBMIT_BOUND window.
        let deadline = Instant::now() + WTYPE_SUBMIT_BOUND;
        let mut command = match self.helper.command(HelperEnvironment::WaylandTextInput) {
            Ok(command) => command,
            Err(resolve) => return wtype_resolve_submission(resolve),
        };
        command
            .arg("-")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(StrictSpawnError::Resolve(resolve)) => return wtype_resolve_submission(resolve),
            Err(StrictSpawnError::Io) => return wtype_failure("WTYPE_SPAWN_FAILED"),
        };
        if cancellation.is_cancelled() {
            return match terminate_group_and_reap(&mut child, deadline) {
                Ok(()) => WtypeSubmission::CancelledBeforeDispatch,
                Err(process_error) => wtype_process_failure(process_error),
            };
        }
        let Some(stdin) = child.stdin.take() else {
            return terminate_wtype_or_failure(&mut child, deadline, "WTYPE_STDIN_UNAVAILABLE");
        };

        #[cfg(test)]
        if let Some(ControlledStdinFault::SetupAfter(admitted)) = self.next_stdin_fault.as_ref() {
            admitted.wait();
            self.next_stdin_fault.take();
            return terminate_wtype_or_failure(&mut child, deadline, "WTYPE_STDIN_SETUP_FAILED");
        }
        let mut stdin = match OwnedStdinPump::new(stdin) {
            Ok(stdin) => stdin,
            Err(_) => {
                return terminate_wtype_or_failure(
                    &mut child,
                    deadline,
                    "WTYPE_STDIN_SETUP_FAILED",
                );
            }
        };

        #[cfg(test)]
        if let Some(DispatchBoundary::BeforeFirstWrite(pause)) = self.dispatch_boundary.as_ref() {
            pause.pause_submitter();
            self.dispatch_boundary.take();
        }

        // A positive write is the explicit point of no return. Before that,
        // cancellation can still return cleanly after child termination/reap.
        let mut stdin_finished = false;
        loop {
            if cancellation.is_cancelled() {
                return match terminate_group_and_reap(&mut child, deadline) {
                    Ok(()) if stdin.has_accepted_bytes() => {
                        wtype_failure("WTYPE_CANCELLED_AFTER_DISPATCH")
                    }
                    Ok(()) => WtypeSubmission::CancelledBeforeDispatch,
                    Err(process_error) => wtype_process_failure(process_error),
                };
            }
            #[cfg(test)]
            match self.next_stdin_fault.take() {
                Some(ControlledStdinFault::PanicAfter(admitted)) => {
                    admitted.wait();
                    // The production design has no writer thread to panic.
                    // This controlled counterpart proves the same cleanup
                    // contract for a failure at the former writer boundary.
                    return terminate_wtype_or_failure(
                        &mut child,
                        deadline,
                        "WTYPE_STDIN_PANICKED",
                    );
                }
                Some(ControlledStdinFault::SetupAfter(_)) | None => {}
            }
            match stdin.advance(payload) {
                Ok(StdinPumpState::Complete) => stdin_finished = true,
                Ok(StdinPumpState::Pending) => {}
                Err(_) => {
                    return terminate_wtype_or_failure(&mut child, deadline, "WTYPE_STDIN_FAILED");
                }
            }
            #[cfg(test)]
            if stdin.has_accepted_bytes() {
                if let Some(DispatchBoundary::AfterPositiveWrite(pause)) =
                    self.dispatch_boundary.take()
                {
                    pause.pause_submitter();
                }
            }
            match try_wait_contained(&mut child, deadline) {
                Ok(Some(status)) if stdin_finished && status.success() => {
                    return WtypeSubmission::SubmittedUnverified;
                }
                Ok(Some(status)) if stdin_finished => {
                    return wtype_failure(match status.code() {
                        Some(code) => match code {
                            1 => "WTYPE_EXIT_1",
                            2 => "WTYPE_EXIT_2",
                            _ => "WTYPE_EXIT_NONZERO",
                        },
                        None => "WTYPE_EXIT_SIGNAL",
                    });
                }
                Ok(Some(status)) => {
                    // A child can exit after a short nonblocking write. The
                    // parent-owned pump has not reached EOF, so this direct
                    // attempt is a typed stdin failure; no writer result or
                    // join is outstanding.
                    let _ = status;
                    return wtype_failure("WTYPE_STDIN_FAILED");
                }
                Ok(None) if !cleanup_window_open(deadline) => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Ok(None) => {
                    return match terminate_group_and_reap(&mut child, deadline) {
                        Ok(()) => WtypeSubmission::Failed(FallbackReason::Timeout),
                        Err(process_error) => wtype_process_failure(process_error),
                    };
                }
                Err(process_error) => {
                    return match terminate_group_and_reap(&mut child, deadline) {
                        Ok(()) => wtype_process_failure(process_error),
                        Err(cleanup_error) => wtype_process_failure(cleanup_error),
                    };
                }
            }
        }
    }
}

/// The keys of one paste chord.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasteKeys {
    /// Shift+Insert: pastes in almost every Linux application, terminals
    /// included, and is not taken as "paste image" by terminal programs.
    ShiftInsert,
    /// Ctrl+V: ordinary applications.
    CtrlV,
    /// Ctrl+Shift+V: terminals.
    CtrlShiftV,
}

impl PasteKeys {
    fn wtype_args(self) -> &'static [&'static str] {
        match self {
            Self::ShiftInsert => &["-M", "shift", "-k", "Insert", "-m", "shift"],
            Self::CtrlV => &["-M", "ctrl", "-k", "v", "-m", "ctrl"],
            Self::CtrlShiftV => &[
                "-M", "ctrl", "-M", "shift", "-k", "v", "-m", "shift", "-m", "ctrl",
            ],
        }
    }
}

/// Sends one paste chord through the same trusted, system-owned `wtype` as
/// [`SystemWtypeSubmitter`]. Only key names are passed; no text ever reaches
/// the helper.
#[derive(Debug, Clone)]
pub struct SystemWtypeChord {
    helper: StrictHelper,
}

impl SystemWtypeChord {
    pub fn new() -> Result<Self, PlatformError> {
        let helper =
            StrictHelper::Trusted(resolve_wtype_submitter().map_err(wtype_resolve_platform_error)?);
        Ok(Self { helper })
    }

    #[cfg(test)]
    fn with_program_for_test(program: PathBuf) -> Self {
        Self {
            helper: StrictHelper::Controlled(program),
        }
    }

    /// Send `keys` once, bounded by [`WTYPE_SUBMIT_BOUND`]. Returns a stable,
    /// content-free code on failure.
    pub fn send(
        &mut self,
        keys: PasteKeys,
        cancellation: &SessionCancellation,
    ) -> Result<(), &'static str> {
        if cancellation.is_cancelled() {
            return Err("cancelled");
        }
        let deadline = Instant::now() + WTYPE_SUBMIT_BOUND;
        let mut command = self
            .helper
            .command(HelperEnvironment::WaylandTextInput)
            .map_err(|_| "WTYPE_CHORD_UNAVAILABLE")?;
        command
            .args(keys.wtype_args())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = command.spawn().map_err(|_| "WTYPE_CHORD_SPAWN_FAILED")?;
        loop {
            match try_wait_contained(&mut child, deadline) {
                Ok(Some(status)) if status.success() => return Ok(()),
                Ok(Some(_)) => return Err("WTYPE_CHORD_EXIT_NONZERO"),
                Ok(None) if !cleanup_window_open(deadline) => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Ok(None) | Err(_) => {
                    let _ = terminate_group_and_reap(&mut child, deadline);
                    return Err("WTYPE_CHORD_TIMEOUT");
                }
            }
        }
    }
}

/// Strict Phase 2B adapter: one unverified `wtype -` submission attempt,
/// followed by one atomic Inbox recovery write with the same transaction id.
pub struct WtypeThenInboxDelivery<S = SystemWtypeSubmitter> {
    store: InboxStore,
    submitter: S,
}

impl WtypeThenInboxDelivery<SystemWtypeSubmitter> {
    pub fn new(dir: PathBuf) -> Result<Self, PlatformError> {
        let store = InboxStore::new(dir);
        // The recovery target is prepared before a wtype child can receive
        // any payload. If this fails, strict composition never starts a
        // direct attempt with no durable recovery destination available.
        store
            .ensure_private_directory()
            .map_err(|_| error("inbox_unavailable"))?;
        Ok(Self {
            store,
            submitter: SystemWtypeSubmitter::new()?,
        })
    }
}

impl<S> WtypeThenInboxDelivery<S> {
    #[cfg(test)]
    pub fn with_submitter_for_controlled_test(dir: PathBuf, submitter: S) -> Self {
        Self {
            store: InboxStore::new(dir),
            submitter,
        }
    }
}

impl<S: WtypeSubmitter> DeliveryPort for WtypeThenInboxDelivery<S> {
    fn deliver(
        &mut self,
        intent: &DeliveryIntent,
        text: &str,
        cancellation: &SessionCancellation,
    ) -> Result<DeliveryReport, PlatformError> {
        if intent.backend != "wtype-then-inbox" {
            return Err(error("delivery_intent_invalid"));
        }
        let submission = if text.len() > crate::delivery::MAX_DIRECT_TEXT_BYTES {
            WtypeSubmission::Failed(FallbackReason::PolicyDenied)
        } else {
            self.submitter.submit(text.as_bytes(), cancellation)
        };
        let direct = match submission {
            WtypeSubmission::CancelledBeforeDispatch => return Err(error("cancelled")),
            WtypeSubmission::SubmittedUnverified => DeliveryAttempt {
                backend: "wtype-stdin".to_owned(),
                outcome: DeliveryOutcome::SubmittedUnverified,
                fallback_reason: None,
                at_unix_ms: now_ms(),
            },
            WtypeSubmission::Failed(reason) => DeliveryAttempt {
                backend: "wtype-stdin".to_owned(),
                outcome: DeliveryOutcome::Failed,
                fallback_reason: Some(reason),
                at_unix_ms: now_ms(),
            },
        };
        let mut attempts = vec![direct];
        match self.store.store_text(&intent.transaction_id, text) {
            Ok(_) => {
                attempts.push(DeliveryAttempt {
                    backend: "inbox".to_owned(),
                    outcome: DeliveryOutcome::InboxFallback,
                    fallback_reason: None,
                    at_unix_ms: now_ms(),
                });
                Ok(DeliveryReport {
                    transaction_id: intent.transaction_id.clone(),
                    backend: "wtype-then-inbox".to_owned(),
                    attempts,
                    outcome: DeliveryOutcome::InboxFallback,
                    failure_reason: None,
                })
            }
            Err(err) => {
                let reason = match err {
                    crate::delivery::InboxError::InvalidTxid => "INBOX_TXID_INVALID",
                    crate::delivery::InboxError::Io(_) => "INBOX_WRITE_FAILED",
                };
                let failure = FallbackReason::BackendError {
                    code: reason.to_owned(),
                };
                attempts.push(DeliveryAttempt {
                    backend: "inbox".to_owned(),
                    outcome: DeliveryOutcome::Failed,
                    fallback_reason: Some(failure.clone()),
                    at_unix_ms: now_ms(),
                });
                Ok(DeliveryReport {
                    transaction_id: intent.transaction_id.clone(),
                    backend: "wtype-then-inbox".to_owned(),
                    attempts,
                    outcome: DeliveryOutcome::Failed,
                    failure_reason: Some(failure),
                })
            }
        }
    }
}

/// Actor bridge for one validated portal/EIS text frame. The caller's absolute
/// deadline must be forwarded unchanged to the daemon-owned portal actor.
pub trait PortalEisTextSubmitter: Send {
    fn submit(
        &mut self,
        payload: PortalEisTextPayload,
        cancellation: &SessionCancellation,
        deadline: Instant,
    ) -> PortalEisTextSubmission;
}

/// Phase 2C adapter: one portal/EIS attempt followed
/// by exactly one atomic Inbox recovery write under the same transaction ID.
/// Construction requires a reviewed submitter owner and one transaction-wide
/// absolute deadline; it cannot request portal permission itself.
pub struct PortalEisThenInboxDelivery<S> {
    store: InboxStore,
    submitter: S,
    deadline: Instant,
}

impl<S> PortalEisThenInboxDelivery<S> {
    #[must_use]
    pub fn new(dir: PathBuf, submitter: S, deadline: Instant) -> Self {
        Self {
            store: InboxStore::new(dir),
            submitter,
            deadline,
        }
    }

    #[cfg(test)]
    pub fn with_submitter_for_controlled_test(
        dir: PathBuf,
        submitter: S,
        deadline: Instant,
    ) -> Self {
        Self::new(dir, submitter, deadline)
    }
}

impl<S: PortalEisTextSubmitter> DeliveryPort for PortalEisThenInboxDelivery<S> {
    fn deliver(
        &mut self,
        intent: &DeliveryIntent,
        text: &str,
        cancellation: &SessionCancellation,
    ) -> Result<DeliveryReport, PlatformError> {
        if intent.backend != "portal-eis-then-inbox" {
            return Err(error("delivery_intent_invalid"));
        }
        if cancellation.is_cancelled() {
            return Err(error("cancelled"));
        }

        let submission = if Instant::now() >= self.deadline {
            PortalEisTextSubmission::RejectedBeforeDispatch(PortalEisTextFailure::TimedOut)
        } else {
            match PortalEisTextPayload::parse(text) {
                Ok(payload) => self.submitter.submit(payload, cancellation, self.deadline),
                Err(failure) => PortalEisTextSubmission::RejectedBeforeDispatch(failure),
            }
        };
        let direct = match submission {
            PortalEisTextSubmission::CancelledBeforeDispatch => return Err(error("cancelled")),
            PortalEisTextSubmission::SubmittedUnverified => DeliveryAttempt {
                backend: "portal-eis-text".to_owned(),
                outcome: DeliveryOutcome::SubmittedUnverified,
                fallback_reason: None,
                at_unix_ms: now_ms(),
            },
            PortalEisTextSubmission::RejectedBeforeDispatch(reason)
            | PortalEisTextSubmission::DispatchUncertain(reason) => DeliveryAttempt {
                backend: "portal-eis-text".to_owned(),
                outcome: DeliveryOutcome::Failed,
                fallback_reason: Some(FallbackReason::BackendError {
                    code: reason.code().to_owned(),
                }),
                at_unix_ms: now_ms(),
            },
        };

        let mut attempts = vec![direct];
        match self.store.store_text(&intent.transaction_id, text) {
            Ok(_) => {
                attempts.push(DeliveryAttempt {
                    backend: "inbox".to_owned(),
                    outcome: DeliveryOutcome::InboxFallback,
                    fallback_reason: None,
                    at_unix_ms: now_ms(),
                });
                Ok(DeliveryReport {
                    transaction_id: intent.transaction_id.clone(),
                    backend: "portal-eis-then-inbox".to_owned(),
                    attempts,
                    outcome: DeliveryOutcome::InboxFallback,
                    failure_reason: None,
                })
            }
            Err(err) => {
                let code = match err {
                    crate::delivery::InboxError::InvalidTxid => "INBOX_TXID_INVALID",
                    crate::delivery::InboxError::Io(_) => "INBOX_WRITE_FAILED",
                };
                let failure = FallbackReason::BackendError {
                    code: code.to_owned(),
                };
                attempts.push(DeliveryAttempt {
                    backend: "inbox".to_owned(),
                    outcome: DeliveryOutcome::Failed,
                    fallback_reason: Some(failure.clone()),
                    at_unix_ms: now_ms(),
                });
                Ok(DeliveryReport {
                    transaction_id: intent.transaction_id.clone(),
                    backend: "portal-eis-then-inbox".to_owned(),
                    attempts,
                    outcome: DeliveryOutcome::Failed,
                    failure_reason: Some(failure),
                })
            }
        }
    }
}

fn wtype_failure(code: &str) -> WtypeSubmission {
    WtypeSubmission::Failed(FallbackReason::BackendError {
        code: code.to_owned(),
    })
}

fn wtype_resolve_submission(error: ResolveError) -> WtypeSubmission {
    wtype_failure(match error {
        ResolveError::Missing => "WTYPE_MISSING",
        ResolveError::NonExecutable => "WTYPE_NONEXECUTABLE",
        ResolveError::Untrusted => "WTYPE_UNTRUSTED",
        ResolveError::IdentityChanged => "WTYPE_IDENTITY_CHANGED",
        ResolveError::UnsupportedPlatform => "WTYPE_ISOLATION_UNAVAILABLE",
    })
}

fn wtype_process_failure(process_error: ProcessError) -> WtypeSubmission {
    wtype_failure(match process_error {
        ProcessError::Wait(_) => "WTYPE_WAIT_FAILED",
        ProcessError::Signal(_) => "WTYPE_TERMINATION_FAILED",
        ProcessError::UnexpectedDescendant => "WTYPE_UNEXPECTED_DESCENDANT",
        ProcessError::TerminationTimeout => "WTYPE_TERMINATION_TIMEOUT",
    })
}

fn terminate_wtype_or_failure(
    child: &mut std::process::Child,
    deadline: Instant,
    code: &str,
) -> WtypeSubmission {
    match terminate_group_and_reap(child, deadline) {
        Ok(()) => wtype_failure(code),
        Err(process_error) => wtype_process_failure(process_error),
    }
}

fn cleanup_window_open(deadline: Instant) -> bool {
    // This reserve is part of the original absolute deadline, not an extra
    // grace period. It gives SIGKILL/try_wait a bounded chance to complete.
    const REAP_RESERVE: Duration = Duration::from_millis(50);
    deadline
        .checked_sub(REAP_RESERVE)
        .is_none_or(|cleanup_start| Instant::now() >= cleanup_start)
}

fn error(code: &'static str) -> PlatformError {
    PlatformError {
        code,
        message: code.to_owned(),
    }
}

fn pipewire_resolve_platform_error(resolve_error: ResolveError) -> PlatformError {
    error(match resolve_error {
        ResolveError::Missing => "pipewire_recorder_missing",
        ResolveError::NonExecutable => "pipewire_recorder_nonexecutable",
        ResolveError::Untrusted => "pipewire_recorder_untrusted",
        ResolveError::IdentityChanged => "pipewire_recorder_identity_changed",
        ResolveError::UnsupportedPlatform => "pipewire_recorder_isolation_unavailable",
    })
}

fn wtype_resolve_platform_error(resolve_error: ResolveError) -> PlatformError {
    error(match resolve_error {
        ResolveError::Missing => "wtype_missing",
        ResolveError::NonExecutable => "wtype_nonexecutable",
        ResolveError::Untrusted => "wtype_untrusted",
        ResolveError::IdentityChanged => "wtype_identity_changed",
        ResolveError::UnsupportedPlatform => "wtype_isolation_unavailable",
    })
}

fn resolve_pipewire_recorder() -> Result<TrustedExecutable, ResolveError> {
    TrustedExecutable::resolve_system(std::ffi::OsStr::new("pw-record"))
}

fn resolve_wtype_submitter() -> Result<TrustedExecutable, ResolveError> {
    TrustedExecutable::resolve_system(std::ffi::OsStr::new("wtype"))
}

pub fn pipewire_recorder_status() -> PipeWireRecorderStatus {
    pipewire_recorder_status_from(resolve_pipewire_recorder())
}

pub fn wtype_submitter_status() -> WtypeSubmitterStatus {
    wtype_submitter_status_from(resolve_wtype_submitter())
}

fn wtype_submitter_status_from(
    resolved: Result<TrustedExecutable, ResolveError>,
) -> WtypeSubmitterStatus {
    match resolved {
        Ok(executable) => WtypeSubmitterStatus::Available(executable.launch_path().to_owned()),
        Err(ResolveError::Missing) => WtypeSubmitterStatus::Missing,
        Err(ResolveError::NonExecutable) => WtypeSubmitterStatus::NonExecutable,
        Err(ResolveError::Untrusted) => WtypeSubmitterStatus::Untrusted,
        Err(ResolveError::IdentityChanged) => WtypeSubmitterStatus::IdentityChanged,
        Err(ResolveError::UnsupportedPlatform) => WtypeSubmitterStatus::UnsupportedPlatform,
    }
}

fn pipewire_recorder_status_from(
    resolved: Result<TrustedExecutable, ResolveError>,
) -> PipeWireRecorderStatus {
    match resolved {
        Ok(executable) => PipeWireRecorderStatus::Available(executable.launch_path().to_owned()),
        Err(ResolveError::Missing) => PipeWireRecorderStatus::Missing,
        Err(ResolveError::NonExecutable) => PipeWireRecorderStatus::NonExecutable,
        Err(ResolveError::Untrusted) => PipeWireRecorderStatus::Untrusted,
        Err(ResolveError::IdentityChanged) => PipeWireRecorderStatus::IdentityChanged,
        Err(ResolveError::UnsupportedPlatform) => PipeWireRecorderStatus::UnsupportedPlatform,
    }
}

fn set_nonblocking(stdout: &ChildStdout) -> rustix::io::Result<()> {
    let current = rustix::fs::fcntl_getfl(stdout)?;
    rustix::fs::fcntl_setfl(stdout, current | rustix::fs::OFlags::NONBLOCK)
}

fn capture_process_error(process_error: ProcessError) -> PlatformError {
    error(match process_error {
        ProcessError::Wait(_) => "pipewire_wait_failed",
        ProcessError::Signal(_) => "capture_termination_failed",
        ProcessError::UnexpectedDescendant => "capture_unexpected_descendant",
        ProcessError::TerminationTimeout => "capture_termination_timeout",
    })
}

fn decode_raw_f32(
    mut bytes: &[u8],
    carry: &mut [u8; 4],
    carry_len: &mut usize,
    samples: &mut Vec<f32>,
    max_samples: usize,
) -> Result<(), PlatformError> {
    if *carry_len != 0 {
        let needed = 4 - *carry_len;
        let taken = needed.min(bytes.len());
        carry[*carry_len..*carry_len + taken].copy_from_slice(&bytes[..taken]);
        *carry_len += taken;
        bytes = &bytes[taken..];
        if *carry_len == 4 {
            push_f32_sample(*carry, samples, max_samples)?;
            *carry_len = 0;
        }
    }
    let mut chunks = bytes.chunks_exact(4);
    for chunk in &mut chunks {
        push_f32_sample(
            [chunk[0], chunk[1], chunk[2], chunk[3]],
            samples,
            max_samples,
        )?;
    }
    let remainder = chunks.remainder();
    carry[..remainder.len()].copy_from_slice(remainder);
    *carry_len = remainder.len();
    Ok(())
}

fn push_f32_sample(
    bytes: [u8; 4],
    samples: &mut Vec<f32>,
    max_samples: usize,
) -> Result<(), PlatformError> {
    if samples.len() >= max_samples {
        return Err(error("capture_output_overflow"));
    }
    let sample = f32::from_le_bytes(bytes);
    if !sample.is_finite() {
        return Err(error("capture_output_nonfinite"));
    }
    samples.push(sample);
    Ok(())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn expected_budget_exit(
    status_code: Option<i32>,
    observed_samples: usize,
    expected_samples: usize,
) -> bool {
    status_code == Some(1) && observed_samples == expected_samples
}

#[cfg(test)]
mod tests {
    use super::*;
    use narrivy_platform_api::DeliveryPort;
    use narrivy_types::TransactionId;
    use std::error::Error;
    use std::io;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::sync::{Arc, Barrier, Mutex, MutexGuard, mpsc};
    use std::thread::JoinHandle;

    type TestResult<T = ()> = Result<T, Box<dyn Error>>;

    fn expected_error<T, E>(result: Result<T, E>, context: &str) -> TestResult<E> {
        match result {
            Ok(_) => Err(io::Error::other(context).into()),
            Err(error) => Ok(error),
        }
    }

    fn mutex_lock<T>(mutex: &Mutex<T>) -> TestResult<MutexGuard<'_, T>> {
        mutex
            .lock()
            .map_err(|_| io::Error::other("controlled test mutex was poisoned").into())
    }

    fn join_thread<T>(handle: JoinHandle<T>) -> TestResult<T> {
        handle
            .join()
            .map_err(|_| io::Error::other("controlled test thread panicked").into())
    }

    struct ControlledWtypeSubmitter {
        outcome: WtypeSubmission,
        payloads: Arc<Mutex<Vec<Vec<u8>>>>,
    }

    impl WtypeSubmitter for ControlledWtypeSubmitter {
        fn submit(&mut self, payload: &[u8], _: &SessionCancellation) -> WtypeSubmission {
            self.payloads
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(payload.to_vec());
            self.outcome.clone()
        }
    }

    struct ControlledPortalSubmitter {
        outcome: PortalEisTextSubmission,
        payloads: Arc<Mutex<Vec<Vec<u8>>>>,
        expected_deadline: Option<Instant>,
    }

    impl PortalEisTextSubmitter for ControlledPortalSubmitter {
        fn submit(
            &mut self,
            payload: PortalEisTextPayload,
            _: &SessionCancellation,
            deadline: Instant,
        ) -> PortalEisTextSubmission {
            if let Some(expected) = self.expected_deadline {
                assert_eq!(deadline, expected);
            }
            self.payloads
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(payload.as_bytes().to_vec());
            self.outcome
        }
    }

    #[test]
    fn inbox_only_reports_real_backend_and_transaction() -> TestResult {
        let dir = tempfile::tempdir()?;
        let cancel = SessionCancellation::new();
        let tx = TransactionId::new("tx-inbox");
        let intent = DeliveryIntent {
            transaction_id: tx.clone(),
            backend: "inbox".to_owned(),
        };
        let report = InboxOnlyDelivery::new(dir.path().to_path_buf())
            .deliver(&intent, "controlled", &cancel)
            .map_err(|error| io::Error::other(format!("Inbox delivery failed: {error:?}")))?;
        assert_eq!(report.transaction_id, tx);
        assert_eq!(report.backend, "inbox");
        assert_eq!(report.outcome, DeliveryOutcome::Delivered);
        Ok(())
    }

    #[test]
    fn expected_pw_record_budget_exit_requires_exact_valid_frame_count() {
        let expected = 16_000usize;
        assert!(expected_budget_exit(Some(1), expected, expected));
        assert!(!expected_budget_exit(Some(1), expected - 1, expected));
        assert!(!expected_budget_exit(Some(2), expected, expected));
    }

    #[test]
    fn fractional_capture_window_preserves_its_exact_frame_budget() {
        assert_eq!(
            capture_frame_budget(1.5).map_err(|error| error.code),
            Ok(24_000)
        );
        assert_eq!(
            capture_frame_budget(1.000_062_5).map_err(|error| error.code),
            Ok(16_001)
        );
        assert_eq!(
            capture_frame_budget(f32::NAN).map_err(|error| error.code),
            Err("capture_duration_invalid")
        );
    }

    #[test]
    fn documented_maximum_window_is_accepted_and_extremes_are_rejected() {
        // The production default is the documented maximum, not the historical
        // three-second probe window.
        assert_eq!(narrivy_types::MAX_CAPTURE_SECONDS, 300);
        assert_eq!(
            validate_capture_window(max_capture_seconds()).map_err(|error| error.code),
            Ok(())
        );
        assert_eq!(
            capture_frame_budget(max_capture_seconds()).map_err(|error| error.code),
            Ok(4_800_000)
        );
        assert_eq!(
            usize::try_from(4_800_000).ok(),
            Some(narrivy_types::MAX_CAPTURE_SAMPLES)
        );
        // Extremes and boundary escapes fail before any process side effect.
        for rejected in [
            0.0_f32,
            0.999,
            max_capture_seconds() + 0.001,
            f32::NAN,
            f32::INFINITY,
            f32::MAX,
            -1.0,
        ] {
            assert_eq!(
                validate_capture_window(rejected).map_err(|error| error.code),
                Err("capture_duration_invalid"),
                "window {rejected} must be rejected"
            );
            assert_eq!(
                capture_frame_budget(rejected).map_err(|error| error.code),
                Err("capture_duration_invalid"),
                "frame budget for {rejected} must be rejected"
            );
        }
        // The adapter accepts any whole or fractional window inside the
        // documented maximum; the daemon's `NARRIVY_CAPTURE_SECONDS` probe cap
        // is a configuration policy, not a second adapter bound.
        assert_eq!(
            capture_frame_budget(30.0).map_err(|error| error.code),
            Ok(480_000)
        );
        assert_eq!(
            capture_frame_budget(30.5).map_err(|error| error.code),
            Ok(488_000)
        );
    }

    #[test]
    fn ordinary_batch_status_poll_does_not_arm_cleanup_deadline() -> TestResult {
        let mut child = Command::new("/bin/sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("controlled stdout missing"))?;
        let started_at = Instant::now();
        let mut session = PipeWireCaptureSession {
            child: Some(child),
            stdout: Some(stdout),
            info: CaptureStreamInfo {
                source_selection: PIPEWIRE_TARGET.to_owned(),
                sample_rate_hz: CAPTURE_RATE_HZ,
                channels: CAPTURE_CHANNELS,
            },
            expected_samples: usize::try_from(CAPTURE_RATE_HZ)?,
            samples: Vec::new(),
            carry: [0; 4],
            carry_len: 0,
            started_at,
            first_frame_at: None,
            safety_deadline: started_at + Duration::from_secs(5),
            cleanup_deadline: None,
            completed: false,
        };
        assert!(
            session
                .reap_completed_child()
                .map_err(|error| io::Error::other(error.code))?
                .is_none()
        );
        assert!(session.cleanup_deadline.is_none());
        session
            .stop_child(Instant::now() + CAPTURE_CANCEL_BOUND)
            .map_err(|error| io::Error::other(error.code))?;
        session.completed = true;
        session.child.take();
        session.stdout.take();
        Ok(())
    }

    #[test]
    fn pipewire_recorder_readiness_preserves_every_trust_failure() {
        assert_eq!(
            pipewire_recorder_status_from(Err(ResolveError::Missing)),
            PipeWireRecorderStatus::Missing
        );
        assert_eq!(
            pipewire_recorder_status_from(Err(ResolveError::NonExecutable)),
            PipeWireRecorderStatus::NonExecutable
        );
        assert_eq!(
            pipewire_recorder_status_from(Err(ResolveError::Untrusted)),
            PipeWireRecorderStatus::Untrusted
        );
        assert_eq!(
            pipewire_recorder_status_from(Err(ResolveError::IdentityChanged)),
            PipeWireRecorderStatus::IdentityChanged
        );
        assert_eq!(
            pipewire_recorder_status_from(Err(ResolveError::UnsupportedPlatform)),
            PipeWireRecorderStatus::UnsupportedPlatform
        );
    }

    #[test]
    fn wtype_submitter_readiness_preserves_every_trust_failure() {
        assert_eq!(
            wtype_submitter_status_from(Err(ResolveError::Missing)),
            WtypeSubmitterStatus::Missing
        );
        assert_eq!(
            wtype_submitter_status_from(Err(ResolveError::NonExecutable)),
            WtypeSubmitterStatus::NonExecutable
        );
        assert_eq!(
            wtype_submitter_status_from(Err(ResolveError::Untrusted)),
            WtypeSubmitterStatus::Untrusted
        );
        assert_eq!(
            wtype_submitter_status_from(Err(ResolveError::IdentityChanged)),
            WtypeSubmitterStatus::IdentityChanged
        );
        assert_eq!(
            wtype_submitter_status_from(Err(ResolveError::UnsupportedPlatform)),
            WtypeSubmitterStatus::UnsupportedPlatform
        );
    }

    fn controlled_pw_record_program(dir: &Path, marker: &Path) -> TestResult<PathBuf> {
        let program = dir.join("pw-record");
        let argv = dir.join("capture.argv");
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\necho $$ > '{}'\nprintf '%s\\n' \"$@\" > '{}'\nprintf '\\000\\000\\000\\077'\nexec sleep 10\n",
                marker.display(),
                argv.display()
            ),
        )?;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700))?;
        Ok(program)
    }

    fn controlled_stream_program(
        dir: &Path,
        marker: &Path,
        total_samples: usize,
        delay_ms: u64,
        tail_seconds: u64,
    ) -> TestResult<PathBuf> {
        let program = dir.join("pw-record-stream");
        std::fs::write(
            &program,
            format!(
                "#!/usr/bin/python3\nimport os, struct, sys, time\nwith open({marker:?}, 'w', encoding='ascii') as out:\n    out.write(str(os.getpid()))\nblock = struct.pack('<f', 0.25) * {chunk_samples}\nremaining = {total_samples}\nwhile remaining:\n    take = min({chunk_samples}, remaining)\n    sys.stdout.buffer.write(block[:take * 4])\n    sys.stdout.buffer.flush()\n    remaining -= take\n    time.sleep({delay})\ntime.sleep({tail_seconds})\n",
                marker = marker.as_os_str(),
                chunk_samples = STREAM_CAPTURE_CHUNK_SAMPLES,
                delay = delay_ms as f64 / 1_000.0,
            ),
        )?;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700))?;
        Ok(program)
    }

    fn streaming_capture_for_test(
        program: PathBuf,
        boundary: Option<StreamControlBoundary>,
    ) -> PipeWireCapture {
        streaming_capture_for_window(program, 1.0, boundary)
    }

    fn streaming_capture_for_window(
        program: PathBuf,
        seconds: f32,
        boundary: Option<StreamControlBoundary>,
    ) -> PipeWireCapture {
        PipeWireCapture {
            seconds,
            target: PIPEWIRE_TARGET.to_owned(),
            helper: StrictHelper::Controlled(program),
            stream_control_boundary: boundary,
            producer_after_first_send: None,
        }
    }

    /// Controlled paced recorder for batch tests. It emits `total_samples`
    /// raw f32 frames in bounded writes, then records completion in
    /// `done_path` and stays alive so the test owns the Stop/terminate
    /// decision. No wall-clock sleep is needed to produce over-thirty-second
    /// sample counts.
    fn controlled_paced_batch_program(
        dir: &Path,
        pid_path: &Path,
        done_path: &Path,
        total_samples: usize,
        exit_code: i32,
        tail_seconds: u64,
    ) -> TestResult<PathBuf> {
        let program = dir.join("pw-record-paced");
        std::fs::write(
            &program,
            format!(
                "#!/usr/bin/python3\nimport os, struct, sys, time\nwith open({pid:?}, 'w', encoding='ascii') as out:\n    out.write(str(os.getpid()))\nblock = struct.pack('<f', 0.25) * 4096\nremaining = {total}\nwhile remaining:\n    take = min(4096, remaining)\n    sys.stdout.buffer.write(block[:take * 4])\n    sys.stdout.buffer.flush()\n    remaining -= take\nwith open({done:?}, 'w', encoding='ascii') as out:\n    out.write('done')\ntime.sleep({tail})\nsys.exit({code})\n",
                pid = pid_path.as_os_str(),
                done = done_path.as_os_str(),
                total = total_samples,
                tail = tail_seconds,
                code = exit_code,
            ),
        )?;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700))?;
        Ok(program)
    }

    fn wait_for_controlled_file(path: &Path, bound: Duration) -> TestResult {
        let deadline = Instant::now() + bound;
        while Instant::now() < deadline {
            if path.is_file() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        Err(io::Error::other(format!(
            "controlled recorder did not signal {}",
            path.display()
        ))
        .into())
    }

    #[test]
    fn finalization_returns_prefetched_audio_and_reaps_controlled_child() -> TestResult {
        let dir = tempfile::tempdir()?;
        let marker = dir.path().join("child.pid");
        let program = controlled_pw_record_program(dir.path(), &marker)?;
        let control = SessionControl::new();
        let child_control = control.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            let mut capture = PipeWireCapture {
                seconds: 1.0,
                target: PIPEWIRE_TARGET.to_owned(),
                helper: StrictHelper::Controlled(program),
                stream_control_boundary: None,
                producer_after_first_send: None,
            };
            let result = capture.start().and_then(|mut session| {
                session.wait_ready(&child_control)?;
                let _ = ready_tx.send(());
                session.collect(&child_control)
            });
            let _ = done_tx.send(result);
        });
        ready_rx.recv_timeout(TEST_STREAM_READY_BOUND)?;
        control.request_finalization();
        let captured = done_rx
            .recv_timeout(CAPTURE_CANCEL_BOUND + Duration::from_secs(1))?
            .map_err(|error| io::Error::other(error.code))?;
        join_thread(handle)?;
        assert_eq!(captured.end, CaptureEnd::FinalizationRequested);
        assert_eq!(captured.audio.samples, [0.5]);
        assert_eq!(captured.audio.sample_rate_hz, CAPTURE_RATE_HZ);
        assert_eq!(captured.audio.channels, CAPTURE_CHANNELS);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("capture.argv"))?
                .lines()
                .collect::<Vec<_>>(),
            [
                "--raw",
                "--rate",
                "16000",
                "--channels",
                "1",
                "--format",
                "f32",
                "--target",
                PIPEWIRE_TARGET,
                "--latency",
                CAPTURE_LATENCY,
                "-n",
                "16000",
                "-",
            ]
        );
        assert_pid_is_gone(&marker, "finalized recorder")?;
        Ok(())
    }

    #[test]
    fn cancellation_wins_after_readiness_and_reaps_controlled_child() -> TestResult {
        let dir = tempfile::tempdir()?;
        let marker = dir.path().join("child.pid");
        let program = controlled_pw_record_program(dir.path(), &marker)?;
        let control = SessionControl::new();
        let child_control = control.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            let mut capture = PipeWireCapture {
                seconds: 1.0,
                target: PIPEWIRE_TARGET.to_owned(),
                helper: StrictHelper::Controlled(program),
                stream_control_boundary: None,
                producer_after_first_send: None,
            };
            let result = capture.start().and_then(|mut session| {
                session.wait_ready(&child_control)?;
                let _ = ready_tx.send(());
                session.collect(&child_control)
            });
            let _ = done_tx.send(result);
        });
        ready_rx.recv_timeout(TEST_STREAM_READY_BOUND)?;
        control.request_finalization();
        control.cancel();
        let error = expected_error(
            done_rx.recv_timeout(CAPTURE_CANCEL_BOUND + Duration::from_secs(1))?,
            "cancelled capture unexpectedly succeeded",
        )?;
        assert_eq!(error.code, "cancelled");
        join_thread(handle)?;
        assert_pid_is_gone(&marker, "cancelled recorder")?;
        Ok(())
    }

    #[test]
    fn early_source_end_is_typed_result_and_child_is_reaped() -> TestResult {
        let dir = tempfile::tempdir()?;
        let marker = dir.path().join("child.pid");
        let program = dir.path().join("pw-record");
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\necho $$ > '{}'\nprintf '\\000\\000\\000\\077'\n",
                marker.display()
            ),
        )?;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700))?;
        let control = SessionControl::new();
        let mut capture = PipeWireCapture {
            seconds: 1.0,
            target: PIPEWIRE_TARGET.to_owned(),
            helper: StrictHelper::Controlled(program),
            stream_control_boundary: None,
            producer_after_first_send: None,
        };
        let mut session = capture
            .start()
            .map_err(|error| io::Error::other(error.code))?;
        session
            .wait_ready(&control)
            .map_err(|error| io::Error::other(error.code))?;
        // The recorder ends before its bound with valid frames, so the adapter
        // returns typed transaction truth; the portable slice journals the
        // reason and owns the fail-closed `capture_source_ended` outcome.
        let captured = session
            .collect(&control)
            .map_err(|error| io::Error::other(error.code))?;
        assert_eq!(captured.end, CaptureEnd::SourceEnded);
        assert_eq!(captured.audio.samples, [0.5]);
        assert_eq!(captured.audio.sample_rate_hz, CAPTURE_RATE_HZ);
        assert_eq!(captured.audio.channels, CAPTURE_CHANNELS);
        assert_pid_is_gone(&marker, "early-exit recorder")?;
        Ok(())
    }

    #[test]
    fn successful_pipewire_leader_with_live_descendant_fails_closed() -> TestResult {
        let dir = tempfile::tempdir()?;
        let descendant = dir.path().join("descendant.pid");
        let program = dir.path().join("pw-record");
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\nsleep 30 &\necho $! > '{}'\nprintf '\\000\\000\\000\\077'\nexit 0\n",
                descendant.display()
            ),
        )?;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700))?;

        let mut capture = PipeWireCapture {
            seconds: 1.0,
            target: PIPEWIRE_TARGET.to_owned(),
            helper: StrictHelper::Controlled(program),
            stream_control_boundary: None,
            producer_after_first_send: None,
        };
        let control = SessionControl::new();
        let mut session = capture
            .start()
            .map_err(|error| io::Error::other(error.code))?;
        session
            .wait_ready(&control)
            .map_err(|error| io::Error::other(error.code))?;
        let failure = expected_error(
            session.collect(&control),
            "capture with a residual descendant unexpectedly succeeded",
        )?;
        assert_eq!(failure.code, "capture_unexpected_descendant");
        assert_pid_is_gone(&descendant, "residual PipeWire")?;
        Ok(())
    }

    #[test]
    fn batch_stop_retains_over_thirty_seconds_of_paced_samples() -> TestResult {
        let dir = tempfile::tempdir()?;
        let pid_path = dir.path().join("child.pid");
        let done_path = dir.path().join("child.done");
        // 40 s at 16 kHz: deliberately beyond the historical 30 s bound and
        // produced without any wall-clock speech-length sleep.
        let total = 640_000_usize;
        let program =
            controlled_paced_batch_program(dir.path(), &pid_path, &done_path, total, 0, 10)?;
        let mut capture = PipeWireCapture {
            seconds: max_capture_seconds(),
            target: PIPEWIRE_TARGET.to_owned(),
            helper: StrictHelper::Controlled(program),
            stream_control_boundary: None,
            producer_after_first_send: None,
        };
        let control = SessionControl::new();
        let mut session = capture
            .start()
            .map_err(|error| io::Error::other(error.code))?;
        session
            .wait_ready(&control)
            .map_err(|error| io::Error::other(error.code))?;
        let child_control = control.clone();
        let collect = std::thread::spawn(move || session.collect(&child_control));
        wait_for_controlled_file(&done_path, Duration::from_secs(20))?;
        control.request_finalization();
        let captured = join_thread(collect)?.map_err(|error| io::Error::other(error.code))?;
        assert_eq!(captured.end, CaptureEnd::FinalizationRequested);
        assert_eq!(captured.audio.samples.len(), total);
        assert!(captured.audio.samples.iter().all(|sample| *sample == 0.25));
        assert_eq!(captured.audio.sample_rate_hz, CAPTURE_RATE_HZ);
        assert_eq!(captured.audio.channels, CAPTURE_CHANNELS);
        assert_pid_is_gone(&pid_path, "stopped over-thirty-second recorder")?;
        Ok(())
    }

    #[test]
    fn maximum_bounded_window_terminates_explicitly_with_every_sample() -> TestResult {
        let dir = tempfile::tempdir()?;
        let pid_path = dir.path().join("child.pid");
        let done_path = dir.path().join("child.done");
        let total = narrivy_types::MAX_CAPTURE_SAMPLES;
        let program =
            controlled_paced_batch_program(dir.path(), &pid_path, &done_path, total, 0, 0)?;
        let mut capture = PipeWireCapture {
            seconds: max_capture_seconds(),
            target: PIPEWIRE_TARGET.to_owned(),
            helper: StrictHelper::Controlled(program),
            stream_control_boundary: None,
            producer_after_first_send: None,
        };
        let control = SessionControl::new();
        let mut session = capture
            .start()
            .map_err(|error| io::Error::other(error.code))?;
        session
            .wait_ready(&control)
            .map_err(|error| io::Error::other(error.code))?;
        let captured = session
            .collect(&control)
            .map_err(|error| io::Error::other(error.code))?;
        assert_eq!(captured.end, CaptureEnd::SafetyLimit);
        assert_eq!(captured.audio.samples.len(), total);
        assert_eq!(captured.audio.sample_rate_hz, CAPTURE_RATE_HZ);
        assert_eq!(captured.audio.channels, CAPTURE_CHANNELS);
        assert_pid_is_gone(&pid_path, "maximum-window recorder")?;
        Ok(())
    }

    #[test]
    fn raw_f32_decoder_handles_split_frames_and_rejects_invalid_data() -> TestResult {
        let values = [0.25_f32, -0.5_f32];
        let bytes: Vec<u8> = values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        let mut carry = [0_u8; 4];
        let mut carry_len = 0;
        let mut samples = Vec::new();
        decode_raw_f32(&bytes[..3], &mut carry, &mut carry_len, &mut samples, 2)
            .map_err(|error| io::Error::other(error.code))?;
        decode_raw_f32(&bytes[3..], &mut carry, &mut carry_len, &mut samples, 2)
            .map_err(|error| io::Error::other(error.code))?;
        assert_eq!(samples, values);
        assert_eq!(carry_len, 0);

        assert_eq!(
            expected_error(
                decode_raw_f32(
                    &f32::NAN.to_le_bytes(),
                    &mut [0; 4],
                    &mut 0,
                    &mut Vec::new(),
                    1,
                ),
                "non-finite sample unexpectedly accepted",
            )?
            .code,
            "capture_output_nonfinite"
        );
        assert_eq!(
            expected_error(
                decode_raw_f32(&0_f32.to_le_bytes(), &mut [0; 4], &mut 0, &mut vec![0.0], 1,),
                "overflow sample unexpectedly accepted",
            )?
            .code,
            "capture_output_overflow"
        );
        Ok(())
    }

    #[test]
    fn bounded_stream_emits_contiguous_chunks_then_exact_safety_summary() -> TestResult {
        let dir = tempfile::tempdir()?;
        let marker = dir.path().join("stream.pid");
        let program = controlled_stream_program(dir.path(), &marker, 16_000, 3, 0)?;
        let mut capture = streaming_capture_for_test(program, None);
        let control = SessionControl::new();
        let mut session = capture
            .start_stream(&control)
            .map_err(|error| io::Error::other(error.code))?;
        session
            .wait_ready(&control)
            .map_err(|error| io::Error::other(error.code))?;

        let mut total = 0_usize;
        let mut expected_sequence = 0_u64;
        loop {
            match session
                .next_event(&control)
                .map_err(|error| io::Error::other(error.code))?
            {
                CaptureStreamEvent::Audio(chunk) => {
                    assert_eq!(chunk.sequence, expected_sequence);
                    assert!(chunk.audio.samples.len() <= STREAM_CAPTURE_CHUNK_SAMPLES);
                    assert_eq!(chunk.audio.sample_rate_hz, CAPTURE_RATE_HZ);
                    assert_eq!(chunk.audio.channels, CAPTURE_CHANNELS);
                    total += chunk.audio.samples.len();
                    expected_sequence += 1;
                }
                CaptureStreamEvent::End(summary) => {
                    assert_eq!(summary.end, CaptureEnd::SafetyLimit);
                    assert_eq!(summary.total_samples, 16_000);
                    break;
                }
            }
        }
        assert_eq!(total, 16_000);
        assert_eq!(expected_sequence, 13);
        assert_pid_is_gone(&marker, "streaming safety-limit recorder")?;
        Ok(())
    }

    #[test]
    fn streaming_window_above_thirty_seconds_delivers_every_sample_once() -> TestResult {
        let dir = tempfile::tempdir()?;
        let marker = dir.path().join("stream.pid");
        let window_seconds = 40.0_f32;
        let total = usize::try_from(
            capture_frame_budget(window_seconds).map_err(|error| io::Error::other(error.code))?,
        )?;
        assert!(
            total > 480_000,
            "regression must exceed the historical 30 s capture bound"
        );
        let program = controlled_stream_program(dir.path(), &marker, total, 2, 0)?;
        let mut capture = streaming_capture_for_window(program, window_seconds, None);
        let control = SessionControl::new();
        let mut session = capture
            .start_stream(&control)
            .map_err(|error| io::Error::other(error.code))?;
        session
            .wait_ready(&control)
            .map_err(|error| io::Error::other(error.code))?;

        let mut observed = 0_usize;
        let mut expected_sequence = 0_u64;
        loop {
            match session
                .next_event(&control)
                .map_err(|error| io::Error::other(error.code))?
            {
                CaptureStreamEvent::Audio(chunk) => {
                    assert_eq!(chunk.sequence, expected_sequence);
                    assert!(!chunk.audio.samples.is_empty());
                    assert!(chunk.audio.samples.len() <= STREAM_CAPTURE_CHUNK_SAMPLES);
                    assert!(chunk.audio.samples.iter().all(|sample| *sample == 0.25));
                    observed += chunk.audio.samples.len();
                    expected_sequence += 1;
                }
                CaptureStreamEvent::End(summary) => {
                    // The documented bound is explicit, not a silent clip.
                    assert_eq!(summary.end, CaptureEnd::SafetyLimit);
                    assert_eq!(summary.total_samples, u64::try_from(total)?);
                    break;
                }
            }
        }
        assert_eq!(observed, total);
        assert_pid_is_gone(&marker, "over-thirty-second streaming recorder")?;
        Ok(())
    }

    #[test]
    fn bounded_stream_finalization_drains_audio_and_reaps_child() -> TestResult {
        let dir = tempfile::tempdir()?;
        let marker = dir.path().join("stream.pid");
        let program = controlled_stream_program(
            dir.path(),
            &marker,
            STREAM_CAPTURE_CHUNK_SAMPLES * 2,
            2,
            10,
        )?;
        let mut capture = streaming_capture_for_test(program, None);
        let control = SessionControl::new();
        let mut session = capture
            .start_stream(&control)
            .map_err(|error| io::Error::other(error.code))?;
        session
            .wait_ready(&control)
            .map_err(|error| io::Error::other(error.code))?;
        control.request_finalization();

        let mut total = 0_u64;
        loop {
            match session
                .next_event(&control)
                .map_err(|error| io::Error::other(error.code))?
            {
                CaptureStreamEvent::Audio(chunk) => {
                    total += u64::try_from(chunk.audio.samples.len())?;
                }
                CaptureStreamEvent::End(summary) => {
                    assert_eq!(summary.end, CaptureEnd::FinalizationRequested);
                    assert_eq!(summary.total_samples, total);
                    assert!(summary.first_frame_after_start_us.is_some());
                    break;
                }
            }
        }
        assert!(total >= u64::try_from(STREAM_CAPTURE_CHUNK_SAMPLES)?);
        assert_pid_is_gone(&marker, "finalized streaming recorder")?;
        Ok(())
    }

    #[test]
    fn bounded_stream_preserves_endpoint_finalization_origin() -> TestResult {
        let dir = tempfile::tempdir()?;
        let marker = dir.path().join("stream.pid");
        let program = controlled_stream_program(
            dir.path(),
            &marker,
            STREAM_CAPTURE_CHUNK_SAMPLES * 2,
            2,
            10,
        )?;
        let mut capture = streaming_capture_for_test(program, None);
        let control = SessionControl::new();
        let mut session = capture
            .start_stream(&control)
            .map_err(|error| io::Error::other(error.code))?;
        session
            .wait_ready(&control)
            .map_err(|error| io::Error::other(error.code))?;
        assert!(control.request_endpoint_finalization());

        loop {
            match session
                .next_event(&control)
                .map_err(|error| io::Error::other(error.code))?
            {
                CaptureStreamEvent::Audio(_) => {}
                CaptureStreamEvent::End(summary) => {
                    assert_eq!(summary.end, CaptureEnd::EndpointDetected);
                    break;
                }
            }
        }
        assert_pid_is_gone(&marker, "endpoint-finalized streaming recorder")?;
        Ok(())
    }

    #[test]
    fn finalization_before_first_poll_preserves_already_queued_audio() -> TestResult {
        let dir = tempfile::tempdir()?;
        let marker = dir.path().join("stream.pid");
        let program =
            controlled_stream_program(dir.path(), &marker, STREAM_CAPTURE_CHUNK_SAMPLES, 0, 10)?;
        let (pause, gate) = dispatch_pause();
        let mut capture = PipeWireCapture {
            seconds: 1.0,
            target: PIPEWIRE_TARGET.to_owned(),
            helper: StrictHelper::Controlled(program),
            stream_control_boundary: None,
            producer_after_first_send: Some(pause),
        };
        let control = SessionControl::new();
        let mut session = capture
            .start_stream_owned(&control)
            .map_err(|error| io::Error::other(error.code))?;
        gate.wait_arrived("producer after-first-send boundary")?;
        control.request_finalization();
        gate.release();

        let first = session
            .next_event(&control)
            .map_err(|error| io::Error::other(error.code))?;
        let CaptureStreamEvent::Audio(first) = first else {
            return Err(io::Error::other("queued audio was not returned before End").into());
        };
        assert_eq!(first.sequence, 0);
        assert_eq!(first.audio.samples.len(), STREAM_CAPTURE_CHUNK_SAMPLES);
        let end = session
            .next_event(&control)
            .map_err(|error| io::Error::other(error.code))?;
        let CaptureStreamEvent::End(summary) = end else {
            return Err(io::Error::other("finalized stream emitted extra audio").into());
        };
        assert_eq!(summary.end, CaptureEnd::FinalizationRequested);
        assert_eq!(
            summary.total_samples,
            u64::try_from(STREAM_CAPTURE_CHUNK_SAMPLES)?
        );
        assert_pid_is_gone(&marker, "queued-finalization recorder")?;
        Ok(())
    }

    #[test]
    fn bounded_stream_abort_uses_caller_deadline_and_reaps_child() -> TestResult {
        let dir = tempfile::tempdir()?;
        let marker = dir.path().join("stream.pid");
        let program =
            controlled_stream_program(dir.path(), &marker, STREAM_CAPTURE_CHUNK_SAMPLES, 0, 10)?;
        let mut capture = streaming_capture_for_test(program, None);
        let control = SessionControl::new();
        let mut session = capture
            .start_stream(&control)
            .map_err(|error| io::Error::other(error.code))?;
        session
            .wait_ready(&control)
            .map_err(|error| io::Error::other(error.code))?;
        session
            .abort(Instant::now() + CAPTURE_CANCEL_BOUND)
            .map_err(|error| io::Error::other(error.code))?;
        assert_pid_is_gone(&marker, "aborted streaming recorder")?;
        Ok(())
    }

    #[test]
    fn bounded_stream_queue_overflow_is_terminal_and_never_drops_silently() -> TestResult {
        let dir = tempfile::tempdir()?;
        let marker = dir.path().join("stream.pid");
        let program = controlled_stream_program(
            dir.path(),
            &marker,
            (STREAM_CAPTURE_QUEUE_CHUNKS + 2) * STREAM_CAPTURE_CHUNK_SAMPLES,
            0,
            10,
        )?;
        let mut child = std::process::Command::new(program)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("controlled stream stdout missing"))?;
        set_nonblocking(&stdout)?;
        let producer_control = SessionControl::new();
        let terminal = Arc::new(AtomicU8::new(StreamProducerTerminal::Running as u8));
        let (sender, _receiver) = sync_channel(STREAM_CAPTURE_QUEUE_CHUNKS);
        let producer_terminal = Arc::clone(&terminal);
        let parser = std::thread::spawn(move || {
            stream_capture_producer(
                stdout,
                sender,
                producer_terminal,
                producer_control,
                None,
                Instant::now(),
            );
        });
        // The controlled program paces its chunks, so filling the queue takes
        // roughly one pacing interval per chunk.
        let deadline = Instant::now() + Duration::from_secs(10);
        while !parser.is_finished() && Instant::now() < deadline {
            std::thread::sleep(CAPTURE_READ_POLL);
        }
        assert!(
            parser.is_finished(),
            "producer did not report bounded overflow"
        );
        join_thread(parser)?;
        assert_eq!(
            StreamProducerTerminal::load(&terminal),
            StreamProducerTerminal::Backpressure
        );
        terminate_group_and_reap(&mut child, Instant::now() + Duration::from_secs(1))?;
        assert_pid_is_gone(&marker, "overflowed streaming recorder")?;
        Ok(())
    }

    #[test]
    fn session_surfaces_overflow_without_returning_queued_audio_and_reaps() -> TestResult {
        let dir = tempfile::tempdir()?;
        let marker = dir.path().join("stream.pid");
        let program = controlled_stream_program(
            dir.path(),
            &marker,
            (STREAM_CAPTURE_QUEUE_CHUNKS + 2) * STREAM_CAPTURE_CHUNK_SAMPLES,
            0,
            10,
        )?;
        let (pause, gate) = dispatch_pause();
        let mut capture = PipeWireCapture {
            seconds: 1.0,
            target: PIPEWIRE_TARGET.to_owned(),
            helper: StrictHelper::Controlled(program),
            stream_control_boundary: None,
            producer_after_first_send: Some(pause),
        };
        let control = SessionControl::new();
        let mut session = capture
            .start_stream_owned(&control)
            .map_err(|error| io::Error::other(error.code))?;
        gate.wait_arrived("session overflow producer boundary")?;
        session
            .wait_ready(&control)
            .map_err(|error| io::Error::other(error.code))?;
        assert!(session.pending.is_some());
        gate.release();
        let deadline = Instant::now() + Duration::from_secs(1);
        while StreamProducerTerminal::load(&session.terminal) == StreamProducerTerminal::Running
            && Instant::now() < deadline
        {
            std::thread::sleep(CAPTURE_READ_POLL);
        }
        assert_eq!(
            StreamProducerTerminal::load(&session.terminal),
            StreamProducerTerminal::Backpressure
        );
        let error = expected_error(
            session.next_event(&control),
            "overflowed stream unexpectedly returned its prefetched audio",
        )?;
        assert_eq!(error.code, "capture_backpressure");
        assert!(session.child.is_none());
        assert!(session.parser.is_none());
        assert_pid_is_gone(&marker, "session-overflow recorder")?;
        Ok(())
    }

    #[test]
    fn cancellation_discards_prefetched_readiness_chunk_and_reaps() -> TestResult {
        let dir = tempfile::tempdir()?;
        let marker = dir.path().join("stream.pid");
        let program =
            controlled_stream_program(dir.path(), &marker, STREAM_CAPTURE_CHUNK_SAMPLES, 0, 10)?;
        let mut capture = streaming_capture_for_test(program, None);
        let control = SessionControl::new();
        let mut session = capture
            .start_stream_owned(&control)
            .map_err(|error| io::Error::other(error.code))?;
        session
            .wait_ready(&control)
            .map_err(|error| io::Error::other(error.code))?;
        assert!(session.pending.is_some());
        control.cancel();
        let error = expected_error(
            session.next_event(&control),
            "cancelled prefetched chunk unexpectedly escaped",
        )?;
        assert_eq!(error.code, "cancelled");
        assert_pid_is_gone(&marker, "prefetch-cancel recorder")?;
        Ok(())
    }

    #[test]
    fn only_the_first_stream_chunk_carries_the_first_frame_time() -> TestResult {
        // Regression: the first-frame time was measured again after the first
        // chunk took it, so the second chunk carried one too and the portable
        // slice ended every multi-chunk stream as `capture_measurement_invalid`.
        let dir = tempfile::tempdir()?;
        let marker = dir.path().join("stream.pid");
        let program =
            controlled_stream_program(dir.path(), &marker, STREAM_CAPTURE_CHUNK_SAMPLES * 3, 0, 0)?;
        let mut capture = streaming_capture_for_test(program, None);
        let control = SessionControl::new();
        let mut session = capture
            .start_stream_owned(&control)
            .map_err(|error| io::Error::other(error.code))?;
        let mut first_frame = None;
        for sequence in 0..3_u64 {
            let event = session
                .next_event(&control)
                .map_err(|error| io::Error::other(error.code))?;
            let CaptureStreamEvent::Audio(chunk) = event else {
                return Err(io::Error::other("stream ended before three chunks").into());
            };
            assert_eq!(chunk.sequence, sequence);
            if sequence == 0 {
                assert!(chunk.first_frame_after_start_us.is_some());
                first_frame = chunk.first_frame_after_start_us;
            } else {
                assert_eq!(chunk.first_frame_after_start_us, None);
            }
        }
        let end = session
            .next_event(&control)
            .map_err(|error| io::Error::other(error.code))?;
        let CaptureStreamEvent::End(summary) = end else {
            return Err(io::Error::other("stream did not return a typed end").into());
        };
        assert_eq!(summary.first_frame_after_start_us, first_frame);
        assert_eq!(
            summary.total_samples,
            u64::try_from(STREAM_CAPTURE_CHUNK_SAMPLES * 3)?
        );
        assert_pid_is_gone(&marker, "multi-chunk streaming recorder")?;
        Ok(())
    }

    #[test]
    fn streaming_early_eof_returns_typed_source_ended_summary() -> TestResult {
        let dir = tempfile::tempdir()?;
        let marker = dir.path().join("stream.pid");
        let program =
            controlled_stream_program(dir.path(), &marker, STREAM_CAPTURE_CHUNK_SAMPLES, 0, 0)?;
        let mut capture = streaming_capture_for_test(program, None);
        let control = SessionControl::new();
        let mut session = capture
            .start_stream_owned(&control)
            .map_err(|error| io::Error::other(error.code))?;
        let first = session
            .next_event(&control)
            .map_err(|error| io::Error::other(error.code))?;
        let CaptureStreamEvent::Audio(first) = first else {
            return Err(io::Error::other("short natural stream ended before any audio").into());
        };
        assert_eq!(first.sequence, 0);
        assert_eq!(first.audio.samples.len(), STREAM_CAPTURE_CHUNK_SAMPLES);
        // The short EOF is typed transaction truth: the portable slice journals
        // this end reason and fails terminal through its owned cleanup path.
        let end = session
            .next_event(&control)
            .map_err(|error| io::Error::other(error.code))?;
        let CaptureStreamEvent::End(summary) = end else {
            return Err(io::Error::other("short natural stream did not return a typed end").into());
        };
        assert_eq!(summary.end, CaptureEnd::SourceEnded);
        assert_eq!(
            summary.total_samples,
            u64::try_from(STREAM_CAPTURE_CHUNK_SAMPLES)?
        );
        assert_eq!(
            summary.first_frame_after_start_us,
            first.first_frame_after_start_us
        );
        assert_pid_is_gone(&marker, "streaming early-EOF recorder")?;
        Ok(())
    }

    #[test]
    fn streaming_nonfinite_and_partial_frame_fail_closed_and_reap() -> TestResult {
        for (name, body) in [
            (
                "nonfinite",
                "sys.stdout.buffer.write(struct.pack('<f', float('nan')))\n",
            ),
            ("partial", "sys.stdout.buffer.write(b'\\x00\\x00\\x00')\n"),
        ] {
            let dir = tempfile::tempdir()?;
            let marker = dir.path().join("stream.pid");
            let program = dir.path().join("pw-record-stream-invalid");
            std::fs::write(
                &program,
                format!(
                    "#!/usr/bin/python3\nimport os, struct, sys\nwith open({marker:?}, 'w', encoding='ascii') as out:\n    out.write(str(os.getpid()))\n{body}sys.stdout.buffer.flush()\n",
                    marker = marker.as_os_str(),
                ),
            )?;
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700))?;
            let mut capture = streaming_capture_for_test(program, None);
            let control = SessionControl::new();
            let mut session = capture
                .start_stream_owned(&control)
                .map_err(|error| io::Error::other(error.code))?;
            let error = expected_error(
                session.wait_ready(&control),
                "invalid streaming payload unexpectedly reached readiness",
            )?;
            assert_eq!(error.code, "capture_output_invalid", "case={name}");
            assert_pid_is_gone(&marker, name)?;
        }
        Ok(())
    }

    #[test]
    fn cleanup_attempts_parser_join_even_when_child_misses_shared_deadline() -> TestResult {
        let child = std::process::Command::new("/bin/sleep")
            .arg("10")
            .process_group(0)
            .spawn()?;
        let parser = std::thread::spawn(|| {});
        while !parser.is_finished() {
            std::thread::yield_now();
        }
        let (_sender, receiver) = sync_channel(STREAM_CAPTURE_QUEUE_CHUNKS);
        let now = Instant::now();
        let mut session = PipeWireStreamingCaptureSession {
            child: Some(child),
            receiver: Some(receiver),
            parser: Some(parser),
            terminal: Arc::new(AtomicU8::new(StreamProducerTerminal::Running as u8)),
            info: CaptureStreamInfo {
                source_selection: "controlled".to_owned(),
                sample_rate_hz: CAPTURE_RATE_HZ,
                channels: CAPTURE_CHANNELS,
            },
            expected_samples: STREAM_CAPTURE_CHUNK_SAMPLES,
            delivered_samples: 0,
            expected_sequence: 0,
            first_frame_after_start_us: None,
            pending: None,
            started_at: now,
            safety_deadline: now,
            stop_reason: None,
            completed: false,
            control_boundary: None,
            force_child_cleanup_timeout: true,
        };
        let error = expected_error(
            session.terminate_owned_until(now + CAPTURE_CANCEL_BOUND),
            "forced child cleanup timeout unexpectedly succeeded",
        )?;
        assert_eq!(error.code, "capture_termination_timeout");
        assert!(
            session.parser.is_none(),
            "finished parser was not joined after child cleanup failure"
        );
        let deadline =
            checked_deadline(CAPTURE_CANCEL_BOUND).map_err(|error| io::Error::other(error.code))?;
        let child = session
            .child
            .as_mut()
            .ok_or_else(|| io::Error::other("unreaped controlled child was not retained"))?;
        terminate_group_and_reap(child, deadline)?;
        session.child.take();
        session.completed = true;
        Ok(())
    }

    #[test]
    fn producer_observes_finalization_without_consumer_polling() -> TestResult {
        let control = SessionControl::new();
        control.request_finalization();
        let (sender, receiver) = sync_channel(STREAM_CAPTURE_QUEUE_CHUNKS);
        let terminal = AtomicU8::new(StreamProducerTerminal::Running as u8);
        let mut samples = vec![0.25; 320];
        let mut first_frame = Some(7);

        assert!(producer_control_stop(
            &control,
            &sender,
            &terminal,
            0,
            &mut samples,
            &mut first_frame,
        ));
        assert!(samples.is_empty());
        assert_eq!(
            StreamProducerTerminal::load(&terminal),
            StreamProducerTerminal::Finalized
        );
        let emitted = receiver.recv_timeout(Duration::from_millis(100))?;
        assert_eq!(emitted.sequence, 0);
        assert_eq!(emitted.audio.samples.len(), 320);
        assert_eq!(emitted.first_frame_after_start_us, Some(7));
        Ok(())
    }

    #[test]
    fn producer_observes_cancellation_without_consumer_polling_and_discards_audio() -> TestResult {
        let control = SessionControl::new();
        control.cancel();
        let (sender, receiver) = sync_channel(STREAM_CAPTURE_QUEUE_CHUNKS);
        let terminal = AtomicU8::new(StreamProducerTerminal::Running as u8);
        let mut samples = vec![0.25; 320];
        let mut first_frame = Some(7);

        assert!(producer_control_stop(
            &control,
            &sender,
            &terminal,
            0,
            &mut samples,
            &mut first_frame,
        ));
        assert_eq!(samples.len(), 320, "cancelled audio must not be dispatched");
        assert!(receiver.try_recv().is_err());
        assert_eq!(
            StreamProducerTerminal::load(&terminal),
            StreamProducerTerminal::Cancelled
        );
        Ok(())
    }

    #[test]
    fn cancellation_after_receive_discards_chunk_and_reaps_child() -> TestResult {
        let dir = tempfile::tempdir()?;
        let marker = dir.path().join("stream.pid");
        let program =
            controlled_stream_program(dir.path(), &marker, STREAM_CAPTURE_CHUNK_SAMPLES, 0, 10)?;
        let (pause, gate) = dispatch_pause();
        let mut capture =
            streaming_capture_for_test(program, Some(StreamControlBoundary::AfterReceive(pause)));
        let control = SessionControl::new();
        let child_control = control.clone();
        let (result_tx, result_rx) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            let result = capture
                .start_stream(&child_control)
                .and_then(|mut session| session.next_event(&child_control).map(|_| ()));
            let _ = result_tx.send(result);
        });
        if let Err(error) = gate.wait_arrived("stream after-receive boundary") {
            // The worker failed before reaching the boundary (for example a
            // failed helper or worker-thread spawn). Cancel and release so the
            // recorder is never stranded, collect the worker's own failure,
            // then report the coordination failure. The gate's bounded backstop
            // means this path cannot wait forever.
            control.cancel();
            gate.release();
            let _ = result_rx.recv_timeout(CAPTURE_CANCEL_BOUND + Duration::from_secs(1));
            let _ = join_thread(handle);
            return Err(error);
        }
        control.cancel();
        gate.release();
        let error = expected_error(
            result_rx.recv_timeout(CAPTURE_CANCEL_BOUND + Duration::from_secs(1))?,
            "cancelled received chunk unexpectedly escaped",
        )?;
        assert_eq!(error.code, "cancelled");
        join_thread(handle)?;
        assert_pid_is_gone(&marker, "receive-race streaming recorder")?;
        Ok(())
    }

    #[test]
    fn controlled_wtype_submission_uses_exact_stdin_bytes_then_one_inbox_recovery() -> TestResult {
        let dir = tempfile::tempdir()?;
        let captured = Arc::new(Mutex::new(Vec::new()));
        let submitter = ControlledWtypeSubmitter {
            outcome: WtypeSubmission::SubmittedUnverified,
            payloads: Arc::clone(&captured),
        };
        let mut delivery = WtypeThenInboxDelivery::with_submitter_for_controlled_test(
            dir.path().to_path_buf(),
            submitter,
        );
        let txid = TransactionId::new("tx-wtype-inbox");
        let intent = DeliveryIntent {
            transaction_id: txid.clone(),
            backend: "wtype-then-inbox".to_owned(),
        };
        let payload = "-leading\nmultiline café";
        let report = delivery
            .deliver(&intent, payload, &SessionCancellation::new())
            .map_err(|error| io::Error::other(format!("controlled delivery failed: {error:?}")))?;
        assert_eq!(mutex_lock(&captured)?.as_slice(), [payload.as_bytes()]);
        assert_eq!(report.transaction_id, txid);
        assert_eq!(report.outcome, DeliveryOutcome::InboxFallback);
        assert_eq!(report.attempts.len(), 2);
        assert_eq!(
            report.attempts[0].outcome,
            DeliveryOutcome::SubmittedUnverified
        );
        assert_eq!(report.attempts[1].outcome, DeliveryOutcome::InboxFallback);
        assert!(
            !format!("{report:?}").contains(payload),
            "delivery report must not embed payload text"
        );
        assert_eq!(delivery.store.list().len(), 1);
        assert_eq!(delivery.store.read_text(&txid)?, payload);
        Ok(())
    }

    #[test]
    fn controlled_portal_submission_is_unverified_then_exactly_one_inbox_recovery() -> TestResult {
        let dir = tempfile::tempdir()?;
        let captured = Arc::new(Mutex::new(Vec::new()));
        let deadline = Instant::now() + Duration::from_secs(1);
        let submitter = ControlledPortalSubmitter {
            outcome: PortalEisTextSubmission::SubmittedUnverified,
            payloads: Arc::clone(&captured),
            expected_deadline: Some(deadline),
        };
        let mut delivery =
            PortalEisThenInboxDelivery::new(dir.path().to_path_buf(), submitter, deadline);
        let txid = TransactionId::new("tx-portal-inbox");
        let payload = "controlled portal frame";
        let report = delivery
            .deliver(
                &DeliveryIntent {
                    transaction_id: txid.clone(),
                    backend: "portal-eis-then-inbox".to_owned(),
                },
                payload,
                &SessionCancellation::new(),
            )
            .map_err(|error| io::Error::other(format!("controlled delivery failed: {error:?}")))?;

        assert_eq!(mutex_lock(&captured)?.as_slice(), [payload.as_bytes()]);
        assert_eq!(report.transaction_id, txid);
        assert_eq!(report.backend, "portal-eis-then-inbox");
        assert_eq!(report.outcome, DeliveryOutcome::InboxFallback);
        assert_eq!(report.attempts.len(), 2);
        assert_eq!(report.attempts[0].backend, "portal-eis-text");
        assert_eq!(
            report.attempts[0].outcome,
            DeliveryOutcome::SubmittedUnverified
        );
        assert_eq!(report.attempts[1].backend, "inbox");
        assert_eq!(delivery.store.list().len(), 1);
        assert_eq!(delivery.store.read_text(&txid)?, payload);
        assert!(!format!("{report:?}").contains(payload));
        Ok(())
    }

    #[test]
    fn controlled_portal_invalid_payload_fails_before_submit_and_recovers_to_inbox() -> TestResult {
        let dir = tempfile::tempdir()?;
        let captured = Arc::new(Mutex::new(Vec::new()));
        let submitter = ControlledPortalSubmitter {
            outcome: PortalEisTextSubmission::SubmittedUnverified,
            payloads: Arc::clone(&captured),
            expected_deadline: None,
        };
        let mut delivery = PortalEisThenInboxDelivery::with_submitter_for_controlled_test(
            dir.path().to_path_buf(),
            submitter,
            Instant::now() + Duration::from_secs(1),
        );
        let txid = TransactionId::new("tx-portal-invalid");
        let report = delivery
            .deliver(
                &DeliveryIntent {
                    transaction_id: txid.clone(),
                    backend: "portal-eis-then-inbox".to_owned(),
                },
                "multiline\nnot-admitted",
                &SessionCancellation::new(),
            )
            .map_err(|error| io::Error::other(format!("controlled delivery failed: {error:?}")))?;

        assert!(mutex_lock(&captured)?.is_empty());
        assert_eq!(report.attempts[0].outcome, DeliveryOutcome::Failed);
        assert_eq!(
            report.attempts[0].fallback_reason,
            Some(FallbackReason::BackendError {
                code: PortalEisTextFailure::PayloadUnsupported.code().to_owned(),
            })
        );
        assert_eq!(report.attempts[1].outcome, DeliveryOutcome::InboxFallback);
        assert_eq!(delivery.store.read_text(&txid)?, "multiline\nnot-admitted");
        Ok(())
    }

    #[test]
    fn controlled_portal_expired_caller_deadline_never_reaches_submitter() -> TestResult {
        let dir = tempfile::tempdir()?;
        let captured = Arc::new(Mutex::new(Vec::new()));
        let submitter = ControlledPortalSubmitter {
            outcome: PortalEisTextSubmission::SubmittedUnverified,
            payloads: Arc::clone(&captured),
            expected_deadline: None,
        };
        let mut delivery = PortalEisThenInboxDelivery::with_submitter_for_controlled_test(
            dir.path().to_path_buf(),
            submitter,
            Instant::now(),
        );
        let txid = TransactionId::new("tx-portal-expired");
        let report = delivery
            .deliver(
                &DeliveryIntent {
                    transaction_id: txid.clone(),
                    backend: "portal-eis-then-inbox".to_owned(),
                },
                "controlled",
                &SessionCancellation::new(),
            )
            .map_err(|error| io::Error::other(format!("controlled delivery failed: {error:?}")))?;

        assert!(mutex_lock(&captured)?.is_empty());
        assert_eq!(
            report.attempts[0].fallback_reason,
            Some(FallbackReason::BackendError {
                code: PortalEisTextFailure::TimedOut.code().to_owned(),
            })
        );
        assert_eq!(report.outcome, DeliveryOutcome::InboxFallback);
        assert_eq!(delivery.store.list().len(), 1);
        assert_eq!(delivery.store.read_text(&txid)?, "controlled");
        Ok(())
    }

    #[test]
    fn controlled_portal_clean_pre_dispatch_cancel_writes_no_inbox() -> TestResult {
        let dir = tempfile::tempdir()?;
        let deadline = Instant::now() + Duration::from_secs(1);
        let submitter = ControlledPortalSubmitter {
            outcome: PortalEisTextSubmission::CancelledBeforeDispatch,
            payloads: Arc::new(Mutex::new(Vec::new())),
            expected_deadline: Some(deadline),
        };
        let mut delivery = PortalEisThenInboxDelivery::with_submitter_for_controlled_test(
            dir.path().to_path_buf(),
            submitter,
            deadline,
        );
        let txid = TransactionId::new("tx-portal-cancelled");
        let error = expected_error(
            delivery.deliver(
                &DeliveryIntent {
                    transaction_id: txid,
                    backend: "portal-eis-then-inbox".to_owned(),
                },
                "controlled",
                &SessionCancellation::new(),
            ),
            "portal delivery unexpectedly succeeded",
        )?;
        assert_eq!(error.code, "cancelled");
        assert!(delivery.store.list().is_empty());
        Ok(())
    }

    #[test]
    fn controlled_wtype_failure_still_creates_one_inbox_recovery_item() -> TestResult {
        let dir = tempfile::tempdir()?;
        let submitter = ControlledWtypeSubmitter {
            outcome: wtype_failure("WTYPE_EXIT_1"),
            payloads: Arc::new(Mutex::new(Vec::new())),
        };
        let mut delivery = WtypeThenInboxDelivery::with_submitter_for_controlled_test(
            dir.path().to_path_buf(),
            submitter,
        );
        let txid = TransactionId::new("tx-wtype-failed");
        let report = delivery
            .deliver(
                &DeliveryIntent {
                    transaction_id: txid.clone(),
                    backend: "wtype-then-inbox".to_owned(),
                },
                "controlled",
                &SessionCancellation::new(),
            )
            .map_err(|error| io::Error::other(format!("controlled delivery failed: {error:?}")))?;
        assert_eq!(report.attempts.len(), 2);
        assert_eq!(report.attempts[0].outcome, DeliveryOutcome::Failed);
        assert_eq!(report.attempts[1].outcome, DeliveryOutcome::InboxFallback);
        assert_eq!(delivery.store.list().len(), 1);
        Ok(())
    }

    #[test]
    fn controlled_pre_dispatch_cancellation_has_no_attempt_or_inbox_item() -> TestResult {
        let dir = tempfile::tempdir()?;
        let submitter = ControlledWtypeSubmitter {
            outcome: WtypeSubmission::CancelledBeforeDispatch,
            payloads: Arc::new(Mutex::new(Vec::new())),
        };
        let mut delivery = WtypeThenInboxDelivery::with_submitter_for_controlled_test(
            dir.path().to_path_buf(),
            submitter,
        );
        let txid = TransactionId::new("tx-wtype-cancelled");
        let error = expected_error(
            delivery.deliver(
                &DeliveryIntent {
                    transaction_id: txid,
                    backend: "wtype-then-inbox".to_owned(),
                },
                "controlled",
                &SessionCancellation::new(),
            ),
            "wtype delivery unexpectedly succeeded",
        )?;
        assert_eq!(error.code, "cancelled");
        assert!(delivery.store.list().is_empty());
        Ok(())
    }

    #[test]
    fn controlled_wtype_failures_are_typed_and_always_recover_to_one_inbox_item() -> TestResult {
        let cases = [
            WtypeSubmission::Failed(FallbackReason::BackendError {
                code: "WTYPE_EXIT_1".to_owned(),
            }),
            WtypeSubmission::Failed(FallbackReason::BackendError {
                code: "WTYPE_SPAWN_FAILED".to_owned(),
            }),
            WtypeSubmission::Failed(FallbackReason::BackendError {
                code: "WTYPE_STDIN_FAILED".to_owned(),
            }),
            WtypeSubmission::Failed(FallbackReason::Timeout),
            WtypeSubmission::Failed(FallbackReason::BackendError {
                code: "WTYPE_COMPOSITOR_REJECTED".to_owned(),
            }),
        ];

        for (index, outcome) in cases.into_iter().enumerate() {
            let dir = tempfile::tempdir()?;
            let mut delivery = WtypeThenInboxDelivery::with_submitter_for_controlled_test(
                dir.path().to_path_buf(),
                ControlledWtypeSubmitter {
                    outcome,
                    payloads: Arc::new(Mutex::new(Vec::new())),
                },
            );
            let txid = TransactionId::new(format!("tx-wtype-failure-{index}"));
            let report = delivery
                .deliver(
                    &DeliveryIntent {
                        transaction_id: txid.clone(),
                        backend: "wtype-then-inbox".to_owned(),
                    },
                    "controlled-payload",
                    &SessionCancellation::new(),
                )
                .map_err(|error| {
                    io::Error::other(format!("controlled delivery failed: {error:?}"))
                })?;
            assert_eq!(report.attempts.len(), 2);
            assert_eq!(report.attempts[0].outcome, DeliveryOutcome::Failed);
            assert!(report.attempts[0].fallback_reason.is_some());
            assert_eq!(report.attempts[1].outcome, DeliveryOutcome::InboxFallback);
            assert_eq!(delivery.store.list().len(), 1);
            assert_eq!(delivery.store.read_text(&txid)?, "controlled-payload");
        }
        Ok(())
    }

    #[test]
    fn controlled_wtype_preserves_empty_and_maximum_permitted_payload_bytes() -> TestResult {
        let payloads = [
            String::new(),
            "x".repeat(crate::delivery::MAX_DIRECT_TEXT_BYTES),
        ];
        for (index, payload) in payloads.into_iter().enumerate() {
            let dir = tempfile::tempdir()?;
            let captured = Arc::new(Mutex::new(Vec::new()));
            let mut delivery = WtypeThenInboxDelivery::with_submitter_for_controlled_test(
                dir.path().to_path_buf(),
                ControlledWtypeSubmitter {
                    outcome: WtypeSubmission::SubmittedUnverified,
                    payloads: Arc::clone(&captured),
                },
            );
            let txid = TransactionId::new(format!("tx-wtype-boundary-{index}"));
            let report = delivery
                .deliver(
                    &DeliveryIntent {
                        transaction_id: txid.clone(),
                        backend: "wtype-then-inbox".to_owned(),
                    },
                    &payload,
                    &SessionCancellation::new(),
                )
                .map_err(|error| {
                    io::Error::other(format!("controlled delivery failed: {error:?}"))
                })?;
            assert_eq!(mutex_lock(&captured)?.as_slice(), [payload.as_bytes()]);
            assert_eq!(delivery.store.read_text(&txid)?, payload);
            assert_eq!(report.outcome, DeliveryOutcome::InboxFallback);
        }
        Ok(())
    }

    const SCHEDULER_ALLOWANCE: Duration = Duration::from_millis(300);

    fn wait_for_marker(marker: &Path) -> TestResult {
        let deadline = Instant::now() + Duration::from_secs(1);
        while !marker.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(marker.exists(), "controlled wtype child did not start");
        Ok(())
    }

    fn assert_pid_is_gone(marker: &Path, context: &str) -> TestResult {
        let pid = std::fs::read_to_string(marker)?;
        assert!(
            !std::process::Command::new("kill")
                .args(["-0", pid.trim()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()?
                .success(),
            "{context} child still exists"
        );
        Ok(())
    }

    /// The observing half of [`DispatchPause`]. It deliberately owns no
    /// arrival sender: when the paused side fails before the boundary the
    /// channel disconnects and [`Self::wait_arrived`] reports the failure
    /// immediately instead of waiting for the observe bound.
    #[derive(Debug)]
    struct DispatchPauseObserver {
        arrived: mpsc::Receiver<()>,
        release: mpsc::Sender<()>,
    }

    impl DispatchPauseObserver {
        fn wait_arrived(&self, context: &str) -> TestResult {
            self.arrived
                .recv_timeout(DISPATCH_BOUNDARY_OBSERVE_BOUND)
                .map_err(|error| {
                    // Release a late arrival so a failed coordination cannot
                    // leave the paused side waiting for the full backstop.
                    let _ = self.release.send(());
                    io::Error::other(format!(
                        "{context} was not reached within {DISPATCH_BOUNDARY_OBSERVE_BOUND:?}: {error}"
                    ))
                    .into()
                })
        }

        fn release(&self) {
            let _ = self.release.send(());
        }
    }

    fn dispatch_pause() -> (DispatchPause, DispatchPauseObserver) {
        let (arrived, arrived_observer) = mpsc::channel();
        let (release, released) = mpsc::channel();
        (
            DispatchPause {
                arrived,
                release: Arc::new(Mutex::new(released)),
            },
            DispatchPauseObserver {
                arrived: arrived_observer,
                release,
            },
        )
    }

    #[test]
    fn dispatch_boundary_reports_a_peer_that_failed_before_arrival() -> TestResult {
        let (pause, gate) = dispatch_pause();
        // The paused side is dropped before it ever reaches the boundary, as a
        // failed recorder/worker spawn does. The observer must report a bounded
        // error at once instead of waiting for a barrier peer.
        drop(pause);
        let error = match gate.wait_arrived("controlled missing boundary peer") {
            Ok(()) => return Err(io::Error::other("boundary observed without a peer").into()),
            Err(error) => error,
        };
        assert!(
            error
                .to_string()
                .contains("controlled missing boundary peer"),
            "unexpected boundary error: {error}"
        );
        Ok(())
    }

    #[test]
    fn dispatch_boundary_release_wakes_the_paused_side() -> TestResult {
        let (pause, gate) = dispatch_pause();
        let (done_tx, done_rx) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            pause.pause_submitter();
            let _ = done_tx.send(());
        });
        gate.wait_arrived("controlled released boundary")?;
        gate.release();
        done_rx
            .recv_timeout(DISPATCH_BOUNDARY_OBSERVE_BOUND)
            .map_err(|error| io::Error::other(format!("paused side was not released: {error}")))?;
        join_thread(handle)?;
        Ok(())
    }

    #[test]
    fn dispatch_boundary_drop_by_observer_releases_the_paused_side() -> TestResult {
        let (pause, gate) = dispatch_pause();
        let (done_tx, done_rx) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            pause.pause_submitter();
            let _ = done_tx.send(());
        });
        gate.wait_arrived("controlled dropped boundary")?;
        // A failed or panicking test drops the observer while the paused side
        // is still waiting; the channel disconnect must release it.
        drop(gate);
        done_rx
            .recv_timeout(DISPATCH_BOUNDARY_OBSERVE_BOUND)
            .map_err(|error| io::Error::other(format!("paused side was stranded: {error}")))?;
        join_thread(handle)?;
        Ok(())
    }

    #[test]
    fn system_wtype_submitter_uses_only_dash_argv_and_parent_owned_stdin() -> TestResult {
        let _guard = mutex_lock(&WTYPE_SYSTEM_TEST_LOCK)?;
        let dir = tempfile::tempdir()?;
        let argv = dir.path().join("argv");
        let stdin = dir.path().join("stdin");
        let marker = dir.path().join("wtype.pid");
        let program = dir.path().join("wtype");
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\necho $$ > '{}'\nprintf '%s' \"$1\" > '{}'\ncat > '{}'\n",
                marker.display(),
                argv.display(),
                stdin.display()
            ),
        )?;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700))?;
        let payload = b"controlled\nstdin";
        let submission = SystemWtypeSubmitter::with_program_for_test(program)
            .submit(payload, &SessionCancellation::new());
        assert_eq!(submission, WtypeSubmission::SubmittedUnverified);
        assert_eq!(std::fs::read_to_string(argv)?, "-");
        assert_eq!(std::fs::read(stdin)?, payload);
        assert_pid_is_gone(&marker, "successful wtype")?;
        Ok(())
    }

    #[test]
    fn system_wtype_chord_passes_only_key_names_and_reports_failure() -> TestResult {
        let _guard = mutex_lock(&WTYPE_SYSTEM_TEST_LOCK)?;
        let dir = tempfile::tempdir()?;
        let argv = dir.path().join("argv");
        let program = dir.path().join("wtype");
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh
printf '%s ' \"$@\" > '{}'\n",
                argv.display()
            ),
        )?;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700))?;
        let mut chord = SystemWtypeChord::with_program_for_test(program);
        chord.send(PasteKeys::ShiftInsert, &SessionCancellation::new())?;
        assert_eq!(
            std::fs::read_to_string(&argv)?,
            "-M shift -k Insert -m shift "
        );
        chord.send(PasteKeys::CtrlShiftV, &SessionCancellation::new())?;
        assert_eq!(
            std::fs::read_to_string(&argv)?,
            "-M ctrl -M shift -k v -m shift -m ctrl "
        );

        let failing = dir.path().join("wtype-fails");
        std::fs::write(&failing, "#!/bin/sh\nexit 1\n")?;
        std::fs::set_permissions(&failing, std::fs::Permissions::from_mode(0o700))?;
        let mut chord = SystemWtypeChord::with_program_for_test(failing);
        assert_eq!(
            chord.send(PasteKeys::CtrlV, &SessionCancellation::new()),
            Err("WTYPE_CHORD_EXIT_NONZERO")
        );
        let cancelled = SessionCancellation::new();
        cancelled.cancel();
        assert_eq!(chord.send(PasteKeys::CtrlV, &cancelled), Err("cancelled"));
        Ok(())
    }

    #[test]
    fn system_wtype_late_cancellation_is_bounded_and_reaps_the_child() -> TestResult {
        let _guard = mutex_lock(&WTYPE_SYSTEM_TEST_LOCK)?;
        let dir = tempfile::tempdir()?;
        let marker = dir.path().join("wtype.pid");
        let program = dir.path().join("wtype");
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\necho $$ > '{}'\nexec sleep 10\n",
                marker.display()
            ),
        )?;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700))?;

        let cancellation = SessionCancellation::new();
        let submit_cancel = cancellation.clone();
        let (result_tx, result_rx) = mpsc::channel();
        let mut submitter = SystemWtypeSubmitter::with_program_for_test(program);
        let handle = std::thread::spawn(move || {
            let result = submitter.submit(b"controlled-late-cancel", &submit_cancel);
            let _ = result_tx.send(result);
        });
        wait_for_marker(&marker)?;
        cancellation.cancel();
        assert_eq!(
            result_rx
                .recv_timeout(WTYPE_SUBMIT_BOUND + SCHEDULER_ALLOWANCE)
                .map_err(|error| io::Error::other(format!("submission did not finish: {error}")))?,
            wtype_failure("WTYPE_CANCELLED_AFTER_DISPATCH")
        );
        join_thread(handle)?;
        assert_pid_is_gone(&marker, "late-cancelled wtype")?;
        Ok(())
    }

    #[test]
    fn system_wtype_cancellation_before_first_accepted_byte_is_clean_and_writes_no_inbox()
    -> TestResult {
        let _guard = mutex_lock(&WTYPE_SYSTEM_TEST_LOCK)?;
        let dir = tempfile::tempdir()?;
        let inbox = dir.path().join("inbox");
        let marker = dir.path().join("wtype.pid");
        let program = dir.path().join("wtype");
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\necho $$ > '{}'\nexec sleep 10\n",
                marker.display()
            ),
        )?;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700))?;

        let (pause, gate) = dispatch_pause();
        let mut submitter = SystemWtypeSubmitter::with_program_for_test(program);
        submitter.pause_at_dispatch_boundary_for_test(DispatchBoundary::BeforeFirstWrite(pause));
        let cancellation = SessionCancellation::new();
        let submit_cancel = cancellation.clone();
        let txid = TransactionId::new("tx-pre-dispatch-cancel");
        let (result_tx, result_rx) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            let mut delivery =
                WtypeThenInboxDelivery::with_submitter_for_controlled_test(inbox, submitter);
            let result = delivery.deliver(
                &DeliveryIntent {
                    transaction_id: txid,
                    backend: "wtype-then-inbox".to_owned(),
                },
                "controlled-pre-dispatch-cancel",
                &submit_cancel,
            );
            let _ = result_tx.send(result);
        });

        gate.wait_arrived("wtype before-first-write boundary")?;
        wait_for_marker(&marker)?;
        cancellation.cancel();
        gate.release();
        let result = result_rx
            .recv_timeout(WTYPE_SUBMIT_BOUND + SCHEDULER_ALLOWANCE)
            .map_err(|error| io::Error::other(format!("delivery did not finish: {error}")))?;
        assert_eq!(
            expected_error(result, "pre-dispatch delivery unexpectedly succeeded")?.code,
            "cancelled"
        );
        join_thread(handle)?;
        assert_pid_is_gone(&marker, "pre-dispatch-cancelled wtype")?;
        assert!(
            InboxStore::new(dir.path().join("inbox")).list().is_empty(),
            "clean pre-dispatch cancellation must not create an Inbox item"
        );
        Ok(())
    }

    #[test]
    fn system_wtype_cancellation_after_positive_write_is_uncertain_and_recovers_to_inbox()
    -> TestResult {
        let _guard = mutex_lock(&WTYPE_SYSTEM_TEST_LOCK)?;
        let dir = tempfile::tempdir()?;
        let inbox = dir.path().join("inbox");
        let marker = dir.path().join("wtype.pid");
        let program = dir.path().join("wtype");
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\necho $$ > '{}'\nexec sleep 10\n",
                marker.display()
            ),
        )?;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700))?;

        let (pause, gate) = dispatch_pause();
        let mut submitter = SystemWtypeSubmitter::with_program_for_test(program);
        submitter.pause_at_dispatch_boundary_for_test(DispatchBoundary::AfterPositiveWrite(pause));
        let cancellation = SessionCancellation::new();
        let submit_cancel = cancellation.clone();
        let txid = TransactionId::new("tx-post-dispatch-cancel");
        let expected_txid = txid.clone();
        let (result_tx, result_rx) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            let mut delivery =
                WtypeThenInboxDelivery::with_submitter_for_controlled_test(inbox, submitter);
            let result = delivery.deliver(
                &DeliveryIntent {
                    transaction_id: txid,
                    backend: "wtype-then-inbox".to_owned(),
                },
                "controlled-post-dispatch-cancel",
                &submit_cancel,
            );
            let _ = result_tx.send(result);
        });

        gate.wait_arrived("wtype after-positive-write boundary")?;
        wait_for_marker(&marker)?;
        cancellation.cancel();
        gate.release();
        let report = result_rx
            .recv_timeout(WTYPE_SUBMIT_BOUND + SCHEDULER_ALLOWANCE)
            .map_err(|error| io::Error::other(format!("delivery did not finish: {error}")))?
            .map_err(|error| io::Error::other(format!("delivery failed: {error:?}")))?;
        assert_eq!(report.outcome, DeliveryOutcome::InboxFallback);
        assert_eq!(report.attempts.len(), 2);
        assert_eq!(report.attempts[0].backend, "wtype-stdin");
        assert_eq!(report.attempts[0].outcome, DeliveryOutcome::Failed);
        assert_eq!(
            report.attempts[0].fallback_reason,
            Some(FallbackReason::BackendError {
                code: "WTYPE_CANCELLED_AFTER_DISPATCH".to_owned(),
            })
        );
        assert_eq!(report.attempts[1].backend, "inbox");
        assert_eq!(report.attempts[1].outcome, DeliveryOutcome::InboxFallback);
        join_thread(handle)?;
        assert_pid_is_gone(&marker, "post-dispatch-cancelled wtype")?;
        let store = InboxStore::new(dir.path().join("inbox"));
        assert_eq!(store.list().len(), 1);
        assert_eq!(
            store.read_text(&expected_txid)?,
            "controlled-post-dispatch-cancel"
        );
        Ok(())
    }

    #[test]
    fn system_wtype_stdin_error_reaps_an_alive_child_without_a_writer_thread() -> TestResult {
        let _guard = mutex_lock(&WTYPE_SYSTEM_TEST_LOCK)?;
        let dir = tempfile::tempdir()?;
        let marker = dir.path().join("wtype.pid");
        let program = dir.path().join("wtype");
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\necho $$ > '{}'\nexec 0<&-\nexec sleep 10\n",
                marker.display()
            ),
        )?;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700))?;

        let submission = SystemWtypeSubmitter::with_program_for_test(program)
            .submit(&vec![b'x'; 1024 * 1024], &SessionCancellation::new());
        assert_eq!(submission, wtype_failure("WTYPE_STDIN_FAILED"));
        wait_for_marker(&marker)?;
        assert_pid_is_gone(&marker, "stdin-error wtype")?;
        Ok(())
    }

    #[test]
    fn system_wtype_parent_owned_panic_fault_reaps_an_alive_child() -> TestResult {
        let _guard = mutex_lock(&WTYPE_SYSTEM_TEST_LOCK)?;
        let dir = tempfile::tempdir()?;
        let marker = dir.path().join("wtype.pid");
        let program = dir.path().join("wtype");
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\necho $$ > '{}'\nexec sleep 10\n",
                marker.display()
            ),
        )?;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700))?;

        let admitted = Arc::new(Barrier::new(2));
        let mut submitter = SystemWtypeSubmitter::with_program_for_test(program);
        submitter.fail_next_stdin_for_test(ControlledStdinFault::PanicAfter(Arc::clone(&admitted)));
        let (result_tx, result_rx) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            let result = submitter.submit(b"controlled-panic", &SessionCancellation::new());
            let _ = result_tx.send(result);
        });
        wait_for_marker(&marker)?;
        admitted.wait();
        assert_eq!(
            result_rx
                .recv_timeout(WTYPE_SUBMIT_BOUND + SCHEDULER_ALLOWANCE)
                .map_err(|error| io::Error::other(format!("submission did not finish: {error}")))?,
            wtype_failure("WTYPE_STDIN_PANICKED")
        );
        join_thread(handle)?;
        assert_pid_is_gone(&marker, "controlled-panic wtype")?;
        Ok(())
    }

    #[test]
    fn system_wtype_timeout_uses_one_absolute_deadline_and_reaps_the_child() -> TestResult {
        let _guard = mutex_lock(&WTYPE_SYSTEM_TEST_LOCK)?;
        let dir = tempfile::tempdir()?;
        let marker = dir.path().join("wtype.pid");
        let program = dir.path().join("wtype");
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\necho $$ > '{}'\nexec sleep 10\n",
                marker.display()
            ),
        )?;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700))?;

        let started = Instant::now();
        let submission = SystemWtypeSubmitter::with_program_for_test(program)
            .submit(b"controlled-timeout", &SessionCancellation::new());
        assert_eq!(submission, WtypeSubmission::Failed(FallbackReason::Timeout));
        assert!(
            started.elapsed() < WTYPE_SUBMIT_BOUND + SCHEDULER_ALLOWANCE,
            "wtype cleanup started a second deadline window: {:?}",
            started.elapsed()
        );
        wait_for_marker(&marker)?;
        assert_pid_is_gone(&marker, "timed-out wtype")?;
        Ok(())
    }

    #[test]
    fn system_wtype_non_draining_child_cannot_block_parent_owned_stdin() -> TestResult {
        let _guard = mutex_lock(&WTYPE_SYSTEM_TEST_LOCK)?;
        let dir = tempfile::tempdir()?;
        let marker = dir.path().join("wtype.pid");
        let program = dir.path().join("wtype");
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\necho $$ > '{}'\nexec sleep 10\n",
                marker.display()
            ),
        )?;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700))?;

        let started = Instant::now();
        let submission = SystemWtypeSubmitter::with_program_for_test(program)
            .submit(&vec![b'x'; 1024 * 1024], &SessionCancellation::new());
        assert_eq!(submission, WtypeSubmission::Failed(FallbackReason::Timeout));
        assert!(
            started.elapsed() < WTYPE_SUBMIT_BOUND + SCHEDULER_ALLOWANCE,
            "non-draining stdin opened a second deadline window: {:?}",
            started.elapsed()
        );
        wait_for_marker(&marker)?;
        assert_pid_is_gone(&marker, "non-draining wtype")?;
        Ok(())
    }

    #[test]
    fn system_wtype_nonzero_exit_reaps_the_child() -> TestResult {
        let _guard = mutex_lock(&WTYPE_SYSTEM_TEST_LOCK)?;
        let dir = tempfile::tempdir()?;
        let marker = dir.path().join("wtype.pid");
        let program = dir.path().join("wtype");
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\necho $$ > '{}'\ncat > /dev/null\nexit 7\n",
                marker.display()
            ),
        )?;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700))?;

        let submission = SystemWtypeSubmitter::with_program_for_test(program)
            .submit(b"controlled-nonzero", &SessionCancellation::new());
        assert_eq!(submission, wtype_failure("WTYPE_EXIT_NONZERO"));
        wait_for_marker(&marker)?;
        assert_pid_is_gone(&marker, "nonzero-exit wtype")?;
        Ok(())
    }

    #[test]
    fn successful_wtype_leader_with_live_descendant_fails_closed() -> TestResult {
        let _guard = mutex_lock(&WTYPE_SYSTEM_TEST_LOCK)?;
        let dir = tempfile::tempdir()?;
        let descendant = dir.path().join("descendant.pid");
        let program = dir.path().join("wtype");
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\ncat > /dev/null\nsleep 30 &\necho $! > '{}'\nexit 0\n",
                descendant.display()
            ),
        )?;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700))?;

        let submission = SystemWtypeSubmitter::with_program_for_test(program)
            .submit(b"controlled-descendant", &SessionCancellation::new());
        assert_eq!(submission, wtype_failure("WTYPE_UNEXPECTED_DESCENDANT"));
        assert_pid_is_gone(&descendant, "residual wtype")?;
        Ok(())
    }

    #[test]
    fn system_wtype_parent_owned_setup_failure_reaps_started_child() -> TestResult {
        let _guard = mutex_lock(&WTYPE_SYSTEM_TEST_LOCK)?;
        let dir = tempfile::tempdir()?;
        let marker = dir.path().join("wtype.pid");
        let program = dir.path().join("wtype");
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\necho $$ > '{}'\nexec sleep 10\n",
                marker.display()
            ),
        )?;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700))?;

        let admitted = Arc::new(Barrier::new(2));
        let mut submitter = SystemWtypeSubmitter::with_program_for_test(program);
        submitter.fail_next_stdin_for_test(ControlledStdinFault::SetupAfter(Arc::clone(&admitted)));
        let (result_tx, result_rx) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            let result = submitter.submit(b"controlled-setup-failure", &SessionCancellation::new());
            let _ = result_tx.send(result);
        });
        wait_for_marker(&marker)?;
        admitted.wait();
        assert_eq!(
            result_rx
                .recv_timeout(WTYPE_SUBMIT_BOUND + SCHEDULER_ALLOWANCE)
                .map_err(|error| io::Error::other(format!("submission did not finish: {error}")))?,
            wtype_failure("WTYPE_STDIN_SETUP_FAILED")
        );
        join_thread(handle)?;
        assert_pid_is_gone(&marker, "stdin-setup-failed wtype")?;
        Ok(())
    }
}
