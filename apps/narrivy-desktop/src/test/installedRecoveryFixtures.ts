// Installed strict-recovery daemon fixtures for frontend tests.
//
// `INSTALLED_RECOVERY_*` mirrors the accepted installed default:
// `NARRIVY_RECOVERY_MODE=linux-mic-inbox` + `NARRIVY_DELIVERY_BACKEND=inbox`
// Inbox-only lane, which remains available and safe.
// `ROUTED_RECOVERY_*` mirrors the explicitly configured target-aware routed
// lane (`NARRIVY_RECOVERY_MODE=linux-mic-routed-inbox` with
// `NARRIVY_DELIVERY_BACKEND=routed-inbox`), kept selectable by configuration
// for controlled/native qualification only: the same two real built-in modes
// plus the routed delivery tokens, no verified-insertion claim, no
// pre-activation target binding yet, and no accepted clipboard helper. Both are
// fixtures, never product logic: components must still consume whatever the
// bridge returns.
import type { PipelineSummary, StatusSnapshot } from '../lib/bridge';

export const CLEAN_MODE_ID = 'clean';
export const RAW_MODE_ID = 'raw';

export const CLEAN_MODE: PipelineSummary = {
  id: CLEAN_MODE_ID,
  name: 'Clean dictation',
  version: 1,
  enabled: true,
};

export const RAW_MODE: PipelineSummary = {
  id: RAW_MODE_ID,
  name: 'Raw dictation',
  version: 1,
  enabled: true,
};

/** Daemon advertisement order: the configured default mode first. */
export const STRICT_MODES_ENABLED: PipelineSummary[] = [CLEAN_MODE, RAW_MODE];

export const STRICT_MODES_DISABLED: PipelineSummary[] = STRICT_MODES_ENABLED.map((mode) => ({
  ...mode,
  enabled: false,
}));

export const INSTALLED_RECOVERY_AWAITING_CAPABILITIES = [
  'capture.pipewire-helper=ready:system-owned-revalidated',
  'capture.pipewire-signal=unverified',
  'capture.window_seconds=300',
  'capture.max_seconds=300',
  'model.gguf=unavailable:installed_model_missing',
  'asr.transcribe-cpp.cpu=unavailable:installed_model_missing',
  'asr.transcribe-cpp.runtime=resident-worker',
  'policy.local-only=enabled',
  'cancellation.shared=available',
  'recovery.behavioral_qualification=unqualified',
  'pipeline.clean=unavailable',
  'pipeline.raw=unavailable',
  'recovery.static-readiness=ready',
  'delivery.inbox-only=available',
];

export const INSTALLED_RECOVERY_READY_CAPABILITIES =
  INSTALLED_RECOVERY_AWAITING_CAPABILITIES.map((token) => {
    if (token === 'model.gguf=unavailable:installed_model_missing') return 'model.gguf=available';
    if (token === 'asr.transcribe-cpp.cpu=unavailable:installed_model_missing') {
      return 'asr.transcribe-cpp.cpu=available';
    }
    if (token === 'pipeline.clean=unavailable') return 'pipeline.clean=enabled';
    if (token === 'pipeline.raw=unavailable') return 'pipeline.raw=enabled';
    return token;
  });

export const INSTALLED_RECOVERY_AWAITING_STATUS: StatusSnapshot = {
  capabilities: [...INSTALLED_RECOVERY_AWAITING_CAPABILITIES],
  active_session: null,
  daemon_build: 'narrivyd 0.3.0',
  installed_model: 'awaiting_model',
};

export const INSTALLED_RECOVERY_READY_STATUS: StatusSnapshot = {
  capabilities: [...INSTALLED_RECOVERY_READY_CAPABILITIES],
  active_session: { state: 'idle', pipeline_id: null },
  daemon_build: 'narrivyd 0.3.0',
  installed_model: 'ready',
};

export function installedRecoveryStatus(
  overrides: Partial<StatusSnapshot> = {},
): StatusSnapshot {
  return { ...INSTALLED_RECOVERY_AWAITING_STATUS, ...overrides };
}

export function readyRecoveryStatus(overrides: Partial<StatusSnapshot> = {}): StatusSnapshot {
  return { ...INSTALLED_RECOVERY_READY_STATUS, ...overrides };
}

/**
 * Explicitly configured routed lane for controlled/native qualification (the
 * packaged default stays on the accepted Inbox lane): the same canonical modes
 * over the target-aware ladder. The daemon advertises its mechanisms and
 * evidence only: routing is composed, verification is unavailable, the
 * pre-activation target binding is still missing (so target-requiring rungs are
 * skipped), and no trusted clipboard helper is accepted
 * (the packaged unit unsets `NARRIVY_CLIPBOARD_AUTHORIZED`).
 */
export const ROUTED_RECOVERY_CAPABILITIES = [
  'capture.pipewire-helper=ready:system-owned-revalidated',
  'capture.pipewire-signal=unverified',
  'capture.window_seconds=30',
  'model.gguf=available',
  'asr.transcribe-cpp.cpu=available',
  'asr.transcribe-cpp.runtime=resident-worker',
  'policy.local-only=enabled',
  'cancellation.shared=available',
  'recovery.behavioral_qualification=unqualified',
  'delivery.route=target-aware',
  'delivery.verified=unavailable',
  'delivery.inbox-recovery=enabled',
  'delivery.target-binding=unavailable:pre_activation_target_binding_absent',
  'delivery.wtype-helper=ready:system-owned-revalidated',
  'delivery.wtype-submission=unverified',
  'delivery.backend.portal-eis=unverified:permission-gated',
  'delivery.backend.clipboard=unavailable:clipboard_not_authorized',
  'delivery.clipboard-restore=unavailable:clipboard_not_authorized',
  'delivery.rung.clipboard-paste=unavailable:clipboard_not_authorized',
  'delivery.rung.clipboard-copy=unavailable:clipboard_not_authorized',
  'pipeline.clean=enabled',
  'pipeline.raw=enabled',
  'recovery.static-readiness=ready',
];

/** Same routed composition with no trustworthy typing helper in this session. */
export const ROUTED_RECOVERY_NO_HELPER_CAPABILITIES = ROUTED_RECOVERY_CAPABILITIES.map(
  (token) =>
    token === 'delivery.wtype-helper=ready:system-owned-revalidated'
      ? 'delivery.wtype-helper=unavailable:wtype_missing'
      : token,
);

export const ROUTED_RECOVERY_READY_STATUS: StatusSnapshot = {
  capabilities: [...ROUTED_RECOVERY_CAPABILITIES],
  active_session: { state: 'idle', pipeline_id: null },
  daemon_build: 'narrivyd 0.3.0',
  installed_model: 'ready',
};

export function routedRecoveryStatus(overrides: Partial<StatusSnapshot> = {}): StatusSnapshot {
  return { ...ROUTED_RECOVERY_READY_STATUS, ...overrides };
}
