// Typed wrapper over the Narrivy Tauri bridge.
// Per CONTRACT.md this is the ONLY module that imports @tauri-apps/api.
// Surface mirrors apps/narrivy-desktop/src-tauri/BRIDGE-NOTES.md.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export const SESSION_EVENT = "session-event";

/** Meeting options; the device fields come from protocol minor 43. */
export interface MeetingOptions {
  auto_notes: boolean;
  /** Whether the call's sound (the output) is recorded too. */
  call_audio?: boolean;
  /** Whether the microphone is recorded (not both off). */
  room_audio?: boolean;
  /** The chosen microphone; "" follows the Microphone stage. */
  microphone?: string;
  /** The chosen output (the call's sound); "" is the system default. */
  speaker?: string;
  microphones?: Array<{ name: string; description: string }>;
  speakers?: Array<{ name: string; description: string }>;
  /** Recording now, or what a meeting started now would record. */
  microphone_in_use?: string;
  speaker_in_use?: string;
  /** Keep pictures of what is shown on screen (daemon minor 45). */
  screen?: boolean;
  /** Whether this computer can keep the screen. */
  screen_available?: boolean;
  /** Go over a recording as soon as it stops; off keeps it as heard until
   * asked (daemon minor 52; older daemons always go over it). */
  process_on_stop?: boolean;
}

/** A picture of the screen kept during a meeting. */
export interface MeetingSlide {
  file: string;
  at_ms: number;
  /** What the model saw; empty until described. */
  description: string;
}

export interface BridgeError {
  code: string;
  message: string;
}

export function toBridgeError(raw: unknown): BridgeError {
  if (typeof raw === "string") {
    try {
      const parsed = JSON.parse(raw) as Partial<BridgeError>;
      if (parsed && typeof parsed.code === "string") {
        return {
          code: parsed.code,
          message: typeof parsed.message === "string" ? parsed.message : raw,
        };
      }
    } catch {
      // plain string payload
    }
    return {
      code: raw === "daemon_unavailable" ? "daemon_unavailable" : "unknown",
      message: raw,
    };
  }
  if (raw && typeof raw === "object") {
    const o = raw as Record<string, unknown>;
    return {
      code: typeof o.code === "string" ? o.code : "unknown",
      message: typeof o.message === "string" ? o.message : String(raw),
    };
  }
  return { code: "unknown", message: String(raw) };
}

export interface ActiveSessionInfo {
  session_id?: string;
  transaction_id?: string | null;
  state: string;
  pipeline_id?: string | null;
}

export interface StatusSnapshot {
  capabilities: string[];
  active_session: ActiveSessionInfo | null;
  daemon_build?: string;
  protocol_minor?: number;
  portal_keyboard?: PortalKeyboardStatus;
  /** Rust InstalledModelLifecycleDto serializes as a string, including future
   * lifecycle tokens; it is not an object containing a state field. */
  installed_model?: string | null;
  model_provisioning?: ModelProvisionStatus;
}

export type SpokenLanguageSetting =
  | { kind: "unspecified" }
  | { kind: "auto_detect" }
  | { kind: "explicit"; tag: string };

export interface RecognitionSettings {
  spoken_language?: SpokenLanguageSetting;
  output_language?: string;
  output_script?: string;
  translation_target?: string;
  locale?: string;
  context_hints?: string[];
}

/** One speech model and its state on this computer (daemon protocol minor 13+). */
export interface ModelInfo {
  id: string;
  family: string;
  variant: string;
  /** Quantization as the publisher names it, for example `Q8_0` or `INT8`. */
  quant: string;
  size_bytes: number;
  license: string;
  engine: string;
  languages: string[];
  streaming: boolean;
  /** Device classes the model may be qualified on: `CPU`, `GPU`, `NPU`. */
  devices?: string[];
  source_repo: string;
  source_revision: string;
  qualified: boolean;
  state:
    | "active"
    | "installed"
    | "available"
    | "needs_engine"
    | "loading"
    | "downloading";
  /** This service can download the model when asked (protocol minor 16+). */
  downloadable?: boolean;
  /** Bytes on disk so far while downloading, or left by an unfinished download. */
  downloaded_bytes?: number;
  /** The service can delete this installed model (an add-on model pack). */
  removable?: boolean;
}

/** One item the Store offers (daemon protocol minor 17+). The service decides
 * every state and verdict; the screen only shows them. */
export interface StoreItem {
  id: string;
  /** `model` only from an older service; the Store no longer lists models. */
  kind: "runtime" | "module" | "extension" | "model";
  /** The runtime family for a runtime, or the module's job (`refine`, `denoise`). */
  category: string;
  name: string;
  size_bytes: number;
  license: string;
  devices: string[];
  state: "available" | "installing" | "installed" | "in_use" | "failed";
  installed_bytes?: number;
  failure?: string;
  fit: "fits" | "needs_runtime" | "does_not_fit" | "unknown";
  fit_reason?: string;
  /** `published`, `local` (a developer source), `unavailable`, `built_in` (already part of Narrivy) or `planned` (coming later). */
  source: string;
  requires?: string;
}

/** One keyboard shortcut bound to a pipeline (daemon protocol minor 11+). */
export interface ShortcutBinding {
  /** `Super`, `Ctrl`, `Alt`, `Shift`. */
  modifiers: string[];
  key: string;
  pipeline_id: string;
  spoken_language?: string;
  output_language?: string;
}

export interface ShortcutsInfo {
  /** `managed`, or `manual:<reason>` when the user must bind the command themselves. */
  support: string;
  /** The command to bind by hand; append `--pipeline ID` and language flags. */
  manual_command: string;
  bindings: ShortcutBinding[];
}

export interface ShortcutSetResult {
  outcome: "created" | "replaced" | "conflict" | "invalid" | "unsupported";
  /** For `conflict`: another custom shortcut, or one the desktop ships. */
  conflict_kind?: "shortcut" | "desktop";
  conflict_name?: string;
}

export interface ModelProvisionStatus {
  operation_id: string;
  state: string;
  publication: string;
  cleanup: string;
  failure?: string;
  revision: number;
}

export interface PortalKeyboardStatus {
  eligibility: string;
  lifecycle: string;
  can_request: boolean;
  permission_active: boolean;
  transport_connected: boolean;
  sender_available: boolean;
  delivery_qualified: boolean;
  cleanup: string;
}

/** One pipeline advertised by the daemon (`request.list_pipelines`).
 * Mirrors `narrivy_ipc::PipelineSummaryDto` exactly: the daemon only
 * advertises pipelines its own configuration accepts for StartSession, so the
 * frontend must not invent or rename lanes. */
export interface PipelineSummary {
  id: string;
  name: string;
  version: number;
  /** False while the lane is advertised but not currently startable (for
   * example the strict recovery lane before its model is provisioned). */
  enabled: boolean;
  /** The model this pipeline recognises with; absent when it uses whatever
   * model is loaded (daemon protocol minor 15+). */
  model_id?: string;
  /** The device this pipeline asks for: 'auto', 'gpu' or 'cpu' (minor 15+). */
  compute?: string;
}

export type PipelineCompute = "auto" | "gpu" | "npu" | "cpu";

/** Content-safe optional-History row returned by daemon_history. */
export interface JournalEntryDto {
  txid: string;
  state?: string;
  pipeline_id?: string | null;
  has_delivered_text?: boolean;
  updated_unix_ms?: number;
  /** How long transcription took, in microseconds (daemon protocol minor 12+). */
  transcribe_us?: number | null;
}

/** Full optional-History row returned by daemon_history_entry(txid). */
export interface HistoryEntryFull extends JournalEntryDto {
  text?: string | null;
  /** Typed variants (P7): used for the UX-009 one-step undo. */
  raw?: string | null;
  corrected?: string | null;
  delivered?: string | null;
  /** Typed, content-free reason capture stopped (protocol minor 6). Absent on
   * older daemons; a missing value must render as "not reported". */
  capture_end?: string | null;
}

export interface CapabilityCheckResult {
  id: string;
  pass: boolean;
  disabled?: boolean;
  detail?: string;
}

/** P7 FUN-033: report of a versioned pipelines export. */
export interface PipelinesExportReport {
  count: number;
  path: string;
}

/** P7 FUN-033/UX-011: import result with required-dependencies preview. */
export interface PipelinesImportReport {
  dry_run: boolean;
  imported: number;
  skipped_duplicates: number;
  required_dependencies: string[];
}

export type PortalPermissionOutcome =
  | "permission_active"
  | "permission_inactive"
  | "parent_export_failed"
  | "cancelled"
  | "cleanup_unconfirmed";

export interface PortalPermissionResult {
  outcome: PortalPermissionOutcome;
}

/** Additive daemon event union. Transcript text exists only on the volatile
 * partial variant and must never be stored by the bridge. */
/** A language model on this computer (daemon protocol minor 20). */
export interface LanguageModel {
  id: string;
  /** As its licence requires it to be named. */
  name: string;
  publisher: string;
  size_bytes: number;
  license: string;
  license_notice?: string;
  /** What it can do: `clean_up`, `assistant`. */
  roles: string[];
  languages: string[];
  recommended_for: string[];
  in_use_for: string[];
  /** `loading`, `ready:<device>` or `failed:<code>` while used for clean-up. */
  clean_up_state?: string;
  /** `loaded:<device>` or `unloaded` while used for the assistant. */
  assistant_state?: string;
}

/** Extra for a question asked into a kept chat (minor 44). */
export interface AssistantSendOptions {
  chatId?: string;
  speak?: boolean;
  pictures?: string[];
}

/** Voice-mode settings; each is changed only when given. */
export interface VoiceSettings {
  speed_percent?: number;
  push_to_talk?: boolean;
  interrupt?: 'stop' | 'queue';
  sounds?: boolean;
  muted?: boolean;
  voice?: number;
}

/** One message of an assistant conversation. */
export interface AssistantMessage {
  role: "system" | "user" | "assistant";
  content: string;
}

/** A recorded meeting (daemon protocol minor 25). */
/** Text options (protocol minor 35). */
export interface TextOptions {
  numbers_as_digits: boolean;
  /** `paste` or `type` (protocol minor 38). */
  delivery_method?: string;
  /** Unload the clean-up model after this many idle minutes; 0 never (protocol minor 40). */
  cleanup_idle_minutes?: number;
  /** "new line" / "new paragraph" as line breaks on Clean (protocol minor 42). */
  spoken_breaks?: boolean;
}

/** Press and hold (protocol minor 32). */
export interface HoldKey {
  key: number | null;
  /** `off`, `starting`, `reading`, `denied` or `unavailable`. */
  state: string;
  keys: Array<{ code: number; name: string }>;
}

/** Start at login (the desktop app's own setting). */
export interface Autostart {
  enabled: boolean;
  hidden: boolean;
  service_installed: boolean;
}

/** The user's vocabulary (protocol minor 30). */
export interface Vocabulary {
  words: string[];
  replacements: Array<{ from: string; to: string }>;
}

/** Audio options (protocol minor 27). */
export interface AudioOptions {
  /** Remove the computer's own sound (videos, calls on speakers) from the microphone. */
  echo_cancel: boolean;
  /** Whether that is working now; when not, the plain microphone is used. */
  echo_cancel_active: boolean;
  /** Whether the voice filter can run (protocol minor 28). */
  voice_filter_available?: boolean;
  /** Whether the user's voice is trained. */
  voice_trained?: boolean;
  /** Keep only the user's voice once trained. */
  only_my_voice?: boolean;
  /** `idle`, `recording` or `failed:<code>`. */
  voice_training?: string;
  /** Background noise removed: `off`, `standard` or `strong` (protocol minor 29). */
  noise_suppression?: string;
  /** The chosen microphone, `""` for the system default (protocol minor 31). */
  microphone?: string;
  /** The chosen microphone is unplugged; the default is used meanwhile. */
  microphone_missing?: boolean;
  /** The microphones to choose from. */
  microphones?: Array<{ name: string; description: string }>;
  /** A short sound when listening starts and stops (protocol minor 33). */
  sounds?: boolean;
  /** How strictly "Only my voice" keeps only the user (protocol minor 41). */
  voice_strictness?: string;
  /** Pipelines with their own Voice settings; `null` follows the default (protocol minor 36). */
  pipelines?: Record<
    string,
    { echo_cancel?: boolean | null; only_my_voice?: boolean | null }
  >;
}

export interface Meeting {
  id: string;
  title: string;
  started_unix_ms: number;
  duration_ms: number;
  /** `recording`, `processing`, `ready` or `failed:<code>`. */
  state: string;
  /** While processing: 0..1000. */
  progress: number;
  /** While processing: `load`, `speakers` or `words`. */
  stage?: string;
  /** `turbo` or `large-v3`. */
  model: string;
  speakers: number;
  /** A screen session (screen share or recording). */
  watch?: boolean;
  /** Processing is paused (daemon minor 48). */
  paused?: boolean;
  /** Started as a live screen share (daemon minor 49). */
  share?: boolean;
  /** While recording: the screen and the sound are kept now (minor 50). */
  screen_on?: boolean;
  sound_on?: boolean;
  /** While recording: the microphone is kept now (daemon minor 51). */
  mic_on?: boolean;
  /** Kept as heard when it stopped: the live transcript and the screens,
   * not gone over yet (daemon minor 52). */
  raw?: boolean;
}

/** How the screen is shared or recorded. */
export interface ScreenStartOptions {
  /** Keep the screen too; off: only the sound, as a live transcript. */
  screen: boolean;
  /** Live with the open chat, or recorded without one; both are kept as a session. */
  share: boolean;
  microphone: boolean;
  computerSound: boolean;
}

/** One turn of a meeting transcript. */
export interface MeetingTurn {
  /** While recording: the words being said now, not final (daemon minor 53). */
  partial?: boolean;
  /** `Person 1`, `Person 2`… in the order people first speak, or a name. */
  speaker: string;
  source: "room" | "call" | string;
  start_ms: number;
  end_ms: number;
  text: string;
  language: string;
}

/** What to switch in a recording; unset leaves it. */
export interface SourceChange {
  screen?: boolean;
  /** The computer's sound. */
  sound?: boolean;
  microphone?: boolean;
}

export interface MeetingDetail {
  meeting: Meeting;
  turns: MeetingTurn[];
  /** Label to name. */
  names: Record<string, string>;
  folder: string;
  /** Installed meeting speech models: `large-v3`, `turbo`. */
  models: string[];
  /** Saved notes by kind: `notes`, `email`, `custom` (Markdown). */
  notes?: Record<string, string>;
  /** Whether the full recording of everyone is kept. */
  recording?: boolean;
  /** What was shown on screen (daemon minor 45). */
  slides?: MeetingSlide[];
  /** Why the screen could not be kept. */
  screen_error?: string;
  /** The screens and windows being watched; live pictures as `live-<n>.png`. */
  screen_sources?: Array<{ source: number; kind: string; width: number; height: number }>;
}

export interface SessionEvent {
  event?:
    | "session_state_changed"
    | "transcript_partial"
    | "events_resubscribed"
    | "assistant_delta"
    | "assistant_done"
    | string;
  /** Assistant events: the turn they belong to. */
  turn_id?: string;
  /** `meeting_changed`: the meeting, its progress (0..1000) and stage. */
  id?: string;
  progress?: number;
  stage?: string;
  /** `assistant_done`: `ok`, `cancelled` or `failed:<code>`. */
  outcome?: string;
  session_id?: string;
  transaction_id?: string | null;
  pipeline_id?: string | null;
  event_seq?: number;
  seq?: number;
  state?: string;
  text?: string;
}

export type SessionEventHandler = (event: SessionEvent) => void;

export interface BridgeApi {
  status(): Promise<StatusSnapshot>;
  start(
    pipelineId?: string | null,
    recognition?: RecognitionSettings | null,
  ): Promise<void>;
  stop(): Promise<void>;
  cancel(): Promise<void>;
  listPipelines(): Promise<PipelineSummary[]>;
  history(): Promise<JournalEntryDto[]>;
  historyEntry(txid: string): Promise<HistoryEntryFull>;
  /** Delete one optional-History entry (daemon protocol minor 10+). */
  deleteHistoryEntry(txid: string): Promise<void>;
  /** The speech models the daemon knows about (daemon protocol minor 13+). */
  models(): Promise<ModelInfo[]>;
  /**
   * Load a downloaded model in place of the one in use (daemon protocol minor
   * 14+). Resolves once loading has started; the listing then reports the
   * model as `loading` and later `active`.
   */
  selectModel(id: string): Promise<void>;
  /**
   * Set the model and device one pipeline recognises with (daemon protocol
   * minor 15+). A null model means "whatever model is loaded". The service
   * loads the choice at once when nothing is dictating.
   */
  setPipelineEngine(
    pipelineId: string,
    modelId: string | null,
    compute: PipelineCompute,
  ): Promise<void>;
  /**
   * Download a published model (daemon protocol minor 16+). The only time
   * Narrivy uses the network, and only on this request. Resolves once the
   * download has started; the listing then reports its progress.
   */
  downloadModel(id: string): Promise<void>;
  /** Stop a running download; what arrived is kept for resuming. */
  cancelModelDownload(id: string): Promise<void>;
  /** What the Store offers (daemon protocol minor 17+). Install with `downloadModel`. */
  storeItems(): Promise<StoreItem[]>;
  /** Remove an installed Store item; refused while it is in use. */
  removeStoreItem(id: string): Promise<void>;
  /** Ask the AI assistant (daemon protocol minor 19). The reply streams as
   * `assistant_delta` session events and ends with `assistant_done`. With
   * `options.chatId` (minor 44) the daemon keeps the chat: `messages` is
   * only the new question, and the reply is saved into the chat. */
  assistantSend(turnId: string, messages: AssistantMessage[], options?: AssistantSendOptions): Promise<void>;
  /** The assistant's library (daemon protocol minor 44): chats, memory,
   * personas, projects, skills, routines, pictures and voice settings.
   * Replies with JSON text. */
  assistantLibrary(op: string, kind: string, id?: string | null, value?: string | null): Promise<string>;
  /** Voice mode (daemon protocol minor 44): `start`, `stop`, `settings`,
   * `ptt`. What it hears arrives as `voice` session events. */
  voiceMode(action: string, settings?: VoiceSettings | null, down?: boolean | null): Promise<void>;
  /** Allow or refuse a tool call (daemon protocol minor 46). */
  assistantToolDecision(callId: string, allow: boolean, always: boolean): Promise<void>;
  /** Stop the reply to `turnId`. */
  assistantCancel(turnId: string): Promise<void>;
  /** Free the assistant model's memory now (daemon protocol minor 22). */
  assistantUnload(): Promise<void>;
  /** The desktop's folder chooser; null when cancelled. */
  pickFolder(title: string): Promise<string | null>;
  /** Language models on this computer (daemon protocol minor 20). */
  languageModels(): Promise<LanguageModel[]>;
  /** Use model `id` for `role` (`clean_up`, `assistant`), or none with null. */
  selectLanguageModel(role: string, id: string | null): Promise<void>;
  /** Start recording a meeting (daemon protocol minor 25). Progress arrives
   * as `meeting_changed` session events. */
  audioOptions(): Promise<AudioOptions>;
  setEchoCancel(on: boolean): Promise<AudioOptions>;
  setOnlyMyVoice(on: boolean): Promise<AudioOptions>;
  trainVoice(): Promise<AudioOptions>;
  setNoiseSuppression(level: string): Promise<AudioOptions>;
  setMicrophone(name: string): Promise<AudioOptions>;
  setSounds(on: boolean): Promise<AudioOptions>;
  setVoiceStrictness(level: string): Promise<AudioOptions>;
  setPipelineVoice(
    pipelineId: string,
    echoCancel: boolean | null,
    onlyMyVoice: boolean | null,
  ): Promise<AudioOptions>;
  vocabulary(): Promise<Vocabulary>;
  autostart(): Promise<Autostart>;
  holdKey(): Promise<HoldKey>;
  textOptions(): Promise<TextOptions>;
  setNumbersAsDigits(on: boolean): Promise<TextOptions>;
  setDeliveryMethod(method: string): Promise<TextOptions>;
  setCleanupIdle(minutes: number): Promise<TextOptions>;
  setSpokenBreaks(on: boolean): Promise<TextOptions>;
  setHistoryRetention(retention: string): Promise<void>;
  clearHistory(): Promise<void>;
  setHoldKey(key: number | null): Promise<HoldKey>;
  setAutostart(enabled: boolean, hidden: boolean): Promise<Autostart>;
  setVocabulary(vocabulary: Vocabulary): Promise<Vocabulary>;
  forgetVoice(): Promise<AudioOptions>;
  /** With `watch` ("Watch with me", daemon minor 47) the screen is kept too. */
  meetingStart(title: string | null, watch?: boolean): Promise<Meeting>;
  /** Share the screen live with the assistant, or record it as a session,
   * with the sound chosen for this one (daemon minor 49). */
  screenStart(title: string | null, options: ScreenStartOptions): Promise<Meeting>;
  /** Stop recording; transcribing starts at once. */
  meetingStop(): Promise<void>;
  /** Pause, resume or stop a meeting's processing (daemon minor 48). */
  meetingControl(id: string, action: "pause" | "resume" | "stop"): Promise<void>;
  /** Switch the screen or the sound of the meeting being recorded on or off
   * (daemon minor 50). */
  /** `sound` is the computer's sound, `microphone` the microphone (minor 51). */
  meetingSources(id: string, change: SourceChange): Promise<Meeting>;
  meetings(): Promise<Meeting[]>;
  meeting(id: string): Promise<MeetingDetail>;
  /** Transcribe again with another model or fixed speaker counts. */
  meetingProcess(
    id: string,
    model: string | null,
    roomSpeakers: number | null,
    callSpeakers: number | null,
  ): Promise<void>;
  /** Name a speaker label; an empty name restores the label. */
  meetingRename(id: string, speaker: string, name: string): Promise<void>;
  meetingDelete(id: string): Promise<void>;
  /** Show a meeting's folder in the file manager, or open one of its files
   * (`recording.wav`, `transcript.md`) in the default app. */
  openMeetingFolder(path: string, file?: string): Promise<void>;
  /** Write notes with the assistant model (daemon protocol minor 26): `notes`,
   * `email` or `custom` with an instruction. Streams as assistant events
   * under `turnId`; the result is saved with the meeting. */
  meetingSummarize(
    id: string,
    turnId: string,
    kind: string,
    instruction: string | null,
  ): Promise<void>;
  /** Tell the host the theme, accent and overlay position, so the overlay matches. */
  setOverlayAppearance(
    theme: "light" | "dark",
    accent: string,
    position: string,
  ): Promise<void>;
  /** Shortcuts Narrivy manages in the desktop's settings (daemon protocol minor 11+). */
  shortcuts(): Promise<ShortcutsInfo>;
  setShortcut(
    binding: ShortcutBinding,
    replace: boolean,
  ): Promise<ShortcutSetResult>;
  removeShortcut(modifiers: string[], key: string): Promise<void>;
  recoveryEntry(txid: string): Promise<HistoryEntryFull>;
  capabilityCheck(id: string): Promise<CapabilityCheckResult>;
  pipelinesExport(path: string): Promise<PipelinesExportReport>;
  pipelinesImport(
    path: string,
    dryRun: boolean,
  ): Promise<PipelinesImportReport>;
  requestPortalKeyboardPermission(): Promise<PortalPermissionResult>;
  cancelPortalKeyboardPermission(): Promise<void>;
  installBundledModel(): Promise<ModelProvisionStatus>;
  cancelModelInstall(operationId: string): Promise<ModelProvisionStatus>;
  onSessionEvent(handler: SessionEventHandler): Promise<UnlistenFn>;
  /** Files dropped on the window, as paths. */
  onFileDrop(handler: (paths: string[]) => void): Promise<UnlistenFn>;
  meetingImport(path: string): Promise<Meeting>;
  meetingOptions(): Promise<MeetingOptions>;
  setAutoNotes(on: boolean): Promise<MeetingOptions>;
  setProcessOnStop(on: boolean): Promise<MeetingOptions>;
  /** Choose the meeting's microphone and output; "" is the default. */
  setMeetingDevices(choice: {
    microphone?: string;
    speaker?: string;
    call_audio?: boolean;
    room_audio?: boolean;
  }): Promise<MeetingOptions>;
  /** Keep pictures of the screen in meetings (daemon minor 45). */
  setMeetingScreen(on: boolean): Promise<MeetingOptions>;
  /** A meeting's screen picture, small, as base64 PNG. */
  meetingSlide(id: string, file: string): Promise<string>;
}

class TauriBridge implements BridgeApi {
  status(): Promise<StatusSnapshot> {
    return invoke<StatusSnapshot>("daemon_status");
  }
  start(
    pipelineId?: string | null,
    recognition?: RecognitionSettings | null,
  ): Promise<void> {
    return invoke<void>("daemon_start", {
      pipelineId: pipelineId ?? null,
      recognition: recognition ?? null,
    });
  }
  stop(): Promise<void> {
    return invoke<void>("daemon_stop");
  }
  cancel(): Promise<void> {
    return invoke<void>("daemon_cancel");
  }
  listPipelines(): Promise<PipelineSummary[]> {
    return invoke<PipelineSummary[]>("daemon_list_pipelines");
  }
  history(): Promise<JournalEntryDto[]> {
    return invoke<JournalEntryDto[]>("daemon_history");
  }
  historyEntry(txid: string): Promise<HistoryEntryFull> {
    return invoke<HistoryEntryFull>("daemon_history_entry", { txid });
  }
  deleteHistoryEntry(txid: string): Promise<void> {
    return invoke<void>("daemon_history_delete", { txid });
  }
  models(): Promise<ModelInfo[]> {
    return invoke<ModelInfo[]>("daemon_models");
  }
  selectModel(id: string): Promise<void> {
    return invoke<void>("daemon_model_select", { id });
  }
  setPipelineEngine(
    pipelineId: string,
    modelId: string | null,
    compute: PipelineCompute,
  ): Promise<void> {
    return invoke<void>("daemon_pipeline_engine_set", {
      pipelineId,
      modelId,
      compute,
    });
  }
  downloadModel(id: string): Promise<void> {
    return invoke<void>("daemon_model_download", { id });
  }
  cancelModelDownload(id: string): Promise<void> {
    return invoke<void>("daemon_model_download_cancel", { id });
  }
  storeItems(): Promise<StoreItem[]> {
    return invoke<StoreItem[]>("daemon_store_list");
  }
  removeStoreItem(id: string): Promise<void> {
    return invoke<void>("daemon_store_remove", { id });
  }
  assistantSend(turnId: string, messages: AssistantMessage[], options?: AssistantSendOptions): Promise<void> {
    return invoke<void>("daemon_assistant_send", {
      turnId,
      messages,
      chatId: options?.chatId ?? null,
      speak: options?.speak ?? null,
      pictures: options?.pictures ?? null,
    });
  }
  assistantLibrary(op: string, kind: string, id?: string | null, value?: string | null): Promise<string> {
    return invoke<string>("daemon_assistant_library", { op, kind, id: id ?? null, value: value ?? null });
  }
  voiceMode(action: string, settings?: VoiceSettings | null, down?: boolean | null): Promise<void> {
    return invoke<void>("daemon_voice_mode", { action, settings: settings ?? null, down: down ?? null });
  }
  assistantToolDecision(callId: string, allow: boolean, always: boolean): Promise<void> {
    return invoke<void>("daemon_assistant_tool_decision", { callId, allow, always });
  }
  assistantCancel(turnId: string): Promise<void> {
    return invoke<void>("daemon_assistant_cancel", { turnId });
  }
  assistantUnload(): Promise<void> {
    return invoke<void>("daemon_assistant_unload");
  }
  pickFolder(title: string): Promise<string | null> {
    return invoke<string | null>("pick_folder", { title });
  }
  languageModels(): Promise<LanguageModel[]> {
    return invoke<LanguageModel[]>("daemon_language_models");
  }
  selectLanguageModel(role: string, id: string | null): Promise<void> {
    return invoke<void>("daemon_language_model_select", { role, id });
  }
  audioOptions(): Promise<AudioOptions> {
    return invoke<AudioOptions>("daemon_audio_options");
  }
  setEchoCancel(on: boolean): Promise<AudioOptions> {
    return invoke<AudioOptions>("daemon_set_echo_cancel", { on });
  }
  setOnlyMyVoice(on: boolean): Promise<AudioOptions> {
    return invoke<AudioOptions>("daemon_set_only_my_voice", { on });
  }
  trainVoice(): Promise<AudioOptions> {
    return invoke<AudioOptions>("daemon_train_voice");
  }
  setNoiseSuppression(level: string): Promise<AudioOptions> {
    return invoke<AudioOptions>("daemon_set_noise_suppression", { level });
  }
  setMicrophone(name: string): Promise<AudioOptions> {
    return invoke<AudioOptions>("daemon_set_microphone", { name });
  }
  setSounds(on: boolean): Promise<AudioOptions> {
    return invoke<AudioOptions>("daemon_set_sounds", { on });
  }
  setVoiceStrictness(level: string): Promise<AudioOptions> {
    return invoke<AudioOptions>("daemon_set_voice_strictness", { level });
  }
  setPipelineVoice(
    pipelineId: string,
    echoCancel: boolean | null,
    onlyMyVoice: boolean | null,
  ): Promise<AudioOptions> {
    return invoke<AudioOptions>("daemon_set_pipeline_voice", {
      pipelineId,
      echoCancel,
      onlyMyVoice,
    });
  }
  vocabulary(): Promise<Vocabulary> {
    return invoke<Vocabulary>("daemon_vocabulary");
  }
  autostart(): Promise<Autostart> {
    return invoke<Autostart>("app_autostart");
  }
  holdKey(): Promise<HoldKey> {
    return invoke<HoldKey>("daemon_hold_key");
  }
  textOptions(): Promise<TextOptions> {
    return invoke<TextOptions>("daemon_text_options");
  }
  setNumbersAsDigits(on: boolean): Promise<TextOptions> {
    return invoke<TextOptions>("daemon_set_numbers_as_digits", { on });
  }
  setDeliveryMethod(method: string): Promise<TextOptions> {
    return invoke<TextOptions>("daemon_set_delivery_method", { method });
  }
  setCleanupIdle(minutes: number): Promise<TextOptions> {
    return invoke<TextOptions>("daemon_set_cleanup_idle", { minutes });
  }
  setSpokenBreaks(on: boolean): Promise<TextOptions> {
    return invoke<TextOptions>("daemon_set_spoken_breaks", { on });
  }
  setHistoryRetention(retention: string): Promise<void> {
    return invoke<void>("daemon_set_history_retention", { retention });
  }
  clearHistory(): Promise<void> {
    return invoke<void>("daemon_clear_history");
  }
  setHoldKey(key: number | null): Promise<HoldKey> {
    return invoke<HoldKey>("daemon_set_hold_key", { key });
  }
  setAutostart(enabled: boolean, hidden: boolean): Promise<Autostart> {
    return invoke<Autostart>("app_set_autostart", { enabled, hidden });
  }
  setVocabulary(vocabulary: Vocabulary): Promise<Vocabulary> {
    return invoke<Vocabulary>("daemon_set_vocabulary", { vocabulary });
  }
  forgetVoice(): Promise<AudioOptions> {
    return invoke<AudioOptions>("daemon_forget_voice");
  }
  screenStart(title: string | null, options: ScreenStartOptions): Promise<Meeting> {
    return invoke<Meeting>("daemon_screen_start", {
      title,
      screen: options.screen,
      share: options.share,
      microphone: options.microphone,
      computerSound: options.computerSound,
    });
  }
  meetingStart(title: string | null, watch?: boolean): Promise<Meeting> {
    return invoke<Meeting>("daemon_meeting_start", { title, watch: watch ?? null });
  }

  meetingImport(path: string): Promise<Meeting> {
    return invoke<Meeting>("daemon_meeting_import", { path });
  }
  meetingOptions(): Promise<MeetingOptions> {
    return invoke<MeetingOptions>("daemon_meeting_options");
  }
  setAutoNotes(on: boolean): Promise<MeetingOptions> {
    return invoke<MeetingOptions>("daemon_set_auto_notes", { on });
  }
  setProcessOnStop(on: boolean): Promise<MeetingOptions> {
    return invoke<MeetingOptions>("daemon_set_process_on_stop", { on });
  }
  setMeetingDevices(choice: {
    microphone?: string;
    speaker?: string;
    call_audio?: boolean;
    room_audio?: boolean;
  }): Promise<MeetingOptions> {
    return invoke<MeetingOptions>("daemon_set_meeting_devices", {
      microphone: choice.microphone ?? null,
      speaker: choice.speaker ?? null,
      callAudio: choice.call_audio ?? null,
      roomAudio: choice.room_audio ?? null,
    });
  }  setMeetingScreen(on: boolean): Promise<MeetingOptions> {
    return invoke<MeetingOptions>("daemon_set_meeting_screen", { on });
  }
  meetingSlide(id: string, file: string): Promise<string> {
    return invoke<string>("daemon_meeting_slide", { id, file });
  }

  async onFileDrop(handler: (paths: string[]) => void): Promise<UnlistenFn> {
    const { getCurrentWebview } = await import("@tauri-apps/api/webview");
    return getCurrentWebview().onDragDropEvent((event) => {
      if (event.payload.type === "drop") handler(event.payload.paths);
    });
  }
  meetingStop(): Promise<void> {
    return invoke<void>("daemon_meeting_stop");
  }
  meetingControl(id: string, action: "pause" | "resume" | "stop"): Promise<void> {
    return invoke<void>("daemon_meeting_control", { id, action });
  }
  meetingSources(id: string, change: SourceChange): Promise<Meeting> {
    return invoke<Meeting>("daemon_meeting_sources", {
      id,
      screen: change.screen ?? null,
      sound: change.sound ?? null,
      microphone: change.microphone ?? null,
    });
  }
  meetings(): Promise<Meeting[]> {
    return invoke<Meeting[]>("daemon_meetings");
  }
  meeting(id: string): Promise<MeetingDetail> {
    return invoke<MeetingDetail>("daemon_meeting", { id });
  }
  meetingProcess(
    id: string,
    model: string | null,
    roomSpeakers: number | null,
    callSpeakers: number | null,
  ): Promise<void> {
    return invoke<void>("daemon_meeting_process", {
      id,
      model,
      roomSpeakers,
      callSpeakers,
    });
  }
  meetingRename(id: string, speaker: string, name: string): Promise<void> {
    return invoke<void>("daemon_meeting_rename", { id, speaker, name });
  }
  meetingDelete(id: string): Promise<void> {
    return invoke<void>("daemon_meeting_delete", { id });
  }
  openMeetingFolder(path: string, file?: string): Promise<void> {
    return invoke<void>("open_meeting_folder", { path, file: file ?? null });
  }
  meetingSummarize(
    id: string,
    turnId: string,
    kind: string,
    instruction: string | null,
  ): Promise<void> {
    return invoke<void>("daemon_meeting_summarize", {
      id,
      turnId,
      kind,
      instruction,
    });
  }
  setOverlayAppearance(
    theme: "light" | "dark",
    accent: string,
    position: string,
  ): Promise<void> {
    return invoke<void>("overlay_appearance", { theme, accent, position });
  }
  shortcuts(): Promise<ShortcutsInfo> {
    return invoke<ShortcutsInfo>("daemon_shortcuts_list");
  }
  setShortcut(
    binding: ShortcutBinding,
    replace: boolean,
  ): Promise<ShortcutSetResult> {
    return invoke<ShortcutSetResult>("daemon_shortcut_set", {
      binding,
      replace,
    });
  }
  removeShortcut(modifiers: string[], key: string): Promise<void> {
    return invoke<void>("daemon_shortcut_remove", { modifiers, key });
  }
  recoveryEntry(txid: string): Promise<HistoryEntryFull> {
    return invoke<HistoryEntryFull>("daemon_recovery_entry", { txid });
  }
  capabilityCheck(id: string): Promise<CapabilityCheckResult> {
    return invoke<CapabilityCheckResult>("daemon_capability_check", { id });
  }
  pipelinesExport(path: string): Promise<PipelinesExportReport> {
    return invoke<PipelinesExportReport>("pipelines_export", { path });
  }
  pipelinesImport(
    path: string,
    dryRun: boolean,
  ): Promise<PipelinesImportReport> {
    return invoke<PipelinesImportReport>("pipelines_import", { path, dryRun });
  }
  requestPortalKeyboardPermission(): Promise<PortalPermissionResult> {
    return Promise.reject({
      code: "unavailable",
      message: "portal keyboard permission is not used in this release",
    });
  }
  cancelPortalKeyboardPermission(): Promise<void> {
    return Promise.reject({
      code: "unavailable",
      message: "portal keyboard permission is not used in this release",
    });
  }
  installBundledModel(): Promise<ModelProvisionStatus> {
    return invoke<ModelProvisionStatus>("model_install_bundled");
  }
  cancelModelInstall(operationId: string): Promise<ModelProvisionStatus> {
    return invoke<ModelProvisionStatus>("model_install_cancel", {
      operationId,
    });
  }
  onSessionEvent(handler: SessionEventHandler): Promise<UnlistenFn> {
    return listen<SessionEvent>(SESSION_EVENT, (e) => handler(e.payload));
  }
}

// Tauri 2 is the desktop host on every OS (ADR-0075).
export let bridge: BridgeApi = new TauriBridge();

/** Development preview only (see main.tsx): swap the transport before the
 * app renders, so `npm run dev` works in an ordinary browser. */
export function setPreviewBridge(next: BridgeApi): void {
  bridge = next;
}

/**
 * In-memory BridgeApi for tests and offline development.
 * Holds canned snapshots and lets tests push synthetic session events.
 */
export class FakeBridge implements BridgeApi {
  statusSnapshot: StatusSnapshot;
  pipelines: PipelineSummary[];
  entries: JournalEntryDto[];
  fullText: Record<string, string>;
  checks: Record<string, CapabilityCheckResult>;
  exportReport?: PipelinesExportReport;
  importReport?: PipelinesImportReport;

  private handlers = new Set<SessionEventHandler>();
  private seq = 0;

  constructor(
    init: {
      status?: Partial<StatusSnapshot>;
      pipelines?: PipelineSummary[];
      entries?: JournalEntryDto[];
      fullText?: Record<string, string>;
      checks?: Record<string, CapabilityCheckResult>;
    } = {},
  ) {
    this.statusSnapshot = {
      capabilities: [
        "mic.available",
        "asr.transcribe-cpp",
        "mock.asr",
        "delivery.clipboard",
      ],
      active_session: { state: "idle", pipeline_id: null },
      daemon_build: "narrivyd 0.0.0-dev",
      installed_model: "ready",
      portal_keyboard: {
        eligibility: "ready_to_request",
        lifecycle: "idle",
        can_request: true,
        permission_active: false,
        transport_connected: false,
        sender_available: false,
        delivery_qualified: false,
        cleanup: "not_required",
      },
      ...init.status,
    };
    this.pipelines = init.pipelines ?? [
      {
        id: "mock-dictation",
        name: "Mock dictation pipeline",
        version: 1,
        enabled: true,
      },
    ];
    this.entries = init.entries ?? [];
    this.fullText = init.fullText ?? {};
    this.checks = init.checks ?? {};
  }

  status(): Promise<StatusSnapshot> {
    return Promise.resolve(
      JSON.parse(JSON.stringify(this.statusSnapshot)) as StatusSnapshot,
    );
  }
  start(
    _pipelineId?: string | null,
    _recognition?: RecognitionSettings | null,
  ): Promise<void> {
    this.statusSnapshot.active_session = {
      state: "recording",
      pipeline_id: _pipelineId ?? null,
    };
    this.emit("recording");
    return Promise.resolve();
  }
  stop(): Promise<void> {
    this.statusSnapshot.active_session = { state: "idle", pipeline_id: null };
    this.emit("idle");
    return Promise.resolve();
  }
  cancel(): Promise<void> {
    this.statusSnapshot.active_session = { state: "idle", pipeline_id: null };
    this.emit("idle");
    return Promise.resolve();
  }
  listPipelines(): Promise<PipelineSummary[]> {
    return Promise.resolve(
      JSON.parse(JSON.stringify(this.pipelines)) as PipelineSummary[],
    );
  }
  history(): Promise<JournalEntryDto[]> {
    return Promise.resolve(
      JSON.parse(JSON.stringify(this.entries)) as JournalEntryDto[],
    );
  }
  deleteHistoryEntry(txid: string): Promise<void> {
    this.entries = this.entries.filter((e) => e.txid !== txid);
    delete this.fullText[txid];
    return Promise.resolve();
  }
  overlayAppearance: {
    theme: string;
    accent: string;
    position: string;
  } | null = null;
  setOverlayAppearance(
    theme: "light" | "dark",
    accent: string,
    position: string,
  ): Promise<void> {
    this.overlayAppearance = { theme, accent, position };
    return Promise.resolve();
  }
  protected modelList: ModelInfo[] = [];
  /** Downloads finish at once: the model becomes downloaded. */
  downloadModel(id: string): Promise<void> {
    const item = this.storeList.find((entry) => entry.id === id);
    if (item) {
      if (
        item.state !== "available" ||
        item.fit !== "fits" ||
        item.source === "unavailable"
      ) {
        return Promise.reject({
          code: "bad_request",
          message: "this item cannot be installed",
        });
      }
      this.storeList = this.storeList.map((entry) => {
        if (entry.id === id) return { ...entry, state: "installed" };
        // Models waiting for this runtime now fit.
        if (entry.requires === id)
          return { ...entry, fit: "fits", fit_reason: item.fit_reason };
        return entry;
      });
      this.modelList = this.modelList.map((m) => {
        if (m.id === id)
          return { ...m, state: "installed", removable: m.engine === "add-on" };
        if (item.kind === "runtime" && m.state === "needs_engine")
          return { ...m, state: "available", downloadable: true };
        return m;
      });
      return Promise.resolve();
    }
    if (
      !this.modelList.some(
        (m) => m.id === id && m.state === "available" && m.downloadable,
      )
    ) {
      return Promise.reject({
        code: "bad_request",
        message: "this model cannot be downloaded",
      });
    }
    this.modelList = this.modelList.map((m) =>
      m.id === id
        ? {
            ...m,
            state: "installed",
            downloaded_bytes: undefined,
            removable: m.engine === "add-on",
          }
        : m,
    );
    return Promise.resolve();
  }
  cancelModelDownload(_id: string): Promise<void> {
    return Promise.reject({
      code: "bad_request",
      message: "that model is not downloading",
    });
  }
  protected storeList: StoreItem[] = [];
  storeItems(): Promise<StoreItem[]> {
    return Promise.resolve(this.storeList.map((item) => ({ ...item })));
  }
  removeStoreItem(id: string): Promise<void> {
    if (
      this.modelList.some(
        (m) => m.id === id && m.state === "installed" && m.removable,
      )
    ) {
      this.modelList = this.modelList.map((m) =>
        m.id === id ? { ...m, state: "available", downloadable: true } : m,
      );
      return Promise.resolve();
    }
    const item = this.storeList.find((entry) => entry.id === id);
    if (!item || item.state !== "installed") {
      return Promise.reject({ code: "bad_request", message: "not_installed" });
    }
    this.storeList = this.storeList.map((entry) =>
      entry.id === id ? { ...entry, state: "available" } : entry,
    );
    return Promise.resolve();
  }
  /** Stores the choice and switches at once, as the service does when idle. */
  setPipelineEngine(
    pipelineId: string,
    modelId: string | null,
    compute: PipelineCompute,
  ): Promise<void> {
    if (
      modelId !== null &&
      !this.modelList.some(
        (m) =>
          m.id === modelId &&
          m.state !== "available" &&
          m.state !== "needs_engine",
      )
    ) {
      return Promise.reject({
        code: "bad_request",
        message: "this model is not downloaded",
      });
    }
    this.pipelines = this.pipelines.map((p) =>
      p.id === pipelineId
        ? {
            ...p,
            ...(modelId !== null
              ? { model_id: modelId }
              : { model_id: undefined }),
            compute,
          }
        : p,
    );
    if (modelId !== null) {
      this.modelList = this.modelList.map((m) =>
        m.id === modelId
          ? { ...m, state: "active" }
          : m.state === "active"
            ? { ...m, state: "installed" }
            : m,
      );
    }
    return Promise.resolve();
  }
  /** Switches at once: the chosen model becomes active, the old one installed. */
  selectModel(id: string): Promise<void> {
    if (!this.modelList.some((m) => m.id === id && m.state === "installed")) {
      return Promise.reject({
        code: "bad_request",
        message: "this model is not downloaded",
      });
    }
    this.modelList = this.modelList.map((m) =>
      m.id === id
        ? { ...m, state: "active" }
        : m.state === "active"
          ? { ...m, state: "installed" }
          : m,
    );
    return Promise.resolve();
  }
  models(): Promise<ModelInfo[]> {
    return Promise.resolve(
      JSON.parse(JSON.stringify(this.modelList)) as ModelInfo[],
    );
  }
  // Shortcuts held in memory. Ctrl+Space stands in for keys another program
  // already uses, so the conflict flow can be exercised.
  private shortcutList: ShortcutBinding[] = [];
  shortcuts(): Promise<ShortcutsInfo> {
    return Promise.resolve({
      support: "managed",
      manual_command: "/usr/bin/narrivy-cli toggle",
      bindings: JSON.parse(
        JSON.stringify(this.shortcutList),
      ) as ShortcutBinding[],
    });
  }
  setShortcut(
    binding: ShortcutBinding,
    replace: boolean,
  ): Promise<ShortcutSetResult> {
    const same = (b: ShortcutBinding) =>
      b.key === binding.key &&
      b.modifiers.join("+") === binding.modifiers.join("+");
    const taken =
      binding.key === "space" &&
      binding.modifiers.join("+") === "Ctrl" &&
      !this.shortcutList.some(same);
    if (taken && !replace) {
      return Promise.resolve({
        outcome: "conflict",
        conflict_kind: "shortcut",
        conflict_name: "another-app",
      });
    }
    this.shortcutList = [...this.shortcutList.filter((b) => !same(b)), binding];
    return Promise.resolve({ outcome: taken ? "replaced" : "created" });
  }
  removeShortcut(modifiers: string[], key: string): Promise<void> {
    this.shortcutList = this.shortcutList.filter(
      (b) => !(b.key === key && b.modifiers.join("+") === modifiers.join("+")),
    );
    return Promise.resolve();
  }
  historyEntry(txid: string): Promise<HistoryEntryFull> {
    const base = this.entries.find((e) => e.txid === txid) ?? { txid };
    return Promise.resolve({ ...base, text: this.fullText[txid] ?? null });
  }
  recoveryEntry(txid: string): Promise<HistoryEntryFull> {
    const base = this.entries.find((e) => e.txid === txid) ?? { txid };
    return Promise.resolve({ ...base, text: this.fullText[txid] ?? null });
  }
  capabilityCheck(id: string): Promise<CapabilityCheckResult> {
    const found = this.checks[id];
    return Promise.resolve(
      found ? { ...found } : { id, pass: false, detail: "no such check" },
    );
  }
  pipelinesExport(path: string): Promise<PipelinesExportReport> {
    if (this.exportReport)
      return Promise.resolve({ ...this.exportReport, path });
    return Promise.resolve({ count: this.pipelines.length, path });
  }
  pipelinesImport(
    path: string,
    dryRun: boolean,
  ): Promise<PipelinesImportReport> {
    void path;
    if (this.importReport) {
      return Promise.resolve({ ...this.importReport, dry_run: dryRun });
    }
    return Promise.resolve({
      dry_run: dryRun,
      imported: dryRun ? 0 : this.pipelines.length,
      skipped_duplicates: 0,
      required_dependencies: ["narrivy:test-module@1"],
    });
  }
  requestPortalKeyboardPermission(): Promise<PortalPermissionResult> {
    return Promise.resolve({ outcome: "permission_active" });
  }
  cancelPortalKeyboardPermission(): Promise<void> {
    return Promise.resolve();
  }
  installBundledModel(): Promise<ModelProvisionStatus> {
    return Promise.resolve({
      operation_id: "fake-model-operation",
      state: "ready",
      publication: "verified_installed",
      cleanup: "confirmed",
      revision: 1,
    });
  }
  cancelModelInstall(operationId: string): Promise<ModelProvisionStatus> {
    return Promise.resolve({
      operation_id: operationId,
      state: "cancel_requested",
      publication: "not_published",
      cleanup: "not_required",
      revision: 2,
    });
  }
  /** Replies with a fixed answer, streamed in two pieces. */
  /** The assistant library, in memory: kind -> id -> JSON object. */
  library: Record<string, Record<string, Record<string, unknown>>> = {};
  assistantLibrary(op: string, kind: string, id?: string | null, value?: string | null): Promise<string> {
    const items = (this.library[kind] ??= {});
    if (kind === "voice" && op === "get") {
      return Promise.resolve(
        JSON.stringify({ speedPercent: 100, pushToTalk: false, interrupt: "stop", sounds: true, voice: 3, running: false, available: true }),
      );
    }
    if (op === "list" && kind === "chat") {
      const list = Object.values(items)
        .map((chat) => ({ ...chat, updatedAt: Number(chat.updatedAt ?? 0), turns: ((chat.turns as unknown[]) ?? []).length }))
        .sort((a, b) => b.updatedAt - a.updatedAt);
      return Promise.resolve(JSON.stringify(list));
    }
    if (op === "list") return Promise.resolve(JSON.stringify(Object.values(items)));
    if (op === "get") {
      const item = id ? items[id] : undefined;
      return item ? Promise.resolve(JSON.stringify(item)) : Promise.reject({ code: "assistant_chat_not_found" });
    }
    if (op === "put" && kind === "picture") {
      const pictureId = `pic-${Date.now().toString(36)}`;
      items[pictureId] = { id: pictureId, png: value ?? "" };
      return Promise.resolve(JSON.stringify({ id: pictureId }));
    }
    if (op === "put" || op === "import") {
      const parsed = JSON.parse(value ?? "null") as Record<string, unknown> | Record<string, unknown>[];
      for (const item of Array.isArray(parsed) ? parsed : [parsed]) {
        const itemId = typeof item.id === "string" ? item.id : `${kind}-${Date.now().toString(36)}`;
        items[itemId] = { ...item, id: itemId };
        if (op === "put") return Promise.resolve(JSON.stringify(items[itemId]));
      }
      return Promise.resolve("{}");
    }
    if (op === "delete" && id) delete items[id];
    return Promise.resolve("{}");
  }
  voiceMode(_action: string): Promise<void> {
    return Promise.resolve();
  }
  assistantToolDecision(_callId: string, _allow: boolean, _always: boolean): Promise<void> {
    return Promise.resolve();
  }
  assistantSend(turnId: string, _messages: AssistantMessage[], options?: AssistantSendOptions): Promise<void> {
    const pieces = ["This is a ", "local answer."];
    const chatId = options?.chatId;
    if (chatId) {
      const chats = (this.library.chat ??= {});
      const chat = (chats[chatId] ??= { id: chatId, title: _messages[0]?.content.slice(0, 60) ?? "", turns: [] });
      const turns = chat.turns as Record<string, unknown>[];
      turns.push({ id: `${turnId}-q`, role: "user", text: _messages[_messages.length - 1]?.content ?? "", pictures: options?.pictures ?? [] });
      turns.push({ id: turnId, role: "assistant", text: pieces.join(""), state: "done" });
      chat.updatedAt = Date.now();
    }
    setTimeout(() => {
      pieces.forEach((text) =>
        this.dispatch({ event: "assistant_delta", turn_id: turnId, text }),
      );
      this.dispatch({
        event: "assistant_done",
        turn_id: turnId,
        outcome: "ok",
        text: pieces.join(""),
      });
    }, 0);
    return Promise.resolve();
  }
  assistantCancel(_turnId: string): Promise<void> {
    return Promise.resolve();
  }
  pickFolder(_title: string): Promise<string | null> {
    return Promise.resolve("/home/you/Projects/website");
  }
  assistantUnload(): Promise<void> {
    this.languageModelList = this.languageModelList.map((model) =>
      model.assistant_state ? { ...model, assistant_state: "unloaded" } : model,
    );
    return Promise.resolve();
  }
  languageModelList: LanguageModel[] = [
    {
      id: "clean-up-small",
      name: "Clean-up model",
      publisher: "Example publisher",
      size_bytes: 623_000_000,
      license: "Apache-2.0 with a naming term",
      license_notice: "Must be shown with its original name.",
      roles: ["clean_up"],
      languages: ["en"],
      recommended_for: ["clean_up"],
      in_use_for: ["clean_up"],
      clean_up_state: "ready:npu",
    },
    {
      id: "assistant-4b",
      name: "Assistant model 4B",
      publisher: "Example publisher",
      size_bytes: 3_577_000_000,
      license: "Apache-2.0",
      roles: ["clean_up", "assistant"],
      languages: ["en"],
      recommended_for: ["assistant"],
      in_use_for: ["assistant"],
      assistant_state: "loaded:gpu",
    },
  ];
  languageModels(): Promise<LanguageModel[]> {
    return Promise.resolve(
      this.languageModelList.map((model) => ({ ...model })),
    );
  }
  selectLanguageModel(role: string, id: string | null): Promise<void> {
    this.languageModelList = this.languageModelList.map((model) => {
      const others = model.in_use_for.filter((r) => r !== role);
      return {
        ...model,
        in_use_for: model.id === id ? [...others, role] : others,
      };
    });
    return Promise.resolve();
  }
  /** In-memory meetings: recording is instant, transcribing finishes at once. */
  audio: AudioOptions = {
    echo_cancel: true,
    echo_cancel_active: true,
    voice_filter_available: true,
    voice_trained: false,
    only_my_voice: true,
    voice_training: "idle",
    noise_suppression: "standard",
    microphone: "",
    microphone_missing: false,
    sounds: false,
    voice_strictness: "normal",
    pipelines: {},
    microphones: [
      { name: "usb-mic", description: "USB condenser microphone" },
      { name: "laptop-mic", description: "Laptop microphone" },
    ],
  };
  setMicrophone(name: string): Promise<AudioOptions> {
    this.audio = { ...this.audio, microphone: name };
    return Promise.resolve({ ...this.audio });
  }
  setSounds(on: boolean): Promise<AudioOptions> {
    this.audio = { ...this.audio, sounds: on };
    return Promise.resolve({ ...this.audio });
  }
  setVoiceStrictness(level: string): Promise<AudioOptions> {
    this.audio = { ...this.audio, voice_strictness: level };
    return Promise.resolve({ ...this.audio });
  }
  setPipelineVoice(
    pipelineId: string,
    echoCancel: boolean | null,
    onlyMyVoice: boolean | null,
  ): Promise<AudioOptions> {
    const pipelines = { ...(this.audio.pipelines ?? {}) };
    if (echoCancel === null && onlyMyVoice === null)
      delete pipelines[pipelineId];
    else
      pipelines[pipelineId] = {
        echo_cancel: echoCancel,
        only_my_voice: onlyMyVoice,
      };
    this.audio = { ...this.audio, pipelines };
    return Promise.resolve({ ...this.audio });
  }
  setNoiseSuppression(level: string): Promise<AudioOptions> {
    this.audio = { ...this.audio, noise_suppression: level };
    return Promise.resolve({ ...this.audio });
  }
  hold: HoldKey = {
    key: null,
    state: "off",
    keys: [
      { code: 97, name: "Right Ctrl" },
      { code: 100, name: "Right Alt" },
    ],
  };
  holdKey(): Promise<HoldKey> {
    return Promise.resolve({ ...this.hold });
  }
  text: TextOptions = { numbers_as_digits: true, delivery_method: "paste" };
  setDeliveryMethod(method: string): Promise<TextOptions> {
    this.text = { ...this.text, delivery_method: method };
    return Promise.resolve({ ...this.text });
  }
  setCleanupIdle(minutes: number): Promise<TextOptions> {
    this.text = { ...this.text, cleanup_idle_minutes: minutes };
    return Promise.resolve({ ...this.text });
  }
  setSpokenBreaks(on: boolean): Promise<TextOptions> {
    this.text = { ...this.text, spoken_breaks: on };
    return Promise.resolve({ ...this.text });
  }
  textOptions(): Promise<TextOptions> {
    return Promise.resolve({ ...this.text });
  }
  setNumbersAsDigits(on: boolean): Promise<TextOptions> {
    this.text = { ...this.text, numbers_as_digits: on };
    return Promise.resolve({ ...this.text });
  }
  setHistoryRetention(_retention: string): Promise<void> {
    return Promise.resolve();
  }
  clearHistory(): Promise<void> {
    return Promise.resolve();
  }
  setHoldKey(key: number | null): Promise<HoldKey> {
    this.hold = { ...this.hold, key, state: key === null ? "off" : "reading" };
    return Promise.resolve({ ...this.hold });
  }
  startup: Autostart = {
    enabled: false,
    hidden: false,
    service_installed: true,
  };
  autostart(): Promise<Autostart> {
    return Promise.resolve({ ...this.startup });
  }
  setAutostart(enabled: boolean, hidden: boolean): Promise<Autostart> {
    this.startup = { ...this.startup, enabled, hidden: enabled && hidden };
    return Promise.resolve({ ...this.startup });
  }
  vocab: Vocabulary = {
    words: ["Narrivy", "Lahore"],
    replacements: [{ from: "btw", to: "by the way" }],
  };
  vocabulary(): Promise<Vocabulary> {
    return Promise.resolve(structuredClone(this.vocab));
  }
  setVocabulary(vocabulary: Vocabulary): Promise<Vocabulary> {
    const words = [
      ...new Set(vocabulary.words.map((w) => w.trim()).filter(Boolean)),
    ];
    this.vocab = { words, replacements: vocabulary.replacements };
    return Promise.resolve(structuredClone(this.vocab));
  }
  audioOptions(): Promise<AudioOptions> {
    return Promise.resolve({ ...this.audio });
  }
  setEchoCancel(on: boolean): Promise<AudioOptions> {
    this.audio = { ...this.audio, echo_cancel: on, echo_cancel_active: on };
    return Promise.resolve({ ...this.audio });
  }
  setOnlyMyVoice(on: boolean): Promise<AudioOptions> {
    this.audio = { ...this.audio, only_my_voice: on };
    return Promise.resolve({ ...this.audio });
  }
  trainVoice(): Promise<AudioOptions> {
    this.audio = { ...this.audio, voice_training: "recording" };
    setTimeout(() => {
      this.audio = {
        ...this.audio,
        voice_training: "idle",
        voice_trained: true,
      };
    }, 3000);
    return Promise.resolve({ ...this.audio });
  }
  forgetVoice(): Promise<AudioOptions> {
    this.audio = { ...this.audio, voice_trained: false };
    return Promise.resolve({ ...this.audio });
  }
  onFileDrop(_handler: (paths: string[]) => void): Promise<UnlistenFn> {
    return Promise.resolve(() => {});
  }
  autoNotes = false;
  processOnStop = false;
  meetingDevices = {
    microphone: "",
    speaker: "",
    call_audio: true,
    room_audio: true,
  };
  private meetingOptionsNow(): MeetingOptions {
    const microphones = [
      { name: "desk", description: "Desk microphone" },
      { name: "headset", description: "Headset microphone" },
    ];
    const speakers = [
      { name: "speakers", description: "Speakers" },
      { name: "headphones", description: "Headphones" },
    ];
    const { microphone, speaker, call_audio, room_audio } = this.meetingDevices;
    return {
      auto_notes: this.autoNotes,
      process_on_stop: this.processOnStop,
      screen: this.meetingScreen,
      screen_available: true,
      call_audio,
      room_audio,
      microphone,
      speaker,
      microphones,
      speakers,
      microphone_in_use:
        microphones.find((m) => m.name === microphone)?.description ??
        "Desk microphone",
      speaker_in_use:
        speakers.find((s) => s.name === speaker)?.description ?? "Speakers",
    };
  }
  meetingOptions(): Promise<MeetingOptions> {
    return Promise.resolve(this.meetingOptionsNow());
  }
  setMeetingDevices(choice: {
    microphone?: string;
    speaker?: string;
    call_audio?: boolean;
    room_audio?: boolean;
  }): Promise<MeetingOptions> {
    this.meetingDevices = { ...this.meetingDevices, ...choice };
    return Promise.resolve(this.meetingOptionsNow());
  }  meetingScreen = false;
  setMeetingScreen(on: boolean): Promise<MeetingOptions> {
    this.meetingScreen = on;
    return Promise.resolve(this.meetingOptionsNow());
  }
  meetingSlide(_id: string, _file: string): Promise<string> {
    return Promise.reject({ code: "meeting_slide_not_found", message: "no slide" });
  }

  setAutoNotes(on: boolean): Promise<MeetingOptions> {
    this.autoNotes = on;
    return Promise.resolve(this.meetingOptionsNow());
  }
  setProcessOnStop(on: boolean): Promise<MeetingOptions> {
    this.processOnStop = on;
    return Promise.resolve(this.meetingOptionsNow());
  }
  meetingImport(path: string): Promise<Meeting> {
    const name =
      path
        .split("/")
        .pop()
        ?.replace(/\.[^.]+$/, "") || "Imported recording";
    const meeting: Meeting = {
      id: `m${Date.now()}`,
      title: name,
      started_unix_ms: Date.now(),
      duration_ms: 0,
      state: "processing",
      progress: 0,
      stage: "import",
      model: "large-v3",
      speakers: 0,
    };
    this.meetingList = [meeting, ...this.meetingList];
    return Promise.resolve({ ...meeting });
  }
  meetingList: Meeting[] = [];
  meetingDetails: Record<string, MeetingDetail> = {};
  async screenStart(title: string | null, options: ScreenStartOptions): Promise<Meeting> {
    const meeting = await this.meetingStart(title);
    const sources = {
      watch: options.screen,
      share: options.share,
      screen_on: options.screen,
      sound_on: options.computerSound,
      mic_on: options.microphone,
    };
    Object.assign(meeting, sources);
    const kept = this.meetingList.find((m) => m.id === meeting.id);
    if (kept) Object.assign(kept, sources);
    return meeting;
  }
  meetingStart(title: string | null): Promise<Meeting> {
    if (this.meetingList.some((m) => m.state === "recording")) {
      return Promise.reject({
        code: "bad_request",
        message: "meeting_already_recording",
      });
    }
    const meeting: Meeting = {
      id: `m${Date.now()}`,
      title: title || "Meeting",
      started_unix_ms: Date.now(),
      duration_ms: 0,
      state: "recording",
      progress: 0,
      model: "large-v3",
      speakers: 0,
    };
    this.meetingList = [meeting, ...this.meetingList];
    this.meetingDetails[meeting.id] = {
      meeting,
      turns: [],
      names: {},
      folder: `/meetings/${meeting.id}`,
      models: ["large-v3", "turbo"],
      notes: {},
      recording: false,
    };
    this.dispatch({
      event: "meeting_changed",
      id: meeting.id,
      state: "recording",
      progress: 0,
    });
    return Promise.resolve({ ...meeting });
  }
  meetingSources(id: string, change: SourceChange): Promise<Meeting> {
    const meeting = this.meetingList.find((m) => m.id === id && m.state === "recording");
    if (!meeting) return Promise.reject({ code: "bad_request", message: "meeting_not_recording" });
    if (change.screen !== undefined) meeting.screen_on = change.screen;
    if (change.sound !== undefined) meeting.sound_on = change.sound;
    if (change.microphone !== undefined) meeting.mic_on = change.microphone;
    if (change.screen) meeting.watch = true;
    return Promise.resolve({ ...meeting });
  }
  meetingControl(id: string, action: "pause" | "resume" | "stop"): Promise<void> {
    const meeting = this.meetingList.find((m) => m.id === id);
    if (!meeting) return Promise.reject({ code: "bad_request", message: "meeting_not_found" });
    if (meeting.state === "recording") {
      return action === "stop"
        ? this.meetingStop()
        : Promise.reject({ code: "bad_request", message: "meeting_still_recording" });
    }
    if (action === "pause") meeting.paused = true;
    else if (action === "resume") meeting.paused = false;
    else {
      meeting.paused = false;
      meeting.state = "ready";
    }
    return Promise.resolve();
  }
  meetingStop(): Promise<void> {
    const meeting = this.meetingList.find((m) => m.state === "recording");
    if (!meeting)
      return Promise.reject({
        code: "bad_request",
        message: "meeting_not_recording",
      });
    const turns: MeetingTurn[] = [
      {
        speaker: "Person 1",
        source: "room",
        start_ms: 0,
        end_ms: 3000,
        text: "Shall we start with the release plan?",
        language: "en",
      },
      {
        speaker: "Person 2",
        source: "call",
        start_ms: 3500,
        end_ms: 7000,
        text: "جی، پہلے ادائیگی والے صفحے کی بات کر لیتے ہیں۔",
        language: "ur",
      },
      {
        speaker: "Person 3",
        source: "room",
        start_ms: 7500,
        end_ms: 10_000,
        text: "The payments page needs four more days.",
        language: "en",
      },
    ];
    Object.assign(meeting, {
      state: "ready",
      duration_ms: Date.now() - meeting.started_unix_ms,
      progress: 1000,
      speakers: 3,
      raw: !this.processOnStop,
    });
    this.meetingDetails[meeting.id] = {
      ...this.meetingDetails[meeting.id],
      meeting: { ...meeting },
      turns,
      recording: true,
    };
    this.dispatch({
      event: "meeting_changed",
      id: meeting.id,
      state: "ready",
      progress: 1000,
    });
    return Promise.resolve();
  }
  meetings(): Promise<Meeting[]> {
    return Promise.resolve(this.meetingList.map((m) => ({ ...m })));
  }
  meeting(id: string): Promise<MeetingDetail> {
    const detail = this.meetingDetails[id];
    if (!detail)
      return Promise.reject({
        code: "bad_request",
        message: "meeting_not_found",
      });
    const meeting = this.meetingList.find((m) => m.id === id) ?? detail.meeting;
    // A preview transcript that grows while the sound is recorded.
    if (meeting.state === "recording" && (meeting.sound_on || meeting.mic_on)) {
      const said = Math.floor((Date.now() - meeting.started_unix_ms) / 4000);
      const lines = [
        "Welcome back to the show. Today we look at the new laptop.",
        "The battery lasts about eleven hours in our tests.",
        "It starts at six hundred and ninety-nine dollars.",
        "Repairs are easy: every part has a QR code inside.",
      ];
      while (detail.turns.length < Math.min(said, lines.length)) {
        const n = detail.turns.length;
        detail.turns.push({
          speaker: "Call 1",
          source: "call",
          start_ms: n * 4000,
          end_ms: n * 4000 + 3500,
          text: lines[n],
          language: "en",
        });
      }
    }
    return Promise.resolve({
      ...detail,
      turns: [...detail.turns],
      meeting: { ...meeting },
      names: { ...detail.names },
    });
  }
  meetingProcess(id: string, model: string | null): Promise<void> {
    const meeting = this.meetingList.find((m) => m.id === id);
    if (meeting && model) meeting.model = model;
    if (meeting) meeting.raw = false;
    this.dispatch({
      event: "meeting_changed",
      id,
      state: "ready",
      progress: 1000,
    });
    return Promise.resolve();
  }
  meetingRename(id: string, speaker: string, name: string): Promise<void> {
    const detail = this.meetingDetails[id];
    if (detail) {
      if (name.trim()) detail.names[speaker] = name.trim();
      else delete detail.names[speaker];
    }
    return Promise.resolve();
  }
  meetingDelete(id: string): Promise<void> {
    this.meetingList = this.meetingList.filter((m) => m.id !== id);
    delete this.meetingDetails[id];
    this.dispatch({
      event: "meeting_changed",
      id,
      state: "deleted",
      progress: 0,
    });
    return Promise.resolve();
  }
  openMeetingFolder(_path: string, _file?: string): Promise<void> {
    return Promise.resolve();
  }
  /** Streams fixed notes and saves them. */
  meetingSummarize(id: string, turnId: string, kind: string): Promise<void> {
    const text =
      kind === "email"
        ? "Subject: Release plan follow-up\n\nHi all,\n\nThanks for today. The payments page needs four more days.\n\nBest,\n[Your name]"
        : "## Summary\n- Release plan reviewed.\n- Payments page needs four more days.\n\n## Action items\n- [ ] Finish the payments page — Person 3";
    setTimeout(() => {
      this.dispatch({ event: "assistant_delta", turn_id: turnId, text });
      this.dispatch({
        event: "assistant_done",
        turn_id: turnId,
        outcome: "ok",
        text,
      });
      const detail = this.meetingDetails[id];
      if (detail) detail.notes = { ...(detail.notes ?? {}), [kind]: text };
      this.dispatch({
        event: "meeting_changed",
        id,
        state: "ready",
        progress: 1000,
      });
    }, 0);
    return Promise.resolve();
  }
  onSessionEvent(handler: SessionEventHandler): Promise<UnlistenFn> {
    this.handlers.add(handler);
    return Promise.resolve(() => {
      this.handlers.delete(handler);
    });
  }
  /** Push a synthetic SessionStateChanged event to all subscribers. */
  emit(state: string): void {
    this.seq += 1;
    this.dispatch({ seq: this.seq, state });
  }
  protected dispatch(event: SessionEvent): void {
    this.handlers.forEach((h) => h(event));
  }
}
