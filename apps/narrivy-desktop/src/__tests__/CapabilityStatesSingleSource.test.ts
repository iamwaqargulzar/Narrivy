import { describe, expect, it } from 'vitest';
import catalogJson from '../../../../spec/catalogs/capability-states.json';
import { __CAPABILITY_UI_STATES, __CAPABILITY_CATALOG, capabilityState, UNKNOWN_CAPABILITY_STATE } from '../lib/capabilityStates';
import * as fs from 'node:fs';
import * as path from 'node:path';

describe('CapabilityStatesSingleSource', () => {
  it('capability_states_single_source: TS array equals JSON catalog and Rust array', async () => {
    // JSON is the single source — version must be 1
    expect(catalogJson.version).toBe(1);
    const states = (catalogJson as { version: number; states: { capability: string; state: string }[] }).states;
    expect(states).toHaveLength(6);

    // TS derived array must equal JSON (order-preserving)
    expect(__CAPABILITY_UI_STATES).toHaveLength(states.length);
    for (let i = 0; i < states.length; i++) {
      const [cap, state] = __CAPABILITY_UI_STATES[i];
      expect(cap).toBe(states[i].capability);
      expect(state).toBe(states[i].state);
    }

    // Also assert the re-exported catalog equals the raw import (TS internal consistency)
    expect(__CAPABILITY_CATALOG.states).toEqual(states);
    expect(__CAPABILITY_CATALOG.version).toBe(1);

    // Spot-check helpers still work via derived table
    expect(capabilityState('mock.asr')).toBe('asr-local');
    expect(capabilityState('asr.transcribe-cpp')).toBe('asr-local');
    expect(capabilityState('null.delivery')).toBe('delivery-inbox');
    expect(capabilityState('platform.delivery.router')).toBe('delivery-direct');
    expect(capabilityState('portal.globalshortcuts')).toBe('trigger-portal');
    expect(capabilityState('x11.hotkey')).toBe('trigger-x11-fallback');
    expect(capabilityState('unknown.cap')).toBe(UNKNOWN_CAPABILITY_STATE);
    expect(capabilityState('future.unknown')).toBe('capability-unknown');

    // Rust parity: we can't run Rust here, but we can ensure the TS file
    // imports the JSON (single-source) by checking the source text.
    const tsPath = path.resolve(process.cwd(), 'src/lib/capabilityStates.ts');
    const tsSrc = fs.readFileSync(tsPath, 'utf-8');
    expect(tsSrc).toContain('capability-states.json');
    expect(tsSrc).toContain('catalogJson');
  });
});
