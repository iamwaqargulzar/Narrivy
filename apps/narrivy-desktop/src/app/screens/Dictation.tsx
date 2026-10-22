// Dictation: how dictation behaves. The session panel (state, Start/Stop,
// live words), the modes the daemon advertises, how to trigger dictation from
// any app, recognition settings and capture limits. Modes are daemon
// pipelines: only enabled lanes are selectable and Start sends the exact id.
import { Button } from '../../ui/primitives/Button';
import { Icon } from '../../ui/icons';
import { t } from '../../i18n';
import { activationPresentation } from '../../lib/activation';
import { documentedCaptureMaxSeconds, reportedCaptureLimitSeconds } from '../../lib/capturePolicy';
import { useDaemonStatus } from '../../state/StatusContext';
import { usePipelines } from '../../state/PipelinesContext';
import { useSession } from '../../state/SessionContext';
import { useDictationActions } from '../../hooks/useDictationActions';
import { useNav } from '../../state/NavContext';
import { StatusPill } from '../../ui/primitives/StatusPill';
import { RecognitionSettingsCard } from '../components/SettingsCompute/RecognitionSettingsCard';
import { DictationNotices, SessionDetails } from '../components/DictationShared';
import '../components/SettingsCompute/SettingsCompute.css';

const MODE_HINT_KEYS: Record<string, string> = {
  clean: 'dictationPage.mode.clean',
  raw: 'dictationPage.mode.raw',
};

export function DictationScreen() {
  const { status } = useDaemonStatus();
  const { catalog, select, acknowledgeSelectionChange } = usePipelines();
  const session = useSession();
  const actions = useDictationActions();
  const { navigate } = useNav();
  const activation = activationPresentation(status);
  const captureLimit = reportedCaptureLimitSeconds(status);
  const captureMax = documentedCaptureMaxSeconds(status);
  const live = ['preparing', 'recording', 'listening'].includes(session.phase);
  const selected = catalog.pipelines.find((p) => p.id === catalog.selectedId) ?? null;
  // A daemon-advertised selection whose lane is momentarily not enabled is
  // kept for context, but it is not offered as a startable choice.
  const preservedUnavailable = selected !== null && !selected.enabled;

  return (
    <div className="page-wide">
      <div className="page-head">
        <h2 className="page-title">{t('nav.dictation')}</h2>
        <p className="page-lead">{t('dictationPage.lead')}</p>
      </div>

      <DictationNotices />

      <section className="card session-card" data-live={live ? 'true' : 'false'}>
        <div className="card-head">
          <Icon name="mic" className="card-head-icon" />
          <h3 className="card-head-title">{t('dictationPage.session')}</h3>
          <StatusPill phase={session.phase || (status ? 'idle' : '')} />
          <span className="card-head-spacer" />
          <Button
            variant="primary"
            onClick={() => {
              acknowledgeSelectionChange();
              void actions.start();
            }}
            disabled={!actions.canStart}
            aria-label={t('home.start')}
            data-testid="dictation-start"
            className="btn-press"
          >
            <Icon name="mic" />
            {t('dictation.start')}
          </Button>
          <Button onClick={() => void actions.stop()} disabled={!actions.canStop} aria-label={t('home.stopLabel')} data-testid="dictation-stop">
            {t('dictation.stop')}
          </Button>
          <Button onClick={() => void actions.cancel()} disabled={!actions.canCancel} aria-label={t('home.cancelLabel')} data-testid="dictation-cancel">
            {t('dictation.cancel')}
          </Button>
        </div>
        <div className="transcript-box">
          <p className={`transcript-text${session.tentativeTranscript ? (live ? ' is-live' : '') : ' transcript-empty'}`} aria-live="polite">
            {session.tentativeTranscript ?? (live ? t('dictation.liveNone') : session.lastTranscript ?? t('dictation.none'))}
          </p>
        </div>
      </section>

      <h3 className="section-title">{t('dictationPage.modes')}</h3>
      <section className="card mode-card">
        <label className="mode-field">
          <span className="mode-label">{t('dictation.mode')}</span>
          <select
            aria-label={t('dictation.mode')}
            value={catalog.selectedId ?? ''}
            onChange={(e) => select(e.target.value)}
            disabled={actions.busy || catalog.enabled.length === 0}
            data-testid="dictation-mode-select"
          >
            {preservedUnavailable && selected ? (
              <option value={selected.id} disabled>
                {selected.name} ({selected.id}) — {t('dictation.modeUnavailable')}
              </option>
            ) : null}
            {catalog.enabled.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name} ({p.id})
              </option>
            ))}
            {catalog.enabled.length === 0 && !preservedUnavailable ? (
              <option value="">{t('dictation.modeNone')}</option>
            ) : null}
          </select>
        </label>
        <p className="card-text">
          {selected && MODE_HINT_KEYS[selected.id] ? t(MODE_HINT_KEYS[selected.id]) : t('dictationPage.modeHint')}
        </p>
        <SessionDetails />
      </section>

      <h3 className="section-title">{t('dictationPage.trigger')}</h3>
      <div className="card-row">
        <section className="card">
          <div className="card-head">
            <Icon name="keyboard" className="card-head-icon" />
            <h4 className="card-head-title">{t('dictationPage.shortcut')}</h4>
          </div>
          {activation.suggestedCommand ? (
            <>
              <p className="card-text">{t('onboard.shortcut.body')}</p>
              <code className="code-chip code-block">{activation.suggestedCommand}</code>
              <p className="source">{t('onboard.shortcut.notConfigured')}</p>
            </>
          ) : activation.portalListenerReady ? (
            <p className="card-text">{t('onboard.shortcut.listenerReady')}</p>
          ) : (
            <p className="card-text">{t('fm.value.notReported')}</p>
          )}
        </section>
        <section className="card">
          <div className="card-head">
            <Icon name="apps" className="card-head-icon" />
            <h4 className="card-head-title">{t('dictationPage.tray')}</h4>
          </div>
          <p className="card-text">{t('dictationPage.trayBody')}</p>
        </section>
        <section className="card">
          <div className="card-head">
            <Icon name="info" className="card-head-icon" />
            <h4 className="card-head-title">{t('dictationPage.limits')}</h4>
          </div>
          <p className="card-text">
            {captureLimit !== null
              ? captureMax !== null && captureMax !== captureLimit
                ? t('dictation.captureLimitDeveloper', { seconds: captureLimit, max: captureMax })
                : t('dictation.captureLimit', { seconds: captureLimit })
              : t('fm.value.notReported')}
          </p>
          <button type="button" className="link-action" onClick={() => navigate('settings')}>
            {t('dictationPage.moreSettings')}
            <Icon name="arrowRight" />
          </button>
        </section>
      </div>

      <h3 className="section-title">{t('dictationPage.recognition')}</h3>
      <RecognitionSettingsCard />
    </div>
  );
}
