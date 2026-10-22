// Atomic activation presentation derived ONLY from daemon-reported capability
// facts. Narrivy never configures a desktop/compositor shortcut itself: the
// user owns that binding, and this module decides only when the exact installed
// command may be *suggested*. Never branch on an OS or desktop name here.
//
// The three daemon facts are separate and must not be conflated:
//   activation.atomic-command          — this daemon serves one atomic toggle
//   activation.portal-global-shortcuts — the direct portal interface probe
//   activation.portal-listener         — a real owned portal listener is live
//
// A probe result is not a listener, and an available command is not a
// configured shortcut.

import type { StatusSnapshot } from './bridge';

export const ATOMIC_COMMAND_CAPABILITY = 'activation.atomic-command';

export const PORTAL_INTERFACE_CAPABILITY = 'activation.portal-global-shortcuts';

export const PORTAL_LISTENER_CAPABILITY = 'activation.portal-listener';

/**
 * The exact installed command a user may bind in their desktop's shortcut
 * settings. Kept byte-identical to the packaging documentation and to the
 * installed client (`/usr/bin/narrivy-cli toggle`).
 */
export const USER_MANAGED_SHORTCUT_COMMAND = '/usr/bin/narrivy-cli toggle';

export interface ActivationPresentation {
  /** The daemon serves one atomic toggle request. */
  atomicCommandAvailable: boolean;
  /** The exact portal interface answered a bounded probe. */
  portalInterfaceAvailable: boolean;
  /** A real daemon-owned portal listener is live. */
  portalListenerReady: boolean;
  /**
   * The exact command to suggest when the atomic command is available and no
   * live portal listener exists; `null` when it must not be shown. A non-null
   * value means "bind this yourself", never "this is configured".
   */
  suggestedCommand: string | null;
}

/** Value of the first advertised capability starting with `prefix=`; else null. */
function capabilityValue(capabilities: readonly string[], prefix: string): string | null {
  const token = capabilities.find((cap) => cap.startsWith(`${prefix}=`));
  return token ? token.slice(prefix.length + 1) : null;
}

/**
 * Derive the activation facts from one daemon status snapshot. Absent or
 * unknown tokens fail closed: nothing is claimed and no command is suggested.
 */
export function activationPresentation(
  status: StatusSnapshot | null | undefined,
): ActivationPresentation {
  const capabilities = status?.capabilities ?? [];
  const atomicCommandAvailable =
    capabilityValue(capabilities, ATOMIC_COMMAND_CAPABILITY) === 'available';
  const portalInterfaceAvailable =
    capabilityValue(capabilities, PORTAL_INTERFACE_CAPABILITY) === 'available';
  const portalListenerReady = capabilityValue(capabilities, PORTAL_LISTENER_CAPABILITY) === 'ready';
  return {
    atomicCommandAvailable,
    portalInterfaceAvailable,
    portalListenerReady,
    suggestedCommand:
      atomicCommandAvailable && !portalListenerReady ? USER_MANAGED_SHORTCUT_COMMAND : null,
  };
}
