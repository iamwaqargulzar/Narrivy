// Read-only projections of the daemon capability report plus the one control
// shape this tab is allowed to render: a switch that states it is unavailable
// and why. Nothing here writes a value, because no command accepts one.
import { t } from '../../../i18n';
import type { AdvertisedToken } from './capabilityInventory';
import './SettingsCompute.css';

export function ReportedTokens({
  id,
  label,
  tokens,
  cited,
}: {
  id: string;
  label: string;
  tokens: AdvertisedToken[];
  cited?: string | null;
}) {
  if (tokens.length === 0) {
    return (
      <p className="control-reason" data-testid={`${id}-empty`}>
        {t('settings.compute.noCapabilitiesReported')}
      </p>
    );
  }
  return (
    <ol className="token-list" id={id} aria-label={label}>
      {tokens.map((token) => (
        <li
          key={token.raw}
          className="token-item"
          data-cited={token.raw === cited ? 'true' : undefined}
          data-testid={`${id}-item`}
        >
          <span className="token-name">{token.raw}</span>
          {token.raw === cited ? (
            <span className="token-cited">{t('settings.compute.citedCapability')}</span>
          ) : null}
        </li>
      ))}
    </ol>
  );
}

export function UnavailableSwitch({
  id,
  label,
  checked,
  reason,
}: {
  id: string;
  label: string;
  checked: boolean;
  reason: string;
}) {
  return (
    <div className="toggle-field">
      <div className="toggle-row">
        <span className="toggle-row-label" id={`${id}-label`}>
          {label}
        </span>
        <button
          type="button"
          role="switch"
          className="switch"
          data-testid={id}
          aria-checked={checked}
          aria-labelledby={`${id}-label`}
          aria-describedby={`${id}-reason`}
          aria-disabled="true"
          disabled
        >
          <span className="switch-thumb" />
        </button>
      </div>
      <p className="control-reason" id={`${id}-reason`}>
        {reason}
      </p>
    </div>
  );
}
