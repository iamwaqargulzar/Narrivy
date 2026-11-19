import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createFakeDaemon, type FakeDaemon } from '../test/fakeDaemonInvoke';

// No model ships with Narrivy (D40): the Models screen says to download one and
// offers no install from the package.
let daemon: FakeDaemon;
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (cmd: string, args?: Record<string, unknown>) => daemon.invoke(cmd, args),
}));
vi.mock('@tauri-apps/api/event', () => ({ listen: async () => () => {} }));

async function renderApp() {
  const { default: App } = await import('../App');
  render(<App />);
  fireEvent.click(await screen.findByRole('button', { name: 'Store' }));
  fireEvent.click(await screen.findByRole('tab', { name: 'Speech to text' }));
}

describe('no speech model in use', () => {
  beforeEach(() => {
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
  });
  afterEach(() => {
    vi.resetModules();
  });

  it('says to download a model, and offers no install from the package', async () => {
    daemon = createFakeDaemon();
    await renderApp();
    expect(await screen.findByTestId('model-needed')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Set up' })).toBeNull();
    expect(daemon.calls.filter(([cmd]) => cmd === 'model_install_bundled')).toHaveLength(0);
  });

  it('hides the note once a model is in use', async () => {
    daemon = createFakeDaemon();
    daemon.on('daemon_status', () => ({
      capabilities: [],
      active_session: null,
      installed_model: 'ready',
    }));
    await renderApp();
    await waitFor(() => expect(screen.queryByTestId('model-needed')).toBeNull());
  });
});
