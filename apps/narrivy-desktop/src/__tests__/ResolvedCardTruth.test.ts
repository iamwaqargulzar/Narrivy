import { describe, expect, it } from 'vitest';
import type { StatusSnapshot } from '../lib/bridge';
import { resolveCompute, resolveDelivery, resolveMic, resolveModel } from '../lib/resolved';
import { FIELD_SPECS } from '../lib/settings';
import { deliveryChip } from '../lib/homeSummary';
import {
  ROUTED_RECOVERY_CAPABILITIES,
  ROUTED_RECOVERY_NO_HELPER_CAPABILITIES,
  routedRecoveryStatus,
} from '../test/installedRecoveryFixtures';

function status(overrides: Partial<StatusSnapshot>): StatusSnapshot {
  return { capabilities: [], active_session: null, ...overrides };
}

function resolveFor(key: string, caps: string[]): string {
  const spec = FIELD_SPECS.find((f) => f.key === key);
  if (!spec?.resolve) throw new Error(`no resolver for ${key}`);
  return spec.resolve(caps);
}

describe('resolved card values', () => {
  it('never labels an Inbox-only daemon as clipboard copy', () => {
    const delivery = resolveDelivery(
      status({ capabilities: ['delivery.inbox-only=available'] }),
      'auto',
    );
    expect(delivery.resolved).toContain('Inbox recovery only');
    expect(delivery.resolved).not.toContain('Clipboard copy');
    expect(delivery.source).toContain('delivery.inbox-only=available');
  });

  it('reports delivery as unavailable when no delivery capability is advertised', () => {
    const delivery = resolveDelivery(status({}), 'auto');
    expect(delivery.resolved).toBe('Auto → Unavailable');
    expect(delivery.resolved).not.toContain('Clipboard');
  });

  it('keeps the explicit clipboard capability label', () => {
    const delivery = resolveDelivery(status({ capabilities: ['delivery.clipboard'] }), 'auto');
    expect(delivery.resolved).toBe('Auto → Clipboard paste');
  });

  it('describes wtype-assisted Inbox recovery without claiming verified typing', () => {
    const delivery = resolveDelivery(
      status({
        capabilities: [
          'delivery.wtype-helper=ready:system-owned-revalidated',
          'delivery.verified=unavailable',
          'delivery.inbox-recovery=enabled',
        ],
      }),
      'auto',
    );
    expect(delivery.resolved).toContain('Inbox recovery');
    expect(delivery.resolved).toContain('unverified');
    expect(delivery.resolved).not.toContain('Clipboard copy');
  });

  it('describes the routed lane as target-aware routing without claiming insertion', () => {
    const delivery = resolveDelivery(routedRecoveryStatus(), 'auto');
    expect(delivery.resolved).toContain('Inbox recovery');
    expect(delivery.resolved).toContain('target-aware routing');
    expect(delivery.resolved).toContain('unverified');
    expect(delivery.resolved).not.toContain('Clipboard copy');
    expect(delivery.resolved).not.toContain('verified insertion');
    expect(delivery.source).toContain('delivery.route=target-aware');
    expect(resolveFor('delivery.mode', [...ROUTED_RECOVERY_CAPABILITIES])).toContain(
      'typing unverified',
    );
  });

  it('keeps the routed lane truthful when no typing helper is qualified', () => {
    const delivery = resolveDelivery(
      routedRecoveryStatus({ capabilities: [...ROUTED_RECOVERY_NO_HELPER_CAPABILITIES] }),
      'auto',
    );
    expect(delivery.resolved).toContain('Inbox recovery');
    // A missing package names its fix (F-C4); other reasons stay generic.
    expect(delivery.resolved).toContain('wtype is not installed (sudo apt install wtype)');
    expect(
      resolveFor('delivery.mode', [...ROUTED_RECOVERY_NO_HELPER_CAPABILITIES]),
    ).toContain('sudo apt install wtype');
    const untrusted = resolveDelivery(
      routedRecoveryStatus({
        capabilities: ROUTED_RECOVERY_NO_HELPER_CAPABILITIES.map((token) =>
          token === 'delivery.wtype-helper=unavailable:wtype_missing'
            ? 'delivery.wtype-helper=unavailable:wtype_untrusted'
            : token,
        ),
      }),
      'auto',
    );
    expect(untrusted.resolved).toContain('typing helper unavailable');
    expect(untrusted.resolved).not.toContain('apt install');
    // Home still says the text only goes to the Inbox.
    expect(deliveryChip(delivery).label).toBe('Saves to Inbox');
    expect(deliveryChip(untrusted).label).toBe('Saves to Inbox');
  });

  it('never advertises clipboard fallback while no trusted helper is accepted', () => {
    const denied = resolveDelivery(routedRecoveryStatus(), 'auto');
    expect(denied.resolved).not.toContain('clipboard restore authorized');
    // Explicit authorization cannot conjure the unaccepted trusted helper: the
    // installed lane keeps reporting the clipboard rungs unavailable.
    const authorizedButUnimplemented = resolveDelivery(
      routedRecoveryStatus({
        capabilities: ROUTED_RECOVERY_CAPABILITIES.map((token) =>
          token === 'delivery.backend.clipboard=unavailable:clipboard_not_authorized'
            ? 'delivery.backend.clipboard=unavailable:trusted_clipboard_helper_not_accepted'
            : token,
        ),
      }),
      'auto',
    );
    expect(authorizedButUnimplemented.resolved).not.toContain('clipboard restore authorized');
  });

  it('describes an awaiting model lifecycle as not installed', () => {
    const model = resolveModel(
      status({
        installed_model: 'awaiting_model',
        capabilities: ['model.gguf=unavailable:installed_model_missing'],
      }),
      'auto',
    );
    expect(model.resolved).toContain('Not installed');
    expect(model.source).toContain('awaiting_model');
  });

  it('describes a ready installed model from lifecycle and capability truth', () => {
    const model = resolveModel(
      status({ installed_model: 'ready', capabilities: ['model.gguf=available'] }),
      'auto',
    );
    expect(model.resolved).toBe('Auto → Local GGUF (transcribe.cpp)');
  });

  it('reports strict resident compute from its namespaced capability', () => {
    const compute = resolveCompute(
      status({
        capabilities: [
          'asr.transcribe-cpp.cpu=available',
          'asr.transcribe-cpp.runtime=resident-worker',
        ],
      }),
      'auto',
    );
    expect(compute.resolved).toBe('Auto → CPU (transcribe.cpp, resident-worker)');
  });

  it('reports the pipewire capture helper as unverified instead of claiming a microphone', () => {
    const mic = resolveMic(
      status({ capabilities: ['capture.pipewire-helper=ready:system-owned-revalidated'] }),
      'auto',
    );
    expect(mic.resolved).toContain('capture helper');
    expect(mic.resolved).toContain('unverified');
  });

  it('does not attach Auto labels to explicit manual choices', () => {
    const custom = resolveDelivery(status({ capabilities: ['delivery.inbox-only=available'] }), 'custom');
    expect(custom.resolved).not.toContain('Auto →');
  });
});

describe('settings resolved values', () => {
  it('never falls back to a clipboard-copy delivery claim', () => {
    expect(resolveFor('delivery.mode', ['delivery.inbox-only=available'])).toContain(
      'Inbox recovery only',
    );
    expect(resolveFor('delivery.mode', ['delivery.inbox-only=available'])).not.toContain(
      'Clipboard copy',
    );
    expect(resolveFor('delivery.mode', [])).toBe('Unavailable');
    expect(resolveFor('delivery.mode', ['delivery.clipboard'])).toBe('Clipboard paste');
  });

  it('reports an unavailable strict model rather than claiming a local engine', () => {
    expect(resolveFor('asr.model', ['model.gguf=unavailable:installed_model_missing'])).toContain(
      'Not installed',
    );
    expect(resolveFor('asr.model', ['model.gguf=available'])).toContain('Local GGUF');
    expect(resolveFor('compute.device', ['asr.transcribe-cpp.cpu=unavailable:missing'])).toBe(
      'No local engine detected',
    );
    expect(resolveFor('compute.device', ['asr.transcribe-cpp.cpu=available'])).toBe(
      'CPU (transcribe.cpp)',
    );
  });
});
