// Policy-free pipeline catalog and selection logic shared by Home, Dictation,
// the command palette and onboarding.
//
// Inputs are ONLY the daemon `request.list_pipelines` reply plus the daemon
// status snapshot. This module never names an OS, never hardcodes a lane id,
// and never decides correction or delivery policy: it renders what the daemon
// advertises and keeps the user's selection stable across refreshes.
import type { PipelineSummary, StatusSnapshot } from './bridge';

export type PipelineAvailability =
  | 'loading'
  | 'error'
  | 'empty'
  | 'awaiting_model'
  | 'unavailable'
  | 'ready';

export interface PipelineCatalog {
  availability: PipelineAvailability;
  /** Every advertised pipeline, enabled or not. */
  pipelines: PipelineSummary[];
  /** Advertised pipelines the daemon reports as currently startable. */
  enabled: PipelineSummary[];
  /** Selected pipeline id; preserved across refreshes while still enabled. */
  selectedId: string | null;
  /** Daemon-reported token explaining a non-ready state, when present. */
  reason: string | null;
  /** True when no lane is startable because its model is not provisioned. */
  modelBlocked: boolean;
}

export function isPipelineEnabled(pipelines: PipelineSummary[], id: string | null): boolean {
  if (!id) return false;
  return pipelines.some((p) => p.id === id && p.enabled);
}

export function isPipelineAdvertised(pipelines: PipelineSummary[], id: string | null): boolean {
  if (!id) return false;
  return pipelines.some((p) => p.id === id);
}

/**
 * Keep the current selection across refreshes when the daemon still advertises
 * it as startable; otherwise fall back to the first enabled pipeline; never
 * invent an id the daemon did not advertise.
 */
export function preserveSelection(
  previous: string | null,
  pipelines: PipelineSummary[],
): string | null {
  if (isPipelineEnabled(pipelines, previous)) return previous;
  const firstEnabled = pipelines.find((p) => p.enabled);
  return firstEnabled ? firstEnabled.id : null;
}

const MODEL_CAPABILITY_PREFIX = 'model.gguf=';

/** Daemon-reported model readiness token, from lifecycle first, then capability. */
export function modelReadinessToken(status: StatusSnapshot | null): string | null {
  const lifecycle = status?.installed_model;
  if (lifecycle) return lifecycle;
  const cap = (status?.capabilities ?? []).find((c) => c.startsWith(MODEL_CAPABILITY_PREFIX));
  if (!cap) return null;
  const value = cap.slice(MODEL_CAPABILITY_PREFIX.length);
  return value.startsWith('unavailable:') ? value.slice('unavailable:'.length) : value;
}

/** First capability token explaining why a lane is not startable. */
export function pipelineBlockingReason(status: StatusSnapshot | null): string | null {
  const caps = status?.capabilities ?? [];
  const segments: Array<[string, string]> = [
    ['recovery.static-readiness=', ''],
    ['capture.pipewire-helper=', 'unavailable:'],
    [`asr.transcribe-cpp.cpu=`, 'unavailable:'],
    [MODEL_CAPABILITY_PREFIX, 'unavailable:'],
  ];
  for (const [prefix, unavailablePrefix] of segments) {
    for (const cap of caps) {
      if (!cap.startsWith(prefix)) continue;
      const value = cap.slice(prefix.length);
      if (unavailablePrefix) {
        if (!value.startsWith(unavailablePrefix)) continue;
        return value.slice(unavailablePrefix.length);
      }
      if (value !== 'ready' && value !== 'available') return value;
    }
  }
  return null;
}

/** True when the reported model token means the bundled model is not active. */
export function isModelUnprovisioned(status: StatusSnapshot | null): boolean {
  const token = modelReadinessToken(status);
  if (!token) return false;
  return (
    token === 'awaiting_model' ||
    token === 'provisioning' ||
    token.includes('installed_model_missing') ||
    token.includes('resident_not_initialized') ||
    token.includes('model_not_started')
  );
}

export function buildPipelineCatalog(args: {
  loading: boolean;
  failed: boolean;
  pipelines: PipelineSummary[];
  selectedId: string | null;
  status: StatusSnapshot | null;
}): PipelineCatalog {
  const { loading, failed, pipelines, selectedId, status } = args;
  const enabled = pipelines.filter((p) => p.enabled);
  const base = { pipelines, enabled, selectedId, reason: null as string | null, modelBlocked: false };

  if (loading) return { ...base, availability: 'loading' };
  if (failed) return { ...base, availability: 'error', reason: null };
  if (pipelines.length === 0) return { ...base, availability: 'empty' };
  if (enabled.length === 0) {
    const reason = modelReadinessToken(status) ?? pipelineBlockingReason(status);
    const modelBlocked = isModelUnprovisioned(status);
    return {
      ...base,
      availability: modelBlocked ? 'awaiting_model' : 'unavailable',
      reason,
      modelBlocked,
    };
  }
  return { ...base, availability: 'ready', reason: pipelineBlockingReason(status) };
}

/** i18n key for the non-ready catalog state; null when startable. */
export function availabilityMessageKey(catalog: PipelineCatalog): string | null {
  switch (catalog.availability) {
    case 'loading':
      return 'pipelines.loading';
    case 'error':
      return 'pipelines.error';
    case 'empty':
      return 'pipelines.empty';
    case 'awaiting_model':
      return 'pipelines.awaitingModel';
    case 'unavailable':
      return 'pipelines.unavailable';
    case 'ready':
      return null;
  }
}
