//! Platform capability contracts. No concrete OS APIs belong here.
//!
//! Phase-0 scaffold: delivery/shortcut signatures are provisional; P1/P3 must evolve them
//! to typed transactions, outcomes, verification, activation/release events and bounded fallback.

/// Why capture stopped accepting audio. This never carries sample data.
///
/// The type lives in portable domain types because the durable recovery
/// journal and the client-facing status render the same typed reason; it is
/// re-exported here so capture implementations and core finalization share one
/// definition.
pub use narrivy_types::CaptureEnd;
use narrivy_types::{
    AudioChunk, Capability, DeliveryIntent, DeliveryReport, SessionCancellation, SessionControl,
    TargetContext,
};
use std::time::Instant;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformError {
    pub code: &'static str,
    pub message: String,
}

pub trait PlatformCapabilities: Send + Sync {
    fn has(&self, capability: Capability) -> bool;
}

pub trait TargetProvider: Send + Sync {
    fn capture_target(&self) -> Result<TargetContext, PlatformError>;
}

pub trait TextDelivery: Send + Sync {
    fn insert_text(&self, target: &TargetContext, text: &str) -> Result<(), PlatformError>;
}

pub trait ShortcutProvider: Send + Sync {
    fn register_primary_trigger(&self) -> Result<(), PlatformError>;
}

/// Content-free identity and negotiated format of one opened capture stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureStreamInfo {
    /// Exact selection expression handed to the platform adapter. This is not
    /// promoted to a resolved hardware identity when the platform uses a
    /// dynamic alias such as PipeWire's default source.
    pub source_selection: String,
    pub sample_rate_hz: u32,
    pub channels: u16,
}

/// Final in-memory audio and content-free monotonic measurements from one
/// owned capture session. Durations are relative; wall-clock adjustment cannot
/// produce false latency values.
pub struct CapturedAudio {
    pub audio: AudioChunk,
    pub end: CaptureEnd,
    pub first_frame_after_start_us: Option<u64>,
    pub capture_duration_us: u64,
}

/// One bounded audio block emitted by a live capture session. Sequence
/// numbers start at zero and must be contiguous; callers reject gaps instead
/// of silently accepting dropped capture data.
pub struct CaptureStreamChunk {
    pub sequence: u64,
    pub audio: AudioChunk,
    pub first_frame_after_start_us: Option<u64>,
}

/// Content-free completion measurements for a streamed capture. Audio is
/// carried only by preceding [`CaptureStreamEvent::Audio`] values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureStreamSummary {
    pub end: CaptureEnd,
    pub total_samples: u64,
    pub first_frame_after_start_us: Option<u64>,
    pub capture_duration_us: u64,
}

/// Ordered output from one live capture session.
pub enum CaptureStreamEvent {
    Audio(CaptureStreamChunk),
    End(CaptureStreamSummary),
}

/// One opened capture session. Implementations own every OS process/thread and
/// must return only after they have boundedly stopped and reaped those owners.
pub trait AudioCaptureSession: Send {
    fn info(&self) -> &CaptureStreamInfo;

    /// Wait until the concrete source has produced at least one complete
    /// frame. Process creation alone is not capture readiness: an adapter may
    /// still fail while negotiating permissions, routing, or the device.
    /// Implementations retain any frame consumed by this probe for `collect`.
    fn wait_ready(&mut self, control: &SessionControl) -> Result<(), PlatformError>;

    fn collect(&mut self, control: &SessionControl) -> Result<CapturedAudio, PlatformError>;

    /// [`Self::collect`], also handing `observe` every sample captured so far
    /// while recording continues (for a live preview). `observe` runs on the
    /// capture loop, so it must return quickly. The default observes nothing.
    fn collect_observed(
        &mut self,
        control: &SessionControl,
        observe: &mut dyn FnMut(&[f32]),
    ) -> Result<CapturedAudio, PlatformError> {
        let _ = observe;
        self.collect(control)
    }
}

/// Portable capture factory. `start` owns/spawns the concrete capture resource;
/// readiness is established separately by [`AudioCaptureSession::wait_ready`].
pub trait AudioCapturePort: Send {
    fn start(&mut self) -> Result<Box<dyn AudioCaptureSession>, PlatformError>;
}

/// One opened, bounded live capture stream. The implementation owns every OS
/// child and producer thread. Queue saturation is a typed terminal error;
/// samples must never be silently dropped or overwritten.
pub trait StreamingAudioCaptureSession: Send {
    fn info(&self) -> &CaptureStreamInfo;

    /// Establish actual signal flow by retaining the first complete chunk for
    /// the next call to [`Self::next_event`].
    fn wait_ready(&mut self, control: &SessionControl) -> Result<(), PlatformError>;

    /// Return the next contiguous audio block or the single terminal summary.
    /// Cancellation is terminal and wins races with finalization.
    fn next_event(&mut self, control: &SessionControl)
    -> Result<CaptureStreamEvent, PlatformError>;

    /// Abort because a downstream stage failed or declined the stream. The
    /// caller supplies the transaction's existing absolute deadline; cleanup
    /// may not silently open a fresh timeout window.
    fn abort(&mut self, deadline: Instant) -> Result<(), PlatformError>;
}

/// Portable factory for live bounded capture. This is separate from the
/// batch factory so existing one-shot paths cannot accidentally claim live
/// streaming merely because their adapter buffers a whole recording.
pub trait StreamingAudioCapturePort: Send {
    fn start_stream(
        &mut self,
        control: &SessionControl,
    ) -> Result<Box<dyn StreamingAudioCaptureSession>, PlatformError>;
}

/// Portable delivery boundary. The caller persists the intent before calling
/// this method; the adapter reports the backend it actually used.
pub trait DeliveryPort: Send {
    fn deliver(
        &mut self,
        intent: &DeliveryIntent,
        text: &str,
        cancellation: &SessionCancellation,
    ) -> Result<DeliveryReport, PlatformError>;
}
