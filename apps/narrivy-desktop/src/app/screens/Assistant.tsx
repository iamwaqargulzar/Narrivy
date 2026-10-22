// The AI assistant extension (D33, F24): chat with a language model that runs
// on this computer, by typing, dictating or talking (voice mode). The chats
// and the library (memory, personas, projects, skills, routines) are kept by
// the daemon, on this computer only.
import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type ClipboardEvent,
  type DragEvent,
  type KeyboardEvent,
  type ReactNode,
} from 'react';
import { Icon } from '../../ui/icons';
import { t } from '../../i18n';
import { bridge, type Meeting, type SessionEvent } from '../../lib/bridge';
import { useAssistant, type ChatSummary, type ThinkingLevel, type ToolUse, type Turn } from '../../state/AssistantContext';
import { VoicePanel } from '../components/assistant/VoicePanel';
import { AssistantLibraryView } from '../components/assistant/AssistantLibrary';
import { WatchBar, WatchButton } from '../components/assistant/WatchBar';
import { ActivityPanel, hasActivity as activityIn } from '../components/assistant/ActivityPanel';
import { LivePanel, LiveSessionProvider, useLive, type LiveSource } from '../components/assistant/LiveSession';

/** Where the model is, in words: "Loaded on GPU" or "Not loaded". */
function modelStateLabel(state: string | null): string | null {
  if (!state) return null;
  if (state.startsWith('loaded:')) {
    return t('assistant.model.loaded', { device: state.slice('loaded:'.length).toUpperCase() });
  }
  return t('assistant.model.unloaded');
}

/** A picture scaled down for the model and stored as PNG (base64). Large
 * photos are made smaller until they fit what the service accepts. */
async function pictureToPng(file: Blob): Promise<string> {
  const bitmap = await createImageBitmap(file);
  let scale = Math.min(1, Math.sqrt((1024 * 768) / (bitmap.width * bitmap.height)));
  for (let attempt = 0; attempt < 6; attempt += 1) {
    const canvas = document.createElement('canvas');
    canvas.width = Math.max(32, Math.round(bitmap.width * scale));
    canvas.height = Math.max(32, Math.round(bitmap.height * scale));
    canvas.getContext('2d')?.drawImage(bitmap, 0, 0, canvas.width, canvas.height);
    const data = canvas.toDataURL('image/png').split(',')[1] ?? '';
    if (data.length < 900_000) return data;
    scale *= 0.75;
  }
  throw { code: 'assistant_picture_too_large' };
}

type View = 'chats' | 'library';

export function AssistantScreen() {
  return (
    <LiveSessionProvider>
      <AssistantBody />
    </LiveSessionProvider>
  );
}

/** The chat list starts closed in a narrow window. */
const ACTIVITY_KEY = 'narrivy.assistant.activity';

function wideEnough(): boolean {
  return typeof window === 'undefined' || window.innerWidth >= 900;
}

function AssistantBody() {
  const assistant = useAssistant();
  const live = useLive();
  const [chatsOpen, setChatsOpen] = useState(wideEnough);
  const { current, busy, loadingModel, select, newChat, send, steer, stop, library, voice, unsupported, dictationMode } =
    assistant;
  const [view, setView] = useState<View>('chats');
  // Narrow, the side panel floats over the conversation: closed at first.
  // Shown by default (narrow, it floats over the chat, so it starts closed);
  // hiding it is remembered.
  const [activityOpen, setActivityOpenState] = useState(() => {
    if (typeof window === 'undefined') return true;
    try {
      if (window.localStorage.getItem(ACTIVITY_KEY) === 'hidden') return false;
    } catch {
      // Not remembered.
    }
    return window.innerWidth > 1020;
  });
  const setActivityOpen = (open: boolean) => {
    setActivityOpenState(open);
    try {
      window.localStorage.setItem(ACTIVITY_KEY, open ? 'shown' : 'hidden');
    } catch {
      // Kept for this session.
    }
  };
  const hasActivity = activityIn(current, assistant.streamed);
  const showActivity = activityOpen;
  const [draft, setDraft] = useState('');
  const [pictures, setPictures] = useState<{ id: string; url: string }[]>([]);
  const [attachError, setAttachError] = useState<string | null>(null);
  const scroller = useRef<HTMLDivElement | null>(null);
  const input = useRef<HTMLTextAreaElement | null>(null);
  const fileInput = useRef<HTMLInputElement | null>(null);
  const turns = current?.turns ?? [];
  // Voice is on (listening and speaking) only out of standby; in standby its
  // models stay loaded.
  const voiceLoaded = voice.state !== 'off';
  const voiceOn = voiceLoaded && !voice.standby;
  const toggleVoice = useCallback(() => {
    if (!voiceLoaded) voice.start();
    else voice.setStandby(!voice.standby);
  }, [voice, voiceLoaded]);

  useEffect(() => {
    const element = scroller.current;
    if (element) element.scrollTop = element.scrollHeight;
  }, [turns]);

  const submit = useCallback(() => {
    const text = input.current?.value ?? draft;
    if (!text.trim()) return;
    if (busy) {
      steer(text);
    } else {
      send(text, { pictures: pictures.map((p) => p.id) });
      setPictures([]);
    }
    setDraft('');
  }, [busy, draft, pictures, send, steer]);

  // "Send at once": a dictation into the chat box is sent when it ends.
  useEffect(() => {
    if (dictationMode !== 'send') return undefined;
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    void bridge
      .onSessionEvent((ev: SessionEvent) => {
        if (ev.event !== 'session_state_changed' || ev.state !== 'committed') return;
        if (document.activeElement !== input.current) return;
        window.setTimeout(() => submit(), 350);
      })
      .then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [dictationMode, submit]);

  // Keyboard shortcuts for the assistant.
  useEffect(() => {
    const typing = (target: EventTarget | null) =>
      target instanceof HTMLElement && (target.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName));
    const onDown = (event: globalThis.KeyboardEvent) => {
      if (event.ctrlKey && event.shiftKey && event.key.toLowerCase() === 'v') {
        event.preventDefault();
        toggleVoice();
      } else if (event.ctrlKey && !event.shiftKey && event.key.toLowerCase() === 'm' && voiceOn) {
        event.preventDefault();
        voice.setPrefs({ muted: voice.state !== 'muted' });
      } else if (event.altKey && event.key.toLowerCase() === 'n') {
        event.preventDefault();
        newChat();
        setView('chats');
        window.setTimeout(() => input.current?.focus(), 0);
      } else if (event.key === 'Escape' && busy) {
        stop();
      } else if (event.code === 'Space' && voiceOn && voice.prefs.pushToTalk && !event.repeat && !typing(event.target)) {
        event.preventDefault();
        voice.pushToTalk(true);
      }
    };
    const onUp = (event: globalThis.KeyboardEvent) => {
      if (event.code === 'Space' && voiceOn && voice.prefs.pushToTalk && !typing(event.target)) {
        event.preventDefault();
        voice.pushToTalk(false);
      }
    };
    window.addEventListener('keydown', onDown);
    window.addEventListener('keyup', onUp);
    return () => {
      window.removeEventListener('keydown', onDown);
      window.removeEventListener('keyup', onUp);
    };
  }, [busy, newChat, stop, voice, voiceOn, toggleVoice]);

  const attach = async (files: Iterable<Blob>) => {
    setAttachError(null);
    for (const file of files) {
      if (!file.type.startsWith('image/')) continue;
      if (pictures.length >= 4) break;
      try {
        const png = await pictureToPng(file);
        const id = await assistant.addPicture(png);
        setPictures((list) => [...list, { id, url: `data:image/png;base64,${png}` }].slice(0, 4));
      } catch (error) {
        setAttachError((error as { code?: string })?.code ?? 'assistant_picture_invalid');
      }
    }
  };

  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key === 'Enter' && !event.shiftKey) {
      event.preventDefault();
      submit();
    }
  };
  const onPaste = (event: ClipboardEvent<HTMLTextAreaElement>) => {
    const files = Array.from(event.clipboardData.files).filter((file) => file.type.startsWith('image/'));
    if (files.length) {
      event.preventDefault();
      void attach(files);
    }
  };
  const onDrop = (event: DragEvent<HTMLDivElement>) => {
    const files = Array.from(event.dataTransfer.files).filter((file) => file.type.startsWith('image/'));
    if (files.length) {
      event.preventDefault();
      void attach(files);
    }
  };

  const persona = library.persona.find((p) => p.id === current?.persona);

  return (
    <div className="page-wide assistant-page">
      <h2 className="sr-only">{t('nav.assistant')}</h2>
      {unsupported ? <p className="assistant-attached">{t('assistant.error.olderService')}</p> : null}

      <div
        className={`assistant-shell${view === 'library' ? ' is-library' : ''}${showActivity && view === 'chats' ? ' with-activity' : ''}${chatsOpen ? '' : ' chats-closed'}`}
      >
        {chatsOpen ? (
          <ChatList
            view={view}
            onClose={() => setChatsOpen(false)}
            onNew={() => {
              newChat();
              setView('chats');
              if (!wideEnough()) setChatsOpen(false);
              // Already a new chat or not, the message box is where it starts.
              window.setTimeout(() => input.current?.focus(), 0);
            }}
            onOpen={(id) => {
              select(id);
              setView('chats');
              if (!wideEnough()) setChatsOpen(false);
            }}
            onLibrary={() => setView(view === 'library' ? 'chats' : 'library')}
          />
        ) : null}

        <div className="assistant-main" onDragOver={(event) => event.preventDefault()} onDrop={onDrop}>
          <div className="assistant-topbar">
            {chatsOpen ? null : (
              <button
                type="button"
                className="composer-icon assistant-chats-toggle"
                aria-label={t('assistant.showChats')}
                title={t('assistant.showChats')}
                onClick={() => setChatsOpen(true)}
              >
                <Icon name="sidebar" />
              </button>
            )}
            {view === 'library' ? (
              <strong className="assistant-topbar-title">{t('assistant.library')}</strong>
            ) : (
              <ChatTitle />
            )}
            <div className="assistant-topbar-tools">
              {view === 'chats' ? <WatchButton /> : null}
              <ModelControl />
              {view === 'chats' ? (
                <button
                  type="button"
                  className="composer-icon assistant-activity-toggle"
                  aria-pressed={activityOpen}
                  aria-label={activityOpen ? t('assistant.activity.hide') : t('assistant.activity.show')}
                  title={activityOpen ? t('assistant.activity.hide') : t('assistant.activity.show')}
                  onClick={() => setActivityOpen(!activityOpen)}
                >
                  <Icon name="sidebar" className="flip-x" />
                </button>
              ) : null}
            </div>
          </div>

          {view === 'library' ? (
            <AssistantLibraryView
              onOpenChat={(id) => {
                select(id);
                setView('chats');
              }}
            />
          ) : (
            <>
              <WatchBar />
              {current?.meeting ? (
                <div className="assistant-attached">
                  <Icon name="people" />
                  <span>{t('assistant.aboutMeeting', { title: current.meeting.title })}</span>
                </div>
              ) : null}
              <div className="assistant-conversation" ref={scroller} aria-live="polite">
                <div className="assistant-column">
                  {turns.length === 0 ? (
                    <div className="empty-state assistant-empty">
                      <Icon name={current?.meeting ? 'people' : 'spark'} />
                      <strong>
                        {current?.meeting
                          ? t('assistant.meetingEmptyTitle')
                          : persona
                            ? t('assistant.personaEmptyTitle', { name: persona.name })
                            : t('assistant.emptyTitle')}
                      </strong>
                      <span className="screen-empty-note">
                        {current?.meeting ? t('assistant.meetingEmptyNote') : t('assistant.emptyNote')}
                      </span>
                    </div>
                  ) : (
                    turns.map((turn) => <TurnView key={turn.id} turn={turn} loadingModel={loadingModel} />)
                  )}
                </div>
              </div>

              <div className="assistant-column assistant-bottom">
                {voiceOn ? <VoicePanel /> : null}

                {pictures.length ? (
                  <div className="assistant-pictures" aria-label={t('assistant.pictures')}>
                    {pictures.map((picture) => (
                      <span key={picture.id} className="assistant-picture">
                        <img src={picture.url} alt={t('assistant.pictureAlt')} />
                        <button
                          type="button"
                          aria-label={t('assistant.removePicture')}
                          onClick={() => setPictures((list) => list.filter((p) => p.id !== picture.id))}
                        >
                          <Icon name="close" />
                        </button>
                      </span>
                    ))}
                  </div>
                ) : null}
                {attachError ? (
                  <p className="assistant-meta assistant-error">{t('assistant.error.other', { code: attachError })}</p>
                ) : null}

                <div className="assistant-composer">
                  <textarea
                    ref={input}
                    className="assistant-input"
                    aria-label={t('assistant.inputLabel')}
                    placeholder={
                      busy ? t('assistant.steerPlaceholder') : turns.length ? t('assistant.replyPlaceholder') : t('assistant.placeholder')
                    }
                    value={draft}
                    rows={Math.min(8, Math.max(1, draft.split('\n').length))}
                    onChange={(event) => setDraft(event.target.value)}
                    onKeyDown={onKeyDown}
                    onPaste={onPaste}
                  />
                  <div className="assistant-composer-row">
                    <AddMenu disabled={busy} onPicture={() => fileInput.current?.click()} />
                    <input
                      ref={fileInput}
                      type="file"
                      accept="image/*"
                      multiple
                      hidden
                      onChange={(event) => {
                        if (event.target.files) void attach(Array.from(event.target.files));
                        event.target.value = '';
                      }}
                    />
                    <ComposerToggles />
                    <span className="composer-spacer" />
                    <button
                      type="button"
                      className={`composer-icon${voiceOn ? ' is-live' : ''}`}
                      aria-pressed={voiceOn}
                      aria-label={voiceOn ? t('assistant.voice.stop') : t('assistant.voice.start')}
                      title={`${voiceOn ? t('assistant.voice.stop') : voiceLoaded ? t('assistant.voice.startLoaded') : t('assistant.voice.start')} (Ctrl+Shift+V)`}
                      data-loaded={voiceLoaded && !voiceOn ? 'true' : undefined}
                      onClick={toggleVoice}
                      disabled={!voice.prefs.available && !voiceOn}
                    >
                      <Icon name="wave" />
                    </button>
                    {busy && draft.trim() ? (
                      <button type="button" className="composer-send" onClick={submit} title={t('assistant.steerHint')}>
                        <Icon name="send" />
                        {t('assistant.steer')}
                      </button>
                    ) : busy ? (
                      <button
                        type="button"
                        className="composer-send is-stop"
                        onClick={stop}
                        aria-label={t('assistant.stop')}
                        title={t('assistant.shortcut', { keys: 'Esc' })}
                      >
                        <Icon name="stop" />
                      </button>
                    ) : (
                      <button
                        type="button"
                        className="composer-send"
                        onClick={submit}
                        disabled={!draft.trim()}
                        aria-label={t('assistant.send')}
                        title={t('assistant.send')}
                      >
                        <Icon name="send" />
                      </button>
                    )}
                  </div>
                </div>
              </div>
            </>
          )}
        </div>
        {showActivity && view === 'chats' ? (
          <aside className="assistant-activity" aria-label={t('assistant.activity.title')}>
            <button
              type="button"
              className="composer-icon assistant-activity-close"
              aria-label={t('assistant.activity.hide')}
              title={t('assistant.activity.hide')}
              onClick={() => setActivityOpen(false)}
            >
              <Icon name="close" />
            </button>
            <LivePanel />
            {current && hasActivity ? (
              <ActivityPanel chat={current} />
            ) : live.id === null && live.saved === null ? (
              <section className="activity-section activity-empty" aria-label={t('assistant.activity.title')}>
                <h3>{t('assistant.activity.title')}</h3>
                <p className="source">{t('assistant.activity.empty')}</p>
              </section>
            ) : null}
          </aside>
        ) : null}
      </div>
    </div>
  );
}

/** Closes a popover on a click outside it or on Escape. */
function useDismiss(open: boolean, close: () => void) {
  const ref = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    if (!open) return undefined;
    const onDown = (event: MouseEvent) => {
      if (ref.current && !ref.current.contains(event.target as Node)) close();
    };
    const onKey = (event: globalThis.KeyboardEvent) => {
      if (event.key === 'Escape') close();
    };
    document.addEventListener('mousedown', onDown);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey);
    };
  }, [open, close]);
  return ref;
}

type Group = 'pinned' | 'today' | 'yesterday' | 'week' | 'month' | 'older';
const GROUPS: Group[] = ['pinned', 'today', 'yesterday', 'week', 'month', 'older'];

function groupOf(chat: ChatSummary, now: Date): Group {
  if (chat.pinned) return 'pinned';
  const day = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
  const at = chat.updatedAt;
  if (at >= day) return 'today';
  if (at >= day - 86_400_000) return 'yesterday';
  if (at >= day - 7 * 86_400_000) return 'week';
  if (at >= day - 30 * 86_400_000) return 'month';
  return 'older';
}

function whenLabel(at: number, group: Group): string {
  const date = new Date(at);
  if (group === 'today' || group === 'yesterday' || group === 'pinned') {
    return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  }
  if (group === 'week') return date.toLocaleDateString([], { weekday: 'short' });
  return date.toLocaleDateString([], { day: 'numeric', month: 'short' });
}

/** The chats, newest first, in groups, with search. */
function ChatList({
  view,
  onClose,
  onNew,
  onOpen,
  onLibrary,
}: {
  view: View;
  onClose: () => void;
  onNew: () => void;
  onOpen: (id: string) => void;
  onLibrary: () => void;
}) {
  const { chats, current, busy, watch } = useAssistant();
  const [query, setQuery] = useState('');
  const now = new Date();
  const needle = query.trim().toLowerCase();
  const shown = needle ? chats.filter((chat) => (chat.title || '').toLowerCase().includes(needle)) : chats;
  const grouped = GROUPS.map((group) => ({
    group,
    list: shown.filter((chat) => groupOf(chat, now) === group).sort((a, b) => b.updatedAt - a.updatedAt),
  })).filter((g) => g.list.length);
  return (
    <aside className="assistant-chats" aria-label={t('assistant.chats')}>
      <div className="assistant-chats-head">
        <button
          type="button"
          className="assistant-new"
          onClick={onNew}
          disabled={busy}
          title={t('assistant.shortcut', { keys: 'Alt+N' })}
        >
          <Icon name="edit" />
          {t('assistant.newChat')}
        </button>
        <button
          type="button"
          className="composer-icon"
          aria-label={t('assistant.hideChats')}
          title={t('assistant.hideChats')}
          onClick={onClose}
        >
          <Icon name="sidebar" />
        </button>
      </div>
      <label className="assistant-search">
        <Icon name="search" />
        <input
          type="search"
          value={query}
          placeholder={t('assistant.searchChats')}
          aria-label={t('assistant.searchChats')}
          onChange={(event) => setQuery(event.target.value)}
        />
      </label>
      <div className="assistant-chat-groups">
        {grouped.map(({ group, list }) => (
          <section key={group} aria-label={t(`assistant.group.${group}`)}>
            <h3>{t(`assistant.group.${group}`)}</h3>
            <ul>
              {list.map((chat) => (
                <ChatRow
                  key={chat.id}
                  chat={chat}
                  group={group}
                  current={view === 'chats' && chat.id === current?.id}
                  watching={watch.chatId === chat.id && watch.phase !== 'idle'}
                  onOpen={() => onOpen(chat.id)}
                />
              ))}
            </ul>
          </section>
        ))}
        {needle && !shown.length ? <p className="assistant-chats-none">{t('assistant.noChatsFound')}</p> : null}
      </div>
      <button type="button" className="assistant-library-link" aria-pressed={view === 'library'} onClick={onLibrary}>
        <Icon name="bookmark" />
        {t('assistant.library')}
      </button>
    </aside>
  );
}

function ChatRow({
  chat,
  group,
  current,
  watching,
  onOpen,
}: {
  chat: ChatSummary;
  group: Group;
  current: boolean;
  watching: boolean;
  onOpen: () => void;
}) {
  const { remove, changeChat } = useAssistant();
  const [menu, setMenu] = useState(false);
  const [renaming, setRenaming] = useState<string | null>(null);
  const close = useCallback(() => setMenu(false), []);
  const ref = useDismiss(menu, close);
  const title = chat.title || t('assistant.untitled');
  return (
    <li data-current={current ? 'true' : 'false'}>
      {renaming !== null ? (
        <input
          className="assistant-rename"
          aria-label={t('assistant.renameChat')}
          value={renaming}
          autoFocus
          onChange={(event) => setRenaming(event.target.value)}
          onBlur={() => {
            changeChat(chat.id, { title: renaming });
            setRenaming(null);
          }}
          onKeyDown={(event) => {
            if (event.key === 'Enter') event.currentTarget.blur();
            if (event.key === 'Escape') setRenaming(null);
          }}
        />
      ) : (
        <button type="button" className="assistant-chat" onClick={onOpen} title={title}>
          <span className="assistant-chat-title">
            {chat.pinned ? <Icon name="pin" /> : null}
            {chat.routine ? <Icon name="refresh" /> : null}
            {watching ? <span className="watch-dot" aria-label={t('assistant.screen.button')} /> : null}
            {title}
          </span>
          <span className="assistant-chat-date">{whenLabel(chat.updatedAt, group)}</span>
        </button>
      )}
      <div className="assistant-chat-more" ref={ref}>
        <button
          type="button"
          className="assistant-chat-menu-button"
          aria-label={t('assistant.chatMenu', { title })}
          aria-expanded={menu}
          onClick={() => setMenu(!menu)}
        >
          <Icon name="dots" />
        </button>
        {menu ? (
          <div className="assistant-menu" role="menu">
            <button
              type="button"
              role="menuitem"
              onClick={() => {
                setMenu(false);
                changeChat(chat.id, { pinned: !chat.pinned });
              }}
            >
              <Icon name="pin" />
              {chat.pinned ? t('assistant.unpin') : t('assistant.pin')}
            </button>
            <button
              type="button"
              role="menuitem"
              onClick={() => {
                setMenu(false);
                setRenaming(chat.title);
              }}
            >
              <Icon name="pencil" />
              {t('assistant.rename')}
            </button>
            <button
              type="button"
              role="menuitem"
              className="is-danger"
              aria-label={t('assistant.deleteChat', { title: chat.title })}
              onClick={() => {
                setMenu(false);
                remove(chat.id);
              }}
            >
              <Icon name="trash" />
              {t('assistant.deleteTurn')}
            </button>
          </div>
        ) : null}
      </div>
    </li>
  );
}

/** The open chat's title, with its persona and project behind it. */
function ChatTitle() {
  const { current, library, setChatSetup, busy } = useAssistant();
  const [open, setOpen] = useState(false);
  const close = useCallback(() => setOpen(false), []);
  const ref = useDismiss(open, close);
  const persona = library.persona.find((p) => p.id === current?.persona);
  const project = library.project.find((p) => p.id === current?.project);
  const hasSetup = library.persona.length > 0 || library.project.length > 0;
  return (
    <div className="assistant-title-menu" ref={ref}>
      <button
        type="button"
        className="assistant-topbar-title"
        aria-expanded={hasSetup ? open : undefined}
        disabled={!hasSetup}
        onClick={() => setOpen(!open)}
      >
        <span>{current?.title || t('assistant.untitled')}</span>
        {persona || project ? (
          <span className="assistant-title-setup">· {[persona?.name, project?.name].filter(Boolean).join(' · ')}</span>
        ) : null}
        {hasSetup ? <Icon name="chevron" /> : null}
      </button>
      {open ? (
        <div className="assistant-menu assistant-setup-menu" role="dialog" aria-label={t('assistant.chatSetup')}>
          {library.persona.length ? (
            <label className="assistant-field">
              <span>{t('assistant.persona')}</span>
              <select
                className="pipe-select"
                value={current?.persona ?? ''}
                disabled={busy}
                onChange={(event) => setChatSetup({ persona: event.target.value || null })}
              >
                <option value="">{t('assistant.personaNone')}</option>
                {library.persona.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.name}
                  </option>
                ))}
              </select>
            </label>
          ) : null}
          {library.project.length ? (
            <label className="assistant-field">
              <span>{t('assistant.project')}</span>
              <select
                className="pipe-select"
                value={current?.project ?? ''}
                disabled={busy}
                onChange={(event) => setChatSetup({ project: event.target.value || null })}
              >
                <option value="">{t('assistant.projectNone')}</option>
                {library.project.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.name}
                  </option>
                ))}
              </select>
            </label>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

/** Where the model is, with Load or Unload and "Unload when idle". */
function ModelControl() {
  const { loadingModel, modelState, modelName, busy, load, unload, autoUnload, setAutoUnload } = useAssistant();
  const [open, setOpen] = useState(false);
  const close = useCallback(() => setOpen(false), []);
  const ref = useDismiss(open, close);
  const loaded = modelState?.startsWith('loaded:') ?? false;
  const label = loadingModel ? t('assistant.loadingModel') : modelStateLabel(modelState);
  if (!label) return null;
  return (
    <div className="assistant-model-control" ref={ref}>
      <button
        type="button"
        className="assistant-model-chip"
        data-state={loadingModel ? 'loading' : loaded ? 'loaded' : 'unloaded'}
        aria-expanded={open}
        onClick={() => setOpen(!open)}
      >
        <span className="assistant-model-dot" aria-hidden="true" />
        <span data-testid="assistant-model-state">{label}</span>
        <Icon name="chevron" />
      </button>
      {open ? (
        <div className="assistant-menu assistant-model-menu" role="dialog" aria-label={t('assistant.modelMenu')}>
          {modelName ? <strong className="assistant-model-name">{modelName}</strong> : null}
          <p className="assistant-model-where">{label}</p>
          {loaded ? (
            <button type="button" className="btn btn-default" onClick={unload} disabled={busy} title={t('assistant.unloadHint')}>
              {t('assistant.unload')}
            </button>
          ) : modelState ? (
            <button type="button" className="btn btn-primary" onClick={load} disabled={loadingModel} title={t('assistant.loadHint')}>
              {t('assistant.load')}
            </button>
          ) : null}
          <div className="assistant-switch-row">
            <span>
              <span className="assistant-switch-label" id="assistant-auto-unload">
                {t('assistant.autoUnload')}
              </span>
              <span className="assistant-switch-hint">{t('assistant.autoUnloadHint')}</span>
            </span>
            <button
              type="button"
              role="switch"
              className="switch"
              aria-checked={autoUnload}
              aria-labelledby="assistant-auto-unload"
              onClick={() => setAutoUnload(!autoUnload)}
            >
              <span className="switch-thumb" />
            </button>
          </div>
        </div>
      ) : null}
    </div>
  );
}

/** The composer's "+": a picture, or a recorded session (a screen
 * recording or a meeting) for the chat to know. */
function AddMenu({ disabled, onPicture }: { disabled: boolean; onPicture: () => void }) {
  const { attachRecording } = useAssistant();
  const [open, setOpen] = useState(false);
  const [sessions, setSessions] = useState<Meeting[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const close = useCallback(() => setOpen(false), []);
  const ref = useDismiss(open, close);
  useEffect(() => {
    if (!open) return;
    setError(null);
    bridge
      .meetings()
      .then((list) => setSessions(list.filter((m) => m.state === 'ready').slice(0, 12)))
      .catch(() => setSessions([]));
  }, [open]);
  return (
    <div className="assistant-abilities" ref={ref}>
      <button
        type="button"
        className="composer-icon"
        aria-label={t('assistant.attach')}
        title={t('assistant.attachHint')}
        aria-expanded={open}
        onClick={() => setOpen(!open)}
        disabled={disabled}
      >
        <Icon name="plus" />
      </button>
      {open ? (
        <div className="assistant-menu assistant-abilities-menu" role="menu">
          <button
            type="button"
            role="menuitem"
            onClick={() => {
              setOpen(false);
              onPicture();
            }}
          >
            <Icon name="paste" />
            {t('assistant.addPicture')}
          </button>
          <p className="assistant-menu-heading">{t('assistant.addRecording')}</p>
          {sessions === null ? <p className="assistant-menu-note">{t('assistant.loadingRecordings')}</p> : null}
          {sessions && !sessions.length ? <p className="assistant-menu-note">{t('assistant.noRecordings')}</p> : null}
          {sessions?.map((session) => (
            <button
              key={session.id}
              type="button"
              role="menuitem"
              onClick={() => {
                setOpen(false);
                void attachRecording(session.id).catch((e: unknown) => setError((e as { code?: string })?.code ?? 'unavailable'));
              }}
            >
              <Icon name={session.watch ? 'play' : 'people'} />
              <span className="assistant-menu-item-text">
                <span>{session.title}</span>
                <span className="assistant-switch-hint">
                  {new Date(session.started_unix_ms).toLocaleString([], { dateStyle: 'medium', timeStyle: 'short' })}
                </span>
              </span>
            </button>
          ))}
          {error ? <p className="assistant-menu-note assistant-error">{t('assistant.error.other', { code: error })}</p> : null}
        </div>
      ) : null}
    </div>
  );
}

/** What the chat may use (the web, the computer, tool servers), in the
 * composer like a chip. */
/** The composer's switches, one button each: web research, the computer
 * (with how much it may do without asking, and MCP tools), and the live
 * transcript and screen. With nothing running, Transcript or Screen starts
 * a live share of just that; while something runs they switch its sources. */
function ComposerToggles() {
  const { current, library, setChatSetup, busy, watch, startScreen, defaultFolder } = useAssistant();
  const live = useLive();
  const [open, setOpen] = useState(false);
  const close = useCallback(() => setOpen(false), []);
  const ref = useDismiss(open, close);
  const tools = current?.tools ?? [];
  const toggle = (id: string, on: boolean) =>
    setChatSetup({ tools: on ? [...tools, id] : tools.filter((tool) => tool !== id) });
  const computer = tools.includes('computer');
  const extra = library.tool.filter((server) => tools.includes(server.id)).length;
  const starting = watch.phase === 'starting' || watch.phase === 'processing';
  const source = (which: LiveSource) => {
    if (live.id) live.flip(which);
    else
      startScreen('share', {
        screen: which === 'screen',
        microphone: which === 'microphone',
        computerSound: which === 'sound',
      });
  };
  return (
    <div className="composer-toggles" ref={ref}>
      <button
        type="button"
        role="switch"
        aria-checked={tools.includes('web')}
        aria-label={t('assistant.webShort')}
        className="composer-toggle"
        disabled={busy}
        title={t('assistant.webHint')}
        onClick={() => toggle('web', !tools.includes('web'))}
      >
        <Icon name="globe" />
        <span>{t('assistant.webShort')}</span>
      </button>
      <span className="composer-split">
        <button
          type="button"
          role="switch"
          aria-checked={computer}
          aria-label={t('assistant.computer')}
          className="composer-toggle"
          disabled={busy}
          title={t('assistant.computerHint')}
          onClick={() => toggle('computer', !computer)}
        >
          <Icon name="cpu" />
          <span>
            {t('assistant.computer')}
            {computer && (current?.folder ?? defaultFolder) ? ` · ${folderName(current?.folder ?? defaultFolder ?? '')}` : ''}
            {computer && current?.fullAccess ? ` · ${t('assistant.access.fullShort')}` : ''}
            {extra ? ` +${extra}` : ''}
          </span>
        </button>
        <button
          type="button"
          className="composer-toggle composer-more"
          aria-expanded={open}
          aria-label={t('assistant.abilities')}
          title={t('assistant.abilitiesHint')}
          onClick={() => setOpen(!open)}
        >
          <Icon name="chevron" />
        </button>
      </span>
      {(
        [
          ['microphone', 'mic', live.micOn],
          ['sound', 'speaker', live.soundOn],
          ['screen', 'play', live.screenOn],
        ] as const
      ).map(([which, icon, on]) => (
        <button
          key={which}
          type="button"
          role="switch"
          aria-checked={live.id !== null && on}
          aria-label={t(`assistant.live.button.${which}`)}
          className="composer-toggle composer-live"
          disabled={live.busy || starting}
          title={live.id ? t(`assistant.live.button.${which}Hint`) : t(`assistant.live.button.${which}Start`)}
          onClick={() => source(which)}
        >
          <Icon name={icon} />
          <span>{t(`assistant.live.button.${which}`)}</span>
        </button>
      ))}
      <ThinkingPicker />
      {open ? (
        <div className="assistant-menu assistant-abilities-menu" role="dialog" aria-label={t('assistant.abilities')}>
          <p className="assistant-menu-heading">{t('assistant.access.label')}</p>
          <div className="assistant-access" role="radiogroup" aria-label={t('assistant.access.label')}>
            {([false, true] as const).map((full) => (
              <button
                key={String(full)}
                type="button"
                role="radio"
                aria-checked={Boolean(current?.fullAccess) === full}
                disabled={busy || !computer}
                className="screen-mode"
                onClick={() => setChatSetup({ fullAccess: full })}
              >
                <strong>{full ? t('assistant.access.full') : t('assistant.access.ask')}</strong>
                <span>{full ? t('assistant.access.fullHint') : t('assistant.access.askHint')}</span>
              </button>
            ))}
          </div>
          <WorkingFolder disabled={busy} />
          {library.tool.length ? <p className="assistant-menu-heading">{t('assistant.tools')}</p> : null}
          {library.tool.map((server) => (
            <AbilityRow
              key={server.id}
              icon="apps"
              label={server.name}
              hint={t('assistant.toolsHint')}
              on={tools.includes(server.id)}
              disabled={busy}
              onChange={(on) => toggle(server.id, on)}
            />
          ))}
        </div>
      ) : null}
    </div>
  );
}

const THINKING: Array<ThinkingLevel | null> = [null, 'short', 'deep'];

/** How much the model thinks before answering, for this chat: not at all
 * (the default, quickest), briefly, or carefully. */
function ThinkingPicker() {
  const { current, setChatSetup, busy } = useAssistant();
  const [open, setOpen] = useState(false);
  const close = useCallback(() => setOpen(false), []);
  const ref = useDismiss(open, close);
  const level = current?.thinking ?? null;
  const key = (choice: ThinkingLevel | null) => choice ?? 'off';
  return (
    <span className="composer-thinking" ref={ref}>
      <button
        type="button"
        className="composer-toggle"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-label={t('assistant.thinking.label')}
        data-on={level !== null}
        disabled={busy}
        title={t('assistant.thinking.hint')}
        onClick={() => setOpen(!open)}
      >
        <Icon name="spark" />
        <span>{level ? t(`assistant.thinking.${level}`) : t('assistant.thinking.label')}</span>
        <Icon name="chevron" />
      </button>
      {open ? (
        <div className="assistant-menu thinking-menu" role="listbox" aria-label={t('assistant.thinking.label')}>
          {THINKING.map((choice) => (
            <button
              key={key(choice)}
              type="button"
              role="option"
              aria-selected={level === choice}
              onClick={() => {
                setChatSetup({ thinking: choice });
                setOpen(false);
              }}
            >
              <span className="assistant-menu-item-text">
                <span>{t(`assistant.thinking.${key(choice)}`)}</span>
                <span className="source">{t(`assistant.thinking.${key(choice)}Hint`)}</span>
              </span>
              {level === choice ? <Icon name="check" /> : null}
            </button>
          ))}
        </div>
      ) : null}
    </span>
  );
}

function folderName(path: string): string {
  return path.replace(/\/+$/, '').split('/').pop() || path;
}

/** Where the computer tools work: this chat's folder, or the default one
 * for chats without their own (the home folder when none is set). */
function WorkingFolder({ disabled }: { disabled: boolean }) {
  const { current, setChatSetup, defaultFolder, setDefaultFolder } = useAssistant();
  const [error, setError] = useState<string | null>(null);
  const own = current?.folder ?? null;
  const used = own ?? defaultFolder;
  const choose = (then: (folder: string) => void) => {
    setError(null);
    bridge
      .pickFolder(t('assistant.folder.pickTitle'))
      .then((folder) => {
        if (folder) then(folder);
      })
      .catch((e: unknown) => setError((e as { code?: string })?.code ?? 'unavailable'));
  };
  return (
    <div className="working-folder">
      <p className="assistant-menu-heading">{t('assistant.folder.title')}</p>
      <div className="working-folder-row">
        <Icon name="folder" />
        <span className="working-folder-path" title={used ?? undefined}>
          {used ?? t('assistant.folder.home')}
          {own ? null : <span className="working-folder-note">{defaultFolder ? t('assistant.folder.default') : ''}</span>}
        </span>
        <button
          type="button"
          className="btn btn-default"
          disabled={disabled}
          onClick={() => choose((folder) => setChatSetup({ folder }))}
        >
          {t('assistant.folder.choose')}
        </button>
      </div>
      <div className="working-folder-actions">
        {own && own !== defaultFolder ? (
          <button type="button" className="btn btn-ghost" disabled={disabled} onClick={() => setDefaultFolder(own)}>
            {t('assistant.folder.makeDefault')}
          </button>
        ) : null}
        {own ? (
          <button type="button" className="btn btn-ghost" disabled={disabled} onClick={() => setChatSetup({ folder: null })}>
            {t('assistant.folder.useDefault')}
          </button>
        ) : null}
        {!own && !defaultFolder ? (
          <button type="button" className="btn btn-ghost" disabled={disabled} onClick={() => choose(setDefaultFolder)}>
            {t('assistant.folder.chooseDefault')}
          </button>
        ) : null}
        {!own && defaultFolder ? (
          <button type="button" className="btn btn-ghost" disabled={disabled} onClick={() => setDefaultFolder(null)}>
            {t('assistant.folder.clearDefault')}
          </button>
        ) : null}
      </div>
      <p className="voice-hint">{t('assistant.folder.hint')}</p>
      {error ? <p className="assistant-error">{t('assistant.error.other', { code: error })}</p> : null}
    </div>
  );
}

function AbilityRow({
  icon,
  label,
  hint,
  on,
  disabled,
  onChange,
}: {
  icon: 'globe' | 'cpu' | 'apps';
  label: string;
  hint: string;
  on: boolean;
  disabled: boolean;
  onChange: (on: boolean) => void;
}) {
  return (
    <div className="assistant-switch-row">
      <Icon name={icon} />
      <span>
        <span className="assistant-switch-label">{label}</span>
        <span className="assistant-switch-hint">{hint}</span>
      </span>
      <button
        type="button"
        role="switch"
        className="switch"
        aria-checked={on}
        aria-label={label}
        disabled={disabled}
        onClick={() => onChange(!on)}
      >
        <span className="switch-thumb" />
      </button>
    </div>
  );
}

function PictureView({ id }: { id: string }) {
  const { picture } = useAssistant();
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    void picture(id).then((found) => alive && setUrl(found));
    return () => {
      alive = false;
    };
  }, [id, picture]);
  return url ? <img className="assistant-turn-picture" src={url} alt={t('assistant.pictureAlt')} /> : null;
}

/** A link the answer cites, numbered in order of first use. */
interface Source {
  url: string;
  host: string;
}

const LINK = /\[([^\]\n]+)\]\((https?:\/\/[^)\s]+)\)/g;

function hostOf(url: string): string {
  try {
    return new URL(url).hostname.replace(/^www\./, '');
  } catch {
    return url;
  }
}

/** The answer's sources: its Markdown links, and the pages its tools opened.
 * A closing list of nothing but links ("Sources:") becomes the chips. */
function splitSources(text: string, tools: ToolUse[] | undefined): { body: string; sources: Source[] } {
  const sources: Source[] = [];
  const add = (url: string) => {
    if (!sources.some((s) => s.url === url)) sources.push({ url, host: hostOf(url) });
  };
  const lines = text.split('\n');
  let end = lines.length;
  while (end > 0 && !lines[end - 1].trim()) end -= 1;
  let cut = end;
  while (cut > 0) {
    const line = lines[cut - 1].trim();
    const onlyLink = /^([-*•]|\d+[.)])?\s*\[[^\]]+\]\(https?:\/\/[^)\s]+\)\s*$/.test(line);
    if (onlyLink) cut -= 1;
    else break;
  }
  if (cut < end && cut > 0 && /^(\*\*)?#*\s*(sources?|references?)\s*:?(\*\*)?\s*:?$/i.test(lines[cut - 1].trim())) cut -= 1;
  for (const match of text.matchAll(LINK)) add(match[2]);
  for (const use of tools ?? []) {
    if (use.tool !== 'open_page' || use.state !== 'done') continue;
    try {
      const url = (JSON.parse(use.arguments) as { url?: string }).url;
      if (url && /^https?:\/\//.test(url)) add(url);
    } catch {
      // Not a page.
    }
  }
  const body = cut < end ? lines.slice(0, cut).join('\n').trimEnd() : text;
  return { body, sources };
}

function SourceChips({ sources }: { sources: Source[] }) {
  const [copied, setCopied] = useState<string | null>(null);
  if (!sources.length) return null;
  return (
    <ol className="assistant-sources" aria-label={t('assistant.sources')}>
      {sources.map((source, i) => (
        <li key={source.url}>
          <button
            type="button"
            title={`${source.url}\n${t('assistant.copyLink')}`}
            onClick={() => {
              void navigator.clipboard?.writeText(source.url);
              setCopied(source.url);
              window.setTimeout(() => setCopied(null), 1500);
            }}
          >
            <span className="assistant-cite">{i + 1}</span>
            {copied === source.url ? t('assistant.copied') : source.host}
          </button>
        </li>
      ))}
    </ol>
  );
}

function TurnView({ turn, loadingModel }: { turn: Turn; loadingModel: boolean }) {
  const { editTurn, deleteTurn, busy, saveItem, modelName } = useAssistant();
  const [copied, setCopied] = useState(false);
  const [editing, setEditing] = useState<string | null>(null);
  const [remembered, setRemembered] = useState(false);
  if (turn.role === 'user') {
    return (
      <div className="assistant-turn assistant-user">
        <div className="assistant-turn-actions">
          <button
            type="button"
            aria-label={t('assistant.editTurn')}
            title={t('assistant.editTurn')}
            disabled={busy}
            onClick={() => setEditing(turn.text)}
          >
            <Icon name="pencil" />
          </button>
          <button
            type="button"
            aria-label={t('assistant.deleteTurn')}
            title={t('assistant.deleteTurn')}
            disabled={busy}
            onClick={() => deleteTurn(turn.id)}
          >
            <Icon name="trash" />
          </button>
        </div>
        <div className="assistant-bubble">
          {turn.pictures?.map((id) => <PictureView key={id} id={id} />)}
          {editing !== null ? (
            <div className="assistant-edit">
              <textarea
                aria-label={t('assistant.editTurn')}
                value={editing}
                rows={Math.min(8, Math.max(2, editing.split('\n').length))}
                onChange={(event) => setEditing(event.target.value)}
              />
              <div>
                <button type="button" className="btn btn-default" onClick={() => setEditing(null)}>
                  {t('assistant.cancel')}
                </button>
                <button
                  type="button"
                  className="btn btn-primary"
                  disabled={!editing.trim()}
                  onClick={() => {
                    editTurn(turn.id, editing);
                    setEditing(null);
                  }}
                >
                  {t('assistant.askAgain')}
                </button>
              </div>
            </div>
          ) : (
            <>
              {turn.spoken ? <Icon name="mic" /> : null}
              {turn.text}
            </>
          )}
        </div>
      </div>
    );
  }
  // Tool calls and thinking written while the reply streams are not shown.
  const visible = turn.text
    .replace(/<tool_call>[\s\S]*?(<\/tool_call>|$)/g, '')
    .replace(/<think>[\s\S]*?(<\/think>|$)/g, '')
    .trim();
  const waiting = turn.state === 'streaming' && visible.length === 0;
  const { body, sources } = splitSources(visible, turn.tools);
  return (
    <div className="assistant-turn assistant-reply" data-state={turn.state}>
      <div className="assistant-bubble">
        {turn.tools?.map((use, i) => <ToolCard key={use.callId ?? i} use={use} />)}
        {waiting ? (
          <span className="assistant-thinking">
            {loadingModel ? t('assistant.loadingModel') : t('assistant.thinking')}
          </span>
        ) : (
          <Markdown text={body} sources={sources} />
        )}
        {turn.state !== 'streaming' ? <SourceChips sources={sources} /> : null}
        {turn.state === 'cancelled' ? <p className="assistant-meta">{t('assistant.stopped')}</p> : null}
        {turn.state === 'failed' ? (
          <p className="assistant-meta assistant-error">{failureText(turn.failure)}</p>
        ) : null}
        {turn.state === 'done' && turn.text ? (
          <div className="assistant-reply-actions">
            <button
              type="button"
              aria-label={copied ? t('assistant.copied') : t('assistant.copy')}
              title={copied ? t('assistant.copied') : t('assistant.copy')}
              onClick={() => {
                void navigator.clipboard?.writeText(visible);
                setCopied(true);
                window.setTimeout(() => setCopied(false), 1500);
              }}
            >
              <Icon name={copied ? 'check' : 'copy'} />
            </button>
            <button
              type="button"
              aria-label={remembered ? t('assistant.remembered') : t('assistant.remember')}
              title={remembered ? t('assistant.remembered') : t('assistant.rememberHint')}
              disabled={remembered}
              onClick={() =>
                void saveItem('memory', { text: turn.text.slice(0, 500) }).then(() => setRemembered(true))
              }
            >
              <Icon name={remembered ? 'check' : 'bookmark'} />
            </button>
            <button
              type="button"
              aria-label={t('assistant.deleteTurn')}
              title={t('assistant.deleteTurn')}
              disabled={busy}
              onClick={() => deleteTurn(turn.id)}
            >
              <Icon name="trash" />
            </button>
            <span className="assistant-reply-meta">
              {[modelName, turn.spoken ? t('assistant.spokenReply') : null, turn.tools?.length ? t('assistant.usedTools', { n: turn.tools.length }) : null]
                .filter(Boolean)
                .join(' · ')}
            </span>
          </div>
        ) : null}
      </div>
    </div>
  );
}

/** A tool the reply uses: what it calls with what, and, while it waits,
 * the user's choice. */
function ToolCard({ use }: { use: ToolUse }) {
  const { decideTool } = useAssistant();
  const offerAlways = (() => {
    try {
      return (JSON.parse(use.detail ?? '{}') as { always?: boolean }).always !== false;
    } catch {
      return true;
    }
  })();
  const [open, setOpen] = useState(use.state === 'ask');
  let args = use.arguments;
  try {
    args = JSON.stringify(JSON.parse(use.arguments), null, 2);
  } catch {
    // Shown as it came.
  }
  return (
    <div className="assistant-tool" data-state={use.state}>
      <button type="button" className="assistant-tool-head" aria-expanded={open} onClick={() => setOpen(!open)}>
        <Icon name="apps" />
        <span>
          <strong>{use.tool}</strong> <span className="source">· {use.server}</span>
        </span>
        <span className="chip">{t(`assistant.tool.state.${use.state}`)}</span>
      </button>
      {use.summary && use.state !== 'ask' ? <p className="assistant-tool-summary">{use.summary}</p> : null}
      {open ? <pre className="assistant-code assistant-tool-args">{args}</pre> : null}
      {use.state === 'ask' && use.callId ? (
        <div className="assistant-tool-ask">
          <span>{t('assistant.tool.ask')}</span>
          <button type="button" className="btn btn-primary" onClick={() => decideTool(use.callId ?? '', true, false)}>
            {t('assistant.tool.allowOnce')}
          </button>
          {offerAlways ? (
            <button type="button" className="btn btn-default" onClick={() => decideTool(use.callId ?? '', true, true)}>
              {t('assistant.tool.allowChat')}
            </button>
          ) : null}
          <button type="button" className="btn btn-default" onClick={() => decideTool(use.callId ?? '', false, false)}>
            {t('assistant.tool.refuse')}
          </button>
        </div>
      ) : null}
    </div>
  );
}

function failureText(code: string | undefined): string {
  switch (code) {
    case 'assistant_not_configured':
    case 'assistant_model_missing':
      return t('assistant.error.notConfigured');
    case 'incompatible_protocol':
      return t('assistant.error.olderService');
    case 'assistant_timeout':
      return t('assistant.error.timeout');
    case 'assistant_failed':
      return t('assistant.error.failed');
    default:
      return t('assistant.error.other', { code: code ?? 'unknown' });
  }
}

/** A chart the model wrote as a `chart` block: bars or a line, drawn here. */
function Chart({ source }: { source: string }) {
  type ChartSpec = { type?: string; title?: string; labels?: unknown[]; values?: unknown[] };
  const parse = (): ChartSpec | null => {
    try {
      const value = JSON.parse(source) as unknown;
      return value && typeof value === 'object' ? (value as ChartSpec) : null;
    } catch {
      return null;
    }
  };
  const spec = parse();
  const labels = (spec?.labels ?? []).map(String).slice(0, 24);
  const values = (spec?.values ?? []).map(Number).slice(0, labels.length);
  if (!spec || !labels.length || values.some((v) => !Number.isFinite(v))) {
    return (
      <pre className="assistant-code">
        <code>{source}</code>
      </pre>
    );
  }
  const width = 480;
  const height = 220;
  const pad = { left: 44, right: 12, top: 26, bottom: 40 };
  const max = Math.max(...values, 0);
  const min = Math.min(...values, 0);
  const span = max - min || 1;
  const plotW = width - pad.left - pad.right;
  const plotH = height - pad.top - pad.bottom;
  const x = (i: number) => pad.left + (plotW / labels.length) * (i + 0.5);
  const y = (v: number) => pad.top + plotH - ((v - min) / span) * plotH;
  const barW = Math.max(4, (plotW / labels.length) * 0.6);
  return (
    <figure className="assistant-chart">
      <svg viewBox={`0 0 ${width} ${height}`} role="img" aria-label={spec.title ?? t('assistant.chart')}>
        {spec.title ? (
          <text x={width / 2} y={16} textAnchor="middle" className="assistant-chart-title">
            {spec.title}
          </text>
        ) : null}
        <line x1={pad.left} x2={width - pad.right} y1={y(0)} y2={y(0)} className="assistant-chart-axis" />
        <text x={pad.left - 6} y={y(max) + 4} textAnchor="end" className="assistant-chart-label">
          {max.toLocaleString()}
        </text>
        {min < 0 ? (
          <text x={pad.left - 6} y={y(min) + 4} textAnchor="end" className="assistant-chart-label">
            {min.toLocaleString()}
          </text>
        ) : null}
        {spec.type === 'line' ? (
          <polyline
            className="assistant-chart-line"
            points={values.map((v, i) => `${x(i)},${y(v)}`).join(' ')}
          />
        ) : (
          values.map((v, i) => (
            <rect
              key={i}
              className="assistant-chart-bar"
              x={x(i) - barW / 2}
              y={Math.min(y(v), y(0))}
              width={barW}
              height={Math.abs(y(v) - y(0))}
            >
              <title>{`${labels[i]}: ${v.toLocaleString()}`}</title>
            </rect>
          ))
        )}
        {labels.map((label, i) => (
          <text key={label + i} x={x(i)} y={height - pad.bottom + 16} textAnchor="middle" className="assistant-chart-label">
            {label.length > 12 ? `${label.slice(0, 11)}…` : label}
          </text>
        ))}
      </svg>
    </figure>
  );
}

/** A small, safe Markdown subset: paragraphs, headings, bullet and numbered
 * lists, tables, fenced code (and `chart` blocks), inline code and bold.
 * Never renders HTML. */
export function Markdown({ text, sources = [] }: { text: string; sources?: Source[] }) {
  const inline = (line: string) => inlineOf(line, sources);
  const blocks: ReactNode[] = [];
  const parts = text.split(/```/);
  parts.forEach((part, index) => {
    if (index % 2 === 1) {
      const language = /^([a-zA-Z0-9_-]*)\n/.exec(part)?.[1] ?? '';
      const code = part.replace(/^[a-zA-Z0-9_-]*\n/, '').replace(/\n$/, '');
      if (language === 'chart' && index < parts.length - 1) {
        blocks.push(<Chart key={`chart-${index}`} source={code} />);
        return;
      }
      blocks.push(
        <pre key={`code-${index}`} className="assistant-code">
          <code>{code}</code>
        </pre>,
      );
      return;
    }
    part.split(/\n{2,}/).forEach((chunk, chunkIndex) => {
      const lines = chunk.split('\n').filter((line) => line.trim().length > 0);
      // A block can mix a heading, a list, a table and plain lines (models
      // often put a list right under its heading): render each run in turn.
      let run: { kind: 'ul' | 'ol' | 'p' | 'table'; lines: string[] } | null = null;
      const runs: Array<{ kind: 'ul' | 'ol' | 'p' | 'h' | 'table'; lines: string[] }> = [];
      for (const line of lines) {
        const kind = /^#+\s+/.test(line)
          ? 'h'
          : /^\s*\|.*\|\s*$/.test(line)
            ? 'table'
            : /^\s*[-*•]\s+/.test(line)
              ? 'ul'
              : /^\s*\d+[.)]\s+/.test(line)
                ? 'ol'
                : 'p';
        if (kind === 'h') {
          run = null;
          runs.push({ kind, lines: [line] });
        } else if (run && run.kind === kind) {
          run.lines.push(line);
        } else {
          run = { kind, lines: [line] };
          runs.push(run);
        }
      }
      runs.forEach((block, runIndex) => {
        const key = `b-${index}-${chunkIndex}-${runIndex}`;
        if (block.kind === 'h') {
          blocks.push(
            <p key={key} className="assistant-heading">
              <strong>{inline(block.lines[0].replace(/^#+\s+/, ''))}</strong>
            </p>,
          );
        } else if (block.kind === 'table') {
          const rows = block.lines
            .filter((line) => !/^\s*\|[\s:|-]+\|\s*$/.test(line))
            .map((line) =>
              line
                .trim()
                .replace(/^\|/, '')
                .replace(/\|$/, '')
                .split('|')
                .map((cell) => cell.trim()),
            );
          const [head, ...body] = rows;
          blocks.push(
            <div key={key} className="assistant-table">
              <table>
                <thead>
                  <tr>
                    {head?.map((cell, i) => (
                      <th key={i}>{inline(cell)}</th>
                    ))}
                  </tr>
                </thead>
                <tbody>
                  {body.map((row, r) => (
                    <tr key={r}>
                      {row.map((cell, i) => (
                        <td key={i}>{inline(cell)}</td>
                      ))}
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>,
          );
        } else if (block.kind === 'ul') {
          blocks.push(
            <ul key={key}>
              {block.lines.map((line, i) => (
                <li key={i}>
                  {inline(line.replace(/^\s*[-*•]\s+/, '').replace(/^\[ \]\s*/, '☐ ').replace(/^\[x\]\s*/i, '☑ '))}
                </li>
              ))}
            </ul>,
          );
        } else if (block.kind === 'ol') {
          blocks.push(
            <ol key={key}>
              {block.lines.map((line, i) => (
                <li key={i}>{inline(line.replace(/^\s*\d+[.)]\s+/, ''))}</li>
              ))}
            </ol>,
          );
        } else {
          blocks.push(
            <p key={key}>
              {block.lines.map((line, i) => (
                <span key={i}>
                  {i > 0 ? <br /> : null}
                  {inline(line)}
                </span>
              ))}
            </p>,
          );
        }
      });
    });
  });
  return <div className="assistant-markdown">{blocks}</div>;
}

function inlineOf(line: string, sources: Source[]): ReactNode[] {
  return line.split(/(`[^`]+`|\*\*[^*]+\*\*|\[[^\]\n]+\]\(https?:\/\/[^)\s]+\))/).map((piece, i) => {
    const link = /^\[([^\]\n]+)\]\((https?:\/\/[^)\s]+)\)$/.exec(piece);
    if (link) {
      const n = sources.findIndex((s) => s.url === link[2]) + 1;
      return (
        <span key={i} className="assistant-link" title={link[2]}>
          {link[1]}
          {n ? <span className="assistant-cite">{n}</span> : null}
        </span>
      );
    }
    if (piece.startsWith('`') && piece.endsWith('`') && piece.length > 1) {
      return <code key={i}>{piece.slice(1, -1)}</code>;
    }
    if (piece.startsWith('**') && piece.endsWith('**') && piece.length > 4) {
      return <strong key={i}>{piece.slice(2, -2)}</strong>;
    }
    return <span key={i}>{piece}</span>;
  });
}
