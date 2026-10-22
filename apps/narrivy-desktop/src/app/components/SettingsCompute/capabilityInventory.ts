// The Compute tab renders daemon truth only. Every inventory below is projected
// from the capability strings the daemon advertises in StatusDto, in the order
// the daemon reported them, and nothing is named here that the daemon did not
// name. The bridge exposes no settings-mutating command and RunCapabilityCheck
// only answers for mic, delivery, shortcuts and theme-store, so a control with
// no supporting capability is reported as unavailable together with whatever
// the daemon did say about it.
import type { StatusSnapshot } from '../../../lib/bridge';
import { t } from '../../../i18n';

export interface AdvertisedToken {
  /** The capability string exactly as advertised. */
  raw: string;
  /** The name before the first `=`. */
  name: string;
  /** Everything after the first `=`, or null for a bare token. */
  value: string | null;
}

const CAPABILITY_SOURCE_PREFIX = 'capability: ';

/** Values that report a capability as present but not usable. */
const UNUSABLE_PREFIXES = ['unavailable', 'unverified', 'invalid', 'disabled', 'unsupported'];

function toToken(raw: string): AdvertisedToken {
  const at = raw.indexOf('=');
  return at < 0
    ? { raw, name: raw, value: null }
    : { raw, name: raw.slice(0, at), value: raw.slice(at + 1) };
}

function project(
  capabilities: string[] | undefined,
  matches: (name: string) => boolean,
): AdvertisedToken[] {
  return (capabilities ?? []).map(toToken).filter((token) => matches(token.name));
}

/** Advertised tokens whose name matches a control's subject. The pattern must
 * not carry the global flag, or its lastIndex would leak between calls. */
export function advertisedMatching(
  capabilities: string[] | undefined,
  pattern: RegExp,
): AdvertisedToken[] {
  return project(capabilities, (name) => pattern.test(name));
}

/** Compute-runtime capabilities: the bare engine token and its namespace. */
export function computeTokens(status: StatusSnapshot | null): AdvertisedToken[] {
  return project(
    status?.capabilities,
    (name) => name === 'asr.transcribe-cpp' || name.startsWith('asr.transcribe-cpp.'),
  );
}

/** Delivery capabilities: the daemon's lane, rungs, backends and helper state. */
export function deliveryTokens(status: StatusSnapshot | null): AdvertisedToken[] {
  return project(
    status?.capabilities,
    (name) =>
      name.startsWith('delivery.') ||
      name === 'inbox.delivery' ||
      name === 'null.delivery' ||
      name === 'wtype.inbox-recovery.delivery' ||
      name === 'platform.delivery.router',
  );
}

/** Automation-policy capabilities: privacy policy, cancellation and recovery. */
export function automationTokens(status: StatusSnapshot | null): AdvertisedToken[] {
  return project(
    status?.capabilities,
    (name) =>
      name.startsWith('policy.') ||
      name.startsWith('cancellation.') ||
      name.startsWith('recovery.'),
  );
}

/** The capability a resolver cited, or null when it cited none. */
export function citedCapability(source: string): string | null {
  return source.startsWith(CAPABILITY_SOURCE_PREFIX) ? source.slice(CAPABILITY_SOURCE_PREFIX.length) : null;
}

export interface ControlAvailability {
  available: false;
  /** Accessible reason naming exactly what the daemon reported, if anything. */
  reason: string;
}

/** Availability of one control in this tab. The daemon advertises no capability
 * that lets a client set a value and the bridge has no command for it, so a
 * control is only ever available: false, and its reason stays derived from the
 * report instead of from a hard-coded feature list. */
export function controlAvailability(reported: AdvertisedToken[]): ControlAvailability {
  return {
    available: false,
    reason:
      reported.length > 0
        ? t('settings.compute.controlUnavailable', { reported: reported.map((token) => token.raw).join(', ') })
        : t('settings.compute.controlNoCapability'),
  };
}

/** Availability of a control, quoted from whatever the daemon reported about it.
 * Nothing is assumed about how such a capability would be named, so an absent
 * capability yields the honest "no capability reported" reason. */
export function controlReporting(
  status: StatusSnapshot | null,
  pattern: RegExp,
): ControlAvailability {
  return controlAvailability(advertisedMatching(status?.capabilities, pattern));
}

/**
 * Whether the daemon reports the behaviour a switch would represent. A control
 * with no report at all is reported as off, because nothing claimed it on.
 */
export function reportedOn(reported: AdvertisedToken[]): boolean {
  if (reported.length === 0) return false;
  return reported.every((token) => {
    const value = token.value ?? '';
    if (value.length === 0) return true;
    const head = value.split(':')[0] ?? value;
    return !UNUSABLE_PREFIXES.includes(head);
  });
}
