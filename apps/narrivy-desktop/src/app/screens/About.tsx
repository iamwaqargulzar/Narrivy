// About: product identity, the installed versions, and update status.
//
// Update policy (docs/maintainers/operations/update-policy.md): no in-app
// updater; updates come through the package manager, and a check against
// published releases may only run when the user asks and has allowed it to go
// online. That check is not built yet, so the status is "Not checked" and the
// button is shown disabled with the reason rather than claiming "Up to date".
import { useEffect, useState } from 'react';
import { t } from '../../i18n';
import { bridge } from '../../lib/bridge';
import { BrandMark } from '../../ui/icons';
import { useDaemonStatus } from '../../state/StatusContext';

export function AboutScreen() {
  const { status, loading } = useDaemonStatus();
  // The update route comes from the desktop shell, which knows how it was
  // installed; a build without one shows no command.
  const [updateCommand, setUpdateCommand] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    bridge.installFacts().then(
      (facts) => alive && setUpdateCommand(facts.update_command),
      () => alive && setUpdateCommand(null),
    );
    return () => {
      alive = false;
    };
  }, []);
  const service = status?.daemon_build ?? t('about.unknown');
  const localOnly = status?.capabilities.includes('policy.local-only=enabled') ?? false;
  const protocol = typeof status?.protocol_minor === 'number' ? t('about.protocolMinor', { minor: status.protocol_minor }) : t('about.unknown');

  return (
    <div className="page">
      <div className="panel about-hero">
        <BrandMark />
        <div>
          <div className="about-name">{t('app.name')}</div>
          <p className="page-lead" style={{ marginTop: 4 }}>
            {t('about.blurb')}
          </p>
        </div>
      </div>

      <section className="panel about-updates" aria-labelledby="about-updates-title">
        <div className="about-updates-head">
          <div>
            <h3 className="card-head-title" id="about-updates-title">{t('about.updates')}</h3>
            <p className="about-updates-state" data-state="unchecked">
              <span className="status-dot" aria-hidden="true" />
              {t('version.label', { version: __APP_VERSION__ })}
            </p>
          </div>
        </div>
        <p className="source">{t('about.updatesBody')}</p>
        {updateCommand ? (
          <>
            <p className="source">{t('about.updatesHowTo')}</p>
            <code className="about-command">{updateCommand}</code>
          </>
        ) : null}
      </section>

      <dl className="about-facts">
        <div className="detail-cell">
          <dt>{t('about.app')}</dt>
          <dd className="resolved">{t('about.version', { version: __APP_VERSION__ })}</dd>
        </div>
        <div className="detail-cell">
          <dt>{t('about.daemon')}</dt>
          <dd className="resolved">{t('about.version', { version: service })}</dd>
        </div>
        <div className="detail-cell">
          <dt>{t('about.protocol')}</dt>
          <dd>{protocol}</dd>
        </div>
        <div className="detail-cell">
          <dt>{t('about.privacy')}</dt>
          <dd>{localOnly ? t('about.privacyBody') : t('about.privacyUnreported')}</dd>
        </div>
      </dl>
      {loading ? <p className="source" style={{ marginTop: 10 }}>{t('common.loading')}</p> : null}
    </div>
  );
}
