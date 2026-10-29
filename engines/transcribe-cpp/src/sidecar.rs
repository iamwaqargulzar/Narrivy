//! Sidecar wiring for the `runtime` feature: drives a pinned, operator-
//! provided transcribe.cpp CLI build as a versioned subprocess.
//!
//! # Why a sidecar (decision recorded in ../BUILD-NOTES.md)
//!
//! transcribe.cpp v0.2.1 exposes two integration surfaces: a single-header C
//! API (`include/transcribe.h`, 2600+ lines) and a documented CLI
//! (`build/bin/transcribe-cli -m model.gguf audio.wav`). The P2 walking
//! skeleton binds the CLI sidecar: it is the upstream surface with a stable,
//! observable contract (argv in, transcript file out, exit status), needs no
//! `unsafe`, and keeps this crate inside the workspace `unsafe_code = forbid`
//! rule. (The old local `transcribe-sys` FFI stub was deleted, F-L1.)
//!
//! # Operator contract (env is read once, at model-load time)
//!
//! - `TRANSCRIBE_CPP_BIN` — path to the built `transcribe-cli` binary
//!   (REQUIRED; nothing is downloaded, built, or vendored here).
//! - Model path comes from the constructor (`with_model_path`).
//! - Decode argv: `<bin> -m <model> -q --backend cpu -o <out.txt> <in.wav>`.
//!   The CPU backend is the P2 correctness lane by design.
//!
//! Streaming honesty note: unchanged from the P2 prep design — one `Started`
//! event up front, one blocking decode at `finalize`, then a single `Final`.
//! No fabricated partials; `capabilities().streaming` stays `false`.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use narrivy_asr_api::{AsrError, AsrEvent, AsrStream, CancelToken};
use narrivy_types::{AudioChunk, SessionId, Transcript};
use tempfile::{Builder, NamedTempFile, TempDir};

/// Environment variable naming the built transcribe.cpp CLI binary.
pub const SIDECAR_BIN_ENV: &str = "TRANSCRIBE_CPP_BIN";

/// Hard bound on one sidecar decode. The pinned P2 reference model
/// (whisper-tiny.en Q8_0) decodes ~11 s of audio in well under a second on
/// the qualified CPU lane; 120 s leaves orders of magnitude of headroom
/// while guaranteeing a session worker can never hang forever on a wedged
/// child process.
pub const DECODE_TIMEOUT: Duration = Duration::from_secs(120);

/// Maximum time from observing cancellation to reaping a sidecar process.
pub const CANCEL_TERMINATION_BOUND: Duration = Duration::from_secs(2);

// ---------------------------------------------------------------------------
// LoadedModel
// ---------------------------------------------------------------------------

/// A pinned decode target: validated GGUF model plus validated sidecar
/// binary. Cheaply clonable; both paths are checked at load time and no
/// subprocess is spawned during construction.
#[derive(Debug, Clone)]
pub(crate) struct LoadedModel {
    model_path: PathBuf,
    bin_path: PathBuf,
}

impl LoadedModel {
    /// Resolves [`SIDECAR_BIN_ENV`] once, then validates both paths.
    pub(crate) fn load(model_path: &Path) -> Result<Self, AsrError> {
        crate::validate_gguf_model(model_path)?;
        let raw_bin = std::env::var_os(SIDECAR_BIN_ENV).ok_or_else(|| {
            AsrError::new(
                "sidecar_bin_missing",
                format!(
                    "env {SIDECAR_BIN_ENV} must name the built transcribe-cli \
                     binary (pinned build; see ../BUILD-NOTES.md)"
                ),
            )
        })?;
        Self::load_with_bin(model_path, PathBuf::from(raw_bin))
    }

    /// Validates an explicit (bin, model) pair without touching the process
    /// environment. Tests use this directly; `load` is a thin env wrapper.
    pub(crate) fn load_with_bin(model_path: &Path, bin_path: PathBuf) -> Result<Self, AsrError> {
        crate::validate_gguf_model(model_path)?;
        if !bin_path.is_file() {
            return Err(AsrError::new(
                "sidecar_bin_missing",
                "configured transcribe.cpp executable is unavailable",
            ));
        }
        if std::fs::metadata(&bin_path)
            .map(|meta| meta.permissions().mode() & 0o111 == 0)
            .unwrap_or(true)
        {
            return Err(AsrError::new(
                "sidecar_bin_nonexecutable",
                "configured transcribe.cpp executable is not executable",
            ));
        }
        Ok(Self {
            model_path: model_path.to_path_buf(),
            bin_path,
        })
    }

    /// Shared handle for streams opened from one engine instance.
    pub(crate) fn shared(&self) -> Arc<Self> {
        Arc::new(self.clone())
    }
}

// ---------------------------------------------------------------------------
// SidecarStream
// ---------------------------------------------------------------------------

/// Stream state machine: buffer resampled audio between pushes, run one
/// bounded sidecar decode at finalize, drain buffered events via poll_event.
pub(crate) struct SidecarStream {
    session: SessionId,
    model: Arc<LoadedModel>,
    pending: std::collections::VecDeque<AsrEvent>,
    samples_i16: Vec<i16>,
    next_seq: u64,
    finalized: bool,
    cancelled: CancelToken,
}

impl SidecarStream {
    #[cfg(test)]
    pub(crate) fn new(session: SessionId, model: Arc<LoadedModel>) -> Self {
        Self::with_cancellation(session, model, CancelToken::new())
    }

    pub(crate) fn with_cancellation(
        session: SessionId,
        model: Arc<LoadedModel>,
        cancelled: CancelToken,
    ) -> Self {
        let mut pending = std::collections::VecDeque::new();
        // Contract: Started is observable before any push.
        pending.push_back(AsrEvent::Started { session });
        Self {
            session,
            model,
            pending,
            samples_i16: Vec::new(),
            next_seq: 0,
            finalized: false,
            cancelled,
        }
    }

    fn error_event(code: &'static str, message: impl Into<String>) -> AsrEvent {
        AsrEvent::Error(AsrError::new(code, message))
    }

    /// Test-only probe for the final-only sidecar contract. It never simulates
    /// partials; it returns the stable unsupported error while exercising
    /// monotone sequence bookkeeping.
    #[cfg(test)]
    pub fn stream_partial(&mut self, _chunk: &AudioChunk) -> Result<Option<AsrEvent>, AsrError> {
        self.next_seq += 1;
        Err(AsrError::new(
            "streaming_not_yet_supported",
            format!(
                "the CLI sidecar is final-only (seq {}); select a model-attested resident streaming lane for native Partial events (next seq {})",
                self.next_seq, self.next_seq
            ),
        ))
    }

    /// Non-mutating helper for tests: current sequence number.
    #[cfg(test)]
    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    /// Runs the blocking sidecar decode: buffered i16 PCM -> canonical 16 kHz
    /// mono WAV -> pinned `transcribe-cli` -> trimmed UTF-8 text.
    ///
    /// Cancellation is checked before and during decode; cancellation kills
    /// and reaps the sidecar inside [`CANCEL_TERMINATION_BOUND`].
    fn decode_blocking(&mut self) -> Result<String, AsrError> {
        if self.cancelled.is_cancelled() {
            return Err(AsrError::new(
                "cancelled",
                "stream was cancelled before decode",
            ));
        }
        let workdir = secure_tempdir("narrivy-tcpp-")
            .map_err(|_| AsrError::new("decode_failed", "sidecar workdir unavailable"))?;
        let wav = secure_tempfile(workdir.path(), ".audio-")
            .map_err(|_| AsrError::new("decode_failed", "temporary audio unavailable"))?;
        let transcript = secure_tempfile(workdir.path(), ".transcript-")
            .map_err(|_| AsrError::new("decode_failed", "temporary transcript unavailable"))?;
        let write_result = write_wav_mono_16k(wav.path(), &self.samples_i16);
        if write_result.is_err() {
            return Err(AsrError::new(
                "decode_failed",
                "temporary audio write failed",
            ));
        }

        let run_result = run_bounded(
            &self.model.bin_path,
            &self.model.model_path,
            transcript.path(),
            wav.path(),
            &self.cancelled,
        );
        match run_result {
            Ok(()) => {}
            Err(err) => return Err(err),
        }

        let read_result = std::fs::read_to_string(transcript.path());
        let text = read_result
            .map_err(|_| AsrError::new("decode_failed", "sidecar did not produce a transcript"))?;
        Ok(text.trim().to_owned())
    }
}

fn secure_tempdir(prefix: &str) -> std::io::Result<TempDir> {
    let parent = std::env::temp_dir();
    secure_tempdir_in(&parent, prefix)
}

fn secure_tempdir_in(parent: &Path, prefix: &str) -> std::io::Result<TempDir> {
    let parent_meta = std::fs::symlink_metadata(parent)?;
    if parent_meta.file_type().is_symlink() || !parent_meta.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "temporary parent is unsafe",
        ));
    }
    let dir = Builder::new().prefix(prefix).tempdir_in(parent)?;
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
    Ok(dir)
}

fn secure_tempfile(dir: &Path, prefix: &str) -> std::io::Result<NamedTempFile> {
    let meta = std::fs::symlink_metadata(dir)?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "temporary directory is unsafe",
        ));
    }
    let file = Builder::new().prefix(prefix).tempfile_in(dir)?;
    file.as_file()
        .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

/// Spawns the pinned sidecar with fixed argv and bounds it by
/// [`DECODE_TIMEOUT`]. Output is discarded: only the designated output file
/// is a contract channel, which prevents protected input from entering logs.
fn run_bounded(
    bin: &Path,
    model: &Path,
    out_path: &Path,
    wav_path: &Path,
    cancellation: &CancelToken,
) -> Result<(), AsrError> {
    let mut child = Command::new(bin)
        .arg("-m")
        .arg(model)
        .args(["-q", "--backend", "cpu"])
        .arg("-o")
        .arg(out_path)
        .arg(wav_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|_| AsrError::new("sidecar_spawn_failed", "unable to start transcribe.cpp"))?;

    let deadline = Instant::now() + DECODE_TIMEOUT;
    let status = loop {
        if cancellation.is_cancelled() {
            terminate(&mut child)?;
            return Err(AsrError::new("cancelled", "ASR cancellation requested"));
        }
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    break None;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => {
                terminate(&mut child)?;
                return Err(AsrError::new("decode_failed", "sidecar wait failed"));
            }
        }
    };

    let Some(status) = status else {
        terminate(&mut child)?;
        return Err(AsrError::new(
            "decode_timeout",
            "transcribe.cpp exceeded its decode bound",
        ));
    };
    if !status.success() {
        return Err(AsrError::new(
            "decode_failed",
            "transcribe.cpp exited unsuccessfully",
        ));
    }
    Ok(())
}

fn terminate(child: &mut std::process::Child) -> Result<(), AsrError> {
    match child.try_wait() {
        Ok(Some(_)) => return Ok(()),
        Ok(None) => {}
        Err(_) => {
            return Err(AsrError::new(
                "sidecar_termination_failed",
                "unable to inspect transcribe.cpp during termination",
            ));
        }
    }

    // A racing natural exit can make kill report an error. Inspect once more
    // before treating that as a cleanup failure.
    if child.kill().is_err() {
        return match child.try_wait() {
            Ok(Some(_)) => Ok(()),
            _ => Err(AsrError::new(
                "sidecar_termination_failed",
                "unable to terminate transcribe.cpp",
            )),
        };
    }

    let deadline = Instant::now() + CANCEL_TERMINATION_BOUND;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return Ok(()),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                return Err(AsrError::new(
                    "sidecar_termination_timeout",
                    "transcribe.cpp did not terminate within its cleanup bound",
                ));
            }
            Err(_) => {
                return Err(AsrError::new(
                    "sidecar_termination_failed",
                    "unable to reap transcribe.cpp",
                ));
            }
        }
    }
}

impl AsrStream for SidecarStream {
    fn session(&self) -> SessionId {
        self.session
    }

    fn cancellation(&self) -> CancelToken {
        self.cancelled.clone()
    }

    fn push_audio(&mut self, chunk: &AudioChunk) -> Result<(), AsrError> {
        // Cancellation is observed between pushes at the latest (REL-007).
        if self.cancelled.is_cancelled() {
            return Err(AsrError::new("cancelled", "stream was cancelled"));
        }
        if self.finalized {
            return Err(AsrError::new(
                "stream_finalized",
                "cannot push audio after finalize",
            ));
        }
        let mono = crate::ensure_mono_16k(chunk)?;
        self.samples_i16.extend_from_slice(&mono);
        Ok(())
    }

    fn poll_event(&mut self) -> Option<AsrEvent> {
        self.pending.pop_front()
    }

    fn finalize(&mut self) -> Result<(), AsrError> {
        if self.cancelled.is_cancelled() {
            self.pending
                .push_back(Self::error_event("cancelled", "stream was cancelled"));
            return Ok(());
        }
        if self.finalized {
            return Ok(()); // idempotent, mirroring the reference mock
        }
        self.finalized = true;

        if self.samples_i16.is_empty() {
            self.pending.push_back(Self::error_event(
                "no_audio",
                "no audio pushed before finalize",
            ));
            return Ok(());
        }

        match self.decode_blocking() {
            Ok(text) => {
                self.next_seq += 1;
                self.pending.push_back(AsrEvent::Final {
                    seq: self.next_seq,
                    transcript: Transcript {
                        text,
                        is_final: true,
                    },
                    confidence: None, // no verified confidence surface yet
                    first_audio_offset_ms: None,
                });
                Ok(())
            }
            Err(err) => {
                self.pending.push_back(AsrEvent::Error(err));
                Ok(())
            }
        }
    }

    fn finish(&mut self, _timeout: std::time::Duration) -> Result<(), AsrError> {
        // The decode ran inline during finalize, so there is no worker to join;
        // bounded finish is satisfied immediately. When the decode moves onto a
        // worker thread (P5 integration), `_timeout` becomes the join budget.
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// WAV writer
// ---------------------------------------------------------------------------

/// Writes a canonical 44-byte-header RIFF/WAVE file: PCM16 LE, mono, 16 kHz —
/// exactly the input contract transcribe.cpp documents ("Input must be
/// 16 kHz mono WAV").
fn write_wav_mono_16k(path: &Path, samples_i16: &[i16]) -> Result<(), String> {
    let data_len = samples_i16
        .len()
        .checked_mul(2)
        .ok_or_else(|| "wav too large".to_owned())?;
    let data_len_u32 = u32::try_from(data_len).map_err(|_| "wav too large for RIFF".to_owned())?;
    let rate = crate::TARGET_SAMPLE_RATE_HZ;
    let byte_rate = rate
        .checked_mul(2)
        .ok_or_else(|| "rate overflow".to_owned())?;

    let mut header = [0u8; 44];
    header[0..4].copy_from_slice(b"RIFF");
    let riff_len = 36u32
        .checked_add(data_len_u32)
        .ok_or_else(|| "wav too large for RIFF".to_owned())?;
    header[4..8].copy_from_slice(&riff_len.to_le_bytes());
    header[8..12].copy_from_slice(b"WAVE");
    header[12..16].copy_from_slice(b"fmt ");
    header[16..20].copy_from_slice(&16_u32.to_le_bytes()); // fmt chunk size
    header[20..22].copy_from_slice(&1_u16.to_le_bytes()); // PCM
    header[22..24].copy_from_slice(&1_u16.to_le_bytes()); // mono
    header[24..28].copy_from_slice(&rate.to_le_bytes());
    header[28..32].copy_from_slice(&byte_rate.to_le_bytes());
    header[32..34].copy_from_slice(&2_u16.to_le_bytes()); // block align
    header[34..36].copy_from_slice(&16_u16.to_le_bytes()); // bits per sample
    header[36..40].copy_from_slice(b"data");
    header[40..44].copy_from_slice(&data_len_u32.to_le_bytes());

    let mut bytes = Vec::with_capacity(44 + data_len);
    bytes.extend_from_slice(&header);
    for sample in samples_i16 {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn load_rejects_missing_model_before_env_lookup() {
        let err = LoadedModel::load(Path::new("/definitely/not/here/model.gguf"))
            .err()
            .unwrap_or_else(|| AsrError::new("test_bug", "expected model_missing"));
        assert_eq!(err.code, "model_missing");
    }

    #[test]
    fn sidecar_temp_storage_is_private_symlink_safe_and_raii_cleaned() {
        let outer = tempfile::tempdir().unwrap();
        let parent = outer.path().join("private-parent");
        std::fs::create_dir(&parent).unwrap();
        let workdir = secure_tempdir_in(&parent, "narrivy-test-").unwrap();
        let work_path = workdir.path().to_path_buf();
        assert_eq!(
            std::fs::metadata(&work_path).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let audio = secure_tempfile(&work_path, ".audio-").unwrap();
        let transcript = secure_tempfile(&work_path, ".transcript-").unwrap();
        assert_eq!(
            std::fs::metadata(audio.path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(transcript.path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        drop(audio);
        drop(transcript);
        drop(workdir);
        assert!(!work_path.exists(), "RAII did not remove sidecar workdir");

        let symlink_parent = outer.path().join("symlink-parent");
        symlink(&parent, &symlink_parent).unwrap();
        assert!(secure_tempdir_in(&symlink_parent, "narrivy-test-").is_err());

        let workdir = secure_tempdir_in(&parent, "narrivy-test-").unwrap();
        let symlink_dir = outer.path().join("symlink-workdir");
        symlink(workdir.path(), &symlink_dir).unwrap();
        assert!(secure_tempfile(&symlink_dir, ".audio-").is_err());
    }

    #[test]
    fn cancellation_terminates_and_reaps_controlled_sidecar_child() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("child.pid");
        let bin = dir.path().join("transcribe-cli");
        std::fs::write(
            &bin,
            format!(
                "#!/bin/sh\necho $$ > '{}'\nexec sleep 10\n",
                marker.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).unwrap();
        let model_path = dir.path().join("model.gguf");
        std::fs::write(&model_path, b"GGUF\x03\0\0\0").unwrap();
        let model = LoadedModel::load_with_bin(&model_path, bin)
            .unwrap()
            .shared();
        let cancellation = CancelToken::new();
        let child_cancel = cancellation.clone();
        let mut stream = SidecarStream::with_cancellation(SessionId(77), model, child_cancel);
        assert!(matches!(
            stream.poll_event(),
            Some(AsrEvent::Started { .. })
        ));
        stream
            .push_audio(&AudioChunk {
                sample_rate_hz: crate::TARGET_SAMPLE_RATE_HZ,
                channels: 1,
                samples: vec![0.0; 160],
            })
            .unwrap();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            let _ = stream.finalize();
            let code = match stream.poll_event() {
                Some(AsrEvent::Error(err)) => err.code,
                _ => "unexpected_event",
            };
            let _ = done_tx.send(code);
        });
        let deadline = Instant::now() + Duration::from_secs(1);
        while !marker.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(marker.exists(), "controlled sidecar did not start");
        let pid = std::fs::read_to_string(&marker).unwrap();
        cancellation.cancel();
        assert_eq!(
            done_rx
                .recv_timeout(CANCEL_TERMINATION_BOUND + Duration::from_secs(1))
                .unwrap(),
            "cancelled"
        );
        handle.join().unwrap();
        assert!(
            !std::process::Command::new("kill")
                .args(["-0", pid.trim()])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap()
                .success(),
            "cancelled sidecar child still exists"
        );
    }

    #[test]
    fn wav_writer_produces_canonical_header() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.wav");
        write_wav_mono_16k(&path, &[0, -1, 1, i16::MAX, i16::MIN])
            .unwrap_or_else(|p| panic!("write failed: {p}"));
        let bytes = std::fs::read(&path).unwrap_or_default();
        assert_eq!(bytes.len(), 44 + 10);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(
            u32::from_le_bytes(bytes[24..28].try_into().unwrap_or([0; 4])),
            crate::TARGET_SAMPLE_RATE_HZ
        );
        assert_eq!(
            u16::from_le_bytes(bytes[22..24].try_into().unwrap_or([0; 2])),
            1
        );
        assert_eq!(
            u32::from_le_bytes(bytes[40..44].try_into().unwrap_or([0; 4])),
            10
        );
    }
}
