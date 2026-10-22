// Text Delivery: the lane the daemon reports. There is no method menu or
// fallback chain here, because neither the daemon nor the bridge describes one;
// a list invented by the client would claim rungs that were never advertised.
import { t } from '../../../i18n';
import { resolveDelivery } from '../../../lib/resolved';
import type { StatusSnapshot } from '../../../lib/bridge';
import { useDaemonStatus } from '../../../state/StatusContext';
import {
  advertisedMatching,
  citedCapability,
  controlAvailability,
  controlReporting,
  deliveryTokens,
  reportedOn,
} from './capabilityInventory';
import { ReportedTokens, UnavailableSwitch } from './ReportedTokens';
import './SettingsCompute.css';

const NO_STATUS: StatusSnapshot = { capabilities: [], active_session: null };

const DELIVERY_TOKENS_ID = 'delivery-reported-tokens';
const CLIPBOARD_RESTORE_SUBJECT = /clipboard-restore/;
const CLIPBOARD_WATCH_SUBJECT = /clipboard-(watch|change|detect|monitor)/;
const TRANSCRIPT_RETAIN_SUBJECT = /transcript-(retain|keep)/;

function SendIcon(props: React.SVGProps<SVGSVGElement>) {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.7} aria-hidden="true" {...props}>
      <path d="M22 2L11 13" />
      <path d="M22 2l-7 20-4-9-9-4 20-7z" />
    </svg>
  );
}

export function TextDeliveryCard() {
  const { status } = useDaemonStatus();
  const resolved = resolveDelivery(status ?? NO_STATUS, 'auto');
  const cited = citedCapability(resolved.source);
  const tokens = deliveryTokens(status);

  const restoreTokens = advertisedMatching(status?.capabilities, CLIPBOARD_RESTORE_SUBJECT);
  const restore = controlAvailability(restoreTokens);
  const watch = controlReporting(status, CLIPBOARD_WATCH_SUBJECT);
  const retain = controlReporting(status, TRANSCRIPT_RETAIN_SUBJECT);

  return (
    <section className="settings-card" aria-labelledby="text-delivery-heading" data-testid="text-delivery-card">
      <div className="settings-card-header" id="text-delivery-heading">
        <SendIcon />
        {t('settings.delivery.title')}
      </div>
      <p className="settings-card-desc">{t('settings.delivery.description')}</p>
      <div className="text-delivery-grid">
        <div>
          <div className="compute-section-title">{t('settings.delivery.laneInEffect')}</div>
          <p className="resolved-line resolved-line--plain" role="status" aria-live="polite" data-testid="delivery-resolved">
            {resolved.resolved}
          </p>
          <p className="control-reason" data-testid="delivery-source">
            {t('settings.compute.derivedFrom', { source: resolved.source })}
          </p>
          <div className="compute-section-title">{t('settings.delivery.reportedCapabilities')}</div>
          <ReportedTokens
            id={DELIVERY_TOKENS_ID}
            label={t('settings.delivery.reportedCapabilities')}
            tokens={tokens}
            cited={cited}
          />
        </div>
        <div>
          <div className="compute-section-title">{t('settings.delivery.perAppOverrides')}</div>
          <p className="control-reason" data-testid="delivery-per-app">
            {t('settings.delivery.perAppUnavailable')}
          </p>
          <div className="toggles-stack">
            <UnavailableSwitch
              id="delivery-restore-clipboard"
              label={t('settings.delivery.restoreClipboard')}
              checked={reportedOn(restoreTokens)}
              reason={restore.reason}
            />
            <UnavailableSwitch
              id="delivery-detect-clipboard-changes"
              label={t('settings.delivery.detectClipboardChanges')}
              checked={false}
              reason={watch.reason}
            />
            <UnavailableSwitch
              id="delivery-keep-transcript"
              label={t('settings.delivery.keepTranscript')}
              checked={false}
              reason={retain.reason}
            />
          </div>
        </div>
      </div>
    </section>
  );
}
