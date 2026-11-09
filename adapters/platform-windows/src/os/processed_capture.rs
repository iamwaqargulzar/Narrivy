//! "Ignore this computer's sound" capture: the chosen (or default)
//! microphone and a loopback of the default speakers, both opened on the
//! capture producer thread, through
//! [`crate::voice_processing::ProcessedSource`]. The ordinary capture
//! sessions carry the result, so it meets the same contract as
//! [`super::WasapiCapture`]: mono f32 at 16 kHz.
//!
//! No `unsafe` here: the WASAPI calls are in `wasapi`.

use narrivy_platform_api::{
    AudioCapturePort, AudioCaptureSession, PlatformError, StreamingAudioCapturePort,
    StreamingAudioCaptureSession,
};
use narrivy_types::SessionControl;

use super::wasapi::{DeviceSelection, Endpoint, open_source};
use crate::capture::{
    BatchCaptureSession, CaptureConfig, CaptureSource, SourceOpener, StreamingCaptureSession,
};
use crate::voice_processing::{ProcessedSource, Processing};

/// [`AudioCapturePort`] and [`StreamingAudioCapturePort`] for the processed
/// microphone.
#[derive(Debug, Clone)]
pub struct ProcessedCapture {
    seconds: f32,
    microphone: DeviceSelection,
    processing: Processing,
}

impl ProcessedCapture {
    /// `seconds` is the session's bound, as for [`super::WasapiCapture`].
    ///
    /// # Errors
    /// `capture_duration_invalid` outside `[1, MAX_CAPTURE_SECONDS]`.
    pub fn new(
        seconds: f32,
        microphone: DeviceSelection,
        processing: Processing,
    ) -> Result<Self, PlatformError> {
        CaptureConfig::new("processed", seconds)?;
        Ok(Self {
            seconds,
            microphone,
            processing,
        })
    }

    fn config(&self) -> Result<CaptureConfig, PlatformError> {
        let label = match &self.microphone {
            DeviceSelection::Default => "processed:default".to_owned(),
            DeviceSelection::Id(id) => format!("processed:{id}"),
        };
        CaptureConfig::new(label, self.seconds)
    }

    fn opener(&self) -> SourceOpener {
        let microphone = Endpoint::Capture(self.microphone.clone());
        let processing = self.processing;
        Box::new(move || {
            let microphone = open_source(&microphone)?;
            // No speakers (none plugged in, or the device refuses a
            // loopback): noise and gain still apply.
            let speakers = open_source(&Endpoint::Loopback(DeviceSelection::Default)).ok();
            let source = ProcessedSource::new(microphone, speakers, processing).with_reopen(
                Box::new(|| open_source(&Endpoint::Loopback(DeviceSelection::Default))),
            );
            Ok(Box::new(source) as Box<dyn CaptureSource>)
        })
    }
}

impl AudioCapturePort for ProcessedCapture {
    fn start(&mut self) -> Result<Box<dyn AudioCaptureSession>, PlatformError> {
        let session = BatchCaptureSession::start(&self.config()?, self.opener())?;
        Ok(Box::new(session))
    }
}

impl StreamingAudioCapturePort for ProcessedCapture {
    fn start_stream(
        &mut self,
        control: &SessionControl,
    ) -> Result<Box<dyn StreamingAudioCaptureSession>, PlatformError> {
        let session = StreamingCaptureSession::start(&self.config()?, self.opener(), control)?;
        Ok(Box::new(session))
    }
}

/// Dictation's capture: the plain microphone, or the processed one when
/// "Ignore this computer's sound" is on.
#[derive(Debug, Clone)]
pub enum MicrophoneCapture {
    Plain(super::WasapiCapture),
    Processed(ProcessedCapture),
}

impl AudioCapturePort for MicrophoneCapture {
    fn start(&mut self) -> Result<Box<dyn AudioCaptureSession>, PlatformError> {
        match self {
            Self::Plain(capture) => capture.start(),
            Self::Processed(capture) => capture.start(),
        }
    }
}

impl StreamingAudioCapturePort for MicrophoneCapture {
    fn start_stream(
        &mut self,
        control: &SessionControl,
    ) -> Result<Box<dyn StreamingAudioCaptureSession>, PlatformError> {
        match self {
            Self::Plain(capture) => capture.start_stream(control),
            Self::Processed(capture) => capture.start_stream(control),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn energy_db(samples: &[f32]) -> f32 {
        let mean = samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32;
        10.0 * mean.max(1e-12).log10()
    }

    /// Run on the Windows test machine with `--ignored --nocapture` while
    /// something plays through the default speakers: prints how much
    /// speaker audio the loopback delivered and the microphone level before
    /// and after processing (levels only, no audio is kept).
    #[test]
    #[ignore = "needs Windows audio devices and something playing"]
    fn echo_reference_probe() {
        let seconds: u64 = std::env::var("NARRIVY_PROBE_SECONDS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(12);
        let processing = Processing {
            noise: crate::voice_processing::NoiseLevel::Off,
            auto_gain: false,
        };
        let speakers = open_source(&Endpoint::Loopback(DeviceSelection::Default));
        println!("speakers opened: {:?}", speakers.as_ref().err());
        // How the loopback reads on its own for a second.
        if let Ok(mut alone) = open_source(&Endpoint::Loopback(DeviceSelection::Default)) {
            let mut got = Vec::new();
            let mut results = Vec::new();
            for _ in 0..50 {
                let read = alone.read(&mut got, Duration::from_millis(20));
                results.push(format!("{read:?}"));
            }
            results.dedup();
            println!("loopback alone: {} samples, reads {results:?}", got.len());
        }
        let microphone = match open_source(&Endpoint::Capture(DeviceSelection::Default)) {
            Ok(source) => source,
            Err(failure) => unreachable!("{failure:?}"),
        };
        let mut plain = match open_source(&Endpoint::Capture(DeviceSelection::Default)) {
            Ok(source) => source,
            Err(failure) => unreachable!("{failure:?}"),
        };
        let mut processed = ProcessedSource::new(microphone, speakers.ok(), processing)
            .with_reopen(Box::new(|| {
                open_source(&Endpoint::Loopback(DeviceSelection::Default))
            }));
        let (mut raw, mut out) = (Vec::new(), Vec::new());
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(seconds) {
            let _ = processed.read(&mut out, Duration::from_millis(20));
            let _ = plain.read(&mut raw, Duration::ZERO);
        }
        let tail = |v: &[f32]| v[v.len() / 2..].to_vec();
        println!(
            "speaker samples {} (last loss {:?}) | mic samples {} -> {} | second half: mic {:.1} dB, processed {:.1} dB",
            processed.speaker_samples(),
            processed.speaker_failure(),
            raw.len(),
            out.len(),
            energy_db(&tail(&raw)),
            energy_db(&tail(&out)),
        );
    }

    /// Copies each chunk a source hands out into a shared take, tagged
    /// microphone (0) or speakers (1), in the order the processor sees them.
    struct Tee {
        inner: Box<dyn CaptureSource>,
        kind: u8,
        take: std::rc::Rc<std::cell::RefCell<Vec<u8>>>,
    }

    impl CaptureSource for Tee {
        fn read(
            &mut self,
            out: &mut Vec<f32>,
            wait: Duration,
        ) -> Result<crate::capture::SourceRead, PlatformError> {
            let start = out.len();
            let read = self.inner.read(out, wait);
            let mut take = self.take.borrow_mut();
            take.push(self.kind);
            take.extend_from_slice(&((out.len() - start) as u32).to_le_bytes());
            for sample in &out[start..] {
                take.extend_from_slice(&sample.to_le_bytes());
            }
            read
        }

        fn finish(&mut self, out: &mut Vec<f32>) {
            self.inner.finish(out);
        }
    }

    /// Records a room take for tuning the echo gate on another machine:
    /// the raw microphone and speaker chunks, in order, to
    /// `NARRIVY_TAKE_PATH` (test audio played on purpose, never a user's).
    #[test]
    #[ignore = "needs Windows audio devices and a test clip playing"]
    fn record_room_take() {
        let Some(path) = std::env::var_os("NARRIVY_TAKE_PATH") else {
            return;
        };
        let seconds: u64 = std::env::var("NARRIVY_PROBE_SECONDS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(14);
        let take = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let tee = |inner, kind| -> Box<dyn CaptureSource> {
            Box::new(Tee {
                inner,
                kind,
                take: take.clone(),
            })
        };
        let microphone = match open_source(&Endpoint::Capture(DeviceSelection::Default)) {
            Ok(source) => tee(source, 0),
            Err(failure) => unreachable!("{failure:?}"),
        };
        let speakers = open_source(&Endpoint::Loopback(DeviceSelection::Default))
            .ok()
            .map(|source| tee(source, 1));
        let mut processed = ProcessedSource::new(
            microphone,
            speakers,
            Processing {
                noise: crate::voice_processing::NoiseLevel::Standard,
                auto_gain: false,
            },
        );
        let mut out = Vec::new();
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(seconds) {
            let _ = processed.read(&mut out, Duration::from_millis(20));
        }
        drop(processed);
        let bytes = take.borrow().clone();
        println!("take: {} bytes", bytes.len());
        assert!(std::fs::write(path, bytes).is_ok());
    }
}
