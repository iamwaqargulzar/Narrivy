//! Deterministic ASR adapter for tests and early integration.

use narrivy_asr_api::{AsrCapabilities, AsrEngine, AsrError, AsrEvent, AsrStream, CancelToken};
use narrivy_types::{AudioChunk, SessionId, Transcript};

/// Mock engine emitting deterministic Partial+Final events from a fixed
/// response. Emits an error instead when `fail_code` is set.
#[derive(Clone)]
pub struct MockAsr {
    response: String,
    fail_code: Option<&'static str>,
}

pub struct MockStream {
    session: SessionId,
    response: String,
    fail_code: Option<&'static str>,
    buffered: Vec<AsrEvent>,
    next_seq: u64,
    finalized: bool,
    cancelled: CancelToken,
    got_audio: bool,
}

impl MockAsr {
    pub fn new(response: impl Into<String>) -> Self {
        Self {
            response: response.into(),
            fail_code: None,
        }
    }

    /// Engine that fails deterministically (typed error path testing).
    pub fn failing(fail_code: &'static str) -> Self {
        Self {
            response: String::new(),
            fail_code: Some(fail_code),
        }
    }
}

impl AsrEngine for MockAsr {
    fn capabilities(&self) -> AsrCapabilities {
        AsrCapabilities {
            streaming: true,
            language_detection: false,
            translation: false,
            confidence: true,
            hotword_biasing: false,
            timestamps: false,
            ..AsrCapabilities::default()
        }
    }

    fn open_stream(
        &mut self,
        session: SessionId,
        cancellation: &CancelToken,
    ) -> Result<Box<dyn AsrStream>, AsrError> {
        Ok(Box::new(MockStream {
            session,
            response: self.response.clone(),
            fail_code: self.fail_code,
            buffered: vec![AsrEvent::Started { session }],
            next_seq: 0,
            finalized: false,
            cancelled: cancellation.clone(),
            got_audio: false,
        }))
    }

    fn transcribe(&mut self, _audio: &AudioChunk) -> Result<Transcript, AsrError> {
        if let Some(code) = self.fail_code {
            return Err(AsrError::new(code, "mock failure requested"));
        }
        Ok(Transcript {
            text: self.response.clone(),
            is_final: true,
        })
    }
}

impl MockStream {
    fn error(&self) -> AsrError {
        AsrError::new(
            self.fail_code.unwrap_or("mock_cancelled"),
            "mock failure requested",
        )
    }
}

impl AsrStream for MockStream {
    fn session(&self) -> SessionId {
        self.session
    }

    fn cancellation(&self) -> CancelToken {
        self.cancelled.clone()
    }

    fn push_audio(&mut self, _chunk: &AudioChunk) -> Result<(), AsrError> {
        if self.cancelled.is_cancelled() {
            return Err(self.error());
        }
        self.got_audio = true;
        // Deterministic partial on each push after the first.
        if self.next_seq == 0 {
            let partial = Transcript {
                text: self.response.clone(),
                is_final: false,
            };
            self.buffered.push(AsrEvent::Partial {
                seq: 1,
                transcript: partial,
            });
            self.next_seq = 1;
        }
        Ok(())
    }

    fn poll_event(&mut self) -> Option<AsrEvent> {
        if !self.buffered.is_empty() {
            return Some(self.buffered.remove(0));
        }
        None
    }

    fn finalize(&mut self) -> Result<(), AsrError> {
        if self.finalized {
            return Ok(());
        }
        if let Some(code) = self.fail_code {
            self.buffered.push(AsrEvent::Error(self.error()));
            self.finalized = true;
            let _ = code;
            return Ok(());
        }
        if !self.got_audio && self.response.is_empty() {
            self.buffered.push(AsrEvent::Error(AsrError::new(
                "no_audio",
                "no audio pushed",
            )));
            self.finalized = true;
            return Ok(());
        }
        self.next_seq += 1;
        self.buffered.push(AsrEvent::Final {
            seq: self.next_seq,
            transcript: Transcript {
                text: self.response.clone(),
                is_final: true,
            },
            confidence: Some(0.99),
            first_audio_offset_ms: None,
        });
        self.finalized = true;
        Ok(())
    }

    fn finish(&mut self, timeout: std::time::Duration) -> Result<(), AsrError> {
        let _ = timeout;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_stream_emits_started_partial_final_in_order() -> Result<(), AsrError> {
        let mut engine = MockAsr::new("hello world");
        let caps = engine.capabilities();
        assert!(caps.streaming);
        let cancellation = CancelToken::new();
        let mut stream = engine.open_stream(SessionId(1), &cancellation)?;
        stream.push_audio(&AudioChunk {
            sample_rate_hz: 16000,
            channels: 1,
            samples: vec![0.0; 160],
        })?;
        stream.finalize()?;
        let mut seen = Vec::new();
        while let Some(ev) = stream.poll_event() {
            match ev {
                AsrEvent::Started { .. } => seen.push("started"),
                AsrEvent::Partial { .. } => seen.push("partial"),
                AsrEvent::Final {
                    transcript,
                    confidence,
                    ..
                } => {
                    seen.push("final");
                    assert_eq!(transcript.text, "hello world");
                    assert!(transcript.is_final);
                    let confidence = confidence.ok_or_else(|| {
                        AsrError::new("missing_confidence", "mock final omitted confidence")
                    })?;
                    assert!(confidence > 0.9);
                }
                AsrEvent::Error(error) => return Err(error),
            }
        }
        assert_eq!(seen, ["started", "partial", "final"]);
        stream.finish(std::time::Duration::from_millis(100))?;
        Ok(())
    }

    #[test]
    fn mock_failing_engine_reports_typed_error() -> Result<(), AsrError> {
        let mut engine = MockAsr::failing("engine_dead");
        let result = engine.transcribe(&AudioChunk {
            sample_rate_hz: 16000,
            channels: 1,
            samples: vec![],
        });
        match result {
            Err(error) => assert_eq!(error.code, "engine_dead"),
            Ok(_) => {
                return Err(AsrError::new(
                    "unexpected_success",
                    "failing mock returned a transcript",
                ));
            }
        }
        Ok(())
    }
}
