// Local settings envelope persisted at localStorage['narrivy.settings.v1'].
// Every knob defaults to the sentinel 'auto'; explicit choices overwrite it
// and can be reset back to auto. Resolved values are derived ONLY from
// daemon capability strings (see resolved.ts).
import { typingUnavailable } from './resolved';

export const SETTINGS_STORAGE_KEY = 'narrivy.settings.v1';

export type SettingValue = 'auto' | string | number | boolean;

export interface NarrivySettingsEnvelope {
  schema_version: 1;
  values: Record<string, SettingValue>;
}

export const SECTION_ORDER = [
  'general',
  'automation',
  'compute',
  'asr',
  'delivery',
  'audio',
  'formatting',
  'privacy',
  'advanced',
] as const;

export type SectionId = (typeof SECTION_ORDER)[number];

export interface ChoiceOption {
  value: string;
  labelKey: string;
}

export interface FieldSpec {
  key: string;
  section: SectionId;
  labelKey: string;
  kind: 'choice' | 'toggle';
  options?: ChoiceOption[];
  /** Resolve the effective value of 'auto' from daemon capability strings. */
  resolve?: (caps: string[]) => string;
  /** Fallback resolution when no capability applies. */
  resolvedFallback: string;
}

export const FIELD_SPECS: FieldSpec[] = [
  // General
  { key: 'general.language', section: 'general', labelKey: 'settings.field.general.language', kind: 'choice',
    options: [{ value: 'auto', labelKey: 'settings.opt.auto' }, { value: 'en', labelKey: 'settings.opt.en' }],
    resolvedFallback: 'English (system)' },
  { key: 'general.launch_startup', section: 'general', labelKey: 'settings.field.general.startup', kind: 'toggle',
    resolvedFallback: 'Disabled' },
  // Automation
  { key: 'automation.copy_on_finalize', section: 'automation', labelKey: 'settings.field.automation.copyOnFinalize', kind: 'toggle',
    resolve: (caps) => (caps.includes('delivery.clipboard') ? 'Enabled (clipboard ready)' : 'Disabled (no clipboard capability)'),
    resolvedFallback: 'Disabled' },
  { key: 'automation.pause_on_idle', section: 'automation', labelKey: 'settings.field.automation.pauseOnIdle', kind: 'toggle',
    resolvedFallback: 'Disabled' },
  // Compute
  { key: 'compute.device', section: 'compute', labelKey: 'settings.field.compute.device', kind: 'choice',
    options: [{ value: 'auto', labelKey: 'settings.opt.auto' }, { value: 'cpu', labelKey: 'settings.opt.cpu' }],
    resolve: (caps) =>
      caps.includes('asr.transcribe-cpp') || caps.includes('asr.transcribe-cpp.cpu=available')
        ? 'CPU (transcribe.cpp)'
        : 'No local engine detected',
    resolvedFallback: 'No local engine detected' },
  { key: 'compute.threads', section: 'compute', labelKey: 'settings.field.compute.threads', kind: 'choice',
    options: [{ value: 'auto', labelKey: 'settings.opt.auto' }, { value: '2', labelKey: 'settings.opt.twoThreads' }, { value: '4', labelKey: 'settings.opt.fourThreads' }],
    resolvedFallback: 'Half of logical cores' },
  // ASR
  { key: 'asr.model', section: 'asr', labelKey: 'settings.field.asr.model', kind: 'choice',
    options: [{ value: 'auto', labelKey: 'settings.opt.auto' }],
    resolve: (caps) => {
      if (caps.includes('asr.transcribe-cpp')) return 'Local GGUF (transcribe.cpp)';
      if (caps.includes('model.gguf=available') || caps.includes('asr.transcribe-cpp.cpu=available')) {
        return 'Local GGUF (transcribe.cpp)';
      }
      if (caps.some((c) => c.startsWith('model.gguf=unavailable:'))) {
        return 'Not installed — install the bundled model';
      }
      if (caps.includes('mock.asr')) return 'Built-in test model';
      return 'None detected';
    },
    resolvedFallback: 'None detected' },
  // Text Delivery
  { key: 'delivery.mode', section: 'delivery', labelKey: 'settings.field.delivery.mode', kind: 'choice',
    options: [{ value: 'auto', labelKey: 'settings.opt.auto' }, { value: 'paste', labelKey: 'settings.opt.paste' }, { value: 'copy', labelKey: 'settings.opt.copy' }],
    resolve: (caps) => {
      if (caps.includes('delivery.clipboard')) return 'Clipboard paste';
      if (caps.includes('delivery.route=target-aware')) {
        const helper = caps.find((c) => c.startsWith('delivery.wtype-helper='));
        return helper?.startsWith('delivery.wtype-helper=ready:')
          ? 'Inbox recovery (target-aware routing, typing unverified)'
          : `Inbox recovery only (target-aware routing, ${typingUnavailable(
              helper?.slice('delivery.wtype-helper='.length) ?? null,
            )})`;
      }
      if (
        caps.includes('delivery.inbox-only=available') ||
        caps.includes('inbox.delivery') ||
        caps.includes('delivery.inbox-recovery=enabled') ||
        caps.includes('wtype.inbox-recovery.delivery')
      ) {
        return 'Inbox recovery only — no keyboard paste';
      }
      if (caps.includes('null.delivery')) return 'No delivery backend (null sink)';
      if (caps.includes('platform.delivery.router')) return 'Platform delivery router';
      return 'Unavailable';
    },
    resolvedFallback: 'Unavailable' },
  { key: 'delivery.paste_delay_ms', section: 'delivery', labelKey: 'settings.field.delivery.pasteDelay', kind: 'choice',
    options: [{ value: 'auto', labelKey: 'settings.opt.auto' }, { value: '50', labelKey: 'settings.opt.ms50' }, { value: '150', labelKey: 'settings.opt.ms150' }],
    resolvedFallback: '100 ms' },
  // Audio
  { key: 'audio.input_device', section: 'audio', labelKey: 'settings.field.audio.inputDevice', kind: 'choice',
    options: [{ value: 'auto', labelKey: 'settings.opt.auto' }, { value: 'default', labelKey: 'settings.opt.defaultDevice' }],
    resolve: (caps) => (caps.includes('mic.available') ? 'System default microphone' : 'None detected'),
    resolvedFallback: 'None detected' },
  { key: 'audio.noise_gate', section: 'audio', labelKey: 'settings.field.audio.noiseGate', kind: 'toggle',
    resolvedFallback: 'Off' },
  // Formatting
  { key: 'formatting.punctuation', section: 'formatting', labelKey: 'settings.field.formatting.punctuation', kind: 'toggle',
    resolvedFallback: 'On' },
  { key: 'formatting.spoken_punctuation', section: 'formatting', labelKey: 'settings.field.formatting.spokenCommands', kind: 'toggle',
    resolvedFallback: 'Off' },
  // Privacy
  { key: 'privacy.retention', section: 'privacy', labelKey: 'settings.field.privacy.retention', kind: 'choice',
    options: [{ value: 'auto', labelKey: 'settings.opt.auto' }, { value: 'session', labelKey: 'settings.opt.sessionOnly' }],
    resolvedFallback: 'Keep until cleared' },
  { key: 'privacy.local_only', section: 'privacy', labelKey: 'settings.field.privacy.localOnly', kind: 'toggle',
    resolve: (caps) => (caps.some((c) => c.startsWith('asr.')) ? 'Enforced for available engines' : 'No engines installed'),
    resolvedFallback: 'Enforced' },
  // Advanced (last, progressive disclosure)
  { key: 'advanced.log_level', section: 'advanced', labelKey: 'settings.field.advanced.logLevel', kind: 'choice',
    options: [{ value: 'auto', labelKey: 'settings.opt.auto' }, { value: 'info', labelKey: 'settings.opt.info' }, { value: 'debug', labelKey: 'settings.opt.debug' }],
    resolvedFallback: 'info' },
];

export const FIELDS_BY_SECTION: Record<SectionId, FieldSpec[]> = SECTION_ORDER.reduce(
  (acc, section) => {
    acc[section] = FIELD_SPECS.filter((f) => f.section === section);
    return acc;
  },
  {} as Record<SectionId, FieldSpec[]>,
);

export const DEFAULT_SETTINGS: NarrivySettingsEnvelope = {
  schema_version: 1,
  values: {
    ...Object.fromEntries(FIELD_SPECS.map((f) => [f.key, 'auto' as const])),
    'asr.language': 'auto',
    'asr.output_language': 'auto',
    'asr.output_script': 'auto',
    'asr.translation_target': 'auto',
    'asr.locale': 'auto',
    'asr.context_hints': 'auto',
  },
};

export function loadSettings(): NarrivySettingsEnvelope {
  try {
    const raw = window.localStorage.getItem(SETTINGS_STORAGE_KEY);
    if (!raw) return structuredClone(DEFAULT_SETTINGS);
    const parsed = JSON.parse(raw) as Partial<NarrivySettingsEnvelope>;
    if (parsed.schema_version !== 1 || typeof parsed.values !== 'object' || parsed.values === null) {
      return structuredClone(DEFAULT_SETTINGS);
    }
    return { schema_version: 1, values: { ...structuredClone(DEFAULT_SETTINGS).values, ...parsed.values } };
  } catch {
    return structuredClone(DEFAULT_SETTINGS);
  }
}

export function saveSettings(env: NarrivySettingsEnvelope): void {
  try {
    window.localStorage.setItem(SETTINGS_STORAGE_KEY, JSON.stringify(env));
  } catch {
    // storage unavailable; settings stay in-memory for this run
  }
}
