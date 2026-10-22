// Accent swatches (Mono, Blue, Gold). Used in the top bar and in Settings;
// both write the same persisted choice through ThemeProvider.
import { ACCENTS, useTheme } from '../theme/ThemeProvider';
import { t } from '../i18n';

export function AccentPicker({ size = 'small' }: { size?: 'small' | 'large' }) {
  const { accent, setAccent } = useTheme();
  return (
    <div className={`accent-picker accent-picker-${size}`} role="radiogroup" aria-label={t('accent.label')}>
      {ACCENTS.map((choice) => (
        <button
          key={choice}
          type="button"
          role="radio"
          aria-checked={accent === choice}
          aria-label={t(`accent.${choice}`)}
          title={t(`accent.${choice}`)}
          className="accent-swatch"
          data-swatch={choice}
          onClick={() => setAccent(choice)}
        >
          {size === 'large' ? <span className="accent-name">{t(`accent.${choice}`)}</span> : null}
        </button>
      ))}
    </div>
  );
}
