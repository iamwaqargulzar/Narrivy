//! The AI assistant extension's library (D33, F24): chats, memory,
//! personas, projects, skills, routines and pictures, kept by the daemon so
//! a reply is saved even when no window is open and routines run on their
//! own.
//!
//! Everything lives in `<state>/assistant/` (private to the user), one JSON
//! file per chat plus one per kind, and pictures as PNG files. Nothing is
//! sent anywhere and nothing is logged. A question asked into a chat is
//! built here: the chat's history, its persona and project, what the
//! assistant remembers, the skills it may use, and the meeting it is about.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use narrivy_ipc::Event;
use serde::{Deserialize, Serialize};

use super::session::{DaemonEvent, DaemonState};

/// A chat keeps at most this many turns; older ones are dropped.
const MAX_TURNS: usize = 400;
const MAX_CHATS: usize = 500;
const MAX_ITEMS: usize = 200;
const MAX_ITEM_BYTES: usize = 32 * 1024;
/// A picture as stored, after the client has scaled it down.
const MAX_PICTURE_BYTES: usize = 4 * 1024 * 1024;
/// What goes to the model: the worker's frame holds 64 KiB, the system
/// message and the newest turns are kept first.
const PROMPT_BUDGET: usize = 60 * 1024;
const MEMORY_BUDGET: usize = 4 * 1024;
const PROJECT_FILE_BUDGET: usize = 8 * 1024;
/// A project folder's own instructions, the first that exists.
const PROJECT_FILES: [&str; 4] = ["ASSISTANT.md", "AGENTS.md", "CLAUDE.md", "README.md"];

/// The assistant's base instructions for a chat.
const BASE_SYSTEM: &str = "You are the assistant in Narrivy, a dictation app. You run entirely \
on the user's computer. Answer helpfully and concisely. Use Markdown for lists, tables and code.";

/// Said only when the chat has no tools: with the web or the computer
/// switched on, the tools' own description says what the model can reach.
const OFFLINE: &str = "In this chat you have no internet access and cannot use the user's \
computer; if asked, say the user can switch on Web or Computer for this chat.";

/// Added only when the question asks for a chart: small models otherwise
/// draw charts nobody asked for.
const CHART_HINT: &str = "Draw the chart as a fenced block whose language is `chart`, holding \
JSON: {\"type\": \"bar\" or \"line\", \"title\": \"...\", \"labels\": [...], \"values\": [...]}.";

fn wants_chart(question: &str) -> bool {
    let lower = question.to_lowercase();
    ["chart", "graph", "plot", "diagram"].iter().any(|word| lower.contains(word))
}

/// Added for a spoken turn (voice mode).
const SPOKEN_STYLE: &str = "This conversation is spoken aloud. Answer the way a person talks: \
short, natural sentences, usually one to three, unless the user asks for more. No Markdown, \
lists, tables, headings, code, links or emoji; spell out symbols. Answer in the language the \
user spoke.";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Turn {
    pub id: String,
    /// `user` or `assistant`.
    pub role: String,
    pub text: String,
    /// Assistant turns: `streaming`, `done`, `cancelled` or `failed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pictures: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub spoken: bool,
    #[serde(default)]
    pub at: u64,
    /// Tools the reply used (MCP), in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolUse>,
}

/// One tool call of a reply, as the chat keeps it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolUse {
    pub server: String,
    pub tool: String,
    pub arguments: String,
    /// `done`, `failed` or `refused`.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// What the step found, for the activity panel (JSON).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMeeting {
    pub id: String,
    pub title: String,
    /// Notes and transcript, already cut to what the model can hold.
    #[serde(default)]
    pub context: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Chat {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub updated_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persona: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meeting: Option<ChatMeeting>,
    /// The routine whose results this chat collects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routine: Option<String>,
    /// Tool servers switched on for this chat (none by default).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    /// `server/tool` the user allowed for the rest of this chat.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_tools: Vec<String>,
    /// Kept at the top of the chat list.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
    /// The computer tools run without asking first ("Full access").
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub full_access: bool,
    /// The live screen share this chat sees while it lasts (a meeting id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live: Option<String>,
    /// Where the computer tools work: relative paths are in it and
    /// commands run there (the home folder when unset).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// How much the model thinks before answering: `short` or `deep`;
    /// unset (the default) answers straight away.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    #[serde(default)]
    pub turns: Vec<Turn>,
}

/// What the chat list shows.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ChatSummary<'a> {
    id: &'a str,
    title: &'a str,
    updated_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    persona: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    project: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    meeting: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    routine: Option<&'a str>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pinned: bool,
    turns: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Memory {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub created_at: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Persona {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub instructions: String,
    /// Voice mode: the English voice's number and speed in percent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed_percent: Option<u16>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    /// A folder whose own instructions file (`ASSISTANT.md`, `AGENTS.md`,
    /// `CLAUDE.md` or `README.md`) goes with every question.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    #[serde(default)]
    pub instructions: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    pub id: String,
    /// Used as `/name` at the start of a question.
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub instructions: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Routine {
    pub id: String,
    pub name: String,
    pub prompt: String,
    /// `hourly`, `daily`, `weekdays` or `weekly`.
    pub every: String,
    /// `HH:MM`, local time (daily, weekdays, weekly; the minute for hourly).
    #[serde(default = "default_time")]
    pub time: String,
    /// Weekly: 0 = Monday … 6 = Sunday.
    #[serde(default)]
    pub weekday: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persona: Option<String>,
    /// Give the routine the notes of meetings since it last ran.
    #[serde(default)]
    pub include_meetings: bool,
    #[serde(default = "enabled_default")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<String>,
}

fn default_time() -> String {
    "08:00".to_owned()
}

const fn enabled_default() -> bool {
    true
}

static LOCK: Mutex<()> = Mutex::new(());

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn new_id(prefix: &str) -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{prefix}-{:x}-{n}", now_ms())
}

fn root() -> Result<PathBuf, &'static str> {
    let dir = narrivy_platform_linux::instance::state_dir()
        .map_err(|_| "assistant_library_unavailable")?
        .join("assistant");
    for sub in ["", "chats", "pictures"] {
        let path = dir.join(sub);
        if !path.is_dir() {
            std::fs::create_dir_all(&path).map_err(|_| "assistant_library_unavailable")?;
            set_private(&path, 0o700);
        }
    }
    Ok(dir)
}

fn set_private(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), &'static str> {
    let part = path.with_extension("part");
    std::fs::write(&part, bytes).map_err(|_| "assistant_library_write_failed")?;
    set_private(&part, 0o600);
    std::fs::rename(&part, path).map_err(|_| "assistant_library_write_failed")
}

fn read_json<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> T {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), &'static str> {
    let bytes = serde_json::to_vec(value).map_err(|_| "assistant_library_write_failed")?;
    write_private(path, &bytes)
}

fn kind_file(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "memory" => "memory.json",
        "persona" => "personas.json",
        "project" => "projects.json",
        "skill" => "skills.json",
        "routine" => "routines.json",
        "tool" => "tools.json",
        _ => return None,
    })
}

fn chat_path(dir: &Path, id: &str) -> Result<PathBuf, &'static str> {
    if !valid_id(id) {
        return Err("assistant_library_bad_id");
    }
    Ok(dir.join("chats").join(format!("{id}.json")))
}

/// A picture's file, when the id is well formed and the picture exists.
pub fn picture_path(id: &str) -> Option<PathBuf> {
    if !valid_id(id) {
        return None;
    }
    let path = root().ok()?.join("pictures").join(format!("{id}.png"));
    path.is_file().then_some(path)
}

/// Keep a copy of a picture (a PNG file) in the picture store; its id.
pub fn keep_picture(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() > MAX_PICTURE_BYTES || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return None;
    }
    let dir = root().ok()?;
    let id = new_id("pic");
    write_private(&dir.join("pictures").join(format!("{id}.png")), &bytes).ok()?;
    Some(id)
}

/// A chat, if it exists.
pub fn chat(id: &str) -> Option<Chat> {
    let dir = root().ok()?;
    let path = chat_path(&dir, id).ok()?;
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn save_chat(dir: &Path, chat: &mut Chat) -> Result<(), &'static str> {
    if chat.turns.len() > MAX_TURNS {
        let excess = chat.turns.len() - MAX_TURNS;
        chat.turns.drain(..excess);
    }
    write_json(&chat_path(dir, &chat.id)?, chat)
}

/// Change a chat under the library lock.
pub fn update_chat(id: &str, change: impl FnOnce(&mut Chat)) -> Result<(), &'static str> {
    let _guard = LOCK.lock().map_err(|_| "assistant_library_unavailable")?;
    let dir = root()?;
    let path = chat_path(&dir, id)?;
    let mut chat: Chat = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .ok_or("assistant_chat_not_found")?;
    change(&mut chat);
    save_chat(&dir, &mut chat)
}

fn items<T: for<'de> Deserialize<'de>>(dir: &Path, kind: &str) -> Vec<T> {
    kind_file(kind)
        .map(|file| read_json::<Vec<serde_json::Value>>(&dir.join(file)))
        .unwrap_or_default()
        .into_iter()
        .filter_map(|value| serde_json::from_value(value).ok())
        .collect()
}

/// The library's items of `kind` (`memory`, `persona`, ...).
pub fn list<T: for<'de> Deserialize<'de>>(kind: &str) -> Vec<T> {
    root().map(|dir| items(&dir, kind)).unwrap_or_default()
}

/// Handle an `AssistantLibrary` request. Replies with JSON.
pub fn handle(
    op: &str,
    kind: &str,
    id: Option<&str>,
    value: Option<&str>,
    state: &Arc<DaemonState>,
) -> Result<String, &'static str> {
    if op == "run" && kind == "routine" {
        let id = id.ok_or("assistant_library_bad_id")?;
        run_routine(id, state)?;
        return Ok("{}".to_owned());
    }
    let _guard = LOCK.lock().map_err(|_| "assistant_library_unavailable")?;
    let dir = root()?;
    match (op, kind) {
        ("list", "chat") => list_chats(&dir),
        ("get", "chat") => {
            let path = chat_path(&dir, id.ok_or("assistant_library_bad_id")?)?;
            std::fs::read_to_string(path).map_err(|_| "assistant_chat_not_found")
        }
        ("put", "chat") => {
            let mut chat: Chat = serde_json::from_str(value.ok_or("assistant_library_bad_value")?)
                .map_err(|_| "assistant_library_bad_value")?;
            if chat.updated_at == 0 {
                chat.updated_at = now_ms();
            }
            save_chat(&dir, &mut chat)?;
            Ok("{}".to_owned())
        }
        ("delete", "chat") => {
            let id = id.ok_or("assistant_library_bad_id")?;
            let path = chat_path(&dir, id)?;
            let old: Chat = read_json(&path);
            for picture in old.turns.iter().flat_map(|turn| turn.pictures.iter()) {
                if valid_id(picture) {
                    let _ =
                        std::fs::remove_file(dir.join("pictures").join(format!("{picture}.png")));
                }
            }
            let _ = std::fs::remove_file(path);
            Ok("{}".to_owned())
        }
        ("import", "chat") => {
            // Chats kept by an older app on its own: moved in once.
            let chats: Vec<Chat> =
                serde_json::from_str(value.ok_or("assistant_library_bad_value")?)
                    .map_err(|_| "assistant_library_bad_value")?;
            for mut chat in chats.into_iter().take(MAX_CHATS) {
                if valid_id(&chat.id)
                    && !dir.join("chats").join(format!("{}.json", chat.id)).exists()
                {
                    for turn in &mut chat.turns {
                        if turn.state.as_deref() == Some("streaming") {
                            turn.state = Some("cancelled".to_owned());
                        }
                    }
                    save_chat(&dir, &mut chat)?;
                }
            }
            Ok("{}".to_owned())
        }
        ("put", "picture") => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(value.ok_or("assistant_library_bad_value")?)
                .map_err(|_| "assistant_picture_invalid")?;
            if bytes.len() > MAX_PICTURE_BYTES || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
                return Err("assistant_picture_invalid");
            }
            let id = new_id("pic");
            write_private(&dir.join("pictures").join(format!("{id}.png")), &bytes)?;
            Ok(serde_json::json!({ "id": id }).to_string())
        }
        ("get", "picture") => {
            let id = id.ok_or("assistant_library_bad_id")?;
            if !valid_id(id) {
                return Err("assistant_library_bad_id");
            }
            let bytes = std::fs::read(dir.join("pictures").join(format!("{id}.png")))
                .map_err(|_| "assistant_picture_missing")?;
            Ok(serde_json::json!({
                "id": id,
                "png": base64::engine::general_purpose::STANDARD.encode(bytes),
            })
            .to_string())
        }
        ("list", kind) if kind_file(kind).is_some() => {
            let file = dir.join(kind_file(kind).unwrap_or_default());
            serde_json::to_string(&read_json::<Vec<serde_json::Value>>(&file))
                .map_err(|_| "assistant_library_unavailable")
        }
        ("put", kind) if kind_file(kind).is_some() => {
            let raw = value.ok_or("assistant_library_bad_value")?;
            if raw.len() > MAX_ITEM_BYTES {
                return Err("assistant_library_bad_value");
            }
            let mut item: serde_json::Value =
                serde_json::from_str(raw).map_err(|_| "assistant_library_bad_value")?;
            let object = item.as_object_mut().ok_or("assistant_library_bad_value")?;
            let id = match object.get("id").and_then(|v| v.as_str()) {
                Some(id) if valid_id(id) => id.to_owned(),
                Some("") | None => {
                    let id = new_id(kind);
                    object.insert("id".into(), id.clone().into());
                    id
                }
                Some(_) => return Err("assistant_library_bad_id"),
            };
            if kind == "memory" && !object.contains_key("createdAt") {
                object.insert("createdAt".into(), now_ms().into());
            }
            // A new routine starts with its next time, not one already
            // passed today.
            if kind == "routine" && object.get("lastRun").is_none_or(serde_json::Value::is_null) {
                object.insert("lastRun".into(), now_ms().into());
            }
            check_item(kind, &item)?;
            let file = dir.join(kind_file(kind).unwrap_or_default());
            let mut all: Vec<serde_json::Value> = read_json(&file);
            match all
                .iter_mut()
                .find(|existing| existing.get("id").and_then(|v| v.as_str()) == Some(&id))
            {
                Some(existing) => *existing = item.clone(),
                None => {
                    if all.len() >= MAX_ITEMS {
                        return Err("assistant_library_full");
                    }
                    all.push(item.clone());
                }
            }
            write_json(&file, &all)?;
            Ok(item.to_string())
        }
        ("delete", kind) if kind_file(kind).is_some() => {
            let id = id.ok_or("assistant_library_bad_id")?;
            let file = dir.join(kind_file(kind).unwrap_or_default());
            let mut all: Vec<serde_json::Value> = read_json(&file);
            all.retain(|existing| existing.get("id").and_then(|v| v.as_str()) != Some(id));
            write_json(&file, &all)?;
            Ok("{}".to_owned())
        }
        _ => Err("assistant_library_bad_request"),
    }
}

/// An item must parse as its kind.
fn check_item(kind: &str, item: &serde_json::Value) -> Result<(), &'static str> {
    let ok =
        match kind {
            "memory" => serde_json::from_value::<Memory>(item.clone())
                .is_ok_and(|m| !m.text.trim().is_empty()),
            "persona" => serde_json::from_value::<Persona>(item.clone())
                .is_ok_and(|p| !p.name.trim().is_empty()),
            "project" => serde_json::from_value::<Project>(item.clone()).is_ok_and(|p| {
                !p.name.trim().is_empty()
                    && p.folder
                        .as_deref()
                        .is_none_or(|f| f.is_empty() || Path::new(f).is_absolute())
            }),
            "skill" => {
                serde_json::from_value::<Skill>(item.clone()).is_ok_and(|s| skill_name_ok(&s.name))
            }
            "tool" => serde_json::from_value::<super::assistant_tools::ToolServer>(item.clone())
            .is_ok_and(|t| {
                !t.name.trim().is_empty()
                    && !t.command.trim().is_empty()
                    && (Path::new(&t.command).is_absolute() || !t.command.contains('/'))
                    && t.args.len() <= 32
            }),
        "routine" => serde_json::from_value::<Routine>(item.clone()).is_ok_and(|r| {
                matches!(r.every.as_str(), "hourly" | "daily" | "weekdays" | "weekly")
                    && clock(&r.time).is_some()
                    && r.weekday < 7
                    && !r.prompt.trim().is_empty()
            }),
            _ => false,
        };
    ok.then_some(()).ok_or("assistant_library_bad_value")
}

fn skill_name_ok(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 40
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
}

fn list_chats(dir: &Path) -> Result<String, &'static str> {
    let mut chats: Vec<Chat> = std::fs::read_dir(dir.join("chats"))
        .map_err(|_| "assistant_library_unavailable")?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|e| e == "json"))
        .filter_map(|entry| std::fs::read(entry.path()).ok())
        .filter_map(|bytes| serde_json::from_slice(&bytes).ok())
        .collect();
    chats.sort_by_key(|chat| std::cmp::Reverse(chat.updated_at));
    chats.truncate(MAX_CHATS);
    let summaries: Vec<ChatSummary<'_>> = chats
        .iter()
        .map(|chat| ChatSummary {
            id: &chat.id,
            title: &chat.title,
            updated_at: chat.updated_at,
            persona: chat.persona.as_deref(),
            project: chat.project.as_deref(),
            meeting: chat
                .meeting
                .as_ref()
                .map(|m| serde_json::json!({"id": m.id, "title": m.title})),
            routine: chat.routine.as_deref(),
            pinned: chat.pinned,
            turns: chat.turns.len(),
        })
        .collect();
    serde_json::to_string(&summaries).map_err(|_| "assistant_library_unavailable")
}

fn title_of(text: &str) -> String {
    let line = text.trim().lines().next().unwrap_or_default();
    if line.chars().count() > 60 {
        format!("{}…", line.chars().take(57).collect::<String>())
    } else {
        line.to_owned()
    }
}

/// "Remember that …" at the start of a question: the fact to keep.
fn fact_to_remember(text: &str) -> Option<String> {
    let lower = text.trim_start().to_lowercase();
    let rest = [
        "please remember that ",
        "remember that ",
        "please remember ",
        "remember: ",
        "remember ",
    ]
    .iter()
    .find_map(|prefix| lower.strip_prefix(prefix).map(|_| prefix.len()))?;
    let fact = text.trim_start()[rest..]
        .trim()
        .trim_end_matches(['.', '!']);
    (fact.len() >= 3 && fact.len() <= 500).then(|| fact.to_owned())
}

/// A question asked into a chat: the messages for the model, with the
/// question saved into the chat and an empty reply waiting for the answer.
pub struct Prepared {
    pub messages: serde_json::Value,
}

/// Save the question into the chat (made when new) and build what the model
/// sees.
pub fn prepare(
    chat_id: &str,
    turn_id: &str,
    question: &str,
    pictures: &[String],
    speak: bool,
) -> Result<Prepared, &'static str> {
    prepare_with(chat_id, turn_id, question, pictures, speak, None, None)
}

/// [`prepare`] with the tools the model may call, described first in the
/// system message (where the model's chat template puts them).
pub fn prepare_with(
    chat_id: &str,
    turn_id: &str,
    question: &str,
    pictures: &[String],
    speak: bool,
    tools: Option<&str>,
    live: Option<&str>,
) -> Result<Prepared, &'static str> {
    if !valid_id(chat_id) {
        return Err("assistant_library_bad_id");
    }
    let picture_paths = pictures
        .iter()
        .map(|id| picture_path(id).ok_or("assistant_picture_missing"))
        .collect::<Result<Vec<_>, _>>()?;
    let _guard = LOCK.lock().map_err(|_| "assistant_library_unavailable")?;
    let dir = root()?;
    let path = chat_path(&dir, chat_id)?;
    let mut chat: Chat = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_else(|| Chat {
            id: chat_id.to_owned(),
            title: title_of(question),
            ..Chat::default()
        });
    if chat.title.is_empty() {
        chat.title = title_of(question);
    }
    // "/skill question": that skill's instructions go with this question.
    let skills: Vec<Skill> = items(&dir, "skill");
    let (skill, question_text) = match question.trim_start().strip_prefix('/') {
        Some(rest) => {
            let name = rest.split_whitespace().next().unwrap_or_default();
            match skills.iter().find(|s| s.name.eq_ignore_ascii_case(name)) {
                Some(skill) => (Some(skill.clone()), rest[name.len()..].trim().to_owned()),
                None => (None, question.to_owned()),
            }
        }
        None => (None, question.to_owned()),
    };
    if let Some(fact) = fact_to_remember(&question_text) {
        let file = dir.join("memory.json");
        let mut all: Vec<serde_json::Value> = read_json(&file);
        if all.len() < MAX_ITEMS {
            all.push(
                serde_json::to_value(Memory {
                    id: new_id("memory"),
                    text: fact,
                    created_at: now_ms(),
                })
                .map_err(|_| "assistant_library_unavailable")?,
            );
            write_json(&file, &all)?;
        }
    }
    let at = now_ms();
    chat.turns.push(Turn {
        id: format!("{turn_id}-q"),
        role: "user".to_owned(),
        text: question.to_owned(),
        pictures: pictures.to_vec(),
        spoken: speak,
        at,
        ..Turn::default()
    });
    let mut system = system_message(&dir, &chat, skill.as_ref(), &skills, speak, tools.is_some());
    if let Some(tools) = tools {
        system = format!("{tools}\n\n{system}");
    }
    if !speak && wants_chart(&question_text) {
        system.push_str("\n\n");
        system.push_str(CHART_HINT);
    }
    // What is shared live goes right beside the question: a small model
    // overlooks it at the end of a long system message.
    let question_text = match live {
        Some(live) => format!("{live}\n\nThe user's question: {question_text}"),
        None => question_text,
    };
    let messages = history(&chat, &system, &question_text, &picture_paths);
    chat.turns.push(Turn {
        id: turn_id.to_owned(),
        role: "assistant".to_owned(),
        state: Some("streaming".to_owned()),
        spoken: speak,
        at,
        ..Turn::default()
    });
    chat.updated_at = at;
    save_chat(&dir, &mut chat)?;
    Ok(Prepared { messages })
}

fn system_message(
    dir: &Path,
    chat: &Chat,
    skill: Option<&Skill>,
    skills: &[Skill],
    speak: bool,
    tools: bool,
) -> String {
    let mut parts = vec![BASE_SYSTEM.to_owned()];
    if !tools {
        parts.push(OFFLINE.to_owned());
    }
    if let Some(persona) = chat.persona.as_ref().and_then(|id| {
        items::<Persona>(dir, "persona")
            .into_iter()
            .find(|p| &p.id == id)
    }) && !persona.instructions.trim().is_empty()
    {
        parts.push(format!(
            "You are \"{}\". {}",
            persona.name,
            persona.instructions.trim()
        ));
    }
    if let Some(project) = chat.project.as_ref().and_then(|id| {
        items::<Project>(dir, "project")
            .into_iter()
            .find(|p| &p.id == id)
    }) {
        let mut text = format!("The user is working on the project \"{}\".", project.name);
        if !project.instructions.trim().is_empty() {
            text.push_str(&format!(
                " Project instructions:\n{}",
                project.instructions.trim()
            ));
        }
        if let Some((name, file)) = project.folder.as_deref().and_then(project_file) {
            text.push_str(&format!("\nThe project's {name}:\n{file}"));
        }
        parts.push(text);
    }
    let memory: Vec<Memory> = items(dir, "memory");
    if !memory.is_empty() {
        let mut text = "What you remember about the user (they asked you to):".to_owned();
        for fact in memory.iter().rev() {
            if text.len() + fact.text.len() > MEMORY_BUDGET {
                break;
            }
            text.push_str("\n- ");
            text.push_str(fact.text.trim());
        }
        parts.push(text);
    }
    if let Some(skill) = skill {
        parts.push(format!(
            "Use the skill \"{}\" for this question:\n{}",
            skill.name,
            skill.instructions.trim()
        ));
    } else if !skills.is_empty() {
        let names: Vec<String> = skills
            .iter()
            .take(30)
            .map(|s| format!("/{} ({})", s.name, s.description.trim()))
            .collect();
        parts.push(format!(
            "The user can start a question with a skill name to use it: {}.",
            names.join(", ")
        ));
    }
    if let Some(meeting) = &chat.meeting {
        parts.push(format!(
            "The user is asking about a meeting they recorded, \"{}\". Answer from the meeting \
material below; say so when it does not cover something, and never invent names, decisions or \
dates. Speakers are names the user gave or labels: Room N (heard by the microphone) and Call N \
(heard through the computer).\n\n{}",
            meeting.title, meeting.context
        ));
    }
    if speak {
        parts.push(SPOKEN_STYLE.to_owned());
    }
    parts.join("\n\n")
}

/// A project folder's instructions file, cut to the budget.
fn project_file(folder: &str) -> Option<(&'static str, String)> {
    let folder = Path::new(folder);
    if !folder.is_absolute() {
        return None;
    }
    PROJECT_FILES.iter().find_map(|name| {
        let path = folder.join(name);
        let meta = std::fs::metadata(&path).ok()?;
        if !meta.is_file() {
            return None;
        }
        let text = std::fs::read(&path).ok()?;
        let text =
            String::from_utf8_lossy(&text[..text.len().min(PROJECT_FILE_BUDGET)]).into_owned();
        Some((*name, text))
    })
}

/// The chat as model messages, newest turns first to fit the budget.
fn history(chat: &Chat, system: &str, question: &str, pictures: &[PathBuf]) -> serde_json::Value {
    let mut budget = PROMPT_BUDGET.saturating_sub(system.len() + question.len());
    let earlier = &chat.turns[..chat.turns.len().saturating_sub(1)];
    let mut kept = Vec::new();
    for turn in earlier.iter().rev() {
        let usable = turn.role == "user"
            || (turn.role == "assistant"
                && !turn.text.is_empty()
                && turn.state.as_deref() != Some("failed"));
        if !usable {
            continue;
        }
        let mut text = turn.text.clone();
        if turn.role == "assistant" && turn.state.as_deref() == Some("cancelled") {
            text.push_str(" […]");
        }
        if !turn.pictures.is_empty() {
            text.push_str(" [a picture was shown]");
        }
        if text.len() + 64 > budget {
            break;
        }
        budget -= text.len() + 64;
        kept.push(serde_json::json!({"role": turn.role, "content": text}));
    }
    kept.reverse();
    let mut messages = vec![serde_json::json!({"role": "system", "content": system})];
    messages.extend(kept);
    let mut last = serde_json::json!({"role": "user", "content": question});
    if !pictures.is_empty() {
        last["images"] = pictures
            .iter()
            .map(|p| serde_json::Value::from(p.to_string_lossy().into_owned()))
            .collect();
    }
    messages.push(last);
    serde_json::Value::Array(messages)
}

/// Save a finished reply into its chat.
pub fn finish(chat_id: &str, turn_id: &str, outcome: &str, text: &str) {
    finish_with_tools(chat_id, turn_id, outcome, text, Vec::new());
}

/// [`finish`], with the tools the reply used.
pub fn finish_with_tools(
    chat_id: &str,
    turn_id: &str,
    outcome: &str,
    text: &str,
    tools: Vec<ToolUse>,
) {
    let _ = update_chat(chat_id, |chat| {
        chat.updated_at = now_ms();
        if let Some(turn) = chat.turns.iter_mut().find(|t| t.id == turn_id) {
            turn.text = text.to_owned();
            turn.tools = tools;
            if let Some(code) = outcome.strip_prefix("failed:") {
                turn.state = Some("failed".to_owned());
                turn.failure = Some(code.to_owned());
            } else {
                turn.state = Some(
                    if outcome == "cancelled" {
                        "cancelled"
                    } else {
                        "done"
                    }
                    .to_owned(),
                );
            }
        }
    });
}

// ------------------------------------------------------------------ routines

/// `HH:MM` as minutes after midnight.
fn clock(time: &str) -> Option<u32> {
    let (h, m) = time.split_once(':')?;
    let (h, m) = (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// The local time now: (days since epoch, weekday 0 = Monday, minute of day).
fn local_now() -> (i64, u32, u32) {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
    let offset = local_offset_seconds();
    let local = secs + offset;
    let days = local.div_euclid(86_400);
    let minute = u32::try_from(local.rem_euclid(86_400) / 60).unwrap_or(0);
    // 1970-01-01 was a Thursday (3 when Monday is 0).
    let weekday = u32::try_from((days + 3).rem_euclid(7)).unwrap_or(0);
    (days, weekday, minute)
}

/// The local UTC offset, from `date` (no time-zone crate in the daemon).
fn local_offset_seconds() -> i64 {
    static CACHED: Mutex<Option<(u64, i64)>> = Mutex::new(None);
    let now = now_ms() / 1000;
    if let Ok(cached) = CACHED.lock()
        && let Some((at, offset)) = *cached
        && now.saturating_sub(at) < 3600
    {
        return offset;
    }
    let offset = std::process::Command::new("date")
        .arg("+%z")
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .and_then(|text| {
            let text = text.trim();
            let sign = if text.starts_with('-') { -1 } else { 1 };
            let digits = text.trim_start_matches(['+', '-']);
            let hours: i64 = digits.get(..2)?.parse().ok()?;
            let minutes: i64 = digits.get(2..4)?.parse().ok()?;
            Some(sign * (hours * 3600 + minutes * 60))
        })
        .unwrap_or(0);
    if let Ok(mut cached) = CACHED.lock() {
        *cached = Some((now, offset));
    }
    offset
}

/// When `routine` last should have run, in local minutes since the epoch,
/// or `None` if it is not due yet today.
fn due(routine: &Routine, now: (i64, u32, u32)) -> bool {
    let (days, weekday, minute) = now;
    let Some(at) = clock(&routine.time) else {
        return false;
    };
    let slot = match routine.every.as_str() {
        "hourly" => {
            let past = minute % 60 >= at % 60;
            if !past {
                return false;
            }
            days * 1440 + i64::from(minute / 60 * 60 + at % 60)
        }
        "daily" => {
            if minute < at {
                return false;
            }
            days * 1440 + i64::from(at)
        }
        "weekdays" => {
            if weekday > 4 || minute < at {
                return false;
            }
            days * 1440 + i64::from(at)
        }
        "weekly" => {
            if weekday != u32::from(routine.weekday) || minute < at {
                return false;
            }
            days * 1440 + i64::from(at)
        }
        _ => return false,
    };
    // The slot in UTC milliseconds.
    let slot_ms = (slot * 60 - local_offset_seconds()) * 1000;
    let last = routine
        .last_run
        .map_or(0, |t| i64::try_from(t).unwrap_or(i64::MAX));
    // A routine missed while the computer was off runs once when it is back,
    // but not one that is many hours late (hourly ones: within the hour).
    let late_ms = now_ms_i64() - slot_ms;
    let window = if routine.every == "hourly" {
        3_600_000
    } else {
        12 * 3_600_000
    };
    last < slot_ms && late_ms < window
}

fn now_ms_i64() -> i64 {
    i64::try_from(now_ms()).unwrap_or(i64::MAX)
}

static SCHEDULER: OnceLock<()> = OnceLock::new();

/// Start the routine scheduler (once per daemon).
pub fn start_routines(state: &Arc<DaemonState>) {
    if SCHEDULER.set(()).is_err() {
        return;
    }
    let state = Arc::clone(state);
    let _ = std::thread::Builder::new()
        .name("narrivy-routines".into())
        .spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(30));
                let now = local_now();
                let routines: Vec<Routine> = list("routine");
                for routine in routines.into_iter().filter(|r| r.enabled) {
                    if due(&routine, now) {
                        let _ = run_routine(&routine.id, &state);
                    }
                }
            }
        });
}

/// Run a routine now: its question goes into its own chat, and the answer
/// is saved there.
pub fn run_routine(id: &str, state: &Arc<DaemonState>) -> Result<(), &'static str> {
    let assistant = super::assistant::shared(state)?;
    let (routine, chat_id) = {
        let _guard = LOCK.lock().map_err(|_| "assistant_library_unavailable")?;
        let dir = root()?;
        let file = dir.join("routines.json");
        let mut all: Vec<Routine> = items(&dir, "routine");
        let routine = all
            .iter_mut()
            .find(|r| r.id == id)
            .ok_or("assistant_routine_not_found")?;
        let previous = routine.last_run;
        routine.last_run = Some(now_ms());
        let chat_id = routine
            .chat_id
            .clone()
            .unwrap_or_else(|| format!("routine-{}", routine.id));
        routine.chat_id = Some(chat_id.clone());
        let mut copy = routine.clone();
        copy.last_run = previous;
        let path = chat_path(&dir, &chat_id)?;
        if !path.exists() {
            let mut chat = Chat {
                id: chat_id.clone(),
                title: routine.name.clone(),
                persona: routine.persona.clone(),
                routine: Some(routine.id.clone()),
                updated_at: now_ms(),
                ..Chat::default()
            };
            save_chat(&dir, &mut chat)?;
        }
        write_json(&file, &all)?;
        (copy, chat_id)
    };
    let mut question = routine.prompt.clone();
    if routine.include_meetings {
        question.push_str("\n\n");
        question.push_str(&recent_meetings(state, routine.last_run.unwrap_or(0)));
    }
    let turn_id = new_id("routine");
    let prepared = prepare(&chat_id, &turn_id, &question, &[], false)?;
    // The chat shows the routine's own question, not the meeting material.
    let shown = routine.prompt.clone();
    let _ = update_chat(&chat_id, |chat| {
        if let Some(turn) = chat
            .turns
            .iter_mut()
            .find(|t| t.id == format!("{turn_id}-q"))
        {
            turn.text = shown;
        }
    });
    let reply = assistant.ask(Some(turn_id.clone()), &prepared.messages)?;
    let state = Arc::clone(state);
    let _ = std::thread::Builder::new()
        .name("narrivy-routine".into())
        .spawn(move || {
            let (outcome, text) = reply
                .recv()
                .unwrap_or_else(|_| ("failed:assistant_unavailable".to_owned(), String::new()));
            finish(&chat_id, &turn_id, &outcome, &text);
            state.publish_assistant(DaemonEvent::Extension(Event::AssistantLibraryChanged {
                kind: "chat".to_owned(),
                id: Some(chat_id),
            }));
        });
    Ok(())
}

/// Notes (or, without notes, the start of the transcript) of the meetings
/// recorded since `since_ms`.
fn recent_meetings(state: &Arc<DaemonState>, since_ms: u64) -> String {
    let Ok(meetings) = super::meetings::shared(state) else {
        return "There are no recorded meetings.".to_owned();
    };
    let since = if since_ms == 0 {
        now_ms().saturating_sub(24 * 3_600_000)
    } else {
        since_ms
    };
    let mut text = String::from("Meetings recorded since the last time:\n");
    let mut found = false;
    for meeting in meetings.list() {
        if meeting.started_unix_ms < since || meeting.state != "ready" {
            continue;
        }
        found = true;
        let notes: HashMap<String, String> = meetings.notes(&meeting.id).into_iter().collect();
        let body = notes.get("notes").cloned().unwrap_or_else(|| {
            meetings
                .get(&meeting.id)
                .map(|detail| {
                    detail
                        .turns
                        .iter()
                        .map(|turn| format!("{}: {}", turn.speaker, turn.text))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default()
        });
        let body: String = body.chars().take(6000).collect();
        text.push_str(&format!("\n## {}\n{}\n", meeting.title, body));
        if text.len() > 40_000 {
            break;
        }
    }
    if !found {
        text.push_str("(none)");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_chat_without_tools_is_told_it_is_offline() {
        let dir = std::env::temp_dir().join(format!("narrivy-system-{}", std::process::id()));
        let chat = Chat::default();
        let offline = system_message(&dir, &chat, None, &[], false, false);
        let online = system_message(&dir, &chat, None, &[], false, true);
        assert!(offline.contains("no internet access"));
        assert!(!online.contains("no internet access"));
        assert!(!online.contains("cannot use"));
    }

    #[test]
    fn remembered_facts_are_found_at_the_start_only() {
        assert_eq!(
            fact_to_remember("Remember that my sister is called Sara."),
            Some("my sister is called Sara".to_owned())
        );
        assert_eq!(fact_to_remember("Do you remember that film?"), None);
        assert_eq!(
            fact_to_remember("remember: I use metric units"),
            Some("I use metric units".to_owned())
        );
    }

    #[test]
    fn routines_are_due_once_per_slot() {
        let routine = Routine {
            id: "r".into(),
            name: "Morning".into(),
            prompt: "Hi".into(),
            every: "daily".into(),
            time: "08:00".into(),
            enabled: true,
            ..Routine::default()
        };
        let (days, weekday, _) = local_now();
        assert!(!due(&routine, (days, weekday, 7 * 60)));
        let ran = Routine {
            last_run: Some(now_ms()),
            ..routine.clone()
        };
        // Run just now, so after yesterday's 08:00 slot whatever the time
        // of day (today's slot may still be ahead).
        assert!(!due(&ran, (days - 1, weekday, 8 * 60 + 1)));
        let long_ago = Routine {
            last_run: Some(0),
            ..routine.clone()
        };
        assert!(due(&long_ago, (days, weekday, 8 * 60 + 1)));
    }

    #[test]
    fn history_keeps_the_newest_turns_and_the_pictures_of_the_question() {
        let mut chat = Chat {
            id: "c".into(),
            ..Chat::default()
        };
        for i in 0..3 {
            chat.turns.push(Turn {
                id: format!("u{i}"),
                role: "user".into(),
                text: format!("question {i}"),
                ..Turn::default()
            });
            chat.turns.push(Turn {
                id: format!("a{i}"),
                role: "assistant".into(),
                text: format!("answer {i}"),
                state: Some("done".into()),
                ..Turn::default()
            });
        }
        chat.turns.push(Turn {
            id: "q".into(),
            role: "user".into(),
            text: "now".into(),
            ..Turn::default()
        });
        let messages = history(&chat, "system", "now", &[PathBuf::from("/tmp/p.png")]);
        let list = messages.as_array().map(Vec::as_slice).unwrap_or_default();
        assert_eq!(list.len(), 8);
        assert_eq!(list[0]["role"], "system");
        assert_eq!(list[7]["content"], "now");
        assert_eq!(list[7]["images"][0], "/tmp/p.png");
    }
}
