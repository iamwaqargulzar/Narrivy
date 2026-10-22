//! Narrivy's native-messaging host (D26, SQ-WRITE).
//!
//! The browser starts this program when Narrivy's extension connects, and
//! talks to it over standard input and output: each message is a 32-bit
//! native-endian length and that many bytes of JSON. The host forwards a
//! small set of requests to narrivyd over the same-user socket.
//!
//! **This program is the boundary.** The daemon does not trust handshake
//! roles (ADR-0031), so what the browser can reach is decided here:
//! [`plan`] maps every message the extension may send onto exactly one of
//! six daemon commands (see [`REACHABLE`]), and events are forwarded only for
//! the writing actions this host started. Text is never logged.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use narrivy_ipc::{Command, Event, ResponsePayload};
use narrivy_ipc_unix::{IpcClient, default_socket_path};
use serde_json::{Value, json};

/// The browser accepts at most 1 MB from a host; the extension sends at
/// most 64 MB, but nothing it may send needs more than this.
const MAX_INCOMING: usize = 256 * 1024;
const MAX_OUTGOING: usize = 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const EVENT_WAIT: Duration = Duration::from_millis(500);
const ROLE: &str = "browser-host";

/// The daemon commands the browser can reach through this host, by wire tag.
pub const REACHABLE: [&str; 6] = [
    "request.get_status",
    "request.list_language_models",
    "request.check_writing",
    "request.list_writing_rules",
    "request.writing_action",
    "request.assistant_cancel",
];

/// What one extension message turns into.
#[derive(Debug)]
enum Plan {
    /// Send this command and reply with its answer.
    Ask(Command),
    /// Start a writing action; its reply streams back under the message id.
    Write(Command, String),
    /// Stop the writing action started by message `target`.
    Cancel(u64),
}

/// Map one extension message onto a daemon command. Anything else is
/// refused here, before it reaches the daemon.
fn plan(message: &Value, turn_id: impl FnOnce() -> String) -> Result<Plan, &'static str> {
    let text = |key: &str| message.get(key).and_then(Value::as_str).map(str::to_owned);
    match message.get("type").and_then(Value::as_str) {
        Some("status") => Ok(Plan::Ask(Command::ListLanguageModels)),
        Some("ping") => Ok(Plan::Ask(Command::GetStatus)),
        Some("rules") => Ok(Plan::Ask(Command::ListWritingRules)),
        Some("check") => {
            let mut fields = serde_json::Map::new();
            fields.insert("text".into(), json!(text("text").ok_or("missing_text")?));
            for key in ["dialect", "rules", "words"] {
                if let Some(value) = message.get(key) {
                    fields.insert(key.into(), value.clone());
                }
            }
            command("check_writing", fields).map(Plan::Ask)
        }
        Some("write") => {
            let turn = turn_id();
            let mut fields = serde_json::Map::new();
            fields.insert("turn_id".into(), json!(turn));
            fields.insert(
                "action".into(),
                json!(text("action").ok_or("missing_action")?),
            );
            fields.insert("text".into(), json!(text("text").ok_or("missing_text")?));
            if let Some(instruction) = text("instruction") {
                fields.insert("instruction".into(), json!(instruction));
            }
            command("writing_action", fields).map(|command| Plan::Write(command, turn))
        }
        Some("cancel") => message
            .get("target")
            .and_then(Value::as_u64)
            .map(Plan::Cancel)
            .ok_or("missing_target"),
        _ => Err("unknown_message"),
    }
}

/// Build a command through its own deserializer, so the daemon's bounds
/// (text length, dictionary size, ...) apply here too.
fn command(name: &str, fields: serde_json::Map<String, Value>) -> Result<Command, &'static str> {
    let mut fields = fields;
    fields.insert("command".into(), json!(name));
    serde_json::from_value(Value::Object(fields)).map_err(|_| "invalid_message")
}

/// Turn a daemon answer into the reply for the extension.
fn answer(id: u64, payload: ResponsePayload) -> Value {
    match payload {
        ResponsePayload::WritingFindings(findings) => {
            json!({"id": id, "type": "findings", "findings": findings})
        }
        ResponsePayload::WritingRules(rules) => json!({"id": id, "type": "rules", "rules": rules}),
        ResponsePayload::LanguageModels(models) => {
            let assistant = models
                .iter()
                .find(|model| model.in_use_for.iter().any(|role| role == "assistant"));
            json!({
                "id": id,
                "type": "status",
                "daemon": true,
                "assistant": assistant.map(|model| json!({
                    "name": model.name,
                    "state": model.assistant_state,
                })),
            })
        }
        ResponsePayload::Status(_) => json!({"id": id, "type": "pong"}),
        ResponsePayload::Accepted => json!({"id": id, "type": "accepted"}),
        ResponsePayload::Rejected { code, detail } => {
            // Writing and assistant refusals carry a short code as detail.
            let code = if !detail.is_empty()
                && detail.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
            {
                detail
            } else {
                format!("{code:?}")
            };
            json!({"id": id, "type": "error", "code": code})
        }
        _ => json!({"id": id, "type": "error", "code": "unexpected_reply"}),
    }
}

/// Writing actions in flight: daemon turn id to extension message id.
type Turns = Arc<Mutex<HashMap<String, u64>>>;
type Out = Arc<Mutex<io::Stdout>>;

fn send(out: &Out, message: &Value) {
    let Ok(bytes) = serde_json::to_vec(message) else {
        return;
    };
    if bytes.len() > MAX_OUTGOING {
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        send(
            out,
            &json!({"id": id, "type": "error", "code": "reply_too_large"}),
        );
        return;
    }
    let Ok(length) = u32::try_from(bytes.len()) else {
        return;
    };
    if let Ok(mut out) = out.lock() {
        let _ = out.write_all(&length.to_ne_bytes());
        let _ = out.write_all(&bytes);
        let _ = out.flush();
    }
}

/// Read one message; `None` when the browser closed the pipe.
fn read_message(input: &mut impl Read) -> Option<Result<Value, &'static str>> {
    let mut length = [0u8; 4];
    input.read_exact(&mut length).ok()?;
    let length = u32::from_ne_bytes(length) as usize;
    if length > MAX_INCOMING {
        // Drain it so the next message stays framed.
        io::copy(&mut input.take(length as u64), &mut io::sink()).ok()?;
        return Some(Err("message_too_large"));
    }
    let mut bytes = vec![0u8; length];
    input.read_exact(&mut bytes).ok()?;
    Some(serde_json::from_slice(&bytes).map_err(|_| "invalid_json"))
}

/// Forward the events of this host's writing actions, reconnecting while
/// the daemon restarts.
fn forward_events(turns: &Turns, out: &Out) {
    let socket = default_socket_path();
    loop {
        let subscribed = IpcClient::connect_as(&socket, ROLE)
            .ok()
            .and_then(|mut client| {
                matches!(
                    client.request(&Command::Subscribe, REQUEST_TIMEOUT),
                    Ok(ResponsePayload::Accepted)
                )
                .then_some(client)
            });
        let Some(mut client) = subscribed else {
            thread::sleep(Duration::from_secs(1));
            continue;
        };
        loop {
            match client.next_event(EVENT_WAIT) {
                Ok(event) => forward(&event, turns, out),
                Err(err) if err.code == narrivy_ipc::IpcErrorCode::Timeout => {}
                Err(_) => break,
            }
        }
        // Replies in flight are lost with the daemon.
        let lost: Vec<u64> = turns
            .lock()
            .map(|mut turns| turns.drain().map(|(_, id)| id).collect())
            .unwrap_or_default();
        for id in lost {
            send(
                out,
                &json!({"id": id, "type": "done", "outcome": "failed:daemon_restarted", "text": ""}),
            );
        }
    }
}

fn forward(event: &Event, turns: &Turns, out: &Out) {
    match event {
        Event::AssistantDelta { turn_id, text } => {
            let id = turns
                .lock()
                .ok()
                .and_then(|turns| turns.get(turn_id).copied());
            if let Some(id) = id {
                send(out, &json!({"id": id, "type": "delta", "text": text}));
            }
        }
        Event::AssistantDone {
            turn_id,
            outcome,
            text,
            warning,
        } => {
            let id = turns
                .lock()
                .ok()
                .and_then(|mut turns| turns.remove(turn_id));
            if let Some(id) = id {
                send(
                    out,
                    &json!({"id": id, "type": "done", "outcome": outcome, "text": text, "warning": warning}),
                );
            }
        }
        Event::CapabilityChanged {
            capability,
            available,
        } if capability.starts_with("assistant.") => {
            send(
                out,
                &json!({"type": "assistant", "capability": capability, "available": available}),
            );
        }
        _ => {}
    }
}

fn main() {
    let out: Out = Arc::new(Mutex::new(io::stdout()));
    let turns: Turns = Arc::new(Mutex::new(HashMap::new()));
    {
        let (turns, out) = (Arc::clone(&turns), Arc::clone(&out));
        thread::spawn(move || forward_events(&turns, &out));
    }
    let socket = default_socket_path();
    let mut client: Option<IpcClient> = None;
    let next_turn = AtomicU64::new(0);
    let session = std::process::id();
    let mut input = io::stdin().lock();

    while let Some(message) = read_message(&mut input) {
        let message = match message {
            Ok(message) => message,
            Err(code) => {
                send(&out, &json!({"type": "error", "code": code}));
                continue;
            }
        };
        let id = message.get("id").and_then(Value::as_u64).unwrap_or(0);
        let turn_id = || format!("bx-{session}-{}", next_turn.fetch_add(1, Ordering::Relaxed));
        let plan = match plan(&message, turn_id) {
            Ok(plan) => plan,
            Err(code) => {
                send(&out, &json!({"id": id, "type": "error", "code": code}));
                continue;
            }
        };
        let (command, turn) = match plan {
            Plan::Ask(command) => (command, None),
            Plan::Write(command, turn) => (command, Some(turn)),
            Plan::Cancel(target) => {
                let turn = turns.lock().ok().and_then(|turns| {
                    turns
                        .iter()
                        .find(|(_, message)| **message == target)
                        .map(|(turn, _)| turn.clone())
                });
                let Some(turn_id) = turn else {
                    send(&out, &json!({"id": id, "type": "accepted"}));
                    continue;
                };
                (Command::AssistantCancel { turn_id }, None)
            }
        };
        if let Some(turn) = &turn
            && let Ok(mut turns) = turns.lock()
        {
            turns.insert(turn.clone(), id);
        }
        if client.as_ref().is_none_or(|client| !client.is_connected()) {
            client = IpcClient::connect_as(&socket, ROLE).ok();
        }
        let reply = match client.as_mut() {
            Some(connected) => match connected.request(&command, REQUEST_TIMEOUT) {
                Ok(payload) => answer(id, payload),
                // The daemon answered with a refusal: pass its reason on and
                // keep the connection.
                Err(err) if connected.is_connected() => answer(
                    id,
                    ResponsePayload::Rejected {
                        code: err.code,
                        detail: err.message,
                    },
                ),
                Err(_) => {
                    client = None;
                    json!({"id": id, "type": "error", "code": "daemon_unavailable"})
                }
            },
            None => json!({"id": id, "type": "error", "code": "daemon_unavailable"}),
        };
        if reply.get("type").and_then(Value::as_str) == Some("error")
            && let Some(turn) = &turn
            && let Ok(mut turns) = turns.lock()
        {
            turns.remove(turn);
        }
        send(&out, &reply);
    }
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;

    fn tag(plan: &Plan) -> String {
        let command = match plan {
            Plan::Ask(command) | Plan::Write(command, _) => command,
            Plan::Cancel(_) => return "request.assistant_cancel".to_owned(),
        };
        command.kind().wire_tag().into_owned()
    }

    #[test]
    fn every_message_maps_to_a_reachable_command() {
        let messages = [
            json!({"type": "status"}),
            json!({"type": "ping"}),
            json!({"type": "rules"}),
            json!({"type": "check", "text": "hello", "dialect": "british", "words": ["Narrivy"], "rules": {"SpellCheck": false}}),
            json!({"type": "write", "action": "fix", "text": "hello"}),
            json!({"type": "write", "action": "custom", "text": "hello", "instruction": "rhyme"}),
            json!({"type": "cancel", "target": 3}),
        ];
        for message in messages {
            let Ok(plan) = plan(&message, || "bx-1-0".into()) else {
                panic!("{message} was refused");
            };
            assert!(
                REACHABLE.contains(&tag(&plan).as_str()),
                "{message} reached {}",
                tag(&plan)
            );
        }
    }

    #[test]
    fn anything_else_is_refused_before_the_daemon() {
        for message in [
            json!({"type": "toggle"}),
            json!({"type": "history"}),
            json!({"type": "StartSession"}),
            json!({"StartSession": {}}),
            json!({"type": "check"}),
            json!({"type": "write", "text": "no action"}),
            json!({"type": "check", "text": "x".repeat(70 * 1024)}),
            json!({"type": "write", "action": "fix", "text": "hi", "instruction": "x".repeat(2000)}),
        ] {
            assert!(
                plan(&message, || "bx-1-0".into()).is_err(),
                "{message} was accepted"
            );
        }
    }

    #[test]
    fn only_this_hosts_turns_are_forwarded() {
        let turns: Turns = Arc::new(Mutex::new(HashMap::from([("bx-1-0".to_owned(), 7)])));
        let out: Out = Arc::new(Mutex::new(io::stdout()));
        // A turn of the desktop's Assistant tab is not ours: not in the map,
        // nothing written (and the map is untouched).
        forward(
            &Event::AssistantDone {
                turn_id: "desktop-turn".into(),
                outcome: "ok".into(),
                text: "private".into(),
                warning: None,
            },
            &turns,
            &out,
        );
        assert_eq!(turns.lock().map(|t| t.len()).unwrap_or(0), 1);
    }

    #[test]
    fn framing_round_trips_and_refuses_oversized_messages() {
        let body = br#"{"id":1,"type":"ping"}"#;
        let mut framed = (body.len() as u32).to_ne_bytes().to_vec();
        framed.extend_from_slice(body);
        let mut reader = &framed[..];
        assert!(matches!(read_message(&mut reader), Some(Ok(_))));
        assert!(read_message(&mut reader).is_none());

        let mut big = ((MAX_INCOMING + 1) as u32).to_ne_bytes().to_vec();
        big.extend(std::iter::repeat_n(b' ', MAX_INCOMING + 1));
        let mut reader = &big[..];
        assert!(matches!(
            read_message(&mut reader),
            Some(Err("message_too_large"))
        ));
    }
}
