//! P4 audio policy: default-sink volume snapshot / duck / restore via `wpctl`.
//!
//! Contract (REL-005, REL-009):
//! * every mutation follows snapshot -> mutate -> restore;
//! * every mutation is journaled through [`AudioPolicyJournal`] **after** it
//!   succeeds, with the owning PID recorded, so abrupt process death leaves
//!   exactly the unrestored set on disk;
//! * recovery restores open duck entries whose owner PID is gone, then
//!   clears the journal;
//! * ordinary logs never contain anything beyond volume numbers and stable
//!   identifiers (privacy rule).
//!
//! All external invocations go through the mockable [`WpctlRunner`] port so
//! routing logic is unit-testable without spawning anything real (same
//! pattern as `delivery::command::CommandRunner`).

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::capture::{AudioPolicyJournal, PolicyAction, PolicyJournalEntry, collapse_latest};

/// wpctl alias for the system default audio sink.
pub const DEFAULT_SINK: &str = "@DEFAULT_AUDIO_SINK@";
/// Duck factor: active session audio drops the default sink to this fraction
/// of its prior linear volume.
pub const DEFAULT_DUCK_FACTOR: f32 = 0.4;
/// Bounded wall-clock budget for one wpctl invocation.
const WPCTL_TIMEOUT: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Typed errors; no panics on production paths.
#[derive(Debug)]
pub enum AudioPolicyError {
    /// wpctl invocation failed or produced no usable output.
    CommandFailed(String),
    /// Output could not be parsed into a volume value.
    Parse(String),
    /// Journal open/read/write failure.
    Journal(crate::capture::JournalError),
    /// wpctl exceeded its wall-clock bound and was killed.
    Timeout,
}

impl std::fmt::Display for AudioPolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AudioPolicyError::CommandFailed(s) => write!(f, "wpctl command failed: {s}"),
            AudioPolicyError::Parse(s) => write!(f, "cannot parse wpctl output: {s}"),
            AudioPolicyError::Journal(e) => write!(f, "policy journal error: {e}"),
            AudioPolicyError::Timeout => write!(f, "wpctl timed out after {WPCTL_TIMEOUT:?}"),
        }
    }
}

impl std::error::Error for AudioPolicyError {}

impl From<crate::capture::JournalError> for AudioPolicyError {
    fn from(e: crate::capture::JournalError) -> Self {
        AudioPolicyError::Journal(e)
    }
}

// ---------------------------------------------------------------------------
// Runner port + std implementation
// ---------------------------------------------------------------------------

/// External-process port so policy decisions are unit-testable without
/// spawning real processes. Implementations must never panic.
pub trait WpctlRunner {
    /// Raw stdout of `wpctl get-volume @DEFAULT_AUDIO_SINK@`.
    fn get_volume(&self) -> Result<String, AudioPolicyError>;
    /// `wpctl set-volume @DEFAULT_AUDIO_SINK@ <v>`.
    fn set_volume(&self, volume: f32) -> Result<(), AudioPolicyError>;
    /// `wpctl set-mute @DEFAULT_AUDIO_SINK@ 1|0`.
    fn set_mute(&self, muted: bool) -> Result<(), AudioPolicyError>;
}

fn run_bounded(cmd: &mut Command) -> Result<(bool, String), AudioPolicyError> {
    let mut child = cmd
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| {
            AudioPolicyError::CommandFailed(format!(
                "spawn {}: {e}",
                cmd.get_program().to_string_lossy()
            ))
        })?;
    // Bounded wait: poll try_wait, kill on overrun (no libc; std-only).
    let deadline = Instant::now() + WPCTL_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut out = String::new();
                if let Some(mut io) = child.stdout.take() {
                    use std::io::Read;
                    let _ = io.read_to_string(&mut out);
                }
                return Ok((status.success(), out));
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(AudioPolicyError::Timeout);
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(e) => return Err(AudioPolicyError::CommandFailed(format!("wait: {e}"))),
        }
    }
}

/// Production runner invoking the real `wpctl` binary.
pub struct StdWpctlRunner {
    program: PathBuf,
}

impl Default for StdWpctlRunner {
    fn default() -> Self {
        Self {
            program: PathBuf::from("wpctl"),
        }
    }
}

impl WpctlRunner for StdWpctlRunner {
    fn get_volume(&self) -> Result<String, AudioPolicyError> {
        let mut cmd = Command::new(&self.program);
        cmd.args(["get-volume", DEFAULT_SINK]);
        let (ok, out) = run_bounded(&mut cmd)?;
        if !ok || out.trim().is_empty() {
            return Err(AudioPolicyError::CommandFailed(format!(
                "wpctl get-volume exited unsuccessfully (output: {} bytes)",
                out.len()
            )));
        }
        Ok(out)
    }

    fn set_volume(&self, volume: f32) -> Result<(), AudioPolicyError> {
        if !volume.is_finite() || volume < 0.0 {
            return Err(AudioPolicyError::Parse(format!(
                "refusing to set non-finite/negative volume {volume}"
            )));
        }
        let mut cmd = Command::new(&self.program);
        cmd.args(["set-volume", DEFAULT_SINK, &format!("{volume}")]);
        let (ok, _out) = run_bounded(&mut cmd)?;
        if !ok {
            return Err(AudioPolicyError::CommandFailed(
                "wpctl set-volume exited unsuccessfully".to_string(),
            ));
        }
        Ok(())
    }

    fn set_mute(&self, muted: bool) -> Result<(), AudioPolicyError> {
        let mut cmd = Command::new(&self.program);
        cmd.args(["set-mute", DEFAULT_SINK, if muted { "1" } else { "0" }]);
        let (ok, _out) = run_bounded(&mut cmd)?;
        if !ok {
            return Err(AudioPolicyError::CommandFailed(
                "wpctl set-mute exited unsuccessfully".to_string(),
            ));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Snapshot parsing
// ---------------------------------------------------------------------------

/// Snapshot of the default sink volume as reported by `wpctl get-volume`.
#[derive(Debug, Clone, PartialEq)]
pub struct VolumeSnapshot {
    /// Raw line from wpctl, e.g. `Volume: 0.65 [MUTED]`.
    pub raw: String,
    /// Parsed linear volume (0.0 ..= device maximum).
    pub volume: f32,
    /// True when wpctl reports `[MUTED]`.
    pub muted: bool,
}

/// Parse `Volume: <float>[ [MUTED]]` output. Pure; unit-tested offline.
pub fn parse_wpctl_volume(output: &str) -> Result<VolumeSnapshot, AudioPolicyError> {
    let line = output
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .ok_or_else(|| AudioPolicyError::Parse("empty wpctl output".to_string()))?;
    let rest = line
        .strip_prefix("Volume:")
        .ok_or_else(|| AudioPolicyError::Parse(format!("unexpected wpctl line: {line:?}")))?
        .trim();
    let (value_part, muted) = match rest.split_once('[') {
        Some((before, after)) => {
            let tag = after.trim_end_matches(']').trim();
            if tag != "MUTED" {
                return Err(AudioPolicyError::Parse(format!("unknown tag [{tag}]")));
            }
            (before.trim(), true)
        }
        None => (rest, false),
    };
    let volume: f32 = value_part
        .parse()
        .map_err(|_| AudioPolicyError::Parse(format!("malformed volume {value_part:?}")))?;
    if !volume.is_finite() || volume < 0.0 {
        return Err(AudioPolicyError::Parse(format!(
            "implausible volume {volume}"
        )));
    }
    Ok(VolumeSnapshot {
        raw: line.to_string(),
        volume,
        muted,
    })
}

/// Run a live snapshot through any runner.
pub fn snapshot_default_sink_volume(
    runner: &dyn WpctlRunner,
) -> Result<VolumeSnapshot, AudioPolicyError> {
    parse_wpctl_volume(&runner.get_volume()?)
}

// ---------------------------------------------------------------------------
// Mutations (each: snapshot -> mutate; restoration data returned/journaled)
// ---------------------------------------------------------------------------

/// Duck the default sink to `factor * current`, returning the prior snapshot.
pub fn duck(runner: &dyn WpctlRunner, factor: f32) -> Result<VolumeSnapshot, AudioPolicyError> {
    let prior = snapshot_default_sink_volume(runner)?;
    let target = (prior.volume * factor).max(0.0);
    runner.set_volume(target)?;
    Ok(prior)
}

/// Restore an exact prior snapshot (volume + mute state).
pub fn restore(
    runner: &dyn WpctlRunner,
    snapshot: &VolumeSnapshot,
) -> Result<(), AudioPolicyError> {
    runner.set_volume(snapshot.volume)?;
    runner.set_mute(snapshot.muted)
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Ownership / recovery (REL-009)
// ---------------------------------------------------------------------------

/// True when the process `pid` still exists and is not a zombie (a zombie has
/// already died and only awaits reaping — its mutations are orphaned).
///
/// Linux-only by design: this crate is the Linux platform adapter.
pub fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let stat = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(s) => s,
        Err(_) => return false,
    };
    // State char sits right after the comm field's closing paren.
    match stat.rsplit_once(')') {
        Some((_, rest)) => {
            let rest = rest.trim_start();
            rest.chars().next().map(|c| c != 'Z').unwrap_or(false)
        }
        None => false,
    }
}

/// Entries whose owning process is gone (pid dead, unknown, or self-reaped).
pub fn orphaned_entries(entries: &[PolicyJournalEntry]) -> Vec<PolicyJournalEntry> {
    let me = std::process::id();
    entries
        .iter()
        .filter(|e| e.owner_pid != me && !process_alive(e.owner_pid))
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------
// Policy engine
// ---------------------------------------------------------------------------

/// Policy selected by the session owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyEngine {
    /// Do not touch user volume at all.
    Leave,
    /// Duck default sink volume to [`DEFAULT_DUCK_FACTOR`] while active.
    Duck,
}

/// Outcome of applying a policy at session start.
#[derive(Debug, Clone, PartialEq)]
pub struct AppliedPolicy {
    pub engine: PolicyEngine,
    /// Prior snapshot when a mutation was applied (`None` for Leave).
    pub prior: Option<VolumeSnapshot>,
}

impl PolicyEngine {
    /// Apply the policy at session start and journal any mutation.
    ///
    /// Journal protocol matches `capture.rs`: mutate FIRST, then append the
    /// entry, so a crash between the two leaves the user volume mutated but
    /// recoverable — never journaled-without-effect.
    pub fn apply(
        self,
        runner: &dyn WpctlRunner,
        journal: &AudioPolicyJournal,
    ) -> Result<AppliedPolicy, AudioPolicyError> {
        match self {
            PolicyEngine::Leave => Ok(AppliedPolicy {
                engine: self,
                prior: None,
            }),
            PolicyEngine::Duck => {
                let prior = duck(runner, DEFAULT_DUCK_FACTOR)?;
                let now = now_unix_ms();
                let entry = PolicyJournalEntry {
                    action: PolicyAction::Duck,
                    target_sink: DEFAULT_SINK.to_string(),
                    restore_volume: Some(prior.volume),
                    at_unix_ms: now,
                    owner_pid: std::process::id(),
                    applied_at_ms: Some(now),
                };
                journal.append(&entry)?;
                Ok(AppliedPolicy {
                    engine: self,
                    prior: Some(prior),
                })
            }
        }
    }

    /// Clean-exit restoration (REL-005): undo every open entry owned by this
    /// process, then clear the journal. Returns restored entry count.
    pub fn restore_own(
        self,
        runner: &dyn WpctlRunner,
        journal: &AudioPolicyJournal,
    ) -> Result<usize, AudioPolicyError> {
        let me = std::process::id();
        let entries = journal.recover_open_actions()?;
        let owned: Vec<PolicyJournalEntry> = entries
            .iter()
            .filter(|e| e.owner_pid == me)
            .cloned()
            .collect();
        if owned.is_empty() {
            return Ok(0);
        }
        // Latest-wins per (action, target): restore the collapsed view.
        let mut restored = 0usize;
        for entry in collapse_latest(&owned) {
            if let Some(v) = entry.restore_volume {
                runner.set_volume(v)?;
                restored += 1;
            }
        }
        journal.clear()?;
        Ok(restored)
    }

    /// Verify that calling `restore_own` twice is idempotent — the second
    /// call is a no-op and returns 0. This is the REL-005 double-restore
    /// guard: after a clean exit has cleared the journal, a stray second
    /// restoration must not re-apply volume changes.
    pub fn verify_restoration_idempotent(
        self,
        runner: &dyn WpctlRunner,
        journal: &AudioPolicyJournal,
    ) -> Result<bool, AudioPolicyError> {
        let _first = self.restore_own(runner, journal)?;
        let second = self.restore_own(runner, journal)?;
        // Second must be a no-op regardless of whether first did work.
        Ok(second == 0)
    }

    /// Crash recovery (REL-009): scan the journal for open entries whose
    /// owner PID is dead and restore them. Returns how many entries were
    /// restored. The journal is cleared only when nothing live remains.
    pub fn restore_dead_owner_entries(
        runner: &dyn WpctlRunner,
        journal: &AudioPolicyJournal,
    ) -> Result<usize, AudioPolicyError> {
        let entries = journal.recover_open_actions()?;
        let orphans = orphaned_entries(&entries);
        if orphans.is_empty() {
            return Ok(0);
        }
        let mut restored = 0usize;
        for entry in collapse_latest(&orphans) {
            if let Some(v) = entry.restore_volume {
                runner.set_volume(v)?;
                restored += 1;
            }
        }
        // Anything left belongs to a living process; keep it journalled.
        let remaining_live = entries
            .iter()
            .any(|e| e.owner_pid != 0 && process_alive(e.owner_pid));
        if !remaining_live {
            journal.clear()?;
        }
        Ok(restored)
    }
}

// ---------------------------------------------------------------------------
// Tests: mocked runner, no real wpctl spawned
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct MockRunner {
        state: RefCell<MockState>,
        fail_get: bool,
    }

    #[derive(Default)]
    struct MockState {
        volume: f32,
        muted: bool,
        sets: Vec<(f32, bool)>,
    }

    impl MockRunner {
        fn new(volume: f32) -> Self {
            Self {
                state: RefCell::new(MockState {
                    volume,
                    ..Default::default()
                }),
                fail_get: false,
            }
        }
    }

    impl WpctlRunner for MockRunner {
        fn get_volume(&self) -> Result<String, AudioPolicyError> {
            if self.fail_get {
                return Err(AudioPolicyError::CommandFailed("mock failure".into()));
            }
            let s = self.state.borrow();
            let tag = if s.muted { " [MUTED]" } else { "" };
            Ok(format!("Volume: {}{tag}", s.volume))
        }

        fn set_volume(&self, volume: f32) -> Result<(), AudioPolicyError> {
            let mut s = self.state.borrow_mut();
            let muted = s.muted;
            s.sets.push((volume, muted));
            s.volume = volume;
            Ok(())
        }

        fn set_mute(&self, muted: bool) -> Result<(), AudioPolicyError> {
            let mut s = self.state.borrow_mut();
            let volume = s.volume;
            s.sets.push((volume, muted));
            s.muted = muted;
            Ok(())
        }
    }

    fn temp_journal(
        name: &str,
    ) -> Result<(tempdir::TempDirGuard, AudioPolicyJournal), std::io::Error> {
        tempdir::temp_journal(name)
    }

    // Tiny scoped tempdir helper local to this module's tests.
    mod tempdir {
        use super::*;
        use std::path::PathBuf;

        pub struct TempDirGuard(PathBuf);
        impl Drop for TempDirGuard {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }

        pub fn temp_journal(
            name: &str,
        ) -> Result<(TempDirGuard, AudioPolicyJournal), std::io::Error> {
            let dir = std::env::temp_dir().join(format!(
                "narrivy-p4-policy-test-{}-{}",
                std::process::id(),
                name
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir)?;
            Ok((
                TempDirGuard(dir.clone()),
                AudioPolicyJournal::new(dir.join(name)),
            ))
        }
    }

    #[test]
    fn parses_plain_and_muted_wpctl_lines() -> Result<(), Box<dyn std::error::Error>> {
        let s = parse_wpctl_volume("Volume: 0.70\n")?;
        assert_eq!(s.volume, 0.70);
        assert!(!s.muted);
        let m = parse_wpctl_volume("Volume: 0.42 [MUTED]\n")?;
        assert_eq!(m.volume, 0.42);
        assert!(m.muted);
        assert!(parse_wpctl_volume("").is_err());
        assert!(parse_wpctl_volume("Garbage: 1.0").is_err());
        assert!(parse_wpctl_volume("Volume: banana").is_err());
        assert!(parse_wpctl_volume("Volume: -1.0").is_err());
        Ok(())
    }

    #[test]
    fn duck_lowers_by_factor_and_returns_prior() -> Result<(), Box<dyn std::error::Error>> {
        let r = MockRunner::new(0.8);
        let prior = duck(&r, DEFAULT_DUCK_FACTOR)?;
        assert_eq!(prior.volume, 0.8);
        assert_eq!(r.state.borrow().volume, 0.8 * DEFAULT_DUCK_FACTOR);
        Ok(())
    }

    #[test]
    fn restore_recovers_exact_volume_and_mute_state() -> Result<(), Box<dyn std::error::Error>> {
        let r = MockRunner::new(0.5);
        r.state.borrow_mut().muted = true;
        let snap = snapshot_default_sink_volume(&r)?;
        r.state.borrow_mut().muted = false;
        restore(&r, &snap)?;
        let s = r.state.borrow();
        assert_eq!(s.volume, 0.5);
        assert!(s.muted);
        Ok(())
    }

    #[test]
    fn leave_never_touches_volume_or_journal() -> Result<(), Box<dyn std::error::Error>> {
        let r = MockRunner::new(0.7);
        let (_g, j) = temp_journal("leave.jsonl")?;
        let applied = PolicyEngine::Leave.apply(&r, &j)?;
        assert!(applied.prior.is_none());
        assert!(r.state.borrow().sets.is_empty());
        assert!(!j.exists());
        Ok(())
    }

    #[test]
    fn duck_writes_journal_entry_owned_by_self_then_clean_restore_clears_it()
    -> Result<(), Box<dyn std::error::Error>> {
        let r = MockRunner::new(0.6);
        let (_g, j) = temp_journal("duck.jsonl")?;
        let applied = PolicyEngine::Duck.apply(&r, &j)?;
        let prior = applied
            .prior
            .ok_or_else(|| std::io::Error::other("duck must record prior state"))?;
        assert_eq!(prior.volume, 0.6);
        assert_eq!(r.state.borrow().volume, 0.6 * DEFAULT_DUCK_FACTOR);
        let entries = j.read_all()?;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].owner_pid, std::process::id());
        assert_eq!(entries[0].restore_volume, Some(0.6));

        // Clean exit restores and clears.
        let n = PolicyEngine::Duck.restore_own(&r, &j)?;
        assert_eq!(n, 1);
        assert_eq!(r.state.borrow().volume, 0.6);
        assert!(!j.exists());
        Ok(())
    }

    #[test]
    fn recovery_restores_only_dead_owner_entries() -> Result<(), Box<dyn std::error::Error>> {
        let r = MockRunner::new(0.99);
        let (_g, j) = temp_journal("recovery.jsonl")?;
        // Orphaned entry (dead pid 999999999) plus a live-owner entry (self).
        j.append(&PolicyJournalEntry {
            action: PolicyAction::Duck,
            target_sink: DEFAULT_SINK.into(),
            restore_volume: Some(0.75),
            at_unix_ms: 1,
            owner_pid: 999_999_999,
            applied_at_ms: Some(1),
        })?;
        j.append(&PolicyJournalEntry {
            action: PolicyAction::Duck,
            target_sink: DEFAULT_SINK.into(),
            restore_volume: Some(0.5),
            at_unix_ms: 2,
            owner_pid: std::process::id(),
            applied_at_ms: Some(2),
        })?;

        let n = PolicyEngine::restore_dead_owner_entries(&r, &j)?;
        assert_eq!(n, 1, "only the dead-owner entry is restored");
        assert_eq!(r.state.borrow().volume, 0.75);
        // Journal stays: the live-owner entry must keep waiting for its
        // owner's own clean-exit restoration. (Individual entries are never
        // deleted; the journal is only cleared wholesale.)
        assert!(j.exists());
        assert_eq!(j.read_all()?.len(), 2);
        Ok(())
    }

    #[test]
    fn recovery_treats_unknown_owner_as_orphaned_and_clears_when_done()
    -> Result<(), Box<dyn std::error::Error>> {
        let r = MockRunner::new(0.9);
        let (_g, j) = temp_journal("unknown-owner.jsonl")?;
        j.append(&PolicyJournalEntry {
            action: PolicyAction::Duck,
            target_sink: DEFAULT_SINK.into(),
            restore_volume: Some(0.3),
            at_unix_ms: 1,
            owner_pid: 0,
            applied_at_ms: Some(1),
        })?;
        let n = PolicyEngine::restore_dead_owner_entries(&r, &j)?;
        assert_eq!(n, 1);
        assert_eq!(r.state.borrow().volume, 0.3);
        assert!(!j.exists(), "journal cleared when no live owners remain");
        Ok(())
    }

    #[test]
    fn zombie_and_missing_processes_count_as_dead() {
        assert!(!process_alive(0));
        assert!(!process_alive(u32::MAX - 1)); // practically never exists
        assert!(process_alive(std::process::id())); // we exist
    }

    #[test]
    fn verify_restoration_idempotent_second_call_is_noop() -> Result<(), Box<dyn std::error::Error>>
    {
        let r = MockRunner::new(0.8);
        let (_g, j) = temp_journal("idempotent.jsonl")?;
        let applied = PolicyEngine::Duck.apply(&r, &j)?;
        assert!(applied.prior.is_some());
        let vol_after_duck = r.state.borrow().volume;
        assert!((vol_after_duck - 0.8 * DEFAULT_DUCK_FACTOR).abs() < 1e-6);
        // First restore does work
        let first = PolicyEngine::Duck.restore_own(&r, &j)?;
        assert_eq!(first, 1);
        assert!((r.state.borrow().volume - 0.8).abs() < 1e-6);
        assert!(!j.exists());
        // Second restore is no-op
        let second = PolicyEngine::Duck.restore_own(&r, &j)?;
        assert_eq!(second, 0);
        // Verify helper confirms idempotency
        let (_g2, j2) = temp_journal("idempotent2.jsonl")?;
        PolicyEngine::Duck.apply(&r, &j2)?;
        assert!(PolicyEngine::Duck.verify_restoration_idempotent(&r, &j2)?);
        // Second call again stays 0
        assert_eq!(PolicyEngine::Duck.restore_own(&r, &j2)?, 0);
        Ok(())
    }

    #[test]
    fn applied_at_ms_recorded_on_duck() -> Result<(), Box<dyn std::error::Error>> {
        let r = MockRunner::new(0.6);
        let (_g, j) = temp_journal("applied_at_ms.jsonl")?;
        PolicyEngine::Duck.apply(&r, &j)?;
        let entries = j.read_all()?;
        assert_eq!(entries.len(), 1);
        assert!(entries[0].applied_at_ms.is_some());
        // Should be close to at_unix_ms (same now)
        let applied_at_ms = entries[0]
            .applied_at_ms
            .ok_or_else(|| std::io::Error::other("duck must record application time"))?;
        let diff = (applied_at_ms as i64 - entries[0].at_unix_ms as i64).abs();
        assert!(
            diff < 1000,
            "applied_at_ms should be near at_unix_ms, diff={diff}"
        );
        Ok(())
    }
}
