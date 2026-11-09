// Voice mode's panel: an orb that shows whether the assistant is listening,
// hearing you, thinking or speaking; the words heard so far; where the last
// turn's time went; and the controls (mute, stop, speed, push-to-talk,
// interrupting, sounds, and the English and Urdu voices) in a popover.
import { useCallback, useState } from 'react';
import { usePopoverDismiss } from '../../../hooks/useDialogFocus';
import { Icon } from '../../../ui/icons';
import { hasMessage, t } from '../../../i18n';
import { useAssistant } from '../../../state/AssistantContext';
import { VOICES } from '../../../lib/voices';

export function VoicePanel() {
  const { voice } = useAssistant();
  const { state, partial, timing, error, prefs } = voice;
  const muted = state === 'muted';
  const [open, setOpen] = useState(false);
  const close = useCallback(() => setOpen(false), []);
  const ref = usePopoverDismiss(open, close);
  const english = VOICES.find(([id]) => id === prefs.voice)?.[1] ?? VOICES[0][1];
  return (
    <section className="voice-panel" aria-label={t('assistant.voice.title')} data-state={state}>
      <div className="voice-orb" data-state={state} aria-hidden="true">
        <span />
      </div>
      <div className="voice-main">
        <p className="voice-state" role="status">
          {t(`assistant.voice.state.${state}`)}
          {prefs.pushToTalk && (state === 'listening' || state === 'hearing') ? ` · ${t('assistant.voice.holdSpace')}` : ''}
        </p>
        {partial ? (
          <p className="voice-partial">{partial}</p>
        ) : error ? (
          <p className="voice-sub assistant-error" role="alert">
            {hasMessage(`assistant.voice.error.${error}`)
              ? t(`assistant.voice.error.${error}`)
              : t('assistant.voice.error.other', { code: error })}
          </p>
        ) : (
          <p className="voice-sub">
            {t('assistant.voice.speaksWith', { voice: english.split(' (')[0] })}
            {timing ? (
              <span title={t('assistant.voice.timingHint')}>
                {' · '}
                {t('assistant.voice.timingShort', { total: (timing.total_ms / 1000).toFixed(1) })}
              </span>
            ) : null}
          </p>
        )}
      </div>
      <div className="voice-buttons" ref={ref}>
        <button
          type="button"
          className={`voice-round${muted ? ' is-off' : ''}`}
          aria-pressed={muted}
          aria-label={muted ? t('assistant.voice.unmute') : t('assistant.voice.mute')}
          title={t('key.withShortcut', { label: muted ? t('assistant.voice.unmute') : t('assistant.voice.mute'), keys: 'Ctrl+M' })}
          onClick={() => voice.setPrefs({ muted: !muted })}
        >
          <Icon name="mic" />
        </button>
        <button
          type="button"
          className="voice-round"
          aria-expanded={open}
          aria-label={t('assistant.voice.settings')}
          title={t('assistant.voice.settings')}
          onClick={() => setOpen(!open)}
        >
          <Icon name="sliders" />
        </button>
        <button
          type="button"
          className="voice-end"
          onClick={() => voice.setStandby(true)}
          title={t('key.withShortcut', { label: t('assistant.voice.offHint'), keys: 'Ctrl+Shift+V' })}
        >
          <Icon name="close" />
          {t('assistant.voice.end')}
        </button>
        {open ? (
          <div className="assistant-menu voice-menu" role="dialog" aria-label={t('assistant.voice.settings')}>
            <p className="assistant-menu-heading">{t('assistant.voice.voices')}</p>
            <label className="voice-row">
              <span className="voice-row-label">{t('assistant.voice.english')}</span>
              <select
                className="pipe-select"
                value={prefs.voice}
                onChange={(event) => voice.setPrefs({ voice: Number(event.target.value) })}
              >
                {VOICES.map(([id, name]) => (
                  <option key={id} value={id}>
                    {name}
                  </option>
                ))}
              </select>
            </label>
            <div className="voice-row">
              <span className="voice-row-label">{t('assistant.voice.urdu')}</span>
              <span className="voice-fixed">{t('assistant.voice.urduVoice')}</span>
            </div>
            <p className="voice-hint">{t('assistant.voice.urduHint')}</p>

            <p className="assistant-menu-heading">{t('assistant.voice.speaking')}</p>
            <label className="voice-row">
              <span className="voice-row-label">{t('assistant.voice.speedLabel')}</span>
              <input
                type="range"
                min={100}
                max={175}
                step={5}
                value={prefs.speedPercent}
                aria-valuetext={t('assistant.voice.speedValue', { speed: (prefs.speedPercent / 100).toFixed(2) })}
                onChange={(event) => voice.setPrefs({ speedPercent: Number(event.target.value) })}
              />
              <span className="voice-value">{(prefs.speedPercent / 100).toFixed(2)}×</span>
            </label>
            <div className="voice-row voice-row-stack">
              <span className="voice-row-label">{t('assistant.voice.whenTalking')}</span>
              <div className="voice-segments" role="radiogroup" aria-label={t('assistant.voice.whenTalking')}>
                {(['stop', 'queue'] as const).map((choice) => (
                  <button
                    key={choice}
                    type="button"
                    role="radio"
                    aria-checked={prefs.interrupt === choice}
                    onClick={() => voice.setPrefs({ interrupt: choice })}
                  >
                    {choice === 'stop' ? t('assistant.voice.interruptStop') : t('assistant.voice.interruptQueue')}
                  </button>
                ))}
              </div>
            </div>
            {(
              [
                ['pushToTalk', t('assistant.voice.pushToTalk')],
                ['sounds', t('assistant.voice.sounds')],
              ] as const
            ).map(([key, label]) => (
              <div className="assistant-switch-row" key={key}>
                <span>
                  <span className="assistant-switch-label">{label}</span>
                </span>
                <button
                  type="button"
                  role="switch"
                  className="switch"
                  aria-checked={prefs[key]}
                  aria-label={label}
                  onClick={() => voice.setPrefs({ [key]: !prefs[key] })}
                >
                  <span className="switch-thumb" />
                </button>
              </div>
            ))}
            <div className="voice-unload">
              <span className="voice-hint">{t('assistant.voice.unloadHint')}</span>
              <button type="button" className="btn btn-default" onClick={voice.stop}>
                {t('assistant.voice.unload')}
              </button>
            </div>
            {timing ? (
              <p className="voice-hint">
                {t('assistant.voice.timing', {
                  total: (timing.total_ms / 1000).toFixed(1),
                  asr: (timing.asr_ms / 1000).toFixed(1),
                  reply: (timing.reply_ms / 1000).toFixed(1),
                  voice: (timing.voice_ms / 1000).toFixed(1),
                })}
              </p>
            ) : null}
          </div>
        ) : null}
      </div>
    </section>
  );
}
