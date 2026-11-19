// Shown while no speech model is in use: none ships with Narrivy (D32, D40), so
// the user downloads one from the list on the Models screen. Shared by that
// screen and the first-run wizard.
import { t } from '../../i18n';
import { Icon } from '../../ui/icons';

export function ModelNeededNote({ body }: { body: string }) {
  return (
    <section className="panel model-setup" data-testid="model-needed" aria-labelledby="model-needed-title">
      <span className="model-icon" aria-hidden="true">
        <Icon name="mic" />
      </span>
      <div className="model-setup-main">
        <h3 id="model-needed-title" className="model-name">
          {t('setup.title')}
        </h3>
        <p className="model-desc">{body}</p>
      </div>
    </section>
  );
}
