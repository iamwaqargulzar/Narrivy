// What is being shared or recorded right now, for the assistant screen: a
// screen share from this screen, or a meeting recording started elsewhere.
// The composer's Mic, Sound and Screen buttons switch its sources, and the
// side panel shows small pictures of the screens and the live transcript,
// with people named by voice. When it stops, the panel keeps showing it as
// a saved session (raw, as heard: gone over when given to a chat).
import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { bridge, type Meeting, type MeetingTurn, type SourceChange } from '../../../lib/bridge';
import { t } from '../../../i18n';
import { Icon } from '../../../ui/icons';
import { useAssistant } from '../../../state/AssistantContext';
import { clock } from '../../../lib/meetingContext';

interface Source {
  source: number;
  kind: string;
  width: number;
  height: number;
}

export type LiveSource = 'screen' | 'sound' | 'microphone';

/** A stopped session, while it is gone over and once it is saved. */
export interface SavedSession {
  id: string;
  title: string;
  /** `processing`, `ready` or `failed:<code>`. */
  state: string;
  progress: number;
  turns: MeetingTurn[];
  names: Record<string, string>;
  /** What was on screen: the newest few, as pictures. */
  slides: string[];
  /** Kept as heard, not gone over yet. */
  raw: boolean;
}

export interface LiveSession {
  /** The meeting being recorded, or null when nothing is. */
  id: string | null;
  title: string;
  screenOn: boolean;
  /** The computer's sound. */
  soundOn: boolean;
  micOn: boolean;
  sources: Source[];
  pictures: Record<number, string>;
  screenError: string | null;
  turns: MeetingTurn[];
  /** Speaker label to name ("You" for the user's own voice). */
  names: Record<string, string>;
  /** A switch is being turned. */
  busy: boolean;
  error: string | null;
  /** Switch the screen, the computer's sound or the microphone. */
  flip: (which: LiveSource) => void;
  /** The session that just stopped, kept in the panel. */
  saved: SavedSession | null;
  dismissSaved: () => void;
  /** Go over the saved session (transcribe again, describe the screens). */
  goOver: () => Promise<void>;
  /** Show an earlier saved session in the panel. */
  showSaved: (id: string) => void;
}

type LiveState = Omit<LiveSession, 'flip' | 'busy' | 'error' | 'saved' | 'dismissSaved' | 'goOver' | 'showSaved'>;

const NOTHING: LiveState = {
  id: null,
  title: '',
  screenOn: false,
  soundOn: false,
  micOn: false,
  sources: [],
  pictures: {},
  screenError: null,
  turns: [],
  names: {},
};

const LiveContext = createContext<LiveSession>({
  ...NOTHING,
  busy: false,
  error: null,
  flip: () => {},
  saved: null,
  dismissSaved: () => {},
  goOver: () => Promise.resolve(),
  showSaved: () => {},
});

export function useLive(): LiveSession {
  return useContext(LiveContext);
}

const KEY: Record<LiveSource, keyof SourceChange> = { screen: 'screen', sound: 'sound', microphone: 'microphone' };

export function LiveSessionProvider({ children }: { children: ReactNode }) {
  const { watch, stopScreen } = useAssistant();
  const [recording, setRecording] = useState<string | null>(null);
  const [state, setState] = useState<LiveState>(NOTHING);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [savedId, setSavedId] = useState<string | null>(null);
  // Bumped to follow the saved session again (it is being gone over).
  const [savedLook, setSavedLook] = useState(0);
  const [saved, setSaved] = useState<SavedSession | null>(null);
  const own = watch.phase === 'watching' && watch.id ? watch.id : null;
  const id = own ?? recording;

  // A meeting recorded from the Meetings screen counts too: the assistant
  // hears it as well.
  useEffect(() => {
    if (own) return undefined;
    let alive = true;
    const look = () =>
      Promise.resolve()
        .then(() => bridge.meetings())
        .then((list) => {
          if (alive) setRecording(list.find((m) => m.state === 'recording')?.id ?? null);
        })
        .catch(() => {});
    void look();
    const timer = window.setInterval(() => void look(), 5000);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, [own]);

  const refresh = useCallback(async (meetingId: string, alive: () => boolean) => {
    try {
      const detail = await bridge.meeting(meetingId);
      if (!alive()) return;
      if (detail.meeting.state !== 'recording') {
        // Stopped: it stays in the panel as a saved session.
        setRecording(null);
        setState(NOTHING);
        setSavedId(meetingId);
        return;
      }
      const sources = detail.screen_sources ?? [];
      const screenOn = Boolean(detail.meeting.screen_on);
      const pictures: Record<number, string> = {};
      if (screenOn) {
        for (const source of sources) {
          try {
            const png = await bridge.meetingSlide(meetingId, `live-${source.source}.png`);
            if (png) pictures[source.source] = `data:image/png;base64,${png}`;
          } catch {
            // Not taken yet.
          }
        }
      }
      if (!alive()) return;
      setSavedId((old) => (old === meetingId ? null : old));
      setState((now) => ({
        id: meetingId,
        title: detail.meeting.title,
        screenOn,
        soundOn: Boolean(detail.meeting.sound_on),
        micOn: Boolean(detail.meeting.mic_on),
        sources,
        // A picture that could not be read this time stays as it was.
        pictures: screenOn ? { ...(now.id === meetingId ? now.pictures : {}), ...pictures } : {},
        screenError: detail.screen_error ?? null,
        turns: detail.turns ?? [],
        names: detail.names ?? {},
      }));
    } catch {
      // The meeting is gone or the service restarted.
    }
  }, []);

  useEffect(() => {
    if (!id) {
      setState((now) => {
        // Stopped between two looks: keep it as the saved session.
        if (now.id) setSavedId(now.id);
        return NOTHING;
      });
      return undefined;
    }
    let alive = true;
    void refresh(id, () => alive);
    // Every second: the words being said now and the screens' pictures
    // change that often.
    const timer = window.setInterval(() => void refresh(id, () => alive), 1000);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, [id, refresh]);

  // The saved session: followed while it is gone over, then kept as it is.
  useEffect(() => {
    if (!savedId) {
      setSaved(null);
      return undefined;
    }
    let alive = true;
    let timer = 0;
    const look = async () => {
      try {
        const detail = await bridge.meeting(savedId);
        const slides: string[] = [];
        for (const slide of (detail.slides ?? []).slice(-3)) {
          try {
            const png = await bridge.meetingSlide(savedId, slide.file);
            if (png) slides.push(`data:image/png;base64,${png}`);
          } catch {
            // Not kept.
          }
        }
        if (!alive) return;
        const meeting = detail.meeting;
        setSaved({
          id: savedId,
          title: meeting.title,
          state: meeting.state,
          progress: meeting.progress,
          turns: detail.turns ?? [],
          names: detail.names ?? {},
          slides,
          raw: Boolean(meeting.raw),
        });
        if (meeting.state === 'processing' || meeting.state === 'recording') timer = window.setTimeout(() => void look(), 3000);
      } catch {
        if (alive) setSaved(null);
      }
    };
    void look();
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
  }, [savedId, savedLook]);

  const flip = useCallback(
    (which: LiveSource) => {
      if (!state.id) return;
      const meetingId = state.id;
      const on = which === 'screen' ? state.screenOn : which === 'sound' ? state.soundOn : state.micOn;
      setBusy(true);
      setError(null);
      Promise.resolve()
        .then(() => bridge.meetingSources(meetingId, { [KEY[which]]: !on }))
        .then((meeting) => {
          // The last source switched off: the service has stopped and saved
          // the session. End this app's share now rather than waiting for
          // the service's event.
          if (meeting.state !== 'recording' && watch.id === meetingId) stopScreen();
          setState((now) => ({
            ...now,
            screenOn: Boolean(meeting.screen_on),
            soundOn: Boolean(meeting.sound_on),
            micOn: Boolean(meeting.mic_on),
          }));
        })
        .then(() => refresh(meetingId, () => true))
        .catch((e: unknown) => setError((e as { code?: string })?.code ?? 'unavailable'))
        .finally(() => setBusy(false));
    },
    [state.id, state.screenOn, state.soundOn, state.micOn, refresh, watch.id, stopScreen],
  );

  const dismissSaved = useCallback(() => setSavedId(null), []);
  const goOver = useCallback(async () => {
    if (!savedId) return;
    await bridge.meetingProcess(savedId, null, null, null);
    setSavedLook((n) => n + 1);
  }, [savedId]);
  const showSaved = useCallback((meetingId: string) => setSavedId(meetingId), []);
  const value = useMemo(
    () => ({ ...state, busy, error, flip, saved, dismissSaved, goOver, showSaved }),
    [state, busy, error, flip, saved, dismissSaved, goOver, showSaved],
  );
  return <LiveContext.Provider value={value}>{children}</LiveContext.Provider>;
}

function Transcript({ turns, names, empty }: { turns: MeetingTurn[]; names: Record<string, string>; empty: string }) {
  const end = useRef<HTMLDivElement | null>(null);
  const last = turns[turns.length - 1];
  useEffect(() => {
    end.current?.scrollIntoView?.({ block: 'nearest' });
  }, [turns.length, last?.text]);
  return (
    <div className="live-transcript" aria-live="polite" data-testid="live-transcript">
      {turns.length ? (
        turns.slice(-60).map((turn, i) => (
          <p
            key={turn.partial ? 'partial' : `${turn.start_ms}-${i}`}
            data-source={turn.source}
            data-partial={turn.partial ? 'true' : undefined}
            title={turn.partial ? t('assistant.live.partial') : undefined}
          >
            <span className="live-time">{clock(turn.start_ms)}</span>
            <span className="live-speaker" title={turn.source === 'room' ? t('assistant.live.fromMic') : t('assistant.live.fromSound')}>
              <Icon name={turn.source === 'room' ? 'mic' : 'speaker'} />
              {turn.partial ? '…' : (names[turn.speaker] ?? turn.speaker)}
            </span>
            {turn.text}
          </p>
        ))
      ) : (
        <p className="source">{empty}</p>
      )}
      <div ref={end} />
    </div>
  );
}

/** The side panel's live part: the screens, small, and the transcript; or,
 * once stopped, the saved session. */
export function LivePanel() {
  const live = useLive();
  const { watch, stopScreen } = useAssistant();
  const [stopping, setStopping] = useState(false);
  // Stopped and saved, whether this app started it or not (after a
  // restart, or from a chat since deleted).
  const stop = () => {
    setStopping(true);
    if (watch.id === live.id) {
      stopScreen();
      return;
    }
    bridge
      .meetingStop()
      .catch(() => {})
      .finally(() => setStopping(false));
  };
  if (!live.id)
    return live.saved ? <SavedPanel session={live.saved} onClose={live.dismissSaved} onGoOver={live.goOver} /> : null;
  const shown = live.sources.slice(0, 3);
  const hearing = live.soundOn || live.micOn;
  return (
    <section className="live-panel" aria-label={t('assistant.live.title')}>
      <h3>
        <span className="watch-dot" aria-hidden="true" />
        {t('assistant.live.title')}
        <button
          type="button"
          className="btn btn-default live-stop"
          disabled={stopping}
          title={t('assistant.live.stopHint')}
          onClick={stop}
        >
          <Icon name="stop" />
          {t('assistant.live.stop')}
        </button>
      </h3>
      {live.screenOn ? (
        <div className="live-block">
          <p className="live-label">
            <Icon name="play" />
            {t('assistant.live.screen')}
          </p>
          {live.screenError ? (
            <p className="assistant-error">{t('meetings.screen.liveError', { code: live.screenError })}</p>
          ) : shown.length ? (
            <ul className={`live-screens count-${shown.length}`} aria-label={t('meetings.screen.watching')}>
              {shown.map((source) => (
                <li key={source.source}>
                  {live.pictures[source.source] ? (
                    <img src={live.pictures[source.source]} alt={t('meetings.screen.liveAlt', { n: source.source + 1 })} />
                  ) : (
                    <span className="live-waiting" role="status">
                      {t('meetings.screen.waitingPicture')}
                    </span>
                  )}
                </li>
              ))}
            </ul>
          ) : (
            <p className="source">{t('meetings.screen.choosing')}</p>
          )}
        </div>
      ) : null}
      {hearing || live.turns.length ? (
        <div className="live-block">
          <p className="live-label">
            <Icon name="transcript" />
            {t('assistant.live.transcript')}
            <span className="live-hearing">
              {live.micOn ? (
                <span>
                  <Icon name="mic" />
                  {t('assistant.live.mic')}
                </span>
              ) : null}
              {live.soundOn ? (
                <span>
                  <Icon name="speaker" />
                  {t('assistant.live.sound')}
                </span>
              ) : null}
            </span>
          </p>
          <Transcript
            turns={live.turns}
            names={live.names}
            empty={hearing ? t('assistant.live.nothingYet') : t('assistant.live.paused')}
          />
        </div>
      ) : null}
      {!live.screenOn && !hearing ? <p className="source">{t('assistant.live.allOff')}</p> : null}
      {live.error ? <p className="assistant-error">{t('assistant.error.other', { code: live.error })}</p> : null}
    </section>
  );
}

function SavedPanel({
  session,
  onClose,
  onGoOver,
}: {
  session: SavedSession;
  onClose: () => void;
  onGoOver: () => Promise<void>;
}) {
  const { attachRecording, current } = useAssistant();
  const [attachError, setAttachError] = useState<string | null>(null);
  const [starting, setStarting] = useState(false);
  const [given, setGiven] = useState(false);
  const working = session.state === 'processing' || session.state === 'recording';
  const failed = session.state.startsWith('failed:');
  const alreadyHere = current?.meeting?.id === session.id;
  return (
    <section className="live-panel saved-panel" aria-label={t('assistant.saved.title')}>
      <h3>
        <Icon name={working ? 'refresh' : failed ? 'alert' : 'checkCircle'} />
        {t('assistant.saved.title')}
      </h3>
      <p className="saved-title">{session.title}</p>
      <p className="source" role="status">
        {working
          ? t('assistant.saved.working', { percent: Math.round(session.progress / 10) })
          : failed
            ? t('assistant.saved.failed', { code: session.state.slice('failed:'.length) })
            : session.raw
              ? t('assistant.saved.raw')
              : t('assistant.saved.ready')}
      </p>
      {session.slides.length ? (
        <div className="live-block">
          <p className="live-label">
            <Icon name="play" />
            {t('assistant.saved.screens')}
          </p>
          <ul className={`live-screens count-${session.slides.length}`}>
            {session.slides.map((src, i) => (
              <li key={i}>
                <img src={src} alt={t('assistant.saved.screenAlt', { n: i + 1 })} />
              </li>
            ))}
          </ul>
        </div>
      ) : null}
      {session.turns.length ? (
        <div className="live-block">
          <p className="live-label">
            <Icon name="transcript" />
            {t('assistant.saved.transcript')}
          </p>
          <Transcript turns={session.turns} names={session.names} empty="" />
        </div>
      ) : null}
      <div className="saved-actions">
        <button
          type="button"
          className="btn btn-primary"
          disabled={working || failed || given || alreadyHere}
          title={working ? t('assistant.saved.waitHint') : undefined}
          onClick={() => {
            setAttachError(null);
            attachRecording(session.id)
              .then(() => setGiven(true))
              .catch((e: unknown) => setAttachError((e as { code?: string })?.code ?? 'unavailable'));
          }}
        >
          <Icon name="spark" />
          {given || alreadyHere ? t('assistant.saved.given') : t('assistant.screen.giveToChat')}
        </button>
        {session.raw && !working ? (
          <button
            type="button"
            className="btn btn-default"
            disabled={starting}
            onClick={() => {
              setStarting(true);
              setAttachError(null);
              onGoOver()
                .catch((e: unknown) => setAttachError((e as { code?: string })?.code ?? 'unavailable'))
                .finally(() => setStarting(false));
            }}
          >
            <Icon name="refresh" />
            {t('assistant.saved.goOver')}
          </button>
        ) : null}
        <button type="button" className="btn btn-ghost" onClick={onClose}>
          {t('assistant.screen.close')}
        </button>
      </div>
      {attachError ? <p className="assistant-error">{t('assistant.error.other', { code: attachError })}</p> : null}
    </section>
  );
}

/** Every session kept from the assistant (shares and recordings), newest
 * first: none is lost when a new one starts, and any can be shown or given
 * to the open chat. */
export function SavedSessions() {
  const live = useLive();
  const { attachRecording, current } = useAssistant();
  const [sessions, setSessions] = useState<Meeting[]>([]);
  const mine = useMemo(() => current?.sessions ?? [], [current?.sessions]);
  const [given, setGiven] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    const look = () =>
      Promise.resolve()
        .then(() => bridge.meetings())
        .then((list) => {
          if (!alive) return;
          // Only this chat's: the shares and recordings made in it or given
          // to it (older ones are on the Meetings screen).
          setSessions(
            list
              .filter((m) => mine.includes(m.id) && (m.watch || m.share) && m.state !== 'recording')
              .sort((a, b) => b.started_unix_ms - a.started_unix_ms),
          );
        })
        .catch(() => {});
    void look();
    const timer = window.setInterval(() => void look(), 10_000);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, [live.id, live.saved?.id, mine]);
  if (!sessions.length) return null;
  return (
    <section className="live-panel saved-sessions" aria-label={t('assistant.sessions.title')}>
      <h3>{t('assistant.sessions.title')}</h3>
      <ul>
        {sessions.map((session) => {
          const here = current?.meeting?.id === session.id || given === session.id;
          return (
            <li key={session.id}>
              <span className="saved-sessions-name">
                <span className="saved-title">{session.title}</span>
                <span className="source">
                  {clock(session.duration_ms)}
                  {session.raw ? ` · ${t('assistant.sessions.raw')}` : ''}
                </span>
              </span>
              <span className="saved-actions">
                <button type="button" className="btn btn-ghost" onClick={() => live.showSaved(session.id)}>
                  {t('assistant.sessions.show')}
                </button>
                <button
                  type="button"
                  className="btn btn-default"
                  disabled={here || session.state !== 'ready'}
                  onClick={() => {
                    setError(null);
                    attachRecording(session.id)
                      .then(() => setGiven(session.id))
                      .catch((e: unknown) => setError((e as { code?: string })?.code ?? 'unavailable'));
                  }}
                >
                  {here ? t('assistant.saved.given') : t('assistant.sessions.give')}
                </button>
              </span>
            </li>
          );
        })}
      </ul>
      {error ? <p className="assistant-error">{t('assistant.watch.error.other', { code: error })}</p> : null}
    </section>
  );
}

