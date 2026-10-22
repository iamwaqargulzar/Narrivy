//! Development probe for the assistant extension (protocol minor 44): talk
//! to the running daemon like the desktop app does.
//!
//!   assistant_probe library <op> <kind> [id] [value]
//!   assistant_probe ask <chat-id> <question> [picture.png ...]   (PROBE_SPOKEN=1: as voice mode asks)
//!   assistant_probe voice <action> [speed-percent]
//!   assistant_probe events <seconds>
//!
//! `ask` prints the reply as it streams; the chat is kept by the daemon.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use narrivy_ipc::{AssistantMessageDto, Command, Event, ResponsePayload, VoiceSettingsDto};
use narrivy_ipc_unix::IpcClient;

const T: Duration = Duration::from_secs(60);

fn socket() -> PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
    runtime.join("narrivy/narrivyd.sock")
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut client = IpcClient::connect_as(&socket(), "desktop").expect("connect");
    let library = |client: &mut IpcClient, op: &str, kind: &str, id: Option<String>, value: Option<String>| {
        match client
            .request(&Command::AssistantLibrary {
                op: op.to_owned(),
                kind: kind.to_owned(),
                id,
                value,
            }, T)
            .expect("request")
        {
            ResponsePayload::AssistantLibrary(json) => json,
            other => format!("{other:?}"),
        }
    };
    match args.first().map(String::as_str) {
        Some("library") => {
            let json = library(&mut client, &args[1], &args[2], args.get(3).cloned(), args.get(4).cloned());
            println!("{json}");
        }
        Some("ask") => {
            let chat = args[1].clone();
            let mut pictures = Vec::new();
            for path in &args[3..] {
                let bytes = std::fs::read(path).expect("picture");
                let encoded = base64_encode(&bytes);
                let answer = library(&mut client, "put", "picture", None, Some(encoded));
                let id = answer.split('"').nth(3).unwrap_or_default().to_owned();
                pictures.push(id);
            }
            let _ = client.request(&Command::Subscribe, T);
            let turn = format!("probe-{}", std::process::id());
            let started = Instant::now();
            let reply = client
                .request(&Command::AssistantSend {
                    turn_id: turn.clone(),
                    messages: vec![AssistantMessageDto {
                        role: "user".into(),
                        content: args[2].clone(),
                    }],
                    chat_id: Some(chat),
                    speak: std::env::var_os("PROBE_SPOKEN").is_some(),
                    pictures,
                }, T)
                .expect("send");
            println!("send: {reply:?}");
            let mut first = None;
            loop {
                match client.next_event(Duration::from_secs(600)) {
                    Ok(Event::AssistantDelta { turn_id, text }) if turn_id == turn => {
                        first.get_or_insert_with(|| started.elapsed());
                        print!("{text}");
                    }
                    Ok(Event::AssistantTool { turn_id, call_id, server, tool, arguments, state, summary, detail }) if turn_id == turn => {
                        let _ = detail;
                        println!("\n[tool {server}/{tool} {arguments} -> {state} {summary:?}]");
                        if state == "ask" {
                            let allow = std::env::var("PROBE_ALLOW").map_or(true, |v| v != "0");
                            let mut decider = IpcClient::connect_as(&socket(), "desktop").expect("connect");
                            println!("decision: {:?}", decider.request(&Command::AssistantToolDecision { call_id, allow, always: false }, T));
                        }
                    }
                    Ok(Event::AssistantDone { turn_id, outcome, .. }) if turn_id == turn => {
                        println!(
                            "\n[{outcome}] first text {:?}, total {:?}",
                            first.unwrap_or_default(),
                            started.elapsed()
                        );
                        break;
                    }
                    Ok(_) => {}
                    Err(error) => {
                        println!("event error: {error:?}");
                        break;
                    }
                }
            }
        }
        Some("voice") => {
            let settings = args.get(2).map(|speed| VoiceSettingsDto {
                speed_percent: speed.parse().ok(),
                ..VoiceSettingsDto::default()
            });
            println!(
                "{:?}",
                client.request(&Command::VoiceMode {
                    action: args[1].clone(),
                    settings,
                    down: None,
                }, T)
            );
        }
        Some("events") => {
            println!("subscribe {:?}", client.request(&Command::Subscribe, T));
            let until = Instant::now() + Duration::from_secs(args[1].parse().unwrap_or(30));
            while Instant::now() < until {
                match client.next_event(until.saturating_duration_since(Instant::now()).max(Duration::from_millis(10))) {
                    Ok(Event::Voice { kind, state, text, timing, code, .. }) => {
                        let words = text.map(|t| format!("[{} chars]", t.len()));
                        println!("voice {kind} {state:?} {words:?} {timing:?} {code:?}");
                    }
                    Ok(Event::AssistantLibraryChanged { kind, id }) => println!("library {kind} {id:?}"),
                    Ok(Event::AssistantDone { outcome, .. }) => println!("done {outcome}"),
                    Ok(_) => {}
                    Err(error) if matches!(error.code, narrivy_ipc::IpcErrorCode::Timeout) => {}
                    Err(error) => {
                        println!("error {error:?}");
                        break;
                    }
                }
            }
        }
        _ => eprintln!("usage: see the file's documentation"),
    }
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}
