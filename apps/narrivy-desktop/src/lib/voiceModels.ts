// The voice packs (text-to-speech models) voice mode can read replies with,
// as the service's voice settings list them, and which one is in use.

export interface VoiceModel {
  id: string;
  name: string;
  language: 'en' | 'ur';
  installed: boolean;
}

export interface VoiceModelChoice {
  englishModel: string;
  urduModel: string;
  models: VoiceModel[];
}

export function readVoiceModels(value: unknown): VoiceModelChoice {
  const raw = (value ?? {}) as { englishModel?: unknown; urduModel?: unknown; models?: unknown };
  const models = Array.isArray(raw.models)
    ? raw.models.flatMap((item): VoiceModel[] => {
        const m = item as Partial<VoiceModel>;
        return typeof m.id === 'string' && (m.language === 'en' || m.language === 'ur')
          ? [{ id: m.id, name: typeof m.name === 'string' ? m.name : m.id, language: m.language, installed: Boolean(m.installed) }]
          : [];
      })
    : [];
  return {
    englishModel: typeof raw.englishModel === 'string' ? raw.englishModel : '',
    urduModel: typeof raw.urduModel === 'string' ? raw.urduModel : '',
    models,
  };
}

/** The pack that is read now for `language` (the chosen one if it is installed). */
export function modelInUse(choice: VoiceModelChoice, language: 'en' | 'ur'): VoiceModel | null {
  const chosen = language === 'en' ? choice.englishModel : choice.urduModel;
  if (language === 'ur' && chosen === 'off') return null;
  const installed = choice.models.filter((m) => m.language === language && m.installed);
  return installed.find((m) => m.id === chosen) ?? installed[0] ?? null;
}
