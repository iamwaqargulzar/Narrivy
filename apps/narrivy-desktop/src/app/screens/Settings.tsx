// Settings: every Narrivy feature in one map (lib/featureMap.ts), grouped
// into sections. Live cards change behaviour now; each feature row carries an
// honest status, so a planned option is visible but never pretends to work.
// A search box finds any setting across sections.
import { useMemo, useState } from 'react';
import { Button } from '../../ui/primitives/Button';
import { DisclosureSection } from '../../ui/primitives/DisclosureSection';
import { Icon } from '../../ui/icons';
import { t } from '../../i18n';
import {
  FEATURE_SECTIONS,
  type FeatureItem,
  type FeatureSection,
  type SectionCard,
} from '../../lib/featureMap';
import { useDaemonStatus } from '../../state/StatusContext';
import { useErrorHandler } from '../../ui/primitives/Toast';
import { bridge, type PortalPermissionOutcome, type StatusSnapshot } from '../../lib/bridge';
import { ThemeCard } from '../components/SettingsCompute/ThemeCard';
import { EchoCancelCard } from '../components/SettingsCompute/EchoCancelCard';
import { VoiceCard } from '../components/SettingsCompute/VoiceCard';
import { StartupCard } from '../components/SettingsCompute/StartupCard';
import { StartShortcutCard } from '../components/SettingsCompute/StartShortcutCard';
import { HoldKeyCard } from '../components/SettingsCompute/HoldKeyCard';
import { CancelShortcutCard } from '../components/SettingsCompute/CancelShortcutCard';
import { OverlayPositionCard } from '../components/SettingsCompute/OverlayPositionCard';
import { AutomationCard } from '../components/SettingsCompute/AutomationCard';
import { ComputeCard } from '../components/SettingsCompute/ComputeCard';
import { TextDeliveryCard } from '../components/SettingsCompute/TextDeliveryCard';
import { RecognitionSettingsCard } from '../components/SettingsCompute/RecognitionSettingsCard';
import { PipelinesIoButtons } from '../components/PipelinesIo';
import { ComputePriorityCard, InsertionChainCard } from '../components/SettingsCompute/LadderCards';
import '../components/SettingsCompute/SettingsCompute.css';

function PortalPermissionSection() {
  const { status, reload } = useDaemonStatus();
  const report = useErrorHandler();
  const [working, setWorking] = useState(false);
  const [outcome, setOutcome] = useState<PortalPermissionOutcome | null>(null);
  const portal = status?.portal_keyboard;
  const active = Boolean(portal?.permission_active);
  const canRequest = Boolean(portal?.can_request) && !active;

  const requestPermission = async () => {
    setWorking(true);
    try {
      const result = await bridge.requestPortalKeyboardPermission();
      setOutcome(result.outcome);
      await reload();
    } catch (err) {
      report(err, { retry: () => void requestPermission() });
    } finally {
      setWorking(false);
    }
  };

  const cancelRequest = async () => {
    try {
      await bridge.cancelPortalKeyboardPermission();
    } catch (err) {
      report(err, { retry: () => void cancelRequest() });
    }
  };

  const visibleOutcome = outcome ?? (active ? 'permission_active' : null);
  return (
    <div className="settings-card" data-testid="portal-permission">
      <div className="settings-card-header">{t('settings.portal.title')}</div>
      <p>{t('settings.portal.body')}</p>
      {working ? (
        <Button onClick={() => void cancelRequest()}>{t('settings.portal.cancel')}</Button>
      ) : (
        <Button onClick={() => void requestPermission()} disabled={!canRequest}>
          {t('settings.portal.request')}
        </Button>
      )}
      {!portal || (!canRequest && !active && !visibleOutcome) ? (
        <p className="source">{t('settings.portal.unavailable')}</p>
      ) : null}
      {visibleOutcome ? (
        <p className="setting-resolved" role="status">
          {t(`settings.portal.${visibleOutcome}`)}
        </p>
      ) : null}
    </div>
  );
}

function Card({ card }: { card: SectionCard }) {
  switch (card) {
    case 'theme':
      return <ThemeCard />;
    case 'echoCancel':
      return <EchoCancelCard />;
    case 'voice':
      return <VoiceCard />;
    case 'startup':
      return <StartupCard />;
    case 'start':
      return <StartShortcutCard />;
    case 'hold':
      return <HoldKeyCard />;
    case 'cancel':
      return <CancelShortcutCard />;
    case 'recognition':
      return <RecognitionSettingsCard />;
    case 'automation':
      return <AutomationCard />;
    case 'compute':
      return <ComputeCard />;
    case 'delivery':
      return <TextDeliveryCard />;
    case 'portal':
      return <PortalPermissionSection />;
    case 'priority':
      return <ComputePriorityCard />;
    case 'chain':
      return <InsertionChainCard />;
    case 'overlayPosition':
      return <OverlayPositionCard />;
    case 'pipelines':
      return (
        <DisclosureSection title={t('settings.pipelines.title')} defaultOpen={false}>
          <div className="page-head-actions">
            <PipelinesIoButtons />
          </div>
        </DisclosureSection>
      );
  }
}

function text(value: string | null): string {
  if (value === null) return t('fm.value.notReported');
  return value.startsWith('fm.') ? t(value) : value;
}

function StatusBadge({ item }: { item: FeatureItem }) {
  switch (item.status) {
    case 'live':
      return <span className="chip chip-ok fm-badge">{t('fm.status.live')}</span>;
    case 'reported':
      return <span className="chip fm-badge">{t('fm.status.reported')}</span>;
    case 'planned':
      return (
        <span className="chip chip-warn fm-badge" title={t('fm.status.plannedTitle', { task: item.task ?? '' })}>
          {t('fm.status.planned', { task: item.task ?? '' })}
        </span>
      );
    case 'proposed':
      return (
        <span className="chip fm-badge fm-badge-proposed" title={t('fm.status.proposedTitle')}>
          {t('fm.status.proposed')}
        </span>
      );
  }
}

function Control({ item, label, status }: { item: FeatureItem; label: string; status: StatusSnapshot | null }) {
  const locked = item.status === 'planned' || item.status === 'proposed';
  const control = item.control;
  switch (control.kind) {
    case 'toggle':
      return (
        <button
          type="button"
          role="switch"
          className="switch"
          aria-checked={control.on}
          aria-label={label}
          disabled={locked}
        >
          <span className="switch-thumb" />
        </button>
      );
    case 'select':
      return (
        <select aria-label={label} disabled={locked} value="default" onChange={() => undefined}>
          <option value="default">{t(control.valueKey)}</option>
        </select>
      );
    case 'number':
      return (
        <span className="fm-number">
          <input
            type="number"
            aria-label={label}
            disabled={locked}
            min={control.min}
            max={control.max}
            value={control.value}
            onChange={() => undefined}
          />
          {control.unitKey ? <span className="source">{t(control.unitKey)}</span> : null}
        </span>
      );
    case 'keys':
      return control.keys.length > 0 ? (
        <span className="fm-keys">
          {control.keys.map((key) => (
            <kbd key={key}>{key}</kbd>
          ))}
        </span>
      ) : (
        <span className="fm-value fm-muted">{t('fm.value.notSet')}</span>
      );
    case 'value': {
      const value = control.read(status);
      const isCommand = value?.includes('narrivy-cli') ?? false;
      return <span className={`fm-value${value === null ? ' fm-muted' : ''}${isCommand ? ' mono fm-command' : ''}`}>{text(value)}</span>;
    }
    case 'none':
      return (
        <Button disabled={locked} aria-label={label}>
          {t('fm.action.manage')}
        </Button>
      );
  }
}

function FeatureRow({ section, item, status }: { section: string; item: FeatureItem; status: StatusSnapshot | null }) {
  const title = t(`fm.${section}.${item.id}.title`);
  return (
    <div className="fm-row" data-feature={`${section}.${item.id}`} data-status={item.status}>
      <div className="fm-text">
        <div className="fm-title">
          {title}
          <StatusBadge item={item} />
        </div>
        <p className="fm-desc">{t(`fm.${section}.${item.id}.desc`)}</p>
      </div>
      <div className="fm-control">
        <Control item={item} label={title} status={status} />
      </div>
    </div>
  );
}

function SectionBody({ section, status }: { section: FeatureSection; status: StatusSnapshot | null }) {
  return (
    <div className="settings-stack">
      <div className="fm-section-head">
        <h3 className="fm-section-title">{t(`fm.${section.id}.title`)}</h3>
        <p className="page-lead">{t(`fm.${section.id}.desc`)}</p>
      </div>
      {section.cards.map((card) => (
        <Card key={card} card={card} />
      ))}
      <div className="settings-card fm-list">
        {section.items.map((item) => (
          <FeatureRow key={item.id} section={section.id} item={item} status={status} />
        ))}
      </div>
    </div>
  );
}

function matches(section: FeatureSection, item: FeatureItem, query: string): boolean {
  const haystack = [
    t(`fm.${section.id}.title`),
    t(`fm.${section.id}.${item.id}.title`),
    t(`fm.${section.id}.${item.id}.desc`),
  ]
    .join(' ')
    .toLowerCase();
  return query
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean)
    .every((word) => haystack.includes(word));
}

export function SettingsScreen() {
  const { status } = useDaemonStatus();
  const [active, setActive] = useState(FEATURE_SECTIONS[0].id);
  const [query, setQuery] = useState('');
  const current = FEATURE_SECTIONS.find((s) => s.id === active) ?? FEATURE_SECTIONS[0];
  const results = useMemo(
    () =>
      query.trim().length === 0
        ? []
        : FEATURE_SECTIONS.flatMap((section) =>
            section.items.filter((item) => matches(section, item, query)).map((item) => ({ section, item })),
          ),
    [query],
  );
  const counts = useMemo(() => {
    const all = FEATURE_SECTIONS.flatMap((s) => s.items);
    return {
      live: all.filter((i) => i.status === 'live' || i.status === 'reported').length,
      planned: all.filter((i) => i.status === 'planned').length,
      total: all.length,
    };
  }, []);

  return (
    <div className="settings-page">
      <aside className="settings-side" aria-label={t('settings.sections')}>
        <label className="settings-search">
          <Icon name="search" />
          <input
            type="search"
            aria-label={t('settings.search')}
            placeholder={t('settings.searchPlaceholder', { count: counts.total })}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
        </label>
        <div className="settings-tabs" role="tablist" aria-label={t('settings.sections')} aria-orientation="vertical">
          {FEATURE_SECTIONS.map((section) => (
            <button
              key={section.id}
              type="button"
              role="tab"
              aria-selected={query.trim() === '' && active === section.id}
              aria-controls={`settings-panel-${section.id}`}
              id={`settings-tab-${section.id}`}
              className="settings-tab"
              onClick={() => {
                setQuery('');
                setActive(section.id);
              }}
            >
              <Icon name={section.icon} />
              {t(`fm.${section.id}.title`)}
            </button>
          ))}
        </div>
        <p className="settings-tally source">
          {t('settings.tally', { live: counts.live, planned: counts.planned })}
        </p>
      </aside>
      <div className="settings-main">
        <div className="page-wide">
          <div className="page-head">
            <h2 className="page-title">{t('nav.settings')}</h2>
            <p className="page-lead">{t('fm.legend')}</p>
          </div>
          {query.trim() !== '' ? (
            <div className="settings-panel" role="region" aria-label={t('settings.results')}>
              {results.length === 0 ? (
                <div className="panel empty-state">
                  <Icon name="search" />
                  <strong>{t('settings.noResults')}</strong>
                </div>
              ) : (
                <div className="settings-card fm-list">
                  {results.map(({ section, item }) => (
                    <div key={`${section.id}.${item.id}`}>
                      <button
                        type="button"
                        className="fm-crumb"
                        onClick={() => {
                          setQuery('');
                          setActive(section.id);
                        }}
                      >
                        {t(`fm.${section.id}.title`)}
                        <Icon name="arrowRight" />
                      </button>
                      <FeatureRow section={section.id} item={item} status={status} />
                    </div>
                  ))}
                </div>
              )}
            </div>
          ) : (
            <div
              id={`settings-panel-${current.id}`}
              role="tabpanel"
              aria-labelledby={`settings-tab-${current.id}`}
              className="settings-panel"
            >
              <SectionBody key={current.id} section={current} status={status} />
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
