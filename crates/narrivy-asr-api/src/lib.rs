//! Speech-recognition engine contracts.
//!
//! P1 contract: engines expose a streaming session capable of Started/Partial/
//! Final events, cooperative cancellation, and optional timestamps/confidence
//! capabilities. The Phase-0 synchronous scaffold is superseded; `transcribe`
//! remains as the bounded one-shot convenience built on the stream path.
//! Recognition requests keep spoken language, written language/script,
//! translation, locale, and context bias as distinct typed fields.

mod request;
pub mod resident_protocol;
pub mod self_test;

pub use request::{
    AsrCapabilities, AsrRequest, AsrTag, CONFLICTING_LANGUAGE_SETTINGS, ContextBias,
    INVALID_CAPABILITIES, INVALID_CONTEXT_BIAS, INVALID_LANGUAGE_TAG, MAX_ATTESTED_TAGS,
    MAX_CONTEXT_HINT_BYTES, MAX_CONTEXT_HINTS, MAX_TAG_BYTES, REQUEST_FIELD_TOO_LONG,
    SpokenLanguage, TOO_MANY_CONTEXT_HINTS, UNSUPPORTED_CONTEXT_BIAS, UNSUPPORTED_LANGUAGE_DETECT,
    UNSUPPORTED_LOCALE, UNSUPPORTED_OUTPUT_LANGUAGE, UNSUPPORTED_REQUEST_APPLY, UNSUPPORTED_SCRIPT,
    UNSUPPORTED_SPOKEN_LANGUAGE, UNSUPPORTED_TRANSLATION, WrittenOutput,
};

use narrivy_types::{AudioChunk, SessionCancellation, SessionId, Transcript};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsrError {
    pub code: &'static str,
    pub message: String,
}

impl AsrError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// One event from a running ASR stream. Sequence numbers are assigned by the
/// engine and are monotonically increasing within a session; consumers reject
/// stale (lower-than-observed) sequences.
#[derive(Debug, Clone, PartialEq)]
pub enum AsrEvent {
    /// Stream accepted; recognition is warming/ready for audio.
    Started { session: SessionId },
    /// Non-final hypothesis. `transcript.is_final` is always false here.
    Partial { seq: u64, transcript: Transcript },
    /// Final transcript for the accumulated utterance segment.
    Final {
        seq: u64,
        transcript: Transcript,
        confidence: Option<f32>,
        first_audio_offset_ms: Option<u64>,
    },
    /// Engine-level failure; the stream is finished after this event.
    Error(AsrError),
}

/// Cancellation signal shared with a running ASR stream.
///
/// Engines must observe it between audio pushes at the latest; long-running
/// implementations should also observe it inside decode loops when practical.
pub type CancelToken = SessionCancellation;

/// One-shot portable ASR boundary for a bounded recovery session. Streaming
/// engines can implement this through their stream contract, but callers keep
/// one cancellation signal across every stage.
pub trait OneShotAsrPort: Send {
    fn recognize(
        &mut self,
        session: SessionId,
        audio: &AudioChunk,
        cancellation: &SessionCancellation,
    ) -> Result<Transcript, AsrError>;

    /// Default path: a default request is forwarded to [`Self::recognize`].
    /// Non-default requests are rejected unless the engine overrides this
    /// method and applies the settings it advertised.
    fn recognize_with(
        &mut self,
        session: SessionId,
        audio: &AudioChunk,
        cancellation: &SessionCancellation,
        request: &AsrRequest,
    ) -> Result<Transcript, AsrError> {
        if request.is_default() {
            return self.recognize(session, audio, cancellation);
        }
        Err(AsrError::new(
            UNSUPPORTED_REQUEST_APPLY,
            "this engine cannot apply the requested language or context settings",
        ))
    }
}

/// A live ASR stream bound to one session.
///
/// Contract (REL-007): every stream has explicit cancellation and bounded
/// finish semantics — `finish` returns once events up to cancellation/final
/// are drained or `timeout` elapses (engine returns `AsrError` timeout code).
pub trait AsrStream: Send {
    fn session(&self) -> SessionId;

    /// Clonable handle for cancelling an in-flight synchronous engine call
    /// from another owner thread. The returned handle controls this stream;
    /// it is not a detached or advisory token.
    fn cancellation(&self) -> CancelToken;

    /// Push one chunk of audio; engine may emit events into the sink.
    fn push_audio(&mut self, chunk: &AudioChunk) -> Result<(), AsrError>;

    /// Drain pending events without blocking indefinitely.
    fn poll_event(&mut self) -> Option<AsrEvent>;

    /// Request final result for audio pushed so far (end-of-utterance).
    fn finalize(&mut self) -> Result<(), AsrError>;

    /// Cooperative cancellation + bounded join of the underlying worker.
    fn cancel(&self) {
        self.cancellation().cancel();
    }

    /// Bounded wait until the stream worker has stopped after `cancel`.
    fn finish(&mut self, timeout: std::time::Duration) -> Result<(), AsrError>;
}

/// Engine factory contract. P5 evolves this toward device/runtime selection;
/// P1 only requires that engines can open sessions with cancellation.
pub trait AsrEngine: Send {
    fn capabilities(&self) -> AsrCapabilities;

    /// Open a streaming session under a caller-owned cancellation signal.
    /// Admission and the returned stream observe the same signal, so a native
    /// setup operation cannot become an uncancellable pre-stream gap. The
    /// engine owns no global mutable state that outlives the returned stream.
    fn open_stream(
        &mut self,
        session: SessionId,
        cancellation: &CancelToken,
    ) -> Result<Box<dyn AsrStream>, AsrError>;

    /// Open a session with distinct language, translation, locale, and context
    /// settings. The default implementation validates against
    /// [`Self::capabilities`] and, when the request is the engine default or
    /// an equivalent sole-language selection, forwards to [`Self::open_stream`].
    /// Engines that advertise additional settings must override this method
    /// and apply every accepted field.
    fn open_stream_with(
        &mut self,
        session: SessionId,
        cancellation: &CancelToken,
        request: &AsrRequest,
    ) -> Result<Box<dyn AsrStream>, AsrError> {
        let capabilities = self.capabilities();
        capabilities.validate(request)?;
        if request.applies_as_engine_default(&capabilities) {
            return self.open_stream(session, cancellation);
        }
        Err(AsrError::new(
            UNSUPPORTED_REQUEST_APPLY,
            "this engine cannot apply the requested language or context settings",
        ))
    }

    /// Bounded one-shot transcription convenience for tests/simple lanes.
    fn transcribe(&mut self, audio: &AudioChunk) -> Result<Transcript, AsrError> {
        let session = SessionId(0xC0FFEE);
        let cancellation = CancelToken::new();
        let mut stream = self.open_stream(session, &cancellation)?;
        finish_one_shot(&mut *stream, audio)
    }

    /// Bounded one-shot recognition with caller-supplied language and context
    /// settings. Engines reject any setting they cannot both attest and apply.
    fn transcribe_with(
        &mut self,
        audio: &AudioChunk,
        request: &AsrRequest,
    ) -> Result<Transcript, AsrError> {
        let session = SessionId(0xC0FFEE);
        let cancellation = CancelToken::new();
        let mut stream = self.open_stream_with(session, &cancellation, request)?;
        finish_one_shot(&mut *stream, audio)
    }
}

fn finish_one_shot(stream: &mut dyn AsrStream, audio: &AudioChunk) -> Result<Transcript, AsrError> {
    stream.push_audio(audio)?;
    stream.finalize()?;
    let mut final_text = None;
    while let Some(event) = stream.poll_event() {
        if let AsrEvent::Final { transcript, .. } = event {
            final_text = Some(transcript);
        }
    }
    stream.finish(std::time::Duration::from_secs(2))?;
    final_text.ok_or_else(|| AsrError::new("no_final", "stream ended without a Final event"))
}

impl AsrEngine for Box<dyn AsrEngine> {
    fn capabilities(&self) -> AsrCapabilities {
        (**self).capabilities()
    }

    fn open_stream(
        &mut self,
        session: SessionId,
        cancellation: &CancelToken,
    ) -> Result<Box<dyn AsrStream>, AsrError> {
        (**self).open_stream(session, cancellation)
    }

    fn open_stream_with(
        &mut self,
        session: SessionId,
        cancellation: &CancelToken,
        request: &AsrRequest,
    ) -> Result<Box<dyn AsrStream>, AsrError> {
        (**self).open_stream_with(session, cancellation, request)
    }

    fn transcribe(&mut self, audio: &AudioChunk) -> Result<Transcript, AsrError> {
        (**self).transcribe(audio)
    }

    fn transcribe_with(
        &mut self,
        audio: &AudioChunk,
        request: &AsrRequest,
    ) -> Result<Transcript, AsrError> {
        (**self).transcribe_with(audio, request)
    }
}
