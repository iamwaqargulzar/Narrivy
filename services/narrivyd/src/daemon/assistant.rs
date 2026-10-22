//! The AI assistant extension (D33, F24): the daemon side.
//!
//! The model runs in its own `narrivy-openvino-llm-worker` process, started
//! when the assistant is first used and stopped after it has been idle for
//! [`IDLE_UNLOAD`] (D31: the AI model is loaded on demand). One turn runs at
//! a time on the assistant's own thread, which also owns the worker (a
//! worker's parent-death signal follows the thread that spawned it). The
//! reply streams to clients as events; the daemon keeps no conversation and
//! never logs text.

use std::ffi::OsString;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use narrivy_asr_openvino::refine_protocol::{
    self, Frame, OP_CANCEL, OP_CHAT, OP_SHUTDOWN, STATUS_CANCELLED, STATUS_OK, STATUS_PARTIAL,
    STATUS_READY,
};
use narrivy_ipc::AssistantMessageDto;
use narrivy_process_linux::{HelperEnvironment, VerifiedArtifactExecutable};

use super::session::{DaemonEvent, DaemonState};

/// The assistant model pack directory (until Models → Language models
/// installs it, D32).
pub const MODEL_DIR_ENV: &str = "NARRIVY_ASSISTANT_MODEL_DIR";
/// `auto`, `npu`, `gpu` or `cpu`. Defaults to `auto`: the GPU, else the CPU,
/// for the vision-language models the assistant uses (the worker skips the
/// NPU for them); measured on the 288V, 22 tokens/s on the GPU against 8 on
/// the CPU.
pub const COMPUTE_ENV: &str = "NARRIVY_ASSISTANT_COMPUTE";

const LOAD_BOUND: Duration = Duration::from_secs(900);
/// A turn that produces nothing for this long is abandoned.
const PIECE_BOUND: Duration = Duration::from_secs(120);
/// The model is unloaded after this long without a turn.
const IDLE_UNLOAD: Duration = Duration::from_secs(10 * 60);
/// With auto-unload off, how often the idle wait looks at the setting.
const KEEP_CHECK: Duration = Duration::from_secs(60);

/// Used when the conversation brings no system message of its own.
const DEFAULT_SYSTEM: &str = "You are the assistant in Narrivy, a dictation app. You run \
entirely on the user's computer and have no internet access. Answer helpfully and concisely. \
Use Markdown for lists and code.";

/// The worker's stdin, the worker's id for the running turn, and its turn id.
type Running = Arc<Mutex<Option<(Arc<Mutex<ChildStdin>>, u64, String)>>>;

struct Job {
    turn_id: String,
    messages: String,
    /// A writing action's reply is tidied and checked against this.
    guard: Option<super::writing::Guard>,
    /// Also hand the finished reply (outcome, text) to this caller.
    reply: Option<Sender<(String, String)>>,
    /// Publish nothing: a step of a larger job (meeting notes in parts).
    silent: bool,
    /// Save the reply into this kept chat (the assistant library).
    chat: Option<String>,
    /// Read the reply aloud (voice mode).
    speak: bool,
    /// Publish `AssistantDone` at the end (a tool round leaves that to
    /// the round's owner).
    done: bool,
}

/// The finished reply of a turn asked through [`Assistant::ask`].
pub type Reply = Receiver<(String, String)>;

enum Request {
    Turn(Job),
    /// Stop the model now to free its memory; the next turn loads it again.
    Unload,
    /// Load the model now, ready for the first question.
    Load,
}

/// Whether the model is unloaded after [`IDLE_UNLOAD`] without a question
/// (the default), or kept loaded until the user unloads it. Kept in
/// `<state>/assistant-model.json`.
#[must_use]
pub fn auto_unload() -> bool {
    model_settings()
        .get("autoUnload")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true)
}

fn model_settings_path() -> Option<PathBuf> {
    narrivy_platform_linux::instance::state_dir()
        .ok()
        .map(|dir| dir.join("assistant-model.json"))
}

fn model_settings() -> serde_json::Value {
    model_settings_path()
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_else(|| serde_json::json!({}))
}

/// Keep the model loaded (`false`) or unload it when idle (`true`).
///
/// # Errors
/// `assistant_settings_unavailable` when it cannot be saved.
pub fn set_auto_unload(on: bool) -> Result<(), &'static str> {
    let path = model_settings_path().ok_or("assistant_settings_unavailable")?;
    let mut settings = model_settings();
    settings["autoUnload"] = on.into();
    std::fs::write(path, settings.to_string()).map_err(|_| "assistant_settings_unavailable")
}

/// The folder the computer tools work in for chats that did not choose
/// their own; `None` is the home folder.
#[must_use]
pub fn default_folder() -> Option<String> {
    model_settings()
        .get("folder")
        .and_then(serde_json::Value::as_str)
        .filter(|f| std::path::Path::new(f).is_absolute())
        .map(str::to_owned)
}

/// Set (an absolute folder) or clear (`None`) the default working folder.
///
/// # Errors
/// `assistant_folder_invalid` for a folder that is not absolute or does not
/// exist; `assistant_settings_unavailable` when it cannot be saved.
pub fn set_default_folder(folder: Option<&str>) -> Result<(), &'static str> {
    if let Some(folder) = folder
        && !(std::path::Path::new(folder).is_absolute() && std::path::Path::new(folder).is_dir())
    {
        return Err("assistant_folder_invalid");
    }
    let path = model_settings_path().ok_or("assistant_settings_unavailable")?;
    let mut settings = model_settings();
    settings["folder"] = folder.map_or(serde_json::Value::Null, Into::into);
    std::fs::write(path, settings.to_string()).map_err(|_| "assistant_settings_unavailable")
}

/// The model's settings and state for clients, as JSON.
#[must_use]
pub fn model_json() -> String {
    serde_json::json!({
        "autoUnload": auto_unload(),
        "folder": default_folder(),
        "state": current_state(),
        "configured": current_dir().is_some(),
    })
    .to_string()
}

/// Where the model is loaded (`npu`, `gpu`, `cpu`), or `None` when it is not.
type Loaded = Arc<Mutex<Option<&'static str>>>;

/// The assistant, shared by every client.
pub struct Assistant {
    jobs: Sender<Request>,
    /// The worker's stdin and the id of the turn running, for cancels.
    running: Running,
    loaded: Loaded,
}

/// Published while the assistant's model is in memory.
pub const LOADED_CAPABILITY: &str = "assistant.loaded";

struct Current {
    initialised: bool,
    active: Option<(PathBuf, Arc<Assistant>)>,
}

static CURRENT: Mutex<Current> = Mutex::new(Current {
    initialised: false,
    active: None,
});

/// The assistant for the selected model, started on first use. Errors when
/// no assistant model is selected.
pub fn shared(state: &Arc<DaemonState>) -> Result<Arc<Assistant>, &'static str> {
    let mut current = CURRENT.lock().map_err(|_| "assistant_unavailable")?;
    if !current.initialised {
        current.initialised = true;
        if let Some(dir) =
            super::language_models::selected_dir(super::language_models::Role::Assistant)
        {
            current.active = Some((dir.clone(), Arc::new(start(&dir, state)?)));
        }
    }
    current
        .active
        .as_ref()
        .map(|(_, assistant)| Arc::clone(assistant))
        .ok_or("assistant_not_configured")
}

/// The selected assistant model's directory.
#[must_use]
pub fn current_dir() -> Option<PathBuf> {
    let current = CURRENT.lock().ok()?;
    if current.initialised {
        current.active.as_ref().map(|(dir, _)| dir.clone())
    } else {
        super::language_models::selected_dir(super::language_models::Role::Assistant)
    }
}

/// Switch to another assistant model (or none). A reply in progress
/// finishes on the old model, which then stops.
pub fn reconfigure(dir: Option<PathBuf>, state: &Arc<DaemonState>) -> Result<(), &'static str> {
    let mut current = CURRENT.lock().map_err(|_| "assistant_unavailable")?;
    current.initialised = true;
    if current.active.as_ref().map(|(active, _)| active) == dir.as_ref() {
        return Ok(());
    }
    current.active = match dir {
        Some(dir) => Some((dir.clone(), Arc::new(start(&dir, state)?))),
        None => None,
    };
    Ok(())
}

/// Start the assistant thread for the model in `dir`; the model itself
/// loads with the first turn.
fn start(dir: &Path, state: &Arc<DaemonState>) -> Result<Assistant, &'static str> {
    let launch = Launch::new(dir)?;
    let (jobs, queue) = mpsc::channel();
    let running = Arc::new(Mutex::new(None));
    let loaded: Loaded = Arc::new(Mutex::new(None));
    let owner_running = Arc::clone(&running);
    let owner_loaded = Arc::clone(&loaded);
    let state = Arc::clone(state);
    std::thread::Builder::new()
        .name("narrivy-assistant".into())
        .spawn(move || serve(&launch, &queue, &owner_running, &owner_loaded, &state))
        .map_err(|_| "assistant_thread_failed")?;
    Ok(Assistant {
        jobs,
        running,
        loaded,
    })
}

/// Where the current assistant model is loaded, without starting anything:
/// `loaded:<device>` or `unloaded`; `None` without an assistant model.
#[must_use]
pub fn current_state() -> Option<String> {
    let current = CURRENT.lock().ok()?;
    if !current.initialised {
        return current_dir_unlocked().map(|_| "unloaded".to_owned());
    }
    let (_, assistant) = current.active.as_ref()?;
    Some(match assistant.loaded() {
        Some(device) => format!("loaded:{device}"),
        None => "unloaded".to_owned(),
    })
}

fn current_dir_unlocked() -> Option<PathBuf> {
    super::language_models::selected_dir(super::language_models::Role::Assistant)
}

/// What the worker is sent for a turn: the messages, or, when the model
/// is to think first (`short` or `deep`), the messages with that asked of
/// it and `"thinking": true` (the worker switches the template's thinking
/// on; the NPU's runner does not think).
#[must_use]
pub fn request(messages: &serde_json::Value, thinking: Option<&str>) -> String {
    let note = match thinking {
        Some("short") => "Think briefly before answering: a few short sentences of reasoning at most.",
        Some("deep") => {
            "Think it through carefully before answering: work step by step, consider other \
readings of the question, and check your reasoning."
        }
        _ => return messages.to_string(),
    };
    let mut messages = messages.clone();
    if let Some(list) = messages.as_array_mut() {
        match list.first_mut() {
            Some(first) if first.get("role").and_then(serde_json::Value::as_str) == Some("system") => {
                let content = first.get("content").and_then(serde_json::Value::as_str).unwrap_or_default();
                first["content"] = format!("{content}\n\n{note}").into();
            }
            _ => list.insert(0, serde_json::json!({"role": "system", "content": note})),
        }
    }
    serde_json::json!({"messages": messages, "thinking": true}).to_string()
}

impl Assistant {
    /// Queue one turn. The reply arrives as events carrying `turn_id`.
    pub fn send(
        &self,
        turn_id: String,
        messages: &[AssistantMessageDto],
    ) -> Result<(), &'static str> {
        let mut list: Vec<serde_json::Value> = Vec::with_capacity(messages.len() + 1);
        if messages
            .first()
            .is_none_or(|message| message.role != "system")
        {
            list.push(serde_json::json!({"role": "system", "content": DEFAULT_SYSTEM}));
        }
        list.extend(
            messages.iter().map(
                |message| serde_json::json!({"role": message.role, "content": message.content}),
            ),
        );
        let messages = serde_json::Value::Array(list).to_string();
        if messages.len() > refine_protocol::MAX_TEXT_BYTES {
            return Err("assistant_conversation_too_long");
        }
        self.jobs
            .send(Request::Turn(Job {
                turn_id,
                messages,
                guard: None,
                reply: None,
                silent: false,
                chat: None,
                speak: false,
                done: true,
            }))
            .map_err(|_| "assistant_unavailable")
    }

    /// Queue a question asked into a kept chat: the library saves it, builds
    /// what the model sees, and saves the reply when it ends. A spoken
    /// question's reply is read aloud in voice mode.
    pub fn send_in_chat(
        &self,
        turn_id: String,
        chat_id: String,
        question: &str,
        pictures: &[String],
        speak: bool,
        live: Option<&str>,
    ) -> Result<(), &'static str> {
        let prepared = super::assistant_library::prepare_with(
            &chat_id, &turn_id, question, pictures, speak, None, live,
        )?;
        // A spoken answer is read as it comes: no thinking first.
        let thinking = (!speak)
            .then(|| super::assistant_library::chat(&chat_id).and_then(|chat| chat.thinking))
            .flatten();
        let messages = request(&prepared.messages, thinking.as_deref());
        if messages.len() > refine_protocol::MAX_TEXT_BYTES {
            super::assistant_library::finish(
                &chat_id,
                &turn_id,
                "failed:assistant_conversation_too_long",
                "",
            );
            return Err("assistant_conversation_too_long");
        }
        if speak {
            super::voice_mode::reply_started(&turn_id);
        }
        self.jobs
            .send(Request::Turn(Job {
                turn_id,
                messages,
                guard: None,
                reply: None,
                silent: false,
                chat: Some(chat_id),
                speak,
                done: true,
            }))
            .map_err(|_| "assistant_unavailable")
    }

    /// Queue one round of a reply with tools: the pieces stream to clients
    /// under `turn_id`, but the round's end is only handed back.
    pub fn ask_streaming(
        &self,
        turn_id: &str,
        messages: &serde_json::Value,
        thinking: Option<&str>,
    ) -> Result<Reply, &'static str> {
        let messages = request(messages, thinking);
        if messages.len() > refine_protocol::MAX_TEXT_BYTES {
            return Err("assistant_conversation_too_long");
        }
        let (tx, rx) = mpsc::channel();
        self.jobs
            .send(Request::Turn(Job {
                turn_id: turn_id.to_owned(),
                messages,
                guard: None,
                reply: Some(tx),
                silent: false,
                chat: None,
                speak: false,
                done: false,
            }))
            .map_err(|_| "assistant_unavailable")?;
        Ok(rx)
    }

    /// Queue a turn built by the daemon itself (`messages` as JSON chat
    /// messages) and get its finished reply back. With `turn_id` the reply
    /// also streams to clients like any turn; without it nothing is
    /// published.
    pub fn ask(
        &self,
        turn_id: Option<String>,
        messages: &serde_json::Value,
    ) -> Result<Reply, &'static str> {
        let messages = messages.to_string();
        if messages.len() > refine_protocol::MAX_TEXT_BYTES {
            return Err("assistant_conversation_too_long");
        }
        let (tx, rx) = mpsc::channel();
        let silent = turn_id.is_none();
        self.jobs
            .send(Request::Turn(Job {
                turn_id: turn_id.unwrap_or_default(),
                messages,
                guard: None,
                reply: Some(tx),
                silent,
                chat: None,
                speak: false,
                done: true,
            }))
            .map_err(|_| "assistant_unavailable")?;
        Ok(rx)
    }

    /// Queue a writing action (D26): `text` edited as `action` says. The
    /// reply streams like a turn's; the finished text is tidied and checked.
    pub fn send_writing(
        &self,
        turn_id: String,
        action: super::writing::Action,
        text: String,
        instruction: Option<&str>,
    ) -> Result<(), &'static str> {
        let system = super::writing::system_prompt(action, instruction)?;
        let messages = serde_json::json!([
            {"role": "system", "content": system},
            {"role": "user", "content": text},
        ])
        .to_string();
        if messages.len() > refine_protocol::MAX_TEXT_BYTES {
            return Err("writing_text_too_long");
        }
        self.jobs
            .send(Request::Turn(Job {
                turn_id,
                messages,
                guard: Some(super::writing::Guard {
                    action,
                    original: text,
                }),
                reply: None,
                silent: false,
                chat: None,
                speak: false,
                done: true,
            }))
            .map_err(|_| "assistant_unavailable")
    }

    /// Load the model now (it stays loaded while auto-unload is off).
    pub fn load(&self) {
        let _ = self.jobs.send(Request::Load);
    }

    /// Free the model's memory now. A reply in progress finishes first.
    pub fn unload(&self) {
        let _ = self.jobs.send(Request::Unload);
    }

    /// Where the model is loaded, if it is.
    #[must_use]
    pub fn loaded(&self) -> Option<&'static str> {
        self.loaded.lock().ok().and_then(|loaded| *loaded)
    }

    /// Stop the reply to `turn_id`, if it is the one running.
    pub fn cancel(&self, turn_id: &str) {
        let Ok(running) = self.running.lock() else {
            return;
        };
        if let Some((stdin, id, turn)) = running.as_ref()
            && turn == turn_id
            && let Ok(mut stdin) = stdin.lock()
        {
            let _ = refine_protocol::write_frame(&mut *stdin, OP_CANCEL, *id, &[]);
        }
    }
}

struct Worker {
    stdin: Arc<Mutex<ChildStdin>>,
    replies: Receiver<Frame>,
    child: std::process::Child,
    next_id: u64,
}

impl Drop for Worker {
    fn drop(&mut self) {
        if let Ok(mut stdin) = self.stdin.lock() {
            let _ = refine_protocol::write_frame(&mut *stdin, OP_SHUTDOWN, 0, &[]);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The assistant thread: run turns one at a time, start the worker when a
/// turn needs it, and stop it when idle.
fn serve(
    launch: &Launch,
    queue: &Receiver<Request>,
    running: &Running,
    loaded: &Loaded,
    state: &Arc<DaemonState>,
) {
    let mut worker: Option<Worker> = None;
    let set_loaded = |device: Option<&'static str>| {
        if let Ok(mut slot) = loaded.lock() {
            *slot = device;
        }
        state.publish_assistant(DaemonEvent::Capability {
            capability: LOADED_CAPABILITY,
            available: device.is_some(),
        });
    };
    loop {
        // With auto-unload off the model stays until it is unloaded; the
        // wait still wakes now and then to see the setting change.
        let idle = auto_unload();
        let job = match queue.recv_timeout(if idle { IDLE_UNLOAD } else { KEEP_CHECK }) {
            Ok(Request::Turn(job)) => job,
            Ok(Request::Unload) => {
                if worker.take().is_some() {
                    set_loaded(None);
                }
                continue;
            }
            Err(RecvTimeoutError::Timeout) => {
                if idle && worker.take().is_some() {
                    set_loaded(None);
                }
                continue;
            }
            Ok(Request::Load) => {
                if worker.is_none() {
                    state.publish_assistant_loading(true);
                    let started = launch.start();
                    state.publish_assistant_loading(false);
                    match started {
                        Ok((started, device)) => {
                            worker = Some(started);
                            set_loaded(Some(device));
                        }
                        Err(code) => eprintln!("narrivyd: assistant model unavailable ({code})"),
                    }
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => return,
        };
        if worker.is_none() {
            state.publish_assistant_loading(true);
            let started = launch.start();
            state.publish_assistant_loading(false);
            match started {
                Ok((started, device)) => {
                    worker = Some(started);
                    set_loaded(Some(device));
                }
                Err(code) => {
                    eprintln!("narrivyd: assistant model unavailable ({code})");
                    if let Some(reply) = &job.reply {
                        let _ = reply.send((format!("failed:{code}"), String::new()));
                    }
                    conclude(&job, &format!("failed:{code}"), "");
                    if !job.silent && job.done {
                        state.publish_assistant(DaemonEvent::AssistantDone {
                            turn_id: job.turn_id,
                            outcome: format!("failed:{code}"),
                            text: String::new(),
                            warning: None,
                        });
                    }
                    continue;
                }
            }
        }
        let Some(active) = worker.as_mut() else {
            continue;
        };
        if !run_turn(active, &job, running, state) {
            // The worker broke; the next turn starts a new one.
            worker = None;
            set_loaded(None);
        }
    }
}

/// Run one turn. Returns false when the worker is no longer usable.
fn run_turn(worker: &mut Worker, job: &Job, running: &Running, state: &Arc<DaemonState>) -> bool {
    worker.next_id += 1;
    let id = worker.next_id;
    if let Ok(mut slot) = running.lock() {
        *slot = Some((Arc::clone(&worker.stdin), id, job.turn_id.clone()));
    }
    let sent = worker.stdin.lock().is_ok_and(|mut stdin| {
        refine_protocol::write_frame(&mut *stdin, OP_CHAT, id, job.messages.as_bytes()).is_ok()
    });
    let mut usable = sent;
    let mut reply = String::new();
    let outcome = if sent {
        loop {
            match worker.replies.recv_timeout(PIECE_BOUND) {
                Ok(frame) if frame.id != id => {}
                Ok(frame) if frame.code == STATUS_PARTIAL => {
                    let piece = String::from_utf8_lossy(&frame.payload).into_owned();
                    reply.push_str(&piece);
                    if job.speak {
                        super::voice_mode::reply_piece(&job.turn_id, &piece);
                    }
                    if !job.silent {
                        state.publish_assistant(DaemonEvent::AssistantDelta {
                            turn_id: job.turn_id.clone(),
                            text: piece,
                        });
                    }
                }
                Ok(frame) if frame.code == STATUS_OK => {
                    reply = String::from_utf8_lossy(&frame.payload).into_owned();
                    break "ok".to_owned();
                }
                Ok(frame) if frame.code == STATUS_CANCELLED => {
                    reply = String::from_utf8_lossy(&frame.payload).into_owned();
                    break "cancelled".to_owned();
                }
                Ok(_) => break "failed:assistant_failed".to_owned(),
                Err(_) => {
                    usable = false;
                    break "failed:assistant_timeout".to_owned();
                }
            }
        }
    } else {
        "failed:assistant_worker_lost".to_owned()
    };
    if let Ok(mut slot) = running.lock() {
        *slot = None;
    }
    let (text, warning) = match &job.guard {
        Some(guard) if outcome == "ok" => super::writing::finish(guard, &reply),
        _ => (reply, None),
    };
    if let Some(reply) = &job.reply {
        let _ = reply.send((outcome.clone(), text.clone()));
    }
    conclude(job, &outcome, &text);
    if !job.silent && job.done {
        state.publish_assistant(DaemonEvent::AssistantDone {
            turn_id: job.turn_id.clone(),
            outcome,
            text,
            warning,
        });
    }
    usable
}

/// Save a finished reply into its chat and end its speech.
fn conclude(job: &Job, outcome: &str, text: &str) {
    if let Some(chat) = &job.chat {
        super::assistant_library::finish(chat, &job.turn_id, outcome, text);
    }
    if job.speak {
        super::voice_mode::reply_done(&job.turn_id, outcome);
    }
}

struct Launch {
    executable: VerifiedArtifactExecutable,
    arguments: Vec<OsString>,
}

impl Launch {
    fn new(model_dir: &Path) -> Result<Self, &'static str> {
        let model_dir = model_dir.to_path_buf();
        if !model_dir.is_absolute() || !model_dir.is_dir() {
            return Err("assistant_model_missing");
        }
        let executable = worker()?;
        let speech_model = PathBuf::from(
            std::env::var_os("NARRIVY_TRANSCRIBE_MODEL").ok_or("assistant_not_configured")?,
        );
        let models_dir = super::model_selection::models_directory(&speech_model)
            .ok_or("assistant_not_configured")?;
        let mut arguments = super::openvino::leading_args(&models_dir, &model_dir)
            .ok_or("assistant_runtime_missing")?;
        let compute = std::env::var(COMPUTE_ENV).unwrap_or_else(|_| "auto".to_owned());
        if !matches!(compute.as_str(), "auto" | "npu" | "gpu" | "cpu") {
            return Err("assistant_compute_invalid");
        }
        arguments.push("--compute".into());
        arguments.push(compute.into());
        Ok(Self {
            executable,
            arguments,
        })
    }

    fn start(&self) -> Result<(Worker, &'static str), &'static str> {
        let mut command = self
            .executable
            .command(HelperEnvironment::ResidentAsr)
            .map_err(|_| "assistant_worker_invalid")?;
        command
            .args(&self.arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command
            .spawn()
            .map_err(|_| "assistant_worker_spawn_failed")?;
        let stdin = child.stdin.take().ok_or("assistant_worker_spawn_failed")?;
        let stdout = child.stdout.take().ok_or("assistant_worker_spawn_failed")?;
        let (sender, replies) = mpsc::channel();
        std::thread::Builder::new()
            .name("narrivy-assistant-read".into())
            .spawn(move || {
                let mut reader = BufReader::new(stdout);
                while let Ok(Some(frame)) = refine_protocol::read_frame(&mut reader) {
                    if sender.send(frame).is_err() {
                        break;
                    }
                }
            })
            .map_err(|_| "assistant_worker_spawn_failed")?;
        let worker = Worker {
            stdin: Arc::new(Mutex::new(stdin)),
            replies,
            child,
            next_id: 0,
        };
        let ready = worker
            .replies
            .recv_timeout(LOAD_BOUND)
            .map_err(|_| "assistant_load_timeout")?;
        if ready.code != STATUS_READY {
            return Err("assistant_load_failed");
        }
        let device = match ready.payload.as_slice() {
            b"NPU" => "npu",
            b"GPU" => "gpu",
            _ => "cpu",
        };
        Ok((worker, device))
    }
}

fn worker() -> Result<VerifiedArtifactExecutable, &'static str> {
    // The same language-model worker as clean-up.
    let path = std::env::var_os(super::refine::WORKER_ENV).ok_or("assistant_worker_missing")?;
    let digest =
        std::env::var(super::refine::WORKER_SHA256_ENV).map_err(|_| "assistant_worker_missing")?;
    let size = std::env::var(super::refine::WORKER_SIZE_ENV)
        .ok()
        .and_then(|size| size.parse::<u64>().ok())
        .ok_or("assistant_worker_missing")?;
    VerifiedArtifactExecutable::bind_controlled(Path::new(&path), &digest, size)
        .map_err(|_| "assistant_worker_invalid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thinking_turn_asks_the_worker_to_think() {
        let messages = serde_json::json!([{"role": "system", "content": "Be kind."}, {"role": "user", "content": "q"}]);
        assert_eq!(request(&messages, None), messages.to_string());
        assert_eq!(request(&messages, Some("off")), messages.to_string());
        let deep: serde_json::Value = serde_json::from_str(&request(&messages, Some("deep"))).unwrap_or_default();
        assert_eq!(deep["thinking"], true);
        let system = deep["messages"][0]["content"].as_str().unwrap_or_default();
        assert!(system.starts_with("Be kind.") && system.contains("step by step"));
        let bare = serde_json::json!([{"role": "user", "content": "q"}]);
        let short: serde_json::Value = serde_json::from_str(&request(&bare, Some("short"))).unwrap_or_default();
        assert_eq!(short["messages"][0]["role"], "system");
    }
}
