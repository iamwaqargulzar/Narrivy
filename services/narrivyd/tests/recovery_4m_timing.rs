#![cfg(feature = "asr-transcribe-resident")]

//! Controlled, non-release timing diagnostic for the real resident ASR lane.
//!
//! This test is ignored by default and requires explicit paths for a reviewed
//! PCM WAV, GGUF model, and resident worker. It never prints transcript text,
//! writes qualification evidence, or exercises a microphone.

use std::error::Error;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use narrivy_asr_transcribe::ResidentWorkerEngine;
use narrivy_core::{
    CoreError, CorrectionPort, JournalEntryState, RecoveryDeliveryMode, RecoveryJournal,
    RecoveryTerminal, RecoveryTimingStage, RecoveryTimingTrace, StreamingRecoverySlice, sha256_hex,
};
use narrivy_journal_unix::UnixJournal;
use narrivy_platform_api::{
    CaptureEnd, CaptureStreamChunk, CaptureStreamEvent, CaptureStreamInfo, CaptureStreamSummary,
    PlatformError, StreamingAudioCapturePort, StreamingAudioCaptureSession,
};
use narrivy_platform_linux::{delivery::InboxStore, recovery::InboxOnlyDelivery};
use narrivy_policy::{PolicyDocument, PolicyEngine};
use narrivy_types::{
    AudioChunk, SessionCancellation, SessionControl, SessionId, SessionState, TransactionId,
};
use narrivyd::correct;

const FIXTURE_ENV: &str = "NARRIVY_CONTROLLED_SPEECH_FIXTURE";
const FIXTURE_SHA_ENV: &str = "NARRIVY_CONTROLLED_SPEECH_FIXTURE_SHA256";
const MODEL_ENV: &str = "NARRIVY_TRANSCRIBE_MODEL";
const WORKER_ENV: &str = "NARRIVY_TRANSCRIBE_WORKER";
const SAMPLE_RATE_HZ: u32 = 16_000;
const CHUNK_SAMPLES: usize = 1_280;
const MAX_FIXTURE_BYTES: usize = 64 * 1024 * 1024;
const MAX_FIXTURE_SAMPLES: usize = SAMPLE_RATE_HZ as usize * 30;

type TestResult = Result<(), Box<dyn Error>>;

struct DeterministicCorrector;

impl CorrectionPort for DeterministicCorrector {
    fn correct(
        &mut self,
        raw: &str,
        cancellation: &SessionCancellation,
    ) -> Result<String, CoreError> {
        if cancellation.is_cancelled() {
            return Err(CoreError::Cancelled);
        }
        Ok(correct::correct(raw))
    }
}

struct PacedFixtureCapture {
    samples: Vec<f32>,
}

impl StreamingAudioCapturePort for PacedFixtureCapture {
    fn start_stream(
        &mut self,
        control: &SessionControl,
    ) -> Result<Box<dyn StreamingAudioCaptureSession>, PlatformError> {
        if control.is_cancelled() {
            return Err(platform_error("cancelled"));
        }
        Ok(Box::new(PacedFixtureSession {
            info: CaptureStreamInfo {
                source_selection: "explicit-controlled-wav".to_owned(),
                sample_rate_hz: SAMPLE_RATE_HZ,
                channels: 1,
            },
            samples: std::mem::take(&mut self.samples),
            next_sample: 0,
            next_sequence: 0,
            origin: Instant::now(),
            ended: false,
        }))
    }
}

struct PacedFixtureSession {
    info: CaptureStreamInfo,
    samples: Vec<f32>,
    next_sample: usize,
    next_sequence: u64,
    origin: Instant,
    ended: bool,
}

impl StreamingAudioCaptureSession for PacedFixtureSession {
    fn info(&self) -> &CaptureStreamInfo {
        &self.info
    }

    fn wait_ready(&mut self, control: &SessionControl) -> Result<(), PlatformError> {
        if control.is_cancelled() {
            return Err(platform_error("cancelled"));
        }
        if self.samples.is_empty() {
            return Err(platform_error("capture_empty"));
        }
        wait_until(
            control,
            self.origin
                + Duration::from_micros(
                    (CHUNK_SAMPLES as u64).saturating_mul(1_000_000) / u64::from(SAMPLE_RATE_HZ),
                ),
        )
    }

    fn next_event(
        &mut self,
        control: &SessionControl,
    ) -> Result<CaptureStreamEvent, PlatformError> {
        if control.is_cancelled() {
            return Err(platform_error("cancelled"));
        }
        if self.next_sample < self.samples.len() {
            let scheduled = self.origin
                + Duration::from_micros(
                    self.next_sequence
                        .saturating_add(1)
                        .saturating_mul(CHUNK_SAMPLES as u64)
                        .saturating_mul(1_000_000)
                        / u64::from(SAMPLE_RATE_HZ),
                );
            wait_until(control, scheduled)?;
            let end = self
                .next_sample
                .saturating_add(CHUNK_SAMPLES)
                .min(self.samples.len());
            let chunk = CaptureStreamChunk {
                sequence: self.next_sequence,
                audio: AudioChunk {
                    sample_rate_hz: SAMPLE_RATE_HZ,
                    channels: 1,
                    samples: self.samples[self.next_sample..end].to_vec(),
                },
                first_frame_after_start_us: (self.next_sequence == 0).then_some(0),
            };
            self.next_sample = end;
            self.next_sequence = self.next_sequence.saturating_add(1);
            return Ok(CaptureStreamEvent::Audio(chunk));
        }
        if self.ended {
            return Err(platform_error("capture_event_after_end"));
        }
        self.ended = true;
        Ok(CaptureStreamEvent::End(CaptureStreamSummary {
            end: CaptureEnd::FinalizationRequested,
            total_samples: self.samples.len() as u64,
            first_frame_after_start_us: Some(0),
            capture_duration_us: u64::try_from(self.origin.elapsed().as_micros())
                .unwrap_or(u64::MAX),
        }))
    }

    fn abort(&mut self, _deadline: Instant) -> Result<(), PlatformError> {
        self.ended = true;
        Ok(())
    }
}

fn platform_error(code: &'static str) -> PlatformError {
    PlatformError {
        code,
        message: "controlled fixture capture failed".to_owned(),
    }
}

fn wait_until(control: &SessionControl, deadline: Instant) -> Result<(), PlatformError> {
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        if control.is_cancelled() {
            return Err(platform_error("cancelled"));
        }
        thread::sleep(remaining.min(Duration::from_millis(10)));
    }
    Ok(())
}

fn required_path(name: &str) -> Result<std::path::PathBuf, Box<dyn Error>> {
    let value = std::env::var_os(name).ok_or_else(|| format!("{name} must be set explicitly"))?;
    let path = std::path::PathBuf::from(value);
    let metadata = fs::symlink_metadata(&path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!("{name} must name a regular non-symlink file").into());
    }
    Ok(path)
}

fn read_pcm16_mono_16khz(path: &Path) -> Result<Vec<f32>, Box<dyn Error>> {
    let bytes = fs::read(path)?;
    if bytes.len() < 12 || bytes.len() > MAX_FIXTURE_BYTES {
        return Err("controlled WAV size is invalid".into());
    }
    if &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("controlled fixture is not a RIFF/WAVE file".into());
    }

    let mut offset = 12usize;
    let mut format = None;
    let mut data = None;
    while offset.saturating_add(8) <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into()?) as usize;
        let start = offset + 8;
        let end = start
            .checked_add(size)
            .filter(|end| *end <= bytes.len())
            .ok_or("controlled WAV chunk is truncated")?;
        match id {
            b"fmt " if size >= 16 => {
                if format.is_some() {
                    return Err("controlled WAV has duplicate format chunks".into());
                }
                format = Some((
                    u16::from_le_bytes(bytes[start..start + 2].try_into()?),
                    u16::from_le_bytes(bytes[start + 2..start + 4].try_into()?),
                    u32::from_le_bytes(bytes[start + 4..start + 8].try_into()?),
                    u16::from_le_bytes(bytes[start + 14..start + 16].try_into()?),
                ));
            }
            b"data" => {
                if data.is_some() {
                    return Err("controlled WAV has duplicate data chunks".into());
                }
                data = Some(&bytes[start..end]);
            }
            _ => {}
        }
        offset = end.saturating_add(size & 1);
    }

    if format != Some((1, 1, SAMPLE_RATE_HZ, 16)) {
        return Err("controlled WAV must be PCM16 mono 16 kHz".into());
    }
    let data = data.ok_or("controlled WAV has no data chunk")?;
    if data.is_empty() || data.len() % 2 != 0 {
        return Err("controlled WAV sample data is invalid".into());
    }
    let samples = data
        .chunks_exact(2)
        .map(|sample| i16::from_le_bytes([sample[0], sample[1]]) as f32 / 32_768.0)
        .collect::<Vec<_>>();
    if samples.len() > MAX_FIXTURE_SAMPLES {
        return Err("controlled WAV exceeds the 30-second diagnostic bound".into());
    }
    Ok(samples)
}

#[test]
#[ignore = "controlled non-release diagnostic; requires explicit fixture/model/worker paths"]
fn real_resident_stream_records_content_free_monotonic_boundaries() -> TestResult {
    let fixture = required_path(FIXTURE_ENV)?;
    let model = required_path(MODEL_ENV)?;
    let _worker = required_path(WORKER_ENV)?;
    let expected_fixture_hash = std::env::var(FIXTURE_SHA_ENV)
        .map_err(|_| format!("{FIXTURE_SHA_ENV} must be set explicitly"))?;
    if expected_fixture_hash.len() != 64
        || !expected_fixture_hash
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(format!("{FIXTURE_SHA_ENV} must be a SHA-256 hex digest").into());
    }
    let fixture_bytes = fs::read(&fixture)?;
    if sha256_hex(&fixture_bytes) != expected_fixture_hash.to_ascii_lowercase() {
        return Err("controlled fixture hash mismatch".into());
    }
    let samples = read_pcm16_mono_16khz(&fixture)?;

    let temporary = tempfile::Builder::new()
        .prefix("narrivy-4m-timing-")
        .tempdir()?;
    fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o700))?;
    let root = temporary.path().to_path_buf();
    let journal_dir = root.join("journal");
    let inbox_dir = root.join("inbox");

    let journal = UnixJournal::open(&journal_dir)?;
    let mut capture = PacedFixtureCapture { samples };
    let mut engine = ResidentWorkerEngine::with_model_path(model)
        .map_err(|error| format!("resident initialization failed ({})", error.code))?;
    let policy = PolicyEngine::from_document(PolicyDocument::default());
    let mut corrector = DeterministicCorrector;
    let mut delivery = InboxOnlyDelivery::new(inbox_dir.clone());
    let timing = RecoveryTimingTrace::new();
    let control = SessionControl::new();
    let txid = TransactionId::new("controlled-4m-timing");
    let mut partial_count = 0usize;
    let mut partial_observer = |_| {
        partial_count = partial_count.saturating_add(1);
        Ok(())
    };
    let terminal = StreamingRecoverySlice {
        journal: &journal,
        capture: &mut capture,
        asr: &mut engine,
        policy: &policy,
        corrector: &mut corrector,
        delivery: &mut delivery,
        delivery_mode: RecoveryDeliveryMode::InboxPrimary,
        endpointing: None,
        partial_observer: Some(&mut partial_observer),
    }
    .run_with_timing(
        SessionId(1),
        txid.clone(),
        "controlled-resident-timing".to_owned(),
        &control,
        &mut |_state: SessionState| Ok(()),
        &timing,
    )?;

    assert!(matches!(terminal, RecoveryTerminal::DeliveredInbox(_)));
    assert!(
        partial_count > 0,
        "resident stream produced no partial event"
    );
    assert_eq!(timing.observed_count(), RecoveryTimingStage::ALL.len());
    let offsets = RecoveryTimingStage::ALL
        .into_iter()
        .map(|stage| {
            timing
                .offset_us(stage)
                .ok_or("successful diagnostic omitted a timing stage")
        })
        .collect::<Result<Vec<_>, _>>()?;
    assert!(offsets.windows(2).all(|pair| pair[0] <= pair[1]));
    eprintln!(
        "controlled_timing_observed_count={}",
        timing.observed_count()
    );
    for (stage, offset_us) in RecoveryTimingStage::ALL.into_iter().zip(offsets) {
        eprintln!("controlled_timing_{stage:?}_us={offset_us}");
    }

    let entry = journal.read(txid.as_str())?.ok_or("journal row missing")?;
    assert_eq!(journal.entries()?.len(), 1);
    assert_eq!(entry.state, JournalEntryState::Delivered);
    let journal_text = entry
        .variants
        .delivered
        .as_deref()
        .ok_or("journal delivered variant missing")?;
    let inbox = InboxStore::new(inbox_dir);
    assert_eq!(inbox.list().len(), 1);
    assert_eq!(
        fs::metadata(journal_dir.join(format!("{}.json", txid.as_str())))?
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(inbox.path_for(&txid)?)?.permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(&journal_dir)?.permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(&inbox.dir)?.permissions().mode() & 0o777,
        0o700
    );
    let inbox_text = inbox.read_text(&txid)?;
    assert_eq!(
        sha256_hex(journal_text.as_bytes()),
        sha256_hex(inbox_text.as_bytes())
    );

    drop(inbox_text);
    drop(entry);
    drop(delivery);
    drop(engine);
    drop(journal);
    temporary.close()?;
    assert!(!root.exists());
    Ok(())
}
