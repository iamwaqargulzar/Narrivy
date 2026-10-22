import type { RecognitionSettings, StatusSnapshot } from './bridge';
import type { NarrivySettingsEnvelope, SettingValue } from './settings';

export const RECOGNITION_PROTOCOL_MINOR = 9;
export const RECOGNITION_SETTING_KEYS = {
  spokenLanguage: 'asr.language',
  outputLanguage: 'asr.output_language',
  outputScript: 'asr.output_script',
  translationTarget: 'asr.translation_target',
  locale: 'asr.locale',
  contextHints: 'asr.context_hints',
} as const;

export interface RecognitionOptions {
  available: boolean;
  languageDetection: boolean;
  contextBias: boolean;
  languages: string[];
  translationTargets: string[];
  scripts: string[];
  locales: string[];
}

function capabilityValue(capabilities: string[], name: string): string | null {
  const prefix = `${name}=`;
  return capabilities.find((item) => item.startsWith(prefix))?.slice(prefix.length) ?? null;
}

function capabilityTags(capabilities: string[], name: string): string[] {
  const raw = capabilityValue(capabilities, name);
  if (!raw) return [];
  return raw.split(',').filter((tag) => tag.length > 0 && tag.length <= 32);
}

export function recognitionOptions(status: StatusSnapshot | null): RecognitionOptions {
  const capabilities = status?.capabilities ?? [];
  const protocolReady = (status?.protocol_minor ?? 0) >= RECOGNITION_PROTOCOL_MINOR;
  return {
    available: protocolReady && capabilities.includes('asr.recognition-settings=available'),
    languageDetection: capabilities.includes('asr.language-detection=available'),
    contextBias: capabilities.includes('asr.context-bias=available'),
    languages: capabilityTags(capabilities, 'asr.languages'),
    translationTargets: capabilityTags(capabilities, 'asr.translation-targets'),
    scripts: capabilityTags(capabilities, 'asr.scripts'),
    locales: capabilityTags(capabilities, 'asr.locales'),
  };
}

function explicitValue(values: Record<string, SettingValue>, key: string): string | null {
  const value = values[key];
  return typeof value === 'string' && value !== '' && value !== 'auto' ? value : null;
}

function requireOption(value: string | null, options: string[], field: string): string | undefined {
  if (value === null) return undefined;
  if (!options.includes(value)) {
    throw { code: 'unsupported_recognition_setting', message: `${field} is no longer supported by the loaded model` };
  }
  return value;
}

function contextHints(value: string | null, supported: boolean): string[] | undefined {
  if (value === null) return undefined;
  if (!supported) {
    throw { code: 'unsupported_recognition_setting', message: 'Context bias is not supported by the loaded model' };
  }
  const hints = value.split(/[\n,]/).map((hint) => hint.trim()).filter(Boolean);
  if (
    hints.length === 0
    || hints.length > 8
    || hints.some((hint) => new TextEncoder().encode(hint).length > 128 || hint.includes('\0'))
  ) {
    throw { code: 'invalid_recognition_setting', message: 'Use 1–8 context hints of at most 128 characters each' };
  }
  return hints;
}

export function buildRecognitionSettings(
  status: StatusSnapshot | null,
  envelope: NarrivySettingsEnvelope,
): RecognitionSettings | null {
  const values = envelope.values;
  const selected = Object.values(RECOGNITION_SETTING_KEYS).some((key) => explicitValue(values, key) !== null);
  if (!selected) return null;

  const options = recognitionOptions(status);
  if (!options.available) {
    throw { code: 'unsupported_recognition_setting', message: 'Recognition settings are unavailable for the loaded model' };
  }

  const spoken = explicitValue(values, RECOGNITION_SETTING_KEYS.spokenLanguage);
  let spokenLanguage: RecognitionSettings['spoken_language'];
  if (spoken === 'detect') {
    if (!options.languageDetection) {
      throw { code: 'unsupported_recognition_setting', message: 'Automatic language detection is unavailable' };
    }
    spokenLanguage = { kind: 'auto_detect' };
  } else if (spoken !== null) {
    spokenLanguage = { kind: 'explicit', tag: requireOption(spoken, options.languages, 'Spoken language')! };
  }

  const translationTarget = requireOption(
    explicitValue(values, RECOGNITION_SETTING_KEYS.translationTarget),
    options.translationTargets,
    'Translation target',
  );
  const outputLanguages = translationTarget ? options.translationTargets : options.languages;
  const result: RecognitionSettings = {
    spoken_language: spokenLanguage,
    output_language: requireOption(
      explicitValue(values, RECOGNITION_SETTING_KEYS.outputLanguage),
      outputLanguages,
      'Written language',
    ),
    output_script: requireOption(
      explicitValue(values, RECOGNITION_SETTING_KEYS.outputScript),
      options.scripts,
      'Output script',
    ),
    translation_target: translationTarget,
    locale: requireOption(explicitValue(values, RECOGNITION_SETTING_KEYS.locale), options.locales, 'Locale'),
    context_hints: contextHints(
      explicitValue(values, RECOGNITION_SETTING_KEYS.contextHints),
      options.contextBias,
    ),
  };
  return Object.fromEntries(Object.entries(result).filter(([, value]) => value !== undefined)) as RecognitionSettings;
}
