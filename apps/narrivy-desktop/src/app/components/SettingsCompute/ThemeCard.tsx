import { AccentPicker } from '../../../ui/AccentPicker';
import { t } from '../../../i18n';
import { useTheme } from '../../../theme/ThemeProvider';
import './SettingsCompute.css';

function MonitorIcon(props: React.SVGProps<SVGSVGElement>) {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.7} aria-hidden="true" {...props}>
      <rect x="3" y="4" width="18" height="12" rx="2" />
      <path d="M8 20h8" />
      <path d="M12 16v4" />
    </svg>
  );
}
function SunIcon(props: React.SVGProps<SVGSVGElement>) {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.7} aria-hidden="true" {...props}>
      <circle cx="12" cy="12" r="4" />
      <path d="M12 2v2M12 20v2M4.93 4.93l1.42 1.42M17.66 17.66l1.42 1.42M2 12h2M20 12h2M6.34 17.66l-1.42 1.42M19.07 4.93l-1.42 1.42" />
    </svg>
  );
}
function MoonIcon(props: React.SVGProps<SVGSVGElement>) {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.7} aria-hidden="true" {...props}>
      <path d="M21 12.79A9 9 0 1 1 11.21 3a7 7 0 0 0 9.79 9.79z" />
    </svg>
  );
}
function GearIcon(props: React.SVGProps<SVGSVGElement>) {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.7} aria-hidden="true" {...props}>
      <path d="M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6z" />
      <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09a1.65 1.65 0 0 0-1-1.51 1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 9 15a1.65 1.65 0 0 0-1-1.51A1.65 1.65 0 0 0 6.18 13l-.06-.06A2 2 0 1 1 8.95 10.11l.06.06A1.65 1.65 0 0 0 10.83 10a1.65 1.65 0 0 0 1-1.51V8a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06A2 2 0 1 1 21.44 11.1l-.06.06A1.65 1.65 0 0 0 19.4 13c.09.3.14.62.14.96 0 .35-.05.68-.14 1.04z" />
    </svg>
  );
}

export function ThemeCard() {
  const { theme, setTheme, showGreeting, setShowGreeting } = useTheme();
  const options: Array<{ id: 'system' | 'light' | 'dark'; label: string; hint: string; Icon: typeof MonitorIcon }> = [
    { id: 'system', label: t('settings.theme.system'), hint: t('settings.theme.systemHint'), Icon: MonitorIcon },
    { id: 'light', label: t('settings.theme.light'), hint: t('settings.theme.lightHint'), Icon: SunIcon },
    { id: 'dark', label: t('settings.theme.dark'), hint: t('settings.theme.darkHint'), Icon: MoonIcon },
  ];
  return (
    <section className="settings-card" aria-labelledby="theme-heading">
      <div className="settings-card-header" id="theme-heading">
        <GearIcon />
        {t('settings.theme.title')}
      </div>
      <p className="settings-card-desc">{t('settings.theme.desc')}</p>
      <div className="theme-row" role="group" aria-label={t('settings.theme.title')}>
        {options.map((opt) => (
          <button
            key={opt.id}
            type="button"
            className="theme-option"
            aria-pressed={theme === opt.id}
            aria-label={t('settings.theme.optionAria', { label: opt.label, hint: opt.hint })}
            onClick={() => setTheme(opt.id)}
          >
            <opt.Icon className="theme-option-icon" />
            <span className="theme-option-label">{opt.label}</span>
            <span className="theme-option-hint">{opt.hint}</span>
          </button>
        ))}
      </div>
      <div className="accent-row">
        <span className="accent-row-label">{t('accent.label')}</span>
        <AccentPicker size="large" />
      </div>
      <div className="accent-row">
        <span className="accent-row-text">
          <span className="accent-row-label">{t('greeting.label')}</span>
          <span className="accent-row-hint">{t('greeting.hint')}</span>
        </span>
        <span className="card-head-spacer" />
        <button
          type="button"
          role="switch"
          className="switch"
          aria-checked={showGreeting}
          aria-label={t('greeting.label')}
          onClick={() => setShowGreeting(!showGreeting)}
        >
          <span className="switch-thumb" />
        </button>
      </div>
    </section>
  );
}
