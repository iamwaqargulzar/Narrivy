//! A long-running recording of one audio source, for meetings: the
//! microphone or the computer's sound, for as long as the meeting lasts.
//!
//! Unlike a dictation capture ([`crate::capture`]) it has no maximum length
//! and does not end when the device goes away. Its job is a file whose time
//! matches the wall clock, so the microphone's and the computer sound's
//! files line up:
//!
//! - a device that goes away (unplugged, disabled, the audio service
//!   restarting) is opened again after [`REOPEN_DELAY`], and the time it
//!   was missing is silence;
//! - a source that delivers nothing for a while (a loopback while nothing
//!   plays and the engine idles) is filled with silence once it falls more
//!   than [`MAX_LAG`] behind the clock;
//! - when the device to record moves (the user picks another default
//!   speaker), the recorder follows it.
//!
//! One thread owns the source (WASAPI's COM objects stay on the thread that
//! made them) and hands mono f32 16 kHz samples to the caller's sink. The
//! sink is called on that thread and must not block for long.

#![forbid(unsafe_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::sync_channel;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use narrivy_platform_api::PlatformError;

use crate::capture::{CAPTURE_RATE_HZ, CaptureSource, SourceRead};
use crate::error;

/// How long one read waits for audio; bounds how quickly Stop is seen.
const POLL: Duration = Duration::from_millis(20);
/// Wait before opening a device again after it went away.
pub const REOPEN_DELAY: Duration = Duration::from_millis(500);
/// How far the file may fall behind the clock before silence fills it.
pub const MAX_LAG: Duration = Duration::from_secs(1);
/// What is left behind the clock after filling: room for audio the engine
/// still holds, so real samples are not pushed late.
const FILL_MARGIN: Duration = Duration::from_millis(250);
/// How often the recorder asks whether the device to record has moved.
const FOLLOW_EVERY: Duration = Duration::from_secs(2);
/// Bound for the first open to report.
const START_BOUND: Duration = Duration::from_secs(5);

/// Opens the source; called on the recorder thread, again after a loss.
pub type StreamOpener =
    Box<dyn FnMut() -> Result<Box<dyn CaptureSource>, PlatformError> + Send + 'static>;
/// Says whether the device to record has changed since the last open (the
/// default speaker moved); called on the recorder thread.
pub type MovedCheck = Box<dyn FnMut() -> bool + Send + 'static>;
/// Receives contract samples (mono f32, 16 kHz) in order, silence included.
pub type SampleSink = Box<dyn FnMut(&[f32]) + Send + 'static>;

/// Samples due after `elapsed` at the contract rate.
fn due_samples(elapsed: Duration) -> u64 {
    let rate = u128::from(CAPTURE_RATE_HZ);
    u64::try_from(elapsed.as_micros() * rate / 1_000_000).unwrap_or(u64::MAX)
}

/// Silence to add when `written` samples lag the `due` ones by more than
/// [`MAX_LAG`]: enough to come within [`FILL_MARGIN`] of the clock.
#[must_use]
pub fn fill_samples(written: u64, due: u64) -> u64 {
    let lag = due.saturating_sub(written);
    if lag <= due_samples(MAX_LAG) {
        return 0;
    }
    lag - due_samples(FILL_MARGIN)
}

/// A recording running on its own thread until [`StreamRecorder::stop`].
pub struct StreamRecorder {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl StreamRecorder {
    /// Open the source on a new thread and record until stopped.
    ///
    /// # Errors
    /// What the first open reports (the device is missing or refuses), or
    /// `meeting_recorder_unavailable` when the thread cannot start.
    pub fn start(
        mut open: StreamOpener,
        mut moved: Option<MovedCheck>,
        mut sink: SampleSink,
    ) -> Result<Self, PlatformError> {
        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = sync_channel::<Result<(), PlatformError>>(1);
        let stopping = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("narrivy-stream-recorder".into())
            .spawn(move || {
                let first = match open() {
                    Ok(source) => {
                        let _ = ready_tx.send(Ok(()));
                        source
                    }
                    Err(failure) => {
                        let _ = ready_tx.send(Err(failure));
                        return;
                    }
                };
                record(first, &mut open, moved.as_mut(), &mut sink, &stopping);
            })
            .map_err(|_| error("meeting_recorder_unavailable"))?;
        let mut recorder = Self {
            stop,
            thread: Some(thread),
        };
        match ready_rx.recv_timeout(START_BOUND) {
            Ok(Ok(())) => Ok(recorder),
            Ok(Err(failure)) => {
                recorder.halt();
                Err(failure)
            }
            Err(_) => {
                recorder.halt();
                Err(error("meeting_recorder_unavailable"))
            }
        }
    }

    /// Stop recording; returns once every sample reached the sink.
    pub fn stop(mut self) {
        self.halt();
    }

    fn halt(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for StreamRecorder {
    fn drop(&mut self) {
        self.halt();
    }
}

fn record(
    first: Box<dyn CaptureSource>,
    open: &mut StreamOpener,
    mut moved: Option<&mut MovedCheck>,
    sink: &mut SampleSink,
    stop: &AtomicBool,
) {
    let started = Instant::now();
    let mut written = 0_u64;
    let mut source = Some(first);
    let mut retry_at = started;
    let mut followed_at = started;
    let mut samples = Vec::new();
    while !stop.load(Ordering::SeqCst) {
        let now = Instant::now();
        let mut lost = false;
        match source.as_mut() {
            Some(current) => match current.read(&mut samples, POLL) {
                Ok(SourceRead::Data | SourceRead::Idle) => {}
                Ok(SourceRead::Ended) | Err(_) => {
                    current.finish(&mut samples);
                    lost = true;
                }
            },
            None if now >= retry_at => match open() {
                Ok(opened) => source = Some(opened),
                Err(_) => retry_at = now + REOPEN_DELAY,
            },
            None => std::thread::sleep(POLL),
        }
        if lost {
            source = None;
            retry_at = Instant::now() + REOPEN_DELAY;
        }
        if source.is_some()
            && now.duration_since(followed_at) >= FOLLOW_EVERY
            && let Some(check) = moved.as_mut()
        {
            followed_at = now;
            if check() {
                if let Some(mut current) = source.take() {
                    current.finish(&mut samples);
                }
                retry_at = now;
            }
        }
        if !samples.is_empty() {
            sink(&samples);
            written += samples.len() as u64;
            samples.clear();
        }
        let fill = fill_samples(written, due_samples(started.elapsed()));
        if fill > 0 {
            push_silence(sink, fill);
            written += fill;
        }
    }
    if let Some(mut current) = source.take() {
        current.finish(&mut samples);
    }
    if !samples.is_empty() {
        sink(&samples);
    }
}

fn push_silence(sink: &mut SampleSink, mut count: u64) {
    let block = [0.0_f32; 1_600];
    while count > 0 {
        let take = usize::try_from(count.min(block.len() as u64)).unwrap_or(block.len());
        sink(&block[..take]);
        count -= take as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;

    /// Delivers 10 ms of ones per read, ending after `ends_after` reads.
    struct Fake {
        reads: usize,
        ends_after: Option<usize>,
    }

    impl CaptureSource for Fake {
        fn read(
            &mut self,
            out: &mut Vec<f32>,
            wait: Duration,
        ) -> Result<SourceRead, PlatformError> {
            std::thread::sleep(wait.min(Duration::from_millis(10)));
            self.reads += 1;
            if self.ends_after.is_some_and(|n| self.reads > n) {
                return Ok(SourceRead::Ended);
            }
            out.extend(std::iter::repeat_n(1.0, 160));
            Ok(SourceRead::Data)
        }

        fn finish(&mut self, _out: &mut Vec<f32>) {}
    }

    fn collecting() -> (Arc<Mutex<Vec<f32>>>, SampleSink) {
        let all = Arc::new(Mutex::new(Vec::new()));
        let into = Arc::clone(&all);
        let sink: SampleSink = Box::new(move |samples: &[f32]| {
            if let Ok(mut all) = into.lock() {
                all.extend_from_slice(samples);
            }
        });
        (all, sink)
    }

    #[test]
    fn no_fill_within_the_lag() {
        assert_eq!(fill_samples(0, 16_000), 0);
        assert_eq!(fill_samples(10_000, 20_000), 0);
    }

    #[test]
    fn a_long_lag_is_filled_to_the_margin() {
        assert_eq!(fill_samples(0, 32_000), 32_000 - 4_000);
        assert_eq!(fill_samples(50_000, 10_000), 0);
    }

    #[test]
    fn a_failed_first_open_is_reported() {
        let (_, sink) = collecting();
        let opener: StreamOpener = Box::new(|| Err(error("capture_device_unavailable")));
        match StreamRecorder::start(opener, None, sink) {
            Err(failure) => assert_eq!(failure.code, "capture_device_unavailable"),
            Ok(_) => unreachable!("the open failed"),
        }
    }

    #[test]
    fn a_lost_device_is_reopened_and_the_time_kept() {
        let opens = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&opens);
        let opener: StreamOpener = Box::new(move || {
            let n = counted.fetch_add(1, Ordering::SeqCst);
            if (1..=2).contains(&n) {
                // The first reopens fail: the device is still away.
                return Err(error("capture_device_unavailable"));
            }
            Ok(Box::new(Fake {
                reads: 0,
                ends_after: (n == 0).then_some(5),
            }) as Box<dyn CaptureSource>)
        });
        let (all, sink) = collecting();
        let recorder = match StreamRecorder::start(opener, None, sink) {
            Ok(recorder) => recorder,
            Err(failure) => unreachable!("{failure:?}"),
        };
        let started = Instant::now();
        std::thread::sleep(Duration::from_millis(2_500));
        recorder.stop();
        let seconds = started.elapsed().as_secs_f64();
        assert!(opens.load(Ordering::SeqCst) >= 4);
        let samples = all.lock().map(|all| all.clone()).unwrap_or_default();
        // Ones before the loss, silence for the gap, ones again after.
        let first_zero = samples.iter().position(|s| *s == 0.0);
        assert!(first_zero.is_some());
        assert!(samples.iter().rev().take(160).all(|s| *s == 1.0));
        // The file keeps the clock: within the lag allowance.
        let recorded = samples.len() as f64 / f64::from(CAPTURE_RATE_HZ);
        assert!(recorded > seconds - 1.3, "{recorded} of {seconds}");
        assert!(recorded < seconds + 0.2, "{recorded} of {seconds}");
    }

    #[test]
    fn a_moved_device_is_followed() {
        let opens = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&opens);
        let opener: StreamOpener = Box::new(move || {
            counted.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(Fake {
                reads: 0,
                ends_after: None,
            }) as Box<dyn CaptureSource>)
        });
        let moved: MovedCheck = Box::new(|| true);
        let (_, sink) = collecting();
        let recorder = match StreamRecorder::start(opener, Some(moved), sink) {
            Ok(recorder) => recorder,
            Err(failure) => unreachable!("{failure:?}"),
        };
        std::thread::sleep(Duration::from_millis(2_300));
        recorder.stop();
        assert!(opens.load(Ordering::SeqCst) >= 2);
    }
}
