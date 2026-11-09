// Models → Text to speech: the voices voice mode reads replies with, on this
// computer. The voices are Store items (`kind: "voice"`): the service lists
// them, says whether each is installed, installs it with the same checked
// download as every pack, and removes it. Whether voice mode is ready and
// which English voice is chosen come from the service's voice settings; the
// voice is changed in voice mode.
import { useCallback, useEffect, useState } from 'react';
import { bridge, type StoreItem } from '../../lib/bridge';
import { VOICES } from '../../lib/voices';
import { useDaemonStatus } from '../../state/StatusContext';
import { useErrorHandler } from '../../ui/primitives/Toast';
import { Icon } from '../../ui/icons';
import { t } from '../../i18n';
import { StoreCard, type Actions } from './Store';

const POLL_MS = 1000;

interface VoiceFacts {
  available: boolean;
  voice: number;
}

function readFacts(): Promise<VoiceFacts> {
  return bridge.assistantLibrary('get', 'voice').then((raw) => {
    const value = JSON.parse(raw) as { available?: boolean; voice?: number };
    return { available: Boolean(value.available), voice: value.voice ?? VOICES[0][0] };
  });
}

export function TextToSpeechPanel() {
  const { reload } = useDaemonStatus();
  const report = useErrorHandler();
  const [voices, setVoices] = useState<StoreItem[] | null>(null);
  const [facts, setFacts] = useState<VoiceFacts | null>(null);
  const [failed, setFailed] = useState(false);

  const load = useCallback(async () => {
    try {
      const [items, read] = await Promise.all([bridge.storeItems(), readFacts()]);
      setVoices(items.filter((item) => item.kind === 'voice'));
      setFacts(read);
      setFailed(false);
    } catch {
      setFailed(true);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  // While a voice downloads, follow its progress.
  const installing = (voices ?? []).some((item) => item.state === 'installing');
  useEffect(() => {
    if (!installing) return;
    const timer = window.setInterval(() => void load(), POLL_MS);
    return () => window.clearInterval(timer);
  }, [installing, load]);

  const after = (action: Promise<void>) =>
    action
      .then(() => load())
      .then(() => reload())
      .catch((err) => report(err));
  const actions: Actions = {
    busy: installing,
    install: (item) => void after(bridge.downloadModel(item.id)),
    stop: (item) => void after(bridge.cancelModelDownload(item.id)),
    remove: (item) => void after(bridge.removeStoreItem(item.id)),
  };

  if (failed) {
    return (
      <div className="panel empty-state">
        <Icon name="speaker" />
        <strong>{t('tts.unavailableTitle')}</strong>
        <span className="screen-empty-note">{t('tts.unavailable')}</span>
      </div>
    );
  }
  if (voices === null || facts === null) return <p className="source">{t('common.loading')}</p>;

  const chosen = VOICES.find(([id]) => id === facts.voice)?.[1] ?? VOICES[0][1];
  const names = new Map(voices.map((item) => [item.id, item.name]));
  return (
    <section className="model-section">
      <div className="model-section-head">
        <h3>{t('tts.voices')}</h3>
        <span className="model-count">{voices.length}</span>
        <span className="model-section-hint">{t('tts.voicesHint')}</span>
      </div>
      {voices.length === 0 ? (
        <p className="source">{t('tts.none')}</p>
      ) : (
        <ul className="store-grid">
          {voices.map((item) => (
            <StoreCard key={item.id} item={item} names={names} actions={actions} />
          ))}
        </ul>
      )}
      <p className="source lm-more" data-testid="tts-state">
        <span className={`chip${facts.available ? ' chip-ok' : ''}`}>
          {facts.available ? t('tts.ready') : t('tts.notSetUp')}
        </span>{' '}
        {facts.available ? t('tts.inUse', { voice: chosen }) : t('tts.needsVoice')} {t('tts.more')}
      </p>
      <p className="source">{t('tts.urdu')}</p>
    </section>
  );
}
