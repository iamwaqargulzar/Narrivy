// Settings: the settings that apply to the whole app, in a few sections
// (lib/featureMap.ts). Every control here works. What a pipeline does is set
// in Pipelines, models are in the Store.
import { useState } from 'react';
import { Button } from '../../ui/primitives/Button';
import { Icon } from '../../ui/icons';
import { t } from '../../i18n';
import { FEATURE_SECTIONS, type FeatureSection, type SectionCard } from '../../lib/featureMap';
import { useDaemonStatus } from '../../state/StatusContext';
import { useErrorHandler } from '../../ui/primitives/Toast';
import { bridge, type PortalPermissionOutcome } from '../../lib/bridge';
import { ThemeCard } from '../components/SettingsCompute/ThemeCard';
import { StartupCard } from '../components/SettingsCompute/StartupCard';
import { StartShortcutCard } from '../components/SettingsCompute/StartShortcutCard';
import { HoldKeyCard } from '../components/SettingsCompute/HoldKeyCard';
import { CancelShortcutCard } from '../components/SettingsCompute/CancelShortcutCard';
import { OverlayPositionCard } from '../components/SettingsCompute/OverlayPositionCard';
import { ExtensionsCard } from '../components/SettingsCompute/ExtensionsCard';
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
    case 'startup':
      return <StartupCard />;
    case 'start':
      return <StartShortcutCard />;
    case 'hold':
      return <HoldKeyCard />;
    case 'cancel':
      return <CancelShortcutCard />;
    case 'overlayPosition':
      return <OverlayPositionCard />;
    case 'extensions':
      return <ExtensionsCard />;
    case 'portal':
      return <PortalPermissionSection />;
  }
}

function SectionBody({ section }: { section: FeatureSection }) {
  return (
    <div className="settings-stack">
      <div className="fm-section-head">
        <h3 className="fm-section-title">{t(`fm.${section.id}.title`)}</h3>
        <p className="page-lead">{t(`fm.${section.id}.desc`)}</p>
      </div>
      {section.cards.map((card) => (
        <Card key={card} card={card} />
      ))}
    </div>
  );
}

export function SettingsScreen() {
  const { status } = useDaemonStatus();
  const portal = status?.portal_keyboard;
  const offersPortal = Boolean(portal?.can_request || portal?.permission_active);
  const sections = FEATURE_SECTIONS.filter((section) => section.when !== 'portal' || offersPortal);
  const [active, setActive] = useState(sections[0].id);
  const current = sections.find((s) => s.id === active) ?? sections[0];

  return (
    <div className="settings-page">
      <aside className="settings-side" aria-label={t('settings.sections')}>
        <div className="settings-tabs" role="tablist" aria-label={t('settings.sections')} aria-orientation="vertical">
          {sections.map((section) => (
            <button
              key={section.id}
              type="button"
              role="tab"
              aria-selected={active === section.id}
              aria-controls={`settings-panel-${section.id}`}
              id={`settings-tab-${section.id}`}
              className="settings-tab"
              onClick={() => setActive(section.id)}
            >
              <Icon name={section.icon} />
              {t(`fm.${section.id}.title`)}
            </button>
          ))}
        </div>
      </aside>
      <div className="settings-main">
        <div className="page-wide">
          <div className="page-head">
            <h2 className="page-title">{t('nav.settings')}</h2>
          </div>
          <div
            id={`settings-panel-${current.id}`}
            role="tabpanel"
            aria-labelledby={`settings-tab-${current.id}`}
            className="settings-panel"
          >
            <SectionBody key={current.id} section={current} />
          </div>
        </div>
      </div>
    </div>
  );
}
