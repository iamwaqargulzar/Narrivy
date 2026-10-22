//! Framework-neutral overlay state (shared by the desktop hosts).
//!
//! Turns daemon session events into a content-safe overlay presentation:
//! listening (optionally with a live partial), processing, error, or hidden.
//! It enforces transport and per-session sequence ordering, so stale,
//! cross-session or late partials can never replace the current state
//! (`docs/product/ux/overlay.md`: clear transcript state on every entry).
//! `Debug` never prints transcript text.

use std::fmt;
use std::time::Duration;

use serde::Deserialize;

pub(crate) const PROCESSING_LABEL_DELAY: Duration = Duration::from_millis(120);
pub(crate) const ERROR_MESSAGE: &str = "Dictation failed — open Narrivy to recover";

#[derive(Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum OverlayEvent {
    SessionStateChanged {
        session_id: String,
        #[serde(default)]
        transaction_id: Option<String>,
        state: String,
        seq: u64,
        event_seq: u64,
        /// How the text was delivered; only on `committed` (minor 18).
        #[serde(default)]
        delivery: Option<String>,
    },
    TranscriptPartial {
        session_id: String,
        transaction_id: String,
        seq: u64,
        text: String,
        event_seq: u64,
    },
    CapabilityChanged {
        capability: String,
        available: bool,
        event_seq: u64,
    },
    /// The desktop's event stream reconnected (the daemon restarted, for
    /// example); events in between may be lost.
    EventsResubscribed { event_seq: u64 },
    /// The microphone level while listening (minor 21).
    AudioLevel {
        session_id: String,
        level: u16,
        event_seq: u64,
    },
    #[serde(other)]
    Other,
}

/// The daemon publishes this while it loads a model because a dictation
/// asked for a pipeline whose model or device is not the loaded one.
const ENGINE_LOADING: &str = "engine.loading";

/// How a finished dictation reached its destination (content-free).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DeliveryResult {
    /// Insertion was verified.
    Inserted,
    /// Pasted through the clipboard; the user's clipboard was put back.
    Pasted,
    /// Typed through the virtual keyboard.
    Typed,
    /// On the clipboard only; the user pastes it.
    Copied,
    /// Not inserted; kept in the Inbox.
    Inbox,
}

impl DeliveryResult {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "inserted" => Some(Self::Inserted),
            "pasted" => Some(Self::Pasted),
            "typed" => Some(Self::Typed),
            "copied" => Some(Self::Copied),
            "inbox" => Some(Self::Inbox),
            _ => None,
        }
    }

    /// The overlay's words for the result.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Inserted => "Inserted",
            Self::Pasted => "Pasted",
            Self::Typed => "Typed",
            Self::Copied => "Copied — paste with Ctrl+V",
            Self::Inbox => "Not inserted — saved to Inbox",
        }
    }

    /// The text did not reach the window: the user has to act.
    pub(crate) const fn needs_attention(self) -> bool {
        matches!(self, Self::Copied | Self::Inbox)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum OverlayPresentation {
    Hidden,
    /// A model is loading before listening can begin.
    Loading,
    /// That load is over. Listening follows at once when it worked; the
    /// overlay hides shortly after when nothing follows.
    LoadingFinished,
    Listening {
        partial: Option<String>,
    },
    ProcessingPending,
    Processing,
    /// How loud the voice is now (0..=1000), while listening.
    Level(u16),
    /// The text being cleaned up: first as recognised, then as cleaned
    /// (D29 "invisible wait").
    Refining {
        text: String,
    },
    /// The dictation finished; shown briefly before the overlay hides.
    Delivered(DeliveryResult),
    Error,
}

impl fmt::Debug for OverlayPresentation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Hidden => formatter.write_str("Hidden"),
            Self::Loading => formatter.write_str("Loading"),
            Self::LoadingFinished => formatter.write_str("LoadingFinished"),
            Self::Listening { partial } => formatter
                .debug_struct("Listening")
                .field("partial", &partial.as_ref().map(|_| "<redacted>"))
                .finish(),
            Self::ProcessingPending => formatter.write_str("ProcessingPending"),
            Self::Processing => formatter.write_str("Processing"),
            Self::Refining { .. } => formatter.write_str("Refining(<redacted>)"),
            Self::Level(level) => formatter.debug_tuple("Level").field(level).finish(),
            Self::Delivered(result) => formatter.debug_tuple("Delivered").field(result).finish(),
            Self::Error => formatter.write_str("Error"),
        }
    }
}

#[derive(Default)]
pub(crate) struct OverlayReducer {
    accepts_partials: bool,
    /// The text being cleaned up arrives as partials while transforming.
    refining: bool,
    active_session: Option<String>,
    active_transaction: Option<String>,
    state_seq: u64,
    partial_seq: u64,
    transport_seq: u64,
}

impl OverlayReducer {
    pub(crate) fn apply_json(&mut self, payload: &str) -> Option<OverlayPresentation> {
        let event = serde_json::from_str::<OverlayEvent>(payload).ok()?;
        self.apply(event)
    }

    fn apply(&mut self, event: OverlayEvent) -> Option<OverlayPresentation> {
        let event_seq = match &event {
            OverlayEvent::SessionStateChanged { event_seq, .. }
            | OverlayEvent::TranscriptPartial { event_seq, .. }
            | OverlayEvent::CapabilityChanged { event_seq, .. }
            | OverlayEvent::EventsResubscribed { event_seq }
            | OverlayEvent::AudioLevel { event_seq, .. } => *event_seq,
            OverlayEvent::Other => return None,
        };
        if event_seq <= self.transport_seq {
            let restarted_with_new_active_session = matches!(
                &event,
                OverlayEvent::SessionStateChanged {
                    session_id,
                    state,
                    ..
                } if self.active_session.as_ref() != Some(session_id)
                    && !matches!(state.as_str(), "idle" | "committed" | "cancelled" | "failed")
            );
            if !restarted_with_new_active_session {
                return None;
            }
        }
        self.transport_seq = event_seq;

        match event {
            OverlayEvent::SessionStateChanged {
                session_id,
                transaction_id,
                state,
                seq,
                delivery,
                ..
            } => self.apply_state(session_id, transaction_id, state, seq, delivery),
            OverlayEvent::TranscriptPartial {
                session_id,
                transaction_id,
                seq,
                text,
                ..
            } => self.apply_partial(session_id, transaction_id, seq, text),
            OverlayEvent::CapabilityChanged {
                capability,
                available,
                ..
            } => {
                // Only before a session exists: an active dictation's own
                // states always win.
                if capability != ENGINE_LOADING || self.active_session.is_some() {
                    return None;
                }
                Some(if available {
                    OverlayPresentation::Loading
                } else {
                    OverlayPresentation::LoadingFinished
                })
            }
            OverlayEvent::AudioLevel {
                session_id, level, ..
            } => (self.active_session.as_deref() == Some(session_id.as_str())
                && self.accepts_partials
                && !self.refining)
                .then_some(OverlayPresentation::Level(level)),
            OverlayEvent::EventsResubscribed { .. } => {
                // A session shown now may have ended unseen: never leave it
                // on screen. A session still running shows again with its
                // next state.
                self.accepts_partials = false;
                self.active_transaction = None;
                self.active_session
                    .take()
                    .map(|_| OverlayPresentation::Hidden)
            }
            OverlayEvent::Other => None,
        }
    }

    fn apply_state(
        &mut self,
        session_id: String,
        transaction_id: Option<String>,
        state: String,
        seq: u64,
        delivery: Option<String>,
    ) -> Option<OverlayPresentation> {
        let is_new = self.active_session.as_ref() != Some(&session_id);
        if !is_new && seq <= self.state_seq {
            return None;
        }

        let terminal = matches!(
            state.as_str(),
            "idle" | "committed" | "cancelled" | "failed"
        );
        if terminal && is_new && self.active_session.is_some() {
            return None;
        }

        self.active_session = if terminal { None } else { Some(session_id) };
        self.active_transaction = if terminal { None } else { transaction_id };
        self.accepts_partials = state == "listening" || state == "transforming";
        self.refining = state == "transforming";
        self.state_seq = seq;
        self.partial_seq = 0;

        match state.as_str() {
            "preparing" | "listening" => Some(OverlayPresentation::Listening { partial: None }),
            "finalizing_audio" | "transcribing" | "transforming" | "delivering" => {
                Some(OverlayPresentation::ProcessingPending)
            }
            "failed" => Some(OverlayPresentation::Error),
            "committed" => Some(
                delivery
                    .as_deref()
                    .and_then(DeliveryResult::parse)
                    .map_or(OverlayPresentation::Hidden, OverlayPresentation::Delivered),
            ),
            "idle" | "cancelled" => Some(OverlayPresentation::Hidden),
            _ => {
                self.active_session = None;
                self.active_transaction = None;
                Some(OverlayPresentation::Hidden)
            }
        }
    }

    fn apply_partial(
        &mut self,
        session_id: String,
        transaction_id: String,
        seq: u64,
        text: String,
    ) -> Option<OverlayPresentation> {
        // While cleaning up, only the clean-up's own previews count; a late
        // recognition partial never does.
        let preview = seq >= narrivy_ipc::REFINE_PREVIEW_SEQ_BASE;
        if !self.accepts_partials
            || self.refining != preview
            || self.active_session.as_ref() != Some(&session_id)
            || self.active_transaction.as_ref() != Some(&transaction_id)
            || seq <= self.partial_seq
            || text.is_empty()
        {
            return None;
        }
        self.partial_seq = seq;
        Some(if self.refining {
            OverlayPresentation::Refining { text }
        } else {
            OverlayPresentation::Listening {
                partial: Some(text),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn state(
        event_seq: u64,
        session_id: &str,
        transaction_id: Option<&str>,
        state: &str,
        seq: u64,
    ) -> String {
        json!({
            "event": "session_state_changed",
            "event_seq": event_seq,
            "session_id": session_id,
            "transaction_id": transaction_id,
            "pipeline_id": "fast-dictation",
            "state": state,
            "seq": seq,
        })
        .to_string()
    }

    fn partial(
        event_seq: u64,
        session_id: &str,
        transaction_id: &str,
        seq: u64,
        text: &str,
    ) -> String {
        json!({
            "event": "transcript_partial",
            "event_seq": event_seq,
            "session_id": session_id,
            "transaction_id": transaction_id,
            "pipeline_id": "fast-dictation",
            "seq": seq,
            "text": text,
        })
        .to_string()
    }

    fn loading(event_seq: u64, capability: &str, available: bool) -> String {
        json!({
            "event": "capability_changed",
            "event_seq": event_seq,
            "capability": capability,
            "available": available,
        })
        .to_string()
    }

    #[test]
    fn a_model_load_shows_before_listening_and_never_over_a_dictation() {
        let mut reducer = OverlayReducer::default();
        assert_eq!(
            reducer.apply_json(&loading(1, "engine.loading", true)),
            Some(OverlayPresentation::Loading)
        );
        assert_eq!(
            reducer.apply_json(&loading(2, "engine.loading", false)),
            Some(OverlayPresentation::LoadingFinished)
        );
        // Any other capability is not the overlay's business.
        assert_eq!(reducer.apply_json(&loading(3, "mic.available", true)), None);
        assert_eq!(
            reducer.apply_json(&state(4, "1", Some("tx-1"), "listening", 1)),
            Some(OverlayPresentation::Listening { partial: None })
        );
        // While a dictation is active its own states win.
        assert_eq!(
            reducer.apply_json(&loading(5, "engine.loading", true)),
            None
        );
        assert_eq!(
            reducer.apply_json(&loading(6, "engine.loading", false)),
            None
        );
        // A replayed older event is ignored.
        assert_eq!(
            reducer.apply_json(&state(7, "1", Some("tx-1"), "idle", 2)),
            Some(OverlayPresentation::Hidden)
        );
        assert_eq!(
            reducer.apply_json(&loading(2, "engine.loading", true)),
            None
        );
    }

    #[test]
    fn state_entry_clears_previous_partial_and_terminal_hides() {
        let mut reducer = OverlayReducer::default();
        assert_eq!(
            reducer.apply_json(&state(1, "1", Some("tx-1"), "listening", 1)),
            Some(OverlayPresentation::Listening { partial: None })
        );
        assert_eq!(
            reducer.apply_json(&partial(2, "1", "tx-1", 1, "private words")),
            Some(OverlayPresentation::Listening {
                partial: Some("private words".to_owned())
            })
        );
        assert_eq!(
            reducer.apply_json(&state(3, "1", Some("tx-1"), "transcribing", 2)),
            Some(OverlayPresentation::ProcessingPending)
        );
        assert_eq!(
            reducer.apply_json(&state(4, "1", Some("tx-1"), "committed", 3)),
            Some(OverlayPresentation::Hidden)
        );
    }

    #[test]
    fn a_committed_dictation_shows_how_its_text_was_delivered() {
        let committed = |event_seq: u64, session: &str, delivery: &str| {
            json!({
                "event": "session_state_changed",
                "event_seq": event_seq,
                "session_id": session,
                "transaction_id": format!("tx-{session}"),
                "state": "committed",
                "seq": 3,
                "delivery": delivery,
            })
            .to_string()
        };
        let cases = [
            ("pasted", Some(DeliveryResult::Pasted)),
            ("typed", Some(DeliveryResult::Typed)),
            ("inserted", Some(DeliveryResult::Inserted)),
            ("copied", Some(DeliveryResult::Copied)),
            ("inbox", Some(DeliveryResult::Inbox)),
            // An unknown value from a newer daemon hides, as before.
            ("teleported", None),
        ];
        for (index, (wire, expected)) in cases.into_iter().enumerate() {
            let mut reducer = OverlayReducer::default();
            let session = index.to_string();
            let _ = reducer.apply_json(&state(1, &session, Some("tx"), "listening", 1));
            assert_eq!(
                reducer.apply_json(&committed(2, &session, wire)),
                Some(expected.map_or(OverlayPresentation::Hidden, OverlayPresentation::Delivered))
            );
        }
        assert!(DeliveryResult::Inbox.needs_attention());
        assert!(!DeliveryResult::Pasted.needs_attention());
    }

    #[test]
    fn the_text_being_cleaned_up_is_shown_and_replaced() {
        const PREVIEW: u64 = narrivy_ipc::REFINE_PREVIEW_SEQ_BASE;
        let mut reducer = OverlayReducer::default();
        let _ = reducer.apply_json(&state(1, "1", Some("tx-1"), "listening", 1));
        let _ = reducer.apply_json(&state(2, "1", Some("tx-1"), "transcribing", 2));
        // Not while recognising: only listening and clean-up show text.
        assert_eq!(
            reducer.apply_json(&partial(3, "1", "tx-1", 1, "early")),
            None
        );
        assert_eq!(
            reducer.apply_json(&state(4, "1", Some("tx-1"), "transforming", 3)),
            Some(OverlayPresentation::ProcessingPending)
        );
        assert_eq!(
            reducer.apply_json(&partial(
                5,
                "1",
                "tx-1",
                PREVIEW + 1,
                "so um send it friday"
            )),
            Some(OverlayPresentation::Refining {
                text: "so um send it friday".to_owned()
            })
        );
        assert_eq!(
            reducer.apply_json(&partial(6, "1", "tx-1", PREVIEW + 2, "Send it Friday.")),
            Some(OverlayPresentation::Refining {
                text: "Send it Friday.".to_owned()
            })
        );
        // Delivering ends the clean-up: no more text replaces it.
        let _ = reducer.apply_json(&state(7, "1", Some("tx-1"), "delivering", 4));
        assert_eq!(
            reducer.apply_json(&partial(8, "1", "tx-1", PREVIEW + 3, "late")),
            None
        );
    }

    #[test]
    fn the_voice_level_drives_the_waves_only_while_listening() {
        let level = |event_seq: u64, session: &str, level: u16| {
            json!({ "event": "audio_level", "event_seq": event_seq, "session_id": session, "level": level })
                .to_string()
        };
        let mut reducer = OverlayReducer::default();
        // No session: nothing.
        assert_eq!(reducer.apply_json(&level(1, "1", 500)), None);
        let _ = reducer.apply_json(&state(2, "1", Some("tx-1"), "listening", 1));
        assert_eq!(
            reducer.apply_json(&level(3, "1", 640)),
            Some(OverlayPresentation::Level(640))
        );
        // Another session's level never moves this card.
        assert_eq!(reducer.apply_json(&level(4, "2", 900)), None);
        let _ = reducer.apply_json(&state(5, "1", Some("tx-1"), "transcribing", 2));
        assert_eq!(reducer.apply_json(&level(6, "1", 300)), None);
    }

    #[test]
    fn a_reconnect_clears_a_session_that_may_have_ended_unseen() {
        let mut reducer = OverlayReducer::default();
        let resubscribed = |event_seq: u64| {
            json!({ "event": "events_resubscribed", "event_seq": event_seq }).to_string()
        };
        // Nothing on screen: nothing to do.
        assert_eq!(reducer.apply_json(&resubscribed(1)), None);
        let _ = reducer.apply_json(&state(2, "1", Some("tx-1"), "listening", 1));
        assert_eq!(
            reducer.apply_json(&resubscribed(3)),
            Some(OverlayPresentation::Hidden)
        );
        // A session that is still running shows again with its next state.
        assert_eq!(
            reducer.apply_json(&state(4, "1", Some("tx-1"), "transcribing", 4)),
            Some(OverlayPresentation::ProcessingPending)
        );
    }

    #[test]
    fn stale_or_cross_transaction_partial_cannot_replace_current_state() {
        let mut reducer = OverlayReducer::default();
        let _ = reducer.apply_json(&state(10, "7", Some("tx-7"), "listening", 1));
        assert_eq!(
            reducer.apply_json(&partial(9, "7", "tx-7", 1, "stale")),
            None
        );
        assert_eq!(
            reducer.apply_json(&partial(11, "8", "tx-7", 1, "wrong session")),
            None
        );
        assert_eq!(
            reducer.apply_json(&partial(12, "7", "tx-other", 1, "wrong transaction")),
            None
        );
    }

    #[test]
    fn unrelated_terminal_cannot_hide_an_active_session() {
        let mut reducer = OverlayReducer::default();
        let _ = reducer.apply_json(&state(1, "active", Some("tx"), "listening", 1));
        assert_eq!(
            reducer.apply_json(&state(2, "other", Some("other-tx"), "cancelled", 2)),
            None
        );
        assert_eq!(reducer.active_session.as_deref(), Some("active"));
    }

    #[test]
    fn new_active_session_accepts_transport_sequence_reset_after_reconnect() {
        let mut reducer = OverlayReducer::default();
        let _ = reducer.apply_json(&state(40, "old", Some("old-tx"), "listening", 1));
        assert_eq!(
            reducer.apply_json(&state(1, "new", Some("new-tx"), "listening", 1)),
            Some(OverlayPresentation::Listening { partial: None })
        );
        assert_eq!(reducer.active_session.as_deref(), Some("new"));
        assert_eq!(reducer.transport_seq, 1);
    }

    #[test]
    fn debug_never_discloses_partial_text() {
        let presentation = OverlayPresentation::Listening {
            partial: Some("private words".to_owned()),
        };
        let rendered = format!("{presentation:?}");
        assert!(!rendered.contains("private words"));
        assert!(rendered.contains("<redacted>"));
    }

    #[test]
    fn late_partial_cannot_restore_listening_after_capture_ends() {
        for next in [
            "finalizing_audio",
            "transcribing",
            "transforming",
            "delivering",
            "committed",
            "cancelled",
            "failed",
            "idle",
        ] {
            let mut reducer = OverlayReducer::default();
            let _ = reducer.apply_json(&state(1, "1", Some("tx"), "listening", 1));
            let _ = reducer.apply_json(&partial(2, "1", "tx", 1, "fixture"));
            let _ = reducer.apply_json(&state(3, "1", Some("tx"), next, 2));
            assert_eq!(
                reducer.apply_json(&partial(4, "1", "tx", 2, "late")),
                None,
                "partial admitted after {next}"
            );
        }
    }

    #[test]
    fn preparing_does_not_admit_partial_but_listening_does() {
        let mut reducer = OverlayReducer::default();
        let _ = reducer.apply_json(&state(1, "1", Some("tx"), "preparing", 1));
        assert_eq!(reducer.apply_json(&partial(2, "1", "tx", 1, "early")), None);
        let _ = reducer.apply_json(&state(3, "1", Some("tx"), "listening", 2));
        assert_eq!(
            reducer.apply_json(&partial(4, "1", "tx", 2, "fixture")),
            Some(OverlayPresentation::Listening {
                partial: Some("fixture".into())
            })
        );
    }
}
