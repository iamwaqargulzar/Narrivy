// Models → Language models (D32): the language models on this computer,
// separate from speech-to-text models, and which one each job uses. Clean-up
// polishes every dictation; the assistant answers in the Assistant screen.
// Nothing ships with Narrivy: models are downloaded, and "Recommended" marks
// the one the app suggests first (D31).
import { useCallback, useEffect, useState } from 'react';
import { bridge, type LanguageModel } from '../../lib/bridge';
import { Icon } from '../../ui/icons';
import { t } from '../../i18n';
import { useErrorHandler } from '../../ui/primitives/Toast';

const ROLES = ['clean_up', 'assistant'] as const;
type Role = (typeof ROLES)[number];

function formatBytes(bytes: number): string {
  if (bytes >= 1e9) return `${(bytes / 1e9).toFixed(1)} GB`;
  return `${Math.round(bytes / 1e6)} MB`;
}

/** The clean-up model's state, in words. */
function cleanUpState(state: string | undefined): { label: string; ok: boolean } | null {
  if (!state) return null;
  if (state === 'loading') return { label: t('lm.state.loading'), ok: false };
  if (state.startsWith('ready:')) {
    return { label: t('lm.state.ready', { device: state.slice('ready:'.length).toUpperCase() }), ok: true };
  }
  return { label: t('lm.state.failed', { code: state.slice(state.indexOf(':') + 1) }), ok: false };
}

/** The assistant model's state, in words. */
function assistantState(state: string | undefined): { label: string; ok: boolean } | null {
  if (!state) return null;
  if (state.startsWith('loaded:')) {
    return { label: t('assistant.model.loaded', { device: state.slice('loaded:'.length).toUpperCase() }), ok: true };
  }
  return { label: t('assistant.model.unloaded'), ok: false };
}

export function LanguageModelsPanel() {
  const report = useErrorHandler();
  const [models, setModels] = useState<LanguageModel[] | null>(null);
  const [failed, setFailed] = useState(false);

  const load = useCallback(() => {
    bridge
      .languageModels()
      .then((list) => {
        setModels(list);
        setFailed(false);
      })
      .catch(() => setFailed(true));
  }, []);

  useEffect(() => {
    load();
    // The clean-up model loads in the background: follow it until ready.
    const timer = window.setInterval(load, 3000);
    return () => window.clearInterval(timer);
  }, [load]);

  const choose = (role: Role, id: string | null) => {
    bridge
      .selectLanguageModel(role, id)
      .then(load)
      .catch((err) => report(err));
  };

  if (failed) {
    return (
      <div className="panel empty-state">
        <Icon name="models" />
        <strong>{t('lm.unavailableTitle')}</strong>
        <span className="screen-empty-note">{t('lm.unavailable')}</span>
      </div>
    );
  }
  if (models === null) {
    return <p className="source">{t('common.loading')}</p>;
  }

  return (
    <>
      <div className="lm-jobs">
        {ROLES.map((role) => {
          const able = models.filter((model) => model.roles.includes(role));
          const current = models.find((model) => model.in_use_for.includes(role)) ?? null;
          const state = role === 'clean_up' ? cleanUpState(current?.clean_up_state) : assistantState(current?.assistant_state);
          const loaded = role === 'assistant' && (current?.assistant_state ?? '').startsWith('loaded:');
          return (
            <section key={role} className="panel lm-job" aria-labelledby={`lm-job-${role}`}>
              <div className="lm-job-head">
                <span className="model-icon" aria-hidden="true">
                  <Icon name={role === 'clean_up' ? 'sliders' : 'spark'} />
                </span>
                <div>
                  <h3 id={`lm-job-${role}`}>{t(`lm.role.${role}`)}</h3>
                  <p className="model-desc">{t(`lm.role.${role}Hint`)}</p>
                </div>
              </div>
              <label className="model-filter lm-job-select">
                <span>{t('lm.uses')}</span>
                <select
                  aria-label={t('lm.usesFor', { job: t(`lm.role.${role}`) })}
                  value={current?.id ?? ''}
                  onChange={(event) => choose(role, event.target.value || null)}
                >
                  <option value="">{role === 'clean_up' ? t('lm.noneCleanUp') : t('lm.none')}</option>
                  {able.map((model) => (
                    <option key={model.id} value={model.id}>
                      {model.name}
                      {model.recommended_for.includes(role) ? ` · ${t('lm.recommended')}` : ''}
                    </option>
                  ))}
                </select>
              </label>
              <div className="lm-job-state">
                {state ? <span className={`chip${state.ok ? ' chip-ok' : ''}`}>{state.label}</span> : null}
                {loaded ? (
                  <button
                    type="button"
                    className="btn btn-default"
                    title={t('assistant.unloadHint')}
                    onClick={() => {
                      void bridge
                        .assistantUnload()
                        .then(() => window.setTimeout(load, 300))
                        .catch((err) => report(err));
                    }}
                  >
                    {t('assistant.unload')}
                  </button>
                ) : null}
              </div>
            </section>
          );
        })}
      </div>

      <section className="model-section">
        <div className="model-section-head">
          <h3>{t('lm.onComputer')}</h3>
          <span className="model-count">{models.length}</span>
          <span className="model-section-hint">{t('lm.onComputerHint')}</span>
        </div>
        {models.length === 0 ? (
          <div className="panel empty-state">
            <Icon name="models" />
            <strong>{t('lm.emptyTitle')}</strong>
            <span className="screen-empty-note">{t('lm.empty')}</span>
          </div>
        ) : (
          <ul className="model-rows">
            {models.map((model) => (
              <li key={model.id} className="model-row" data-testid={`lm-${model.id}`}>
                <div className="model-row-main">
                  <div className="model-row-title">
                    <span className="model-row-name">{model.name}</span>
                    {model.recommended_for.map((role) => (
                      <span key={`rec-${role}`} className="chip lm-recommended">
                        {t('lm.recommendedFor', { job: t(`lm.role.${role}`) })}
                      </span>
                    ))}
                    {model.in_use_for.map((role) => (
                      <span key={`use-${role}`} className="chip chip-ok">
                        <Icon name="check" />
                        {t('lm.inUseFor', { job: t(`lm.role.${role}`) })}
                      </span>
                    ))}
                  </div>
                  <div className="model-facts">
                    <span className="model-fact">
                      <Icon name="spark" />
                      {model.roles.map((role) => t(`lm.role.${role}`)).join(' · ')}
                    </span>
                    {model.languages.length ? (
                      <span className="model-fact">
                        <Icon name="globe" />
                        {model.languages.join(', ')}
                      </span>
                    ) : null}
                  </div>
                  <p className="model-source">
                    {model.publisher ? `${model.publisher} · ` : ''}
                    {model.license}
                  </p>
                  {model.license_notice ? <p className="lm-notice">{model.license_notice}</p> : null}
                </div>
                <div className="model-row-side">
                  <span className="model-size">
                    <Icon name="drive" />
                    {formatBytes(model.size_bytes)}
                  </span>
                </div>
              </li>
            ))}
          </ul>
        )}
        <p className="source lm-more">{t('lm.more')}</p>
      </section>
    </>
  );
}
