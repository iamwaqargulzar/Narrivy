// Derive human-readable resolved values ("Auto → …") for Home cards.
// Inputs are ONLY the daemon status snapshot (advertised capabilities plus the
// typed installed-model lifecycle) and the local auto/custom setting, never OS
// probing from the frontend. A value is only claimed when the daemon reports
// the capability that supports it; otherwise the card says so honestly.
import type { StatusSnapshot } from './bridge';

export interface ResolvedCardValue {
  resolved: string;
  source: string;
}

function compose(auto: boolean, value: string): string {
  return auto ? `Auto → ${value}` : value;
}

/** Value of the first advertised capability starting with `prefix`. */
function capability(status: StatusSnapshot, prefix: string): string | null {
  const found = (status.capabilities ?? []).find((c) => c.startsWith(prefix));
  return found ? found.slice(prefix.length) : null;
}

function withoutUnavailablePrefix(value: string): string {
  return value.startsWith('unavailable:') ? value.slice('unavailable:'.length) : value;
}

export function resolveCompute(status: StatusSnapshot, mode: 'auto' | 'custom'): ResolvedCardValue {
  const caps = status.capabilities ?? [];
  if (mode !== 'auto') return { resolved: 'CPU (chosen manually)', source: 'settings: compute.device' };
  if (caps.includes('asr.transcribe-cpp')) {
    return { resolved: compose(true, 'CPU (transcribe.cpp)'), source: 'capability: asr.transcribe-cpp' };
  }
  const cpu = capability(status, 'asr.transcribe-cpp.cpu=');
  if (cpu) {
    if (cpu === 'available') {
      const runtime = capability(status, 'asr.transcribe-cpp.runtime=');
      const value = runtime ? `CPU (transcribe.cpp, ${runtime})` : 'CPU (transcribe.cpp)';
      return { resolved: compose(true, value), source: 'capability: asr.transcribe-cpp.cpu=available' };
    }
    return {
      resolved: compose(true, `Unavailable (${withoutUnavailablePrefix(cpu)})`),
      source: `capability: asr.transcribe-cpp.cpu=${cpu}`,
    };
  }
  return { resolved: compose(true, 'Not available'), source: 'no compute capability reported' };
}

export function resolveModel(status: StatusSnapshot, mode: 'auto' | 'custom'): ResolvedCardValue {
  if (mode !== 'auto') return { resolved: 'Manually selected model', source: 'settings: asr.model' };
  const caps = status.capabilities ?? [];
  switch (status.installed_model) {
    case 'ready':
      return { resolved: compose(true, localEngineLabel(status)), source: 'lifecycle: installed model ready' };
    case 'awaiting_model':
      return {
        resolved: compose(true, 'Not installed — install the bundled model'),
        source: 'lifecycle: awaiting_model',
      };
    case 'provisioning':
      return { resolved: compose(true, 'Installing bundled model…'), source: 'lifecycle: provisioning' };
    case 'activation_failed':
      return { resolved: compose(true, 'Activation failed'), source: 'lifecycle: activation_failed' };
    case 'published_unconfirmed':
      return {
        resolved: compose(true, 'Published (not confirmed)'),
        source: 'lifecycle: published_unconfirmed',
      };
    default:
      break;
  }
  const model = capability(status, 'model.gguf=');
  if (model) {
    if (model === 'available') {
      return { resolved: compose(true, 'Local GGUF (transcribe.cpp)'), source: 'capability: model.gguf=available' };
    }
    const reason = withoutUnavailablePrefix(model);
    if (reason.includes('installed_model_missing') || reason.includes('model_not_started')) {
      return {
        resolved: compose(true, 'Not installed — install the bundled model'),
        source: `capability: model.gguf=${model}`,
      };
    }
    return { resolved: compose(true, `Unavailable (${reason})`), source: `capability: model.gguf=${model}` };
  }
  if (caps.includes('asr.transcribe-cpp')) {
    return { resolved: compose(true, 'Local GGUF (transcribe.cpp)'), source: 'capability: asr.transcribe-cpp' };
  }
  if (caps.includes('mock.asr')) {
    return { resolved: compose(true, 'Built-in test model'), source: 'capability: mock.asr' };
  }
  return { resolved: compose(true, 'Not available'), source: 'no ASR capability reported' };
}

function localEngineLabel(status: StatusSnapshot): string {
  const caps = status.capabilities ?? [];
  if (caps.includes('asr.transcribe-cpp') || capability(status, 'model.gguf=') === 'available') {
    return 'Local GGUF (transcribe.cpp)';
  }
  if (caps.includes('mock.asr')) return 'Built-in test model';
  return 'Installed model';
}

export function resolveMic(status: StatusSnapshot, mode: 'auto' | 'custom'): ResolvedCardValue {
  const caps = status.capabilities ?? [];
  if (mode !== 'auto') return { resolved: 'Chosen input device', source: 'settings: audio.input_device' };
  if (caps.includes('mic.available')) {
    return { resolved: compose(true, 'System default microphone'), source: 'capability: mic.available' };
  }
  const helper = capability(status, 'capture.pipewire-helper=');
  if (helper) {
    if (helper.startsWith('ready:')) {
      return {
        resolved: compose(true, 'PipeWire capture helper (signal unverified)'),
        source: 'capability: capture.pipewire-helper=ready',
      };
    }
    return {
      resolved: compose(true, `No microphone capture (${withoutUnavailablePrefix(helper)})`),
      source: `capability: capture.pipewire-helper=${helper}`,
    };
  }
  return { resolved: compose(true, 'No microphone detected'), source: 'missing capability: mic.available' };
}

export function resolveDelivery(status: StatusSnapshot, mode: 'auto' | 'custom'): ResolvedCardValue {
  const caps = status.capabilities ?? [];
  if (mode !== 'auto') return { resolved: 'Manually chosen delivery', source: 'settings: delivery.mode' };
  if (caps.includes('delivery.clipboard')) {
    return { resolved: compose(true, 'Clipboard paste'), source: 'capability: delivery.clipboard' };
  }
  // Target-aware routed lane: the daemon advertises a bounded ladder of
  // qualified backends. No advertised token may claim verified insertion, so
  // the card names routing and the unverified attempt honestly. When the
  // daemon reports no pre-activation target binding, the card must not imply
  // that activation-requiring rungs can run.
  if (capability(status, 'delivery.route=') === 'target-aware') {
    const helper = capability(status, 'delivery.wtype-helper=');
    const helperReady = helper?.startsWith('ready:') ?? false;
    const clipboard = capability(status, 'delivery.backend.clipboard=');
    const clipboardAuthorized = clipboard?.startsWith('unverified:') ?? false;
    const targetBound = capability(status, 'delivery.target-binding=')?.startsWith('bound') ?? false;
    const targetNote = targetBound ? '' : ', no pre-activation target bound';
    const detail = helperReady
      ? `target-aware routing${targetNote}, direct typing unverified`
      : `target-aware routing${targetNote}, typing helper unavailable`;
    return {
      resolved: compose(true, `Inbox recovery (${detail})${clipboardAuthorized ? ' — clipboard restore authorized' : ''}`),
      source: 'capability: delivery.route=target-aware',
    };
  }
  const inboxOnly = caps.includes('delivery.inbox-only=available');
  const inboxRecovery = caps.includes('delivery.inbox-recovery=enabled');
  const wtypeHelper = capability(status, 'delivery.wtype-helper=');
  if (inboxRecovery) {
    const helperReady = wtypeHelper?.startsWith('ready:') ?? false;
    return {
      resolved: compose(
        true,
        helperReady
          ? 'Inbox recovery (direct typing unverified)'
          : 'Inbox recovery only (typing helper unavailable)',
      ),
      source: 'capability: delivery.inbox-recovery=enabled',
    };
  }
  if (inboxOnly) {
    return {
      resolved: compose(true, 'Inbox recovery only — no keyboard paste'),
      source: 'capability: delivery.inbox-only=available',
    };
  }
  if (caps.includes('inbox.delivery')) {
    return { resolved: compose(true, 'Inbox recovery only — no keyboard paste'), source: 'capability: inbox.delivery' };
  }
  if (caps.includes('wtype.inbox-recovery.delivery')) {
    return { resolved: compose(true, 'Inbox recovery (direct typing unverified)'), source: 'capability: wtype.inbox-recovery.delivery' };
  }
  if (caps.includes('platform.delivery.router')) {
    return { resolved: compose(true, 'Platform delivery router'), source: 'capability: platform.delivery.router' };
  }
  if (caps.includes('null.delivery')) {
    return { resolved: compose(true, 'No delivery backend (null sink)'), source: 'capability: null.delivery' };
  }
  return { resolved: compose(true, 'Unavailable'), source: 'no delivery capability reported' };
}
