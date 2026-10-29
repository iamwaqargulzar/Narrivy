// Models → Text to speech: the voices voice mode reads replies with, on this
// computer. Whether they are set up and which English voice is chosen come
// from the service's voice settings; the voice is changed in voice mode.
import { useEffect, useState } from 'react';
import { bridge } from '../../lib/bridge';
import { VOICES } from '../../lib/voices';
import { Icon } from '../../ui/icons';
import { t } from '../../i18n';

interface VoiceFacts {
  available: boolean;
  voice: number;
}

export function TextToSpeechPanel() {
  const [facts, setFacts] = useState<VoiceFacts | null>(null);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let current = true;
    bridge
      .assistantLibrary('get', 'voice')
      .then((raw) => {
        if (!current) return;
        const value = JSON.parse(raw) as { available?: boolean; voice?: number };
        setFacts({ available: Boolean(value.available), voice: value.voice ?? VOICES[0][0] });
      })
      .catch(() => {
        if (current) setFailed(true);
      });
    return () => {
      current = false;
    };
  }, []);

  if (failed) {
    return (
      <div className="panel empty-state">
        <Icon name="speaker" />
        <strong>{t('tts.unavailableTitle')}</strong>
        <span className="screen-empty-note">{t('tts.unavailable')}</span>
      </div>
    );
  }
  if (facts === null) return <p className="source">{t('common.loading')}</p>;

  const state = (
    <span className={`chip${facts.available ? ' chip-ok' : ''}`}>
      {facts.available ? t('tts.ready') : t('tts.notSetUp')}
    </span>
  );
  return (
    <section className="model-section">
      <div className="model-section-head">
        <h3>{t('tts.voices')}</h3>
        <span className="model-count">{2}</span>
        <span className="model-section-hint">{t('tts.voicesHint')}</span>
      </div>
      <ul className="model-rows">
        <li className="model-row" data-testid="tts-kokoro">
          <div className="model-row-main">
            <div className="model-row-title">
              <span className="model-row-name">{t('tts.kokoro')}</span>
              {state}
            </div>
            <div className="model-facts">
              <span className="model-fact">
                <Icon name="globe" />
                {t('assistant.voice.english')}
              </span>
              <span className="model-fact">
                <Icon name="speaker" />
                {t('tts.voiceCount', { count: VOICES.length })}
              </span>
            </div>
            <p className="model-source">
              {t('tts.inUse', { voice: VOICES.find(([id]) => id === facts.voice)?.[1] ?? VOICES[0][1] })}
            </p>
          </div>
        </li>
        <li className="model-row" data-testid="tts-piper-urdu">
          <div className="model-row-main">
            <div className="model-row-title">
              <span className="model-row-name">{t('tts.piperUrdu')}</span>
              {state}
            </div>
            <div className="model-facts">
              <span className="model-fact">
                <Icon name="globe" />
                {t('assistant.voice.urdu')}
              </span>
            </div>
            <p className="model-source">{t('tts.inUse', { voice: t('assistant.voice.urduVoice') })}</p>
          </div>
        </li>
      </ul>
      <p className="source lm-more">{t('tts.more')}</p>
    </section>
  );
}
