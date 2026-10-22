// Store: what can be added to Narrivy for this computer (D23, D25) —
// accelerators and modules. Speech models live on the Models screen only.
// The list, each item's state and whether it fits this computer all come
// from the service (`ListStore`); this screen decides nothing. Built-in
// items are shown so the user knows they are already there; planned ones so
// they know what is coming. Installing uses the same verified download as
// models: every file is checked against its pinned checksum before use.
import { useCallback, useEffect, useState } from 'react';
import { Button } from '../../ui/primitives/Button';
import { Icon, type IconName } from '../../ui/icons';
import { hasMessage, t } from '../../i18n';
import { useDaemonStatus } from '../../state/StatusContext';
import { useErrorHandler } from '../../ui/primitives/Toast';
import { useNav } from '../../state/NavContext';
import { bridge, type StoreItem } from '../../lib/bridge';
import { formatBytes } from '../../lib/overview';

/** `ListStore` needs a service at protocol minor 17 or newer. */
const STORE_PROTOCOL_MINOR = 17;
const POLL_MS = 1000;
/** Install failures the service reports, each with its own sentence. */
const REASONS = new Set([
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
  'worker_unavailable',
  'local_source_missing',
  'not_published',
]);

function reasonText(code: string): string {
  return REASONS.has(code)
    ? t(`store.reason.${code}`)
    : t('store.reason.other', { code });
}

/** Copy for an item: written for its id, else for its category. */
function about(item: StoreItem): string | null {
  for (const key of [`store.about.${item.id}`, `store.about.${item.category}`]) {
    if (hasMessage(key)) return t(key);
  }
  return null;
}

function iconFor(item: StoreItem): IconName {
  if (item.source === 'built_in') return 'memory';
  if (item.kind === 'runtime') return 'cpu';
  if (item.category === 'refine') return 'edit';
  if (item.category === 'denoise') return 'wave';
  if (item.category === 'browser' || item.category === 'agent_browser') return 'globe';
  if (item.category === 'meetings') return 'mic';
  if (item.category === 'voice') return 'sliders';
  return 'spark';
}

function fitText(item: StoreItem, names: Map<string, string>): string {
  // A reason the service gives that has its own sentence.
  const reason = `store.fit.reason.${item.fit_reason ?? ''}`;
  if (item.fit !== 'needs_runtime' && hasMessage(reason)) return t(reason);
  switch (item.fit) {
    case 'fits':
      return item.fit_reason === 'npu_found' ? t('store.fit.npu') : t('store.fit.cpu');
    case 'needs_runtime':
      return t('store.fit.needsRuntime', { name: names.get(item.requires ?? '') ?? t('store.fit.runtime') });
    case 'does_not_fit':
      return t('store.fit.no');
    default:
      return t('store.fit.unknown');
  }
}

function Progress({ item }: { item: StoreItem }) {
  const done = item.installed_bytes ?? 0;
  const percent = item.size_bytes > 0 ? Math.min(100, Math.floor((done / item.size_bytes) * 100)) : 0;
  return (
    <div className="model-progress">
      <div
        className="model-progress-bar"
        role="progressbar"
        aria-label={t('store.progress', { name: item.name })}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={percent}
      >
        <span style={{ width: `${percent}%` }} />
      </div>
      <span className="model-progress-text">
        {t('models.downloadedOf', { done: formatBytes(done) ?? '0 B', total: formatBytes(item.size_bytes) ?? '—', percent })}
      </span>
    </div>
  );
}

interface Actions {
  busy: boolean;
  install: (item: StoreItem) => void;
  stop: (item: StoreItem) => void;
  remove: (item: StoreItem) => void;
}

/** The verdict and action at the foot of a card. */
function CardAction({ item, actions }: { item: StoreItem; actions: Actions }) {
  const [confirming, setConfirming] = useState(false);
  if (item.source === 'built_in') {
    return (
      <span className="chip chip-ok">
        <Icon name="check" />
        {item.state === 'in_use' ? t('store.inUse') : t('store.included')}
      </span>
    );
  }
  if (item.source === 'planned') {
    return <span className="chip store-soon">{t('store.comingLater')}</span>;
  }
  const installable = item.fit === 'fits' && item.source !== 'unavailable';
  switch (item.state) {
    case 'in_use':
      return (
        <span className="chip chip-ok">
          <Icon name="check" />
          {t('store.inUse')}
        </span>
      );
    case 'installing':
      return <Button onClick={() => actions.stop(item)}>{t('store.stop')}</Button>;
    case 'available':
    case 'failed':
      return (
        <Button
          variant="primary"
          disabled={actions.busy || !installable}
          title={item.source === 'unavailable' ? t('store.unpublished') : undefined}
          onClick={() => actions.install(item)}
        >
          <Icon name="download" />
          {item.state === 'failed' ? t('store.retry') : t('store.install')}
        </Button>
      );
    case 'installed':
      return confirming ? (
        <span className="store-confirm" role="group" aria-label={t('store.removeConfirm', { name: item.name })}>
          <span>{t('store.removeConfirm', { name: item.name })}</span>
          <Button className="btn-danger" onClick={() => { setConfirming(false); actions.remove(item); }}>
            {t('store.removeYes')}
          </Button>
          <Button onClick={() => setConfirming(false)}>{t('store.removeNo')}</Button>
        </span>
      ) : (
        <span className="store-installed">
          <span className="chip chip-ok">
            <Icon name="check" />
            {t('store.installed')}
          </span>
          <Button onClick={() => setConfirming(true)}>{t('store.remove')}</Button>
        </span>
      );
    default:
      return null;
  }
}

function StoreCard({ item, names, actions }: { item: StoreItem; names: Map<string, string>; actions: Actions }) {
  const text = about(item);
  const fits = item.fit === 'fits';
  const showFit = item.source !== 'built_in' && item.fit_reason !== 'planned';
  return (
    <li className="store-card" data-state={item.state} data-source={item.source} data-testid={`store-${item.id}`}>
      <div className="store-card-head">
        <span className="store-card-icon" aria-hidden="true">
          <Icon name={iconFor(item)} />
        </span>
        <div className="store-card-title">
          <span className="store-card-name">{item.name}</span>
          <span className="store-card-meta">
            {[
              item.devices.length ? item.devices.join(' · ') : null,
              item.size_bytes > 0 ? formatBytes(item.size_bytes) : null,
              item.license || null,
            ]
              .filter(Boolean)
              .join('  ·  ')}
          </span>
        </div>
        {item.source === 'local' ? (
          <span className="chip" title={t('store.localTitle')}>
            {t('store.local')}
          </span>
        ) : null}
      </div>
      {text ? <p className="store-card-about">{text}</p> : null}
      {item.state === 'installing' ? <Progress item={item} /> : null}
      {item.state === 'failed' && item.failure ? (
        <p className="model-download-error" role="alert">
          {t('store.failed', { reason: reasonText(item.failure) })}
        </p>
      ) : null}
      <div className="store-card-foot">
        {showFit ? (
          <span className="store-fit" data-fit={item.fit}>
            <Icon name={fits ? 'checkCircle' : 'info'} />
            {fitText(item, names)}
          </span>
        ) : (
          <span />
        )}
        <CardAction item={item} actions={actions} />
      </div>
    </li>
  );
}

function Section({ title, hint, items, names, actions }: {
  title: string;
  hint: string;
  items: StoreItem[];
  names: Map<string, string>;
  actions: Actions;
}) {
  if (items.length === 0) return null;
  return (
    <section className="model-section">
      <div className="model-section-head">
        <h3>{title}</h3>
        <span className="model-section-hint">{hint}</span>
      </div>
      <ul className="store-grid">
        {items.map((item) => (
          <StoreCard key={item.id} item={item} names={names} actions={actions} />
        ))}
      </ul>
    </section>
  );
}

export function StoreScreen() {
  const { status, reload } = useDaemonStatus();
  const { navigate } = useNav();
  const report = useErrorHandler();
  const [items, setItems] = useState<StoreItem[] | null>(null);
  const supported = (status?.protocol_minor ?? 0) >= STORE_PROTOCOL_MINOR;

  const load = useCallback(async () => {
    try {
      setItems(await bridge.storeItems());
    } catch (err) {
      setItems([]);
      report(err, { retry: () => void load() });
    }
  }, [report]);

  useEffect(() => {
    if (supported) void load();
  }, [supported, load]);

  // While something installs, follow its progress, then refresh the service
  // facts once it is done.
  const installing = (items ?? []).some((item) => item.state === 'installing');
  useEffect(() => {
    if (!installing) return;
    let current = true;
    const timer = window.setInterval(() => {
      bridge
        .storeItems()
        .then((list) => {
          if (!current) return;
          setItems(list);
          if (!list.some((item) => item.state === 'installing')) void reload().catch(() => {});
        })
        .catch(() => {});
    }, POLL_MS);
    return () => {
      current = false;
      window.clearInterval(timer);
    };
  }, [installing, reload]);

  const after = (action: Promise<void>) =>
    action
      .then(() => bridge.storeItems())
      .then((list) => {
        setItems(list);
        return reload();
      })
      .catch((err) => report(err));

  const actions: Actions = {
    busy: installing,
    install: (item) => void after(bridge.downloadModel(item.id)),
    stop: (item) => void after(bridge.cancelModelDownload(item.id)),
    remove: (item) => void after(bridge.removeStoreItem(item.id)),
  };

  // Speech models belong on the Models screen; an older service may still
  // list them here, so they are left out.
  const list = (items ?? []).filter((item) => item.kind !== 'model');
  const names = new Map(list.map((item) => [item.id, item.name]));
  const accelerators = list.filter((item) => item.kind === 'runtime');
  const modules = list.filter((item) => item.kind === 'module');
  const refine = modules.filter((item) => item.category === 'refine');
  const denoise = modules.filter((item) => item.category === 'denoise');
  const otherModules = modules.filter((item) => item.category !== 'refine' && item.category !== 'denoise');
  const extensions = list.filter((item) => item.kind === 'extension');

  return (
    <div className="page-wide">
      <div className="page-head">
        <h2 className="page-title">{t('nav.store')}</h2>
        <p className="page-lead">{t('store.lead')}</p>
      </div>
      <div className="store-notes">
        <p className="store-trust">
          <Icon name="shield" />
          {t('store.trust')}
        </p>
        <button type="button" className="store-models-link" onClick={() => navigate('models')}>
          <Icon name="models" />
          {t('store.modelsElsewhere')}
          <Icon name="arrowRight" />
        </button>
      </div>
      {!supported ? (
        <div className="panel empty-state">
          <Icon name="store" />
          <strong>{status ? t('store.olderService') : t('common.loading')}</strong>
        </div>
      ) : null}
      {supported && items !== null && list.length === 0 ? (
        <div className="panel empty-state">
          <Icon name="store" />
          <strong>{t('store.emptyTitle')}</strong>
          <span className="screen-empty-note">{t('store.empty')}</span>
        </div>
      ) : null}
      <Section title={t('store.accelerators')} hint={t('store.acceleratorsHint')} items={accelerators} names={names} actions={actions} />
      <Section title={t('store.refine')} hint={t('store.refineHint')} items={refine} names={names} actions={actions} />
      <Section title={t('store.denoise')} hint={t('store.denoiseHint')} items={denoise} names={names} actions={actions} />
      <Section title={t('store.modules')} hint={t('store.modulesHint')} items={otherModules} names={names} actions={actions} />
      <Section title={t('store.extensions')} hint={t('store.extensionsHint')} items={extensions} names={names} actions={actions} />
    </div>
  );
}
