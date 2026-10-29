// The complete Narrivy settings map. Every feature the product has or has
// committed to appears here with an honest status:
//
// - live:     works now and changes behaviour;
// - reported: read from the service; shown, not editable;
// - planned:  decided by the owner and scheduled (a completion-plan task),
//             not connected yet, so its control is disabled;
// - proposed: suggested for parity with peers; needs an owner decision.
//
// Reported values come only from daemon capability strings. Text lives in
// the i18n catalog under `fm.<section>.<item>.*`.
import type { StatusSnapshot } from './bridge';
import { activationPresentation } from './activation';

export type FeatureStatus = 'live' | 'reported' | 'planned' | 'proposed';

export type FeatureControl =
  | { kind: 'toggle'; on: boolean }
  | { kind: 'select'; valueKey: string }
  | { kind: 'number'; value: number; min: number; max: number; unitKey?: string }
  | { kind: 'keys'; keys: string[] }
  | { kind: 'value'; read: (status: StatusSnapshot | null) => string | null }
  | { kind: 'none' };

export interface FeatureItem {
  id: string;
  status: FeatureStatus;
  /** Completion-plan task that connects a planned item. */
  task?: string;
  control: FeatureControl;
}

/** A live card rendered by the Settings screen above a section's rows. */
export type SectionCard =
  | 'theme'
  | 'recognition'
  | 'automation'
  | 'compute'
  | 'delivery'
  | 'portal'
  | 'pipelines'
  | 'priority'
  | 'chain'
  | 'overlayPosition'
  | 'echoCancel'
  | 'voice'
  | 'startup'
  | 'start'
  | 'hold'
  | 'cancel';

export interface FeatureSection {
  id: string;
  icon: 'settings' | 'keyboard' | 'mic' | 'models' | 'cpu' | 'spark' | 'type' | 'info' | 'history' | 'shield' | 'diagnostics';
  cards: SectionCard[];
  items: FeatureItem[];
}

function cap(status: StatusSnapshot | null, prefix: string): string | null {
  const found = status?.capabilities.find((c) => c.startsWith(prefix));
  return found ? found.slice(prefix.length) : null;
}

function has(status: StatusSnapshot | null, token: string): boolean {
  return status?.capabilities.includes(token) ?? false;
}

/** Map a raw readiness value (`ready:…`, `unavailable:…`) to a short word. */
function readiness(value: string | null): string | null {
  if (value === null) return null;
  if (value === 'enabled') return 'fm.value.on';
  if (value.startsWith('ready') || value === 'available') return 'fm.value.ready';
  if (value.startsWith('unavailable') || value.startsWith('disabled')) return 'fm.value.unavailable';
  return value;
}

export const FEATURE_SECTIONS: FeatureSection[] = [
  {
    id: 'general',
    icon: 'settings',
    cards: ['theme', 'startup'],
    items: [
      { id: 'closeToTray', status: 'live', control: { kind: 'value', read: () => 'fm.value.on' } },
      { id: 'uiLanguage', status: 'live', control: { kind: 'value', read: () => 'fm.value.english' } },
    ],
  },
  {
    id: 'shortcuts',
    icon: 'keyboard',
    cards: ['start', 'hold', 'cancel'],
    items: [
      {
        id: 'toggle',
        status: 'live',
        control: {
          kind: 'value',
          read: (s) => activationPresentation(s).suggestedCommand,
        },
      },
      {
        id: 'desktopShortcuts',
        status: 'reported',
        control: { kind: 'value', read: (s) => readiness(cap(s, 'activation.portal-global-shortcuts=')) },
      },
    ],
  },
  {
    id: 'microphone',
    icon: 'mic',
    cards: [],
    items: [
      {
        id: 'capture',
        status: 'reported',
        control: {
          kind: 'value',
          read: (s) => (has(s, 'mic.available') ? 'fm.value.ready' : readiness(cap(s, 'capture.pipewire-helper='))),
        },
      },
      {
        id: 'maxLength',
        status: 'reported',
        control: {
          kind: 'value',
          read: (s) => {
            const seconds = cap(s, 'capture.max_seconds=');
            return seconds ? `${seconds} s` : null;
          },
        },
      },
      { id: 'autoStop', status: 'live', control: { kind: 'value', read: () => 'fm.value.never' } },
      { id: 'otherAudio', status: 'live', control: { kind: 'value', read: () => 'fm.value.untouched' } },
    ],
  },
  {
    id: 'speech',
    icon: 'models',
    cards: ['recognition'],
    items: [
      { id: 'model', status: 'planned', task: 'T2.17', control: { kind: 'select', valueKey: 'fm.value.automatic' } },
      {
        id: 'installed',
        status: 'reported',
        control: { kind: 'value', read: (s) => (s?.installed_model ? `fm.model.${s.installed_model}` : null) },
      },
      { id: 'languages', status: 'reported', control: { kind: 'value', read: (s) => cap(s, 'asr.languages=') } },
      {
        id: 'detectLanguage',
        status: 'reported',
        control: { kind: 'value', read: (s) => readiness(cap(s, 'asr.language-detection=')) },
      },
      {
        id: 'liveText',
        status: 'reported',
        control: {
          kind: 'value',
          read: (s) => {
            const streaming = cap(s, 'asr.transcribe-cpp.streaming=');
            if (streaming === null) return null;
            return streaming.startsWith('disabled') || streaming.startsWith('unavailable')
              ? 'fm.value.afterStop'
              : 'fm.value.whileSpeaking';
          },
        },
      },
    ],
  },
  {
    id: 'compute',
    icon: 'cpu',
    cards: ['automation', 'priority', 'compute'],
    items: [
      {
        id: 'ladder',
        status: 'reported',
        control: {
          kind: 'value',
          read: (s) => {
            // A ready engine attests the device its model runs on.
            const running = cap(s, 'asr.transcribe-cpp.device=');
            if (running === 'cpu' || running === 'igpu' || running === 'dgpu') return `strip.device.${running}`;
            const device = cap(s, 'compute.device=');
            if (device === null) return null;
            return device.startsWith('unavailable') ? 'fm.value.cpuOnly' : device;
          },
        },
      },
      { id: 'gpu', status: 'planned', task: 'T2.15', control: { kind: 'toggle', on: true } },
      { id: 'npu', status: 'planned', task: 'T2.16', control: { kind: 'toggle', on: true } },
    ],
  },
  {
    id: 'modes',
    icon: 'spark',
    cards: ['pipelines'],
    items: [
      {
        id: 'available',
        status: 'reported',
        control: {
          kind: 'value',
          read: (s) => {
            const modes = (s?.capabilities ?? [])
              .filter((c) => c.startsWith('pipeline.') && c.endsWith('=enabled'))
              .map((c) => c.slice('pipeline.'.length, -'=enabled'.length));
            return modes.length > 0 ? modes.join(', ') : null;
          },
        },
      },
      { id: 'aliases', status: 'planned', task: 'T2.14', control: { kind: 'none' } },
      {
        id: 'vocabulary',
        status: 'reported',
        control: { kind: 'value', read: (s) => readiness(cap(s, 'asr.context-bias=')) },
      },
      { id: 'punctuation', status: 'proposed', control: { kind: 'toggle', on: false } },
    ],
  },
  {
    id: 'insertion',
    icon: 'type',
    cards: ['chain', 'delivery', 'portal'],
    items: [
      { id: 'method', status: 'planned', task: 'T2.4', control: { kind: 'select', valueKey: 'fm.value.automatic' } },
      {
        id: 'typing',
        status: 'reported',
        control: { kind: 'value', read: (s) => readiness(cap(s, 'delivery.wtype-helper=')) },
      },
      {
        id: 'inbox',
        status: 'reported',
        control: { kind: 'value', read: (s) => readiness(cap(s, 'delivery.inbox-recovery=')) },
      },
      { id: 'paste', status: 'planned', task: 'T2.4', control: { kind: 'toggle', on: false } },
      { id: 'restoreClipboard', status: 'planned', task: 'T2.4', control: { kind: 'toggle', on: true } },
      { id: 'trailingSpace', status: 'proposed', control: { kind: 'toggle', on: false } },
    ],
  },
  {
    id: 'overlay',
    icon: 'info',
    cards: ['overlayPosition'],
    items: [
      { id: 'focus', status: 'live', control: { kind: 'value', read: () => 'fm.value.never' } },
      { id: 'show', status: 'planned', task: 'T2.18', control: { kind: 'toggle', on: true } },
      { id: 'monitor', status: 'live', control: { kind: 'value', read: () => 'fm.value.pointerMonitor' } },
    ],
  },
  {
    id: 'history',
    icon: 'history',
    cards: [],
    items: [
      {
        id: 'enabled',
        status: 'reported',
        control: { kind: 'value', read: (s) => readiness(cap(s, 'history.optional=')) },
      },
      {
        id: 'currentLimit',
        status: 'reported',
        control: {
          kind: 'value',
          read: (s) => {
            const max = cap(s, 'history.max_entries=');
            const retention = cap(s, 'history.retention=');
            if (!max && !retention) return null;
            return [max ? `${max}` : null, retention].filter(Boolean).join(' · ');
          },
        },
      },
    ],
  },
  {
    id: 'privacy',
    icon: 'shield',
    cards: [],
    items: [
      {
        id: 'localOnly',
        status: 'reported',
        control: { kind: 'value', read: (s) => readiness(cap(s, 'policy.local-only=')) },
      },
      { id: 'recovery', status: 'live', control: { kind: 'value', read: () => 'fm.value.fifteenMinutes' } },
      { id: 'logs', status: 'live', control: { kind: 'value', read: () => 'fm.value.contentFree' } },
    ],
  },
  {
    id: 'advanced',
    icon: 'diagnostics',
    cards: [],
    items: [
      {
        id: 'service',
        status: 'reported',
        control: { kind: 'value', read: (s) => s?.daemon_build ?? null },
      },
      {
        id: 'supportReport',
        status: 'reported',
        control: { kind: 'value', read: (s) => readiness(cap(s, 'diagnostics.support-report=')) },
      },
      { id: 'logLevel', status: 'planned', task: 'T2.12', control: { kind: 'select', valueKey: 'fm.value.info' } },
    ],
  },
];
