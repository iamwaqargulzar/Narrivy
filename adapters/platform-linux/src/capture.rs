//! Offline-testable audio capture foundation for the Linux adapter (P2 prep → P4 salvage).
//!
//! Salvage-adapted from `code/narrivy-v0.4/adapters/platform-linux/src/audio.rs`
//! (see `archive/docs/management/salvage-inventory.md`, P4 row) and re-bound to v1.3
//! contracts: [`narrivy_types::AudioChunk`] as the portable chunk shape, typed
//! errors, and bounded memory everywhere. No OS/PipeWire runtime dependency
//! lives in this module — live device enumeration and stream wiring land in
//! later P4 work; everything here is unit-testable without a microphone.
//!
//! # Contents
//! - [`CaptureConfig`] — capture format + pre-roll configuration.
//! - [`PreRollRingBuffer`] — bounded fixed-duration ring of f32 samples so the
//!   first syllable is never clipped (FUN-006).
//! - [`MicDevice`] / [`rank_mics`] — deterministic microphone ordering (FUN-011).
//! - [`HotplugEvent`] / [`HotplugLog`] / [`parse_pw_monitor_line`] — device
//!   add/remove events with a bounded log and an offline-parseable line grammar.
//! - [`PolicyAction`] / [`PolicyJournalEntry`] / [`AudioPolicyJournal`] —
//!   crash-safe duck/pause restoration journal (REL-005, REL-009).
//! - [`CaptureStage`] / [`run_capture_stages`] — non-blocking pipeline stage
//!   contract (PERF-006).
//!
//! # Non-blocking contract (PERF-006)
//! No inference, disk, network, blocking lock, or unbounded allocation may run
//! on the real-time capture callback/thread. The APIs here are shaped for
//! that: [`PreRollRingBuffer::push`] overwrites oldest samples within a
//! pre-reserved ring (allocation-free in steady state), and [`CaptureStage`]
//! takes a borrowed chunk and returns no buffers, so implementing stages need
//! not allocate. Format conversion and resampling belong on a worker thread,
//! never inside a stage.
//!
//! # Privacy rule
//! Ordinary logs and `Debug` output must never contain transcript text, audio
//! sample content, or context payloads. [`PreRollRingBuffer`]'s manual `Debug`
//! implementation prints only capacity/counter metadata — never samples. This
//! module performs no logging at all; diagnostics elsewhere must carry short
//! stable reason codes only.

use std::cmp::Reverse;
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::fs::File;
use std::io::Write;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

use narrivy_types::AudioChunk;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Default capture sample rate (Hz) — matches the ASR front-end baseline.
pub const DEFAULT_SAMPLE_RATE_HZ: u32 = 16_000;
/// Default channel count (mono).
pub const DEFAULT_CHANNELS: u16 = 1;
/// Default pre-roll duration kept in memory before a session opens.
pub const DEFAULT_PREROLL_SECONDS: f32 = 1.5;
/// Upper bound on pre-roll duration. Keeps resident memory bounded (~384 KiB
/// per buffer at 48 kHz stereo f32); larger values are rejected, not clamped,
/// so callers cannot silently over-commit memory.
pub const MAX_PREROLL_SECONDS: f32 = 10.0;

/// Capture format and pre-roll configuration.
///
/// Defaults follow the v0.4 practice: 16 kHz mono with 1.5 s of pre-roll.
#[derive(Debug, Clone, PartialEq)]
pub struct CaptureConfig {
    pub sample_rate_hz: u32,
    pub channels: u16,
    pub preroll_seconds: f32,
    /// Optional user/device hint used to bias mic selection upstream; never
    /// interpreted as audio content by this module.
    pub device_hint: Option<String>,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            sample_rate_hz: DEFAULT_SAMPLE_RATE_HZ,
            channels: DEFAULT_CHANNELS,
            preroll_seconds: DEFAULT_PREROLL_SECONDS,
            device_hint: None,
        }
    }
}

impl CaptureConfig {
    /// Validate bounds. Typed errors instead of silent clamping so callers
    /// surface honest failure UX.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.sample_rate_hz == 0 || self.sample_rate_hz > 384_000 {
            return Err(ConfigError::SampleRateOutOfRange(self.sample_rate_hz));
        }
        if self.channels == 0 || self.channels > 32 {
            return Err(ConfigError::ChannelsOutOfRange(self.channels));
        }
        if !self.preroll_seconds.is_finite()
            || self.preroll_seconds <= 0.0
            || self.preroll_seconds > MAX_PREROLL_SECONDS
        {
            return Err(ConfigError::PrerollOutOfRange(self.preroll_seconds));
        }
        Ok(())
    }
}

/// Typed configuration errors.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigError {
    SampleRateOutOfRange(u32),
    ChannelsOutOfRange(u16),
    PrerollOutOfRange(f32),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SampleRateOutOfRange(hz) => write!(f, "sample rate out of range: {hz} Hz"),
            Self::ChannelsOutOfRange(c) => write!(f, "channel count out of range: {c}"),
            Self::PrerollOutOfRange(s) => {
                write!(f, "pre-roll seconds out of (0, {MAX_PREROLL_SECONDS}]: {s}")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

// ---------------------------------------------------------------------------
// PreRollRingBuffer
// ---------------------------------------------------------------------------

/// Errors from [`PreRollRingBuffer`] operations.
#[derive(Debug, Clone, PartialEq)]
pub enum PrerollError {
    /// The stored pre-roll duration is not usable (must be finite and inside
    /// `(0, MAX_PREROLL_SECONDS]`).
    InvalidCapacity(f32),
    /// The stream changed sample rate mid-session; continuity would break.
    SampleRateChanged { expected: u32, got: u32 },
    /// The chunk's channel count does not match the configured layout.
    ChannelMismatch { expected: u16, got: u16 },
}

impl fmt::Display for PrerollError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCapacity(s) => write!(f, "invalid pre-roll capacity: {s} s"),
            Self::SampleRateChanged { expected, got } => {
                write!(
                    f,
                    "sample rate changed mid-stream: expected {expected}, got {got}"
                )
            }
            Self::ChannelMismatch { expected, got } => {
                write!(f, "channel count mismatch: expected {expected}, got {got}")
            }
        }
    }
}

impl std::error::Error for PrerollError {}

/// Bounded fixed-duration ring buffer of interleaved f32 samples.
///
/// Holds up to `capacity_seconds` worth of audio, computed from the sample
/// rate observed at the first push. Oldest samples are overwritten when full,
/// so a session start can drain continuous pre-roll audio and never clip the
/// first syllable (FUN-006).
///
/// * The sample rate is **tracked at push time**: the first non-empty push
///   fixes rate and channel layout and resolves the capacity; later pushes
///   with a different rate or channel count fail with a typed error rather
///   than silently corrupting continuity.
/// * Interleaved multi-channel frames are copied verbatim; this buffer never
///   de-interleaves or resamples (conversion happens off the RT thread).
/// * Steady-state operation is allocation-free: the backing [`VecDeque`] is
///   pre-reserved once at capacity resolution and only overwritten afterwards.
/// * Thread sharing: wrap in `Arc<Mutex<_>>` at the call site if a callback
///   and worker share one instance; keep critical sections bounded to single
///   push/drain calls.
///
/// # Privacy rule
/// `Debug` output deliberately excludes sample data (see module docs).
pub struct PreRollRingBuffer {
    samples: VecDeque<f32>,
    preroll_seconds: f32,
    capacity_samples: Option<usize>,
    sample_rate_hz: Option<u32>,
    channels: Option<u16>,
    pushed_total: u64,
    dropped_total: u64,
}

impl fmt::Debug for PreRollRingBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Privacy: never print buffered samples.
        f.debug_struct("PreRollRingBuffer")
            .field("preroll_seconds", &self.preroll_seconds)
            .field("capacity_samples", &self.capacity_samples)
            .field("sample_rate_hz", &self.sample_rate_hz)
            .field("channels", &self.channels)
            .field("len", &self.samples.len())
            .field("pushed_total", &self.pushed_total)
            .field("dropped_total", &self.dropped_total)
            .finish()
    }
}

impl PreRollRingBuffer {
    /// Create an unconfigured buffer holding `preroll_seconds` of audio once
    /// the sample rate is known (first push). Infallible; invalid durations
    /// surface as [`PrerollError::InvalidCapacity`] at first push.
    pub fn new(preroll_seconds: f32) -> Self {
        Self {
            samples: VecDeque::new(),
            preroll_seconds,
            capacity_samples: None,
            sample_rate_hz: None,
            channels: None,
            pushed_total: 0,
            dropped_total: 0,
        }
    }

    /// Create a fully configured buffer from validated config. Capacity is
    /// resolved eagerly; pushes must then match rate and channels exactly.
    pub fn from_config(config: &CaptureConfig) -> Result<Self, PrerollError> {
        config
            .validate()
            .map_err(|_| PrerollError::InvalidCapacity(config.preroll_seconds))?;
        let mut buf = Self::new(config.preroll_seconds);
        let cap = compute_capacity(
            config.preroll_seconds,
            config.sample_rate_hz,
            config.channels,
        )?;
        buf.samples.reserve_exact(cap);
        buf.capacity_samples = Some(cap);
        buf.sample_rate_hz = Some(config.sample_rate_hz);
        buf.channels = Some(config.channels);
        Ok(buf)
    }

    fn resolve_layout(&mut self, rate: u32, channels: u16) -> Result<usize, PrerollError> {
        if let Some(cap) = self.capacity_samples {
            return Ok(cap);
        }
        let cap = compute_capacity(self.preroll_seconds, rate, channels)?;
        self.samples.reserve_exact(cap);
        self.capacity_samples = Some(cap);
        self.sample_rate_hz = Some(rate);
        self.channels = Some(channels);
        Ok(cap)
    }

    /// Push one chunk of interleaved samples, overwriting the oldest samples
    /// once the ring is full. Empty chunks are accepted as no-ops.
    ///
    /// RT note: bounded work proportional to the chunk length, no allocation
    /// in steady state, no locks, no IO — safe for the capture callback path.
    pub fn push(&mut self, chunk: &AudioChunk) -> Result<(), PrerollError> {
        if chunk.samples.is_empty() {
            return Ok(());
        }
        if let Some(expected) = self.sample_rate_hz {
            if expected != chunk.sample_rate_hz {
                return Err(PrerollError::SampleRateChanged {
                    expected,
                    got: chunk.sample_rate_hz,
                });
            }
        }
        if let Some(expected) = self.channels {
            if expected != chunk.channels {
                return Err(PrerollError::ChannelMismatch {
                    expected,
                    got: chunk.channels,
                });
            }
        }
        let cap = self.resolve_layout(chunk.sample_rate_hz, chunk.channels)?;

        let n = chunk.samples.len();
        let room = cap - self.samples.len();
        let copy_now = n.min(room);
        let overflow = n - copy_now;

        // Extending only up to `cap` never reallocates past the reservation.
        self.samples
            .extend(chunk.samples[..copy_now].iter().copied());
        for &s in &chunk.samples[copy_now..] {
            self.samples.pop_front();
            self.samples.push_back(s);
        }
        self.pushed_total += n as u64;
        self.dropped_total += overflow as u64;
        debug_assert!(self.samples.len() <= cap);
        Ok(())
    }

    /// Drain all buffered samples into `out` in original interleaved order,
    /// replacing its contents and setting its format metadata once known.
    /// Returns the number of samples moved. Continuity is preserved: samples
    /// leave in the exact order they were pushed.
    pub fn drain_into(&mut self, out: &mut AudioChunk) -> usize {
        out.samples.clear();
        if let Some(rate) = self.sample_rate_hz {
            out.sample_rate_hz = rate;
        }
        if let Some(ch) = self.channels {
            out.channels = ch;
        }
        let drained = self.samples.len();
        out.samples.extend(self.samples.drain(..));
        drained
    }

    /// Copy buffered samples into `out` without draining (peek), preserving
    /// order. `out` contents are replaced.
    pub fn peek_into(&self, out: &mut Vec<f32>) {
        out.clear();
        out.extend(self.samples.iter().copied());
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn capacity_samples(&self) -> Option<usize> {
        self.capacity_samples
    }

    /// Configured capacity in seconds, whether or not the sample-capacity has
    /// been resolved yet.
    pub fn preroll_seconds(&self) -> f32 {
        self.preroll_seconds
    }

    pub fn sample_rate_hz(&self) -> Option<u32> {
        self.sample_rate_hz
    }

    pub fn channels(&self) -> Option<u16> {
        self.channels
    }

    pub fn pushed_total(&self) -> u64 {
        self.pushed_total
    }

    pub fn dropped_total(&self) -> u64 {
        self.dropped_total
    }

    pub fn clear(&mut self) {
        self.samples.clear();
    }
}

fn compute_capacity(
    preroll_seconds: f32,
    sample_rate_hz: u32,
    channels: u16,
) -> Result<usize, PrerollError> {
    if !preroll_seconds.is_finite()
        || preroll_seconds <= 0.0
        || preroll_seconds > MAX_PREROLL_SECONDS
        || sample_rate_hz == 0
        || channels == 0
    {
        return Err(PrerollError::InvalidCapacity(preroll_seconds));
    }
    let frames_f = (preroll_seconds as f64) * (sample_rate_hz as f64);
    if !frames_f.is_finite() || frames_f <= 0.0 {
        return Err(PrerollError::InvalidCapacity(preroll_seconds));
    }
    // Round to whole frames so multi-channel rings never split a frame pair
    // at the overwrite boundary; guard against float artifacts like
    // 16.000000…7 for 1 ms @ 16 kHz.
    let frames_rounded = (frames_f.round() as u128).max(1);
    let frames_u = usize::try_from(frames_rounded)
        .map_err(|_| PrerollError::InvalidCapacity(preroll_seconds))?;
    frames_u
        .checked_mul(channels as usize)
        .ok_or(PrerollError::InvalidCapacity(preroll_seconds))
}

// ---------------------------------------------------------------------------
// Mic devices, deterministic ranking
// ---------------------------------------------------------------------------

/// One enumerable microphone source.
///
/// `rate_hz == 0` means the native rate is unknown (e.g. parsed from a monitor
/// line without a rate field); unknown-capability devices rank below known
/// ones. Names are device labels, never audio content.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MicDevice {
    /// Stable backend identifier (e.g. PipeWire object serial/path).
    pub id: String,
    /// Human-readable device name (safe for UI/diagnostics).
    pub name: String,
    /// Whether the system marks this as the default source.
    pub default: bool,
    /// Native/current rate in Hz; 0 = unknown.
    pub rate_hz: u32,
    pub channels: u16,
}

impl MicDevice {
    /// A device counts as 16 kHz-capable when its reported rate can cover a
    /// 16 kHz capture stream (equal or higher native rate).
    pub fn supports_16k(&self) -> bool {
        self.rate_hz >= 16_000
    }

    /// True when this device appears to be a synthetic test fixture.
    ///
    /// Test harnesses create virtual sources via `module-remap-source` and
    /// friends that leak names like `narrivy_test` or `remap`. On user
    /// machines these must not pollute mic ranking; they are excluded unless
    /// `NARRIVY_ALLOW_VIRTUAL_MIC` is set (honest test isolation).
    pub fn is_virtual(&self) -> bool {
        self.name.contains("narrivy_test") || self.name.contains("remap")
    }
}

/// Deterministic microphone ordering (FUN-011):
///
/// 1. default devices first;
/// 2. then devices capable of ≥16 kHz (`rate_hz >= 16_000`) before the rest;
/// 3. then ascending by `name` (byte-wise, case-sensitive — deterministic
///    across runs and locales);
/// 4. ties broken by ascending `id`.
///
/// Pure function of the input slice; equal inputs always produce identical
/// orderings regardless of input permutation.
pub fn rank_mics(devices: &[MicDevice]) -> Vec<MicDevice> {
    let allow_virtual = std::env::var("NARRIVY_ALLOW_VIRTUAL_MIC").is_ok();
    let filtered: Vec<MicDevice> = if allow_virtual {
        devices.to_vec()
    } else {
        devices
            .iter()
            .filter(|d| !d.is_virtual())
            .cloned()
            .collect()
    };
    // If filtering removed everything, fall back to the full list so the
    // caller still gets a deterministic ranking (never an empty surprise).
    let mut ranked = if filtered.is_empty() && !devices.is_empty() {
        devices.to_vec()
    } else {
        filtered
    };
    ranked.sort_by(|a, b| mic_sort_key(a).cmp(&mic_sort_key(b)));
    ranked
}

fn mic_sort_key(d: &MicDevice) -> (Reverse<bool>, Reverse<bool>, &str, &str) {
    (
        Reverse(d.default),
        Reverse(d.supports_16k()),
        d.name.as_str(),
        d.id.as_str(),
    )
}

// ---------------------------------------------------------------------------
// Hotplug events, bounded log, offline line parser
// ---------------------------------------------------------------------------

/// Microphone hotplug event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotplugEvent {
    DeviceAdded(MicDevice),
    DeviceRemoved(String),
}

/// Bounded hotplug event log (drop-oldest ring). Prevents unbounded growth
/// during device churn while keeping recent events for diagnostics and
/// fallback decisions (FUN-011).
#[derive(Debug)]
pub struct HotplugLog {
    events: VecDeque<HotplugEvent>,
    capacity: usize,
}

impl HotplugLog {
    /// Create a log holding at most `capacity` events (minimum 1; smaller
    /// values are raised to 1 rather than erroring — the log is best-effort).
    pub fn new(capacity: usize) -> Self {
        Self {
            events: VecDeque::with_capacity(capacity.max(1)),
            capacity: capacity.max(1),
        }
    }

    /// Record an event; drops the oldest when at capacity.
    pub fn record(&mut self, event: HotplugEvent) {
        if self.events.len() >= self.capacity {
            self.events.pop_front();
        }
        self.events.push_back(event);
    }

    pub fn iter(&self) -> impl Iterator<Item = &HotplugEvent> {
        self.events.iter()
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn clear(&mut self) {
        self.events.clear();
    }
}

/// Parse one `pw-monitor`-style line into a [`HotplugEvent`].
///
/// # Accepted grammar
///
/// The v0.4 implementation shelled out to `pw-record` and never parsed monitor
/// text, so there is no legacy wire format to preserve. This grammar is
/// defined here (modeled loosely on `pw-mon` vocabulary) so hotplug handling
/// is fully testable offline; the P4 PipeWire watcher adapts real `pw-mon` /
/// `pw-dump` output into these lines.
///
/// One UTF-8 line, `key=value` tokens separated by ASCII whitespace, verb
/// first, case-insensitive:
///
/// ```text
/// added   id=<id> name=<name> [default=<true|false>] [rate=<hz>] [channels=<n>]
/// removed id=<id>
/// ```
///
/// * Aliases: `add` for `added`; `remove` for `removed`.
/// * Values may be double-quoted with backslash escapes (`\"`, `\\`);
///   unquoted values contain no whitespace.
/// * Defaults when omitted: `default=false`, `rate=0` (= unknown),
///   `channels=1`.
/// * Unknown keys, duplicate keys, missing required keys, malformed numbers,
///   control characters, or trailing garbage reject the line ([`None`]).
///
/// Lines describe devices only; they never carry audio content.
pub fn parse_pw_monitor_line(line: &str) -> Option<HotplugEvent> {
    let trimmed = line.trim();
    let (verb, rest) = trimmed.split_once(char::is_whitespace)?;
    match verb.to_ascii_lowercase().as_str() {
        "added" | "add" => {
            let kv = parse_kv_fields(rest)?;
            const ADDED_KEYS: [&str; 5] = ["id", "name", "default", "rate", "channels"];
            if kv.iter().any(|(k, _)| !ADDED_KEYS.contains(&k.as_str())) {
                return None;
            }
            let id = get_value(&kv, "id")?;
            let name = get_value(&kv, "name")?;
            let default = match get_value(&kv, "default") {
                None => false,
                Some(v) if v.eq_ignore_ascii_case("true") => true,
                Some(v) if v.eq_ignore_ascii_case("false") => false,
                Some(_) => return None,
            };
            let rate_hz = match get_value(&kv, "rate") {
                None => 0,
                Some(v) => parse_u32_field(v)?,
            };
            let channels = match get_value(&kv, "channels") {
                None => 1,
                Some(v) => parse_u16_field(v)?,
            };
            Some(HotplugEvent::DeviceAdded(MicDevice {
                id: id.to_string(),
                name: name.to_string(),
                default,
                rate_hz,
                channels,
            }))
        }
        "removed" | "remove" => {
            let kv = parse_kv_fields(rest)?;
            if kv.iter().any(|(k, _)| k != "id") {
                return None;
            }
            let id = get_value(&kv, "id")?;
            Some(HotplugEvent::DeviceRemoved(id.to_string()))
        }
        _ => None,
    }
}

fn get_value<'a>(kv: &'a [(String, String)], key: &str) -> Option<&'a str> {
    kv.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

fn parse_u32_field(v: &str) -> Option<u32> {
    if v.is_empty() || v.starts_with('+') || v.starts_with('-') {
        return None;
    }
    v.parse::<u32>().ok()
}

fn parse_u16_field(v: &str) -> Option<u16> {
    let n = parse_u32_field(v)?;
    if n == 0 {
        return None;
    }
    u16::try_from(n).ok()
}

/// Tokenize `key=value` fields into ordered owned pairs. Duplicate keys and
/// unknown shapes are rejected by returning [`None`].
fn parse_kv_fields(rest: &str) -> Option<Vec<(String, String)>> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut s = rest;
    loop {
        s = s.trim_start();
        if s.is_empty() {
            return Some(out);
        }
        let eq = s.find('=')?;
        let key = &s[..eq];
        if key.is_empty() || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            return None;
        }
        s = &s[eq + 1..];

        let (value, consumed) = parse_field_value(s)?;
        s = &s[consumed..];
        if out.iter().any(|(k, _)| k == key) {
            return None; // duplicate key
        }
        out.push((key.to_string(), value));
    }
}

/// Parse one field value starting at `s`: either double-quoted (with `\"` /
/// `\\` escapes) or a bare run of non-whitespace. Returns the value and how
/// many bytes of `s` were consumed.
fn parse_field_value(s: &str) -> Option<(String, usize)> {
    if let Some(inner) = s.strip_prefix('"') {
        let mut val = String::new();
        let mut end_idx: Option<usize> = None;
        let mut it = inner.char_indices();
        while end_idx.is_none() {
            let (idx, c) = it.next()?;
            match c {
                '"' => end_idx = Some(idx + 1),
                '\\' => {
                    let (_, esc) = it.next()?;
                    match esc {
                        '"' => val.push('"'),
                        '\\' => val.push('\\'),
                        _ => return None,
                    }
                }
                c if (c as u32) < 0x20 => return None, // control char in quoted value
                c => val.push(c),
            }
        }
        Some((val, 1 + end_idx?)) // +1 for the opening quote before `inner`
    } else {
        let end = s.find(char::is_whitespace).unwrap_or(s.len());
        Some((s[..end].to_string(), end))
    }
}

// ---------------------------------------------------------------------------
// Audio policy journal: duck/pause restoration (REL-005, REL-009)
// ---------------------------------------------------------------------------

/// Session-audio policy action recorded in the journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyAction {
    /// Playback was ducked (volume lowered); restore to prior volume.
    Duck,
    /// Playback was paused; restore by resuming.
    Pause,
}

impl PolicyAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Duck => "duck",
            Self::Pause => "pause",
        }
    }

    /// Strict inverse of [`PolicyAction::as_str`] (lowercase only).
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "duck" => Some(Self::Duck),
            "pause" => Some(Self::Pause),
            _ => None,
        }
    }
}

/// One applied policy mutation awaiting restoration.
///
/// `restore_volume` is the pre-mutation stream/sink volume (`None` when the
/// action has no volume component, e.g. pause). Timestamps are supplied by
/// the caller so writes are deterministic under test.
#[derive(Debug, Clone, PartialEq)]
pub struct PolicyJournalEntry {
    pub action: PolicyAction,
    /// PipeWire sink/node identifier the mutation targeted.
    pub target_sink: String,
    pub restore_volume: Option<f32>,
    pub at_unix_ms: u64,
    /// PID of the process that applied the mutation and owns restoration.
    /// Recovery treats an entry as orphaned when this process no longer
    /// exists (`0` = unknown writer, always treated as orphaned).
    pub owner_pid: u32,
    /// Wall-clock time in Unix milliseconds when the mutation was applied.
    /// Distinct from `at_unix_ms` (which is the journal-append time) — this
    /// field records the moment the OS mutation succeeded, enabling
    /// idempotency checks and honest diagnostics without replaying audio.
    pub applied_at_ms: Option<u64>,
}

/// Typed journal errors.
#[derive(Debug)]
pub enum JournalError {
    Io(std::io::Error),
    /// Journal file exists but is not valid UTF-8.
    NotUtf8,
    /// A complete (newline-terminated) line failed to parse. Torn final lines
    /// are skipped instead (see [`AudioPolicyJournal::read_all`]).
    CorruptLine {
        line_no: usize,
        reason: &'static str,
    },
    /// Volumes must be finite; NaN/Inf cannot round-trip through JSON.
    InvalidVolume(f32),
}

impl fmt::Display for JournalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "journal io error: {e}"),
            Self::NotUtf8 => write!(f, "journal file is not UTF-8"),
            Self::CorruptLine { line_no, reason } => {
                write!(f, "corrupt journal line {line_no}: {reason}")
            }
            Self::InvalidVolume(v) => write!(f, "journal entry volume is not finite: {v}"),
        }
    }
}

impl std::error::Error for JournalError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for JournalError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// Crash-safe duck/pause restoration journal.
///
/// # Format
/// Newline-delimited JSON, one [`PolicyJournalEntry`] per line:
///
/// ```json
/// {"action":"duck","target_sink":"alsa_output.pci-0000_0a_00.analog-stereo","restore_volume":0.65,"at_unix_ms":1755860000123}
/// ```
///
/// # Why append-only is crash-safe here
/// Each record is written as one small `write_all` on an `O_APPEND` handle,
/// followed by `flush` + `sync_data`:
///
/// * With `O_APPEND`, the kernel updates the file offset atomically with each
///   write, so records never overwrite each other even after concurrent
///   writers or a reopen.
/// * A crash mid-write can tear **only the trailing record**; every earlier
///   synced record stays intact and parseable. Recovery therefore skips an
///   unterminated final line instead of failing the whole file.
/// * `sync_data` bounds the loss window to records accepted since the last
///   sync — acceptable for low-frequency policy changes.
/// * tmp+rename per line would be strictly worse: rename replaces the whole
///   file, forcing a rewrite of all earlier records (more IO, more tear risk)
///   while adding no atomicity that appends lack. Rename-based atomicity
///   matters for whole-file rewrites (e.g. the mic override), not logs.
///
/// Restoration flow (REL-005/REL-009): apply the mutation first, then append
/// the entry; on clean shutdown remove entries as they are restored; on
/// startup call [`AudioPolicyJournal::recover_open_actions`] and restore what
/// remains — abrupt process death leaves exactly the unrestored set on disk.
#[derive(Debug)]
pub struct AudioPolicyJournal {
    path: PathBuf,
}

impl AudioPolicyJournal {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    /// Append one entry durably (append + flush + sync_data).
    pub fn append(&self, entry: &PolicyJournalEntry) -> Result<(), JournalError> {
        if let Some(v) = entry.restore_volume {
            if !v.is_finite() {
                return Err(JournalError::InvalidVolume(v));
            }
        }
        let mut line = encode_entry_line(entry);
        line.push('\n');
        let mut file = File::options().create(true).append(true).open(&self.path)?;
        file.write_all(line.as_bytes())?;
        file.flush()?;
        file.sync_data()?;
        Ok(())
    }

    /// Read every complete entry. A torn (unterminated) trailing line — the
    /// only damage an abrupt kill can produce under the append protocol — is
    /// silently skipped; any other malformed complete line is a typed error.
    pub fn read_all(&self) -> Result<Vec<PolicyJournalEntry>, JournalError> {
        let raw = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(JournalError::Io(e)),
        };
        let raw = String::from_utf8(raw).map_err(|_| JournalError::NotUtf8)?;
        let mut lines: Vec<&str> = raw.split('\n').collect();
        if lines.last() == Some(&"") {
            // File ended cleanly with a newline.
            lines.pop();
        } else if !lines.is_empty() {
            // Unterminated trailing line: torn record from a crash; ignore it.
            lines.pop();
        }
        let mut out = Vec::new();
        for (idx, line) in lines.iter().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let entry = decode_entry_line(line).map_err(|reason| JournalError::CorruptLine {
                line_no: idx + 1,
                reason,
            })?;
            out.push(entry);
        }
        Ok(out)
    }

    /// Startup recovery: return all open (possibly unrestored) actions in
    /// chronological order. Entries for the same `(action, target_sink)` may
    /// repeat; the caller restores "latest wins" (see [`collapse_latest`]).
    pub fn recover_open_actions(&self) -> Result<Vec<PolicyJournalEntry>, JournalError> {
        self.read_all()
    }

    /// Remove the journal after confirmed restoration. Missing files are fine.
    pub fn clear(&self) -> Result<(), JournalError> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(JournalError::Io(e)),
        }
    }

    /// Verify that a second restoration is a no-op — i.e., after the journal
    /// is cleared, re-reading yields no open actions. This guards the
    /// REL-005 idempotency invariant: `restore` called twice must not
    /// double-apply volume changes.
    pub fn verify_restoration_idempotent(&self) -> Result<bool, JournalError> {
        let before = self.read_all()?;
        if before.is_empty() {
            // Already empty: idempotent by vacuity — second read also empty.
            return Ok(true);
        }
        self.clear()?;
        let after_clear = self.read_all()?;
        if !after_clear.is_empty() {
            return Ok(false);
        }
        // Second clear must also be a no-op and leave empty.
        self.clear()?;
        let second = self.read_all()?;
        Ok(second.is_empty())
    }
}

/// Reduce ordered entries to one per `(action, target_sink)`, keeping the
/// latest occurrence per key, output sorted by `at_unix_ms` ascending.
pub fn collapse_latest(entries: &[PolicyJournalEntry]) -> Vec<PolicyJournalEntry> {
    let mut last_index: HashMap<(&'static str, &str), usize> = HashMap::new();
    for (i, e) in entries.iter().enumerate() {
        // Latest by timestamp; equal timestamps keep the later-written entry.
        match last_index.get(&(e.action.as_str(), e.target_sink.as_str())) {
            Some(&j) if entries[j].at_unix_ms > e.at_unix_ms => {}
            _ => {
                last_index.insert((e.action.as_str(), e.target_sink.as_str()), i);
            }
        }
    }
    let mut picked: Vec<usize> = last_index.into_values().collect();
    picked.sort_unstable_by_key(|&i| entries[i].at_unix_ms);
    picked.into_iter().map(|i| entries[i].clone()).collect()
}

fn encode_entry_line(e: &PolicyJournalEntry) -> String {
    let volume = match e.restore_volume {
        None => "null".to_string(),
        Some(v) => format!("{v}"), // Display gives shortest round-trip form
    };
    let applied = match e.applied_at_ms {
        None => "null".to_string(),
        Some(v) => format!("{v}"),
    };
    format!(
        "{{\"action\":\"{}\",\"target_sink\":\"{}\",\"restore_volume\":{},\"at_unix_ms\":{},\"owner_pid\":{},\"applied_at_ms\":{}}}",
        e.action.as_str(),
        json_escape_string(&e.target_sink),
        volume,
        e.at_unix_ms,
        e.owner_pid,
        applied
    )
}

fn json_escape_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

struct LineParser<'a> {
    s: &'a str,
    pos: usize,
}

impl<'a> LineParser<'a> {
    fn new(s: &'a str) -> Self {
        Self { s, pos: 0 }
    }

    fn peek(&self) -> Option<u8> {
        self.s.as_bytes().get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ') | Some(b'\t')) {
            self.pos += 1;
        }
    }

    fn expect(&mut self, b: u8) -> Result<(), &'static str> {
        if self.peek() == Some(b) {
            self.pos += 1;
            Ok(())
        } else {
            Err("unexpected character")
        }
    }

    fn parse_hex4(&mut self) -> Result<u32, &'static str> {
        let end = self.pos + 4;
        let hex = self
            .s
            .get(self.pos..end)
            .ok_or("truncated unicode escape")?;
        let v = u32::from_str_radix(hex, 16).map_err(|_| "malformed unicode escape")?;
        self.pos = end;
        Ok(v)
    }

    fn parse_string(&mut self) -> Result<String, &'static str> {
        self.expect(b'"')?;
        let bytes = self.s.as_bytes();
        let mut out = String::new();
        loop {
            let b = *bytes.get(self.pos).ok_or("unterminated string")?;
            match b {
                b'"' => {
                    self.pos += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.pos += 1;
                    let e = *bytes.get(self.pos).ok_or("unterminated escape")?;
                    self.pos += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{08}'),
                        b'f' => out.push('\u{0C}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hi = self.parse_hex4()?;
                            let cp = if (0xD800..0xDC00).contains(&hi) {
                                if bytes.get(self.pos) != Some(&b'\\')
                                    || bytes.get(self.pos + 1) != Some(&b'u')
                                {
                                    return Err("lone high surrogate");
                                }
                                self.pos += 2;
                                let lo = self.parse_hex4()?;
                                if !(0xDC00..0xE000).contains(&lo) {
                                    return Err("bad surrogate pair");
                                }
                                0x1_0000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                            } else if (0xDC00..0xE000).contains(&hi) {
                                return Err("lone low surrogate");
                            } else {
                                hi
                            };
                            let ch = char::from_u32(cp).ok_or("invalid unicode scalar")?;
                            out.push(ch);
                        }
                        _ => return Err("unknown escape"),
                    }
                }
                c if c < 0x20 => return Err("raw control character in string"),
                c => {
                    // Copy one full UTF-8 scalar; continuation bytes are >=0x80
                    // and only reachable here as part of their lead sequence.
                    let len = match c {
                        0x00..=0x7F => 1,
                        0xC0..=0xDF => 2,
                        0xE0..=0xEF => 3,
                        _ => 4,
                    };
                    let end = self.pos + len;
                    let chunk = self.s.get(self.pos..end).ok_or("truncated UTF-8")?;
                    out.push_str(chunk);
                    self.pos = end;
                }
            }
        }
    }

    fn parse_number_raw(&mut self) -> Result<&'a str, &'static str> {
        let start = self.pos;
        let bytes = self.s.as_bytes();
        while let Some(&c) = bytes.get(self.pos) {
            match c {
                b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E' => self.pos += 1,
                _ => break,
            }
        }
        if start == self.pos {
            return Err("expected number");
        }
        Ok(&self.s[start..self.pos])
    }
}

/// Strict decoder for exactly the schema produced by [`encode_entry_line`].
/// Unknown/duplicate keys, missing fields, and trailing data are rejected.
fn decode_entry_line(line: &str) -> Result<PolicyJournalEntry, &'static str> {
    let mut p = LineParser::new(line);
    p.skip_ws();
    p.expect(b'{')?;

    let mut action: Option<PolicyAction> = None;
    let mut target_sink: Option<String> = None;
    let mut restore_volume: Option<f32> = None;
    let mut restore_volume_present = false;
    let mut at_unix_ms: Option<u64> = None;
    let mut owner_pid: Option<u32> = None;
    let mut applied_at_ms: Option<u64> = None;
    let mut applied_at_ms_present = false;

    let mut need_comma = false;
    loop {
        p.skip_ws();
        match p.peek() {
            Some(b'}') => {
                p.pos += 1;
                break;
            }
            Some(b',') if need_comma => {
                p.pos += 1;
                p.skip_ws();
            }
            Some(_) if need_comma => return Err("expected ','"),
            _ => {}
        }
        let key = p.parse_string()?;
        p.skip_ws();
        p.expect(b':')?;
        p.skip_ws();
        match key.as_str() {
            "action" => {
                if action.is_some() {
                    return Err("duplicate key");
                }
                let raw = p.parse_string()?;
                action = Some(PolicyAction::parse(&raw).ok_or("unknown action")?);
            }
            "target_sink" => {
                if target_sink.is_some() {
                    return Err("duplicate key");
                }
                target_sink = Some(p.parse_string()?);
            }
            "restore_volume" => {
                if restore_volume_present {
                    return Err("duplicate key");
                }
                restore_volume_present = true;
                if p.s[p.pos..].starts_with("null") {
                    p.pos += 4;
                } else {
                    let raw = p.parse_number_raw()?;
                    let v: f32 = raw.parse().map_err(|_| "malformed number")?;
                    if !v.is_finite() {
                        return Err("non-finite number");
                    }
                    restore_volume = Some(v);
                }
            }
            "at_unix_ms" => {
                if at_unix_ms.is_some() {
                    return Err("duplicate key");
                }
                let raw = p.parse_number_raw()?;
                at_unix_ms = Some(raw.parse::<u64>().map_err(|_| "malformed integer")?);
            }
            "owner_pid" => {
                if owner_pid.is_some() {
                    return Err("duplicate key");
                }
                let raw = p.parse_number_raw()?;
                owner_pid = Some(raw.parse::<u32>().map_err(|_| "malformed integer")?);
            }
            "applied_at_ms" => {
                if applied_at_ms_present {
                    return Err("duplicate key");
                }
                applied_at_ms_present = true;
                if p.s[p.pos..].starts_with("null") {
                    p.pos += 4;
                } else {
                    let raw = p.parse_number_raw()?;
                    applied_at_ms = Some(raw.parse::<u64>().map_err(|_| "malformed integer")?);
                }
            }
            _ => return Err("unknown key"),
        }
        need_comma = true;
    }

    p.skip_ws();
    if p.pos != p.s.len() {
        return Err("trailing data after object");
    }
    Ok(PolicyJournalEntry {
        action: action.ok_or("missing field: action")?,
        target_sink: target_sink.ok_or("missing field: target_sink")?,
        restore_volume,
        at_unix_ms: at_unix_ms.ok_or("missing field: at_unix_ms")?,
        // Writers older than the P4 ownership field are treated as unknown
        // owners (pid 0), which recovery always considers orphaned.
        owner_pid: owner_pid.unwrap_or(0),
        applied_at_ms,
    })
}

// ---------------------------------------------------------------------------
// Non-blocking capture pipeline stage contract (PERF-006)
// ---------------------------------------------------------------------------

/// Why a capture stage stopped processing a chunk.
///
/// Faults carry short stable reason **codes** only — never audio content or
/// payload text (privacy rule).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageStop {
    /// Bounded pipeline is saturated; upstream applies its drop-oldest policy.
    Backpressured,
    /// Stage-level fault; the code selects the failure UX.
    Fault(&'static str),
}

/// One replaceable stage in the bounded capture chain (FUN-002/P4 modular
/// audio requirement).
///
/// # Real-time contract (PERF-006)
/// Implementations must not allocate, block, log, touch disk/network, run
/// inference, or hold locks beyond try-lock inside [`CaptureStage::process`].
/// The API shape enforces this by construction: input is borrowed and nothing
/// is returned except a control decision, so there is no way to satisfy the
/// contract that requires allocating or returning buffers. Work proportional
/// to the chunk length is allowed. Format conversion/resampling belongs on a
/// worker thread, not in a stage.
pub trait CaptureStage {
    fn process(&mut self, chunk: &AudioChunk) -> ControlFlow<StageStop>;
}

/// Feed one chunk through the stages in order; the first [`ControlFlow::Break`]
/// short-circuits the chain and is returned verbatim.
pub fn run_capture_stages(
    stages: &mut [&mut dyn CaptureStage],
    chunk: &AudioChunk,
) -> ControlFlow<StageStop> {
    for stage in stages.iter_mut() {
        match stage.process(chunk) {
            ControlFlow::Break(stop) => return ControlFlow::Break(stop),
            ControlFlow::Continue(()) => {}
        }
    }
    ControlFlow::Continue(())
}

/// No-op stage; placeholder for the conservative built-in chain and tests.
#[derive(Debug, Default, Clone, Copy)]
pub struct PassthroughStage;

impl CaptureStage for PassthroughStage {
    fn process(&mut self, _chunk: &AudioChunk) -> ControlFlow<StageStop> {
        ControlFlow::Continue(())
    }
}

/// Adapter sinking chunks into a [`PreRollRingBuffer`] from the RT callback
/// path. Format errors become typed fault codes (never payload content).
#[derive(Debug)]
pub struct PrerollStage {
    buffer: PreRollRingBuffer,
}

impl PrerollStage {
    pub fn new(buffer: PreRollRingBuffer) -> Self {
        Self { buffer }
    }

    pub fn buffer(&self) -> &PreRollRingBuffer {
        &self.buffer
    }

    pub fn buffer_mut(&mut self) -> &mut PreRollRingBuffer {
        &mut self.buffer
    }

    pub fn into_inner(self) -> PreRollRingBuffer {
        self.buffer
    }
}

impl CaptureStage for PrerollStage {
    fn process(&mut self, chunk: &AudioChunk) -> ControlFlow<StageStop> {
        match self.buffer.push(chunk) {
            Ok(()) => ControlFlow::Continue(()),
            Err(PrerollError::InvalidCapacity(_)) => {
                ControlFlow::Break(StageStop::Fault("preroll-capacity-invalid"))
            }
            Err(PrerollError::SampleRateChanged { .. }) => {
                ControlFlow::Break(StageStop::Fault("preroll-rate-changed"))
            }
            Err(PrerollError::ChannelMismatch { .. }) => {
                ControlFlow::Break(StageStop::Fault("preroll-channel-mismatch"))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests — all offline; no live mic, no PipeWire daemon required
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;
    use std::io;

    type TestResult<T = ()> = Result<T, Box<dyn Error>>;

    fn expected_error<T, E>(result: Result<T, E>, context: &str) -> TestResult<E> {
        match result {
            Ok(_) => Err(io::Error::other(context).into()),
            Err(error) => Ok(error),
        }
    }

    fn required<T>(value: Option<T>, context: &str) -> TestResult<T> {
        value.ok_or_else(|| io::Error::other(context).into())
    }

    fn chunk(rate: u32, channels: u16, samples: Vec<f32>) -> AudioChunk {
        AudioChunk {
            sample_rate_hz: rate,
            channels,
            samples,
        }
    }

    // ----- CaptureConfig ---------------------------------------------------

    #[test]
    fn config_defaults_match_v04_practice() -> TestResult {
        let c = CaptureConfig::default();
        assert_eq!(c.sample_rate_hz, 16_000);
        assert_eq!(c.channels, 1);
        assert!((c.preroll_seconds - 1.5).abs() < f32::EPSILON);
        assert!(c.device_hint.is_none());
        c.validate()?;
        Ok(())
    }

    #[test]
    fn config_validation_rejects_out_of_bounds() {
        let bad_rate = CaptureConfig {
            sample_rate_hz: 0,
            ..CaptureConfig::default()
        };
        assert!(matches!(
            bad_rate.validate(),
            Err(ConfigError::SampleRateOutOfRange(0))
        ));
        let big_rate = CaptureConfig {
            sample_rate_hz: 400_000,
            ..CaptureConfig::default()
        };
        assert!(bad_rate.validate().is_err());
        assert!(big_rate.validate().is_err());
        let zero_ch = CaptureConfig {
            channels: 0,
            ..CaptureConfig::default()
        };
        assert!(matches!(
            zero_ch.validate(),
            Err(ConfigError::ChannelsOutOfRange(0))
        ));
        for bad_preroll in [-1.0f32, 0.0, f32::NAN, MAX_PREROLL_SECONDS * 2.0] {
            let c = CaptureConfig {
                preroll_seconds: bad_preroll,
                ..CaptureConfig::default()
            };
            assert!(matches!(
                c.validate(),
                Err(ConfigError::PrerollOutOfRange(_))
            ));
        }
    }

    // ----- PreRollRingBuffer -----------------------------------------------

    #[test]
    fn preroll_exact_capacity_and_overwrite_order() -> TestResult {
        // 16 kHz mono, 1.5 s => exactly 24_000 samples.
        let mut buf = PreRollRingBuffer::new(1.5);
        let first: Vec<f32> = (0..24_000).map(|i| i as f32).collect();
        buf.push(&chunk(16_000, 1, first))?;
        assert_eq!(buf.capacity_samples(), Some(24_000));
        assert_eq!(buf.len(), 24_000);
        assert_eq!(buf.dropped_total(), 0);

        buf.push(&chunk(16_000, 1, vec![100_000.0, 100_001.0]))?;
        assert_eq!(buf.len(), 24_000);
        assert_eq!(buf.dropped_total(), 2);
        assert_eq!(buf.pushed_total(), 24_002);

        let mut out = AudioChunk {
            sample_rate_hz: 0,
            channels: 0,
            samples: Vec::new(),
        };
        let n = buf.drain_into(&mut out);
        assert_eq!(n, 24_000);
        assert_eq!(out.sample_rate_hz, 16_000);
        assert_eq!(out.channels, 1);
        // Oldest two samples were dropped; continuity is intact.
        assert_eq!(out.samples[0], 2.0);
        assert_eq!(out.samples[23_998], 100_000.0);
        assert_eq!(out.samples[23_999], 100_001.0);
        Ok(())
    }

    #[test]
    fn preroll_wraparound_continuity_across_many_chunks() -> TestResult {
        // 0.05 s at 16 kHz = 800-sample ring.
        let mut buf = PreRollRingBuffer::from_config(&CaptureConfig {
            preroll_seconds: 0.05,
            ..CaptureConfig::default()
        })?;
        let mut pushed_all: Vec<f32> = Vec::new();
        let chunks = 40;
        for c in 0..chunks {
            let samples: Vec<f32> = (0..160).map(|i| ((c * 160 + i) as f32) * 0.5).collect();
            pushed_all.extend_from_slice(&samples);
            buf.push(&chunk(16_000, 1, samples))?;
        }
        let total = pushed_all.len() as u64;
        assert_eq!(buf.pushed_total(), total);
        assert_eq!(buf.dropped_total(), total - 800);
        let mut got: Vec<f32> = Vec::new();
        buf.peek_into(&mut got);
        assert_eq!(got.len(), 800);
        assert_eq!(got, &pushed_all[pushed_all.len() - 800..]);
        Ok(())
    }

    #[test]
    fn preroll_multichannel_interleaving_preserved_verbatim() -> TestResult {
        // 16 kHz stereo, tiny ring: ceil(0.001 * 16000)=16 frames -> 32 samples.
        let mut buf = PreRollRingBuffer::from_config(&CaptureConfig {
            preroll_seconds: 0.001,
            channels: 2,
            ..CaptureConfig::default()
        })?;
        assert_eq!(buf.capacity_samples(), Some(32));
        let mk = |start: usize| -> Vec<f32> {
            (0..8)
                .flat_map(|i| [(start + i) as f32, -((start + i) as f32)])
                .collect()
        };
        buf.push(&chunk(16_000, 2, mk(0)))?; // frames 0..8
        buf.push(&chunk(16_000, 2, mk(100)))?; // frames 100..108
        let mut out = AudioChunk {
            sample_rate_hz: 0,
            channels: 0,
            samples: vec![],
        };
        buf.drain_into(&mut out);
        assert_eq!(out.samples.len(), 32);
        for f in 0..16usize {
            let l = out.samples[f * 2];
            let r = out.samples[f * 2 + 1];
            if f < 8 {
                assert_eq!((l, r), (f as f32, -(f as f32)));
            } else {
                let i = 100 + (f - 8);
                assert_eq!((l, r), (i as f32, -(i as f32)));
            }
        }
        Ok(())
    }

    #[test]
    fn preroll_layout_resolved_at_first_push_and_changes_rejected() -> TestResult {
        let mut buf = PreRollRingBuffer::new(1.5);
        assert_eq!(buf.sample_rate_hz(), None);
        assert_eq!(buf.capacity_samples(), None);
        buf.push(&chunk(48_000, 2, vec![0.1; 10]))?;
        assert_eq!(buf.sample_rate_hz(), Some(48_000));
        assert_eq!(buf.channels(), Some(2));
        assert_eq!(buf.capacity_samples(), Some(72_000usize * 2));
        let changed_rate = buf.push(&chunk(44_100, 2, vec![0.1; 4]));
        assert!(matches!(
            changed_rate,
            Err(PrerollError::SampleRateChanged {
                expected: 48_000,
                got: 44_100
            })
        ));
        let changed_ch = buf.push(&chunk(48_000, 1, vec![0.1; 4]));
        assert!(matches!(
            changed_ch,
            Err(PrerollError::ChannelMismatch {
                expected: 2,
                got: 1
            })
        ));
        // Empty chunk carries no layout and never errors.
        buf.push(&chunk(1, 32, Vec::new()))?;
        Ok(())
    }

    #[test]
    fn preroll_invalid_capacity_surfaces_typed_error() -> TestResult {
        let mut buf = PreRollRingBuffer::new(0.0);
        let err = expected_error(
            buf.push(&chunk(16_000, 1, vec![0.0; 4])),
            "invalid-capacity push unexpectedly succeeded",
        )?;
        assert!(matches!(err, PrerollError::InvalidCapacity(_)));
        let err_nan = PreRollRingBuffer::from_config(&CaptureConfig {
            preroll_seconds: f32::INFINITY,
            ..CaptureConfig::default()
        });
        assert!(matches!(err_nan, Err(PrerollError::InvalidCapacity(_))));
        Ok(())
    }

    #[test]
    fn preroll_debug_never_contains_sample_content() -> TestResult {
        let mut buf = PreRollRingBuffer::new(1.5);
        buf.push(&chunk(16_000, 1, vec![7.25; 64]))?;
        let dbg = format!("{buf:?}");
        assert!(!dbg.contains("7.25"));
        assert!(!dbg.contains("\"samples\"")); // no sample-content field
        assert!(dbg.contains("PreRollRingBuffer"));
        Ok(())
    }

    #[test]
    fn preroll_clear_and_peek_are_nondestructive_correctly() -> TestResult {
        let mut buf = PreRollRingBuffer::new(1.0);
        buf.push(&chunk(8_000, 1, vec![1.0; 100]))?;
        let mut seen = Vec::new();
        buf.peek_into(&mut seen);
        assert_eq!(seen.len(), 100);
        assert_eq!(buf.len(), 100); // peek does not drain
        buf.clear();
        assert!(buf.is_empty());
        assert_eq!(buf.sample_rate_hz(), Some(8_000)); // layout persists
        Ok(())
    }

    // ----- Mic ranking -------------------------------------------------------

    fn mic(id: &str, name: &str, default: bool, rate: u32, ch: u16) -> MicDevice {
        MicDevice {
            id: id.into(),
            name: name.into(),
            default,
            rate_hz: rate,
            channels: ch,
        }
    }

    #[test]
    fn rank_mics_default_first_then_16k_then_name_asc() {
        let devices = vec![
            mic("d", "Webcam mic", false, 48_000, 2),
            mic("e", "Analog mic", true, 8_000, 1),
            mic("a", "USB mic", false, 16_000, 1),
            mic("b", "USB mic", false, 8_000, 1),
            mic("f", "Unknown-rate mic", false, 0, 1),
        ];
        let ranked = rank_mics(&devices);
        let ids: Vec<&str> = ranked.iter().map(|m| m.id.as_str()).collect();
        // default first ("e"); among non-defaults, 16k-capable {a, d} before
        // incapable {b, f}; byte-wise name asc inside each group; equal names
        // break by id asc (not exercised here).
        assert_eq!(ids, vec!["e", "a", "d", "b", "f"]);
    }

    #[test]
    fn rank_mics_is_deterministic_under_permutation_and_full_ties_break_by_id() {
        let x = mic("x", "Same name", false, 44_100, 1);
        let y = mic("y", "Same name", false, 44_100, 1);
        let z = mic("z", "Same name", false, 8_000, 1);
        let one = rank_mics(&[z.clone(), y.clone(), x.clone()]);
        let two = rank_mics(&[y.clone(), z.clone(), x.clone()]);
        assert_eq!(one, two);
        assert_eq!(one[0].id, "x"); // equal names/rates: id asc
        assert_eq!(one[1].id, "y");
        assert_eq!(one[2].id, "z");
    }

    // ----- pw-monitor line parsing -------------------------------------------

    #[test]
    fn parse_accepts_full_added_line_with_quoted_name() -> TestResult {
        let ev = required(
            parse_pw_monitor_line(
                "added id=pw:42 name=\"Blue Yeti Nano \\\"desk\\\"\" default=true rate=48000 channels=2",
            ),
            "valid added line was rejected",
        )?;
        match ev {
            HotplugEvent::DeviceAdded(m) => {
                assert_eq!(m.id, "pw:42");
                assert_eq!(m.name, "Blue Yeti Nano \"desk\"");
                assert!(m.default);
                assert_eq!(m.rate_hz, 48_000);
                assert_eq!(m.channels, 2);
            }
            other => assert!(
                matches!(other, HotplugEvent::DeviceAdded(_)),
                "expected DeviceAdded, got {other:?}"
            ),
        }
        Ok(())
    }

    #[test]
    fn parse_applies_documented_defaults_and_aliases_case_insensitively() -> TestResult {
        let ev = required(
            parse_pw_monitor_line("ADD id=mic_7 name=yeti"),
            "valid ADD alias was rejected",
        )?;
        assert_eq!(
            ev,
            HotplugEvent::DeviceAdded(MicDevice {
                id: "mic_7".into(),
                name: "yeti".into(),
                default: false,
                rate_hz: 0,
                channels: 1,
            })
        );
        let rm = required(
            parse_pw_monitor_line("Removed   id=mic_7"),
            "valid Removed alias was rejected",
        )?;
        assert_eq!(rm, HotplugEvent::DeviceRemoved("mic_7".into()));
        assert_eq!(
            required(
                parse_pw_monitor_line("remove id=x"),
                "valid remove alias was rejected"
            )?,
            HotplugEvent::DeviceRemoved("x".into())
        );
        Ok(())
    }

    #[test]
    fn parse_rejects_malformed_lines() {
        let rejects = [
            "",
            "added",
            "added name=\"no id\"",
            "added id=a name=b bogus_key=1",
            "added id=a name=b name=c",
            "added id=a name=b rate=-16000",
            "added id=a name=b rate=abc",
            "added id=a name=b default=yes",
            "added id=a name=b channels=0",
            "removed",
            "removed name=no_id",
            "exploded id=a name=b",
            "added id=a name=\"unclosed",
            "added id=a name=b trailing_garbage",
        ];
        for line in rejects {
            assert!(
                parse_pw_monitor_line(line).is_none(),
                "should reject: {line:?}"
            );
        }
    }

    #[test]
    fn hotplug_log_is_bounded_drop_oldest() {
        let mut log = HotplugLog::new(2);
        log.record(HotplugEvent::DeviceRemoved("1".into()));
        log.record(HotplugEvent::DeviceRemoved("2".into()));
        log.record(HotplugEvent::DeviceRemoved("3".into()));
        assert_eq!(log.len(), 2);
        let ids: Vec<&String> = log
            .iter()
            .map(|e| match e {
                HotplugEvent::DeviceRemoved(id) => id,
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(ids, ["2", "3"]);
        log.clear();
        assert!(log.is_empty());
        // Capacity floor of 1 cannot be bypassed into an empty log.
        let mut tiny = HotplugLog::new(0);
        tiny.record(HotplugEvent::DeviceRemoved("only".into()));
        assert_eq!(tiny.len(), 1);
    }

    // ----- Audio policy journal ----------------------------------------------

    struct TempJournalGuard {
        /// Held only to keep the temp directory alive for the test's lifetime.
        _dir: tempfile::TempDir,
    }

    fn temp_journal(name: &str) -> TestResult<(TempJournalGuard, AudioPolicyJournal)> {
        let dir = tempfile::tempdir()?;
        let journal = AudioPolicyJournal::new(dir.path().join(name));
        Ok((TempJournalGuard { _dir: dir }, journal))
    }

    fn duck_entry(sink: &str, vol: f32, at: u64) -> PolicyJournalEntry {
        PolicyJournalEntry {
            action: PolicyAction::Duck,
            target_sink: sink.into(),
            restore_volume: Some(vol),
            at_unix_ms: at,
            owner_pid: std::process::id(),
            applied_at_ms: Some(at),
        }
    }

    #[test]
    fn journal_roundtrips_entries_with_special_characters() -> TestResult {
        let (_g, j) = temp_journal("roundtrip.jsonl")?;
        let entries = [
            duck_entry(
                "alsa_output.pci-0000_0a_00.analog-stereo",
                0.65,
                1_755_860_000_123,
            ),
            PolicyJournalEntry {
                action: PolicyAction::Pause,
                target_sink: "sink with \"quotes\", back\\slash\nnewline\ttab".into(),
                restore_volume: None,
                at_unix_ms: 42,
                owner_pid: 0,
                applied_at_ms: Some(42),
            },
            duck_entry("unicode-\u{00e4}\u{1F600}-sink", 1.0e-3, 7),
        ];
        for e in &entries {
            j.append(e)?;
        }
        assert!(j.exists());
        assert_eq!(j.read_all()?, entries);
        Ok(())
    }

    #[test]
    fn journal_recovers_after_torn_final_line() -> TestResult {
        let (_g, j) = temp_journal("torn.jsonl")?;
        j.append(&duck_entry("sinkA", 0.5, 10))?;
        j.append(&duck_entry("sinkB", 0.25, 20))?;
        // Simulate abrupt death mid-write: partial record without newline.
        {
            let mut f = File::options().append(true).open(j.path())?;
            f.write_all(b"{\"action\":\"du")?;
        }
        let recovered = j.recover_open_actions()?;
        assert_eq!(recovered.len(), 2);
        assert_eq!(recovered[0].target_sink, "sinkA");
        assert_eq!(recovered[1].target_sink, "sinkB");
        Ok(())
    }

    #[test]
    fn journal_flags_complete_corrupt_lines_but_skips_only_the_tail() -> TestResult {
        let (_g, j) = temp_journal("corrupt.jsonl")?;
        j.append(&duck_entry("ok-sink", 0.9, 1))?;
        std::fs::write(
            j.path(),
            b"{\"action\":\"duck\",\"target_sink\":\"ok\",\"restore_volume\":0.9,\"at_unix_ms\":1}\n"
                .iter()
                .copied()
                .chain(b"{\"action\":\"duck\",\"oops\":1}\n".iter().copied())
                .collect::<Vec<u8>>(),
        )
        ?;
        let err = expected_error(j.read_all(), "corrupt journal unexpectedly parsed")?;
        assert!(matches!(err, JournalError::CorruptLine { line_no: 2, .. }));
        // A torn (unterminated) corrupt tail is tolerated instead.
        std::fs::write(
            j.path(),
            b"{\"action\":\"duck\",\"target_sink\":\"ok\",\"restore_vol",
        )?;
        assert_eq!(j.read_all()?.len(), 0);
        Ok(())
    }

    #[test]
    fn journal_collapse_latest_keeps_last_per_action_target_sorted_by_time() {
        let entries = [
            duck_entry("A", 0.5, 30),
            PolicyJournalEntry {
                action: PolicyAction::Pause,
                target_sink: "B".into(),
                restore_volume: None,
                at_unix_ms: 10,
                owner_pid: 0,
                applied_at_ms: Some(10),
            },
            duck_entry("A", 0.7, 20),
            PolicyJournalEntry {
                action: PolicyAction::Pause,
                target_sink: "C".into(),
                restore_volume: None,
                at_unix_ms: 40,
                owner_pid: 0,
                applied_at_ms: Some(40),
            },
        ];
        let collapsed = collapse_latest(&entries);
        let keys: Vec<(u64, String)> = collapsed
            .iter()
            .map(|e| (e.at_unix_ms, e.target_sink.clone()))
            .collect();
        // A keeps the later entry (ts 30); pause B survives; pause C survives;
        // output sorted ascending by time.
        assert_eq!(
            keys,
            vec![(10, "B".into()), (30, "A".into()), (40, "C".into())]
        );
        assert_eq!(collapsed[1].restore_volume, Some(0.5)); // latest duck value
    }

    #[test]
    fn journal_clear_removes_file_and_missing_parent_dir_is_typed_io_error() -> TestResult {
        let dir = tempfile::tempdir()?;
        let nested = AudioPolicyJournal::new(dir.path().join("missing_dir").join("j.jsonl"));
        let entry = duck_entry("s", 0.1, 1);
        assert!(matches!(nested.append(&entry), Err(JournalError::Io(_))));
        assert!(nested.clear().is_ok()); // clearing a nonexistent journal is fine

        let existing = AudioPolicyJournal::new(dir.path().join("j.jsonl"));
        existing.append(&entry)?;
        assert!(existing.exists());
        existing.clear()?;
        assert!(!existing.exists());
        existing.clear()?; // idempotent
        Ok(())
    }

    #[test]
    fn journal_rejects_non_finite_volumes_up_front() -> TestResult {
        let (_g, j) = temp_journal("nan.jsonl")?;
        let err = expected_error(
            j.append(&duck_entry("s", f32::NAN, 1)),
            "non-finite volume unexpectedly appended",
        )?;
        assert!(matches!(err, JournalError::InvalidVolume(_)));
        Ok(())
    }

    #[test]
    fn journal_empty_or_absent_file_reads_as_no_actions() -> TestResult {
        let dir = tempfile::tempdir()?;
        let absent = AudioPolicyJournal::new(dir.path().join("absent.jsonl"));
        assert!(absent.recover_open_actions()?.is_empty());

        let empty_path = dir.path().join("empty.jsonl");
        std::fs::write(&empty_path, b"")?;
        let empty = AudioPolicyJournal::new(empty_path);
        assert!(empty.read_all()?.is_empty());
        Ok(())
    }

    // ----- Capture stages ------------------------------------------------------

    struct CountingStage {
        calls: u32,
        stop_on_call: Option<StageStop>,
    }

    impl CountingStage {
        fn passthrough() -> Self {
            Self {
                calls: 0,
                stop_on_call: None,
            }
        }
    }

    impl CaptureStage for CountingStage {
        fn process(&mut self, _chunk: &AudioChunk) -> ControlFlow<StageStop> {
            self.calls += 1;
            match self.stop_on_call.clone() {
                Some(stop) => ControlFlow::Break(stop),
                None => ControlFlow::Continue(()),
            }
        }
    }

    #[test]
    fn stage_chain_feeds_every_stage_and_short_circuits_on_fault() {
        let mut first = CountingStage::passthrough();
        let mut second = CountingStage {
            calls: 0,
            stop_on_call: Some(StageStop::Fault("unit-fault")),
        };
        let mut third = CountingStage::passthrough();
        let c = chunk(16_000, 1, vec![0.0]);
        let out = run_capture_stages(&mut [&mut first, &mut second, &mut third], &c);
        assert_eq!(out, ControlFlow::Break(StageStop::Fault("unit-fault")));
        assert_eq!((first.calls, second.calls, third.calls), (1, 1, 0));

        let mut a = CountingStage::passthrough();
        let mut b = CountingStage::passthrough();
        let ok = run_capture_stages(&mut [&mut a, &mut b], &c);
        assert_eq!(ok, ControlFlow::Continue(()));
    }

    #[test]
    fn preroll_stage_sinks_chunks_and_maps_errors_to_reason_codes() {
        let mut stage = PrerollStage::new(PreRollRingBuffer::new(1.5));
        let c = chunk(16_000, 1, vec![0.5; 320]);
        assert_eq!(
            run_capture_stages(&mut [&mut stage], &c),
            ControlFlow::Continue(())
        );
        assert_eq!(stage.buffer().len(), 320);

        let wrong_rate = chunk(44_100, 1, vec![0.5; 8]);
        assert_eq!(
            run_capture_stages(&mut [&mut stage], &wrong_rate),
            ControlFlow::Break(StageStop::Fault("preroll-rate-changed"))
        );
        let wrong_ch = chunk(16_000, 2, vec![0.5; 8]);
        assert_eq!(
            run_capture_stages(&mut [&mut stage], &wrong_ch),
            ControlFlow::Break(StageStop::Fault("preroll-channel-mismatch"))
        );
        let mut drained_to = stage.into_inner();
        let mut out = AudioChunk {
            sample_rate_hz: 0,
            channels: 0,
            samples: vec![],
        };
        assert_eq!(drained_to.drain_into(&mut out), 320);
    }

    #[test]
    fn backpressure_stop_passes_through_chain_boundary() {
        let mut s = CountingStage {
            calls: 0,
            stop_on_call: Some(StageStop::Backpressured),
        };
        let c = chunk(16_000, 1, vec![0.0]);
        let out = run_capture_stages(&mut [&mut s], &c);
        assert_eq!(out, ControlFlow::Break(StageStop::Backpressured));
    }

    #[test]
    fn journal_applied_at_ms_roundtrips_and_backward_compatible() -> TestResult {
        let (_g, j) = temp_journal("applied_at.jsonl")?;
        let entry = PolicyJournalEntry {
            action: PolicyAction::Duck,
            target_sink: "sinkA".into(),
            restore_volume: Some(0.5),
            at_unix_ms: 100,
            owner_pid: 123,
            applied_at_ms: Some(101),
        };
        j.append(&entry)?;
        let read = j.read_all()?;
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].applied_at_ms, Some(101));
        // Backward compat: old line without applied_at_ms must parse as None
        let old_line = "{\"action\":\"duck\",\"target_sink\":\"sinkB\",\"restore_volume\":0.5,\"at_unix_ms\":200,\"owner_pid\":456}";
        // Write old line directly and verify it reads as None
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("old.jsonl");
        std::fs::write(&path, format!("{old_line}\n"))?;
        let old_journal = AudioPolicyJournal::new(path);
        let old_read = old_journal.read_all()?;
        assert_eq!(old_read[0].applied_at_ms, None);
        Ok(())
    }

    #[test]
    fn journal_verify_restoration_idempotent_reports_true_when_empty_and_after_clear() -> TestResult
    {
        let (_g, j) = temp_journal("verify.jsonl")?;
        // Empty journal is trivially idempotent
        assert!(j.verify_restoration_idempotent()?);
        j.append(&duck_entry("sinkX", 0.7, 10))?;
        j.append(&duck_entry("sinkY", 0.3, 20))?;
        assert_eq!(j.read_all()?.len(), 2);
        assert!(j.verify_restoration_idempotent()?);
        assert!(j.read_all()?.is_empty());
        // Second verification on already-empty stays true
        assert!(j.verify_restoration_idempotent()?);
        Ok(())
    }

    #[test]
    fn is_virtual_detection_covers_test_and_remap_names() {
        let v1 = MicDevice {
            id: "1".into(),
            name: "narrivy_test_mic".into(),
            default: false,
            rate_hz: 48000,
            channels: 1,
        };
        let v2 = MicDevice {
            id: "2".into(),
            name: "remap-source virtual".into(),
            default: false,
            rate_hz: 16000,
            channels: 1,
        };
        let real = MicDevice {
            id: "3".into(),
            name: "Blue Yeti".into(),
            default: false,
            rate_hz: 48000,
            channels: 1,
        };
        assert!(v1.is_virtual());
        assert!(v2.is_virtual());
        assert!(!real.is_virtual());
    }
}
