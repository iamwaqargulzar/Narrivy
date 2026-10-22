// Applications: per-application behaviour (for example a different insertion
// method for a terminal). This is scheduled after 1.0; the screen explains
// what it will do and never pretends any application is configured.
import { Icon } from '../../ui/icons';
import { t } from '../../i18n';

export function ApplicationsScreen() {
  return (
    <div className="page-wide">
      <div className="page-head page-head-row">
        <div>
          <h2 className="page-title">{t('nav.applications')}</h2>
          <p className="page-lead">{t('apps.lead')}</p>
        </div>
        <button type="button" className="btn btn-default" disabled title={t('apps.badge')}>
          <Icon name="plus" />
          {t('apps.add')}
        </button>
      </div>
      <section className="card empty-state">
        <Icon name="apps" />
        <strong>{t('apps.title')}</strong>
        <span className="screen-empty-note">{t('apps.body')}</span>
        <span className="chip chip-warn">{t('apps.badge')}</span>
      </section>
      <h3 className="section-title">{t('apps.plannedTitle')}</h3>
      <div className="card-row">
        {(['insertion', 'mode', 'vocabulary'] as const).map((item) => (
          <section key={item} className="card">
            <div className="card-head">
              <h4 className="card-head-title">{t(`apps.planned.${item}.title`)}</h4>
            </div>
            <p className="card-text">{t(`apps.planned.${item}.body`)}</p>
          </section>
        ))}
      </div>
    </div>
  );
}
