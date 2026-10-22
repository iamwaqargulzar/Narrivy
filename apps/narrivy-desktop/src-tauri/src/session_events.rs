//! Framework-neutral daemon session-event forwarding for desktop shells.
//!
//! The shell supplies only a stop signal and an event sink. Authentication,
//! explicit subscription, transport sequence preservation, reconnect, and
//! bounded shutdown polling remain one shared implementation.

#![forbid(unsafe_code)]

use std::time::Duration;

use narrivy_ipc::{Command, IpcErrorCode, ResponsePayload};
use narrivy_ipc_unix::{IpcClient, SequencedEvent};

use crate::bridge;

const SUBSCRIBE_TIMEOUT: Duration = Duration::from_secs(4);
const EVENT_WAIT: Duration = Duration::from_millis(500);
const RECONNECT_DELAY: Duration = Duration::from_secs(1);
const STOP_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Whether the shell event sink can accept more events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForwardControl {
    Continue,
    Stop,
}

/// Run until the owner requests shutdown or its event sink closes.
///
/// Daemon absence, failed subscription, and broken transports degrade to a
/// bounded reconnect loop. Events are admitted only after an explicit
/// `Subscribe` response and always include an `event_seq`.
///
/// Each connection numbers its events from 1 again, so `event_seq` is offset
/// to keep rising across reconnects: otherwise a client that drops anything
/// at or below the last number it saw would ignore every event after a daemon
/// restart. After a reconnect, an `events_resubscribed` marker tells clients
/// that events may have been missed and they should re-read the status.
pub fn run(
    should_stop: impl Fn() -> bool,
    mut forward: impl FnMut(serde_json::Value) -> ForwardControl,
) {
    let mut numbering = Numbering::default();
    loop {
        if should_stop() {
            return;
        }

        let socket = bridge::socket_path();
        if let Ok(mut client) = IpcClient::connect_as(&socket, "desktop-events")
            && matches!(
                client.request(&Command::Subscribe, SUBSCRIBE_TIMEOUT),
                Ok(ResponsePayload::Accepted)
            )
        {
            if let Some(marker) = numbering.subscribed()
                && forward(marker) == ForwardControl::Stop
            {
                return;
            }
            if forward_subscribed_events(&mut client, &should_stop, &mut numbering, &mut forward)
                == ForwardControl::Stop
            {
                return;
            }
        }

        if wait_for_reconnect(&should_stop) == ForwardControl::Stop {
            return;
        }
    }
}

/// The forwarded `event_seq`: rises across connections.
#[derive(Default)]
struct Numbering {
    /// Added to this connection's own sequence numbers.
    base: u64,
    /// The last number forwarded.
    last: u64,
    connections: u64,
}

impl Numbering {
    /// A new subscription began. Returns the marker to forward when it is
    /// not the first.
    fn subscribed(&mut self) -> Option<serde_json::Value> {
        self.connections += 1;
        if self.connections == 1 {
            return None;
        }
        self.last += 1;
        self.base = self.last;
        Some(serde_json::json!({
            "event": "events_resubscribed",
            "event_seq": self.last,
        }))
    }

    fn number(&mut self, connection_seq: u64) -> u64 {
        self.last = self.last.max(self.base.saturating_add(connection_seq));
        self.base.saturating_add(connection_seq)
    }
}

fn forward_subscribed_events(
    client: &mut IpcClient,
    should_stop: &impl Fn() -> bool,
    numbering: &mut Numbering,
    forward: &mut impl FnMut(serde_json::Value) -> ForwardControl,
) -> ForwardControl {
    loop {
        if should_stop() {
            return ForwardControl::Stop;
        }
        match client.next_event_with_sequence(EVENT_WAIT) {
            Ok(mut sequenced) => {
                sequenced.event_seq = numbering.number(sequenced.event_seq);
                let Some(payload) = event_payload(sequenced) else {
                    return ForwardControl::Continue;
                };
                if forward(payload) == ForwardControl::Stop {
                    return ForwardControl::Stop;
                }
            }
            Err(err) if err.code == IpcErrorCode::Timeout => {}
            Err(_) => return ForwardControl::Continue,
        }
    }
}

fn event_payload(sequenced: SequencedEvent) -> Option<serde_json::Value> {
    let mut payload = serde_json::to_value(sequenced.event).ok()?;
    let fields = payload.as_object_mut()?;
    fields.insert(
        "event_seq".to_owned(),
        serde_json::Value::from(sequenced.event_seq),
    );
    Some(payload)
}

fn wait_for_reconnect(should_stop: &impl Fn() -> bool) -> ForwardControl {
    let mut waited = Duration::ZERO;
    while waited < RECONNECT_DELAY {
        if should_stop() {
            return ForwardControl::Stop;
        }
        std::thread::sleep(STOP_POLL_INTERVAL);
        waited += STOP_POLL_INTERVAL;
    }
    ForwardControl::Continue
}

#[cfg(test)]
mod tests {
    use super::*;
    use narrivy_ipc::Event;

    #[test]
    fn event_numbers_keep_rising_across_reconnects() {
        let mut numbering = Numbering::default();
        assert_eq!(numbering.subscribed(), None);
        assert_eq!(numbering.number(1), 1);
        assert_eq!(numbering.number(2), 2);
        assert_eq!(numbering.number(3), 3);
        // The daemon restarted: its numbers start again at 1.
        let marker = numbering.subscribed();
        assert_eq!(
            marker,
            Some(serde_json::json!({ "event": "events_resubscribed", "event_seq": 4 }))
        );
        assert_eq!(numbering.number(1), 5);
        assert_eq!(numbering.number(2), 6);
        let marker = numbering.subscribed();
        assert_eq!(marker.and_then(|m| m["event_seq"].as_u64()), Some(7));
        assert_eq!(numbering.number(1), 8);
    }

    #[test]
    fn payload_preserves_transport_and_session_sequences() -> Result<(), &'static str> {
        let payload = event_payload(SequencedEvent {
            event_seq: 11,
            event: Event::SessionStateChanged {
                session_id: 42,
                transaction_id: Some("tx-1".to_owned()),
                pipeline_id: Some("fast-dictation".to_owned()),
                state: "listening".to_owned(),
                seq: 7,
                delivery: None,
            },
        })
        .ok_or("event should serialize to an object")?;

        assert_eq!(payload["event"], "session_state_changed");
        assert_eq!(payload["event_seq"], 11);
        assert_eq!(payload["seq"], 7);
        assert_eq!(payload["session_id"], "42");
        Ok(())
    }

    #[test]
    fn reusable_event_forwarder_has_no_tauri_api_dependency() {
        let forbidden = ["tauri", "::"].concat();
        assert!(!include_str!("session_events.rs").contains(&forbidden));
    }
}
