import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createFakeDaemon, type FakeDaemon } from '../test/fakeDaemonInvoke';

// The real TauriBridge runs; only the transport underneath it is faked, by a
// daemon that answers the install the way the service does: the install call
// returns an operation that is still running, and only later status reads say
// whether the model became ready (F-M16: the old test mocked an instant
// "ready" from the install call while status said no model).
let daemon: FakeDaemon;
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (cmd: string, args?: Record<string, unknown>) => daemon.invoke(cmd, args),
}));
vi.mock('@tauri-apps/api/event', () => ({ listen: async () => () => {} }));

async function renderApp() {
  const { default: App } = await import('../App');
  render(<App />);
  fireEvent.click(await screen.findByRole('button', { name: 'Models' }));
}

const installCalls = () => daemon.calls.filter(([cmd]) => cmd === 'model_install_bundled');

describe('bundled model installation', () => {
  beforeEach(() => {
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
  });
  afterEach(() => {
    vi.resetModules();
  });

  it('starts only from the explicit Models action and follows it to ready', async () => {
    daemon = createFakeDaemon({ bundled: 'ready' });
    await renderApp();

    const install = await screen.findByRole('button', { name: 'Set up' });
    expect(installCalls()).toHaveLength(0);
    fireEvent.click(install);

    await waitFor(() => expect(installCalls()).toHaveLength(1));
    // The real command, with no path from the page: the shell sends the one
    // packaged source itself.
    expect(installCalls()[0]).toEqual(['model_install_bundled', undefined]);
    // Running: the step in words, with Cancel.
    expect(await screen.findByTestId('model-install')).toHaveTextContent(
      /Checking and copying the model…|Starting the speech engine…/,
    );
    expect(screen.getByRole('button', { name: 'Cancel' })).toBeEnabled();
    // The status poll follows the operation until the daemon says ready, and
    // then the card goes away.
    await waitFor(() => expect(screen.queryByTestId('model-install')).toBeNull(), {
      timeout: 5000,
    });
  });

  it('says why an install failed, in words (F-M11, F-C5)', async () => {
    daemon = createFakeDaemon({ bundled: 'source_rejected' });
    await renderApp();
    fireEvent.click(await screen.findByRole('button', { name: 'Set up' }));

    const reason = await screen.findByTestId('model-install-failure');
    expect(reason).toHaveTextContent(/failed its checks/i);
    // Still not set up: the card stays and offers to try again.
    expect(screen.getByTestId('model-install')).toBeInTheDocument();
  });

  it('hides the install panel once the model is in use', async () => {
    daemon = createFakeDaemon();
    daemon.on('daemon_status', () => ({
      capabilities: [],
      active_session: null,
      installed_model: 'ready',
    }));
    await renderApp();
    await screen.findAllByRole('heading', { name: 'Models' });
    expect(screen.queryByTestId('model-install')).toBeNull();
  });
});
