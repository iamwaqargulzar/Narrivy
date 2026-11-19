// Where the dictation overlay sits: six places on a small picture of the
// screen. The choice is this app's own preference (ThemeProvider persists it
// and reports it to the host, which places the overlay).
import { t } from '../../../i18n';
import { OVERLAY_POSITIONS, useTheme } from '../../../theme/ThemeProvider';
import './SettingsCompute.css';

export function OverlayPositionCard() {
  const { overlayPosition, setOverlayPosition } = useTheme();
  return (
    <section className="settings-card" aria-labelledby="overlay-position-heading">
      <div className="settings-card-header" id="overlay-position-heading">
        {t('overlayPosition.title')}
      </div>
      <p className="settings-card-desc">{t('overlayPosition.desc')}</p>
      <div className="overlay-picker">
        <div className="overlay-screen" role="radiogroup" aria-label={t('overlayPosition.title')}>
          {OVERLAY_POSITIONS.map((position) => (
            <button
              key={position}
              type="button"
              role="radio"
              className="overlay-spot"
              data-position={position}
              aria-checked={overlayPosition === position}
              aria-label={t(`overlayPosition.${position}`)}
              title={t(`overlayPosition.${position}`)}
              onClick={() => setOverlayPosition(position)}
            >
              <span className="overlay-spot-pill" />
            </button>
          ))}
        </div>
        <p className="overlay-picker-current">{t(`overlayPosition.${overlayPosition}`)}</p>
      </div>
    </section>
  );
}
