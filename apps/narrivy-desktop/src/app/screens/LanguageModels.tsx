// Models → Correction and Language models (D32): the language models on this
// computer, separate from speech-to-text models, and which one each job uses.
// Clean-up (Correction) polishes every dictation; the assistant answers in
// the Assistant screen. Each section shows its own jobs and models.
// Nothing ships with Narrivy: models are downloaded, and "Recommended" marks
// the one the app suggests first (D31).
import { useCallback, useEffect, useState } from 'react';
import { bridge, type LanguageModel } from '../../lib/bridge';
import { Icon } from '../../ui/icons';
import { t } from '../../i18n';
import { useErrorHandler } from '../../ui/primitives/Toast';

const ROLES = ['clean_up', 'assistant'] as const;
export type Role = (typeof ROLES)[number];

function formatBytes(bytes: number): string {
  if (bytes >= 1e9) return `${(bytes / 1e9).toFixed(1)} GB`;
  return `${Math.round(bytes / 1e6)} MB`;
}

/** The clean-up model's state, in words. */
export function cleanUpState(state: string | undefined): { label: string; ok: boolean } | null {
  if (!state) return null;
  if (state === 'loading') return { label: t('lm.state.loading'), ok: false };
  if (state.startsWith('ready:')) {
    return { label: t('lm.state.ready', { device: state.slice('ready:'.length).toUpperCase() }), ok: true };
  }
  return { label: t('lm.state.failed', { code: state.slice(state.indexOf(':') + 1) }), ok: false };
}

export function LanguageModelsPanel({ listRole }: { listRole?: Role }) {
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

  const forRole = listRole ? models.filter((model) => model.roles.includes(listRole)) : models;
  const listed = forRole.filter((model) => model.installed !== false);
  const offered = forRole.filter((model) => model.installed === false);

  const download = (id: string) => {
    bridge
      .downloadModel(id)
      .then(load)
      .catch((err) => report(err));
  };
  const cancel = (id: string) => {
    bridge
      .cancelModelDownload(id)
      .then(load)
      .catch((err) => report(err));
  };
  const remove = (id: string) => {
    bridge
      .removeStoreItem(id)
      .then(load)
      .catch((err) => report(err));
  };

  return (
    <>
      <section className="model-section">
        <div className="model-section-head">
          <h3>{t('lm.onComputer')}</h3>
          <span className="model-count">{listed.length}</span>
          <span className="model-section-hint">{t('lm.onComputerHint')}</span>
        </div>
        {listed.length === 0 ? (
          <div className="panel empty-state">
            <Icon name="models" />
            <strong>{t('lm.emptyTitle')}</strong>
            <span className="screen-empty-note">{t('lm.empty')}</span>
          </div>
        ) : (
          <ul className="model-rows">
            {listed.map((model) => (
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
                  {model.license_notice ? <p className="lm-notice">{model.license_notice}</p> : null}
                </div>
                <div className="model-row-side">
                  <span className="model-size">
                    <Icon name="drive" />
                    {formatBytes(model.size_bytes)}
                  </span>
                  {model.in_use_for.length === 0 ? (
                    <button
                      type="button"
                      className="btn btn-default"
                      aria-label={t('lm.removeNamed', { name: model.name })}
                      onClick={() => remove(model.id)}
                    >
                      {t('lm.remove')}
                    </button>
                  ) : null}
                </div>
              </li>
            ))}
          </ul>
        )}
      </section>

      {offered.length > 0 ? (
        <section className="model-section">
          <div className="model-section-head">
            <h3>{t('lm.toDownload')}</h3>
            <span className="model-count">{offered.length}</span>
            <span className="model-section-hint">{t('lm.toDownloadHint')}</span>
          </div>
          <ul className="model-rows">
            {offered.map((model) => (
              <OfferedRow
                key={model.id}
                model={model}
                onDownload={() => download(model.id)}
                onCancel={() => cancel(model.id)}
              />
            ))}
          </ul>
        </section>
      ) : null}
      <p className="source lm-more">{t('lm.more')}</p>
    </>
  );
}

/** A language model that can be downloaded, with its download state. */
function OfferedRow({
  model,
  onDownload,
  onCancel,
}: {
  model: LanguageModel;
  onDownload: () => void;
  onCancel: () => void;
}) {
  const state = model.download ?? 'available';
  const failed = state.startsWith('failed:') ? state.slice('failed:'.length) : null;
  const percent =
    state === 'installing' && model.size_bytes > 0
      ? Math.min(100, Math.floor(((model.installed_bytes ?? 0) * 100) / model.size_bytes))
      : null;
  return (
    <li className="model-row" data-testid={`lm-offer-${model.id}`}>
      <div className="model-row-main">
        <div className="model-row-title">
          <span className="model-row-name">{model.name}</span>
          {model.recommended_for.map((role) => (
            <span key={`rec-${role}`} className="chip lm-recommended">
              {t('lm.recommendedFor', { job: t(`lm.role.${role}`) })}
            </span>
          ))}
        </div>
        <div className="model-facts">
          <span className="model-fact">
            <Icon name="spark" />
            {model.roles.map((role) => t(`lm.role.${role}`)).join(' · ')}
          </span>
          <span className="model-fact">{model.publisher}</span>
          <span className="model-fact">{model.license}</span>
        </div>
        {failed ? (
          <p className="lm-notice" role="alert">
            {t('lm.downloadFailed', { reason: failed })}
          </p>
        ) : null}
        {state === 'unpublished' ? <p className="lm-notice">{t('lm.unpublished')}</p> : null}
      </div>
      <div className="model-row-side">
        <span className="model-size">
          <Icon name="drive" />
          {formatBytes(model.size_bytes)}
        </span>
        {percent !== null ? (
          <>
            <span className="chip" role="status">
              {t('lm.downloading', { percent })}
            </span>
            <button type="button" className="btn btn-default" onClick={onCancel}>
              {t('lm.cancel')}
            </button>
          </>
        ) : state === 'unpublished' ? null : (
          <button
            type="button"
            className="btn btn-primary"
            aria-label={t('lm.downloadNamed', { name: model.name })}
            onClick={onDownload}
          >
            {t('lm.download')}
          </button>
        )}
      </div>
    </li>
  );
}

/** The assistant model's state, in words. */
function assistantState(state: string | undefined): { label: string; ok: boolean } | null {
  if (!state) return null;
  if (state === 'loading') return { label: t('assistant.loadingModel'), ok: false };
  if (state.startsWith('loaded:')) {
    return { label: t('assistant.model.loaded', { device: state.slice('loaded:'.length).toUpperCase() }), ok: true };
  }
  if (state.startsWith('failed:')) {
    return { label: t('assistant.model.failed', { code: state.slice('failed:'.length) }), ok: false };
  }
  return { label: t('assistant.model.unloaded'), ok: false };
}

/** Settings → Extensions → AI assistant: which model answers, whether it is
 * loaded (with Load or Unload), and whether it unloads when idle. */
export function AssistantModelSettings({ onGetModel }: { onGetModel: () => void }) {
  const report = useErrorHandler();
  const [models, setModels] = useState<LanguageModel[] | null>(null);
  const [autoUnload, setAutoUnload] = useState<boolean | null>(null);

  const load = useCallback(() => {
    bridge.languageModels().then(setModels, () => setModels(null));
  }, []);
  useEffect(() => {
    load();
    bridge
      .assistantLibrary('get', 'model')
      .then((json) => setAutoUnload(Boolean((JSON.parse(json) as { autoUnload?: boolean }).autoUnload)))
      .catch(() => setAutoUnload(null));
  }, [load]);
  // Follow a load until it is done.
  const loading = (models ?? []).some((model) => model.assistant_state === 'loading');
  useEffect(() => {
    if (!loading) return;
    const timer = window.setInterval(load, 1500);
    return () => window.clearInterval(timer);
  }, [loading, load]);

  if (models === null) return null;
  const able = models.filter((model) => model.installed !== false && model.roles.includes('assistant'));
  const current = able.find((model) => model.in_use_for.includes('assistant')) ?? null;
  const state = assistantState(current?.assistant_state);
  const loaded = (current?.assistant_state ?? '').startsWith('loaded:');

  const after = (action: Promise<unknown>, wait = 0) =>
    void action.then(() => window.setTimeout(load, wait)).catch((err) => report(err));

  return (
    <div className="ext-settings" data-testid="assistant-model-settings">
      <div className="ext-setting-row">
        <span className="ext-setting-text">
          <span id="assistant-model-label">{t('ext.assistant.model')}</span>
          <span className="ext-setting-hint">{t('ext.assistant.modelHint')}</span>
        </span>
        {able.length === 0 ? (
          <button type="button" className="btn btn-primary" onClick={onGetModel}>
            {t('ext.assistant.getModel')}
          </button>
        ) : (
          <select
            aria-labelledby="assistant-model-label"
            value={current?.id ?? ''}
            onChange={(event) => after(bridge.selectLanguageModel('assistant', event.target.value || null), 300)}
          >
            <option value="">{t('lm.none')}</option>
            {able.map((model) => (
              <option key={model.id} value={model.id}>
                {model.name}
                {model.recommended_for.includes('assistant') ? ` · ${t('lm.recommended')}` : ''}
              </option>
            ))}
          </select>
        )}
      </div>
      {current ? (
        <div className="ext-setting-row">
          <span className="ext-setting-text">
            <span>{t('ext.assistant.state')}</span>
            {state ? <span className={`chip${state.ok ? ' chip-ok' : ''}`} role="status">{state.label}</span> : null}
          </span>
          {loaded ? (
            <button type="button" className="btn btn-default" title={t('assistant.unloadHint')} onClick={() => after(bridge.assistantUnload(), 300)}>
              {t('assistant.unload')}
            </button>
          ) : (
            <button
              type="button"
              className="btn btn-primary"
              title={t('assistant.loadHint')}
              disabled={loading}
              onClick={() => after(bridge.assistantLibrary('run', 'model'))}
            >
              {t('assistant.load')}
            </button>
          )}
        </div>
      ) : null}
      {current && autoUnload !== null ? (
        <div className="ext-setting-row">
          <span className="ext-setting-text">
            <span id="assistant-auto-unload-setting">{t('assistant.autoUnload')}</span>
            <span className="ext-setting-hint">{t('ext.assistant.autoUnloadHint')}</span>
          </span>
          <button
            type="button"
            role="switch"
            className="switch"
            aria-checked={autoUnload}
            aria-labelledby="assistant-auto-unload-setting"
            onClick={() => {
              const next = !autoUnload;
              setAutoUnload(next);
              void bridge
                .assistantLibrary('put', 'model', null, JSON.stringify({ autoUnload: next }))
                .catch((err) => {
                  setAutoUnload(!next);
                  report(err);
                });
            }}
          >
            <span className="switch-thumb" />
          </button>
        </div>
      ) : null}
    </div>
  );
}
