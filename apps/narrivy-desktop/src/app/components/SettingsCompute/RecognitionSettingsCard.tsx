import { useState } from 'react';
import { t } from '../../../i18n';
import { RECOGNITION_SETTING_KEYS, recognitionOptions } from '../../../lib/recognition';
import {
  loadSettings,
  saveSettings,
  type NarrivySettingsEnvelope,
  type SettingValue,
} from '../../../lib/settings';
import { useDaemonStatus } from '../../../state/StatusContext';
import './SettingsCompute.css';

function LanguageIcon(props: React.SVGProps<SVGSVGElement>) {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.7} aria-hidden="true" {...props}>
      <circle cx="12" cy="12" r="9" />
      <path d="M3 12h18M12 3a15 15 0 0 1 0 18M12 3a15 15 0 0 0 0 18" />
    </svg>
  );
}

function settingString(env: NarrivySettingsEnvelope, key: string): string {
  const value = env.values[key];
  return typeof value === 'string' && value.length > 0 ? value : 'auto';
}

function advertisedPlusCurrent(advertised: string[], current: string): string[] {
  const tags = advertised.filter((tag) => tag !== 'auto' && tag !== 'detect');
  if (current !== 'auto' && current !== 'detect' && current !== '' && !tags.includes(current)) {
    tags.push(current);
  }
  return tags;
}

function RecognitionSelect({
  id,
  label,
  value,
  tags,
  extra,
  disabled,
  onChange,
}: {
  id: string;
  label: string;
  value: string;
  tags: string[];
  extra?: Array<{ value: string; label: string }>;
  disabled: boolean;
  onChange: (value: string) => void;
}) {
  return (
    <div className="recognition-field">
      <label htmlFor={id}>{label}</label>
      <select
        id={id}
        className="delivery-select"
        aria-label={label}
        value={value}
        disabled={disabled}
        onChange={(event) => onChange(event.target.value)}
      >
        <option value="auto">{t('settings.opt.auto')}</option>
        {extra?.map((option) => (
          <option key={option.value} value={option.value}>
            {option.label}
          </option>
        ))}
        {advertisedPlusCurrent(tags, value).map((tag) => (
          <option key={tag} value={tag}>
            {tag}
          </option>
        ))}
      </select>
    </div>
  );
}

export function RecognitionSettingsCard() {
  const { status } = useDaemonStatus();
  const [env, setEnv] = useState<NarrivySettingsEnvelope>(() => loadSettings());
  const options = recognitionOptions(status);
  const update = (key: string, value: SettingValue) => {
    const current = loadSettings();
    const next: NarrivySettingsEnvelope = {
      schema_version: 1,
      values: { ...current.values, [key]: value },
    };
    setEnv(next);
    saveSettings(next);
  };

  const spoken = settingString(env, RECOGNITION_SETTING_KEYS.spokenLanguage);
  const outputLanguage = settingString(env, RECOGNITION_SETTING_KEYS.outputLanguage);
  const outputScript = settingString(env, RECOGNITION_SETTING_KEYS.outputScript);
  const translationTarget = settingString(env, RECOGNITION_SETTING_KEYS.translationTarget);
  const locale = settingString(env, RECOGNITION_SETTING_KEYS.locale);
  const controlsDisabled = !options.available;
  const statusId = 'recognition-settings-status';
  const detectOption =
    options.languageDetection || spoken === 'detect'
      ? [{ value: 'detect', label: t('settings.recognition.autoDetect') }]
      : undefined;

  return (
    <section
      className="settings-card"
      data-testid="recognition-settings"
      aria-labelledby="recognition-heading"
      aria-describedby={options.available ? undefined : statusId}
    >
      <div className="settings-card-header" id="recognition-heading">
        <LanguageIcon />
        {t('settings.recognition.title')}
      </div>
      <p className="settings-card-desc">{t('settings.recognition.description')}</p>
      {options.available ? null : (
        <p id={statusId} className="recognition-status" role="status">
          {t('settings.recognition.unavailable')}
        </p>
      )}
      <div className="recognition-grid" role="group" aria-label={t('settings.recognition.title')}>
        <RecognitionSelect
          id="recognition-spoken-language"
          label={t('settings.recognition.spokenLanguage')}
          value={spoken}
          tags={options.languages}
          extra={detectOption}
          disabled={controlsDisabled}
          onChange={(value) => update(RECOGNITION_SETTING_KEYS.spokenLanguage, value)}
        />
        <RecognitionSelect
          id="recognition-output-language"
          label={t('settings.recognition.outputLanguage')}
          value={outputLanguage}
          tags={translationTarget === 'auto' ? options.languages : options.translationTargets}
          disabled={controlsDisabled}
          onChange={(value) => update(RECOGNITION_SETTING_KEYS.outputLanguage, value)}
        />
        <RecognitionSelect
          id="recognition-output-script"
          label={t('settings.recognition.outputScript')}
          value={outputScript}
          tags={options.scripts}
          disabled={controlsDisabled}
          onChange={(value) => update(RECOGNITION_SETTING_KEYS.outputScript, value)}
        />
        <RecognitionSelect
          id="recognition-translation-target"
          label={t('settings.recognition.translationTarget')}
          value={translationTarget}
          tags={options.translationTargets}
          disabled={controlsDisabled}
          onChange={(value) => update(RECOGNITION_SETTING_KEYS.translationTarget, value)}
        />
        <RecognitionSelect
          id="recognition-locale"
          label={t('settings.recognition.locale')}
          value={locale}
          tags={options.locales}
          disabled={controlsDisabled}
          onChange={(value) => update(RECOGNITION_SETTING_KEYS.locale, value)}
        />
        {/* Words to recognise are the vocabulary, in the pipeline's
            Transcribe stage ("Your words"): one place for them. */}
      </div>
    </section>
  );
}
