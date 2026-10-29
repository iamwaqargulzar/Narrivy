// Models: the speech models Narrivy knows about, which one is loaded, which
// are on this computer, which are published, and where recognition runs. The
// list and each model's state come from the daemon (`ListModels`); against an
// older daemon the screen falls back to the single reported model. Nothing is
// claimed that the daemon did not report.
import { BundledModelSetup } from '../components/BundledModelSetup';

/** The model the package ships (`/usr/share/narrivy/models`). */
const BUNDLED_MODEL_ID = 'whisper-tiny.en-Q8_0';
import { useEffect, useState } from 'react';
import { Button } from '../../ui/primitives/Button';
import { Icon, type IconName } from '../../ui/icons';
import { t } from '../../i18n';
import { useDaemonStatus } from '../../state/StatusContext';
import { useErrorHandler } from '../../ui/primitives/Toast';
import { bridge, type ModelInfo, type ModelProvisionStatus } from '../../lib/bridge';
import { formatBytes } from '../../lib/overview';
import { languageSummary, modelName, quantBits, usableModels } from '../../lib/modelFacts';
import { useNav } from '../../state/NavContext';
import { LanguageModelsPanel } from './LanguageModels';
import { TextToSpeechPanel } from './TextToSpeech';
import '../components/SettingsCompute/SettingsCompute.css';

/** `ListModels` needs a daemon at protocol minor 13 or newer. */
const MODELS_PROTOCOL_MINOR = 13;
/** How often a running download's progress is read. */
const DOWNLOAD_POLL_MS = 1000;
/** Reasons the service reports for a download that did not finish. */
const DOWNLOAD_REASONS = new Set([
  'network_unreachable',
  'stalled',
  'http_error',
  'tls_failed',
  'redirect_refused',
  'integrity_mismatch',
  'too_large',
  'disk_write_failed',
  'downloader_unavailable',
  'models_directory_unsafe',
  'download_failed',
  'worker_unavailable',
]);

interface DownloadActions {
  /** Another download is running, so this one cannot start. */
  busy: boolean;
  start: (model: ModelInfo) => void;
  cancel: (model: ModelInfo) => void;
  /** Open the Store, where a model's add-on runtime is installed. */
  openStore: () => void;
  /** Delete an installed model pack; the service refuses one in use. */
  remove: (model: ModelInfo) => void;
}


/** Progress of a running or unfinished download, as the service reports it. */
function DownloadProgress({ model }: { model: ModelInfo }) {
  const received = model.downloaded_bytes ?? 0;
  const percent = model.size_bytes > 0 ? Math.min(100, Math.floor((received / model.size_bytes) * 100)) : 0;
  return (
    <div className="model-progress">
      <div
        className="model-progress-bar"
        role="progressbar"
        aria-label={t('models.downloadProgress', { name: modelName(model) })}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={percent}
      >
        <span style={{ width: `${percent}%` }} />
      </div>
      <span className="model-progress-text">
        {t('models.downloadedOf', { done: formatBytes(received) ?? '0 B', total: formatBytes(model.size_bytes) ?? '—', percent })}
      </span>
    </div>
  );
}

function ModelCard({ model, downloads }: { model: ModelInfo; downloads: DownloadActions }) {
  const active = model.state === 'active';
  const [confirming, setConfirming] = useState(false);
  const removable = model.state === 'installed' && model.removable === true;
  const bits = quantBits(model.quant);
  return (
    <li className="model-row" data-state={model.state} data-testid={`model-${model.id}`}>
      <div className="model-row-main">
        <div className="model-row-title">
          <span className="model-row-name">{modelName(model)}</span>
          {model.quant ? (
            <span className="model-quant" title={t('models.quantTitle')}>
              {model.quant}
              {bits ? <em>{bits}</em> : null}
            </span>
          ) : null}
          {active ? (
            <span className="chip chip-ok">
              <Icon name="check" />
              {t('models.inUse')}
            </span>
          ) : null}
        </div>
        <div className="model-facts">
          {model.languages.length ? (
            <span className="model-fact">
              <Icon name="globe" />
              {languageSummary(model)}
            </span>
          ) : null}
          <span className="model-fact" data-on={model.streaming ? 'true' : 'false'}>
            <Icon name="wave" />
            {model.streaming ? t('models.tag.streaming') : t('models.tag.finalOnly')}
          </span>
          {model.devices && model.devices.length ? (
            <span className="model-fact" title={t('models.devicesTitle')}>
              <Icon name="cpu" />
              {model.devices.join(' · ')}
            </span>
          ) : null}
          {model.family ? (
            <span className="model-fact" data-on={model.qualified ? 'true' : 'false'} title={t('models.qualifiedTitle')}>
              <Icon name={model.qualified ? 'checkCircle' : 'info'} />
              {model.qualified ? t('models.qualified') : t('models.notQualified')}
            </span>
          ) : null}
        </div>
        {model.state === 'downloading' || (model.state === 'available' && model.downloaded_bytes) ? (
          <DownloadProgress model={model} />
        ) : null}
        {model.source_repo ? (
          <p className="model-source">
            {t('models.source', { repo: model.source_repo, revision: model.source_revision.slice(0, 12), license: model.license })}
            {' · '}
            {t('models.engine', { engine: model.engine })}
          </p>
        ) : null}
      </div>
      <div className="model-row-side">
        <span className="model-size">
          <Icon name="drive" />
          {formatBytes(model.size_bytes)}
        </span>
        {model.state === 'loading' ? <span className="chip">{t('models.loading')}</span> : null}
        {model.state === 'downloading' ? (
          <span className="model-row-action">
            <Button onClick={() => downloads.cancel(model)}>{t('models.downloadCancel')}</Button>
          </span>
        ) : null}
        {model.state === 'available' && model.downloadable ? (
          <span className="model-row-action">
            <Button variant="primary" disabled={downloads.busy} onClick={() => downloads.start(model)}>
              <Icon name="download" />
              {model.downloaded_bytes ? t('models.downloadResume') : t('models.download')}
            </Button>
          </span>
        ) : null}
        {model.state === 'available' && !model.downloadable ? (
          <span className="chip" title={t('models.notDownloadableTitle')}>
            {t('models.notDownloadable')}
          </span>
        ) : null}
        {removable && !confirming ? (
          <span className="model-row-action">
            <Button onClick={() => setConfirming(true)}>{t('store.remove')}</Button>
          </span>
        ) : null}
        {removable && confirming ? (
          <span className="model-row-action store-confirm" role="group" aria-label={t('store.removeConfirm', { name: modelName(model) })}>
            <span>{t('store.removeConfirm', { name: modelName(model) })}</span>
            <Button className="btn-danger" onClick={() => { setConfirming(false); downloads.remove(model); }}>
              {t('store.removeYes')}
            </Button>
            <Button onClick={() => setConfirming(false)}>{t('store.removeNo')}</Button>
          </span>
        ) : null}
        {model.state === 'needs_engine' ? (
          <span className="model-row-action">
            <Button onClick={downloads.openStore}>
              <Icon name="store" />
              {t('models.getAddOn')}
            </Button>
          </span>
        ) : null}
      </div>
    </li>
  );
}

function ModelSection({ title, hint, models, downloads }: { title: string; hint: string; models: ModelInfo[]; downloads: DownloadActions }) {
  if (models.length === 0) return null;
  return (
    <section className="model-section">
      <div className="model-section-head">
        <h3>{title}</h3>
        <span className="model-count">{models.length}</span>
        <span className="model-section-hint">{hint}</span>
      </div>
      <ul className="model-rows">
        {models.map((model) => (
          <ModelCard key={model.id} model={model} downloads={downloads} />
        ))}
      </ul>
    </section>
  );
}

type LanguageFilter = 'all' | 'english' | 'multilingual';
type DeviceFilter = 'all' | 'CPU' | 'GPU' | 'NPU';
type TextFilter = 'all' | 'live' | 'final';
type SortOrder = 'recommended' | 'name' | 'smallest' | 'largest';

interface ModelFilters {
  query: string;
  language: LanguageFilter;
  device: DeviceFilter;
  text: TextFilter;
  sort: SortOrder;
}

const NO_FILTERS: ModelFilters = { query: '', language: 'all', device: 'all', text: 'all', sort: 'recommended' };

/** Apply the toolbar's search, filters and sort. Pure, so it is tested directly. */
export function filterModels(models: ModelInfo[], filters: ModelFilters): ModelInfo[] {
  const query = filters.query.trim().toLowerCase();
  const kept = models.filter((model) => {
    if (query && !`${model.family} ${model.variant} ${model.quant} ${model.id}`.toLowerCase().includes(query)) return false;
    const multilingual = model.languages.includes('multilingual') || model.languages.length > 1;
    if (filters.language === 'english' && !model.languages.includes('en')) return false;
    if (filters.language === 'multilingual' && !multilingual) return false;
    if (filters.device !== 'all' && !(model.devices ?? []).includes(filters.device)) return false;
    if (filters.text === 'live' && !model.streaming) return false;
    if (filters.text === 'final' && model.streaming) return false;
    return true;
  });
  switch (filters.sort) {
    case 'name':
      return [...kept].sort((a, b) => modelName(a).localeCompare(modelName(b)));
    case 'smallest':
      return [...kept].sort((a, b) => a.size_bytes - b.size_bytes);
    case 'largest':
      return [...kept].sort((a, b) => b.size_bytes - a.size_bytes);
    default:
      // Qualified models first, then the daemon's catalog order.
      return [...kept].sort((a, b) => Number(b.qualified) - Number(a.qualified));
  }
}

function FilterSelect<T extends string>({ label, value, options, onChange }: {
  label: string;
  value: T;
  options: Array<{ value: T; label: string }>;
  onChange: (value: T) => void;
}) {
  return (
    <label className="model-filter">
      <span>{label}</span>
      <select value={value} aria-label={label} onChange={(event) => onChange(event.target.value as T)}>
        {options.map((option) => (
          <option key={option.value} value={option.value}>
            {option.label}
          </option>
        ))}
      </select>
    </label>
  );
}

function ModelToolbar({ filters, onChange, shown, total }: {
  filters: ModelFilters;
  onChange: (filters: ModelFilters) => void;
  shown: number;
  total: number;
}) {
  const changed = JSON.stringify(filters) !== JSON.stringify(NO_FILTERS);
  return (
    <div className="model-toolbar" role="search" aria-label={t('models.toolbar')}>
      <label className="model-search">
        <Icon name="search" />
        <input
          type="search"
          value={filters.query}
          placeholder={t('models.searchPlaceholder')}
          aria-label={t('models.searchPlaceholder')}
          onChange={(event) => onChange({ ...filters, query: event.target.value })}
        />
      </label>
      <div className="model-filters">
        <FilterSelect
          label={t('models.filter.language')}
          value={filters.language}
          onChange={(language) => onChange({ ...filters, language })}
          options={[
            { value: 'all', label: t('models.filter.anyLanguage') },
            { value: 'english', label: t('models.filter.english') },
            { value: 'multilingual', label: t('models.multilingual') },
          ]}
        />
        <FilterSelect
          label={t('models.filter.device')}
          value={filters.device}
          onChange={(device) => onChange({ ...filters, device })}
          options={[
            { value: 'all', label: t('models.filter.anyDevice') },
            { value: 'NPU', label: 'NPU' },
            { value: 'GPU', label: 'GPU' },
            { value: 'CPU', label: 'CPU' },
          ]}
        />
        <FilterSelect
          label={t('models.filter.text')}
          value={filters.text}
          onChange={(text) => onChange({ ...filters, text })}
          options={[
            { value: 'all', label: t('models.filter.anyText') },
            { value: 'live', label: t('models.tag.streaming') },
            { value: 'final', label: t('models.tag.finalOnly') },
          ]}
        />
        <FilterSelect
          label={t('models.filter.sort')}
          value={filters.sort}
          onChange={(sort) => onChange({ ...filters, sort })}
          options={[
            { value: 'recommended', label: t('models.sort.recommended') },
            { value: 'name', label: t('models.sort.name') },
            { value: 'smallest', label: t('models.sort.smallest') },
            { value: 'largest', label: t('models.sort.largest') },
          ]}
        />
      </div>
      <div className="model-toolbar-foot">
        <span role="status">{t('models.showing', { shown, total })}</span>
        {changed ? (
          <button type="button" className="link-action link-quiet" onClick={() => onChange(NO_FILTERS)}>
            {t('models.clearFilters')}
          </button>
        ) : null}
      </div>
    </div>
  );
}

function capValue(caps: string[], prefix: string): string | null {
  const found = caps.find((c) => c.startsWith(prefix));
  return found ? found.slice(prefix.length) : null;
}

const PROVISION_FAILURES = new Set([
  'source_rejected',
  'import_failed',
  'cleanup_failed',
  'published_unconfirmed',
  'binding_failed',
  'activation_failed',
  'shutting_down',
  'worker_failed',
]);

/** The daemon's typed reason a model install failed, in words (F-M11). */
export function provisionFailureText(failure: string | null | undefined): string | null {
  if (!failure) return null;
  return PROVISION_FAILURES.has(failure)
    ? t(`models.installFailure.${failure}`)
    : t('models.installFailure.other', { code: failure });
}

type Tone = '' | ' chip-ok' | ' chip-warn';

function lifecycle(state: string | null | undefined): { label: string; tone: Tone } {
  switch (state) {
    case 'ready':
      return { label: t('models.state.ready'), tone: ' chip-ok' };
    case 'awaiting_model':
      return { label: t('models.state.awaiting'), tone: ' chip-warn' };
    case 'provisioning':
      return { label: t('models.state.installing'), tone: ' chip-warn' };
    case 'activation_failed':
      return { label: t('models.state.failed'), tone: ' chip-warn' };
    case 'published_unconfirmed':
      return { label: t('models.state.unconfirmed'), tone: ' chip-warn' };
    default:
      return { label: t('models.state.notReported'), tone: '' };
  }
}

type ModelKind = 'speech' | 'correction' | 'language' | 'voice';
/** The kinds of model, each a section of the side panel. */
const KINDS: Array<{ id: ModelKind; icon: IconName }> = [
  { id: 'speech', icon: 'mic' },
  { id: 'correction', icon: 'sliders' },
  { id: 'language', icon: 'spark' },
  { id: 'voice', icon: 'speaker' },
];

export function ModelsScreen() {
  const { status, reload } = useDaemonStatus();
  const { navigate } = useNav();
  const report = useErrorHandler();
  const [provision, setProvision] = useState<ModelProvisionStatus | null>(status?.model_provisioning ?? null);
  const caps = status?.capabilities ?? [];
  const hasTestLane = caps.includes('mock.asr');
  const cpu = capValue(caps, 'asr.transcribe-cpp.cpu=');
  const hasGgufLane = caps.includes('asr.transcribe-cpp') || cpu !== null;
  const runtime = capValue(caps, 'asr.transcribe-cpp.runtime=');
  const languages = capValue(caps, 'asr.languages=');
  const streaming = capValue(caps, 'asr.transcribe-cpp.streaming=');
  const installedState = status?.installed_model;
  const provisionState = provision?.state ?? status?.model_provisioning?.state;
  const state = lifecycle(installedState);

  // The model list, read again whenever the daemon's model state changes.
  const [models, setModels] = useState<ModelInfo[] | null>(null);
  const listable = (status?.protocol_minor ?? 0) >= MODELS_PROTOCOL_MINOR;
  const modelArtifact = capValue(caps, 'asr.transcribe-cpp.model-artifact=');
  useEffect(() => {
    if (!listable) {
      setModels(null);
      return;
    }
    let current = true;
    bridge
      .models()
      .then((list) => {
        if (current) setModels(list);
      })
      .catch(() => {
        // Without a listing the screen shows the single reported model.
        if (current) setModels(null);
      });
    return () => {
      current = false;
    };
  }, [listable, installedState, provisionState, modelArtifact]);
  // While a download runs, follow its progress once a second.
  const downloadState = capValue(caps, 'model.download=');
  const downloading = downloadState === 'downloading' || (models ?? []).some((m) => m.state === 'downloading');
  useEffect(() => {
    if (!listable || !downloading) return;
    let current = true;
    const timer = window.setInterval(() => {
      bridge
        .models()
        .then((list) => {
          if (!current) return;
          setModels(list);
          // Refresh the service facts once the download is over.
          if (!list.some((m) => m.state === 'downloading')) void reload().catch(() => {});
        })
        .catch(() => {});
    }, DOWNLOAD_POLL_MS);
    return () => {
      current = false;
      window.clearInterval(timer);
    };
  }, [listable, downloading, reload]);
  const downloadFailed = downloadState?.startsWith('failed:') ? downloadState.slice('failed:'.length) : null;
  const downloads: DownloadActions = {
    busy: downloading,
    start: (model) => {
      bridge
        .downloadModel(model.id)
        .then(() => bridge.models())
        .then((list) => {
          setModels(list);
          return reload();
        })
        .catch((err) => report(err));
    },
    cancel: (model) => {
      bridge
        .cancelModelDownload(model.id)
        .then(() => bridge.models())
        .then(setModels)
        .catch((err) => report(err));
    },
    openStore: () => navigate('store'),
    remove: (model) => {
      bridge
        .removeStoreItem(model.id)
        .then(() => bridge.models())
        .then((list) => {
          setModels(list);
          return reload();
        })
        .catch((err) => report(err));
    },
  };
  const [filters, setFilters] = useState<ModelFilters>(NO_FILTERS);
  const [tab, setTab] = useState<ModelKind>('speech');
  const listed = models !== null && models.length > 0;
  const visible = filterModels(models ?? [], filters);
  const onComputer = usableModels(visible);
  const downloadable = visible.filter(
    (m) => m.state === 'downloading' || (m.state === 'available' && m.downloadable),
  );
  // Listed in the catalog but with no checked download published yet.
  const notYet = visible.filter(
    (m) => m.state === 'available' && !m.downloadable && m.id !== BUNDLED_MODEL_ID,
  );
  const needsEngine = visible.filter((m) => m.state === 'needs_engine');

  // Provisioning progress is refreshed by StatusProvider's bounded freshness
  // read while a daemon-owned operation is active; this screen only reloads
  // explicitly after its own install/cancel actions.
  useEffect(() => {
    if (status?.model_provisioning) setProvision(status.model_provisioning);
  }, [status?.model_provisioning]);

  const install = async () => {
    try {
      setProvision(await bridge.installBundledModel());
      await reload();
    } catch (err) {
      report(err, { retry: () => void install() });
    }
  };

  const cancelInstall = async () => {
    const operationId = provision?.operation_id ?? status?.model_provisioning?.operation_id;
    if (!operationId) return;
    try {
      setProvision(await bridge.cancelModelInstall(operationId));
      await reload();
    } catch (err) {
      report(err, { retry: () => void cancelInstall() });
    }
  };

  return (
    <div className="settings-page">
      <aside className="settings-side" aria-label={t('lm.tabs')}>
        <div className="settings-tabs" role="tablist" aria-label={t('lm.tabs')} aria-orientation="vertical">
          {KINDS.map((kind) => (
            <button
              key={kind.id}
              type="button"
              role="tab"
              aria-selected={tab === kind.id}
              className="settings-tab"
              onClick={() => setTab(kind.id)}
            >
              <Icon name={kind.icon} />
              {t(`lm.tab.${kind.id}`)}
            </button>
          ))}
        </div>
      </aside>
      <div className="settings-main">
        <div className="page-wide">
          <div className="page-head">
            <h2 className="page-title">{t('nav.models')}</h2>
            <p className="page-lead">{t(`lm.lead.${tab}`)}</p>
          </div>
          {tab === 'correction' ? <LanguageModelsPanel roles={['clean_up']} listRole="clean_up" /> : null}
          {tab === 'language' ? <LanguageModelsPanel roles={['assistant']} /> : null}
          {tab === 'voice' ? <TextToSpeechPanel /> : null}
          <div hidden={tab !== 'speech'}>

          {/* Only while no model is in use: once ready the card is gone and
              the model's row says "In use" (it stayed with its button after
              "ready", seen natively). */}
          {installedState === 'awaiting_model' ||
          (provisionState && installedState !== 'ready') ? (
            <BundledModelSetup
              provisionState={provisionState ?? null}
              failureText={provisionFailureText(provision?.failure ?? status?.model_provisioning?.failure)}
              model={(models ?? []).find((m) => m.id === BUNDLED_MODEL_ID) ?? null}
              onInstall={() => void install()}
              onCancel={() => void cancelInstall()}
            />
          ) : null}

          {listed ? (
            <>
              <ModelToolbar filters={filters} onChange={setFilters} shown={visible.length} total={models?.length ?? 0} />
              {visible.length === 0 ? (
                <div className="panel empty-state">
                  <Icon name="search" />
                  <strong>{t('models.noMatchTitle')}</strong>
                  <span className="screen-empty-note">{t('models.noMatch')}</span>
                </div>
              ) : null}
              {downloadFailed ? (
                <p className="model-download-error" role="alert">
                  {t('models.downloadFailed', {
                    reason: t(`models.downloadReason.${DOWNLOAD_REASONS.has(downloadFailed) ? downloadFailed : 'download_failed'}`, {
                      code: downloadFailed,
                    }),
                  })}
                </p>
              ) : null}
              <ModelSection title={t('models.onComputer')} hint={t('models.onComputerHint')} models={onComputer} downloads={downloads} />
              <ModelSection title={t('models.available')} hint={t('models.availableHint')} models={downloadable} downloads={downloads} />
              <ModelSection title={t('models.notYet')} hint={t('models.notYetHint')} models={notYet} downloads={downloads} />
              <ModelSection title={t('models.needsEngine')} hint={t('models.needsEngineHint')} models={needsEngine} downloads={downloads} />
            </>
          ) : null}

          {listed ? null : <h3 className="section-title">{t('models.speechTitle')}</h3>}
          <div className="model-list" hidden={listed}>
            {hasGgufLane ? (
              <div className="panel model-card">
                <span className="model-icon" aria-hidden="true">
                  <Icon name="models" />
                </span>
                <div>
                  <div className="model-name">{t('models.ggufLane')}</div>
                  <p className="model-desc">{t('models.ggufLaneBody')}</p>
                  <div className="model-tags">
                    <span className={`chip${state.tone}`}>{state.label}</span>
                    {runtime ? <span className="chip">{t('models.tag.runtime', { runtime })}</span> : null}
                    {languages ? <span className="chip">{t('models.tag.languages', { languages })}</span> : null}
                    {streaming ? (
                      <span className="chip">
                        {streaming.startsWith('disabled') || streaming.startsWith('unavailable')
                          ? t('models.tag.finalOnly')
                          : t('models.tag.streaming')}
                      </span>
                    ) : null}
                  </div>
                </div>
                <span className="source mono">asr.transcribe-cpp</span>
              </div>
            ) : null}
            {hasTestLane ? (
              <div className="panel model-card">
                <span className="model-icon" aria-hidden="true">
                  <Icon name="spark" />
                </span>
                <div>
                  <div className="model-name">{t('models.testLane')}</div>
                  <p className="model-desc">{t('models.testLaneBody')}</p>
                </div>
                <span className="source mono">mock.asr</span>
              </div>
            ) : null}
            {!hasTestLane && !hasGgufLane ? (
              <div className="panel empty-state">
                <Icon name="models" />
                <strong>{status ? t('models.noLaneTitle') : t('common.loading')}</strong>
                {status ? <span className="screen-empty-note">{t('models.noLane')}</span> : null}
              </div>
            ) : null}
          </div>

          </div>
        </div>
      </div>
    </div>
  );
}
