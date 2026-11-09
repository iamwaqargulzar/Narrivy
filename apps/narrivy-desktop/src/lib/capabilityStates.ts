// Capability -> UI state token vocabulary — SINGLE SOURCE at spec/catalogs/capability-states.json
// Both Rust (src-tauri/src/capability_ui.rs) and this TS file derive from that
// JSON, so the vocabulary cannot drift. Tests assert equality with the JSON.
// Never branch on OS names anywhere in this file or its consumers.

import catalogJson from '../../../../spec/catalogs/capability-states.json';

export type CapabilityState =
  | 'asr-local'
  | 'delivery-inbox'
  | 'delivery-direct'
  | 'trigger-portal'
  | 'trigger-x11-fallback'
  | 'capability-unknown';

export const UNKNOWN_CAPABILITY_STATE: CapabilityState = 'capability-unknown';

interface CatalogEntry {
  capability: string;
  state: CapabilityState;
}

interface CapabilityStatesCatalog {
  version: number;
  states: CatalogEntry[];
}

// The imported JSON is the single source of truth.
const catalog = catalogJson as CapabilityStatesCatalog;

if (catalog.version !== 1) {
  throw new Error(`unsupported capability-states.json version ${catalog.version}`);
}

const CAPABILITY_UI_STATES: ReadonlyArray<readonly [string, CapabilityState]> =
  catalog.states.map((e) => [e.capability, e.state] as const);

export function capabilityState(capability: string): CapabilityState {
  return (
    CAPABILITY_UI_STATES.find(([cap]) => cap === capability)?.[1] ??
    UNKNOWN_CAPABILITY_STATE
  );
}

/** Every known token — used by Diagnostics to enumerate states. */
export function allCapabilityStates(): readonly CapabilityState[] {
  return CAPABILITY_UI_STATES.map(([, state]) => state);
}

// Re-export for tests that assert single-source equality without re-parsing.
export const __CAPABILITY_UI_STATES = CAPABILITY_UI_STATES;
export const __CAPABILITY_CATALOG = catalog;
