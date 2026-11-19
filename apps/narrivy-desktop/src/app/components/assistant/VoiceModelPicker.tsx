// Which voice pack (text-to-speech model) voice mode reads replies with, per
// language: automatic (the first installed), one the user picked, or for
// Urdu none (English reads Urdu replies too). The packs come from the
// service's voice settings; packs not installed are shown but cannot be
// picked (Models → Text to speech installs them).
import { t } from '../../../i18n';
import { modelInUse, type VoiceModelChoice } from '../../../lib/voiceModels';

export function VoiceModelPicker({
  choice,
  onChange,
  disabled,
}: {
  choice: VoiceModelChoice;
  onChange: (change: { englishModel?: string; urduModel?: string }) => void;
  disabled?: boolean;
}) {
  const row = (language: 'en' | 'ur') => {
    const models = choice.models.filter((m) => m.language === language);
    const value = language === 'en' ? choice.englishModel : choice.urduModel;
    const label = language === 'en' ? t('assistant.voice.englishModel') : t('assistant.voice.urduModel');
    const inUse = modelInUse(choice, language);
    return (
      <label className="voice-row" key={language}>
        <span className="voice-row-label">{label}</span>
        <select
          className="pipe-select"
          value={value}
          disabled={disabled}
          aria-describedby="voice-model-hint"
          onChange={(event) =>
            onChange(language === 'en' ? { englishModel: event.target.value } : { urduModel: event.target.value })
          }
        >
          <option value="">
            {inUse ? t('assistant.voice.modelAuto', { name: inUse.name }) : t('assistant.voice.modelAutoNone')}
          </option>
          {models.map((m) => (
            <option key={m.id} value={m.id} disabled={!m.installed && m.id !== value}>
              {m.installed ? m.name : t('assistant.voice.modelNotInstalled', { name: m.name })}
            </option>
          ))}
          {language === 'ur' ? <option value="off">{t('assistant.voice.urduOff')}</option> : null}
        </select>
      </label>
    );
  };
  return (
    <>
      {row('en')}
      {row('ur')}
      <p className="voice-hint" id="voice-model-hint">
        {t('assistant.voice.modelHint')}
      </p>
    </>
  );
}
