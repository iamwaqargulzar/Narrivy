// Settings → Extensions (D48): the extensions that ship with Narrivy, each
// with an on/off switch. They used to be listed in the Store. The AI
// assistant and meeting transcription show their own sidebar screens while
// on; the writing assistant registers the browser connector with Chrome,
// Chromium and Brave, then the user loads the extension once.
import { useEffect, useState } from 'react';
import { bridge, type BrowserConnector } from '../../../lib/bridge';
import { t } from '../../../i18n';
import { useExtensions, type ExtensionId } from '../../../state/ExtensionsContext';
import { useErrorHandler } from '../../../ui/primitives/Toast';
import { useNav } from '../../../state/NavContext';
import { AssistantModelSettings } from '../../screens/LanguageModels';
import './SettingsCompute.css';

function Switch({
  labelledBy,
  on,
  disabled,
  onChange,
}: {
  labelledBy: string;
  on: boolean;
  disabled?: boolean;
  onChange: (on: boolean) => void;
}) {
  return (
    <button
      type="button"
      role="switch"
      className="switch"
      aria-checked={on}
      aria-labelledby={labelledBy}
      disabled={disabled}
      onClick={() => onChange(!on)}
    >
      <span className="switch-thumb" />
    </button>
  );
}

function ScreenExtension({ id }: { id: ExtensionId }) {
  const report = useErrorHandler();
  const extensions = useExtensions();
  const { navigate } = useNav();
  const [saving, setSaving] = useState(false);
  const on = extensions[id];
  const change = async (next: boolean) => {
    setSaving(true);
    try {
      await extensions.setEnabled(id, next);
    } catch (err) {
      report(err, { retry: () => void change(next) });
    } finally {
      setSaving(false);
    }
  };
  return (
    <>
      <div className="settings-card-header settings-card-header-row startup-row">
        <span id={`ext-${id}`}>{t(`ext.${id}.title`)}</span>
        <Switch labelledBy={`ext-${id}`} on={on} disabled={saving} onChange={(next) => void change(next)} />
      </div>
      <p className="settings-card-desc">{t(`ext.${id}.desc`)}</p>
      {id === 'assistant' && on ? <AssistantModelSettings onGetModel={() => navigate('store', 'language')} /> : null}
    </>
  );
}

function WritingExtension() {
  const report = useErrorHandler();
  const [state, setState] = useState<BrowserConnector | null>(null);
  const [saving, setSaving] = useState(false);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    let alive = true;
    bridge.browserConnector().then(
      (found) => alive && setState(found),
      () => alive && setState(null),
    );
    return () => {
      alive = false;
    };
  }, []);

  const change = async (next: boolean) => {
    setSaving(true);
    try {
      setState(await bridge.setBrowserConnector(next));
    } catch (err) {
      report(err, { retry: () => void change(next) });
    } finally {
      setSaving(false);
    }
  };

  const registered = state?.browsers.filter((browser) => browser.registered) ?? [];
  const on = registered.length > 0;
  const folder = state?.extension_dir ?? null;
  return (
    <>
      <div className="settings-card-header settings-card-header-row startup-row">
        <span id="ext-writing">{t('ext.writing.title')}</span>
        <Switch
          labelledBy="ext-writing"
          on={on}
          disabled={!state || saving || (!on && !state.host_installed)}
          onChange={(next) => void change(next)}
        />
      </div>
      <p className="settings-card-desc">
        {state && !state.host_installed ? t('ext.writing.notInstalled') : t('ext.writing.desc')}
      </p>
      {on ? (
        <div className="settings-card-desc ext-steps">
          <p>{t('ext.writing.registered', { browsers: registered.map((browser) => browser.name).join(', ') })}</p>
          <ol>
            <li>
              {t('ext.writing.step1', {
                pages: [...new Set(registered.map((browser) => browser.extensions_page))].join(' / '),
              })}
            </li>
            <li>{t('ext.writing.step2')}</li>
            <li>
              {t('ext.writing.step3')}{' '}
              {folder ? (
                <>
                  <code>{folder}</code>{' '}
                  <button
                    type="button"
                    className="btn btn-default"
                    onClick={() => {
                      void navigator.clipboard
                        ?.writeText(folder)
                        .then(() => setCopied(true))
                        .catch(() => {});
                    }}
                  >
                    {copied ? t('ext.writing.copied') : t('ext.writing.copy')}
                  </button>
                </>
              ) : null}
            </li>
          </ol>
        </div>
      ) : null}
    </>
  );
}

export function ExtensionsCard() {
  return (
    <section className="settings-card" aria-labelledby="extensions-heading">
      <div className="settings-card-header">
        <span id="extensions-heading">{t('ext.title')}</span>
      </div>
      <p className="settings-card-desc">{t('ext.lead')}</p>
      <ScreenExtension id="assistant" />
      <ScreenExtension id="meetings" />
      <WritingExtension />
    </section>
  );
}
