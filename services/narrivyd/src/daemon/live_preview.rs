//! Live text for whole-recording models (F3): while the user speaks, the
//! recent audio is decoded every [`CADENCE`] and published as volatile
//! partials, so the overlay shows text for models that cannot stream (such
//! as Whisper on the NPU). Partials never count as the final text: the final
//! recognition runs on the whole recording after the preview has stopped.

#![cfg(feature = "asr-transcribe-resident")]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use narrivy_asr_api::{AsrRequest, OneShotAsrPort};
use narrivy_core::{LivePreviewPort, VolatilePartial};
use narrivy_types::{AudioChunk, SessionCancellation, SessionId};

/// How often the preview decodes. A decode takes about half a second on
/// the NPU, so the next window is ready as each decode ends and the words
/// trail the voice by about that much; a newer window replaces one still
/// waiting.
pub const CADENCE: Duration = Duration::from_millis(500);
/// The preview decodes at most this much recent audio (Whisper's window).
const WINDOW_SAMPLES: usize = 16_000 * 28;
/// Below this loudness (RMS) the recent audio is silence: a model would
/// only invent words for it.
const SILENCE_RMS: f32 = 0.004;
/// Less new audio than this since the last decode is not worth a decode.
const MIN_NEW_SAMPLES: usize = 16_000 * 3 / 10;

struct Shared {
    /// The latest window handed over by the capture loop, and how many
    /// samples had been captured when it was taken.
    pending: Mutex<Option<(Vec<f32>, usize)>>,
    wake: Condvar,
    stop: AtomicBool,
}

/// The daemon's [`LivePreviewPort`]: a thread that decodes the latest window
/// with the session's engine and publishes what it hears.
pub struct LivePreview {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    last_handoff: Option<Instant>,
    handed: usize,
    /// Receives the microphone level (0..=1000) about every [`LEVEL_EVERY`].
    on_level: Box<dyn FnMut(u16) + Send>,
    last_level: Option<Instant>,
}

/// How often the microphone level is reported.
const LEVEL_EVERY: Duration = Duration::from_millis(50);
/// The level is measured over this many recent samples (50 ms).
const LEVEL_SAMPLES: usize = 800;

/// A perceptual loudness for the overlay: -60 dBFS and below is 0, -12 dBFS
/// and above is 1000.
#[must_use]
pub fn level_of(samples: &[f32]) -> u16 {
    if samples.is_empty() {
        return 0;
    }
    let power = samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32;
    let db = 10.0 * power.max(1e-12).log10();
    let scaled = ((db + 60.0) / 48.0).clamp(0.0, 1.0);
    (scaled * 1000.0).round() as u16
}

impl LivePreview {
    /// Start previewing with `engine`, publishing each preview through
    /// `publish`.
    pub fn start(
        mut engine: Box<dyn OneShotAsrPort>,
        session: SessionId,
        request: AsrRequest,
        cancellation: SessionCancellation,
        mut publish: impl FnMut(VolatilePartial) + Send + 'static,
        on_level: impl FnMut(u16) + Send + 'static,
    ) -> Option<Self> {
        let shared = Arc::new(Shared {
            pending: Mutex::new(None),
            wake: Condvar::new(),
            stop: AtomicBool::new(false),
        });
        let worker = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("narrivy-live-preview".into())
            .spawn(move || {
                let mut seq = 0_u64;
                let mut last_text = String::new();
                loop {
                    let window = {
                        let mut pending = worker
                            .pending
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        while pending.is_none() && !worker.stop.load(Ordering::Acquire) {
                            pending = worker
                                .wake
                                .wait(pending)
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                        }
                        if worker.stop.load(Ordering::Acquire) {
                            return;
                        }
                        pending.take()
                    };
                    let Some((samples, _)) = window else {
                        continue;
                    };
                    let audio = AudioChunk {
                        sample_rate_hz: 16_000,
                        channels: 1,
                        samples,
                    };
                    let Ok(transcript) =
                        engine.recognize_with(session, &audio, &cancellation, &request)
                    else {
                        continue;
                    };
                    let text = transcript.text.trim().to_owned();
                    if worker.stop.load(Ordering::Acquire) {
                        return;
                    }
                    if text.is_empty() || text == last_text || text.len() > 16_384 {
                        continue;
                    }
                    seq += 1;
                    last_text.clone_from(&text);
                    publish(VolatilePartial { seq, text });
                }
            })
            .ok()?;
        Some(Self {
            shared,
            thread: Some(thread),
            last_handoff: None,
            handed: 0,
            on_level: Box::new(on_level),
            last_level: None,
        })
    }
}

impl LivePreviewPort for LivePreview {
    fn audio(&mut self, samples: &[f32]) {
        if self.last_level.is_none_or(|at| at.elapsed() >= LEVEL_EVERY) {
            self.last_level = Some(Instant::now());
            (self.on_level)(level_of(
                &samples[samples.len().saturating_sub(LEVEL_SAMPLES)..],
            ));
        }
        let due = self.last_handoff.is_none_or(|at| at.elapsed() >= CADENCE);
        if !due || samples.len() < self.handed + MIN_NEW_SAMPLES {
            return;
        }
        let window = &samples[samples.len().saturating_sub(WINDOW_SAMPLES)..];
        // The last second decides: a pause is not worth a decode.
        let tail = &window[window.len().saturating_sub(16_000)..];
        let energy = tail.iter().map(|s| s * s).sum::<f32>() / tail.len().max(1) as f32;
        self.last_handoff = Some(Instant::now());
        if energy.sqrt() < SILENCE_RMS {
            return;
        }
        self.handed = samples.len();
        let mut pending = self
            .shared
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // A decode still running means the newest window replaces any
        // waiting one.
        *pending = Some((window.to_vec(), samples.len()));
        drop(pending);
        self.shared.wake.notify_one();
    }

    fn finish(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Ok(mut pending) = self.shared.pending.lock() {
            pending.take();
        }
        self.shared.wake.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for LivePreview {
    fn drop(&mut self) {
        self.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_level_follows_loudness() {
        assert_eq!(level_of(&[]), 0);
        assert_eq!(level_of(&[0.0; 800]), 0);
        let quiet = level_of(&[0.003; 800]);
        let speech = level_of(&[0.05; 800]);
        let loud = level_of(&[0.5; 800]);
        assert!(quiet < speech && speech < loud, "{quiet} {speech} {loud}");
        assert_eq!(loud, 1000);
    }
}
