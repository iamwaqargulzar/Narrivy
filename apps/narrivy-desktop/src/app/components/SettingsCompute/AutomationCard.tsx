// Automation: the daemon decides how it is driven and reports that in its
// capability list. Configuration mode and optimization have no command behind
// them, so they are rendered unavailable instead of saving a value in this app
// that no process reads.
import { t } from '../../../i18n';
import { useDaemonStatus } from '../../../state/StatusContext';
import { automationTokens, controlAvailability } from './capabilityInventory';
import { ReportedTokens } from './ReportedTokens';
import './SettingsCompute.css';

const MODE_REASON_ID = 'automation-mode-reason';
const OPTIMIZATION_REASON_ID = 'automation-optimization-reason';
const AUTOMATION_TOKENS_ID = 'automation-reported-tokens';
const OPTIMIZATIONS = ['settings.automation.balanced', 'settings.automation.bestAccuracy', 'settings.automation.lowestLatency', 'settings.automation.lowestPower'] as const;

function BotIcon(props: React.SVGProps<SVGSVGElement>) {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.7} aria-hidden="true" {...props}>
      <rect x="5" y="7" width="14" height="10" rx="3" />
      <circle cx="9" cy="12" r="1" fill="currentColor" />
      <circle cx="15" cy="12" r="1" fill="currentColor" />
      <path d="M12 7V5" />
      <path d="M8 17l-1 2M16 17l1 2" />
    </svg>
  );
}

export function AutomationCard() {
  const { status } = useDaemonStatus();
  const tokens = automationTokens(status);
  const unavailable = controlAvailability(tokens);

  return (
    <section className="settings-card" aria-labelledby="automation-heading" data-testid="automation-card">
      <div className="settings-card-header" id="automation-heading">
        <BotIcon />
        {t('settings.automation.title')}
      </div>
      <p className="settings-card-desc">{t('settings.automation.description')}</p>
      <div className="automation-grid">
        <div>
          <div className="compute-section-title">{t('settings.automation.configurationMode')}</div>
          <div
            className="radio-group"
            role="radiogroup"
            aria-label={t('settings.automation.configurationMode')}
            aria-describedby={MODE_REASON_ID}
          >
            <label className="radio-option">
              <input
                type="radio"
                name="automation-mode"
                value="automatic"
                checked
                readOnly
                disabled
                aria-disabled="true"
                aria-label={t('settings.automation.automatic')}
              />
              <span>{t('settings.automation.automatic')}</span>
            </label>
            <label className="radio-option">
              <input
                type="radio"
                name="automation-mode"
                value="manual"
                checked={false}
                readOnly
                disabled
                aria-disabled="true"
                aria-label={t('settings.automation.manual')}
              />
              <span>{t('settings.automation.manual')}</span>
            </label>
          </div>
          <p className="control-reason" id={MODE_REASON_ID} data-testid="automation-mode-reason">
            {unavailable.reason}
          </p>
        </div>
        <div>
          <div className="optimization-label">{t('settings.automation.optimization')}</div>
          <div
            className="pill-row"
            role="group"
            aria-label={t('settings.automation.optimization')}
            aria-describedby={OPTIMIZATION_REASON_ID}
          >
            {OPTIMIZATIONS.map((key) => (
              <button
                key={key}
                type="button"
                className="opt-pill"
                data-testid={`automation-optimization-${key}`}
                aria-pressed="false"
                aria-disabled="true"
                disabled
              >
                {t(key)}
              </button>
            ))}
          </div>
          <p className="control-reason" id={OPTIMIZATION_REASON_ID} data-testid="automation-optimization-reason">
            {unavailable.reason}
          </p>
          <div className="compute-section-title">{t('settings.automation.reportedCapabilities')}</div>
          <ReportedTokens
            id={AUTOMATION_TOKENS_ID}
            label={t('settings.automation.reportedCapabilities')}
            tokens={tokens}
          />
        </div>
      </div>
    </section>
  );
}
