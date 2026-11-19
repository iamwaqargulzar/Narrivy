use super::*;
use std::sync::{Arc, Mutex};

/// Recording keyboard: accepts up to `budget` events in total.
#[derive(Clone)]
struct FakeKeyboard {
    sent: Arc<Mutex<Vec<KeyStroke>>>,
    budget: usize,
    held: Arc<Mutex<Vec<u16>>>,
}

impl FakeKeyboard {
    fn new(budget: usize) -> Self {
        Self {
            sent: Arc::default(),
            budget,
            held: Arc::default(),
        }
    }

    fn sent(&self) -> Vec<KeyStroke> {
        self.sent
            .lock()
            .map(|sent| sent.clone())
            .unwrap_or_default()
    }
}

impl KeyboardPort for FakeKeyboard {
    fn send(&mut self, strokes: &[KeyStroke]) -> usize {
        let Ok(mut sent) = self.sent.lock() else {
            return 0;
        };
        let accepted = strokes.len().min(self.budget.saturating_sub(sent.len()));
        sent.extend_from_slice(&strokes[..accepted]);
        accepted
    }

    fn is_down(&mut self, vk: u16) -> bool {
        self.held
            .lock()
            .map(|held| held.contains(&vk))
            .unwrap_or(false)
    }
}

fn typed_text(strokes: &[KeyStroke]) -> String {
    let units: Vec<u16> = strokes
        .iter()
        .filter_map(|stroke| match stroke {
            KeyStroke::Unicode { unit, up: false } => Some(*unit),
            _ => None,
        })
        .collect();
    String::from_utf16(&units).unwrap_or_default()
}

#[test]
fn typing_sends_every_event_in_batches() {
    let keyboard = FakeKeyboard::new(usize::MAX);
    let mut rung = TypingRung::without_pause(keyboard.clone());
    let text = "x".repeat(100);
    assert_eq!(
        rung.attempt(&text, &SessionCancellation::new()),
        RungOutcome::SubmittedUnverified
    );
    assert_eq!(keyboard.sent().len(), 200);
    assert_eq!(typed_text(&keyboard.sent()), text);
}

#[test]
fn blocked_input_is_not_a_dispatch_but_a_partial_one_is() {
    let mut blocked = TypingRung::without_pause(FakeKeyboard::new(0));
    assert_eq!(
        blocked.attempt("hello", &SessionCancellation::new()),
        RungOutcome::NotDispatched(backend_error("delivery_input_blocked"))
    );
    let text = "y".repeat(100);
    let mut partial = TypingRung::without_pause(FakeKeyboard::new(70));
    assert_eq!(
        partial.attempt(&text, &SessionCancellation::new()),
        RungOutcome::Failed(backend_error("delivery_input_interrupted"))
    );
}

#[test]
fn typing_waits_for_modifiers_and_gives_up_without_sending() {
    let keyboard = FakeKeyboard::new(usize::MAX);
    if let Ok(mut held) = keyboard.held.lock() {
        held.push(crate::text_input::VK_CONTROL);
    }
    let mut rung = TypingRung::without_pause(keyboard.clone());
    assert_eq!(
        rung.attempt("hi", &SessionCancellation::new()),
        RungOutcome::NotDispatched(backend_error("delivery_modifiers_held"))
    );
    assert!(keyboard.sent().is_empty());
}

#[test]
fn typing_refuses_control_characters_before_sending() {
    let keyboard = FakeKeyboard::new(usize::MAX);
    let mut rung = TypingRung::without_pause(keyboard.clone());
    assert_eq!(
        rung.attempt("a\u{7}", &SessionCancellation::new()),
        RungOutcome::NotDispatched(backend_error("delivery_text_control_character"))
    );
    assert!(keyboard.sent().is_empty());
}

#[test]
fn typing_cancelled_before_start_sends_nothing() {
    let keyboard = FakeKeyboard::new(usize::MAX);
    let mut rung = TypingRung::without_pause(keyboard.clone());
    let cancellation = SessionCancellation::new();
    cancellation.cancel();
    assert_eq!(
        rung.attempt("hi", &cancellation),
        RungOutcome::CancelledBeforeDispatch
    );
    assert!(keyboard.sent().is_empty());
}

/// Clipboard holding one string; `foreign_copy` simulates the user copying
/// while our text is on it.
#[derive(Default)]
struct FakeClipboard {
    content: Option<String>,
    sequence: u32,
    snapshot_error: Option<&'static str>,
    write_error: Option<&'static str>,
    foreign_copy: bool,
}

impl ClipboardPort for FakeClipboard {
    type Snapshot = Option<String>;

    fn snapshot(&mut self) -> Result<Self::Snapshot, &'static str> {
        match self.snapshot_error {
            Some(code) => Err(code),
            None => Ok(self.content.clone()),
        }
    }

    fn write(&mut self, text: &str) -> Result<u32, &'static str> {
        if let Some(code) = self.write_error {
            self.content = None;
            return Err(code);
        }
        self.content = Some(text.to_owned());
        self.sequence += 1;
        let token = self.sequence;
        if self.foreign_copy {
            self.content = Some("copied by the user".to_owned());
            self.sequence += 1;
        }
        Ok(token)
    }

    fn still_owned(&mut self, token: u32, _text: &str) -> bool {
        self.sequence == token
    }

    fn restore(&mut self, snapshot: &Self::Snapshot) -> Result<(), &'static str> {
        self.content = snapshot.clone();
        self.sequence += 1;
        Ok(())
    }
}

fn paste_rung(
    clipboard: FakeClipboard,
    budget: usize,
) -> (
    ClipboardPasteRung<FakeClipboard, FakeKeyboard>,
    FakeKeyboard,
) {
    let keyboard = FakeKeyboard::new(budget);
    (
        ClipboardPasteRung::with_hold(clipboard, keyboard.clone(), Duration::ZERO),
        keyboard,
    )
}

#[test]
fn paste_sends_ctrl_v_and_restores_the_previous_clipboard() {
    let clipboard = FakeClipboard {
        content: Some("previous".to_owned()),
        ..FakeClipboard::default()
    };
    let (mut rung, keyboard) = paste_rung(clipboard, usize::MAX);
    assert_eq!(
        rung.attempt("dictated", &SessionCancellation::new()),
        RungOutcome::SubmittedUnverified
    );
    assert_eq!(keyboard.sent(), paste_chord());
    assert_eq!(rung.clipboard.content.as_deref(), Some("previous"));
    assert_eq!(
        rung.last_restore_outcome(),
        Some(ClipboardRestoreOutcome::RestoredPrior)
    );
}

#[test]
fn an_empty_clipboard_is_emptied_again() {
    let (mut rung, _) = paste_rung(FakeClipboard::default(), usize::MAX);
    assert_eq!(
        rung.attempt("dictated", &SessionCancellation::new()),
        RungOutcome::SubmittedUnverified
    );
    assert_eq!(rung.clipboard.content, None);
}

#[test]
fn a_newer_user_copy_is_never_clobbered() {
    let clipboard = FakeClipboard {
        content: Some("previous".to_owned()),
        foreign_copy: true,
        ..FakeClipboard::default()
    };
    let (mut rung, _) = paste_rung(clipboard, usize::MAX);
    assert_eq!(
        rung.attempt("dictated", &SessionCancellation::new()),
        RungOutcome::SubmittedUnverified
    );
    assert_eq!(
        rung.clipboard.content.as_deref(),
        Some("copied by the user")
    );
    assert_eq!(
        rung.last_restore_outcome(),
        Some(ClipboardRestoreOutcome::PreservedNewerContent)
    );
}

#[test]
fn an_uncapturable_clipboard_is_not_replaced() {
    let clipboard = FakeClipboard {
        content: Some("previous".to_owned()),
        snapshot_error: Some("clipboard_snapshot_too_large"),
        ..FakeClipboard::default()
    };
    let (mut rung, keyboard) = paste_rung(clipboard, usize::MAX);
    assert_eq!(
        rung.attempt("dictated", &SessionCancellation::new()),
        RungOutcome::NotDispatched(backend_error("clipboard_snapshot_too_large"))
    );
    assert!(keyboard.sent().is_empty());
    assert_eq!(rung.clipboard.content.as_deref(), Some("previous"));
}

#[test]
fn a_failed_write_restores_and_sends_nothing() {
    let clipboard = FakeClipboard {
        content: Some("previous".to_owned()),
        write_error: Some("clipboard_write_failed"),
        ..FakeClipboard::default()
    };
    let (mut rung, keyboard) = paste_rung(clipboard, usize::MAX);
    assert_eq!(
        rung.attempt("dictated", &SessionCancellation::new()),
        RungOutcome::NotDispatched(backend_error("clipboard_write_failed"))
    );
    assert!(keyboard.sent().is_empty());
    assert_eq!(rung.clipboard.content.as_deref(), Some("previous"));
}

#[test]
fn a_blocked_chord_still_restores() {
    let clipboard = FakeClipboard {
        content: Some("previous".to_owned()),
        ..FakeClipboard::default()
    };
    let (mut rung, _) = paste_rung(clipboard, 0);
    assert_eq!(
        rung.attempt("dictated", &SessionCancellation::new()),
        RungOutcome::NotDispatched(backend_error("delivery_input_blocked"))
    );
    assert_eq!(rung.clipboard.content.as_deref(), Some("previous"));
}

/// Scripted route that records whether it ran.
struct Scripted {
    id: &'static str,
    outcome: RungOutcome,
    ran: Arc<Mutex<Vec<&'static str>>>,
}

impl DeliveryRung for Scripted {
    fn id(&self) -> &'static str {
        self.id
    }

    fn attempt(&mut self, _text: &str, _cancellation: &SessionCancellation) -> RungOutcome {
        if let Ok(mut ran) = self.ran.lock() {
            ran.push(self.id);
        }
        self.outcome.clone()
    }
}

#[derive(Clone, Default)]
struct MemoryInbox {
    stored: Arc<Mutex<Vec<(String, String)>>>,
    fail: bool,
}

impl RecoverySink for MemoryInbox {
    fn store(
        &mut self,
        transaction_id: &TransactionId,
        text: &str,
        _cancellation: &SessionCancellation,
    ) -> Result<(), &'static str> {
        if self.fail {
            return Err("INBOX_WRITE_FAILED");
        }
        if let Ok(mut stored) = self.stored.lock() {
            stored.push((transaction_id.as_str().to_owned(), text.to_owned()));
        }
        Ok(())
    }
}

struct Ladder {
    delivery: RoutedDelivery,
    ran: Arc<Mutex<Vec<&'static str>>>,
    inbox: MemoryInbox,
}

fn ladder(method: DeliveryMethod, typing: RungOutcome, paste: RungOutcome) -> Ladder {
    let ran: Arc<Mutex<Vec<&'static str>>> = Arc::default();
    let inbox = MemoryInbox::default();
    let delivery = RoutedDelivery::new(
        method,
        true,
        Box::new(Scripted {
            id: TYPING_BACKEND,
            outcome: typing,
            ran: Arc::clone(&ran),
        }),
        Box::new(Scripted {
            id: CLIPBOARD_PASTE_BACKEND,
            outcome: paste,
            ran: Arc::clone(&ran),
        }),
        Box::new(inbox.clone()),
    );
    Ladder {
        delivery,
        ran,
        inbox,
    }
}

fn intent(txid: &str) -> DeliveryIntent {
    DeliveryIntent {
        transaction_id: TransactionId::new(txid),
        backend: ROUTED_THEN_INBOX_BACKEND.to_owned(),
    }
}

fn deliver(ladder: &mut Ladder, txid: &str, text: &str) -> DeliveryReport {
    match ladder
        .delivery
        .deliver(&intent(txid), text, &SessionCancellation::new())
    {
        Ok(report) => report,
        Err(failure) => unreachable!("{failure:?}"),
    }
}

fn ran(ladder: &Ladder) -> Vec<&'static str> {
    ladder.ran.lock().map(|ran| ran.clone()).unwrap_or_default()
}

#[test]
fn typing_first_then_inbox_in_the_routed_shape() {
    let mut ladder = ladder(
        DeliveryMethod::Type,
        RungOutcome::SubmittedUnverified,
        RungOutcome::SubmittedUnverified,
    );
    let report = deliver(&mut ladder, "tx-1", "hello");
    assert_eq!(ran(&ladder), [TYPING_BACKEND]);
    assert_eq!(report.backend, ROUTED_THEN_INBOX_BACKEND);
    assert_eq!(report.outcome, DeliveryOutcome::InboxFallback);
    assert_eq!(report.failure_reason, None);
    let backends: Vec<&str> = report.attempts.iter().map(|a| a.backend.as_str()).collect();
    assert_eq!(backends, [TYPING_BACKEND, INBOX_BACKEND]);
    assert_eq!(
        report.attempts[0].outcome,
        DeliveryOutcome::SubmittedUnverified
    );
    assert_eq!(report.attempts[1].outcome, DeliveryOutcome::InboxFallback);
    assert_eq!(ladder.inbox.stored.lock().map(|s| s.len()).unwrap_or(0), 1);
}

#[test]
fn paste_setting_tries_the_clipboard_first() {
    let mut ladder = ladder(
        DeliveryMethod::Paste,
        RungOutcome::SubmittedUnverified,
        RungOutcome::SubmittedUnverified,
    );
    deliver(&mut ladder, "tx-1", "hello");
    assert_eq!(ran(&ladder), [CLIPBOARD_PASTE_BACKEND]);
}

#[test]
fn a_route_that_sent_nothing_falls_through_to_the_other() {
    let mut ladder = ladder(
        DeliveryMethod::Type,
        RungOutcome::NotDispatched(backend_error("delivery_input_blocked")),
        RungOutcome::SubmittedUnverified,
    );
    let report = deliver(&mut ladder, "tx-1", "hello");
    assert_eq!(ran(&ladder), [TYPING_BACKEND, CLIPBOARD_PASTE_BACKEND]);
    let outcomes: Vec<&DeliveryOutcome> = report.attempts.iter().map(|a| &a.outcome).collect();
    assert_eq!(
        outcomes,
        [
            &DeliveryOutcome::Failed,
            &DeliveryOutcome::SubmittedUnverified,
            &DeliveryOutcome::InboxFallback
        ]
    );
}

#[test]
fn paste_never_runs_without_clipboard_authorization() {
    let mut ladder = ladder(
        DeliveryMethod::Paste,
        RungOutcome::SubmittedUnverified,
        RungOutcome::SubmittedUnverified,
    );
    ladder.delivery.set_clipboard_authorized(false);
    let report = deliver(&mut ladder, "tx-1", "hello");
    assert_eq!(ran(&ladder), [TYPING_BACKEND]);
    assert_eq!(report.attempts[0].backend, CLIPBOARD_PASTE_BACKEND);
    assert_eq!(
        report.attempts[0].fallback_reason,
        Some(backend_error("clipboard_not_authorized"))
    );
    assert_eq!(report.outcome, DeliveryOutcome::InboxFallback);
}

#[test]
fn a_partial_dispatch_ends_the_ladder() {
    let mut ladder = ladder(
        DeliveryMethod::Type,
        RungOutcome::Failed(backend_error("delivery_input_interrupted")),
        RungOutcome::SubmittedUnverified,
    );
    let report = deliver(&mut ladder, "tx-1", "hello");
    assert_eq!(ran(&ladder), [TYPING_BACKEND]);
    assert_eq!(report.outcome, DeliveryOutcome::InboxFallback);
}

#[test]
fn a_stale_retry_replays_without_typing_again() {
    let mut ladder = ladder(
        DeliveryMethod::Type,
        RungOutcome::SubmittedUnverified,
        RungOutcome::SubmittedUnverified,
    );
    let first = deliver(&mut ladder, "tx-1", "hello");
    let second = deliver(&mut ladder, "tx-1", "hello");
    assert_eq!(first, second);
    assert_eq!(ran(&ladder), [TYPING_BACKEND]);
    assert_eq!(
        ladder
            .delivery
            .deliver(&intent("tx-1"), "other text", &SessionCancellation::new())
            .map_err(|failure| failure.code),
        Err("delivery_transaction_reuse_rejected")
    );
}

#[test]
fn cancellation_before_any_dispatch_leaves_no_record() {
    let mut ladder = ladder(
        DeliveryMethod::Type,
        RungOutcome::CancelledBeforeDispatch,
        RungOutcome::SubmittedUnverified,
    );
    let cancellation = SessionCancellation::new();
    cancellation.cancel();
    assert_eq!(
        ladder
            .delivery
            .deliver(&intent("tx-1"), "hello", &cancellation)
            .map_err(|failure| failure.code),
        Err("cancelled")
    );
    assert!(ran(&ladder).is_empty());
    assert_eq!(ladder.inbox.stored.lock().map(|s| s.len()).unwrap_or(1), 0);
}

#[test]
fn inbox_failure_is_a_typed_terminal_report() {
    let mut ladder = ladder(
        DeliveryMethod::Type,
        RungOutcome::SubmittedUnverified,
        RungOutcome::SubmittedUnverified,
    );
    ladder.delivery.inbox = Box::new(MemoryInbox {
        fail: true,
        ..MemoryInbox::default()
    });
    let report = deliver(&mut ladder, "tx-1", "hello");
    assert_eq!(report.outcome, DeliveryOutcome::Failed);
    assert_eq!(
        report.failure_reason,
        Some(backend_error("INBOX_WRITE_FAILED"))
    );
}

#[test]
fn wrong_intent_backend_is_refused() {
    let mut ladder = ladder(
        DeliveryMethod::Type,
        RungOutcome::SubmittedUnverified,
        RungOutcome::SubmittedUnverified,
    );
    let mut wrong = intent("tx-1");
    wrong.backend = "inbox".to_owned();
    assert_eq!(
        ladder
            .delivery
            .deliver(&wrong, "hello", &SessionCancellation::new())
            .map_err(|failure| failure.code),
        Err("delivery_intent_invalid")
    );
}

#[test]
fn file_inbox_writes_atomically_and_validates_ids() {
    let dir = match tempfile::tempdir() {
        Ok(dir) => dir,
        Err(failure) => unreachable!("{failure}"),
    };
    let mut inbox = FileInbox::new(dir.path().join("inbox"));
    let text = "line one\nzweite Zeile 😀";
    assert_eq!(
        inbox.store(
            &TransactionId::new("tx-1.a_b"),
            text,
            &SessionCancellation::new()
        ),
        Ok(())
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("inbox/tx-1.a_b.txt"))
            .ok()
            .as_deref(),
        Some(text)
    );
    for bad in ["", "../x", ".hidden", "-x", "a/b", "a\\b"] {
        assert_eq!(
            inbox.store(&TransactionId::new(bad), text, &SessionCancellation::new()),
            Err("INBOX_TXID_INVALID"),
            "{bad}"
        );
    }
    let cancelled = SessionCancellation::new();
    cancelled.cancel();
    assert_eq!(
        inbox.store(&TransactionId::new("tx-2"), text, &cancelled),
        Err("cancelled")
    );
    let names: Vec<String> = std::fs::read_dir(dir.path().join("inbox"))
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(names, ["tx-1.a_b.txt"]);
}

#[test]
fn delivery_method_uses_the_stored_names() {
    assert_eq!(DeliveryMethod::parse("type"), Some(DeliveryMethod::Type));
    assert_eq!(DeliveryMethod::parse("paste"), Some(DeliveryMethod::Paste));
    assert_eq!(DeliveryMethod::parse("wtype"), None);
    assert_eq!(DeliveryMethod::Paste.as_str(), "paste");
}

#[test]
fn file_inbox_prunes_only_expired_text_files() {
    let dir = match tempfile::tempdir() {
        Ok(dir) => dir,
        Err(failure) => unreachable!("{failure}"),
    };
    let mut inbox = FileInbox::new(dir.path().to_path_buf());
    let none = SessionCancellation::new();
    assert_eq!(
        inbox.store(&TransactionId::new("tx-old"), "a", &none),
        Ok(())
    );
    assert!(std::fs::write(dir.path().join("keep.json"), b"{}").is_ok());
    assert!(std::fs::create_dir(dir.path().join("folder.txt")).is_ok());
    let max_age = std::time::Duration::from_secs(60);
    let now = std::time::SystemTime::now();
    // Nothing is older than a minute yet.
    assert_eq!(inbox.prune_older_than(max_age, now), 0);
    // Two minutes later the text file has expired; other entries stay.
    let later = now + std::time::Duration::from_secs(120);
    assert_eq!(inbox.prune_older_than(max_age, later), 1);
    assert!(!dir.path().join("tx-old.txt").exists());
    assert!(dir.path().join("keep.json").exists());
    assert!(dir.path().join("folder.txt").is_dir());
    // A missing directory prunes nothing.
    assert_eq!(
        FileInbox::new(dir.path().join("absent")).prune_older_than(max_age, later),
        0
    );
}

#[test]
fn inbox_only_delivery_writes_the_inbox_and_nothing_else() {
    let dir = match tempfile::tempdir() {
        Ok(dir) => dir,
        Err(failure) => unreachable!("{failure}"),
    };
    let mut delivery = InboxOnlyDelivery::new(dir.path().to_path_buf());
    let inbox_intent = DeliveryIntent {
        transaction_id: TransactionId::new("tx-recovered"),
        backend: INBOX_BACKEND.to_owned(),
    };
    let report = delivery.deliver(&inbox_intent, "recovered text", &SessionCancellation::new());
    let Ok(report) = report else {
        unreachable!("inbox delivery failed");
    };
    assert_eq!(report.backend, INBOX_BACKEND);
    assert_eq!(report.outcome, DeliveryOutcome::Delivered);
    assert_eq!(report.attempts.len(), 1);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("tx-recovered.txt"))
            .ok()
            .as_deref(),
        Some("recovered text")
    );
    // Another backend's intent and a cancelled delivery write nothing.
    assert_eq!(
        delivery
            .deliver(&intent("tx-other"), "x", &SessionCancellation::new())
            .map_err(|failure| failure.code),
        Err("delivery_intent_invalid")
    );
    let cancelled = SessionCancellation::new();
    cancelled.cancel();
    let other = DeliveryIntent {
        transaction_id: TransactionId::new("tx-cancelled"),
        backend: INBOX_BACKEND.to_owned(),
    };
    assert_eq!(
        delivery
            .deliver(&other, "x", &cancelled)
            .map_err(|failure| failure.code),
        Err("cancelled")
    );
    let bad = DeliveryIntent {
        transaction_id: TransactionId::new("../escape"),
        backend: INBOX_BACKEND.to_owned(),
    };
    assert_eq!(
        delivery
            .deliver(&bad, "x", &SessionCancellation::new())
            .map_err(|failure| failure.code),
        Err("inbox_txid_invalid")
    );
    assert!(!dir.path().join("tx-other.txt").exists());
    assert!(!dir.path().join("tx-cancelled.txt").exists());
}
