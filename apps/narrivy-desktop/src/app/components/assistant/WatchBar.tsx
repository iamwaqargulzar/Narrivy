// Screen share and screen recording with the assistant. Sharing is live:
// the open chat sees the screen (and hears what is said) while it lasts,
// and nothing is kept. Recording keeps a session, transcribed and with its
// pictures described, to give to a chat later.
import { useCallback, useEffect, useRef, useState } from 'react';
import { Icon } from '../../../ui/icons';
import { hasMessage, t } from '../../../i18n';
import { useAssistant, type ScreenMode } from '../../../state/AssistantContext';
import { useLive } from './LiveSession';
import { clock } from '../../../lib/meetingContext';

/** The top bar's "Record a session": a screen recording (or only the
 * sound) kept as a session to give to a chat later. Sharing live is the
 * composer's Screen and Transcript buttons. */
export function WatchButton() {
  const { watch, startScreen, busy } = useAssistant();
  const [open, setOpen] = useState(false);
  const mode: ScreenMode = 'record';
  const [screen, setScreen] = useState(true);
  const [microphone, setMicrophone] = useState(false);
  const [computerSound, setComputerSound] = useState(true);
  const ref = useRef<HTMLDivElement | null>(null);
  const close = useCallback(() => setOpen(false), []);
  useEffect(() => {
    if (!open) return undefined;
    const onDown = (event: MouseEvent) => {
      if (ref.current && !ref.current.contains(event.target as Node)) close();
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') close();
    };
    document.addEventListener('mousedown', onDown);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey);
    };
  }, [open, close]);
  if (watch.phase !== 'idle' && watch.phase !== 'error') return null;
  return (
    <div className="watch-start" ref={ref}>
      <button
        type="button"
        className="btn btn-default"
        aria-expanded={open}
        disabled={busy}
        title={t('assistant.screen.recordHint')}
        onClick={() => setOpen(!open)}
      >
        <span className="record-dot" aria-hidden="true" />
        {t('assistant.screen.recordButton')}
      </button>
      {open ? (
        <div className="watch-popover" role="dialog" aria-label={t('assistant.screen.recordButton')}>
          <p className="watch-popover-lead">{t('assistant.screen.recordLead')}</p>
          <div className="screen-what" role="radiogroup" aria-label={t('assistant.screen.what')}>
            {([true, false] as const).map((choice) => (
              <button
                key={String(choice)}
                type="button"
                role="radio"
                aria-checked={screen === choice}
                className="screen-what-option"
                onClick={() => setScreen(choice)}
              >
                <Icon name={choice ? 'play' : 'wave'} />
                {choice ? t('assistant.screen.withScreen') : t('assistant.screen.soundOnly')}
              </button>
            ))}
          </div>
          <div className="assistant-switch-row">
            <span>
              <span className="assistant-switch-label" id="screen-computer-sound">
                {t('assistant.screen.computerSound')}
              </span>
              <span className="assistant-switch-hint">{t('assistant.screen.soundHint')}</span>
            </span>
            <button
              type="button"
              role="switch"
              className="switch"
              aria-checked={computerSound}
              aria-labelledby="screen-computer-sound"
              onClick={() => setComputerSound(!computerSound)}
            >
              <span className="switch-thumb" />
            </button>
          </div>
          <div className="assistant-switch-row">
            <span>
              <span className="assistant-switch-label" id="screen-microphone">
                {t('assistant.screen.microphone')}
              </span>
            </span>
            <button
              type="button"
              role="switch"
              className="switch"
              aria-checked={microphone}
              aria-labelledby="screen-microphone"
              onClick={() => setMicrophone(!microphone)}
            >
              <span className="switch-thumb" />
            </button>
          </div>
          {!screen && !microphone && !computerSound ? (
            <p className="assistant-error">{t('assistant.screen.nothing')}</p>
          ) : null}
          <button
            type="button"
            className="btn btn-primary"
            disabled={!screen && !microphone && !computerSound}
            onClick={() => {
              setOpen(false);
              startScreen(mode, { screen, microphone, computerSound });
            }}
          >
            {screen ? t('assistant.screen.startRecord') : t('assistant.screen.startRecordSound')}
          </button>
        </div>
      ) : null}
    </div>
  );
}

export function WatchBar() {
  const { watch, stopScreen, controlWatching, dismissScreen } = useAssistant();
  const [, tick] = useState(0);
  // Its screen and transcript are switched from the composer and shown in
  // the side panel.
  const live = useLive();
  const showScreen = live.id === watch.id ? live.screenOn : watch.screen !== false;
  useEffect(() => {
    if (watch.phase !== 'watching') return undefined;
    const timer = window.setInterval(() => tick((n) => n + 1), 1000);
    return () => window.clearInterval(timer);
  }, [watch.phase]);
  // Once saved, the session shows in the side panel (to give to a chat
  // there); the bar steps aside so a new one can start.
  useEffect(() => {
    if (watch.phase === 'ready') dismissScreen();
  }, [watch.phase, dismissScreen]);
  if (watch.phase === 'idle' || watch.phase === 'ready') return null;
  if (watch.phase === 'error') {
    return (
      <p className="assistant-attached assistant-error" role="status">
        {hasMessage(`assistant.watch.error.${watch.error}`)
          ? t(`assistant.watch.error.${watch.error}`)
          : t('assistant.watch.error.other', { code: watch.error ?? '' })}
        <button type="button" className="btn btn-ghost" onClick={dismissScreen}>
          {t('assistant.screen.close')}
        </button>
      </p>
    );
  }
  const share = watch.mode === 'share';
  const time = clock(Date.now() - watch.startedAt);
  const status =
    watch.phase === 'starting'
      ? t('assistant.screen.starting')
      : watch.phase === 'watching'
        ? share
          ? watch.screen === false
            ? t('assistant.screen.listening', { time })
            : t('assistant.screen.sharing', { time })
          : watch.screen === false
            ? t('assistant.screen.recordingSound', { time })
            : t('assistant.screen.recording', { time })
        : watch.phase === 'processing'
          ? watch.paused
            ? t('assistant.watch.paused')
            : t('assistant.screen.processing')
          : '';
  return (
    <section className="watch-bar" data-mode={watch.mode} aria-label={t('assistant.screen.button')}>
      <div className="watch-bar-head">
        <span
          className="watch-dot"
          data-paused={watch.paused ? 'true' : 'false'}
          aria-hidden="true"
        />
        <strong role="status">{status}</strong>
        {watch.phase === 'watching' ? (
          <button type="button" className="btn btn-primary" onClick={stopScreen}>
            <Icon name="stop" />
            {share ? t('assistant.screen.stopShare') : t('assistant.screen.stopRecord')}
          </button>
        ) : null}
        {watch.phase === 'processing' ? (
          <span className="watch-actions">
            <button
              type="button"
              className="btn btn-default"
              disabled={!watch.id}
              onClick={() => controlWatching(watch.paused ? 'resume' : 'pause')}
            >
              <Icon name={watch.paused ? 'play' : 'pause'} />
              {watch.paused ? t('assistant.watch.resume') : t('assistant.watch.pause')}
            </button>
            <button
              type="button"
              className="btn btn-default"
              title={t('assistant.watch.cancelHint')}
              onClick={() => controlWatching('stop')}
            >
              <Icon name="stop" />
              {t('assistant.watch.cancel')}
            </button>
          </span>
        ) : null}
        {watch.phase === 'starting' ? (
          <button type="button" className="btn btn-default" onClick={() => controlWatching('stop')}>
            <Icon name="close" />
            {t('assistant.cancel')}
          </button>
        ) : null}
      </div>
      {share && watch.phase === 'watching' ? (
        <p className="watch-note">
          {showScreen ? t('assistant.screen.shareNote') : t('assistant.screen.listenNote')}
        </p>
      ) : null}
    </section>
  );
}
