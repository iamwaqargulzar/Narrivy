use super::*;
use narrivy_platform_api::PlatformError;
use std::sync::atomic::AtomicBool;

/// What the fake device does on each read.
#[derive(Clone, Copy)]
enum Step {
    Samples(usize),
    Idle,
    Ended,
    Fail(&'static str),
}

/// Scripted source. After the script it keeps producing `then` samples per
/// read (a live microphone), or idles when `then` is zero.
struct Scripted {
    steps: std::vec::IntoIter<Step>,
    then: usize,
    tail: usize,
    next_value: f32,
    finished: Arc<AtomicBool>,
}

impl CaptureSource for Scripted {
    fn read(&mut self, out: &mut Vec<f32>, wait: Duration) -> Result<SourceRead, PlatformError> {
        let step = self.steps.next().unwrap_or(if self.then == 0 {
            Step::Idle
        } else {
            Step::Samples(self.then)
        });
        match step {
            Step::Samples(count) => {
                for _ in 0..count {
                    out.push(self.next_value);
                    self.next_value += 1.0;
                }
                // Pace like a device so the consumer keeps up.
                std::thread::sleep(Duration::from_millis(1));
                Ok(SourceRead::Data)
            }
            Step::Idle => {
                std::thread::sleep(wait);
                Ok(SourceRead::Idle)
            }
            Step::Ended => Ok(SourceRead::Ended),
            Step::Fail(code) => Err(error(code)),
        }
    }

    fn finish(&mut self, out: &mut Vec<f32>) {
        for _ in 0..self.tail {
            out.push(self.next_value);
            self.next_value += 1.0;
        }
        self.finished.store(true, Ordering::SeqCst);
    }
}

fn opener(steps: Vec<Step>, then: usize, tail: usize) -> (SourceOpener, Arc<AtomicBool>) {
    let finished = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&finished);
    let open: SourceOpener = Box::new(move || {
        Ok(Box::new(Scripted {
            steps: steps.into_iter(),
            then,
            tail,
            next_value: 0.0,
            finished: flag,
        }) as Box<dyn CaptureSource>)
    });
    (open, finished)
}

fn config(seconds: f32) -> CaptureConfig {
    match CaptureConfig::new("default", seconds) {
        Ok(config) => config,
        Err(failure) => unreachable!("{failure:?}"),
    }
}

fn is_contiguous(samples: &[f32]) -> bool {
    samples.iter().enumerate().all(|(i, s)| *s == i as f32)
}

fn code<T>(result: Result<T, PlatformError>) -> &'static str {
    match result {
        Ok(_) => "ok",
        Err(failure) => failure.code,
    }
}

#[test]
fn capture_window_is_validated() {
    assert_eq!(
        code(CaptureConfig::new("default", 0.5)),
        "capture_duration_invalid"
    );
    assert_eq!(
        code(CaptureConfig::new("default", f32::NAN)),
        "capture_duration_invalid"
    );
    assert_eq!(
        code(CaptureConfig::new("default", 301.0)),
        "capture_duration_invalid"
    );
    assert_eq!(config(2.0).max_samples, 32_000);
}

#[test]
fn batch_stop_keeps_everything_including_the_converter_tail() {
    let (open, finished) = opener(vec![], 160, 7);
    let mut session = match BatchCaptureSession::start(&config(10.0), open) {
        Ok(session) => session,
        Err(failure) => unreachable!("{failure:?}"),
    };
    let control = SessionControl::new();
    assert_eq!(code(session.wait_ready(&control)), "ok");
    let stopper = control.clone();
    let mut observed_growth = false;
    let mut last = 0;
    let captured = session.collect_observed(&control, &mut |samples| {
        observed_growth |= samples.len() > last;
        last = samples.len();
        if samples.len() >= 3_200 {
            stopper.request_finalization();
        }
    });
    let captured = match captured {
        Ok(captured) => captured,
        Err(failure) => unreachable!("{failure:?}"),
    };
    assert!(observed_growth);
    assert_eq!(captured.end, CaptureEnd::FinalizationRequested);
    assert_eq!(captured.audio.sample_rate_hz, 16_000);
    assert_eq!(captured.audio.channels, 1);
    assert!(captured.audio.samples.len() >= 3_200);
    assert!(is_contiguous(&captured.audio.samples));
    assert!(finished.load(Ordering::SeqCst), "converter tail flushed");
    assert!(captured.first_frame_after_start_us.is_some());
}

#[test]
fn endpoint_finalization_is_reported_as_endpoint() {
    let (open, _) = opener(vec![], 160, 0);
    let mut session = match BatchCaptureSession::start(&config(10.0), open) {
        Ok(session) => session,
        Err(failure) => unreachable!("{failure:?}"),
    };
    let control = SessionControl::new();
    assert_eq!(code(session.wait_ready(&control)), "ok");
    assert!(control.request_endpoint_finalization());
    match session.collect(&control) {
        Ok(captured) => assert_eq!(captured.end, CaptureEnd::EndpointDetected),
        Err(failure) => unreachable!("{failure:?}"),
    }
}

#[test]
fn device_lost_keeps_what_was_said() {
    // 2 000 samples, then the microphone is unplugged.
    let steps = vec![Step::Samples(1_000), Step::Samples(1_000), Step::Ended];
    let (open, _) = opener(steps, 0, 3);
    let mut session = match BatchCaptureSession::start(&config(10.0), open) {
        Ok(session) => session,
        Err(failure) => unreachable!("{failure:?}"),
    };
    let control = SessionControl::new();
    match session.collect(&control) {
        Ok(captured) => {
            assert_eq!(captured.end, CaptureEnd::SourceEnded);
            assert_eq!(captured.audio.samples.len(), 2_003);
            assert!(is_contiguous(&captured.audio.samples));
        }
        Err(failure) => unreachable!("{failure:?}"),
    }
}

#[test]
fn device_lost_before_any_audio_is_an_error() {
    let (open, _) = opener(vec![Step::Ended], 0, 0);
    let mut session = match BatchCaptureSession::start(&config(10.0), open) {
        Ok(session) => session,
        Err(failure) => unreachable!("{failure:?}"),
    };
    assert_eq!(
        code(session.wait_ready(&SessionControl::new())),
        "capture_output_invalid"
    );
}

#[test]
fn safety_limit_is_exact_and_typed() {
    let (open, _) = opener(vec![], 1_000, 0);
    let mut session = match BatchCaptureSession::start(&config(1.0), open) {
        Ok(session) => session,
        Err(failure) => unreachable!("{failure:?}"),
    };
    match session.collect(&SessionControl::new()) {
        Ok(captured) => {
            assert_eq!(captured.end, CaptureEnd::SafetyLimit);
            assert_eq!(captured.audio.samples.len(), 16_000);
        }
        Err(failure) => unreachable!("{failure:?}"),
    }
}

#[test]
fn cancellation_wins_and_discards_audio() {
    let (open, _) = opener(vec![], 160, 0);
    let mut session = match BatchCaptureSession::start(&config(10.0), open) {
        Ok(session) => session,
        Err(failure) => unreachable!("{failure:?}"),
    };
    let control = SessionControl::new();
    assert_eq!(code(session.wait_ready(&control)), "ok");
    control.request_finalization();
    control.cancel();
    assert_eq!(code(session.collect(&control)), "cancelled");
    assert_eq!(code(session.collect(&control)), "capture_already_collected");
}

#[test]
fn open_failure_is_reported_with_its_code() {
    let open: SourceOpener = Box::new(|| Err(error("capture_device_unavailable")));
    let mut session = match BatchCaptureSession::start(&config(10.0), open) {
        Ok(session) => session,
        Err(failure) => unreachable!("{failure:?}"),
    };
    assert_eq!(
        code(session.wait_ready(&SessionControl::new())),
        "capture_device_unavailable"
    );
}

#[test]
fn read_failure_mid_stream_is_terminal() {
    let steps = vec![Step::Samples(500), Step::Fail("capture_output_unreadable")];
    let (open, _) = opener(steps, 0, 0);
    let mut session = match BatchCaptureSession::start(&config(10.0), open) {
        Ok(session) => session,
        Err(failure) => unreachable!("{failure:?}"),
    };
    assert_eq!(
        code(session.collect(&SessionControl::new())),
        "capture_output_unreadable"
    );
}

#[test]
fn stop_before_audio_is_typed() {
    let (open, _) = opener(vec![], 0, 0);
    let mut session = match BatchCaptureSession::start(&config(10.0), open) {
        Ok(session) => session,
        Err(failure) => unreachable!("{failure:?}"),
    };
    let control = SessionControl::new();
    control.request_finalization();
    assert_eq!(
        code(session.wait_ready(&control)),
        "capture_finalized_before_audio"
    );
}

#[test]
fn dropping_an_open_session_stops_the_producer() {
    let (open, _) = opener(vec![], 160, 0);
    let session = match BatchCaptureSession::start(&config(10.0), open) {
        Ok(session) => session,
        Err(failure) => unreachable!("{failure:?}"),
    };
    let started = Instant::now();
    drop(session);
    assert!(started.elapsed() < CAPTURE_CANCEL_BOUND);
}

fn stream(
    seconds: f32,
    steps: Vec<Step>,
    then: usize,
    tail: usize,
) -> (StreamingCaptureSession, SessionControl) {
    let control = SessionControl::new();
    let (open, _) = opener(steps, then, tail);
    match StreamingCaptureSession::start(&config(seconds), open, &control) {
        Ok(session) => (session, control),
        Err(failure) => unreachable!("{failure:?}"),
    }
}

/// Read events until the end; returns the audio and the summary.
fn drain(
    session: &mut StreamingCaptureSession,
    control: &SessionControl,
    stop_after: Option<usize>,
) -> Result<(Vec<f32>, CaptureStreamSummary, Vec<u64>), PlatformError> {
    let mut audio = Vec::new();
    let mut sequences = Vec::new();
    loop {
        match session.next_event(control)? {
            CaptureStreamEvent::Audio(chunk) => {
                sequences.push(chunk.sequence);
                audio.extend_from_slice(&chunk.audio.samples);
                if stop_after.is_some_and(|limit| audio.len() >= limit) {
                    control.request_finalization();
                }
            }
            CaptureStreamEvent::End(summary) => return Ok((audio, summary, sequences)),
        }
    }
}

#[test]
fn stream_chunks_are_contiguous_and_stop_flushes_the_rest() {
    let (mut session, control) = stream(10.0, vec![], 300, 11);
    assert_eq!(code(session.wait_ready(&control)), "ok");
    let (audio, summary, sequences) = match drain(&mut session, &control, Some(5_000)) {
        Ok(result) => result,
        Err(failure) => unreachable!("{failure:?}"),
    };
    assert_eq!(summary.end, CaptureEnd::FinalizationRequested);
    assert_eq!(summary.total_samples, audio.len() as u64);
    assert!(is_contiguous(&audio));
    assert!(sequences.iter().enumerate().all(|(i, s)| *s == i as u64));
    // The final flush carries a partial chunk (the tail is 11 samples).
    assert_ne!(audio.len() % STREAM_CAPTURE_CHUNK_SAMPLES, 0);
    assert!(summary.first_frame_after_start_us.is_some());
}

#[test]
fn stream_device_lost_ends_with_the_audio_captured() {
    let steps = vec![Step::Samples(2_000), Step::Samples(700), Step::Ended];
    let (mut session, control) = stream(10.0, steps, 0, 0);
    match drain(&mut session, &control, None) {
        Ok((audio, summary, _)) => {
            assert_eq!(summary.end, CaptureEnd::SourceEnded);
            assert_eq!(audio.len(), 2_700);
            assert!(is_contiguous(&audio));
        }
        Err(failure) => unreachable!("{failure:?}"),
    }
}

#[test]
fn stream_reaches_the_safety_limit_exactly() {
    let (mut session, control) = stream(1.0, vec![], 700, 0);
    match drain(&mut session, &control, None) {
        Ok((audio, summary, _)) => {
            assert_eq!(summary.end, CaptureEnd::SafetyLimit);
            assert_eq!(audio.len(), 16_000);
        }
        Err(failure) => unreachable!("{failure:?}"),
    }
}

#[test]
fn a_stalled_consumer_gets_backpressure_not_lost_audio() {
    // 65 chunks arrive at once; the queue holds 64.
    let burst = STREAM_CAPTURE_CHUNK_SAMPLES * (STREAM_CAPTURE_QUEUE_CHUNKS + 1);
    let (mut session, control) = stream(10.0, vec![Step::Samples(burst)], 0, 0);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(code(session.next_event(&control)), "capture_backpressure");
}

#[test]
fn stream_cancellation_is_terminal_even_with_a_prefetched_chunk() {
    let (mut session, control) = stream(10.0, vec![], 300, 0);
    assert_eq!(code(session.wait_ready(&control)), "ok");
    control.cancel();
    assert_eq!(code(session.next_event(&control)), "cancelled");
    assert_eq!(
        code(session.next_event(&control)),
        "capture_already_collected"
    );
}

#[test]
fn stream_abort_releases_the_producer_within_the_deadline() {
    let (mut session, control) = stream(10.0, vec![], 300, 0);
    assert_eq!(code(session.wait_ready(&control)), "ok");
    assert_eq!(
        code(session.abort(Instant::now() + CAPTURE_CANCEL_BOUND)),
        "ok"
    );
    assert_eq!(
        code(session.abort(Instant::now() + CAPTURE_CANCEL_BOUND)),
        "ok"
    );
}

#[test]
fn stream_start_refuses_a_cancelled_transaction() {
    let control = SessionControl::new();
    control.cancel();
    let (open, _) = opener(vec![], 300, 0);
    assert_eq!(
        code(StreamingCaptureSession::start(
            &config(10.0),
            open,
            &control
        )),
        "cancelled"
    );
}

#[test]
fn a_short_capture_gap_is_its_length_in_frames() {
    assert_eq!(gap_frames(48_000, 48_480, 48_000), Some(480));
    // A flag with no measurable gap fills nothing.
    assert_eq!(gap_frames(48_000, 48_000, 48_000), Some(0));
    assert_eq!(gap_frames(48_000, 47_000, 48_000), Some(0));
    assert_eq!(gap_frames(0, 48_000, 48_000), Some(48_000));
}

#[test]
fn a_capture_gap_over_a_second_is_not_filled() {
    assert_eq!(gap_frames(0, 48_001, 48_000), None);
    assert_eq!(gap_frames(16_000, 16_000 + 16_001, 16_000), None);
}

#[test]
fn position_tracker_learns_device_units_and_scales_gaps() {
    // A 16 kHz stream converted from a 48 kHz device: positions move three
    // units per stream frame.
    let mut tracker = PositionTracker::new(1.0);
    assert_eq!(tracker.packet(0, 160, true, 16_000), Some(0));
    assert_eq!(tracker.packet(480, 160, false, 16_000), Some(0));
    assert!((tracker.units_per_frame() - 3.0).abs() < 1e-9);
    // 30 ms lost (1440 device units = 480 stream frames).
    assert_eq!(tracker.packet(960 + 1_440, 160, true, 16_000), Some(480));
    // 0.9 s lost (43 200 device units) is filled, not an overrun.
    assert_eq!(
        tracker.packet(2_880 + 43_200, 160, true, 16_000),
        Some(14_400)
    );
}

#[test]
fn position_tracker_in_stream_frames_and_its_bound() {
    let mut tracker = PositionTracker::new(1.0);
    assert_eq!(tracker.packet(1_000, 160, false, 16_000), Some(0));
    assert_eq!(tracker.packet(1_160, 160, false, 16_000), Some(0));
    assert_eq!(tracker.packet(1_320 + 320, 160, true, 16_000), Some(320));
    // Overlap or a position going back is no gap.
    assert_eq!(tracker.packet(1_600, 160, true, 16_000), Some(0));
    // Over a second is the stream lost.
    assert_eq!(tracker.packet(1_760 + 16_001, 160, true, 16_000), None);
}

#[test]
fn position_tracker_uses_the_guess_before_learning() {
    let mut tracker = PositionTracker::new(3.0);
    assert_eq!(tracker.packet(0, 160, false, 16_000), Some(0));
    assert_eq!(tracker.packet(480 + 1_440, 160, true, 16_000), Some(480));
    assert!((PositionTracker::new(f64::NAN).units_per_frame() - 1.0).abs() < 1e-9);
}
