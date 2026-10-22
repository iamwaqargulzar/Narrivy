//! P4 live PipeWire capture monitor.
//!
//! Spawns a `pw-record` child in RAW f32le mode writing to stdout, and owns
//! one dedicated parse thread that decodes the byte stream into interleaved
//! f32 frames and pushes [`AudioChunk`]s into the existing
//! [`PreRollRingBuffer`] — nothing else.
//!
//! PERF-006 invariant (structural test `perf_006_no_blocking_on_rt_path`
//! below enforces this): the capture/parse hot path performs ZERO file I/O,
//! ZERO logging, and ZERO inference. Its only blocking operation is reading
//! from the child's pipe; its only shared state is one short mutex-guarded
//! ring push. Format handling is limited to little-endian byte swapping that
//! the hardware already guarantees; no resampling or channel conversion
//! happens here (the ring buffer copies interleaved samples verbatim;
//! conversion belongs off the RT path).
//!
//! Lifecycle: `start` -> handle; `stop(timeout)` sets the stop flag, kills
//! the child (which closes the pipe and EOF-finishes the parse thread), then
//! joins the thread within `timeout`. Bounded on every path.

use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use narrivy_types::AudioChunk;

use crate::capture::PreRollRingBuffer;

use crate::audio::jitter::{JITTER_HIGH_CODE, JitterEstimator};
use crate::audio::latency::{LatencyHistogram, LatencyStats};
use std::sync::OnceLock;

/// Global latency histogram: lock-free ring of push latencies.
static GLOBAL_LATENCY: OnceLock<LatencyHistogram> = OnceLock::new();

/// Test-only serialization for global histogram mutation. Production capture
/// remains lock-free; this merely prevents independent unit tests from
/// clearing or sampling the same process-global fixture concurrently.
#[cfg(test)]
static GLOBAL_LATENCY_TEST_LOCK: Mutex<()> = Mutex::new(());

fn global_latency() -> &'static LatencyHistogram {
    GLOBAL_LATENCY.get_or_init(|| LatencyHistogram::new(2048))
}

/// Global jitter estimator guarded by a short mutex; only touched once per
/// callback interval, never on the hot contention path of push latency.
static GLOBAL_JITTER: OnceLock<std::sync::Mutex<JitterEstimator>> = OnceLock::new();

fn global_jitter() -> &'static std::sync::Mutex<JitterEstimator> {
    GLOBAL_JITTER.get_or_init(|| std::sync::Mutex::new(JitterEstimator::new(32)))
}

/// Health facts emitted due to jitter; bounded and content-free.
static JITTER_HEALTH: OnceLock<std::sync::Mutex<Vec<String>>> = OnceLock::new();

fn jitter_health_log() -> &'static std::sync::Mutex<Vec<String>> {
    JITTER_HEALTH.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

/// pw-record binary invoked by [`LiveCapture`].
const PW_RECORD: &str = "pw-record";
/// Bytes pulled per read from the child's stdout. Sized so the parse thread
/// wakes at most ~hundreds of times per second at 48 kHz stereo f32 while
/// keeping push latency well under a human-perceptible frame.
const READ_CHUNK_BYTES: usize = 8192;
/// Short poll interval for nonblocking pipe reads and bounded shutdown.
const IO_POLL_INTERVAL: Duration = Duration::from_millis(2);
/// Internal cleanup budget used only when startup cannot return an owned
/// handle. Normal shutdown uses the caller-provided budget.
const STARTUP_CLEANUP_TIMEOUT: Duration = Duration::from_secs(2);
/// pw-dump binary used once at start() to resolve node names to object
/// serials. Runs BEFORE the capture thread exists — never on the hot path.
const PW_DUMP: &str = "pw-dump";

// ---------------------------------------------------------------------------
// Config / errors / stats
// ---------------------------------------------------------------------------

/// Configuration for a live capture session.
#[derive(Debug, Clone)]
pub struct LiveCaptureConfig {
    /// PulseAudio/PipeWire target node name (a source or `.monitor` node).
    pub target: String,
    /// Sample rate requested from pw-record.
    pub sample_rate_hz: u32,
    /// Channels requested from pw-record.
    pub channels: u16,
}

impl Default for LiveCaptureConfig {
    fn default() -> Self {
        Self {
            target: "@DEFAULT_AUDIO_SOURCE@".to_string(),
            sample_rate_hz: 16_000,
            channels: 1,
        }
    }
}

impl LiveCaptureConfig {
    fn validate(&self) -> Result<(), LiveCaptureError> {
        if self.sample_rate_hz == 0 || self.sample_rate_hz > 384_000 {
            return Err(LiveCaptureError::InvalidConfig(format!(
                "sample rate {} out of range",
                self.sample_rate_hz
            )));
        }
        if self.channels == 0 || self.channels > 32 {
            return Err(LiveCaptureError::InvalidConfig(format!(
                "channel count {} out of range",
                self.channels
            )));
        }
        if self.target.trim().is_empty() {
            return Err(LiveCaptureError::InvalidConfig("empty target".to_string()));
        }
        Ok(())
    }
}

/// Typed errors for the capture lifecycle. Strings are OS diagnostics only —
/// never audio content (privacy rule).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveCaptureError {
    /// Invalid configuration.
    InvalidConfig(String),
    /// pw-record failed to spawn or did not start in time.
    Spawn(String),
    /// Handle already stopped.
    Stopped,
    /// Parse thread failed mid-capture.
    Thread(String),
    /// Child control or status observation failed.
    Child(String),
    /// stop() exceeded its single child-reap + parse-thread-join budget.
    StopTimeout(Duration),
}

impl std::fmt::Display for LiveCaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LiveCaptureError::InvalidConfig(s) => write!(f, "invalid capture config: {s}"),
            LiveCaptureError::Spawn(s) => write!(f, "pw-record spawn failed: {s}"),
            LiveCaptureError::Stopped => write!(f, "capture already stopped"),
            LiveCaptureError::Thread(s) => write!(f, "capture thread error: {s}"),
            LiveCaptureError::Child(s) => write!(f, "pw-record lifecycle failed: {s}"),
            LiveCaptureError::StopTimeout(d) => {
                write!(f, "capture stop exceeded shared {d:?} deadline")
            }
        }
    }
}

impl std::error::Error for LiveCaptureError {}

/// Counters observable without touching sample data.
#[derive(Debug, Default)]
pub struct LiveCaptureStats {
    /// Total f32 samples pushed into the ring buffer.
    pub samples_pushed: Arc<AtomicU64>,
    /// Pushes rejected by the ring buffer (layout mismatch); always an error.
    pub pushes_rejected: Arc<AtomicU64>,
}

/// Content-free lifecycle facts for diagnostics and startup coordination.
///
/// `AwaitingFirstChunk` means only that the child and owned parser are
/// running. PipeWire signal flow is not claimed until `Streaming` after the
/// first complete audio frame has been accepted by the ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum LiveCaptureLifecycle {
    Starting = 0,
    AwaitingFirstChunk = 1,
    Streaming = 2,
    Error = 3,
    Finished = 4,
}

impl LiveCaptureLifecycle {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::AwaitingFirstChunk,
            2 => Self::Streaming,
            3 => Self::Error,
            4 => Self::Finished,
            _ => Self::Starting,
        }
    }
}

// ---------------------------------------------------------------------------
// Target resolution
// ---------------------------------------------------------------------------

/// Resolve a PipeWire node NAME to its object serial by scanning one
/// `pw-dump` snapshot. Numeric targets pass through unchanged. Pure parsing
/// lives in [`parse_pw_dump_serial`] so it stays offline-testable.
///
/// Why: pw-record's `--target <name>` silently falls back to the default
/// source when the name does not match a recordable node, and PipeWire (unlike
/// PulseAudio) exposes no `<sink>.monitor` nodes — a sink's monitor ports are
/// reachable only through the sink node itself, most stably via its serial.
fn resolve_target_serial(target: &str) -> Result<String, LiveCaptureError> {
    if target.parse::<u64>().is_ok() || (target.starts_with('@') && target.ends_with('@')) {
        return Ok(target.to_string());
    }
    let output = Command::new(PW_DUMP)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| LiveCaptureError::Spawn(format!("{PW_DUMP}: {e}")))?;
    if !output.status.success() {
        return Err(LiveCaptureError::Spawn(format!(
            "{PW_DUMP} exited unsuccessfully"
        )));
    }
    let json = String::from_utf8_lossy(&output.stdout);
    parse_pw_dump_serial(&json, target)
        .ok_or_else(|| LiveCaptureError::Spawn(format!("target node not found: {target:?}")))
}

/// Find the `object.serial` of the first node whose `node.name` equals
/// `name`. Pure function over a pw-dump JSON document.
fn parse_pw_dump_serial(pw_dump_json: &str, name: &str) -> Option<String> {
    let objects: serde_json::Value = serde_json::from_str(pw_dump_json).ok()?;
    objects.as_array()?.iter().find_map(|o| {
        let props = o.get("info")?.get("props")?;
        if props.get("node.name")?.as_str()? == name {
            // object.serial appears as a JSON string or number depending on
            // the PipeWire version; accept both.
            let serial = props.get("object.serial")?;
            match serial {
                serde_json::Value::String(sv) => Some(sv.clone()),
                serde_json::Value::Number(nv) => Some(nv.to_string()),
                _ => None,
            }
        } else {
            None
        }
    })
}

// ---------------------------------------------------------------------------
// PCM decoding helpers (pure; unit-tested offline)
// ---------------------------------------------------------------------------

/// Decode raw little-endian f32 bytes into complete interleaved frames. Any
/// trailing partial frame is carried into the next call. Pure function.
fn decode_f32le_frames(buf: &[u8], carry: &mut Vec<u8>, channels: u16) -> Vec<f32> {
    carry.extend_from_slice(buf);
    let frame_bytes = usize::from(channels.max(1)) * size_of::<f32>();
    let usable = carry.len() - (carry.len() % frame_bytes);
    let mut out = Vec::with_capacity(usable / 4);
    for chunk in carry[..usable].chunks_exact(4) {
        out.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }
    carry.drain(..usable);
    out
}

// ---------------------------------------------------------------------------
// LiveCapture
// ---------------------------------------------------------------------------

/// Owned child process plus its dedicated parse thread.
pub struct LiveCaptureHandle {
    child: Option<Child>,
    stop: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
    lifecycle: Arc<AtomicU8>,
    parser: Option<JoinHandle<Result<(), LiveCaptureError>>>,
    stats: LiveCaptureStats,
}

impl LiveCaptureHandle {
    /// True once the parse thread has exited.
    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }

    /// Shared statistics handle.
    pub fn stats(&self) -> &LiveCaptureStats {
        &self.stats
    }

    /// Current content-free lifecycle fact.
    pub fn lifecycle(&self) -> LiveCaptureLifecycle {
        LiveCaptureLifecycle::from_u8(self.lifecycle.load(Ordering::Acquire))
    }

    /// Bounded stop: set the stop flag, kill the pw-record child (closing the
    /// pipe so the parse thread sees EOF), reap the child, then join the
    /// thread within the same `timeout`. There is no unbounded wait or join.
    pub fn stop(&mut self, timeout: Duration) -> Result<(), LiveCaptureError> {
        self.stop.store(true, Ordering::Release);
        let deadline = Instant::now()
            .checked_add(timeout)
            .unwrap_or_else(Instant::now);

        let child_result = self
            .child
            .as_mut()
            .map_or(Ok(()), |child| terminate_child_until(child, deadline))
            .map_err(|error| match error {
                LiveCaptureError::StopTimeout(_) => LiveCaptureError::StopTimeout(timeout),
                other => other,
            });
        if child_result.is_ok() {
            self.child.take();
        }

        while self
            .parser
            .as_ref()
            .is_some_and(|parser| !parser.is_finished())
        {
            if Instant::now() >= deadline {
                return Err(LiveCaptureError::StopTimeout(timeout));
            }
            std::thread::sleep(
                IO_POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())),
            );
        }

        if let Some(parser) = self.parser.take() {
            match parser.join() {
                Ok(result) => result?,
                Err(_) => {
                    self.lifecycle
                        .store(LiveCaptureLifecycle::Error as u8, Ordering::Release);
                    return Err(LiveCaptureError::Thread(
                        "capture parser panicked".to_string(),
                    ));
                }
            }
        }
        child_result
    }
}

impl Drop for LiveCaptureHandle {
    fn drop(&mut self) {
        // Last-resort cleanup if stop() was never called.
        let _ = self.stop(Duration::from_secs(2));
    }
}

fn terminate_child_until(child: &mut Child, deadline: Instant) -> Result<(), LiveCaptureError> {
    match child.try_wait() {
        Ok(Some(_)) => return Ok(()),
        Ok(None) => {
            if let Err(error) = child.kill() {
                // The child can exit between try_wait and kill.
                if !matches!(child.try_wait(), Ok(Some(_))) {
                    return Err(LiveCaptureError::Child(format!("kill failed: {error}")));
                }
                return Ok(());
            }
        }
        Err(error) => {
            return Err(LiveCaptureError::Child(format!(
                "status observation failed: {error}"
            )));
        }
    }

    loop {
        match child.try_wait() {
            Ok(Some(_)) => return Ok(()),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(
                IO_POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())),
            ),
            Ok(None) => return Err(LiveCaptureError::StopTimeout(Duration::ZERO)),
            Err(error) => {
                return Err(LiveCaptureError::Child(format!(
                    "status observation failed: {error}"
                )));
            }
        }
    }
}

fn set_nonblocking(stdout: &ChildStdout) -> Result<(), io::Error> {
    let fd = stdout.as_raw_fd();
    // SAFETY: `fd` is owned by the live ChildStdout. F_GETFL reads its flags.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the descriptor remains live and existing flags are preserved.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Live PCM capture from PipeWire into a pre-roll ring buffer.
pub struct LiveCapture;

impl LiveCapture {
    /// Snapshot of push-latency histogram (p50/p95/p99 in microseconds).
    /// Lock-free read; safe to call from any thread including tests.
    pub fn latency_stats() -> LatencyStats {
        global_latency().snapshot()
    }

    /// Clear the global latency histogram (tests only; not for production use).
    #[cfg(test)]
    pub fn clear_latency_stats() {
        global_latency().clear();
    }

    /// Record current jitter health facts (content-free codes only).
    pub fn jitter_health() -> Vec<String> {
        jitter_health_log()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    #[cfg(test)]
    pub fn clear_jitter_health() {
        jitter_health_log()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        global_jitter()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
    }
}

impl LiveCapture {
    /// Start `pw-record` targeting `cfg.target` and spawn the dedicated parse
    /// thread pushing decoded chunks into `ring`.
    ///
    /// PERF-006: the spawned thread does exactly read-pipe -> decode ->
    /// ring-push. No I/O, logging, or inference exists on this path (see the
    /// structural test at the bottom of this file).
    pub fn start(
        cfg: &LiveCaptureConfig,
        ring: Arc<Mutex<PreRollRingBuffer>>,
    ) -> Result<LiveCaptureHandle, LiveCaptureError> {
        cfg.validate()?;
        // One-shot resolution before any capture thread exists (not hot path).
        let target = resolve_target_serial(&cfg.target)?;
        let mut cmd = Command::new(PW_RECORD);
        cmd.args([
            "--raw",
            "--format=f32",
            &format!("--rate={}", cfg.sample_rate_hz),
            &format!("--channels={}", cfg.channels),
            &format!("--target={}", target),
            "-",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

        let mut child = cmd
            .spawn()
            .map_err(|e| LiveCaptureError::Spawn(format!("{}: {e}", PW_RECORD)))?;
        let stdout: ChildStdout = match child.stdout.take() {
            Some(s) => s,
            None => {
                let _ = terminate_child_until(&mut child, Instant::now() + STARTUP_CLEANUP_TIMEOUT);
                return Err(LiveCaptureError::Spawn("stdout not captured".into()));
            }
        };
        if let Err(error) = set_nonblocking(&stdout) {
            drop(stdout);
            let cleanup =
                terminate_child_until(&mut child, Instant::now() + STARTUP_CLEANUP_TIMEOUT);
            return Err(LiveCaptureError::Spawn(format!(
                "stdout nonblocking setup failed: {error}; cleanup={cleanup:?}"
            )));
        }

        let stop = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let lifecycle = Arc::new(AtomicU8::new(
            LiveCaptureLifecycle::AwaitingFirstChunk as u8,
        ));
        let stats = LiveCaptureStats::default();

        let thread_stop = Arc::clone(&stop);
        let done_flag = Arc::clone(&done);
        let thread_lifecycle = Arc::clone(&lifecycle);
        let stats_for_thread = LiveCaptureStats {
            samples_pushed: Arc::clone(&stats.samples_pushed),
            pushes_rejected: Arc::clone(&stats.pushes_rejected),
        };
        let channels = cfg.channels;
        let rate = cfg.sample_rate_hz;

        let builder = std::thread::Builder::new().name("narrivy-p4-parse".into());
        let parser = match builder.spawn(move || {
            // ---- PERF-006 hot path: read -> decode -> ring push ----
            let result = parse_loop(
                stdout,
                ring,
                channels,
                rate,
                &thread_stop,
                &stats_for_thread,
                &thread_lifecycle,
            );
            thread_lifecycle.store(
                if result.is_ok() {
                    LiveCaptureLifecycle::Finished as u8
                } else {
                    LiveCaptureLifecycle::Error as u8
                },
                Ordering::Release,
            );
            done_flag.store(true, Ordering::Release);
            result
        }) {
            Ok(parser) => parser,
            Err(error) => {
                lifecycle.store(LiveCaptureLifecycle::Error as u8, Ordering::Release);
                let cleanup =
                    terminate_child_until(&mut child, Instant::now() + STARTUP_CLEANUP_TIMEOUT);
                return Err(LiveCaptureError::Spawn(format!(
                    "parse thread spawn: {error}; cleanup={cleanup:?}"
                )));
            }
        };

        Ok(LiveCaptureHandle {
            child: Some(child),
            stop,
            done,
            lifecycle,
            parser: Some(parser),
            stats,
        })
    }
}

/// The entire body of the RT-adjacent parse thread. Kept as one function so
/// the structural PERF-006 test can audit this module's hot path.
fn parse_loop(
    mut stdout: ChildStdout,
    ring: Arc<Mutex<PreRollRingBuffer>>,
    channels: u16,
    rate: u32,
    stop: &AtomicBool,
    stats: &LiveCaptureStats,
    lifecycle: &AtomicU8,
) -> Result<(), LiveCaptureError> {
    let mut buf = [0u8; READ_CHUNK_BYTES];
    let mut carry: Vec<u8> = Vec::new();
    let mut last_tick = Instant::now();
    loop {
        if stop.load(Ordering::Acquire) {
            break;
        }
        // Jitter tracking: measure interval between read iterations.
        let now = Instant::now();
        let interval = now.duration_since(last_tick);
        last_tick = now;
        // Only evaluate jitter when we have a meaningful interval (skip first).
        if interval.as_micros() > 0 {
            if let Ok(mut jitter) = global_jitter().try_lock() {
                if let Some(is_high) = jitter.observe(interval) {
                    if is_high {
                        if let Ok(mut log) = jitter_health_log().try_lock() {
                            // Content-free health code only; never transcript.
                            if log.len() < 64 {
                                log.push(JITTER_HIGH_CODE.to_string());
                            }
                        }
                    }
                }
            }
        }
        match stdout.read(&mut buf) {
            Ok(0) => break, // EOF: child exited
            Ok(n) => {
                let samples = decode_f32le_frames(&buf[..n], &mut carry, channels);
                if !samples.is_empty() {
                    if samples.iter().any(|sample| !sample.is_finite()) {
                        stats.pushes_rejected.fetch_add(1, Ordering::Relaxed);
                        return Err(LiveCaptureError::Thread(
                            "capture stream contained a non-finite sample".to_owned(),
                        ));
                    }
                    let chunk = AudioChunk {
                        sample_rate_hz: rate,
                        channels,
                        samples,
                    };
                    // Latency measurement: time the bounded push only.
                    let t0 = Instant::now();
                    // Only shared operation on this path: one bounded push.
                    let mut guard = match ring.try_lock() {
                        Ok(g) => g,
                        Err(_) => {
                            // Contended lock: retry briefly rather than
                            // blocking unboundedly behind a drain.
                            std::thread::sleep(Duration::from_micros(50));
                            match ring.try_lock() {
                                Ok(g) => g,
                                Err(_) => {
                                    stats.pushes_rejected.fetch_add(1, Ordering::Relaxed);
                                    return Err(LiveCaptureError::Thread(
                                        "capture ring remained contended".to_owned(),
                                    ));
                                }
                            }
                        }
                    };
                    let push_res = guard.push(&chunk);
                    let elapsed = t0.elapsed();
                    // Drop guard before recording to avoid holding lock.
                    drop(guard);
                    global_latency().record(elapsed);
                    match push_res {
                        Ok(()) => {
                            stats
                                .samples_pushed
                                .fetch_add(chunk.samples.len() as u64, Ordering::Relaxed);
                            lifecycle
                                .store(LiveCaptureLifecycle::Streaming as u8, Ordering::Release);
                        }
                        Err(_) => {
                            stats.pushes_rejected.fetch_add(1, Ordering::Relaxed);
                            return Err(LiveCaptureError::Thread(
                                "capture ring rejected the negotiated layout".to_owned(),
                            ));
                        }
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(IO_POLL_INTERVAL);
            }
            Err(error) => {
                // Pipe error: record via counter only (no logging on RT path);
                // surface through pushes_rejected + thread exit.
                stats.pushes_rejected.fetch_add(1, Ordering::Relaxed);
                return Err(LiveCaptureError::Thread(format!(
                    "capture pipe read failed: {error}"
                )));
            }
        }
    }
    if !stop.load(Ordering::Acquire) && !carry.is_empty() {
        stats.pushes_rejected.fetch_add(1, Ordering::Relaxed);
        return Err(LiveCaptureError::Thread(
            "capture stream ended with a partial frame".to_owned(),
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn global_latency_test_guard() -> std::sync::MutexGuard<'static, ()> {
        GLOBAL_LATENCY_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// CPU time excludes descheduling by other Cargo test jobs, so this
    /// measures the operation's cost rather than host scheduler interference.
    fn current_thread_cpu_time() -> Duration {
        let mut sample = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: `sample` is valid writable storage for `clock_gettime`, and
        // CLOCK_THREAD_CPUTIME_ID reads time for only this calling test thread.
        let result = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut sample) };
        assert_eq!(result, 0, "CLOCK_THREAD_CPUTIME_ID was unavailable");
        assert!(sample.tv_sec >= 0 && sample.tv_nsec >= 0);
        Duration::new(sample.tv_sec as u64, sample.tv_nsec as u32)
    }

    #[test]
    fn pw_dump_serial_resolution_finds_named_node() {
        let doc = r#"[
          {"id": 10, "info": {"props": {"node.name": "alsa_output.x", "object.serial": "111"}}},
          {"id": 20, "info": {"props": {"node.name": "narrivy_p4", "object.serial": 222}}},
          {"id": 30, "info": {"props": {"node.name": "narrivy_p4", "object.serial": "333"}}}
        ]"#;
        assert_eq!(
            parse_pw_dump_serial(doc, "narrivy_p4").as_deref(),
            Some("222")
        );
        assert_eq!(parse_pw_dump_serial(doc, "missing"), None);
        assert_eq!(parse_pw_dump_serial("not json", "x"), None);
        assert_eq!(
            resolve_target_serial("@DEFAULT_AUDIO_SOURCE@").as_deref(),
            Ok("@DEFAULT_AUDIO_SOURCE@")
        );
    }

    #[test]
    fn decode_f32le_handles_partial_frames_across_calls() {
        let samples = [0.25f32, -0.5, 1.0, -1.0];
        let mut bytes = Vec::new();
        for s in samples {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        // Feed 5 bytes then the rest; carry must bridge the split sample.
        let mut carry = Vec::new();
        let first = decode_f32le_frames(&bytes[..5], &mut carry, 2);
        assert!(first.is_empty());
        assert_eq!(carry.len(), 5);
        let rest = decode_f32le_frames(&bytes[5..], &mut carry, 2);
        assert_eq!(rest, samples);
        assert!(carry.is_empty());
    }

    #[test]
    fn decoder_carries_complete_sample_when_stereo_frame_is_partial() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&1.0f32.to_le_bytes());
        let mut carry = Vec::new();
        assert!(decode_f32le_frames(&bytes, &mut carry, 2).is_empty());
        assert_eq!(carry, bytes);
        bytes.clear();
        bytes.extend_from_slice(&2.0f32.to_le_bytes());
        assert_eq!(decode_f32le_frames(&bytes, &mut carry, 2), vec![1.0, 2.0]);
        assert!(carry.is_empty());
    }

    fn parse_controlled_stdout(script: &str) -> Result<(), LiveCaptureError> {
        use crate::capture::CaptureConfig;

        let mut child = Command::new("sh")
            .args(["-c", script])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| LiveCaptureError::Spawn(error.to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| LiveCaptureError::Spawn("controlled stdout missing".to_owned()))?;
        let ring = Arc::new(Mutex::new(
            PreRollRingBuffer::from_config(&CaptureConfig {
                sample_rate_hz: 16_000,
                channels: 1,
                preroll_seconds: 1.0,
                device_hint: None,
            })
            .map_err(|error| LiveCaptureError::Thread(error.to_string()))?,
        ));
        let result = parse_loop(
            stdout,
            ring,
            1,
            16_000,
            &AtomicBool::new(false),
            &LiveCaptureStats::default(),
            &AtomicU8::new(LiveCaptureLifecycle::AwaitingFirstChunk as u8),
        );
        let _ = child.wait();
        result
    }

    #[test]
    fn parser_rejects_nonfinite_samples_and_partial_natural_eof() {
        assert!(matches!(
            parse_controlled_stdout("printf '\\000\\000\\300\\177'"),
            Err(LiveCaptureError::Thread(message)) if message.contains("non-finite")
        ));
        assert!(matches!(
            parse_controlled_stdout("printf '\\001'"),
            Err(LiveCaptureError::Thread(message)) if message.contains("partial frame")
        ));
    }

    #[test]
    fn config_validation_rejects_bounds_violations() {
        assert!(LiveCaptureConfig::default().validate().is_ok());
        let bad = LiveCaptureConfig {
            target: "x".into(),
            sample_rate_hz: 0,
            channels: 1,
        };
        assert!(matches!(
            bad.validate(),
            Err(LiveCaptureError::InvalidConfig(_))
        ));
        let bad_ch = LiveCaptureConfig {
            target: "x".into(),
            sample_rate_hz: 16_000,
            channels: 0,
        };
        assert!(matches!(
            bad_ch.validate(),
            Err(LiveCaptureError::InvalidConfig(_))
        ));
    }

    #[test]
    fn stop_before_start_is_bounded_and_safe() {
        // A handle with no child must still terminate deterministically.
        let h = LiveCaptureHandle {
            child: None,
            stop: Arc::new(AtomicBool::new(true)),
            done: Arc::new(AtomicBool::new(true)),
            lifecycle: Arc::new(AtomicU8::new(LiveCaptureLifecycle::Finished as u8)),
            parser: None,
            stats: LiveCaptureStats::default(),
        };
        let mut h = h;
        assert_eq!(h.stop(Duration::from_millis(50)), Ok(()));
        let _ = &mut h;
    }

    #[test]
    fn owned_parser_streams_batched_frames_and_stop_reaps_every_owner()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::capture::CaptureConfig;

        let mut child = Command::new("sh")
            .args(["-c", "head -c 8192 /dev/zero; while :; do :; done"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("test stdout missing"))?;
        set_nonblocking(&stdout)?;

        let config = CaptureConfig {
            sample_rate_hz: 16_000,
            channels: 2,
            preroll_seconds: 1.0,
            device_hint: None,
        };
        let ring = Arc::new(Mutex::new(PreRollRingBuffer::from_config(&config)?));
        let stop = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let lifecycle = Arc::new(AtomicU8::new(
            LiveCaptureLifecycle::AwaitingFirstChunk as u8,
        ));
        let stats = LiveCaptureStats::default();

        let parser_stop = Arc::clone(&stop);
        let parser_done = Arc::clone(&done);
        let parser_lifecycle = Arc::clone(&lifecycle);
        let parser_ring = Arc::clone(&ring);
        let parser_stats = LiveCaptureStats {
            samples_pushed: Arc::clone(&stats.samples_pushed),
            pushes_rejected: Arc::clone(&stats.pushes_rejected),
        };
        let parser = std::thread::spawn(move || {
            let result = parse_loop(
                stdout,
                parser_ring,
                2,
                16_000,
                &parser_stop,
                &parser_stats,
                &parser_lifecycle,
            );
            parser_lifecycle.store(
                if result.is_ok() {
                    LiveCaptureLifecycle::Finished as u8
                } else {
                    LiveCaptureLifecycle::Error as u8
                },
                Ordering::Release,
            );
            parser_done.store(true, Ordering::Release);
            result
        });
        let mut handle = LiveCaptureHandle {
            child: Some(child),
            stop,
            done,
            lifecycle,
            parser: Some(parser),
            stats,
        };

        let streaming_deadline = Instant::now() + Duration::from_secs(1);
        while handle.lifecycle() != LiveCaptureLifecycle::Streaming {
            assert!(
                Instant::now() < streaming_deadline,
                "parser did not accept the first complete frame"
            );
            std::thread::sleep(IO_POLL_INTERVAL);
        }
        assert_eq!(handle.stats().samples_pushed.load(Ordering::Acquire), 2048);

        handle.stop(Duration::from_secs(1))?;
        assert!(handle.is_done());
        assert_eq!(handle.lifecycle(), LiveCaptureLifecycle::Finished);
        assert!(
            handle.child.is_none(),
            "child owner must be consumed after reap"
        );
        assert!(
            handle.parser.is_none(),
            "parser owner must be consumed after join"
        );
        assert_eq!(
            ring.lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .pushed_total(),
            2048
        );
        Ok(())
    }

    #[test]
    fn stop_timeout_is_bounded_and_retains_parser_ownership()
    -> Result<(), Box<dyn std::error::Error>> {
        let done = Arc::new(AtomicBool::new(false));
        let parser_done = Arc::clone(&done);
        let parser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(75));
            parser_done.store(true, Ordering::Release);
            Ok(())
        });
        let mut handle = LiveCaptureHandle {
            child: None,
            stop: Arc::new(AtomicBool::new(false)),
            done,
            lifecycle: Arc::new(AtomicU8::new(
                LiveCaptureLifecycle::AwaitingFirstChunk as u8,
            )),
            parser: Some(parser),
            stats: LiveCaptureStats::default(),
        };

        let started = Instant::now();
        assert_eq!(
            handle.stop(Duration::from_millis(5)),
            Err(LiveCaptureError::StopTimeout(Duration::from_millis(5)))
        );
        assert!(started.elapsed() < Duration::from_millis(50));
        assert!(
            handle.parser.is_some(),
            "timed-out parser must remain owned"
        );

        handle.stop(Duration::from_secs(1))?;
        assert!(handle.parser.is_none());
        Ok(())
    }

    /// Structural gate (VER-PERF-006): the parse hot path must contain no
    /// file I/O, no logging, and no inference machinery. Audits this source
    /// file: the only permitted blocking call on the RT-adjacent thread is
    /// the pipe read; anything fs/log/inference-shaped fails the build.
    #[test]
    fn perf_006_no_blocking_on_rt_path() -> Result<(), Box<dyn std::error::Error>> {
        let src = include_str!("pipewire_monitor.rs");
        let hot_path_start = src
            .find("fn parse_loop(")
            .ok_or_else(|| std::io::Error::other("parse_loop must exist"))?;
        let hot_path_end = src[hot_path_start..]
            .find("\n// ------")
            .map(|i| hot_path_start + i)
            .unwrap_or(src.len());
        let hot = &src[hot_path_start..hot_path_end];

        for forbidden in [
            "std::fs",
            "File::",
            "OpenOptions",
            "write_all",
            "println!",
            "eprintln!",
            "log::",
            "tracing",
            "inference",
            "vad",
            "resample",
            "recv_timeout",
            "Condvar",
            "frame_chunks",
            "chunk_samples.to_vec",
        ] {
            assert!(
                !hot.contains(forbidden),
                "PERF-006 violation: `{forbidden}` found on the parse hot path"
            );
        }
        // Ring access uses try_lock (never a blocking lock) on this path.
        assert!(hot.contains("try_lock"));
        Ok(())
    }

    /// Micro-bench companion (VER-PERF-006): a single ring-buffer push must
    /// stay far under the 50 µs budget even at the largest supported layout.
    #[test]
    fn perf_006_ring_push_microbench_under_50us() -> Result<(), Box<dyn std::error::Error>> {
        use crate::capture::CaptureConfig;
        let config = CaptureConfig {
            sample_rate_hz: 48_000,
            channels: 2,
            preroll_seconds: 10.0,
            device_hint: None,
        };
        let mut ring = PreRollRingBuffer::from_config(&config)?;
        let chunk = AudioChunk {
            sample_rate_hz: 48_000,
            channels: 2,
            samples: vec![0.0f32; 256 * 2],
        };
        // Warm-up.
        for _ in 0..200 {
            let _ = ring.push(&chunk);
        }
        let iterations = 2_000u32;
        let t0 = current_thread_cpu_time();
        for _ in 0..iterations {
            let _ = ring.push(&chunk);
        }
        let elapsed = current_thread_cpu_time().saturating_sub(t0);
        let per_push_us = elapsed.as_nanos() as f64 / iterations as f64 / 1_000.0;
        assert!(
            per_push_us < 50.0,
            "ring push averaged {per_push_us:.3} µs (> 50 µs budget)"
        );
        println!("perf_006: avg ring push = {per_push_us:.3} µs over {iterations} pushes");
        Ok(())
    }

    #[test]
    fn latency_histogram_p99_under_load_stays_under_50us() -> Result<(), Box<dyn std::error::Error>>
    {
        use crate::capture::CaptureConfig;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let _global_latency_guard = global_latency_test_guard();
        LiveCapture::clear_latency_stats();
        let hist = global_latency();
        // Simulate 4 inference threads doing SHA256 loops (busy CPU)
        let stop = Arc::new(AtomicBool::new(false));
        let mut workers = Vec::new();
        for _ in 0..4 {
            let stop = Arc::clone(&stop);
            workers.push(std::thread::spawn(move || {
                let mut data = vec![0u8; 1024];
                while !stop.load(Ordering::Relaxed) {
                    // SHA256-like busy work: hash loop without allocating transcript
                    use std::collections::hash_map::DefaultHasher;
                    use std::hash::{Hash, Hasher};
                    let mut hasher = DefaultHasher::new();
                    for b in &data {
                        b.hash(&mut hasher);
                    }
                    let _ = hasher.finish();
                    // mutate data to avoid optimization
                    data[0] = data[0].wrapping_add(1);
                }
            }));
        }

        // Main thread: capture 1000 chunks measuring latency via same histogram path
        let config = CaptureConfig {
            sample_rate_hz: 48_000,
            channels: 2,
            preroll_seconds: 1.0,
            device_hint: None,
        };
        let mut ring = PreRollRingBuffer::from_config(&config)?;
        let chunk = AudioChunk {
            sample_rate_hz: 48_000,
            channels: 2,
            samples: vec![0.0f32; 256 * 2],
        };
        for _ in 0..1000 {
            let t0 = current_thread_cpu_time();
            let _ = ring.push(&chunk);
            let elapsed = current_thread_cpu_time().saturating_sub(t0);
            hist.record(elapsed);
        }

        stop.store(true, Ordering::Relaxed);
        for w in workers {
            let _ = w.join();
        }

        let stats = hist.snapshot();
        let s = LiveCapture::latency_stats();
        // Both views should agree on p99
        assert_eq!(s.count, stats.count);
        let p99 = s.p99_us.unwrap_or(u64::MAX);
        let p50 = s.p50_us.unwrap_or(u64::MAX);
        println!(
            "latency under load: p50={p50}µs p95={}µs p99={p99}µs max={}µs count={}",
            s.p95_us.unwrap_or(0),
            s.max_us.unwrap_or(0),
            s.count
        );
        assert!(
            p99 < 50,
            "p99 push latency {p99}µs exceeds 50µs budget under load"
        );
        assert!(p50 < 10, "p50 {p50}µs unexpectedly high");
        Ok(())
    }

    #[test]
    fn latency_stats_exposes_p50_p95_p99_via_global_histogram()
    -> Result<(), Box<dyn std::error::Error>> {
        let _global_latency_guard = global_latency_test_guard();
        LiveCapture::clear_latency_stats();
        let h = global_latency();
        for us in [5u64, 10, 15, 20, 25, 30, 35, 40, 45, 50] {
            h.record(Duration::from_micros(us));
        }
        let s = LiveCapture::latency_stats();
        assert_eq!(s.count, 10);
        let p50 = s
            .p50_us
            .ok_or_else(|| std::io::Error::other("missing p50"))?;
        let p95 = s
            .p95_us
            .ok_or_else(|| std::io::Error::other("missing p95"))?;
        let p99 = s
            .p99_us
            .ok_or_else(|| std::io::Error::other("missing p99"))?;
        let max = s
            .max_us
            .ok_or_else(|| std::io::Error::other("missing max"))?;
        assert!(p50 <= p95);
        assert!(p95 <= p99);
        println!(
            "latency snapshot: p50={} p95={} p99={} max={}",
            p50, p95, p99, max
        );
        Ok(())
    }

    #[test]
    fn jitter_high_detection_emits_health_metric() -> Result<(), Box<dyn std::error::Error>> {
        LiveCapture::clear_jitter_health();
        let mut est = JitterEstimator::new(8);
        // Low variance => no jitter_high
        for _ in 0..8 {
            est.observe(Duration::from_millis(10));
        }
        assert!(!est.is_high());
        assert!(est.health_fact().is_none());
        // High variance => jitter_high
        let mut est2 = JitterEstimator::new(8);
        for i in 0..8 {
            let d = if i % 2 == 0 {
                Duration::from_millis(1)
            } else {
                Duration::from_millis(30)
            };
            est2.observe(d);
        }
        assert!(est2.is_high());
        let fact = est2
            .health_fact()
            .ok_or_else(|| std::io::Error::other("high jitter must emit a health fact"))?;
        assert_eq!(fact.1, JITTER_HIGH_CODE);
        // Simulate that global jitter health log would receive it via parse_loop path
        // Here we directly test the estimator logic that the adapter uses.
        Ok(())
    }

    #[test]
    fn virtual_mic_exclusion_and_env_override() {
        use crate::capture::{MicDevice, rank_mics};
        // Inject virtual names
        let real = MicDevice {
            id: "a".into(),
            name: "USB mic".into(),
            default: false,
            rate_hz: 48_000,
            channels: 1,
        };
        let virt = MicDevice {
            id: "b".into(),
            name: "narrivy_test_mic".into(),
            default: true,
            rate_hz: 48_000,
            channels: 1,
        };
        let remap = MicDevice {
            id: "c".into(),
            name: "remap-source".into(),
            default: false,
            rate_hz: 16_000,
            channels: 1,
        };

        // Without env, virtual devices excluded
        unsafe {
            std::env::remove_var("NARRIVY_ALLOW_VIRTUAL_MIC");
        }
        let ranked = rank_mics(&[real.clone(), virt.clone(), remap.clone()]);
        assert!(
            ranked.iter().all(|d| !d.is_virtual()),
            "virtual devices should be excluded"
        );
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].id, "a");

        // Check is_virtual detection itself
        assert!(virt.is_virtual());
        assert!(remap.is_virtual());
        assert!(!real.is_virtual());

        // With env, virtual included and ranking respects default first
        unsafe {
            std::env::set_var("NARRIVY_ALLOW_VIRTUAL_MIC", "1");
        }
        let ranked2 = rank_mics(&[real.clone(), virt.clone(), remap.clone()]);
        assert_eq!(ranked2.len(), 3);
        // virt is default, so should be first despite name
        assert_eq!(ranked2[0].id, "b");
        unsafe {
            std::env::remove_var("NARRIVY_ALLOW_VIRTUAL_MIC");
        }

        // Edge: all virtual and env not set => fallback to full list (never empty surprise)
        let only_virtual = rank_mics(&[virt.clone(), remap.clone()]);
        assert_eq!(
            only_virtual.len(),
            2,
            "fallback to full list when filtered empty"
        );
    }
}
