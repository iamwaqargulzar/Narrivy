//! The AI assistant extension's tools (D34): MCP servers the user adds,
//! used by the assistant only with the user's approval.
//!
//! A server is a local command speaking the Model Context Protocol over
//! stdin/stdout (JSON-RPC 2.0, one message per line). It is started when a
//! chat that has it switched on asks a question, and stopped after it has
//! been idle. Narrivy gives it no network access of its own; what the
//! command itself may do is the user's choice when adding it.
//!
//! A question with tools runs in rounds: the model answers, and when it
//! asks for a tool (Qwen's `<tool_call><function=…>` format) the user is
//! asked to allow it (once, or for the rest of the chat). The result goes
//! back to the model as a `<tool_response>`, and the model answers again,
//! up to [`MAX_ROUNDS`] times. Arguments and results are never logged.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use narrivy_ipc::Event;
use serde::{Deserialize, Serialize};

use super::assistant::Assistant;
use super::assistant_library::{self, ToolUse};
use super::session::{DaemonEvent, DaemonState};

const MAX_ROUNDS: usize = 5;
/// With the web or the computer, research takes more steps.
const AGENT_ROUNDS: usize = 12;
/// Rounds for a task on the computer (coding: read, edit, run, fix).
const CODING_ROUNDS: usize = 25;
const MAX_CALLS_PER_ROUND: usize = 3;
const REQUEST_BOUND: Duration = Duration::from_secs(60);
const START_BOUND: Duration = Duration::from_secs(60);
const DECISION_BOUND: Duration = Duration::from_secs(300);
const IDLE_STOP: Duration = Duration::from_secs(600);
/// What a tool's result may add to the conversation.
const RESULT_BYTES: usize = 7 * 1024 + 512;
const MAX_TOOLS: usize = 40;

/// An MCP server the user added.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolServer {
    pub id: String,
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
}

/// One tool a server offers.
#[derive(Clone, Debug)]
pub struct Tool {
    pub server_id: String,
    pub server_name: String,
    pub name: String,
    pub description: String,
    pub schema: serde_json::Value,
}

struct Running {
    child: Child,
    stdin: ChildStdin,
    replies: Receiver<serde_json::Value>,
    next_id: u64,
    tools: Vec<Tool>,
    used: Instant,
    /// The command it was started with; a changed server is restarted.
    command: (String, Vec<String>),
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

static SERVERS: Mutex<Option<HashMap<String, Running>>> = Mutex::new(None);
/// The user's answer to a call: (allow, for the rest of the chat).
type Decision = Sender<(bool, bool)>;
static DECISIONS: Mutex<Option<HashMap<String, Decision>>> = Mutex::new(None);

impl Running {
    fn start(server: &ToolServer) -> Result<Self, &'static str> {
        if server.command.trim().is_empty() {
            return Err("tool_server_invalid");
        }
        let mut child = Command::new(&server.command)
            .args(&server.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "tool_server_missing")?;
        let stdin = child.stdin.take().ok_or("tool_server_failed")?;
        let stdout = child.stdout.take().ok_or("tool_server_failed")?;
        let (sender, replies) = mpsc::channel();
        std::thread::Builder::new()
            .name("narrivy-mcp-read".into())
            .spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    if line.len() > 4 * 1024 * 1024 {
                        continue;
                    }
                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line)
                        && sender.send(value).is_err()
                    {
                        break;
                    }
                }
            })
            .map_err(|_| "tool_server_failed")?;
        let mut running = Self {
            child,
            stdin,
            replies,
            next_id: 0,
            tools: Vec::new(),
            used: Instant::now(),
            command: (server.command.clone(), server.args.clone()),
        };
        running.request_within(
            "initialize",
            serde_json::json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "narrivy", "version": env!("CARGO_PKG_VERSION")},
            }),
            START_BOUND,
        )?;
        running.notify("notifications/initialized")?;
        let listed = running.request("tools/list", serde_json::json!({}))?;
        running.tools = listed
            .get("tools")
            .and_then(serde_json::Value::as_array)
            .map(|tools| {
                tools
                    .iter()
                    .take(MAX_TOOLS)
                    .filter_map(|tool| {
                        Some(Tool {
                            server_id: server.id.clone(),
                            server_name: server.name.clone(),
                            name: tool.get("name")?.as_str()?.to_owned(),
                            description: tool
                                .get("description")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or_default()
                                .chars()
                                .take(600)
                                .collect(),
                            schema: tool
                                .get("inputSchema")
                                .cloned()
                                .unwrap_or_else(|| serde_json::json!({"type": "object"})),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(running)
    }

    fn notify(&mut self, method: &str) -> Result<(), &'static str> {
        let message = serde_json::json!({"jsonrpc": "2.0", "method": method});
        writeln!(self.stdin, "{message}").map_err(|_| "tool_server_failed")?;
        self.stdin.flush().map_err(|_| "tool_server_failed")
    }

    fn request(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, &'static str> {
        self.request_within(method, params, REQUEST_BOUND)
    }

    fn request_within(
        &mut self,
        method: &str,
        params: serde_json::Value,
        bound: Duration,
    ) -> Result<serde_json::Value, &'static str> {
        self.next_id += 1;
        let id = self.next_id;
        let message =
            serde_json::json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        writeln!(self.stdin, "{message}").map_err(|_| "tool_server_failed")?;
        self.stdin.flush().map_err(|_| "tool_server_failed")?;
        let deadline = Instant::now() + bound;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let reply = self
                .replies
                .recv_timeout(left)
                .map_err(|_| "tool_server_timeout")?;
            // Requests from the server (or notifications) are not answered.
            if reply.get("id").and_then(serde_json::Value::as_u64) != Some(id) {
                continue;
            }
            self.used = Instant::now();
            if reply.get("error").is_some() {
                return Err("tool_server_error");
            }
            return Ok(reply.get("result").cloned().unwrap_or_default());
        }
    }
}

/// The servers the user added.
pub fn servers() -> Vec<ToolServer> {
    assistant_library::list("tool")
}

/// The tools of the servers switched on for a chat, starting them.
fn tools_of(ids: &[String]) -> Vec<Tool> {
    let configured = servers();
    let mut guard = match SERVERS.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let pool = guard.get_or_insert_with(HashMap::new);
    // Stop servers idle for long, or removed.
    pool.retain(|id, running| {
        running.used.elapsed() < IDLE_STOP && configured.iter().any(|s| &s.id == id)
    });
    let mut tools = Vec::new();
    for server in configured.iter().filter(|s| ids.contains(&s.id)) {
        let changed = pool
            .get(&server.id)
            .is_some_and(|r| r.command != (server.command.clone(), server.args.clone()));
        let dead = pool
            .get_mut(&server.id)
            .is_some_and(|r| r.child.try_wait().ok().flatten().is_some());
        if changed || dead {
            pool.remove(&server.id);
        }
        if !pool.contains_key(&server.id) {
            match Running::start(server) {
                Ok(running) => {
                    pool.insert(server.id.clone(), running);
                }
                Err(code) => {
                    eprintln!("narrivyd: a tool server could not start ({code})");
                    continue;
                }
            }
        }
        if let Some(running) = pool.get(&server.id) {
            tools.extend(running.tools.iter().cloned());
        }
    }
    tools
}

/// Call a tool; its result as text.
fn call(tool: &Tool, arguments: &serde_json::Value) -> Result<(String, bool), &'static str> {
    let mut guard = match SERVERS.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let running = guard
        .get_or_insert_with(HashMap::new)
        .get_mut(&tool.server_id)
        .ok_or("tool_server_stopped")?;
    let result = running.request(
        "tools/call",
        serde_json::json!({"name": tool.name, "arguments": arguments}),
    )?;
    let failed = result
        .get("isError")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let text = result
        .get("content")
        .and_then(serde_json::Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .map(|part| match part.get("type").and_then(|t| t.as_str()) {
                    Some("text") => part
                        .get("text")
                        .and_then(|t| t.as_str())
                        .unwrap_or_default()
                        .to_owned(),
                    Some(other) => format!("[{other} content]"),
                    None => String::new(),
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .or_else(|| result.get("structuredContent").map(ToString::to_string))
        .unwrap_or_default();
    Ok((text, failed))
}

/// The tools, described for the model the way its chat template does.
fn tools_prompt(tools: &[Tool]) -> String {
    let mut out = String::from("# Tools\n\nYou have access to the following functions:\n\n<tools>");
    for tool in tools {
        let spec = serde_json::json!({
            "type": "function",
            "function": {
                "name": tool.name,
                "description": tool.description,
                "parameters": tool.schema,
            },
        });
        out.push('\n');
        out.push_str(&spec.to_string());
    }
    out.push_str(
        "\n</tools>\n\nIf you choose to call a function ONLY reply in the following format with NO \
suffix:\n\n<tool_call>\n<function=example_function_name>\n<parameter=example_parameter_1>\nvalue_1\n\
</parameter>\n</function>\n</tool_call>\n\n<IMPORTANT>\nReminder:\n- Function calls MUST follow the \
specified format: an inner <function=...></function> block must be nested within \
<tool_call></tool_call> XML tags\n- Required parameters MUST be specified\n- You may provide optional \
reasoning for your function call in natural language BEFORE the function call, but NOT after\n- If \
there is no function call available, answer the question like normal with your current knowledge \
and do not tell the user about function calls\n- The user approves every call; if one is refused, \
do not ask again, answer without it\n</IMPORTANT>",
    );
    out
}

/// One call the model asked for.
#[derive(Debug, PartialEq)]
struct Call {
    name: String,
    arguments: serde_json::Map<String, serde_json::Value>,
    /// The call was written to its end; a reply cut off at its length
    /// leaves the last one unfinished (a file half written).
    complete: bool,
}

/// The calls in a reply: `<tool_call><function=NAME><parameter=P>V</parameter>…`.
fn calls_in(text: &str) -> Vec<Call> {
    let mut calls = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("<tool_call>") {
        let after = &rest[start + "<tool_call>".len()..];
        let end = after.find("</tool_call>").unwrap_or(after.len());
        let block = &after[..end];
        rest = &after[end..];
        let Some(open) = block.find("<function=") else {
            // Some models write the call as JSON instead.
            if let Some((name, arguments)) = super::assistant_repair::json_call(block) {
                calls.push(Call {
                    name,
                    arguments,
                    complete: after[end..].starts_with("</tool_call>"),
                });
            }
            continue;
        };
        let header = &block[open + "<function=".len()..];
        let Some(close) = header.find('>') else {
            continue;
        };
        let name = header[..close].trim().to_owned();
        let complete = header.contains("</function>");
        let mut arguments = serde_json::Map::new();
        let mut body = &header[close + 1..];
        while let Some(p) = body.find("<parameter=") {
            let head = &body[p + "<parameter=".len()..];
            let Some(close) = head.find('>') else {
                break;
            };
            let key = head[..close].trim().to_owned();
            let value_start = &head[close + 1..];
            let value_end = value_start
                .find("</parameter>")
                .unwrap_or(value_start.len());
            let raw = value_start[..value_end].trim_matches('\n');
            // Numbers, booleans, lists and objects are written as JSON.
            let value = serde_json::from_str::<serde_json::Value>(raw)
                .ok()
                .filter(|v| !v.is_string())
                .unwrap_or_else(|| serde_json::Value::String(raw.to_owned()));
            arguments.insert(key, value);
            body = &value_start[value_end..];
        }
        if !name.is_empty() {
            calls.push(Call {
                name,
                arguments,
                complete,
            });
        }
    }
    calls
}

/// A reply without its tool calls (what the user reads).
pub fn without_calls(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("<tool_call>") {
        out.push_str(&rest[..start]);
        match rest[start..].find("</tool_call>") {
            Some(end) => rest = &rest[start + end + "</tool_call>".len()..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out.trim().to_owned()
}

fn publish(state: &DaemonState, event: Event) {
    state.publish_assistant(DaemonEvent::Extension(event));
}

/// The user's decision on a tool call.
pub fn decide(call_id: &str, allow: bool, always: bool) -> Result<(), &'static str> {
    let sender = DECISIONS
        .lock()
        .ok()
        .and_then(|mut d| d.get_or_insert_with(HashMap::new).remove(call_id))
        .ok_or("tool_call_not_waiting")?;
    sender
        .send((allow, always))
        .map_err(|_| "tool_call_not_waiting")
}

/// Ask the user about a call; `None` when no answer came in time.
fn ask_user(call_id: &str) -> Option<(bool, bool)> {
    let (sender, receiver) = mpsc::channel();
    DECISIONS
        .lock()
        .ok()?
        .get_or_insert_with(HashMap::new)
        .insert(call_id.to_owned(), sender);
    let answer = receiver.recv_timeout(DECISION_BOUND).ok();
    if let Ok(mut decisions) = DECISIONS.lock() {
        decisions.get_or_insert_with(HashMap::new).remove(call_id);
    }
    answer
}

/// Whether a chat has tools switched on (MCP servers, the web, the
/// computer).
#[must_use]
pub fn chat_has_tools(chat_id: &str) -> bool {
    assistant_library::chat(chat_id).is_some_and(|chat| !chat.tools.is_empty())
}

/// Count a failure; true when this exact one happened before.
fn repeated(failures: &mut HashMap<String, u32>, signature: &str) -> bool {
    let count = failures.entry(signature.to_owned()).or_insert(0);
    *count += 1;
    *count >= 2
}

/// Whether a question runs in tool rounds: the chat has tools switched on,
/// or it asks for a document or has documents to change (D55, offered in
/// every chat).
#[must_use]
pub fn needs_rounds(chat_id: &str, question: &str) -> bool {
    chat_has_tools(chat_id)
        || super::assistant_documents::wanted(question)
        || super::assistant_documents::has_any(chat_id)
}

/// Answer a question in a chat with tools, on its own thread: the reply
/// streams under `turn_id` and ends with one `AssistantDone`.
#[allow(clippy::too_many_arguments)]
pub fn answer(
    assistant: Arc<Assistant>,
    state: Arc<DaemonState>,
    turn_id: String,
    chat_id: String,
    question: String,
    pictures: Vec<String>,
    speak: bool,
    live: Option<String>,
) -> Result<(), &'static str> {
    if speak {
        super::voice_mode::reply_started(&turn_id);
    }
    std::thread::Builder::new()
        .name("narrivy-assistant-tools".into())
        .spawn(move || {
            let (outcome, text, uses) = rounds(
                &assistant,
                &state,
                &turn_id,
                &chat_id,
                &question,
                &pictures,
                speak,
                live.as_deref(),
            );
            assistant_library::finish_with_tools(&chat_id, &turn_id, &outcome, &text, uses);
            // A spoken question hears only the final answer, not the
            // thinking between tool calls.
            if speak {
                if outcome == "ok" && !text.trim().is_empty() {
                    super::voice_mode::reply_piece(&turn_id, &text);
                }
                super::voice_mode::reply_done(&turn_id, &outcome);
            }
            state.publish_assistant(DaemonEvent::AssistantDone {
                turn_id,
                outcome,
                text,
                warning: None,
            });
        })
        .map(|_| ())
        .map_err(|_| "assistant_unavailable")
}

#[allow(clippy::too_many_arguments)]
fn rounds(
    assistant: &Assistant,
    state: &DaemonState,
    turn_id: &str,
    chat_id: &str,
    question: &str,
    pictures: &[String],
    speak: bool,
    live: Option<&str>,
) -> (String, String, Vec<ToolUse>) {
    let chat = assistant_library::chat(chat_id).unwrap_or_default();
    let switched_on = chat.tools.clone();
    let plan = chat.permission() == assistant_library::Permission::Plan;
    let mut tools = tools_of(&switched_on);
    let builtin = [
        super::assistant_builtin::WEB,
        super::assistant_builtin::COMPUTER,
    ]
    .into_iter()
    .filter(|id| switched_on.iter().any(|t| t == id))
    .collect::<Vec<_>>();
    for id in &builtin {
        // Plan changes nothing: the tools that write, run or open are not offered.
        tools.extend(super::assistant_builtin::tools(id).into_iter().filter(|t| {
            !plan
                || super::assistant_builtin::approval(&t.server_id, &t.name)
                    != super::assistant_builtin::Approval::Always
        }));
    }
    // Documents (D55): offered when one is asked for, or the chat has one
    // to change.
    let documents = super::assistant_documents::wanted(question)
        || super::assistant_documents::has_any(chat_id);
    if documents {
        tools.extend(super::assistant_documents::tools());
    }
    let mut allowed: Vec<String> = chat.allowed_tools.clone();
    let prompt = (!tools.is_empty()).then(|| {
        let mut prompt = tools_prompt(&tools);
        prompt.push_str(&agent_guidance(&builtin, speak));
        if documents {
            prompt.push_str(super::assistant_documents::guidance());
        }
        if builtin.contains(&super::assistant_builtin::COMPUTER) {
            let folder = chat
                .folder
                .clone()
                .or_else(super::assistant::default_folder);
            prompt.push_str(&working_folder(folder.as_deref()));
        }
        if plan {
            prompt.push_str(PLAN_MODE);
        }
        prompt
    });
    let prepared = match assistant_library::prepare_with(
        chat_id,
        turn_id,
        question,
        pictures,
        speak,
        prompt.as_deref(),
        live,
    ) {
        Ok(prepared) => prepared,
        Err(code) => return (format!("failed:{code}"), String::new(), Vec::new()),
    };
    // Pictures of the screen live only for this reply, in a private folder.
    let scratch = super::host::state_dir()
        .map_or_else(|_| std::env::temp_dir(), |dir| dir.join("assistant"))
        .join(format!("agent-{turn_id}"));
    let _ = super::host::fs::dir_builder(0o700).create(&scratch);
    // On the computer a task takes many steps (read, edit, run, fix).
    let max_rounds = if builtin.contains(&super::assistant_builtin::COMPUTER) {
        CODING_ROUNDS
    } else if builtin.is_empty() && !documents {
        MAX_ROUNDS
    } else {
        AGENT_ROUNDS
    };
    let mut ran_out = false;
    let mut nudged = false;
    let mut failures: HashMap<String, u32> = HashMap::new();
    let mut stuck: Option<(String, String)> = None;
    let mut nudge_text = String::new();
    let mut messages = prepared.messages;
    let mut visible = String::new();
    let mut uses = Vec::new();
    // A spoken answer is read as it comes: no thinking first.
    let thinking = if speak {
        None
    } else {
        chat.thinking.as_deref()
    };
    // With the web on, a question about the world is searched straight
    // away: the model's memory is often out of date, and left to itself it
    // may answer from it or say it cannot look. Not during a live share,
    // where questions are mostly about what is shared, nor for a task on
    // the computer.
    let search_first = live.is_none()
        && builtin.contains(&super::assistant_builtin::WEB)
        && worth_searching(question)
        && !(builtin.contains(&super::assistant_builtin::COMPUTER) && is_computer_task(question));
    for round in 0..max_rounds {
        let (outcome, mut text) = if round == 0 && search_first {
            let query = super::assistant_builtin::unguessed(question.trim(), question);
            (
                "ok".to_owned(),
                format!(
                    "<tool_call>\n<function=web_search>\n<parameter=query>\n{query}\n</parameter>\n</function>\n</tool_call>"
                ),
            )
        } else {
            let reply = match assistant.ask_streaming(turn_id, &messages, thinking) {
                Ok(reply) => reply,
                Err(code) => return (format!("failed:{code}"), visible, uses),
            };
            let Ok(reply) = reply.recv() else {
                return ("failed:assistant_unavailable".to_owned(), visible, uses);
            };
            reply
        };
        let calls = calls_in(&text);
        if !calls.is_empty() {
            // A small model sometimes writes on past its call, inventing the
            // user's next turn and an answer from nothing: dropped.
            text = up_to_last_call(&text);
        }
        let shown = without_calls(&text);
        // Words before a call are the model thinking aloud; only the final
        // answer is the reply.
        if calls.is_empty() && !shown.is_empty() {
            if !visible.is_empty() {
                visible.push_str("\n\n");
            }
            visible.push_str(&shown);
        }
        if outcome != "ok" {
            return (outcome, visible, uses);
        }
        // Asked for a file and answered without making it (a small model
        // may only say it did): told once to call make_document now.
        if calls.is_empty()
            && !nudged
            && round + 1 < max_rounds
            && super::assistant_documents::wanted(question)
            && !uses
                .iter()
                .any(|u| u.tool == "make_document" && u.state == "done")
        {
            nudged = true;
            if let Some(list) = messages.as_array_mut() {
                list.push(serde_json::json!({"role": "assistant", "content": text}));
                list.push(serde_json::json!({"role": "user", "content": super::assistant_documents::NUDGE}));
            }
            // What it wrote stays the content to fall back on.
            nudge_text = std::mem::take(&mut visible);
            continue;
        }
        if calls.is_empty() || round + 1 == max_rounds {
            if !calls.is_empty() && visible.is_empty() {
                visible = shown;
            }
            ran_out = !calls.is_empty();
            break;
        }
        // A spoken question would otherwise hear silence while tools run.
        if speak && round == 0 {
            super::voice_mode::reply_piece(turn_id, "One moment, let me check. ");
        }
        compact(&mut messages);
        if let Some(list) = messages.as_array_mut() {
            list.push(serde_json::json!({"role": "assistant", "content": text}));
        }
        let mut responses = Vec::new();
        let mut seen = Vec::new();
        for (n, mut found) in calls.into_iter().take(MAX_CALLS_PER_ROUND).enumerate() {
            // A small model fills searches with the versions and years it
            // remembers; those the user never wrote are left out.
            if found.name == "web_search"
                && let Some(serde_json::Value::String(query)) = found.arguments.get_mut("query")
            {
                *query = super::assistant_builtin::unguessed(query, question);
            }
            if !found.complete {
                // Cut off at the reply's length: running it would write half
                // a file. The model is asked to do it in parts.
                responses.push(format!(
                    "<tool_response>\nYour {} call was cut off because the reply got too long, so it did not run. \
Do it in smaller parts: for a file, write_file with its first part (about 200 lines), then append_file for each \
further part.\n</tool_response>",
                    found.name
                ));
                continue;
            }
            let call_id = format!("{turn_id}-t{round}{n}");
            // A name or arguments the model got slightly wrong are repaired;
            // what cannot be is told back to it instead of failing blind.
            if let Some(i) = super::assistant_repair::tool_index(&found.name, &tools) {
                found.name.clone_from(&tools[i].name);
                match super::assistant_repair::arguments(&tools[i], &found.arguments) {
                    Ok(fixed) => found.arguments = fixed,
                    Err(problem) => {
                        let signature = format!("{}:{problem}", found.name);
                        if repeated(&mut failures, &signature) {
                            stuck = Some((found.name.clone(), problem.clone()));
                        }
                        uses.push(ToolUse {
                            server: tools[i].server_name.clone(),
                            tool: found.name.clone(),
                            arguments: serde_json::Value::Object(found.arguments.clone())
                                .to_string()
                                .chars()
                                .take(2000)
                                .collect(),
                            state: "failed".to_owned(),
                            summary: Some(problem.clone()),
                            detail: None,
                        });
                        responses.push(format!("<tool_response>\n{problem}\n</tool_response>"));
                        continue;
                    }
                }
            }
            let (result, record, picture) = run_call(
                state,
                turn_id,
                chat_id,
                &call_id,
                &found,
                &tools,
                &mut allowed,
                &scratch,
                &chat,
            );
            // The same call failing the same way twice: the loop stops rather
            // than spin.
            if record.state == "failed" {
                let signature = format!(
                    "{}:{}",
                    found.name,
                    serde_json::Value::Object(found.arguments.clone())
                );
                if repeated(&mut failures, &signature) {
                    stuck = Some((
                        found.name.clone(),
                        record.summary.clone().unwrap_or_default(),
                    ));
                }
            }
            uses.push(record);
            seen.extend(picture);
            responses.push(format!("<tool_response>\n{result}\n</tool_response>"));
        }
        if let Some(list) = messages.as_array_mut() {
            let mut message = serde_json::json!({"role": "user", "content": responses.join("\n")});
            if !seen.is_empty() {
                message["images"] = seen
                    .iter()
                    .map(|p| serde_json::Value::from(p.to_string_lossy().into_owned()))
                    .collect();
            }
            list.push(message);
        }
        if messages.to_string().len() > 120 * 1024 || stuck.is_some() {
            break;
        }
    }
    if let Some((tool, problem)) = &stuck {
        if !visible.is_empty() {
            visible.push_str("\n\n");
        }
        visible.push_str(&format!(
            "I stopped because the same step kept failing ({tool}: {problem}). Try saying it another way, \
or do that step yourself and tell me when it is done."
        ));
    }
    let _ = std::fs::remove_dir_all(&scratch);
    // Asked for a file, the model wrote it into the chat instead: the
    // daemon makes the file from it (D55), so the user still gets it.
    let made = uses
        .iter()
        .any(|u| u.tool == "make_document" && u.state == "done");
    if super::assistant_documents::wanted(question)
        && !made
        && let Some((arguments, outcome)) = super::assistant_documents::from_reply(
            chat_id,
            question,
            if super::assistant_documents::looks_like_document(&visible) {
                &visible
            } else {
                &nudge_text
            },
        )
    {
        let call_id = format!("{turn_id}-doc");
        let state_name = if outcome.failed { "failed" } else { "done" };
        let summary = summary_of(&outcome.text);
        let detail = outcome.detail.map(|d| d.to_string());
        publish(
            state,
            Event::AssistantTool {
                turn_id: turn_id.to_owned(),
                call_id,
                server: "Documents".to_owned(),
                tool: "make_document".to_owned(),
                arguments: arguments.to_string(),
                state: state_name.to_owned(),
                summary: Some(summary.clone()),
                detail: detail.clone(),
            },
        );
        uses.push(ToolUse {
            server: "Documents".to_owned(),
            tool: "make_document".to_owned(),
            arguments: arguments.to_string().chars().take(2000).collect(),
            state: state_name.to_owned(),
            summary: Some(summary),
            detail,
        });
        // The reply keeps only what came before the content: the file holds it.
        if !outcome.failed {
            visible = "Here is the document. Open it, download it, or preview it below.".to_owned();
        }
    }
    if ran_out {
        // Said plainly rather than ending mid-task without a word.
        if !visible.is_empty() {
            visible.push_str("\n\n");
        }
        visible.push_str(&format!(
            "I stopped after {max_rounds} steps, before finishing. Say \"continue\" to let me go on."
        ));
    }
    ("ok".to_owned(), visible, uses)
}

/// While the chat shares a screen live, looking at the screen is a copy of
/// what it shows now: no second screen-sharing dialog.
fn shared_screen(
    chat: &assistant_library::Chat,
    scratch: &std::path::Path,
) -> Option<super::assistant_builtin::Outcome> {
    let meeting = chat.live.as_deref()?;
    let (screens, _) = super::meetings::existing()?.live_view(meeting, 0)?;
    let first = screens.first()?;
    let copy = scratch.join("shared-screen.png");
    std::fs::copy(first, &copy).ok()?;
    Some(super::assistant_builtin::Outcome {
        text: "Here is the screen the user is sharing (the picture comes with this message)."
            .to_owned(),
        failed: false,
        detail: Some(serde_json::json!({"kind": "screen"})),
        picture: Some(copy),
    })
}

/// A reply whose long `content` parameters (files written) are left out.
fn without_written_text(content: &str) -> Option<String> {
    const OPEN: &str = "<parameter=content>";
    const CLOSE: &str = "</parameter>";
    let mut out = String::new();
    let mut rest = content;
    let mut changed = false;
    while let Some(start) = rest.find(OPEN) {
        let after = &rest[start + OPEN.len()..];
        let end = after.find(CLOSE).unwrap_or(after.len());
        out.push_str(&rest[..start + OPEN.len()]);
        if end > 1200 {
            out.push_str(&format!("\n[{end} bytes, written to the file]\n"));
            changed = true;
        } else {
            out.push_str(&after[..end]);
        }
        rest = &after[end..];
    }
    out.push_str(rest);
    changed.then_some(out)
}

/// A reply cut after its last tool call.
fn up_to_last_call(text: &str) -> String {
    text.rfind("</tool_call>").map_or_else(
        || text.to_owned(),
        |end| text[..end + "</tool_call>".len()].to_owned(),
    )
}

/// Whether a question asks for work on the computer (make, write, fix or
/// run something) rather than about the world: it is not searched first.
fn is_computer_task(question: &str) -> bool {
    let lower = question.to_lowercase();
    let doing = [
        "create", "make", "build", "write", "fix", "edit", "change", "update", "delete", "remove",
        "rename", "move", "copy", "run", "install", "open", "list", "find", "read", "check",
        "save", "add",
    ];
    let things = [
        "file",
        "folder",
        "page",
        "website",
        "site",
        "script",
        "code",
        "html",
        "css",
        "app",
        "program",
        "command",
        "document",
        "project",
        "directory",
    ];
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    words.iter().any(|w| doing.contains(w))
        && words
            .iter()
            .any(|w| things.iter().any(|t| w.starts_with(t)))
}

/// Whether a question is worth a web search before the model answers: not
/// a greeting or a thank-you.
fn worth_searching(question: &str) -> bool {
    let words = question.split_whitespace().count();
    let lower = question.to_lowercase();
    let small_talk = [
        "thank",
        "thanks",
        "hello",
        "hi ",
        "hey",
        "good morning",
        "bye",
        "okay",
        "ok ",
    ]
    .iter()
    .any(|w| lower.starts_with(w));
    words >= 4 && !small_talk
}

/// The chat's allowed commands as saved now (one allowed earlier in this
/// reply counts at once).
fn allowed_commands_of(chat_id: &str, chat: &assistant_library::Chat) -> Vec<String> {
    assistant_library::chat(chat_id)
        .map_or_else(|| chat.allowed_commands.clone(), |c| c.allowed_commands)
}

/// What the model is told in Plan mode.
const PLAN_MODE: &str = "\n\nThis chat is in Plan mode: look around with the reading tools, then \
write a short numbered plan of the changes and commands you would make. Change nothing and run \
nothing; the user switches the mode to carry the plan out.";

/// Where the computer tools work, as the model is told it.
fn working_folder(folder: Option<&str>) -> String {
    match folder.filter(|f| std::path::Path::new(f).is_dir()) {
        Some(folder) => format!(
            "\n\nYour working folder is {folder}. Work there unless the user names another place: \
paths that do not start with / or ~ are inside it, commands run in it, and new files go in it."
        ),
        None => {
            "\n\nNo working folder is set: paths that do not start with / are in the home folder, \
and commands run there."
                .to_owned()
        }
    }
}

/// What the model is told about working with the built-in tools.
fn agent_guidance(builtin: &[&str], speak: bool) -> String {
    let mut out = format!(
        "\n\nToday is {}. Work step by step: call a tool, read what comes back, and call \
another when you need more. ",
        today()
    );
    if builtin.contains(&super::assistant_builtin::WEB) {
        out.push_str(
            "You can search the web and read pages: never say you cannot browse or have no \
internet access. Your own knowledge stops well before today and is often out of date. For questions \
about the world, current events, versions, prices, people or anything you are not sure of, \
search the web first, without guessing versions, numbers or dates in the search words; then \
open the two or three most relevant pages (official pages and recent ones first) before \
answering. When pages and your memory disagree, the pages win; say what is newest by the dates \
on the pages. Answer from what the pages say",
        );
        out.push_str(if speak {
            ", and name the sites you used in a few words. "
        } else {
            ", and end with the sources as a Markdown list of links. "
        });
    }
    if builtin.contains(&super::assistant_builtin::COMPUTER) {
        out.push_str(
            "You can use the user's computer: look at folders and files, run commands, open \
things and look at the screen. Prefer reading before changing. For a coding task: look at the \
project first (list_folder, read_file), say your plan in a line, change code with edit_file (small \
exact replacements) rather than rewriting files, then run the build or tests with run_command and fix \
what fails until they pass. Keep going step by step until the task is done; the user can undo every \
file change from the chat. ",
        );
    }
    out
}

/// The date, from `date` (no time-zone crate in the daemon).
fn today() -> String {
    std::process::Command::new("date")
        .arg("+%A %-d %B %Y")
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|d| d.trim().to_owned())
        .unwrap_or_default()
}

/// Older tool results are shortened once the conversation grows, so the
/// newest ones keep room.
fn compact(messages: &mut serde_json::Value) {
    if messages.to_string().len() < 40 * 1024 {
        return;
    }
    let Some(list) = messages.as_array_mut() else {
        return;
    };
    let last = list.len().saturating_sub(2);
    for message in list.iter_mut().take(last) {
        let Some(content) = message.get("content").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if content.starts_with("<tool_response>") && content.len() > 1200 {
            let short: String = content.chars().take(1000).collect();
            message["content"] = format!("{short}\n[… shortened]\n</tool_response>").into();
            message.as_object_mut().map(|m| m.remove("images"));
        } else if message.get("role").and_then(serde_json::Value::as_str) == Some("assistant")
            && let Some(short) = without_written_text(content)
        {
            // A file the model wrote is on disk: it can read it again.
            message["content"] = short.into();
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_call(
    state: &DaemonState,
    turn_id: &str,
    chat_id: &str,
    call_id: &str,
    found: &Call,
    tools: &[Tool],
    allowed: &mut Vec<String>,
    scratch: &std::path::Path,
    chat: &assistant_library::Chat,
) -> (String, ToolUse, Option<std::path::PathBuf>) {
    let arguments = serde_json::Value::Object(found.arguments.clone());
    let tool = tools.iter().find(|t| t.name == found.name);
    let server = tool.map_or_else(String::new, |t| t.server_name.clone());
    let event =
        |state_name: &str, summary: Option<String>, detail: Option<String>| Event::AssistantTool {
            turn_id: turn_id.to_owned(),
            call_id: call_id.to_owned(),
            server: server.clone(),
            tool: found.name.clone(),
            arguments: arguments.to_string(),
            state: state_name.to_owned(),
            summary,
            detail,
        };
    let mut record = ToolUse {
        server: server.clone(),
        tool: found.name.clone(),
        arguments: arguments.to_string().chars().take(2000).collect(),
        state: String::new(),
        summary: None,
        detail: None,
    };
    let Some(tool) = tool else {
        record.state = "failed".to_owned();
        publish(
            state,
            event("failed", Some("No such tool".to_owned()), None),
        );
        return (
            format!(
                "There is no tool named {}. The tools are: {}.",
                found.name,
                tools
                    .iter()
                    .map(|t| t.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            record,
            None,
        );
    };
    let builtin = tool.server_id == super::assistant_builtin::WEB
        || tool.server_id == super::assistant_builtin::COMPUTER;
    let document = tool.server_id == super::assistant_documents::DOCUMENTS;
    // A document is written into the chat's own storage: nothing to ask.
    let folder = chat
        .folder
        .clone()
        .or_else(super::assistant::default_folder);
    let folder = folder.as_deref().map(std::path::Path::new);
    let outside = builtin
        .then(|| super::assistant_builtin::outside(&tool.server_id, &tool.name, &arguments, folder))
        .flatten();
    // A command the user allowed in this chat runs without asking (D56),
    // in the modes that ask only before changes.
    let mode = chat.permission();
    let asks_for_changes = matches!(
        mode,
        assistant_library::Permission::Auto | assistant_library::Permission::AcceptEdits
    );
    let command = (tool.server_id == super::assistant_builtin::COMPUTER
        && tool.name == "run_command")
        .then(|| {
            arguments
                .get("command")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned()
        });
    let allowed_command = asks_for_changes
        && command.as_deref().is_some_and(|c| {
            super::assistant_builtin::command_allowed(c, &allowed_commands_of(chat_id, chat))
        });
    let prefix = command
        .as_deref()
        .filter(|_| asks_for_changes)
        .and_then(super::assistant_builtin::command_prefix);
    let approval = if document || allowed_command {
        super::assistant_builtin::Approval::Never
    } else {
        // The chat's permission mode (Auto unless the user chose another).
        super::assistant_builtin::approval_in(
            chat.permission(),
            &tool.server_id,
            &tool.name,
            builtin,
            outside.is_some(),
        )
    };
    if approval == super::assistant_builtin::Approval::Denied {
        record.state = "refused".to_owned();
        publish(state, event("refused", Some("Plan mode".to_owned()), None));
        return (
            "This chat is in Plan mode: nothing is changed or run. Write the plan instead."
                .to_owned(),
            record,
            None,
        );
    }
    let key = format!("{}/{}", tool.server_id, tool.name);
    let ask = match approval {
        super::assistant_builtin::Approval::Never => false,
        super::assistant_builtin::Approval::Ask => !allowed.contains(&key),
        super::assistant_builtin::Approval::Always => true,
        super::assistant_builtin::Approval::Denied => false,
    };
    if ask {
        // The card offers "always in this chat" only where it is allowed.
        let offer = serde_json::json!({
            "kind": "ask",
            "always": approval != super::assistant_builtin::Approval::Always,
            "outside": outside.as_ref().map(|p| p.display().to_string()),
            "command": prefix,
        });
        publish(state, event("ask", None, Some(offer.to_string())));
        match ask_user(call_id) {
            Some((true, always)) => {
                if let (true, Some(prefix)) = (always, prefix.as_ref()) {
                    // "Always allow `cargo test`": kept with the chat.
                    let _ = assistant_library::update_chat(chat_id, |chat| {
                        if !chat.allowed_commands.contains(prefix) {
                            chat.allowed_commands.push(prefix.clone());
                        }
                    });
                } else if always && approval == super::assistant_builtin::Approval::Ask {
                    allowed.push(key.clone());
                    let _ = assistant_library::update_chat(chat_id, |chat| {
                        if !chat.allowed_tools.contains(&key) {
                            chat.allowed_tools.push(key.clone());
                        }
                    });
                }
            }
            _ => {
                record.state = "refused".to_owned();
                publish(state, event("refused", None, None));
                return ("The user refused this call.".to_owned(), record, None);
            }
        }
    }
    publish(state, event("running", None, None));
    let shared_now = (tool.name == "look_at_screen")
        .then(|| shared_screen(chat, scratch))
        .flatten();
    let result = if let Some(outcome) = shared_now {
        Ok((
            outcome.text,
            outcome.failed,
            outcome.detail,
            outcome.picture,
        ))
    } else if document {
        let outcome = super::assistant_documents::make(chat_id, &arguments);
        Ok((
            outcome.text,
            outcome.failed,
            outcome.detail,
            outcome.picture,
        ))
    } else if builtin {
        let outcome = super::assistant_builtin::call(
            &tool.server_id,
            &tool.name,
            &arguments,
            scratch,
            folder,
            chat_id,
        );
        Ok((
            outcome.text,
            outcome.failed,
            outcome.detail,
            outcome.picture,
        ))
    } else {
        call(tool, &arguments).map(|(text, failed)| (text, failed, None, None))
    };
    match result {
        Ok((text, failed, detail, picture)) => {
            let summary = summary_of(&text);
            let detail = detail.map(|d| d.to_string());
            record.state = if failed { "failed" } else { "done" }.to_owned();
            record.summary = Some(summary.clone());
            record.detail = detail.clone().filter(|d| d.len() < 16 * 1024);
            publish(state, event(&record.state, Some(summary), detail));
            let mut text = text;
            if text.len() > RESULT_BYTES {
                let mut cut = RESULT_BYTES;
                while !text.is_char_boundary(cut) {
                    cut -= 1;
                }
                text.truncate(cut);
                text.push_str("\n[…]");
            }
            (text, record, picture)
        }
        Err(code) => {
            record.state = "failed".to_owned();
            record.summary = Some(code.to_owned());
            publish(state, event("failed", Some(code.to_owned()), None));
            (format!("The tool failed ({code})."), record, None)
        }
    }
}

/// The first line of a result, short, for the card.
fn summary_of(text: &str) -> String {
    let line = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or_default()
        .trim();
    let lines = text.lines().filter(|l| !l.trim().is_empty()).count();
    let mut short: String = line.chars().take(100).collect();
    if line.chars().count() > 100 {
        short.push('…');
    }
    if lines > 1 {
        short.push_str(&format!(" (+{} lines)", lines - 1));
    }
    short
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_web_and_the_screen_never_ask_and_changes_always_do() {
        use super::super::assistant_builtin::{Approval, COMPUTER, WEB, approval};
        assert!(approval(WEB, "web_search") == Approval::Never);
        #[cfg(not(windows))]
        assert!(approval(COMPUTER, "look_at_screen") == Approval::Never);
        // Windows has no sharing dialog: every look asks.
        #[cfg(windows)]
        assert!(approval(COMPUTER, "look_at_screen") == Approval::Always);
        assert!(approval(COMPUTER, "read_file") == Approval::Ask);
        assert!(approval(COMPUTER, "run_command") == Approval::Always);
    }

    #[test]
    fn a_call_written_as_json_is_read_and_a_repeat_failure_is_caught() {
        let reply = "<tool_call>\n{\"name\": \"run_command\", \"arguments\": {\"command\": \"ls\"}}\n</tool_call>";
        let calls = calls_in(reply);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "run_command");
        assert!(calls[0].complete);
        let mut failures = HashMap::new();
        assert!(!repeated(&mut failures, "edit_file:x"));
        assert!(!repeated(&mut failures, "edit_file:y"));
        assert!(repeated(&mut failures, "edit_file:x"));
    }

    #[test]
    fn words_after_the_last_call_are_dropped() {
        let reply = "<tool_call>\n<function=open_page>\n<parameter=url>\nhttps://a.b/\n</parameter>\n\
</function>\n</tool_call>\nuser\nThe answer is 42.";
        let cut = up_to_last_call(reply);
        assert!(cut.ends_with("</tool_call>"));
        assert_eq!(calls_in(&cut).len(), 1);
        assert_eq!(without_calls(&cut), "");
    }

    #[test]
    fn small_talk_is_not_searched() {
        assert!(worth_searching(
            "What is the latest stable version of Python?"
        ));
        assert!(!worth_searching("Thanks, that helps a lot"));
        assert!(!worth_searching("ok"));
    }

    #[test]
    fn a_spoken_answer_names_its_sources_instead_of_listing_links() {
        let web = [super::super::assistant_builtin::WEB];
        assert!(agent_guidance(&web, false).contains("Markdown list of links"));
        let spoken = agent_guidance(&web, true);
        assert!(!spoken.contains("Markdown"));
        assert!(spoken.contains("name the sites"));
    }

    #[test]
    fn calls_are_read_in_qwens_format() {
        let reply = "Let me look.\n<tool_call>\n<function=read_file>\n<parameter=path>\n/tmp/a b.txt\n\
</parameter>\n<parameter=limit>\n20\n</parameter>\n</function>\n</tool_call>";
        let calls = calls_in(reply);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "read_file");
        assert_eq!(calls[0].arguments["path"], "/tmp/a b.txt");
        assert_eq!(calls[0].arguments["limit"], 20);
        assert_eq!(without_calls(reply), "Let me look.");
    }

    #[test]
    fn questions_about_the_world_are_searched_but_computer_tasks_are_not() {
        assert!(is_computer_task(
            "Can you create me a single website about computer repair?"
        ));
        assert!(is_computer_task("fix the css in index.html"));
        assert!(!is_computer_task("What is the latest version of Rust?"));
        assert!(!is_computer_task("Who won the match yesterday?"));
    }

    #[test]
    fn a_call_cut_off_is_not_complete_and_written_files_are_left_out_later() {
        let whole = "<tool_call>\n<function=write_file>\n<parameter=path>\na.html\n</parameter>\n\
<parameter=content>\n<html>\n</parameter>\n</function>\n</tool_call>";
        assert!(calls_in(whole)[0].complete);
        let cut = "<tool_call>\n<function=write_file>\n<parameter=path>\na.html\n</parameter>\n\
<parameter=content>\n<html><body>";
        assert!(!calls_in(cut)[0].complete);
        let long = format!(
            "<tool_call>\n<function=write_file>\n<parameter=path>\na.html\n</parameter>\n\
<parameter=content>\n{}\n</parameter>\n</function>\n</tool_call>",
            "x".repeat(5000)
        );
        let short = without_written_text(&long).unwrap_or_default();
        assert!(short.len() < 400, "{short}");
        assert!(short.contains("a.html") && short.ends_with("</tool_call>"));
        assert!(without_written_text(whole).is_none());
    }

    #[test]
    fn a_reply_without_calls_is_kept_whole() {
        assert!(calls_in("No tools needed.").is_empty());
        assert_eq!(without_calls("No tools needed."), "No tools needed.");
        assert_eq!(without_calls("Cut <tool_call>\n<function=x>"), "Cut");
    }

    #[test]
    fn summaries_are_short() {
        assert_eq!(summary_of("first\nsecond\nthird"), "first (+2 lines)");
        assert_eq!(summary_of(&"x".repeat(150)).chars().count(), 101);
    }
}
