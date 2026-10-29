import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createFakeDaemon, type FakeDaemon } from '../test/fakeDaemonInvoke';

// The real TauriBridge runs; only the transport is faked. The fake answers
// `daemon_capability_check` with the shape the Rust shell serialises
// (`bridge::CapabilityCheckResult`: id, pass, detail), so the page can only
// show verdicts the service can actually produce. F-M16: the old test mocked
// the bridge to return `disabled: true`, a field neither the wire DTO
// (`CapabilityCheckResultDto`) nor the shell ever sends, so its "Disabled"
// state could never appear in the real app; the state has been removed.
let daemon: FakeDaemon;
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (cmd: string, args?: Record<string, unknown>) => daemon.invoke(cmd, args),
}));
vi.mock('@tauri-apps/api/event', () => ({ listen: async () => () => {} }));

/** What the daemon's `run_capability_check_with` reports for each id, as the
 * shell maps it (`available` becomes `pass`). */
function serviceCheck(results: Record<string, { available: boolean; detail: string }>) {
  return (args: Record<string, unknown> | undefined) => {
    const id = String(args?.id);
    const r = results[id];
    if (!r) return { id, pass: false, detail: 'unknown id; supported: mic, delivery, shortcuts, theme-store' };
    return { id, pass: r.available, detail: r.detail };
  };
}

async function openDiagnostics() {
  const { default: App } = await import('../App');
  render(<App />);
  fireEvent.click(await screen.findByRole('button', { name: 'Diagnostics' }));
}

describe('capability check results', () => {
  beforeEach(() => {
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    daemon = createFakeDaemon();
  });
  afterEach(() => {
    vi.resetModules();
  });

  it('shows Fail with the service detail for an unavailable check', async () => {
    daemon.on(
      'daemon_capability_check',
      serviceCheck({
        shortcuts: { available: false, detail: 'GlobalShortcuts portal not offered' },
      }),
    );
    await openDiagnostics();
    fireEvent.click(await screen.findByRole('button', { name: /run shortcuts check/i }));

    expect(await screen.findByText('Fail')).toBeInTheDocument();
    expect(screen.getByText('GlobalShortcuts portal not offered')).toBeInTheDocument();
    expect(screen.queryByText('Disabled')).toBeNull();
    // The real command and argument names reached the transport.
    expect(daemon.calls).toContainEqual(['daemon_capability_check', { id: 'shortcuts' }]);
  });

  it('shows Pass only when the service says available', async () => {
    daemon.on(
      'daemon_capability_check',
      serviceCheck({ 'theme-store': { available: true, detail: 'config dir writable' } }),
    );
    await openDiagnostics();
    fireEvent.click(await screen.findByRole('button', { name: /run theme.* check/i }));

    expect(await screen.findByText('Pass')).toBeInTheDocument();
    expect(screen.getByText('config dir writable')).toBeInTheDocument();
  });

  it('shows the error, not a verdict, when the check cannot run', async () => {
    // No handler: the transport rejects, as it does with the daemon down.
    await openDiagnostics();
    fireEvent.click(await screen.findByRole('button', { name: /run microphone check/i }));

    expect(await screen.findByRole('alert', { name: '' })).toBeInTheDocument();
    expect(screen.queryByText('Pass')).toBeNull();
    expect(screen.queryByText('Fail')).toBeNull();
  });
});
