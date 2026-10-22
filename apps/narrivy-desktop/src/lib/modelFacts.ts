// How a model from the daemon's listing is named and described. The words
// and numbers are the catalog's; nothing here guesses a fact.
import { t } from '../i18n';
import type { ModelInfo } from './bridge';

export function languageName(code: string): string {
  if (code === 'multilingual') return t('models.multilingual');
  try {
    return new Intl.DisplayNames([navigator.language || 'en'], { type: 'language' }).of(code) ?? code;
  } catch {
    return code;
  }
}

/**
 * A reported language list (`en,ur,...`) for one line: the names when there
 * are a few, else the count (a multilingual model reports about a hundred).
 */
export function reportedLanguages(raw: string, few = 3): string {
  const codes = raw.split(/[,\s]+/).filter(Boolean);
  if (codes.length <= few) return codes.map(languageName).join(', ');
  return t('home.fact.languages', { count: codes.length });
}

/** Family and variant as one name, the family capitalised; the words are the catalog's. */
export function modelName(model: ModelInfo): string {
  if (!model.family) return t('models.unlisted');
  return `${model.family.charAt(0).toUpperCase()}${model.family.slice(1)} ${model.variant}`.trim();
}

/** "Q8_0" → "8-bit"; the digits are the publisher's, never guessed. */
export function quantBits(quant: string): string | null {
  const bits = /(?:^|[^0-9])(\d{1,2})(?!\d)/.exec(quant);
  return bits ? t('models.bits', { bits: bits[1] }) : null;
}

export function languageSummary(model: ModelInfo): string {
  if (model.languages.includes('multilingual')) return t('models.multilingual');
  if (model.languages.length === 1) return t('models.onlyLanguage', { language: languageName(model.languages[0]) });
  return model.languages.map(languageName).join(', ');
}

/** Models on this computer that a pipeline can use: in use, downloaded, or loading. */
export function usableModels(models: ModelInfo[]): ModelInfo[] {
  return models.filter((m) => m.state === 'active' || m.state === 'installed' || m.state === 'loading');
}
