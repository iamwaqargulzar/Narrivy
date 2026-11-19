// The Settings sections. Each holds only settings that work and change
// behaviour now. What a pipeline does (microphone, voice, model and device,
// clean-up, delivery) is set per pipeline in Pipelines; models are in the
// Store; history in History; versions in About. Nothing here is a
// placeholder or a read-only copy of a service value.
//
// Text lives in the i18n catalog under `fm.<section>.title` / `.desc`.

/** A card rendered by the Settings screen. */
export type SectionCard =
  | 'theme'
  | 'startup'
  | 'start'
  | 'hold'
  | 'cancel'
  | 'overlayPosition'
  | 'extensions'
  | 'portal';

export interface FeatureSection {
  id: string;
  icon: 'settings' | 'apps' | 'keyboard' | 'type' | 'shield';
  /** Shown only when the desktop offers what it sets. */
  when?: 'portal';
  cards: SectionCard[];
}

export const FEATURE_SECTIONS: FeatureSection[] = [
  { id: 'general', icon: 'settings', cards: ['theme', 'startup'] },
  { id: 'shortcuts', icon: 'keyboard', cards: ['start', 'hold', 'cancel'] },
  { id: 'overlay', icon: 'type', cards: ['overlayPosition'] },
  { id: 'extensions', icon: 'apps', cards: ['extensions'] },
  // The desktop's keyboard permission for typing text (portal), where offered.
  { id: 'permissions', icon: 'shield', cards: ['portal'], when: 'portal' },
];
