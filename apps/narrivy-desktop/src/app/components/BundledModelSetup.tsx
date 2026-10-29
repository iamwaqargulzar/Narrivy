// "Set up speech recognition": the one card that installs the speech model
// shipped inside the package. It exists only while no model is in use, and
// follows the daemon's install operation in plain words; when the model is
// ready it is gone and the model's own row says "In use". Shared by the
// Models screen and the first-run wizard.
import { t } from '../../i18n';
import type { ModelInfo } from '../../lib/bridge';
import { modelName } from '../../lib/modelFacts';
import { formatBytes } from '../../lib/overview';
import { Button } from '../../ui/primitives/Button';
import { Icon } from '../../ui/icons';

const WORKING = new Set(['importing', 'cancel_requested', 'activating']);
const FAILED = new Set(['failed', 'timed_out']);

export function BundledModelSetup({
  provisionState,
  failureText,
  model,
  onInstall,
  onCancel,
}: {
  /** The daemon's install operation state, when one exists. */
  provisionState: string | null;
  /** Why the last install failed, in words. */
  failureText: string | null;
  /** The bundled model's catalog entry, when the screen has the listing. */
  model?: ModelInfo | null;
  onInstall: () => void;
  onCancel: () => void;
}) {
  const working = WORKING.has(provisionState ?? '');
  const failed = FAILED.has(provisionState ?? '') || Boolean(failureText);
  const size = model ? formatBytes(model.size_bytes) : null;
  const body =
    model && size
      ? t('setup.body', { name: modelName(model), size })
      : t('setup.bodyGeneric');
  return (
    <section
      className="panel model-setup"
      data-state={working ? 'working' : failed ? 'failed' : 'idle'}
      data-testid="model-install"
      aria-labelledby="model-setup-title"
    >
      <span className="model-icon" aria-hidden="true">
        <Icon name="mic" />
      </span>
      <div className="model-setup-main">
        <h3 id="model-setup-title" className="model-name">
          {t('setup.title')}
        </h3>
        <p className="model-desc">{body}</p>
        {working ? (
          <div className="model-setup-progress" role="status">
            <div className="model-progress-bar is-indeterminate" aria-hidden="true">
              <span />
            </div>
            <span className="model-progress-text">{t(`setup.step.${provisionState}`)}</span>
          </div>
        ) : null}
        {!working && failed ? (
          <p className="model-download-error" role="alert" data-testid="model-install-failure">
            {failureText ?? t('setup.failedGeneric')}
          </p>
        ) : null}
        {!working && provisionState === 'cancelled' ? (
          <p className="model-setup-note" role="status">
            {t('setup.cancelled')}
          </p>
        ) : null}
      </div>
      <div className="model-setup-action">
        {working ? (
          <Button onClick={onCancel} disabled={provisionState === 'cancel_requested'}>
            {t('setup.cancel')}
          </Button>
        ) : (
          <Button variant="primary" onClick={onInstall}>
            {failed ? t('setup.retry') : t('setup.action')}
          </Button>
        )}
      </div>
    </section>
  );
}
