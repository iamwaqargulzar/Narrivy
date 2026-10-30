// Tips, opened from the top bar next to Search. Only real, working ways to
// use Narrivy are listed; the toggle command appears only when the daemon
// serves it.
import { useCallback, useState } from 'react';
import { usePopoverDismiss } from '../hooks/useDialogFocus';
import { Icon } from '../ui/icons';
import { t } from '../i18n';
import { activationPresentation } from '../lib/activation';
import { useDaemonStatus } from '../state/StatusContext';

export function TipsPopover() {
  const [open, setOpen] = useState(false);
  const { status } = useDaemonStatus();
  const activation = activationPresentation(status);
  const close = useCallback(() => setOpen(false), []);
  const root = usePopoverDismiss(open, close);

  return (
    <div className="tips-anchor" ref={root}>
      <button
        type="button"
        className="topbar-search"
        aria-expanded={open}
        aria-haspopup="dialog"
        aria-label={t('home.tipsTitle')}
        onClick={() => setOpen((v) => !v)}
      >
        <Icon name="bulb" />
        <span>{t('home.tipsTitle')}</span>
      </button>
      {open ? (
        <div className="tips-popover" role="dialog" aria-label={t('home.tipsTitle')}>
          <div className="tips-popover-head">
            <Icon name="bulb" />
            <strong>{t('home.tipsTitle')}</strong>
          </div>
          <dl className="tips">
            {activation.suggestedCommand ? (
              <div>
                <dt>
                  <code className="code-chip">{activation.suggestedCommand}</code>
                </dt>
                <dd>{t('home.tip.toggle')}</dd>
              </div>
            ) : null}
            <div>
              <dt>
                <span className="code-chip">{t('home.tip.trayKey')}</span>
              </dt>
              <dd>{t('home.tip.tray')}</dd>
            </div>
            <div>
              <dt>
                <kbd>{t('key.ctrl')}</kbd> + <kbd>K</kbd>
              </dt>
              <dd>{t('home.tip.search')}</dd>
            </div>
            <div>
              <dt>
                <kbd>{t('key.esc')}</kbd>
              </dt>
              <dd>{t('home.tip.escape')}</dd>
            </div>
          </dl>
        </div>
      ) : null}
    </div>
  );
}
