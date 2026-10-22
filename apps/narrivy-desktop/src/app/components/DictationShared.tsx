// Pieces shared by Home and Dictation: the readiness/availability notices
// (with their recovery actions) and the collapsed setup details.
import { Button } from '../../ui/primitives/Button';
import { Icon } from '../../ui/icons';
import { t } from '../../i18n';
import { availabilityMessageKey } from '../../lib/pipelines';
import {
  captureEndNotice,
  documentedCaptureMaxSeconds,
  reportedCaptureLimitSeconds,
} from '../../lib/capturePolicy';
import { resolveCompute, resolveDelivery, resolveMic, resolveModel } from '../../lib/resolved';
import { engineFacts } from '../../lib/overview';
import { reportedLanguages } from '../../lib/modelFacts';
import { loadSettings } from '../../lib/settings';
import { useDaemonStatus } from '../../state/StatusContext';
import { usePipelines } from '../../state/PipelinesContext';
import { useSession } from '../../state/SessionContext';
import { useDictationActions } from '../../hooks/useDictationActions';
import { useNav } from '../../state/NavContext';

/** Availability, selection-change and capture-end notices plus recovery. */
export function DictationNotices() {
  const { status, error: statusError, reload } = useDaemonStatus();
  const { catalog, refresh, selectionChange } = usePipelines();
  const session = useSession();
  const actions = useDictationActions();
  const { navigate } = useNav();
  const guidanceKey = availabilityMessageKey(catalog);
  const captureNotice = captureEndNotice(status, session.captureEnd);
  const daemonDown = !status && Boolean(statusError);
  const needsRetry = daemonDown || catalog.availability === 'error' || catalog.availability === 'unavailable';
  const needsModels =
    catalog.availability === 'empty' || catalog.availability === 'awaiting_model' || catalog.modelBlocked;
  const idle = !(actions.canStop || actions.canCancel);

  return (
    <div className="notices">
      {daemonDown ? (
        <div className="notice notice-warn" role="status">
          <Icon name="alert" />
          <div className="notice-body">{t('home.readiness.daemonUnavailable')}</div>
        </div>
      ) : null}
      {guidanceKey ? (
        <div className="notice notice-warn" role="status" data-testid="dictation-availability">
          <Icon name="alert" />
          <div className="notice-body">
            {t(guidanceKey)}
            {catalog.availability === 'unavailable' && catalog.reason
              ? ` ${t('pipelines.reason', { reason: catalog.reason })}`
              : ''}
          </div>
        </div>
      ) : null}
      {selectionChange ? (
        <div className="notice notice-info" role="status" data-testid="dictation-selection-change">
          <Icon name="info" />
          <div className="notice-body">
            {selectionChange.to
              ? t('pipelines.selectionChanged', { id: selectionChange.to })
              : t('pipelines.selectionRemoved')}
          </div>
        </div>
      ) : null}
      {captureNotice ? (
        <div className="notice notice-info" role="status" data-testid="dictation-capture-end">
          <Icon name="info" />
          <div className="notice-body">{t(captureNotice.key, captureNotice.values)}</div>
        </div>
      ) : null}
      {idle && (needsRetry || needsModels) ? (
        <div className="notice-actions">
          {needsRetry ? (
            <Button
              onClick={() => {
                void reload();
                void refresh();
              }}
              data-testid="home-retry"
            >
              <Icon name="refresh" />
              {t('action.retry')}
            </Button>
          ) : null}
          {needsModels ? (
            <Button variant="primary" onClick={() => navigate('models')} data-testid="home-open-models">
              {t('home.openModels')}
              <Icon name="arrowRight" />
            </Button>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

/** The full resolved values behind every summary, collapsed by default. */
export function SessionDetails() {
  const { status } = useDaemonStatus();
  const { catalog, refresh } = usePipelines();
  const snap = status ?? { capabilities: [], active_session: null };
  const mode = (key: string): 'auto' | 'custom' => (loadSettings().values[key] === 'auto' ? 'auto' : 'custom');
  const compute = resolveCompute(snap as never, mode('compute.device'));
  const model = resolveModel(snap as never, mode('asr.model'));
  const mic = resolveMic(snap as never, mode('audio.input_device'));
  const delivery = resolveDelivery(snap as never, mode('delivery.mode'));
  const language = loadSettings().values['asr.language'];
  const engine = engineFacts(status);
  const selected = catalog.pipelines.find((p) => p.id === catalog.selectedId) ?? null;
  const modeValue = selected ? `${selected.name} (${selected.id})` : t('home.mode.none');
  const captureLimit = reportedCaptureLimitSeconds(status);
  const captureMax = documentedCaptureMaxSeconds(status);

  const modelStatusText = (() => {
    if (!status) return t('home.status.unknown');
    switch (status.installed_model ?? null) {
      case 'ready':
        return t('home.model.ready');
      case 'awaiting_model':
        return t('home.model.awaiting');
      case 'provisioning':
        return t('home.model.provisioning');
      case 'activation_failed':
        return t('home.model.activationFailed');
      case 'published_unconfirmed':
        return t('home.model.publishedUnconfirmed');
      default:
        return t('home.status.notReported');
    }
  })();

  return (
    <details className="details-toggle">
      <summary>{t('home.details')}</summary>
      <dl className="details-grid">
        <div className="detail-cell">
          <dt>{t('home.mic.title')}</dt>
          <dd>{mic.resolved}</dd>
        </div>
        <div className="detail-cell">
          <dt>{t('home.model.title')}</dt>
          <dd>{model.resolved}</dd>
        </div>
        <div className="detail-cell">
          <dt>{t('home.status.model')}</dt>
          <dd data-testid="home-model-status">{modelStatusText}</dd>
        </div>
        <div className="detail-cell">
          <dt>{t('home.compute.title')}</dt>
          <dd>{compute.resolved}</dd>
        </div>
        <div className="detail-cell">
          <dt>{t('home.delivery.title')}</dt>
          <dd data-testid="home-delivery-value">{delivery.resolved}</dd>
        </div>
        <div className="detail-cell">
          <dt>{t('home.status.mode')}</dt>
          <dd data-testid="home-mode-value">{modeValue}</dd>
        </div>
        <div className="detail-cell">
          <dt>{t('home.language')}</dt>
          <dd title={engine.languages ?? undefined}>
            {engine.languages ? reportedLanguages(engine.languages) : language === 'auto' ? 'Auto detect' : String(language)}
          </dd>
        </div>
        <div className="detail-cell">
          <dt>{t('home.status.daemon')}</dt>
          <dd data-testid="home-daemon-status">
            {status ? (status.daemon_build ?? t('home.status.notReported')) : t('home.status.unavailable')}
          </dd>
        </div>
      </dl>
      <div className="details-foot">
        <p className="source" data-testid="dictation-selected-id">
          {t('dictation.selectedPipeline', { id: catalog.selectedId ?? t('dictation.modeNone') })}
        </p>
        {captureLimit !== null ? (
          <p className="source" data-testid="dictation-capture-limit">
            {captureMax !== null && captureMax !== captureLimit
              ? t('dictation.captureLimitDeveloper', { seconds: captureLimit, max: captureMax })
              : t('dictation.captureLimit', { seconds: captureLimit })}
          </p>
        ) : null}
        <Button variant="ghost" onClick={() => void refresh()} data-testid="dictation-pipelines-refresh">
          <Icon name="refresh" />
          {t('pipelines.refresh')}
        </Button>
      </div>
    </details>
  );
}
