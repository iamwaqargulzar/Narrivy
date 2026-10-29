// The AI assistant extension (D33): its chats and library, and voice mode.
// The daemon keeps the chats (so a reply is saved even when this window is
// closed, and routines can add to them) together with what the assistant
// remembers, its personas, projects, skills and routines. This context
// mirrors the open chat, streams replies into it, and runs voice mode.
// Nothing leaves this computer.
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react';
import { bridge, type SessionEvent, type VoiceSettings } from '../lib/bridge';
import { meetingContext } from '../lib/meetingContext';
import { useToasts } from '../ui/primitives/Toast';
import { useDaemonStatus } from './StatusContext';
import { t } from '../i18n';

export interface Turn {
  id: string;
  role: 'user' | 'assistant';
  text: string;
  /** Assistant turns: still streaming, stopped, or failed with a code. */
  state?: 'streaming' | 'done' | 'cancelled' | 'failed';
  failure?: string;
  /** Picture ids shown with a question. */
  pictures?: string[];
  /** Asked or answered by voice. */
  spoken?: boolean;
  at?: number;
  /** Tools the reply used or asks to use. */
  tools?: ToolUse[];
}

/** A tool call of a reply. `ask` waits for the user's decision. */
export interface ToolUse {
  callId?: string;
  server: string;
  tool: string;
  arguments: string;
  state: 'ask' | 'running' | 'done' | 'failed' | 'refused';
  summary?: string;
  /** What the step found (JSON), for the activity panel. */
  detail?: string;
}

/** A meeting a chat is about: its notes and transcript go with every question. */
export interface ChatMeeting {
  id: string;
  title: string;
  /** Notes and transcript, already cut to what the model can hold. */
  context: string;
}

export interface Chat {
  id: string;
  title: string;
  updatedAt: number;
  turns: Turn[];
  meeting?: ChatMeeting;
  persona?: string;
  project?: string;
  routine?: string;
  /** Tool servers switched on for the chat. */
  tools?: string[];
  allowedTools?: string[];
  /** Kept at the top of the chat list. */
  pinned?: boolean;
  /** The screen share this chat sees while it lasts. */
  live?: string;
  /** The shares and recordings made in this chat or given to it: the
   * chat's saved sessions. */
  sessions?: string[];
  /** The computer tools run without asking first. */
  fullAccess?: boolean;
  /** Where the computer tools work (the default folder when unset). */
  folder?: string;
  /** How much the model thinks before answering (unset: not at all). */
  thinking?: ThinkingLevel;
}

export type ThinkingLevel = 'short' | 'deep';

export interface ChatSummary {
  id: string;
  title: string;
  updatedAt: number;
  persona?: string;
  project?: string;
  meeting?: { id: string; title: string };
  routine?: string;
  pinned?: boolean;
  turns: number;
}

export interface Memory {
  id: string;
  text: string;
  createdAt?: number;
}
export interface Persona {
  id: string;
  name: string;
  instructions: string;
  voice?: number;
  speedPercent?: number;
}
export interface Project {
  id: string;
  name: string;
  folder?: string;
  instructions: string;
}
export interface Skill {
  id: string;
  name: string;
  description: string;
  instructions: string;
}
export interface Routine {
  id: string;
  name: string;
  prompt: string;
  every: 'hourly' | 'daily' | 'weekdays' | 'weekly';
  time: string;
  weekday: number;
  persona?: string;
  includeMeetings: boolean;
  enabled: boolean;
  lastRun?: number;
  chatId?: string;
}

/** An MCP server the user added. */
export interface ToolServer {
  id: string;
  name: string;
  command: string;
  args: string[];
}

export interface Library {
  memory: Memory[];
  persona: Persona[];
  project: Project[];
  skill: Skill[];
  routine: Routine[];
  tool: ToolServer[];
}
export type LibraryKind = keyof Library;

export type VoiceState = 'off' | 'starting' | 'listening' | 'hearing' | 'thinking' | 'speaking' | 'muted';

export interface VoiceTiming {
  asr_ms: number;
  reply_ms: number;
  voice_ms: number;
  total_ms: number;
}

export interface VoicePrefs {
  speedPercent: number;
  pushToTalk: boolean;
  interrupt: 'stop' | 'queue';
  sounds: boolean;
  voice: number;
  available: boolean;
}

/** How dictation into the chat box is handled: checked first, or sent. */
export type DictationMode = 'edit' | 'send';

export interface SendOptions {
  pictures?: string[];
  speak?: boolean;
}

interface AssistantContextValue {
  chats: ChatSummary[];
  current: Chat | null;
  busy: boolean;
  loadingModel: boolean;
  /** `loaded:<device>`, `unloaded`, or null when unknown. */
  modelState: string | null;
  /** The daemon is too old for kept chats. */
  unsupported: boolean;
  select: (id: string | null) => void;
  newChat: (setup?: { persona?: string; project?: string }) => void;
  remove: (id: string) => void;
  /** Rename or pin a chat (open or not). */
  changeChat: (id: string, change: { title?: string; pinned?: boolean }) => void;
  /** The assistant model's name, when one is chosen. */
  modelName: string | null;
  send: (question: string, options?: SendOptions) => void;
  /** While a reply is written: stop it and continue with this instruction. */
  steer: (instruction: string) => void;
  /** Change a question and ask it again; later turns are dropped. */
  editTurn: (turnId: string, text: string) => void;
  deleteTurn: (turnId: string) => void;
  /** Set the open chat's persona, project or tools. */
  setChatSetup: (setup: {
    persona?: string | null;
    project?: string | null;
    tools?: string[];
    fullAccess?: boolean;
    /** A working folder for this chat; null follows the default. */
    folder?: string | null;
    /** Think before answering; null answers straight away. */
    thinking?: ThinkingLevel | null;
  }) => void;
  /** Everything a reply streamed (thinking and tool calls too), by turn:
   * what the activity panel shows of its work. Not kept. */
  streamed: Record<string, string>;
  /** Where the computer tools work in chats without their own folder;
   * null is the home folder. */
  defaultFolder: string | null;
  setDefaultFolder: (folder: string | null) => void;
  /** Allow or refuse a tool call; `always` for the rest of the chat. */
  decideTool: (callId: string, allow: boolean, always: boolean) => void;
  /** Start a new chat about a meeting. */
  askAboutMeeting: (meeting: ChatMeeting) => void;
  stop: () => void;
  unload: () => void;
  /** Load the model now. */
  load: () => void;
  /** Unload the model after ten idle minutes (true), or keep it loaded. */
  autoUnload: boolean;
  setAutoUnload: (on: boolean) => void;
  library: Library;
  saveItem: <K extends LibraryKind>(kind: K, item: Partial<Library[K][number]>) => Promise<void>;
  deleteItem: (kind: LibraryKind, id: string) => Promise<void>;
  runRoutine: (id: string) => Promise<void>;
  /** Store a PNG (base64) for a question; its id. */
  addPicture: (pngBase64: string) => Promise<string>;
  picture: (id: string) => Promise<string | null>;
  voice: {
    state: VoiceState;
    partial: string;
    timing: VoiceTiming | null;
    error: string | null;
    prefs: VoicePrefs;
    /** Voice is off but its models stay loaded (the microphone is not
     * listened to); turning it on again is instant. */
    standby: boolean;
    /** Load the voice models and listen; with `standby`, only load them. */
    start: (options?: { standby?: boolean }) => void;
    /** Turn voice off, keeping its models loaded (true), or on again. */
    setStandby: (on: boolean) => void;
    /** Turn voice off and unload its models. */
    stop: () => void;
    setPrefs: (prefs: Partial<VoicePrefs> & { muted?: boolean }) => void;
    pushToTalk: (down: boolean) => void;
  };
  dictationMode: DictationMode;
  setDictationMode: (mode: DictationMode) => void;
  /** Screen share (live: the open chat sees it while it lasts) or screen
   * recording. Either is kept as a session when it stops, to give to a
   * chat when the user chooses. */
  watch: WatchState;
  startScreen: (mode: ScreenMode, what: { screen: boolean; microphone: boolean; computerSound: boolean }) => void;
  stopScreen: () => void;
  /** Pause, resume or stop going over a recording. */
  controlWatching: (action: 'pause' | 'resume' | 'stop') => void;
  /** Give a recorded session to the open chat (or a new one). */
  attachRecording: (meetingId: string) => Promise<void>;
  /** Close the "recording saved" bar. */
  dismissScreen: () => void;
}

export type ScreenMode = 'share' | 'record';

export interface WatchState {
  /** Sharing live or recording a session. */
  mode?: ScreenMode;
  /** The screen is kept too (off: only the sound, as a live transcript). */
  screen?: boolean;
  /** The meeting recording it, while there is one. */
  id: string | null;
  /** `watching`: sharing or recording now; `ready`: a recording is saved. */
  phase: 'idle' | 'starting' | 'watching' | 'processing' | 'ready' | 'error';
  startedAt: number;
  error?: string;
  /** The chat it belongs to: what was watched is added to it, and deleting
   * the chat stops the watching. */
  chatId?: string;
  /** Going over it is paused. */
  paused?: boolean;
}

const OLD_STORAGE_KEY = 'narrivy.assistant.chats.v1';
const DICTATION_MODE_KEY = 'narrivy.assistant.dictation.v1';

const AssistantContext = createContext<AssistantContextValue | null>(null);

let counter = 0;
function newId(prefix: string): string {
  counter += 1;
  return `${prefix}-${Date.now().toString(36)}-${counter}`;
}

/** A chat's saved sessions with `id` added (once). */
function withSession(sessions: string[] | undefined, id: string): string[] {
  return sessions?.includes(id) ? sessions : [...(sessions ?? []), id];
}

function titleOf(question: string): string {
  const line = question.trim().split('\n')[0] ?? '';
  return line.length > 60 ? `${line.slice(0, 57)}…` : line;
}

const EMPTY_LIBRARY: Library = { memory: [], persona: [], project: [], skill: [], routine: [], tool: [] };
const DEFAULT_PREFS: VoicePrefs = {
  speedPercent: 100,
  pushToTalk: false,
  interrupt: 'stop',
  sounds: true,
  voice: 3,
  available: false,
};

function code(error: unknown): string {
  return (error as { code?: string })?.code ?? 'unavailable';
}

/** Chats kept by an older version of this app, moved into the daemon once. */
async function importOldChats(): Promise<void> {
  let raw: string | null = null;
  try {
    raw = window.localStorage.getItem(OLD_STORAGE_KEY);
  } catch {
    return;
  }
  if (!raw) return;
  await bridge.assistantLibrary('import', 'chat', null, raw);
  try {
    window.localStorage.removeItem(OLD_STORAGE_KEY);
  } catch {
    // Kept; importing again skips chats already there.
  }
}

export function AssistantProvider({ children }: { children: ReactNode }) {
  const [chats, setChats] = useState<ChatSummary[]>([]);
  const [current, setCurrent] = useState<Chat | null>(null);
  const [loadingModel, setLoadingModel] = useState(false);
  const [modelState, setModelState] = useState<string | null>(null);
  const [modelName, setModelName] = useState<string | null>(null);
  const { push } = useToasts();
  // A short notice when the model loads or is unloaded (not on first sight).
  const lastModelState = useRef<string | null | undefined>(undefined);
  useEffect(() => {
    const before = lastModelState.current;
    lastModelState.current = modelState;
    if (before === undefined || before === null || modelState === null || before === modelState) return;
    const loadedNow = modelState.startsWith('loaded:');
    if (loadedNow === before.startsWith('loaded:')) return;
    push(
      loadedNow
        ? t('assistant.notice.loaded', { device: modelState.slice('loaded:'.length).toUpperCase() })
        : t('assistant.notice.unloaded'),
    );
  }, [modelState, push]);
  const [unsupported, setUnsupported] = useState(false);
  const [autoUnload, setAutoUnloadState] = useState(true);
  const [defaultFolder, setDefaultFolderState] = useState<string | null>(null);
  const [streamed, setStreamed] = useState<Record<string, string>>({});
  const [watch, setWatch] = useState<WatchState>({ id: null, phase: 'idle', startedAt: 0 });
  const watchRef = useRef(watch);
  watchRef.current = watch;
  /** When the watched meeting was first ready, to stop waiting for slow
   * picture descriptions. */
  const watchReadyAt = useRef(0);
  /** Raw sessions given to a chat and being gone over now: meeting to the
   * chat that gets the full transcript once it is ready. */
  const goingOver = useRef(new Map<string, string>());
  /** `saveChat`, for callbacks defined before it. */
  const saveChatRef = useRef<(chat: Chat) => Promise<void>>(async () => {});
  const [library, setLibrary] = useState<Library>(EMPTY_LIBRARY);
  const running = useRef<{ chatId: string; turnId: string; speak: boolean } | null>(null);
  const [busy, setBusy] = useState(false);
  const steering = useRef<string | null>(null);
  const currentRef = useRef<Chat | null>(null);
  currentRef.current = current;

  const [voiceState, setVoiceState] = useState<VoiceState>('off');
  const [standby, setStandby] = useState(false);
  const [partial, setPartial] = useState('');
  const [timing, setTiming] = useState<VoiceTiming | null>(null);
  const [voiceError, setVoiceError] = useState<string | null>(null);
  const [prefs, setPrefsState] = useState<VoicePrefs>(DEFAULT_PREFS);
  const prefsRef = useRef(prefs);
  prefsRef.current = prefs;
  const voiceOn = useRef(false);
  /** Questions heard while a reply was being written, asked after it. */
  const heardQueue = useRef<string[]>([]);
  const [dictationMode, setDictationModeState] = useState<DictationMode>(() => {
    try {
      return window.localStorage.getItem(DICTATION_MODE_KEY) === 'send' ? 'send' : 'edit';
    } catch {
      return 'edit';
    }
  });

  const refreshChats = useCallback(async () => {
    try {
      const list = JSON.parse(await bridge.assistantLibrary('list', 'chat')) as ChatSummary[];
      setChats(Array.isArray(list) ? list : []);
      setUnsupported(false);
    } catch (error) {
      if (code(error) === 'incompatible_protocol') setUnsupported(true);
    }
  }, []);

  const refreshLibrary = useCallback(async (kind?: LibraryKind) => {
    const kinds: LibraryKind[] = kind ? [kind] : ['memory', 'persona', 'project', 'skill', 'routine', 'tool'];
    const found = await Promise.all(
      kinds.map(async (k) => {
        try {
          const list = JSON.parse(await bridge.assistantLibrary('list', k)) as unknown[];
          return [k, Array.isArray(list) ? list : []] as const;
        } catch {
          return [k, null] as const;
        }
      }),
    );
    setLibrary((old) => {
      const next = { ...old };
      for (const [k, list] of found) if (list) (next as Record<string, unknown[]>)[k] = list;
      return next;
    });
  }, []);

  const openChat = useCallback(async (id: string | null) => {
    if (!id) {
      setCurrent(null);
      return;
    }
    try {
      const chat = JSON.parse(await bridge.assistantLibrary('get', 'chat', id)) as Chat;
      setCurrent(chat);
    } catch {
      setCurrent(null);
    }
  }, []);

  const refreshModelState = useCallback(() => {
    bridge
      .languageModels()
      .then((models) => {
        const model = models.find((m) => m.in_use_for.includes('assistant'));
        setModelState(model?.assistant_state ?? null);
        setModelName(model?.name ?? null);
      })
      .catch(() => setModelState(null));
  }, []);

  const refreshVoicePrefs = useCallback(async () => {
    try {
      const value = JSON.parse(await bridge.assistantLibrary('get', 'voice')) as VoicePrefs & { running?: boolean };
      setPrefsState({
        speedPercent: value.speedPercent ?? 100,
        pushToTalk: Boolean(value.pushToTalk),
        interrupt: value.interrupt === 'queue' ? 'queue' : 'stop',
        sounds: value.sounds !== false,
        voice: value.voice ?? 3,
        available: Boolean(value.available),
      });
    } catch {
      setPrefsState((old) => ({ ...old, available: false }));
    }
  }, []);

  useEffect(() => {
    bridge
      .assistantLibrary('get', 'model')
      .then((json) => {
        const settings = JSON.parse(json) as { autoUnload?: boolean; folder?: string | null };
        setAutoUnloadState(settings.autoUnload !== false);
        setDefaultFolderState(settings.folder ?? null);
      })
      .catch(() => {});
  }, []);

  // The service may come up after the app (it restarts when a package is
  // installed): ask again for the model and voice once it answers, or the
  // model switches and picker stay hidden.
  const { status: daemonStatus, error: daemonError } = useDaemonStatus();
  const connected = daemonStatus !== null && daemonError === null;
  useEffect(() => {
    if (!connected) return;
    refreshModelState();
    void refreshVoicePrefs();
  }, [connected, refreshModelState, refreshVoicePrefs]);

  useEffect(() => {
    refreshModelState();
    void importOldChats()
      .catch(() => {})
      .then(async () => {
        await refreshChats();
        await refreshLibrary();
        await refreshVoicePrefs();
      });
  }, [refreshChats, refreshLibrary, refreshModelState, refreshVoicePrefs]);

  const updateTurn = useCallback((turnId: string, change: (turn: Turn) => Turn) => {
    setCurrent((chat) =>
      chat && chat.turns.some((turn) => turn.id === turnId)
        ? { ...chat, updatedAt: Date.now(), turns: chat.turns.map((t) => (t.id === turnId ? change(t) : t)) }
        : chat,
    );
  }, []);

  /** A stopped session is saved: kept raw (as heard), it is ready to give
   * to a chat at once; gone over, once its pictures are described too (or
   * after waiting long enough). */
  const finishWatching = useCallback(async (id: string) => {
    let detail;
    try {
      detail = await bridge.meeting(id);
    } catch {
      return;
    }
    if (detail.meeting.raw) {
      watchReadyAt.current = 0;
      setWatch((old) => (old.id === id ? { ...old, phase: 'ready', paused: false } : old));
      return;
    }
    if (!watchReadyAt.current) watchReadyAt.current = Date.now();
    const waiting = (detail.slides ?? []).some((slide) => !slide.description);
    if (waiting && Date.now() - watchReadyAt.current < 5 * 60_000) return;
    watchReadyAt.current = 0;
    setWatch((old) => (old.id === id ? { ...old, phase: 'ready', paused: false } : old));
  }, []);

  /** Give a recorded session to the open chat, or a new one. A session
   * kept raw is given with its live transcript, and gone over now (the
   * full transcript, the screens described): the chat gets that once it
   * is ready. */
  const attachRecording = useCallback(async (id: string) => {
    const detail = await bridge.meeting(id);
    const nameOf = (speaker: string) => detail.names[speaker] ?? speaker;
    const context = meetingContext(detail, nameOf);
    const open = currentRef.current;
    const base: Chat = open ?? { id: newId('chat'), title: detail.meeting.title, updatedAt: Date.now(), turns: [] };
    // A chat given something before keeps it and gains this one.
    const meeting: ChatMeeting =
      base.meeting && base.meeting.id !== id
        ? {
            id,
            title: `${base.meeting.title} + ${detail.meeting.title}`,
            context: `${base.meeting.context}\n\n--- ${detail.meeting.title} ---\n${context}`,
          }
        : { id, title: detail.meeting.title, context };
    const next: Chat = {
      ...base,
      title: base.title || detail.meeting.title,
      meeting,
      sessions: withSession(base.sessions, id),
    };
    setCurrent(next);
    await bridge.assistantLibrary('put', 'chat', null, JSON.stringify({ ...next, updatedAt: Date.now() }));
    if (watchRef.current.id === id && watchRef.current.phase === 'ready') {
      setWatch({ id: null, phase: 'idle', startedAt: 0 });
    }
    if (detail.meeting.raw && detail.meeting.state === 'ready' && !goingOver.current.has(id)) {
      goingOver.current.set(id, next.id);
      // Another one being gone over: this one stays raw, with its live
      // transcript, and can be gone over later.
      await bridge.meetingProcess(id, null, null, null).catch(() => goingOver.current.delete(id));
    }
  }, []);

  /** The share or recording stops (from here, or ended by the service:
   * every source switched off, or stopped elsewhere): its chat no longer
   * sees the screen, and what was shared is kept as a session. */
  const endWatching = useCallback(() => {
    const { mode, chatId } = watchRef.current;
    watchReadyAt.current = 0;
    if (mode === 'share' && chatId) {
      const open = currentRef.current;
      if (open?.id === chatId) void saveChatRef.current({ ...open, live: undefined });
      else
        void bridge
          .assistantLibrary('get', 'chat', chatId)
          .then((json) => {
            const chat = JSON.parse(json) as Chat;
            delete chat.live;
            return bridge.assistantLibrary('put', 'chat', null, JSON.stringify(chat));
          })
          .catch(() => {});
    }
    setWatch((old) => ({ ...old, phase: 'processing' }));
  }, []);

  /** A raw session given to a chat is gone over: the chat gets the full
   * transcript in place of the live one. */
  const updateGivenSession = useCallback(async (id: string, chatId: string) => {
    const detail = await bridge.meeting(id);
    if (detail.meeting.raw || detail.meeting.state !== 'ready') return;
    const nameOf = (speaker: string) => detail.names[speaker] ?? speaker;
    const context = meetingContext(detail, nameOf);
    const open = currentRef.current;
    const chat: Chat =
      open?.id === chatId ? open : (JSON.parse(await bridge.assistantLibrary('get', 'chat', chatId)) as Chat);
    // Given alone (a chat given several keeps what it was given).
    if (chat.meeting?.id !== id) return;
    const next: Chat = { ...chat, meeting: { ...chat.meeting, context } };
    if (currentRef.current?.id === chatId) setCurrent(next);
    await bridge.assistantLibrary('put', 'chat', null, JSON.stringify({ ...next, updatedAt: Date.now() }));
  }, []);

  // `send` is used from the event handler (voice, steering); keep the latest.
  const sendRef = useRef<(question: string, options?: SendOptions) => void>(() => {});

  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    void bridge
      .onSessionEvent((ev: SessionEvent) => {
        if (ev.event === 'capability_changed') {
          const capability = (ev as { capability?: string }).capability;
          const available = Boolean((ev as { available?: boolean }).available);
          if (capability === 'assistant.loading') setLoadingModel(available);
          if (capability === 'assistant.loaded') refreshModelState();
          return;
        }
        if (ev.event === 'assistant_library_changed') {
          const kind = (ev as { kind?: string }).kind;
          void refreshChats();
          if (kind === 'chat' && ev.id && currentRef.current?.id === ev.id && !running.current) {
            void openChat(ev.id);
          }
          if (kind && kind !== 'chat') void refreshLibrary(kind as LibraryKind);
          return;
        }
        if (ev.event === 'meeting_changed' && ev.id && goingOver.current.has(ev.id)) {
          const chatId = goingOver.current.get(ev.id) ?? '';
          if (ev.state === 'deleted' || ev.state?.startsWith('failed')) goingOver.current.delete(ev.id);
          // Ready with its transcript, then again once its screens are
          // described.
          else if (ev.state === 'ready' && !ev.stage) void updateGivenSession(ev.id, chatId).catch(() => {});
        }
        if (ev.event === 'meeting_changed' && ev.id && ev.id === watchRef.current.id) {
          if (ev.state === 'failed:meeting_stopped' || ev.state === 'deleted') {
            setWatch({ id: null, phase: 'idle', startedAt: 0 });
            return;
          }
          const paused = ev.stage === 'paused';
          if (paused !== Boolean(watchRef.current.paused)) setWatch((old) => ({ ...old, paused }));
          if (ev.state?.startsWith('failed')) {
            setWatch({ id: null, phase: 'error', startedAt: 0, error: ev.state.slice('failed:'.length) });
            return;
          }
          // Stopped by the service (every source switched off) or from
          // elsewhere: this share is over too.
          if (ev.state !== 'recording' && watchRef.current.phase === 'watching') endWatching();
          if (ev.state === 'ready' && !ev.stage) void finishWatching(ev.id);
          return;
        }
        if (ev.event === 'voice') {
          const kind = (ev as { kind?: string }).kind;
          if (kind === 'state' && ev.state) {
            if (ev.state === 'stopped') {
              voiceOn.current = false;
              setStandby(false);
              setVoiceState('off');
              setPartial('');
            } else {
              setVoiceState(ev.state as VoiceState);
              if (ev.state === 'listening') setPartial('');
            }
          } else if (kind === 'partial' && typeof ev.text === 'string') {
            setPartial(ev.text);
          } else if (kind === 'heard' && typeof ev.text === 'string' && voiceOn.current) {
            setPartial('');
            const text = ev.text;
            if (running.current) {
              if (prefsRef.current.interrupt === 'queue') heardQueue.current.push(text);
              else steering.current = text;
            } else {
              sendRef.current(text, { speak: true });
            }
          } else if (kind === 'interrupt') {
            if (running.current) void bridge.assistantCancel(running.current.turnId);
          } else if (kind === 'timing') {
            setTiming((ev as { timing?: VoiceTiming }).timing ?? null);
          } else if (kind === 'error') {
            setVoiceError((ev as { code?: string }).code ?? 'voice_failed');
          }
          return;
        }
        if (!ev.turn_id) return;
        const id = ev.turn_id;
        if (ev.event === 'assistant_tool') {
          const tool = ev as SessionEvent & {
            call_id: string;
            server: string;
            tool: string;
            arguments: string;
            summary?: string;
            detail?: string;
          };
          const use: ToolUse = {
            callId: tool.call_id,
            server: tool.server,
            tool: tool.tool,
            arguments: tool.arguments,
            state: (ev.state ?? 'running') as ToolUse['state'],
            summary: tool.summary,
            detail: tool.detail,
          };
          updateTurn(id, (turn) => {
            const tools = turn.tools ?? [];
            const known = tools.some((t) => t.callId === use.callId);
            return {
              ...turn,
              tools: known ? tools.map((t) => (t.callId === use.callId ? use : t)) : [...tools, use],
            };
          });
          return;
        }
        if (ev.event === 'assistant_delta' && typeof ev.text === 'string') {
          const piece = ev.text;
          updateTurn(id, (turn) => ({ ...turn, text: turn.text + piece }));
          setStreamed((all) => ({ ...all, [id]: (all[id] ?? '') + piece }));
        } else if (ev.event === 'assistant_done') {
          const outcome = ev.outcome ?? 'failed:unknown';
          const failed = outcome.startsWith('failed:');
          updateTurn(id, (turn) => ({
            ...turn,
            text: typeof ev.text === 'string' && ev.text.length > 0 ? ev.text : turn.text,
            state: failed ? 'failed' : outcome === 'cancelled' ? 'cancelled' : 'done',
            failure: failed ? outcome.slice('failed:'.length) : undefined,
          }));
          if (running.current?.turnId === id) {
            const wasSpoken = running.current.speak;
            running.current = null;
            setBusy(false);
            void refreshChats();
            const next = steering.current ?? heardQueue.current.shift();
            steering.current = null;
            if (next) window.setTimeout(() => sendRef.current(next, { speak: wasSpoken || voiceOn.current }), 0);
          }
        }
      })
      .then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [endWatching, finishWatching, openChat, refreshChats, refreshLibrary, refreshModelState, updateGivenSession, updateTurn]);

  const send = useCallback(
    (question: string, options?: SendOptions) => {
      const text = question.trim();
      if (!text || running.current) return;
      const turnId = newId('turn');
      const speak = Boolean(options?.speak);
      const pictures = options?.pictures ?? [];
      const user: Turn = { id: `${turnId}-q`, role: 'user', text, pictures, spoken: speak, at: Date.now() };
      const reply: Turn = { id: turnId, role: 'assistant', text: '', state: 'streaming', spoken: speak };
      const base: Chat = currentRef.current ?? {
        id: newId('chat'),
        title: titleOf(text),
        updatedAt: Date.now(),
        turns: [],
      };
      const next: Chat = { ...base, updatedAt: Date.now(), turns: [...base.turns, user, reply] };
      setCurrent(next);
      running.current = { chatId: next.id, turnId, speak };
      setBusy(true);
      const ask = async () => {
        // A chat the daemon does not know yet (new, or about a meeting) is
        // saved first, with its setup.
        if (!chats.some((chat) => chat.id === base.id)) {
          await bridge.assistantLibrary('put', 'chat', null, JSON.stringify({ ...base, turns: base.turns }));
        }
        await bridge.assistantSend(turnId, [{ role: 'user', content: text }], {
          chatId: next.id,
          speak,
          pictures,
        });
      };
      ask()
        .then(() => refreshChats())
        .catch((error: unknown) => {
          updateTurn(turnId, (turn) => ({ ...turn, state: 'failed', failure: code(error) }));
          running.current = null;
          setBusy(false);
        });
    },
    [chats, refreshChats, updateTurn],
  );
  sendRef.current = send;

  const saveChat = useCallback(
    async (chat: Chat) => {
      setCurrent(chat);
      await bridge.assistantLibrary('put', 'chat', null, JSON.stringify({ ...chat, updatedAt: Date.now() }));
      await refreshChats();
    },
    [refreshChats],
  );
  saveChatRef.current = saveChat;

  const setPrefs = useCallback(
    (change: Partial<VoicePrefs> & { muted?: boolean }) => {
      setPrefsState((old) => ({ ...old, ...change }));
      const settings: VoiceSettings = {};
      if (change.speedPercent !== undefined) settings.speed_percent = change.speedPercent;
      if (change.pushToTalk !== undefined) settings.push_to_talk = change.pushToTalk;
      if (change.interrupt !== undefined) settings.interrupt = change.interrupt;
      if (change.sounds !== undefined) settings.sounds = change.sounds;
      if (change.voice !== undefined) settings.voice = change.voice;
      if (change.muted !== undefined) {
        settings.muted = change.muted;
        setVoiceState((state) => (change.muted ? 'muted' : state === 'muted' ? 'listening' : state));
      }
      void bridge.voiceMode('settings', settings).catch(() => {});
    },
    [],
  );

  const value = useMemo<AssistantContextValue>(
    () => ({
      streamed,
      chats,
      current,
      busy,
      loadingModel,
      modelState,
      unsupported,
      select: (id) => void openChat(id),
      newChat: (setup) => {
        if (running.current) return;
        setCurrent(
          setup && (setup.persona || setup.project)
            ? { id: newId('chat'), title: '', updatedAt: Date.now(), turns: [], ...setup }
            : null,
        );
      },
      remove: (id: string) => {
        if (running.current?.chatId === id) return;
        // Deleting a chat stops going over what it watched; a share still
        // running is stopped and saved, below.
        const watching = watchRef.current;
        if (watching.chatId === id && watching.phase === 'processing') {
          if (watching.id) void bridge.meetingControl(watching.id, 'stop').catch(() => {});
          setWatch({ id: null, phase: 'idle', startedAt: 0 });
        }
        // A screen share the chat sees stops too (it is saved as a session):
        // with its chat gone nothing else would show or stop it.
        void bridge
          .assistantLibrary('get', 'chat', id)
          .then(async (json) => {
            const live = (JSON.parse(json) as Chat).live;
            if (live && (await bridge.meeting(live)).meeting.state === 'recording') {
              if (watchRef.current.id === live) setWatch((old) => ({ ...old, phase: 'processing' }));
              await bridge.meetingStop();
            }
          })
          .catch(() => {})
          .then(() => bridge.assistantLibrary('delete', 'chat', id))
          .then(() => refreshChats())
          .catch(() => {});
        setCurrent((chat) => (chat?.id === id ? null : chat));
      },
      changeChat: (id, change) => {
        const open = currentRef.current;
        const apply = (chat: Chat): Chat => ({
          ...chat,
          ...(change.title !== undefined ? { title: change.title.trim() || chat.title } : {}),
          ...(change.pinned !== undefined ? { pinned: change.pinned } : {}),
        });
        if (open?.id === id) {
          if (chats.some((c) => c.id === id)) void saveChat(apply(open));
          else setCurrent(apply(open));
          return;
        }
        void bridge
          .assistantLibrary('get', 'chat', id)
          .then((json) => {
            const chat = apply(JSON.parse(json) as Chat);
            return bridge.assistantLibrary('put', 'chat', null, JSON.stringify(chat));
          })
          .then(() => refreshChats())
          .catch(() => {});
      },
      modelName,
      send,
      steer: (instruction: string) => {
        const text = instruction.trim();
        if (!text) return;
        if (!running.current) {
          send(text);
          return;
        }
        steering.current = text;
        void bridge.assistantCancel(running.current.turnId);
      },
      editTurn: (turnId: string, text: string) => {
        const chat = currentRef.current;
        if (!chat || running.current) return;
        const index = chat.turns.findIndex((turn) => turn.id === turnId);
        if (index < 0 || chat.turns[index].role !== 'user') return;
        const pictures = chat.turns[index].pictures;
        const kept: Chat = { ...chat, turns: chat.turns.slice(0, index) };
        void saveChat(kept).then(() => {
          currentRef.current = kept;
          send(text, { pictures });
        });
      },
      deleteTurn: (turnId: string) => {
        const chat = currentRef.current;
        if (!chat || running.current) return;
        void saveChat({ ...chat, turns: chat.turns.filter((turn) => turn.id !== turnId) });
      },
      setChatSetup: (setup) => {
        const chat = currentRef.current;
        const next: Chat = chat
          ? { ...chat }
          : { id: newId('chat'), title: '', updatedAt: Date.now(), turns: [] };
        if (setup.persona !== undefined) next.persona = setup.persona ?? undefined;
        if (setup.project !== undefined) next.project = setup.project ?? undefined;
        if (setup.tools !== undefined) next.tools = setup.tools;
        if (setup.fullAccess !== undefined) next.fullAccess = setup.fullAccess || undefined;
        if (setup.folder !== undefined) next.folder = setup.folder ?? undefined;
        if (setup.thinking !== undefined) next.thinking = setup.thinking ?? undefined;
        if (chat && chats.some((c) => c.id === chat.id)) void saveChat(next);
        else setCurrent(next);
      },
      decideTool: (callId: string, allow: boolean, always: boolean) => {
        void bridge.assistantToolDecision(callId, allow, always).catch(() => {});
      },
      askAboutMeeting: (meeting: ChatMeeting) => {
        if (running.current) return;
        setCurrent({ id: newId('chat'), title: meeting.title, updatedAt: Date.now(), turns: [], meeting });
      },
      stop: () => {
        steering.current = null;
        heardQueue.current = [];
        if (running.current) void bridge.assistantCancel(running.current.turnId);
      },
      load: () => {
        setLoadingModel(true);
        void bridge.assistantLibrary('run', 'model').catch(() => setLoadingModel(false));
      },
      defaultFolder,
      setDefaultFolder: (folder: string | null) => {
        setDefaultFolderState(folder);
        void bridge.assistantLibrary('put', 'model', null, JSON.stringify({ folder })).catch(() => {});
      },
      autoUnload,
      setAutoUnload: (on: boolean) => {
        setAutoUnloadState(on);
        void bridge.assistantLibrary('put', 'model', null, JSON.stringify({ autoUnload: on })).catch(() => {});
      },
      unload: () => {
        void bridge
          .assistantUnload()
          .then(() => window.setTimeout(refreshModelState, 300))
          .catch(() => {});
      },
      library,
      saveItem: async (kind, item) => {
        await bridge.assistantLibrary('put', kind, null, JSON.stringify(item));
        await refreshLibrary(kind);
      },
      deleteItem: async (kind, id) => {
        await bridge.assistantLibrary('delete', kind, id);
        await refreshLibrary(kind);
      },
      runRoutine: async (id) => {
        await bridge.assistantLibrary('run', 'routine', id);
        await refreshLibrary('routine');
        await refreshChats();
      },
      addPicture: async (png: string) => {
        const answer = JSON.parse(await bridge.assistantLibrary('put', 'picture', null, png)) as { id: string };
        return answer.id;
      },
      picture: async (id: string) => {
        try {
          const answer = JSON.parse(await bridge.assistantLibrary('get', 'picture', id)) as { png?: string };
          return answer.png ? `data:image/png;base64,${answer.png}` : null;
        } catch {
          return null;
        }
      },
      voice: {
        state: voiceState,
        partial,
        timing,
        error: voiceError,
        prefs,
        standby,
        start: (options) => {
          setVoiceError(null);
          setTiming(null);
          voiceOn.current = true;
          setStandby(Boolean(options?.standby));
          setVoiceState('starting');
          bridge
            .voiceMode('start')
            .then(() => {
              if (options?.standby) setPrefs({ muted: true });
            })
            .catch((error: unknown) => {
              voiceOn.current = false;
              setStandby(false);
              setVoiceState('off');
              setVoiceError(code(error));
            });
        },
        setStandby: (on) => {
          setStandby(on);
          setPrefs({ muted: on });
          if (on && running.current?.speak) void bridge.assistantCancel(running.current.turnId);
        },
        stop: () => {
          voiceOn.current = false;
          setStandby(false);
          heardQueue.current = [];
          setVoiceState('off');
          setPartial('');
          void bridge.voiceMode('stop').catch(() => {});
          if (running.current?.speak) void bridge.assistantCancel(running.current.turnId);
        },
        setPrefs,
        pushToTalk: (down: boolean) => {
          void bridge.voiceMode('ptt', null, down).catch(() => {});
        },
      },
      watch,
      startScreen: (mode, what) => {
        const stamp = new Date().toLocaleString([], { dateStyle: 'medium', timeStyle: 'short' });
        const kind = what.screen
          ? mode === 'share'
            ? 'Screen share'
            : 'Screen recording'
          : mode === 'share'
            ? 'Live transcript'
            : 'Sound recording';
        const title = `${kind} ${stamp}`;
        // A share belongs to the open chat (kept first) or a new one: that
        // chat sees the screen while it lasts. A recording is kept with the
        // open chat, if there is one.
        let chat: Chat | null = currentRef.current;
        if (mode === 'share') {
          chat = chat ?? { id: newId('chat'), title, updatedAt: Date.now(), turns: [] };
          if (!chat.title) chat.title = title;
        }
        // Only a share's chat sees the screen.
        const chatId = mode === 'share' ? chat?.id : undefined;
        setWatch({ id: null, mode, screen: what.screen, phase: 'starting', startedAt: Date.now(), chatId });
        void (chat ? saveChat(chat) : Promise.resolve())
          .then(() =>
            bridge.screenStart(title, {
              screen: what.screen,
              share: mode === 'share',
              microphone: what.microphone,
              computerSound: what.computerSound,
            }),
          )
          .then(async (meeting) => {
            if (watchRef.current.phase !== 'starting') {
              // Cancelled meanwhile.
              await bridge.meetingStop().catch(() => {});
              return;
            }
            setWatch({ id: meeting.id, mode, screen: what.screen, phase: 'watching', startedAt: Date.now(), chatId });
            if (chat) {
              const open = currentRef.current?.id === chat.id ? currentRef.current : chat;
              await saveChat({
                ...open,
                live: mode === 'share' ? meeting.id : open.live,
                sessions: withSession(open.sessions, meeting.id),
              });
            }
          })
          .catch((error: unknown) => setWatch({ id: null, phase: 'error', startedAt: 0, error: code(error) }));
      },
      stopScreen: () => {
        const { id } = watchRef.current;
        endWatching();
        bridge.meetingStop().catch(() => {
          if (id) void finishWatching(id);
        });
      },
      attachRecording,
      dismissScreen: () => setWatch({ id: null, phase: 'idle', startedAt: 0 }),
      controlWatching: (action) => {
        const { id } = watchRef.current;
        if (!id) {
          if (action === 'stop') setWatch({ id: null, phase: 'idle', startedAt: 0 });
          return;
        }
        if (action === 'stop') {
          watchReadyAt.current = 0;
          setWatch({ id: null, phase: 'idle', startedAt: 0 });
        } else {
          setWatch((old) => ({ ...old, paused: action === 'pause' }));
        }
        void bridge.meetingControl(id, action).catch((error: unknown) => {
          if (action !== 'stop') setWatch((old) => ({ ...old, paused: action === 'resume', error: code(error) }));
        });
      },
      dictationMode,
      setDictationMode: (mode) => {
        setDictationModeState(mode);
        try {
          window.localStorage.setItem(DICTATION_MODE_KEY, mode);
        } catch {
          // Kept for this session.
        }
      },
    }),
    [
      streamed,
      defaultFolder,
      autoUnload,
      finishWatching,
      endWatching,
      attachRecording,
      watch,
      busy,
      chats,
      current,
      dictationMode,
      library,
      loadingModel,
      modelState,
      modelName,
      openChat,
      partial,
      prefs,
      refreshChats,
      refreshLibrary,
      refreshModelState,
      saveChat,
      send,
      setPrefs,
      timing,
      unsupported,
      voiceError,
      voiceState,
      standby,
    ],
  );

  return <AssistantContext.Provider value={value}>{children}</AssistantContext.Provider>;
}

export function useAssistant(): AssistantContextValue {
  const ctx = useContext(AssistantContext);
  if (!ctx) throw new Error('useAssistant must be used within AssistantProvider');
  return ctx;
}
