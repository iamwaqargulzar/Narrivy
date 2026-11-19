//! Text delivery for Windows: a bounded ladder of the two direct routes,
//! then the durable recovery Inbox, reported in the same shape as the Linux
//! routed delivery (`routed-then-inbox`) so the portable recovery tail
//! accepts it unchanged.
//!
//! Routes, in the order the user's delivery setting picks (`type` or
//! `paste`, the same two values the Linux side stores):
//!
//! - **typing**: `SendInput` with `KEYEVENTF_UNICODE` into the focused
//!   window (see [`crate::text_input`]);
//! - **clipboard paste**: put the text on the clipboard, send Ctrl+V, then
//!   put the user's previous clipboard back if it is still ours.
//!
//! Neither route can observe that the target kept the text, so a dispatch is
//! [`DeliveryOutcome::SubmittedUnverified`] and is always followed by one
//! atomic Inbox write under the same transaction id, exactly as on Linux. A
//! route that sent nothing (blocked by a higher-integrity window, modifiers
//! still held, a control character the typing route refuses) lets the ladder
//! try the other route; one that may have sent part of the text ends the
//! ladder, because a second route could type it twice.
//!
//! A bounded replay record keeps, per transaction, a SHA-256 digest and the
//! byte length of the text with the report: a stale retry gets the same
//! report without typing again, and a reused id with different text is
//! refused.
//!
//! # Privacy
//! Text and clipboard contents are routing inputs only. They are never
//! logged, never in `Debug` output, and never in error codes.

#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use narrivy_platform_api::{DeliveryPort, PlatformError};
use narrivy_types::{
    DeliveryAttempt, DeliveryIntent, DeliveryOutcome, DeliveryReport, FallbackReason,
    SessionCancellation, TransactionId,
};
use sha2::{Digest, Sha256};

use crate::error;
use crate::text_input::{KeyStroke, MODIFIER_KEYS, SEND_BATCH, key_strokes, paste_chord};

/// Identity of the routed report, shared with Linux.
pub const ROUTED_THEN_INBOX_BACKEND: &str = "routed-then-inbox";
/// Durable recovery rung.
pub const INBOX_BACKEND: &str = "inbox";
/// Attempt identities of the two Windows routes.
pub const TYPING_BACKEND: &str = "windows-sendinput";
pub const CLIPBOARD_PASTE_BACKEND: &str = "windows-clipboard-paste";

/// Bound for one typing dispatch, whatever the text length.
pub const TYPING_BOUND: Duration = Duration::from_secs(10);
/// How long typing waits for the user to release modifier keys.
pub const MODIFIER_RELEASE_BOUND: Duration = Duration::from_millis(1_500);
/// Pause between `SendInput` batches so the target keeps up.
pub const BATCH_PAUSE: Duration = Duration::from_millis(2);
/// How long the pasted text stays on the clipboard before the previous
/// contents come back (applications read the clipboard a beat after the key,
/// some more than once). Same as the Linux minimum hold.
pub const PASTE_HOLD: Duration = Duration::from_millis(500);

/// The user's delivery setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryMethod {
    Type,
    Paste,
}

impl DeliveryMethod {
    /// The stored setting: `type` or `paste`.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "type" => Some(Self::Type),
            "paste" => Some(Self::Paste),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Type => "type",
            Self::Paste => "paste",
        }
    }
}

/// Fact observed by one route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RungOutcome {
    /// Sent in full; the target's retention is not observable.
    SubmittedUnverified,
    /// The text is on the clipboard for the user to paste; nothing was typed.
    CopiedOnly,
    /// Failure after part of the payload may have reached a window. Ends the
    /// ladder.
    Failed(FallbackReason),
    /// Failure before anything was sent. The ladder continues.
    NotDispatched(FallbackReason),
    /// Cancelled before anything was sent.
    CancelledBeforeDispatch,
}

/// What happened to the user's clipboard after a paste.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardRestoreOutcome {
    /// The previous contents are back.
    RestoredPrior,
    /// Something else replaced our text meanwhile (the user copied); it is
    /// kept.
    PreservedNewerContent,
    /// Restoring failed; the content-free reason is given.
    Failed(&'static str),
}

/// One direct route.
pub trait DeliveryRung: Send {
    fn id(&self) -> &'static str;
    fn attempt(&mut self, text: &str, cancellation: &SessionCancellation) -> RungOutcome;
    fn last_restore_outcome(&self) -> Option<ClipboardRestoreOutcome> {
        None
    }
}

/// The durable recovery sink.
pub trait RecoverySink: Send {
    /// Store `text` under `transaction_id` unless cancellation is observed
    /// before the commit.
    ///
    /// # Errors
    /// A content-free code.
    fn store(
        &mut self,
        transaction_id: &TransactionId,
        text: &str,
        cancellation: &SessionCancellation,
    ) -> Result<(), &'static str>;
}

fn backend_error(code: &'static str) -> FallbackReason {
    FallbackReason::BackendError {
        code: code.to_owned(),
    }
}

// ---------------------------------------------------------------------------
// Typing
// ---------------------------------------------------------------------------

/// The keyboard as the typing route needs it.
pub trait KeyboardPort: Send {
    /// Send these events; returns how many the system accepted (`SendInput`'s
    /// return value). Fewer than given means input is blocked, typically by a
    /// window running at a higher integrity level.
    fn send(&mut self, strokes: &[KeyStroke]) -> usize;
    /// Is this virtual key physically down?
    fn is_down(&mut self, vk: u16) -> bool;
}

/// Typing route over a [`KeyboardPort`].
pub struct TypingRung<K: KeyboardPort> {
    keyboard: K,
    batch_pause: Duration,
}

impl<K: KeyboardPort> TypingRung<K> {
    pub fn new(keyboard: K) -> Self {
        Self {
            keyboard,
            batch_pause: BATCH_PAUSE,
        }
    }

    #[cfg(test)]
    fn without_pause(keyboard: K) -> Self {
        Self {
            keyboard,
            batch_pause: Duration::ZERO,
        }
    }

    /// Wait (bounded) until no modifier key is down.
    fn modifiers_released(
        &mut self,
        cancellation: &SessionCancellation,
    ) -> Result<(), RungOutcome> {
        let deadline = Instant::now() + MODIFIER_RELEASE_BOUND;
        loop {
            if cancellation.is_cancelled() {
                return Err(RungOutcome::CancelledBeforeDispatch);
            }
            if !MODIFIER_KEYS.iter().any(|vk| self.keyboard.is_down(*vk)) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(RungOutcome::NotDispatched(backend_error(
                    "delivery_modifiers_held",
                )));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl<K: KeyboardPort> DeliveryRung for TypingRung<K> {
    fn id(&self) -> &'static str {
        TYPING_BACKEND
    }

    fn attempt(&mut self, text: &str, cancellation: &SessionCancellation) -> RungOutcome {
        let strokes = match key_strokes(text) {
            Ok(strokes) => strokes,
            Err(refused) => return RungOutcome::NotDispatched(backend_error(refused.code())),
        };
        if let Err(outcome) = self.modifiers_released(cancellation) {
            return outcome;
        }
        let deadline = Instant::now() + TYPING_BOUND;
        let mut sent = 0_usize;
        for batch in strokes.chunks(SEND_BATCH) {
            if cancellation.is_cancelled() {
                return if sent == 0 {
                    RungOutcome::CancelledBeforeDispatch
                } else {
                    RungOutcome::Failed(backend_error("delivery_cancelled_mid_input"))
                };
            }
            if Instant::now() >= deadline {
                return RungOutcome::Failed(FallbackReason::Timeout);
            }
            let accepted = self.keyboard.send(batch);
            if accepted != batch.len() {
                return if sent == 0 && accepted == 0 {
                    RungOutcome::NotDispatched(backend_error("delivery_input_blocked"))
                } else {
                    RungOutcome::Failed(backend_error("delivery_input_interrupted"))
                };
            }
            sent += accepted;
            if !self.batch_pause.is_zero() {
                std::thread::sleep(self.batch_pause);
            }
        }
        RungOutcome::SubmittedUnverified
    }
}

// ---------------------------------------------------------------------------
// Clipboard paste
// ---------------------------------------------------------------------------

/// The clipboard as the paste route needs it.
pub trait ClipboardPort: Send {
    /// Everything on the clipboard now, to put back later.
    type Snapshot: Send;

    /// # Errors
    /// A content-free code when the clipboard cannot be captured whole (busy,
    /// too large, formats that cannot be copied). The paste is then refused:
    /// a clipboard that cannot be restored is never replaced.
    fn snapshot(&mut self) -> Result<Self::Snapshot, &'static str>;

    /// Replace the clipboard with `text`; returns a token identifying our
    /// write (the clipboard sequence number on Windows).
    ///
    /// # Errors
    /// A content-free code; the clipboard may then be empty.
    fn write(&mut self, text: &str) -> Result<u32, &'static str>;

    /// Is our write still what the clipboard holds? `token` identifies the
    /// write; `text` lets an implementation confirm by content when the
    /// token alone is not conclusive.
    fn still_owned(&mut self, token: u32, text: &str) -> bool;

    /// # Errors
    /// A content-free code.
    fn restore(&mut self, snapshot: &Self::Snapshot) -> Result<(), &'static str>;
}

/// Clipboard paste route over a [`ClipboardPort`] and a [`KeyboardPort`].
pub struct ClipboardPasteRung<C: ClipboardPort, K: KeyboardPort> {
    clipboard: C,
    keyboard: K,
    hold: Duration,
    last_restore: Option<ClipboardRestoreOutcome>,
}

impl<C: ClipboardPort, K: KeyboardPort> ClipboardPasteRung<C, K> {
    pub fn new(clipboard: C, keyboard: K) -> Self {
        Self {
            clipboard,
            keyboard,
            hold: PASTE_HOLD,
            last_restore: None,
        }
    }

    #[cfg(test)]
    fn with_hold(clipboard: C, keyboard: K, hold: Duration) -> Self {
        Self {
            clipboard,
            keyboard,
            hold,
            last_restore: None,
        }
    }

    fn restore(&mut self, prior: &C::Snapshot, token: u32, text: &str) -> ClipboardRestoreOutcome {
        if !self.clipboard.still_owned(token, text) {
            return ClipboardRestoreOutcome::PreservedNewerContent;
        }
        match self.clipboard.restore(prior) {
            Ok(()) => ClipboardRestoreOutcome::RestoredPrior,
            Err(code) => ClipboardRestoreOutcome::Failed(code),
        }
    }
}

impl<C: ClipboardPort, K: KeyboardPort> DeliveryRung for ClipboardPasteRung<C, K> {
    fn id(&self) -> &'static str {
        CLIPBOARD_PASTE_BACKEND
    }

    fn attempt(&mut self, text: &str, cancellation: &SessionCancellation) -> RungOutcome {
        self.last_restore = None;
        if text.is_empty() {
            return RungOutcome::NotDispatched(backend_error("delivery_text_empty"));
        }
        if cancellation.is_cancelled() {
            return RungOutcome::CancelledBeforeDispatch;
        }
        // Wait out held modifiers: Ctrl+Alt+V or Win+V is not a paste.
        let deadline = Instant::now() + MODIFIER_RELEASE_BOUND;
        while MODIFIER_KEYS.iter().any(|vk| self.keyboard.is_down(*vk)) {
            if cancellation.is_cancelled() {
                return RungOutcome::CancelledBeforeDispatch;
            }
            if Instant::now() >= deadline {
                return RungOutcome::NotDispatched(backend_error("delivery_modifiers_held"));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let prior = match self.clipboard.snapshot() {
            Ok(prior) => prior,
            Err(code) => return RungOutcome::NotDispatched(backend_error(code)),
        };
        let token = match self.clipboard.write(text) {
            Ok(token) => token,
            Err(code) => {
                // Nothing was pasted; put the user's clipboard back.
                self.last_restore = Some(match self.clipboard.restore(&prior) {
                    Ok(()) => ClipboardRestoreOutcome::RestoredPrior,
                    Err(restore) => ClipboardRestoreOutcome::Failed(restore),
                });
                return RungOutcome::NotDispatched(backend_error(code));
            }
        };
        let chord = paste_chord();
        let accepted = self.keyboard.send(&chord);
        let outcome = if accepted == chord.len() {
            // Give the target time to read our text before it goes.
            std::thread::sleep(self.hold);
            RungOutcome::SubmittedUnverified
        } else if accepted == 0 {
            RungOutcome::NotDispatched(backend_error("delivery_input_blocked"))
        } else {
            // Part of the chord went out; release Ctrl so it is not left
            // logically down, and report a failure after dispatch.
            let release = [chord[3]];
            let _ = self.keyboard.send(&release);
            RungOutcome::Failed(backend_error("delivery_input_interrupted"))
        };
        // Restore whatever the chord did: leaving our text behind would
        // silently replace the user's clipboard.
        self.last_restore = Some(self.restore(&prior, token, text));
        outcome
    }

    fn last_restore_outcome(&self) -> Option<ClipboardRestoreOutcome> {
        self.last_restore
    }
}

// ---------------------------------------------------------------------------
// Recovery Inbox
// ---------------------------------------------------------------------------

/// Validate a transaction id for use as a file name stem (the Linux rule):
/// 1–128 bytes of `[A-Za-z0-9._-]`, not starting with `.` or `-`.
#[must_use]
pub fn is_valid_txid(txid: &str) -> bool {
    !txid.is_empty()
        && txid.len() <= 128
        && !txid.starts_with(['.', '-'])
        && txid
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

/// Inbox of `<txid>.txt` files written atomically (temporary file in the same
/// directory, flushed, then renamed into place).
///
/// The directory must be private to the user; on Windows a folder under the
/// user's local application data inherits an owner-only ACL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileInbox {
    dir: PathBuf,
}

impl FileInbox {
    #[must_use]
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    #[must_use]
    pub fn path_for(&self, transaction_id: &TransactionId) -> Option<PathBuf> {
        is_valid_txid(transaction_id.as_str())
            .then(|| self.dir.join(format!("{}.txt", transaction_id.as_str())))
    }

    /// Remove recovered-text files older than `max_age` (F-M4): the Inbox
    /// keeps undelivered text only as long as the recovery journal does.
    /// Only regular `.txt` files are touched, never through a link. Returns
    /// how many were removed.
    pub fn prune_older_than(
        &self,
        max_age: std::time::Duration,
        now: std::time::SystemTime,
    ) -> usize {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return 0;
        };
        let mut removed = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("txt") {
                continue;
            }
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if !metadata.file_type().is_file() {
                continue;
            }
            let expired = metadata
                .modified()
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .is_some_and(|age| age > max_age);
            if expired && std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
        removed
    }
}

impl RecoverySink for FileInbox {
    fn store(
        &mut self,
        transaction_id: &TransactionId,
        text: &str,
        cancellation: &SessionCancellation,
    ) -> Result<(), &'static str> {
        use std::io::Write as _;
        let Some(path) = self.path_for(transaction_id) else {
            return Err("INBOX_TXID_INVALID");
        };
        std::fs::create_dir_all(&self.dir).map_err(|_| "INBOX_WRITE_FAILED")?;
        let mut temporary = tempfile::Builder::new()
            .prefix(".inbox-")
            .tempfile_in(&self.dir)
            .map_err(|_| "INBOX_WRITE_FAILED")?;
        temporary
            .write_all(text.as_bytes())
            .and_then(|()| temporary.as_file().sync_all())
            .map_err(|_| "INBOX_WRITE_FAILED")?;
        if cancellation.is_cancelled() {
            return Err("cancelled");
        }
        temporary.persist(&path).map_err(|_| "INBOX_WRITE_FAILED")?;
        Ok(())
    }
}

/// Inbox-only delivery port: the startup recovery path, which hands every
/// committed but undelivered transcript to the Inbox and never types or
/// pastes (the Windows counterpart of the Linux `InboxOnlyDelivery`).
#[derive(Debug, Clone)]
pub struct InboxOnlyDelivery {
    inbox: FileInbox,
}

impl InboxOnlyDelivery {
    #[must_use]
    pub fn new(dir: PathBuf) -> Self {
        Self {
            inbox: FileInbox::new(dir),
        }
    }
}

impl DeliveryPort for InboxOnlyDelivery {
    fn deliver(
        &mut self,
        intent: &DeliveryIntent,
        text: &str,
        cancellation: &SessionCancellation,
    ) -> Result<DeliveryReport, PlatformError> {
        if cancellation.is_cancelled() {
            return Err(error("cancelled"));
        }
        if intent.backend != INBOX_BACKEND {
            return Err(error("delivery_intent_invalid"));
        }
        self.inbox
            .store(&intent.transaction_id, text, cancellation)
            .map_err(|code| {
                error(match code {
                    "cancelled" => "cancelled",
                    "INBOX_TXID_INVALID" => "inbox_txid_invalid",
                    _ => "inbox_write_failed",
                })
            })?;
        Ok(DeliveryReport {
            transaction_id: intent.transaction_id.clone(),
            backend: INBOX_BACKEND.to_owned(),
            attempts: vec![attempt(INBOX_BACKEND, DeliveryOutcome::Delivered, None)],
            outcome: DeliveryOutcome::Delivered,
            failure_reason: None,
        })
    }
}

// ---------------------------------------------------------------------------
// Replay record
// ---------------------------------------------------------------------------

/// Transactions remembered per adapter instance.
pub const REPLAY_CAPACITY: usize = 64;

struct ReplayEntry {
    transaction_id: TransactionId,
    digest: [u8; 32],
    length: usize,
    report: DeliveryReport,
}

enum ReplayCheck {
    Admissible,
    Replay(DeliveryReport),
    PayloadMismatch,
}

/// Bounded record of dispatched transactions. The oldest entry is evicted
/// past [`REPLAY_CAPACITY`]; transaction ids are unique per daemon run, so
/// only a retry staler than 64 later transactions loses protection.
#[derive(Default)]
struct ReplayRecord {
    entries: VecDeque<ReplayEntry>,
}

fn digest(text: &str) -> [u8; 32] {
    Sha256::digest(text.as_bytes()).into()
}

impl ReplayRecord {
    fn check(&self, transaction_id: &TransactionId, text: &str) -> ReplayCheck {
        let Some(entry) = self
            .entries
            .iter()
            .find(|entry| &entry.transaction_id == transaction_id)
        else {
            return ReplayCheck::Admissible;
        };
        if entry.length == text.len() && entry.digest == digest(text) {
            ReplayCheck::Replay(entry.report.clone())
        } else {
            ReplayCheck::PayloadMismatch
        }
    }

    fn record(&mut self, transaction_id: &TransactionId, text: &str, report: &DeliveryReport) {
        if self.entries.len() == REPLAY_CAPACITY {
            self.entries.pop_front();
        }
        self.entries.push_back(ReplayEntry {
            transaction_id: transaction_id.clone(),
            digest: digest(text),
            length: text.len(),
            report: report.clone(),
        });
    }
}

// ---------------------------------------------------------------------------
// The ladder
// ---------------------------------------------------------------------------

/// [`DeliveryPort`] over the typing and paste routes plus the Inbox.
pub struct RoutedDelivery {
    method: DeliveryMethod,
    typing: Box<dyn DeliveryRung>,
    paste: Box<dyn DeliveryRung>,
    inbox: Box<dyn RecoverySink>,
    clipboard_authorized: bool,
    replay: ReplayRecord,
    last_restore: Option<ClipboardRestoreOutcome>,
}

impl RoutedDelivery {
    /// `clipboard_authorized` is the user's explicit permission for the
    /// paste route to read and restore the clipboard (as on Linux). Without
    /// it the paste route never runs and is reported as
    /// `clipboard_not_authorized`.
    pub fn new(
        method: DeliveryMethod,
        clipboard_authorized: bool,
        typing: Box<dyn DeliveryRung>,
        paste: Box<dyn DeliveryRung>,
        inbox: Box<dyn RecoverySink>,
    ) -> Self {
        Self {
            method,
            typing,
            paste,
            inbox,
            clipboard_authorized,
            replay: ReplayRecord::default(),
            last_restore: None,
        }
    }

    /// The user changed the delivery setting.
    pub fn set_method(&mut self, method: DeliveryMethod) {
        self.method = method;
    }

    /// The user granted or withdrew the clipboard authorization.
    pub fn set_clipboard_authorized(&mut self, authorized: bool) {
        self.clipboard_authorized = authorized;
    }

    #[must_use]
    pub fn method(&self) -> DeliveryMethod {
        self.method
    }

    /// What happened to the clipboard in the last paste, if one ran.
    #[must_use]
    pub fn last_restore_outcome(&self) -> Option<ClipboardRestoreOutcome> {
        self.last_restore
    }

    fn report(
        transaction_id: &TransactionId,
        attempts: Vec<DeliveryAttempt>,
        outcome: DeliveryOutcome,
        failure_reason: Option<FallbackReason>,
    ) -> DeliveryReport {
        DeliveryReport {
            transaction_id: transaction_id.clone(),
            backend: ROUTED_THEN_INBOX_BACKEND.to_owned(),
            attempts,
            outcome,
            failure_reason,
        }
    }
}

fn attempt(
    backend: &str,
    outcome: DeliveryOutcome,
    reason: Option<FallbackReason>,
) -> DeliveryAttempt {
    DeliveryAttempt {
        backend: backend.to_owned(),
        outcome,
        fallback_reason: reason,
        at_unix_ms: now_unix_ms(),
    }
}

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

impl DeliveryPort for RoutedDelivery {
    fn deliver(
        &mut self,
        intent: &DeliveryIntent,
        text: &str,
        cancellation: &SessionCancellation,
    ) -> Result<DeliveryReport, PlatformError> {
        if intent.backend != ROUTED_THEN_INBOX_BACKEND {
            return Err(error("delivery_intent_invalid"));
        }
        match self.replay.check(&intent.transaction_id, text) {
            ReplayCheck::Replay(report) => return Ok(report),
            ReplayCheck::PayloadMismatch => {
                return Err(error("delivery_transaction_reuse_rejected"));
            }
            ReplayCheck::Admissible => {}
        }
        if cancellation.is_cancelled() {
            return Err(error("cancelled"));
        }
        self.last_restore = None;
        let paste_id = self.paste.id();
        let clipboard_authorized = self.clipboard_authorized;
        let rungs: [&mut Box<dyn DeliveryRung>; 2] = match self.method {
            DeliveryMethod::Type => [&mut self.typing, &mut self.paste],
            DeliveryMethod::Paste => [&mut self.paste, &mut self.typing],
        };
        let mut attempts = Vec::new();
        let mut dispatched = false;
        let mut restore = None;
        for rung in rungs {
            if cancellation.is_cancelled() {
                break;
            }
            if rung.id() == paste_id && !clipboard_authorized {
                attempts.push(attempt(
                    paste_id,
                    DeliveryOutcome::Failed,
                    Some(backend_error("clipboard_not_authorized")),
                ));
                continue;
            }
            let outcome = rung.attempt(text, cancellation);
            restore = rung.last_restore_outcome().or(restore);
            match outcome {
                RungOutcome::CancelledBeforeDispatch => break,
                RungOutcome::SubmittedUnverified => {
                    dispatched = true;
                    attempts.push(attempt(
                        rung.id(),
                        DeliveryOutcome::SubmittedUnverified,
                        None,
                    ));
                    break;
                }
                RungOutcome::CopiedOnly => {
                    dispatched = true;
                    attempts.push(attempt(rung.id(), DeliveryOutcome::CopiedOnly, None));
                    break;
                }
                RungOutcome::Failed(reason) => {
                    dispatched = true;
                    attempts.push(attempt(rung.id(), DeliveryOutcome::Failed, Some(reason)));
                    break;
                }
                RungOutcome::NotDispatched(reason) => {
                    attempts.push(attempt(rung.id(), DeliveryOutcome::Failed, Some(reason)));
                }
            }
        }
        self.last_restore = restore;
        if !dispatched && cancellation.is_cancelled() {
            // Nothing left the process: no record and no Inbox artifact.
            return Err(error("cancelled"));
        }
        let txid = &intent.transaction_id;
        let report = match self.inbox.store(txid, text, &SessionCancellation::new()) {
            Ok(()) => {
                attempts.push(attempt(INBOX_BACKEND, DeliveryOutcome::InboxFallback, None));
                Self::report(txid, attempts, DeliveryOutcome::InboxFallback, None)
            }
            Err(code) => {
                attempts.push(attempt(
                    INBOX_BACKEND,
                    DeliveryOutcome::Failed,
                    Some(backend_error(code)),
                ));
                Self::report(
                    txid,
                    attempts,
                    DeliveryOutcome::Failed,
                    Some(backend_error(code)),
                )
            }
        };
        // A route that ran owns the transaction's dispatch decision for the
        // adapter's lifetime; a purely refused ladder may be retried.
        if dispatched || report.outcome == DeliveryOutcome::InboxFallback {
            self.replay.record(txid, text, &report);
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests;
