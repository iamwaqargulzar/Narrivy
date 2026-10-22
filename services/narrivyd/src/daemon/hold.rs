//! Press and hold to dictate (T2.13): opt-in, off by default.
//!
//! The user chooses one key (`hold.json` in the state directory). While the
//! daemon runs, `narrivy_platform_linux::hold_key` reads that key from the
//! keyboards; pressing it starts a dictation with the active pipeline, and
//! letting go finishes it. Press and release go through this daemon's own
//! socket as `StartSession` / `StopSession`, so they take exactly the same
//! admission path as every other client: a press while a dictation is
//! already running starts nothing, and its release stops nothing.
//!
//! Events are handled one at a time, in order, on one thread, so a quick
//! tap (release before the start is answered) still ends the dictation.

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use narrivy_ipc::{Command, ResponsePayload};
use narrivy_platform_linux::hold_key::{HoldEvent, HoldKey, KEYS};
use serde::{Deserialize, Serialize};

/// Starting may first load the speech model.
const START_BOUND: Duration = Duration::from_secs(30);
const STOP_BOUND: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
struct Options {
    /// The chosen key code; `None` is off.
    #[serde(default)]
    key: Option<u16>,
}

static SOCKET: OnceLock<PathBuf> = OnceLock::new();
static RUNNING: Mutex<Option<HoldKey>> = Mutex::new(None);

fn options_path() -> Option<PathBuf> {
    narrivy_platform_linux::instance::state_dir()
        .ok()
        .map(|dir| dir.join("hold.json"))
}

fn options() -> Options {
    options_path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Start reading the chosen key, if one is chosen. `socket` is where this
/// daemon serves.
pub fn start(socket: PathBuf) {
    let _ = SOCKET.set(socket);
    apply(options().key);
}

fn apply(key: Option<u16>) {
    let Ok(mut running) = RUNNING.lock() else {
        return;
    };
    // Dropping the old reader stops it.
    *running = None;
    if let Some(code) = key {
        let events = handler();
        *running = HoldKey::start(code, move |event| {
            let _ = events.send(event);
        })
        .ok();
    }
}

/// One thread that turns presses and releases into sessions, in order.
fn handler() -> Sender<HoldEvent> {
    let (tx, rx) = mpsc::channel::<HoldEvent>();
    let _ = std::thread::Builder::new()
        .name("narrivy-hold".into())
        .spawn(move || {
            let mut started = false;
            for event in rx {
                match event {
                    HoldEvent::Pressed => {
                        started = request(
                            &Command::StartSession {
                                pipeline_id: None,
                                activation: "hold".to_owned(),
                                recognition: None,
                            },
                            START_BOUND,
                        );
                    }
                    HoldEvent::Released if started => {
                        started = false;
                        let _ = request(&Command::StopSession, STOP_BOUND);
                    }
                    HoldEvent::Released => {}
                }
            }
        });
    tx
}

/// Send one command to this daemon; true when it was accepted.
fn request(command: &Command, bound: Duration) -> bool {
    let Some(socket) = SOCKET.get() else {
        return false;
    };
    narrivy_ipc_unix::IpcClient::connect_as(socket, "hold-key")
        .ok()
        .and_then(|mut client| client.request(command, bound).ok())
        .is_some_and(|reply| matches!(reply, ResponsePayload::Accepted))
}

/// Choose the key, or `None` to turn press and hold off.
///
/// # Errors
/// `hold_key_unknown` for a key not offered, `hold_unavailable` when the
/// choice cannot be saved.
pub fn set(key: Option<u16>) -> Result<(), &'static str> {
    if key.is_some_and(|code| !KEYS.iter().any(|(k, _)| *k == code)) {
        return Err("hold_key_unknown");
    }
    let path = options_path().ok_or("hold_unavailable")?;
    let bytes = serde_json::to_vec_pretty(&Options { key }).map_err(|_| "hold_unavailable")?;
    std::fs::write(path, bytes).map_err(|_| "hold_unavailable")?;
    apply(key);
    Ok(())
}

/// The state for clients: the key, the keys offered, and whether it works.
#[must_use]
pub fn dto() -> narrivy_ipc::HoldKeyDto {
    let key = options().key;
    // A reader opens keyboards on its own threads: give it a moment.
    let state = match key {
        None => "off",
        Some(_) => {
            let running = RUNNING.lock().ok();
            match running.as_ref().and_then(|r| r.as_ref()) {
                Some(reader) if reader.is_reading() => "reading",
                Some(reader) if reader.is_denied() => "denied",
                Some(_) => "starting",
                None => "unavailable",
            }
        }
    };
    narrivy_ipc::HoldKeyDto {
        key,
        state: state.to_owned(),
        keys: KEYS
            .iter()
            .map(|(code, name)| narrivy_ipc::HoldKeyChoiceDto {
                code: *code,
                name: (*name).to_owned(),
            })
            .collect(),
    }
}
