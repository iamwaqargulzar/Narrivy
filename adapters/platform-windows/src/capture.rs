//! Capture sessions for the portable capture contracts, independent of the
//! audio API that feeds them.
//!
//! One producer thread owns the concrete source (on Windows a WASAPI client,
//! whose COM objects must stay on the thread that created them). It converts
//! audio to the capture contract (mono f32, 16 kHz), cuts it into chunks and
//! hands them over a bounded queue. The consumer side implements
//! [`AudioCaptureSession`] (batch) and [`StreamingAudioCaptureSession`]
//! (live) with the same terminal semantics as the Linux PipeWire sessions:
//!
//! - Stop keeps every sample captured before it (`FinalizationRequested`, or
//!   `EndpointDetected` when endpointing asked first);
//! - cancellation is terminal and wins every race with finalization;
//! - reaching the documented maximum is `SafetyLimit`, never a silent clip;
//! - the device going away mid-dictation is `SourceEnded` and the audio
//!   captured until then is returned, so the core can journal it and save it
//!   (D6, F-H2) instead of losing what was already said;
//! - a full queue is the typed terminal error `capture_backpressure`;
//!   samples are never dropped or overwritten.
//!
//! The producer never blocks on the consumer: a full queue ends the session.
//! Every wait is bounded and every thread is joined under a deadline.

#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use narrivy_platform_api::{
    AudioCaptureSession, CaptureEnd, CaptureStreamChunk, CaptureStreamEvent, CaptureStreamInfo,
    CaptureStreamSummary, CapturedAudio, PlatformError, StreamingAudioCaptureSession,
};
use narrivy_types::{AudioChunk, SessionControl, SessionFinalizationReason};

use crate::error;

pub const CAPTURE_RATE_HZ: u32 = narrivy_types::CAPTURE_SAMPLE_RATE_HZ;
pub const CAPTURE_CHANNELS: u16 = 1;
/// Bound for stopping the producer and releasing the device.
pub const CAPTURE_CANCEL_BOUND: Duration = Duration::from_secs(2);
/// Bound for the device to open and deliver its first audio. A WASAPI
/// endpoint delivers packets (silent ones included) every few milliseconds
/// once started, so a longer wait means the device is not working.
pub const CAPTURE_READY_BOUND: Duration = Duration::from_secs(5);
/// Same cadence and queue as the Linux streaming session: 80 ms chunks, 64
/// queued (5.12 s), so a slow engine step is absorbed but a persistently
/// slow one still fails the session instead of losing audio.
pub const STREAM_CAPTURE_CHUNK_SAMPLES: usize = 1_280;
pub const STREAM_CAPTURE_QUEUE_CHUNKS: usize = 64;
/// Batch sessions use 20 ms chunks so readiness and live preview are prompt.
/// Their queue holds a whole maximum-length capture, so it can never
/// saturate: the producer stops at the bound.
pub const BATCH_CAPTURE_CHUNK_SAMPLES: usize = 320;
/// How long the producer waits for the device per turn, and the consumer
/// for the queue. Bounds how quickly Stop and Cancel are observed.
const POLL: Duration = Duration::from_millis(10);

/// What one read of the concrete source found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceRead {
    /// Samples were appended.
    Data,
    /// Nothing arrived within the wait.
    Idle,
    /// The device went away or stopped for good (unplugged, disabled,
    /// format changed under the stream). Samples appended by this call, if
    /// any, are still valid.
    Ended,
}

/// A started audio source producing contract samples (mono f32, 16 kHz).
/// Created and used on the producer thread only.
pub trait CaptureSource {
    /// Wait at most `wait` for audio and append what arrived to `out`.
    ///
    /// # Errors
    /// A content-free [`PlatformError`] for a failure that is not the device
    /// going away (that is [`SourceRead::Ended`]).
    fn read(&mut self, out: &mut Vec<f32>, wait: Duration) -> Result<SourceRead, PlatformError>;

    /// The stream is over: append whatever the converter still holds.
    fn finish(&mut self, out: &mut Vec<f32>);
}

/// Opens the source on the producer thread.
pub type SourceOpener =
    Box<dyn FnOnce() -> Result<Box<dyn CaptureSource>, PlatformError> + Send + 'static>;

const COMMAND_RUN: u8 = 0;
const COMMAND_FINALIZE: u8 = 1;
const COMMAND_CANCEL: u8 = 2;

/// How the producer ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Terminal {
    Running,
    /// Stop: every captured sample was queued.
    Finalized,
    /// The maximum was reached: exactly the bound was queued.
    SafetyLimit,
    /// The device went away: every sample captured until then was queued.
    SourceEnded,
    Cancelled,
    Failed(&'static str),
}

impl Terminal {
    fn error_code(self) -> Option<&'static str> {
        match self {
            Self::Running | Self::Finalized | Self::SafetyLimit | Self::SourceEnded => None,
            Self::Cancelled => Some("cancelled"),
            Self::Failed(code) => Some(code),
        }
    }
}

struct Shared {
    command: AtomicU8,
    terminal: Mutex<Terminal>,
}

impl Shared {
    fn terminal(&self) -> Terminal {
        match self.terminal.lock() {
            Ok(terminal) => *terminal,
            Err(poisoned) => *poisoned.into_inner(),
        }
    }

    fn set_terminal(&self, value: Terminal) {
        match self.terminal.lock() {
            Ok(mut terminal) => *terminal = value,
            Err(poisoned) => *poisoned.into_inner() = value,
        }
    }
}

struct ProducedChunk {
    sequence: u64,
    samples: Vec<f32>,
    first_frame_after_start_us: Option<u64>,
}

/// Parameters fixed when a session starts.
#[derive(Debug, Clone)]
pub struct CaptureConfig {
    /// Content-free selection expression (`default` or a device id).
    pub source_selection: String,
    /// The session's bound in samples; at most
    /// [`narrivy_types::MAX_CAPTURE_SAMPLES`].
    pub max_samples: usize,
}

impl CaptureConfig {
    /// # Errors
    /// `capture_duration_invalid` for a window outside
    /// `[1, MAX_CAPTURE_SECONDS]` seconds.
    pub fn new(source_selection: impl Into<String>, seconds: f32) -> Result<Self, PlatformError> {
        let max_seconds = narrivy_types::MAX_CAPTURE_SECONDS as f32;
        if !seconds.is_finite() || !(1.0..=max_seconds).contains(&seconds) {
            return Err(error("capture_duration_invalid"));
        }
        let samples = (f64::from(seconds) * f64::from(CAPTURE_RATE_HZ)).round() as usize;
        Ok(Self {
            source_selection: source_selection.into(),
            max_samples: samples.min(narrivy_types::MAX_CAPTURE_SAMPLES),
        })
    }

    fn info(&self) -> CaptureStreamInfo {
        CaptureStreamInfo {
            source_selection: self.source_selection.clone(),
            sample_rate_hz: CAPTURE_RATE_HZ,
            channels: CAPTURE_CHANNELS,
        }
    }

    fn safety_deadline(&self, started_at: Instant) -> Instant {
        let seconds = self.max_samples as f64 / f64::from(CAPTURE_RATE_HZ);
        started_at + Duration::from_secs_f64(seconds) + Duration::from_secs(5)
    }
}

/// The consumer's handle on one producer thread.
struct Producer {
    receiver: Option<Receiver<ProducedChunk>>,
    thread: Option<JoinHandle<()>>,
    shared: Arc<Shared>,
}

impl Producer {
    fn spawn(
        open: SourceOpener,
        max_samples: usize,
        chunk_samples: usize,
        queue_chunks: usize,
        started_at: Instant,
    ) -> Result<Self, PlatformError> {
        let shared = Arc::new(Shared {
            command: AtomicU8::new(COMMAND_RUN),
            terminal: Mutex::new(Terminal::Running),
        });
        let (sender, receiver) = sync_channel(queue_chunks);
        let producer_shared = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("narrivy-capture".to_owned())
            .spawn(move || {
                let terminal = match open() {
                    Ok(mut source) => produce(
                        source.as_mut(),
                        &sender,
                        &producer_shared,
                        max_samples,
                        chunk_samples,
                        started_at,
                    ),
                    Err(failure) => Terminal::Failed(failure.code),
                };
                // Set before the sender drops: a consumer that sees the
                // queue close always reads the final terminal.
                producer_shared.set_terminal(terminal);
                drop(sender);
            })
            .map_err(|_| error("capture_thread_spawn_failed"))?;
        Ok(Self {
            receiver: Some(receiver),
            thread: Some(thread),
            shared,
        })
    }

    fn command(&self, command: u8) {
        // Cancel always wins; Finalize never downgrades a Cancel.
        let _ = self.shared.command.fetch_max(command, Ordering::AcqRel);
    }

    fn recv(&self, wait: Duration) -> Received {
        let Some(receiver) = self.receiver.as_ref() else {
            return Received::Closed;
        };
        match receiver.recv_timeout(wait) {
            Ok(chunk) => Received::Chunk(chunk),
            Err(RecvTimeoutError::Timeout) => Received::Empty,
            Err(RecvTimeoutError::Disconnected) => Received::Closed,
        }
    }

    fn terminal(&self) -> Terminal {
        self.shared.terminal()
    }

    /// Wait for the producer to finish on its own (after Stop or the end of
    /// the source) and join it.
    fn join(&mut self, deadline: Instant) -> Result<(), PlatformError> {
        while self
            .thread
            .as_ref()
            .is_some_and(|thread| !thread.is_finished())
        {
            if Instant::now() >= deadline {
                return Err(error("capture_thread_termination_timeout"));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        if let Some(thread) = self.thread.take() {
            thread
                .join()
                .map_err(|_| error("capture_thread_panicked"))?;
        }
        Ok(())
    }

    /// Cancel, discard queued audio and join under `deadline`.
    fn shutdown(&mut self, deadline: Instant) -> Result<(), PlatformError> {
        self.command(COMMAND_CANCEL);
        self.receiver.take();
        self.join(deadline)
    }
}

enum Received {
    Chunk(ProducedChunk),
    Empty,
    Closed,
}

fn elapsed_us(since: Instant) -> u64 {
    u64::try_from(since.elapsed().as_micros()).unwrap_or(u64::MAX)
}

/// The producer's side of the queue.
struct Outbox<'a> {
    sender: &'a SyncSender<ProducedChunk>,
    sequence: u64,
    queued: usize,
    max_samples: usize,
    chunk_samples: usize,
    /// Measured once; travels on the first chunk only.
    first_frame: Option<u64>,
    first_frame_sent: bool,
}

impl Outbox<'_> {
    fn send(&mut self, samples: Vec<f32>) -> Result<(), Terminal> {
        let first_frame_after_start_us = if self.first_frame_sent {
            None
        } else {
            self.first_frame
        };
        let count = samples.len();
        let chunk = ProducedChunk {
            sequence: self.sequence,
            samples,
            first_frame_after_start_us,
        };
        match self.sender.try_send(chunk) {
            Ok(()) => {
                self.first_frame_sent = true;
                self.queued += count;
                self.sequence = self
                    .sequence
                    .checked_add(1)
                    .ok_or(Terminal::Failed("capture_output_invalid"))?;
                Ok(())
            }
            Err(TrySendError::Full(_)) => Err(Terminal::Failed("capture_backpressure")),
            Err(TrySendError::Disconnected(_)) => Err(Terminal::Failed("capture_consumer_gone")),
        }
    }

    /// Queue whole chunks while running. `Some(SafetyLimit)` once the bound
    /// is reached; the remainder below one chunk stays pending.
    fn send_complete(&mut self, pending: &mut Vec<f32>) -> Result<Option<Terminal>, Terminal> {
        loop {
            let room = self.max_samples - self.queued;
            if pending.len() < self.chunk_samples && pending.len() < room {
                return Ok(None);
            }
            let take = self.chunk_samples.min(room);
            let rest = pending.split_off(take);
            let chunk = std::mem::replace(pending, rest);
            self.send(chunk)?;
            if self.queued == self.max_samples {
                return Ok(Some(Terminal::SafetyLimit));
            }
        }
    }

    /// Queue everything pending at the end of the stream, up to the bound.
    fn send_rest(&mut self, pending: &mut Vec<f32>, ended: Terminal) -> Terminal {
        let room = self.max_samples - self.queued;
        let limited = pending.len() >= room;
        pending.truncate(room);
        let rest = std::mem::take(pending);
        for piece in rest.chunks(self.chunk_samples) {
            if let Err(terminal) = self.send(piece.to_vec()) {
                return terminal;
            }
        }
        if limited {
            Terminal::SafetyLimit
        } else {
            ended
        }
    }
}

/// The producer loop. Returns how it ended; never panics.
fn produce(
    source: &mut dyn CaptureSource,
    sender: &SyncSender<ProducedChunk>,
    shared: &Shared,
    max_samples: usize,
    chunk_samples: usize,
    started_at: Instant,
) -> Terminal {
    let mut pending: Vec<f32> = Vec::with_capacity(chunk_samples * 2);
    let mut outbox = Outbox {
        sender,
        sequence: 0,
        queued: 0,
        max_samples,
        chunk_samples,
        first_frame: None,
        first_frame_sent: false,
    };
    loop {
        match shared.command.load(Ordering::Acquire) {
            COMMAND_CANCEL => return Terminal::Cancelled,
            COMMAND_FINALIZE => {
                source.finish(&mut pending);
                return outbox.send_rest(&mut pending, Terminal::Finalized);
            }
            _ => {}
        }
        let read = match source.read(&mut pending, POLL) {
            Ok(read) => read,
            Err(failure) => return Terminal::Failed(failure.code),
        };
        if outbox.first_frame.is_none() && !pending.is_empty() {
            outbox.first_frame = Some(elapsed_us(started_at));
        }
        if read == SourceRead::Ended {
            source.finish(&mut pending);
            return outbox.send_rest(&mut pending, Terminal::SourceEnded);
        }
        match outbox.send_complete(&mut pending) {
            Ok(None) => {}
            Ok(Some(terminal)) | Err(terminal) => return terminal,
        }
    }
}

fn end_for_finalization(control: &SessionControl) -> CaptureEnd {
    if control.finalization().reason() == Some(SessionFinalizationReason::EndpointDetected) {
        CaptureEnd::EndpointDetected
    } else {
        CaptureEnd::FinalizationRequested
    }
}

fn checked_deadline(duration: Duration) -> Result<Instant, PlatformError> {
    Instant::now()
        .checked_add(duration)
        .ok_or_else(|| error("capture_timeout_invalid"))
}

// ---------------------------------------------------------------------------
// Batch session
// ---------------------------------------------------------------------------

/// One batch capture session (see the module documentation).
pub struct BatchCaptureSession {
    producer: Producer,
    info: CaptureStreamInfo,
    max_samples: usize,
    samples: Vec<f32>,
    expected_sequence: u64,
    first_frame_after_start_us: Option<u64>,
    started_at: Instant,
    safety_deadline: Instant,
    completed: bool,
}

impl BatchCaptureSession {
    /// Start the producer; the source opens on its thread.
    ///
    /// # Errors
    /// `capture_thread_spawn_failed`.
    pub fn start(config: &CaptureConfig, open: SourceOpener) -> Result<Self, PlatformError> {
        let started_at = Instant::now();
        let queue = config.max_samples / BATCH_CAPTURE_CHUNK_SAMPLES + 2;
        let producer = Producer::spawn(
            open,
            config.max_samples,
            BATCH_CAPTURE_CHUNK_SAMPLES,
            queue,
            started_at,
        )?;
        Ok(Self {
            producer,
            info: config.info(),
            max_samples: config.max_samples,
            samples: Vec::new(),
            expected_sequence: 0,
            first_frame_after_start_us: None,
            started_at,
            safety_deadline: config.safety_deadline(started_at),
            completed: false,
        })
    }

    fn fail(&mut self, code: &'static str) -> PlatformError {
        self.completed = true;
        match checked_deadline(CAPTURE_CANCEL_BOUND).and_then(|d| self.producer.shutdown(d)) {
            Ok(()) => error(code),
            Err(cleanup) => cleanup,
        }
    }

    fn accept(&mut self, chunk: ProducedChunk) -> Result<(), PlatformError> {
        if chunk.sequence != self.expected_sequence
            || chunk.samples.is_empty()
            || self.samples.len() + chunk.samples.len() > self.max_samples
        {
            return Err(self.fail("capture_stream_sequence_invalid"));
        }
        self.expected_sequence += 1;
        if self.first_frame_after_start_us.is_none() {
            self.first_frame_after_start_us = chunk.first_frame_after_start_us;
        }
        self.samples.extend_from_slice(&chunk.samples);
        Ok(())
    }

    /// Drain until the producer closes the queue, after a Stop or the end of
    /// the source.
    fn drain_to_close(&mut self, control: &SessionControl) -> Result<Terminal, PlatformError> {
        let deadline = checked_deadline(CAPTURE_CANCEL_BOUND)?;
        loop {
            if control.is_cancelled() {
                return Err(self.fail("cancelled"));
            }
            match self.producer.recv(POLL) {
                Received::Chunk(chunk) => self.accept(chunk)?,
                Received::Empty if Instant::now() < deadline => {}
                Received::Empty => return Err(self.fail("capture_output_drain_timeout")),
                Received::Closed => {
                    self.producer
                        .join(deadline)
                        .inspect_err(|_| self.completed = true)?;
                    return Ok(self.producer.terminal());
                }
            }
        }
    }

    fn finish(&mut self, end: CaptureEnd) -> CapturedAudio {
        self.completed = true;
        CapturedAudio {
            audio: AudioChunk {
                sample_rate_hz: CAPTURE_RATE_HZ,
                channels: CAPTURE_CHANNELS,
                samples: std::mem::take(&mut self.samples),
            },
            end,
            first_frame_after_start_us: self.first_frame_after_start_us,
            capture_duration_us: elapsed_us(self.started_at),
        }
    }

    /// Map the producer's terminal to the session result.
    fn conclude(
        &mut self,
        terminal: Terminal,
        control: &SessionControl,
    ) -> Result<CapturedAudio, PlatformError> {
        self.completed = true;
        if control.is_cancelled() {
            return Err(error("cancelled"));
        }
        if let Some(code) = terminal.error_code() {
            return Err(error(code));
        }
        let end = match terminal {
            Terminal::Finalized => end_for_finalization(control),
            Terminal::SafetyLimit => CaptureEnd::SafetyLimit,
            _ => CaptureEnd::SourceEnded,
        };
        if self.samples.is_empty() {
            return Err(error(if terminal == Terminal::Finalized {
                "capture_finalized_before_audio"
            } else {
                "capture_output_invalid"
            }));
        }
        Ok(self.finish(end))
    }
}

impl AudioCaptureSession for BatchCaptureSession {
    fn info(&self) -> &CaptureStreamInfo {
        &self.info
    }

    fn wait_ready(&mut self, control: &SessionControl) -> Result<(), PlatformError> {
        if self.completed {
            return Err(error("capture_already_collected"));
        }
        let ready_deadline = self.started_at + CAPTURE_READY_BOUND;
        while self.samples.is_empty() {
            if control.is_cancelled() {
                return Err(self.fail("cancelled"));
            }
            if control.is_finalization_requested() {
                return Err(self.fail("capture_finalized_before_audio"));
            }
            match self.producer.recv(POLL) {
                Received::Chunk(chunk) => self.accept(chunk)?,
                Received::Empty if Instant::now() < ready_deadline => {}
                Received::Empty => return Err(self.fail("capture_ready_timeout")),
                Received::Closed => {
                    let terminal = self.producer.terminal();
                    let deadline = checked_deadline(CAPTURE_CANCEL_BOUND)?;
                    self.completed = true;
                    self.producer.join(deadline)?;
                    return Err(error(
                        terminal.error_code().unwrap_or("capture_output_invalid"),
                    ));
                }
            }
        }
        Ok(())
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
            if control.is_cancelled() {
                return Err(self.fail("cancelled"));
            }
            if control.is_finalization_requested() {
                self.producer.command(COMMAND_FINALIZE);
                let terminal = self.drain_to_close(control)?;
                return self.conclude(terminal, control);
            }
            match self.producer.recv(POLL) {
                Received::Chunk(chunk) => self.accept(chunk)?,
                Received::Empty => {}
                Received::Closed => {
                    let terminal = self.drain_to_close(control)?;
                    return self.conclude(terminal, control);
                }
            }
            if Instant::now() >= self.safety_deadline {
                return Err(self.fail("capture_timeout"));
            }
        }
    }
}

impl Drop for BatchCaptureSession {
    fn drop(&mut self) {
        if !self.completed {
            let _ = self
                .producer
                .shutdown(Instant::now() + CAPTURE_CANCEL_BOUND);
        }
    }
}

// ---------------------------------------------------------------------------
// Streaming session
// ---------------------------------------------------------------------------

/// One live capture session (see the module documentation).
pub struct StreamingCaptureSession {
    producer: Producer,
    info: CaptureStreamInfo,
    max_samples: usize,
    delivered_samples: usize,
    expected_sequence: u64,
    first_frame_after_start_us: Option<u64>,
    pending: Option<CaptureStreamChunk>,
    started_at: Instant,
    safety_deadline: Instant,
    finalize_sent: bool,
    completed: bool,
}

impl StreamingCaptureSession {
    /// Start the producer; the source opens on its thread.
    ///
    /// # Errors
    /// `cancelled` when `control` is already cancelled, or
    /// `capture_thread_spawn_failed`.
    pub fn start(
        config: &CaptureConfig,
        open: SourceOpener,
        control: &SessionControl,
    ) -> Result<Self, PlatformError> {
        if control.is_cancelled() {
            return Err(error("cancelled"));
        }
        let started_at = Instant::now();
        let producer = Producer::spawn(
            open,
            config.max_samples,
            STREAM_CAPTURE_CHUNK_SAMPLES,
            STREAM_CAPTURE_QUEUE_CHUNKS,
            started_at,
        )?;
        Ok(Self {
            producer,
            info: config.info(),
            max_samples: config.max_samples,
            delivered_samples: 0,
            expected_sequence: 0,
            first_frame_after_start_us: None,
            pending: None,
            started_at,
            safety_deadline: config.safety_deadline(started_at),
            finalize_sent: false,
            completed: false,
        })
    }

    fn fail(&mut self, code: &'static str) -> PlatformError {
        self.completed = true;
        match checked_deadline(CAPTURE_CANCEL_BOUND).and_then(|d| self.producer.shutdown(d)) {
            Ok(()) => error(code),
            Err(cleanup) => cleanup,
        }
    }

    fn producer_error(&mut self) -> Result<(), PlatformError> {
        match self.producer.terminal().error_code() {
            Some(code) => Err(self.fail(code)),
            None => Ok(()),
        }
    }

    fn accept(&mut self, chunk: ProducedChunk) -> Result<CaptureStreamChunk, PlatformError> {
        if chunk.sequence != self.expected_sequence || chunk.samples.is_empty() {
            return Err(self.fail("capture_stream_sequence_invalid"));
        }
        let total = self.delivered_samples + chunk.samples.len();
        if total > self.max_samples {
            return Err(self.fail("capture_output_overflow"));
        }
        self.delivered_samples = total;
        self.expected_sequence += 1;
        if self.first_frame_after_start_us.is_none() {
            self.first_frame_after_start_us = chunk.first_frame_after_start_us;
        }
        Ok(CaptureStreamChunk {
            sequence: chunk.sequence,
            audio: AudioChunk {
                sample_rate_hz: CAPTURE_RATE_HZ,
                channels: CAPTURE_CHANNELS,
                samples: chunk.samples,
            },
            first_frame_after_start_us: chunk.first_frame_after_start_us,
        })
    }

    fn finish_stream(
        &mut self,
        control: &SessionControl,
    ) -> Result<CaptureStreamEvent, PlatformError> {
        let deadline = checked_deadline(CAPTURE_CANCEL_BOUND)?;
        self.completed = true;
        self.producer.join(deadline)?;
        let terminal = self.producer.terminal();
        if control.is_cancelled() {
            return Err(error("cancelled"));
        }
        if let Some(code) = terminal.error_code() {
            return Err(error(code));
        }
        if self.delivered_samples == 0 {
            return Err(error(if terminal == Terminal::Finalized {
                "capture_finalized_before_audio"
            } else {
                "capture_output_invalid"
            }));
        }
        let end = match terminal {
            Terminal::Finalized => end_for_finalization(control),
            Terminal::SafetyLimit => CaptureEnd::SafetyLimit,
            _ => CaptureEnd::SourceEnded,
        };
        Ok(CaptureStreamEvent::End(CaptureStreamSummary {
            end,
            total_samples: self.delivered_samples as u64,
            first_frame_after_start_us: self.first_frame_after_start_us,
            capture_duration_us: elapsed_us(self.started_at),
        }))
    }

    fn poll_next(
        &mut self,
        control: &SessionControl,
        ready_deadline: Option<Instant>,
    ) -> Result<CaptureStreamEvent, PlatformError> {
        loop {
            if control.is_cancelled() {
                return Err(self.fail("cancelled"));
            }
            self.producer_error()?;
            if control.is_finalization_requested() && !self.finalize_sent {
                self.producer.command(COMMAND_FINALIZE);
                self.finalize_sent = true;
            }
            match self.producer.recv(POLL) {
                Received::Chunk(chunk) => {
                    if control.is_cancelled() {
                        return Err(self.fail("cancelled"));
                    }
                    return self.accept(chunk).map(CaptureStreamEvent::Audio);
                }
                Received::Closed => return self.finish_stream(control),
                Received::Empty => {}
            }
            let now = Instant::now();
            if ready_deadline.is_some_and(|deadline| now >= deadline) {
                return Err(self.fail("capture_ready_timeout"));
            }
            if now >= self.safety_deadline {
                return Err(self.fail("capture_timeout"));
            }
        }
    }
}

impl StreamingAudioCaptureSession for StreamingCaptureSession {
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
        match self.poll_next(control, Some(self.started_at + CAPTURE_READY_BOUND))? {
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
            return Err(self.fail("cancelled"));
        }
        // Known loss preempts even a prefetched chunk: no audio from a
        // non-contiguous stream may reach ASR.
        self.producer_error()?;
        if let Some(chunk) = self.pending.take() {
            return Ok(CaptureStreamEvent::Audio(chunk));
        }
        self.poll_next(control, None)
    }

    fn abort(&mut self, deadline: Instant) -> Result<(), PlatformError> {
        if self.completed {
            return Ok(());
        }
        self.completed = true;
        self.producer.shutdown(deadline)
    }
}

impl Drop for StreamingCaptureSession {
    fn drop(&mut self) {
        if !self.completed {
            let _ = self
                .producer
                .shutdown(Instant::now() + CAPTURE_CANCEL_BOUND);
        }
    }
}

/// The longest capture gap filled with silence: one second.
pub const MAX_FILLED_GAP_SECONDS: u64 = 1;

/// Frames a capture gap left out, from where the next packet was expected
/// (`expected`, in device frames) and where the device says it starts
/// (`position`). The audio engine reports a gap when it had to drop samples
/// (the reading thread was late, or the driver glitched); filling it with
/// silence of the same length keeps the dictation going with its timing
/// intact, as PipeWire does on Linux. A gap longer than
/// [`MAX_FILLED_GAP_SECONDS`] is `None`: the stream was lost, not glitched.
#[must_use]
pub fn gap_frames(expected: u64, position: u64, rate_hz: u32) -> Option<usize> {
    let gap = position.saturating_sub(expected);
    if gap > MAX_FILLED_GAP_SECONDS * u64::from(rate_hz) {
        return None;
    }
    usize::try_from(gap).ok()
}

/// Where a WASAPI stream is, from the device positions of its packets, and
/// the frames a discontinuity left out.
///
/// The position counts units that are not always the stream's frames: a
/// stream the audio engine converts (`AUTOCONVERTPCM`, 16 kHz asked of a
/// 48 kHz device) reports device frames, three per stream frame. The ratio
/// is learned from packets that follow each other without a gap (starting
/// from the caller's guess), and a gap is turned into stream frames with it.
/// Without this a glitch would be filled with three times too much silence,
/// and a one-second gap would end the stream as an overrun.
#[derive(Debug, Clone)]
pub struct PositionTracker {
    last: Option<(u64, u32)>,
    units_per_frame: f64,
}

impl PositionTracker {
    /// `units_per_frame` is the guess until a packet pair shows it.
    #[must_use]
    pub fn new(units_per_frame: f64) -> Self {
        Self {
            last: None,
            units_per_frame: if units_per_frame.is_finite() && units_per_frame > 0.0 {
                units_per_frame
            } else {
                1.0
            },
        }
    }

    /// The position units one stream frame takes, as last learned.
    #[must_use]
    pub fn units_per_frame(&self) -> f64 {
        self.units_per_frame
    }

    /// A packet of `frames` stream frames at device `position`; the stream
    /// frames missing before it (`Some(0)` without a discontinuity, and for
    /// the first packet, whose flag is the stream starting). `None` for a
    /// gap over [`MAX_FILLED_GAP_SECONDS`] at `rate_hz`.
    pub fn packet(
        &mut self,
        position: u64,
        frames: u32,
        discontinuity: bool,
        rate_hz: u32,
    ) -> Option<usize> {
        let missing = match self.last {
            None => 0,
            Some((last_position, last_frames)) => {
                let step = position.saturating_sub(last_position) as f64;
                if !discontinuity {
                    if last_frames > 0 && step > 0.0 {
                        self.units_per_frame = step / f64::from(last_frames);
                    }
                    0
                } else {
                    let expected = f64::from(last_frames) * self.units_per_frame;
                    ((step - expected).max(0.0) / self.units_per_frame).round() as u64
                }
            }
        };
        self.last = Some((position, frames));
        if missing > MAX_FILLED_GAP_SECONDS * u64::from(rate_hz) {
            return None;
        }
        usize::try_from(missing).ok()
    }
}

#[cfg(test)]
mod tests;
