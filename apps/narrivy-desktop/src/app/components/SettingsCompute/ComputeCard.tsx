// Compute: what the daemon reports, and the reason no control here can change
// it. The resolved line is the same resolveCompute() value Home renders, so the
// app cannot show two different compute claims. No device, model or runtime name
// is written in this file; the inventory comes from StatusDto.capabilities.
import { t } from '../../../i18n';
import { resolveCompute } from '../../../lib/resolved';
import type { StatusSnapshot } from '../../../lib/bridge';
import { useDaemonStatus } from '../../../state/StatusContext';
import { citedCapability, computeTokens, controlReporting } from './capabilityInventory';
import { ReportedTokens, UnavailableSwitch } from './ReportedTokens';
import './SettingsCompute.css';

const NO_STATUS: StatusSnapshot = { capabilities: [], active_session: null };

const DEVICE_REASON_ID = 'compute-device-reason';
const COMPUTE_TOKENS_ID = 'compute-reported-tokens';
const VULKAN_SUBJECT = /vulkan/i;
const FALLBACK_SUBJECT = /fallback/i;

function ChipIcon(props: React.SVGProps<SVGSVGElement>) {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.7} aria-hidden="true" {...props}>
      <rect x="7" y="7" width="10" height="10" rx="2" />
      <path d="M11 3v3M13 3v3M11 18v3M13 18v3M3 11h3M3 13h3M18 11h3M18 13h3" />
      <circle cx="12" cy="12" r="1" fill="currentColor" />
    </svg>
  );
}

function CheckIcon(props: React.SVGProps<SVGSVGElement>) {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} aria-hidden="true" {...props}>
      <path d="M9 12l2 2 4-4" />
      <circle cx="12" cy="12" r="9" />
    </svg>
  );
}

export function ComputeCard() {
  const { status } = useDaemonStatus();
  const resolved = resolveCompute(status ?? NO_STATUS, 'auto');
  const cited = citedCapability(resolved.source);
  const tokens = computeTokens(status);
  const vulkan = controlReporting(status, VULKAN_SUBJECT);
  const fallback = controlReporting(status, FALLBACK_SUBJECT);

  return (
    <section className="settings-card" aria-labelledby="compute-heading" data-testid="compute-card">
      <div className="settings-card-header" id="compute-heading">
        <ChipIcon />
        {t('settings.compute.title')}
      </div>
      <p className="settings-card-desc">{t('settings.compute.description')}</p>
      <div className="compute-grid">
        <div>
          <div className="compute-section-title">{t('settings.compute.deviceSelection')}</div>
          <div
            className="radio-group"
            role="radiogroup"
            aria-label={t('settings.compute.deviceSelection')}
            aria-describedby={DEVICE_REASON_ID}
          >
            <label className="radio-option">
              <input
                type="radio"
                name="compute-mode"
                value="automatic"
                checked
                readOnly
                disabled
                aria-disabled="true"
                aria-label={t('settings.compute.deviceAutomatic')}
              />
              <span>{t('settings.compute.deviceAutomatic')}</span>
            </label>
            <label className="radio-option">
              <input
                type="radio"
                name="compute-mode"
                value="manual"
                checked={false}
                readOnly
                disabled
                aria-disabled="true"
                aria-label={t('settings.compute.deviceManual')}
              />
              <span>{t('settings.compute.deviceManual')}</span>
            </label>
          </div>
          <p className="control-reason" id={DEVICE_REASON_ID} data-testid="compute-device-reason">
            {t('settings.compute.deviceUnavailable')}
          </p>
        </div>
        <div>
          <div className="compute-section-title">{t('settings.compute.reportedCapabilities')}</div>
          <ReportedTokens
            id={COMPUTE_TOKENS_ID}
            label={t('settings.compute.reportedCapabilities')}
            tokens={tokens}
            cited={cited}
          />
        </div>
        <div>
          <div className="compute-section-title">{t('settings.compute.acceleration')}</div>
          <div className="compute-toggles">
            <UnavailableSwitch
              id="compute-vulkan"
              label={t('settings.compute.useVulkan')}
              checked={false}
              reason={vulkan.reason}
            />
            <UnavailableSwitch
              id="compute-auto-fallback"
              label={t('settings.compute.autoFallback')}
              checked={false}
              reason={fallback.reason}
            />
          </div>
        </div>
      </div>
      <div className="resolved-line" role="status" aria-live="polite">
        <CheckIcon />
        <span data-testid="compute-resolved">{t('settings.compute.currentResolved', { value: resolved.resolved })}</span>
      </div>
      <p className="control-reason" data-testid="compute-source">
        {t('settings.compute.derivedFrom', { source: resolved.source })}
      </p>
    </section>
  );
}
