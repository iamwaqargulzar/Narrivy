//! transcribe.cpp ASR engine binding (GGUF lane: CPU correctness path).
//!
//! # LICENSE + PIN STATUS (P2, 2026-08-22)
//!
//! transcribe.cpp (handy-computer org, announced 2026-06-30) is the owner-
//! selected reference ASR runtime for the CPU/iGPU/dGPU lanes. Its MIT code
//! license is VERIFIED. The retained CLI-sidecar rollback path and Recovery 4B
//! resident worker pin upstream release v0.2.3 and its published
//! safe-wrapper/sys pair at 0.2.3. Digests and integration rules are recorded
//! in BUILD-NOTES.md:
//!
//! - This crate NEVER vendors or copies upstream code. The `runtime` world
//!   drives an operator-built CLI sidecar as a versioned subprocess (see
//!   `sidecar`); nothing upstream links into Narrivy binaries at P2.
//! - Direct staticlib linking through the local `transcribe-sys` stub stays
//!   OUT of every build: its hand-written declarations are hypotheses that
//!   must be rewritten against `include/transcribe.h` before any use (P5).
//! - The real path lives behind the `runtime` cargo feature (default: off),
//!   so default workspace builds need no C toolchain or sidecar at all.
//! - See BUILD-NOTES.md for the pinned revision, model contract, env vars,
//!   and known limitations. Narrivy's current binding is final-only even though
//!   upstream supports streaming for specific streaming-capable model families.

mod catalog;
mod prep;

#[cfg(feature = "resident-worker")]
pub mod resident;
#[cfg(feature = "resident-worker")]
pub use narrivy_asr_api::resident_protocol;
#[cfg(feature = "resident-worker")]
pub use narrivy_asr_api::self_test;
#[cfg(feature = "runtime")]
mod sidecar;

#[cfg(feature = "resident-worker")]
pub use resident::ResidentWorkerEngine;

pub use catalog::{CatalogError, GgufModelSpec, MODEL_CATALOG, lookup};
pub use prep::{TARGET_SAMPLE_RATE_HZ, ensure_mono_16k};

use narrivy_asr_api::{AsrCapabilities, AsrEngine, AsrError, AsrEvent, AsrStream, OneShotAsrPort};
use narrivy_types::{AudioChunk, SessionCancellation, SessionId, Transcript};

/// Minimal safe GGUF identity validation used before the configured model can
/// be advertised as usable. This verifies container identity, not a product
/// name inferred from a pathname. The native sidecar remains responsible for
/// full model compatibility at construction/decode time.
pub fn validate_gguf_model(path: &std::path::Path) -> Result<(), AsrError> {
    let metadata = std::fs::metadata(path)
        .map_err(|_| AsrError::new("model_missing", "configured GGUF model is unavailable"))?;
    if !metadata.is_file() {
        return Err(AsrError::new(
            "model_missing",
            "configured GGUF model is unavailable",
        ));
    }
    let mut header = [0u8; 8];
    let mut file = std::fs::File::open(path)
        .map_err(|_| AsrError::new("model_missing", "configured GGUF model is unavailable"))?;
    use std::io::Read;
    file.read_exact(&mut header)
        .map_err(|_| AsrError::new("model_invalid", "configured model is not a GGUF container"))?;
    if &header[..4] != b"GGUF" {
        return Err(AsrError::new(
            "model_invalid",
            "configured model is not a GGUF container",
        ));
    }
    let version = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
    if !(1..=3).contains(&version) {
        return Err(AsrError::new(
            "model_unsupported",
            "configured GGUF version is unsupported",
        ));
    }
    Ok(())
}

/// Typed code used when the binary was built without native runtime support.
pub const RUNTIME_UNAVAILABLE_CODE: &str = "runtime_unavailable";

#[cfg(not(feature = "runtime"))]
fn runtime_unavailable() -> AsrError {
    AsrError::new(
        RUNTIME_UNAVAILABLE_CODE,
        "built without the 'runtime' feature; enable it and provide the \
         pinned transcribe.cpp sidecar to run real ASR",
    )
}

/// Truthful capability report shared by both build worlds. Narrivy does not yet
/// expose upstream's model-dependent streaming API, and its current qualified
/// Whisper model is final-only. No confidence/timestamp/hotword surface is
/// verified either.
fn capabilities() -> AsrCapabilities {
    AsrCapabilities::default()
}

/// Streaming partial stub error code.
pub const STREAMING_NOT_YET_SUPPORTED_CODE: &str = "streaming_not_yet_supported";

/// Tip returned with the streaming stub error.
pub const STREAMING_TIP: &str =
    "this Narrivy transcribe.cpp binding is final-only; use finalize() for the final transcript";

/// Return the stable unsupported result for callers probing streaming.
/// The sequence detail is diagnostic contract data only; no partial result is
/// simulated or claimed.
pub fn stream_partial_stub(next_seq: u64) -> Result<AsrEvent, AsrError> {
    let seq = next_seq + 1;
    Err(AsrError::new(
        STREAMING_NOT_YET_SUPPORTED_CODE,
        format!("{STREAMING_TIP} (next_seq would be {seq})"),
    ))
}

// ---------------------------------------------------------------------------
// Default world (feature `runtime` OFF)
// ---------------------------------------------------------------------------

/// Engine handle for the transcribe.cpp GGUF lane.
///
/// Default world: an inert placeholder whose constructors always succeed while
/// recognition calls fail with [`RUNTIME_UNAVAILABLE_CODE`].
#[cfg(not(feature = "runtime"))]
#[derive(Default)]
pub struct TranscribeCppEngine {
    model_path: Option<std::path::PathBuf>,
}

#[cfg(not(feature = "runtime"))]
impl TranscribeCppEngine {
    /// Creates an unloaded engine. Always succeeds in this build world.
    pub fn new() -> Result<Self, AsrError> {
        Ok(Self { model_path: None })
    }

    /// Records the intended GGUF model path without touching the filesystem.
    pub fn with_model_path(path: impl Into<std::path::PathBuf>) -> Result<Self, AsrError> {
        Ok(Self {
            model_path: Some(path.into()),
        })
    }

    /// Model path recorded at construction, if any.
    pub fn model_path(&self) -> Option<&std::path::Path> {
        self.model_path.as_deref()
    }
}

#[cfg(not(feature = "runtime"))]
impl AsrEngine for TranscribeCppEngine {
    fn capabilities(&self) -> AsrCapabilities {
        capabilities()
    }

    fn open_stream(
        &mut self,
        _session: SessionId,
        _cancellation: &narrivy_asr_api::CancelToken,
    ) -> Result<Box<dyn AsrStream>, AsrError> {
        Err(runtime_unavailable())
    }

    fn transcribe(&mut self, _audio: &AudioChunk) -> Result<Transcript, AsrError> {
        Err(runtime_unavailable())
    }
}

#[cfg(not(feature = "runtime"))]
impl OneShotAsrPort for TranscribeCppEngine {
    fn recognize(
        &mut self,
        _: SessionId,
        _: &AudioChunk,
        _: &SessionCancellation,
    ) -> Result<Transcript, AsrError> {
        Err(runtime_unavailable())
    }
}

// ---------------------------------------------------------------------------
// Runtime world (feature `runtime` ON)
// ---------------------------------------------------------------------------

/// Engine handle for the transcribe.cpp GGUF lane (sidecar runtime wired).
#[cfg(feature = "runtime")]
#[derive(Default)]
pub struct TranscribeCppEngine {
    model: Option<sidecar::LoadedModel>,
}

#[cfg(feature = "runtime")]
impl TranscribeCppEngine {
    /// Creates an engine without a loaded model; streams opened from it fail
    /// with `model_not_loaded` until [`Self::with_model_path`] succeeds.
    pub fn new() -> Result<Self, AsrError> {
        Ok(Self { model: None })
    }

    /// Loads a GGUF model from `path`.
    ///
    /// The file is validated in safe Rust first (`model_missing`,
    /// `model_invalid`, or `model_unsupported`) so bad
    /// paths never reach the FFI boundary; the hypothesized upstream loader
    /// then decides parseability (`model_load_failed`).
    pub fn with_model_path(path: impl Into<std::path::PathBuf>) -> Result<Self, AsrError> {
        let path = path.into();
        validate_gguf_model(&path)?;
        let model = Some(sidecar::LoadedModel::load(&path)?);
        Ok(Self { model })
    }

    /// Loaded-model state, for diagnostics and tests.
    pub fn is_model_loaded(&self) -> bool {
        self.model.is_some()
    }
}

#[cfg(feature = "runtime")]
impl AsrEngine for TranscribeCppEngine {
    fn capabilities(&self) -> AsrCapabilities {
        capabilities()
    }

    fn open_stream(
        &mut self,
        session: SessionId,
        cancellation: &narrivy_asr_api::CancelToken,
    ) -> Result<Box<dyn AsrStream>, AsrError> {
        let model = self.model.as_ref().ok_or_else(|| {
            AsrError::new(
                "model_not_loaded",
                "no GGUF model loaded; call with_model_path first",
            )
        })?;
        Ok(Box::new(sidecar::SidecarStream::with_cancellation(
            session,
            model.shared(),
            cancellation.clone(),
        )))
    }

    fn transcribe(&mut self, audio: &AudioChunk) -> Result<Transcript, AsrError> {
        // Bounded one-shot convenience built on the stream path (API contract);
        // the inherited default impl would work too, but an explicit override
        // keeps the error text identical to open_stream failures above.
        let session = SessionId(0xC0FFEE);
        let cancellation = SessionCancellation::new();
        let mut stream = self.open_stream(session, &cancellation)?;
        stream.push_audio(audio)?;
        stream.finalize()?;
        let mut final_text = None;
        while let Some(event) = stream.poll_event() {
            if let narrivy_asr_api::AsrEvent::Final { transcript, .. } = event {
                final_text = Some(transcript);
            }
        }
        stream.finish(std::time::Duration::from_secs(2))?;
        final_text.ok_or_else(|| AsrError::new("no_final", "stream ended without a Final event"))
    }
}

#[cfg(feature = "runtime")]
impl OneShotAsrPort for TranscribeCppEngine {
    fn recognize(
        &mut self,
        session: SessionId,
        audio: &AudioChunk,
        cancellation: &SessionCancellation,
    ) -> Result<Transcript, AsrError> {
        let model = self
            .model
            .as_ref()
            .ok_or_else(|| AsrError::new("model_not_loaded", "no GGUF model loaded"))?;
        let mut stream = sidecar::SidecarStream::with_cancellation(
            session,
            model.shared(),
            cancellation.clone(),
        );
        stream.push_audio(audio)?;
        stream.finalize()?;
        while let Some(event) = stream.poll_event() {
            match event {
                AsrEvent::Final { transcript, .. } => return Ok(transcript),
                AsrEvent::Error(err) => return Err(err),
                AsrEvent::Started { .. } | AsrEvent::Partial { .. } => {}
            }
        }
        Err(AsrError::new(
            "no_final",
            "transcribe.cpp produced no final result",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), String>;

    fn test_ok<T, E: std::fmt::Debug>(result: Result<T, E>) -> Result<T, String> {
        result.map_err(|error| format!("unexpected test failure: {error:?}"))
    }

    fn expect_asr_error<T>(
        result: Result<T, AsrError>,
        operation: &str,
    ) -> Result<AsrError, String> {
        match result {
            Err(error) => Ok(error),
            Ok(_) => Err(format!("{operation} unexpectedly succeeded")),
        }
    }

    /// Capability truth is independent of load success; factored so the
    /// constructor test can assert it without constructing an engine.
    #[cfg(feature = "runtime")]
    fn loaded_capabilities_still_truthful() -> bool {
        capabilities()
            == AsrCapabilities {
                streaming: false,
                language_detection: false,
                translation: false,
                confidence: false,
                hotword_biasing: false,
                timestamps: false,
                ..AsrCapabilities::default()
            }
    }

    #[test]
    fn constructors_succeed_and_capabilities_are_truthful() -> TestResult {
        let engine = test_ok(TranscribeCppEngine::new())?;
        assert!(!engine.capabilities().streaming);
        // Runtime-world construction validates paths in safe Rust WITHOUT
        // spawning anything. With no sidecar env configured, a valid model
        // file still fails with the typed sidecar_bin_missing error — that
        // contract is asserted here instead of mutating process env (which
        // is `unsafe` and forbidden workspace-wide).
        let probe =
            std::env::temp_dir().join(format!("narrivy-tcpp-probe-{}.gguf", std::process::id()));
        test_ok(std::fs::write(&probe, b"GGUF\x03\0\0\0"))?;
        // Runtime world only: the default world never consults the sidecar
        // env at all and must keep succeeding with a plain placeholder file.
        #[cfg(feature = "runtime")]
        if std::env::var_os("TRANSCRIBE_CPP_BIN").is_none() {
            // matches sidecar::SIDECAR_BIN_ENV
            let err = expect_asr_error(
                TranscribeCppEngine::with_model_path(&probe).map(|_| ()),
                "loading a model without a configured sidecar",
            )?;
            assert_eq!(err.code, "sidecar_bin_missing");
            assert!(loaded_capabilities_still_truthful());
        }
        #[cfg(not(feature = "runtime"))]
        {
            let loaded = test_ok(TranscribeCppEngine::with_model_path(&probe))?;
            let _ = loaded.model_path();
        }
        let _ = std::fs::remove_file(&probe);
        Ok(())
    }

    #[cfg(not(feature = "runtime"))]
    #[test]
    fn unavailable_world_reports_typed_runtime_unavailable() -> TestResult {
        let mut engine = test_ok(TranscribeCppEngine::new())?;
        let err = expect_asr_error(
            engine
                .open_stream(SessionId(7), &SessionCancellation::new())
                .map(|_| ()),
            "opening a stream without runtime support",
        )?;
        assert_eq!(err.code, RUNTIME_UNAVAILABLE_CODE);
        assert!(err.message.contains("runtime"));

        let audio = AudioChunk {
            sample_rate_hz: 16_000,
            channels: 1,
            samples: vec![0.0; 160],
        };
        let err = expect_asr_error(
            engine.transcribe(&audio),
            "transcribing without runtime support",
        )?;
        assert_eq!(err.code, RUNTIME_UNAVAILABLE_CODE);

        // The recorded path is preserved even though nothing loads it here.
        assert!(engine.model_path().is_none());
        let engine = test_ok(TranscribeCppEngine::with_model_path("/tmp/x.gguf"))?;
        assert_eq!(
            engine.model_path(),
            Some(std::path::Path::new("/tmp/x.gguf"))
        );
        Ok(())
    }

    #[cfg(feature = "runtime")]
    #[test]
    fn runtime_world_without_loaded_model_fails_before_ffi() -> TestResult {
        let mut engine = test_ok(TranscribeCppEngine::new())?;
        assert!(!engine.is_model_loaded());
        assert!(!engine.capabilities().streaming);
        let err = expect_asr_error(
            engine
                .open_stream(SessionId(1), &SessionCancellation::new())
                .map(|_| ()),
            "opening a stream without a loaded model",
        )?;
        assert_eq!(err.code, "model_not_loaded");
        let audio = AudioChunk {
            sample_rate_hz: 16_000,
            channels: 1,
            samples: vec![0.0; 160],
        };
        let err = expect_asr_error(
            engine.transcribe(&audio),
            "transcribing without a loaded model",
        )?;
        assert_eq!(err.code, "model_not_loaded");
        Ok(())
    }

    #[cfg(feature = "runtime")]
    #[test]
    fn runtime_world_rejects_missing_model_files_without_touching_ffi() -> TestResult {
        let result = TranscribeCppEngine::with_model_path("/definitely/not/here/model.gguf");
        let err = expect_asr_error(result.map(|_| ()), "loading a missing model")?;
        assert_eq!(err.code, "model_missing");
        Ok(())
    }

    // Full lifecycle test against the real sidecar. Skipped unless
    // TRANSCRIBE_CPP_TEST_MODEL points at a loadable GGUF file AND
    // TRANSCRIBE_CPP_BIN names the built transcribe-cli (see BUILD-NOTES.md).
    #[cfg(feature = "runtime")]
    #[test]
    fn full_stream_lifecycle_emits_started_then_final() -> TestResult {
        let Some(model_path) = std::env::var_os("TRANSCRIBE_CPP_TEST_MODEL") else {
            eprintln!("skipping: TRANSCRIBE_CPP_TEST_MODEL not set");
            return Ok(());
        };
        if !std::fs::metadata(&model_path).is_ok_and(|meta| meta.is_file()) {
            eprintln!("skipping: TRANSCRIBE_CPP_TEST_MODEL does not name an existing file");
            return Ok(());
        }
        let mut engine = test_ok(TranscribeCppEngine::with_model_path(model_path))?;
        assert!(engine.is_model_loaded());
        let mut stream = test_ok(engine.open_stream(SessionId(42), &SessionCancellation::new()))?;

        // Started must be observable before any audio is pushed.
        assert!(matches!(
            stream.poll_event(),
            Some(narrivy_asr_api::AsrEvent::Started { .. })
        ));

        let chunk = AudioChunk {
            sample_rate_hz: 16_000,
            channels: 1,
            samples: vec![0.125_f32; 16_000],
        };
        test_ok(stream.push_audio(&chunk))?;
        test_ok(stream.finalize())?;

        let mut saw_final = false;
        while let Some(event) = stream.poll_event() {
            if let narrivy_asr_api::AsrEvent::Final {
                transcript, seq, ..
            } = event
            {
                saw_final = true;
                assert!(transcript.is_final);
                assert!(seq >= 1);
            }
        }
        assert!(saw_final);
        test_ok(stream.finish(std::time::Duration::from_secs(1)))?;
        Ok(())
    }

    // Cancellation observed between pushes short-circuits the stream.
    #[cfg(feature = "runtime")]
    #[test]
    fn cancelled_stream_refuses_further_pushes() -> TestResult {
        // Requires a loaded model to reach the push path; skip silently when
        // no test model is configured (link-time lib requirement unchanged).
        let Some(model_path) = std::env::var_os("TRANSCRIBE_CPP_TEST_MODEL") else {
            eprintln!("skipping: TRANSCRIBE_CPP_TEST_MODEL not set");
            return Ok(());
        };
        if !std::fs::metadata(&model_path).is_ok_and(|meta| meta.is_file()) {
            eprintln!("skipping: TRANSCRIBE_CPP_TEST_MODEL does not name an existing file");
            return Ok(());
        }
        let mut engine = test_ok(TranscribeCppEngine::with_model_path(model_path))?;
        let cancellation = SessionCancellation::new();
        let mut stream = test_ok(engine.open_stream(SessionId(9), &cancellation))?;
        // Cancellation is observed between pushes at the latest (API contract).
        stream.cancel();
        let chunk = AudioChunk {
            sample_rate_hz: 16_000,
            channels: 1,
            samples: vec![0.0; 160],
        };
        let err = expect_asr_error(
            stream.push_audio(&chunk),
            "pushing audio after cancellation",
        )?;
        assert_eq!(err.code, "cancelled");
        Ok(())
    }

    // Shared prep/catalog tests compile in both worlds implicitly through
    // their own modules; this guard keeps the contract surface honest.
    #[test]
    fn catalog_is_reachable_from_crate_root() {
        assert!(!MODEL_CATALOG.is_empty());
        assert!(lookup("whisper", "base").is_ok());
    }
}

#[cfg(test)]
mod streaming_contract_tests {
    use super::*;

    type TestResult = Result<(), String>;

    fn test_ok<T, E: std::fmt::Debug>(result: Result<T, E>) -> Result<T, String> {
        result.map_err(|error| format!("unexpected test failure: {error:?}"))
    }

    fn expect_asr_error<T>(
        result: Result<T, AsrError>,
        operation: &str,
    ) -> Result<AsrError, String> {
        match result {
            Err(error) => Ok(error),
            Ok(_) => Err(format!("{operation} unexpectedly succeeded")),
        }
    }

    #[test]
    fn stream_partial_stub_returns_not_yet_supported_with_tip_and_seq() -> TestResult {
        let err = expect_asr_error(stream_partial_stub(5), "requesting a streaming partial")?;
        assert_eq!(err.code, STREAMING_NOT_YET_SUPPORTED_CODE);
        assert!(err.message.contains("binding is final-only"));
        // Sequence number should be incremented correctly.
        assert!(
            err.message.contains("6"),
            "seq should be 6, got {}",
            err.message
        );
        Ok(())
    }

    #[test]
    fn streaming_not_yet_supported_code_is_stable() {
        assert_eq!(
            STREAMING_NOT_YET_SUPPORTED_CODE,
            "streaming_not_yet_supported"
        );
        assert!(STREAMING_TIP.contains("binding is final-only"));
    }

    #[test]
    fn gguf_readiness_distinguishes_missing_invalid_and_unsupported() -> TestResult {
        let dir = test_ok(tempfile::tempdir())?;
        let missing = expect_asr_error(
            validate_gguf_model(&dir.path().join("missing.gguf")),
            "validating a missing model",
        )?;
        assert_eq!(missing.code, "model_missing");
        let invalid = dir.path().join("invalid.gguf");
        test_ok(std::fs::write(&invalid, b"not-a-model"))?;
        let invalid_error =
            expect_asr_error(validate_gguf_model(&invalid), "validating an invalid model")?;
        assert_eq!(invalid_error.code, "model_invalid");
        let unsupported = dir.path().join("unsupported.gguf");
        test_ok(std::fs::write(&unsupported, b"GGUF\x04\0\0\0"))?;
        let unsupported_error = expect_asr_error(
            validate_gguf_model(&unsupported),
            "validating an unsupported model",
        )?;
        assert_eq!(unsupported_error.code, "model_unsupported");
        Ok(())
    }

    #[cfg(feature = "runtime")]
    #[test]
    fn sidecar_stream_partial_increments_seq() -> TestResult {
        use crate::sidecar::LoadedModel;
        use crate::sidecar::SidecarStream;
        use narrivy_types::{AudioChunk, SessionId};

        // Create a dummy loaded model without needing real files by using load_with_bin with temp files.
        use std::os::unix::fs::PermissionsExt;

        let dir = test_ok(tempfile::tempdir())?;
        let model_path = dir.path().join("model.gguf");
        let bin_path = dir.path().join("bin");
        test_ok(std::fs::write(&model_path, b"GGUF\x03\0\0\0"))?;
        test_ok(std::fs::write(&bin_path, b"placeholder"))?;
        test_ok(std::fs::set_permissions(
            &bin_path,
            std::fs::Permissions::from_mode(0o700),
        ))?;
        let model = test_ok(LoadedModel::load_with_bin(&model_path, bin_path))?;
        let mut stream = SidecarStream::new(SessionId(1), model.shared());
        assert_eq!(stream.next_seq(), 0);
        let chunk = AudioChunk {
            sample_rate_hz: 16_000,
            channels: 1,
            samples: vec![0.0; 160],
        };
        let err = expect_asr_error(
            stream.stream_partial(&chunk),
            "requesting the first stream partial",
        )?;
        assert_eq!(err.code, "streaming_not_yet_supported");
        assert_eq!(stream.next_seq(), 1);
        let err2 = expect_asr_error(
            stream.stream_partial(&chunk),
            "requesting the second stream partial",
        )?;
        assert_eq!(stream.next_seq(), 2);
        assert!(err2.message.contains("2"));
        Ok(())
    }

    #[cfg(not(feature = "runtime"))]
    #[test]
    fn non_runtime_stream_partial_stub_is_callable() -> TestResult {
        let err = expect_asr_error(
            stream_partial_stub(0),
            "requesting a non-runtime stream partial",
        )?;
        assert_eq!(err.code, "streaming_not_yet_supported");
        Ok(())
    }
}
