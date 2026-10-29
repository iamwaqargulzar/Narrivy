import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import App from '../App';
import { bridge } from '../lib/bridge';

vi.mock('../lib/bridge', async () => {
  const { createMockBridgeModule } = await import('../test/mockBridge');
  return createMockBridgeModule();
});

describe('bundled model installation', () => {
  it('starts only from the explicit Models action', async () => {
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    vi.mocked(bridge.status).mockResolvedValue({
      capabilities: [],
      active_session: null,
      installed_model: 'awaiting_model',
    });
    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Models' }));

    const install = await screen.findByRole('button', { name: 'Set up' });
    expect(bridge.installBundledModel).not.toHaveBeenCalled();
    fireEvent.click(install);

    await waitFor(() => expect(bridge.installBundledModel).toHaveBeenCalledTimes(1));
    // The mocked daemon still reports no model, so the card stays, idle.
    expect(await screen.findByRole('heading', { name: /set up speech recognition/i })).toBeInTheDocument();
  });

  it('says why an install failed, in words (F-M11)', async () => {
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    vi.mocked(bridge.status).mockResolvedValue({
      capabilities: [],
      active_session: null,
      installed_model: 'awaiting_model',
      model_provisioning: {
        operation_id: 'op',
        state: 'failed',
        publication: 'not_published',
        cleanup: 'not_required',
        failure: 'source_rejected',
        revision: 2,
      },
    });
    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Models' }));
    const reason = await screen.findByTestId('model-install-failure');
    expect(reason).toHaveTextContent(/failed its checks/i);
  });

  it('hides the install panel once the model is in use', async () => {
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    vi.mocked(bridge.status).mockResolvedValue({
      capabilities: [],
      active_session: null,
      installed_model: 'ready',
      model_provisioning: {
        operation_id: 'op',
        state: 'ready',
        publication: 'published',
        cleanup: 'not_required',
        revision: 3,
      },
    });
    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Models' }));
    await screen.findAllByRole('heading', { name: 'Models' });
    expect(screen.queryByTestId('model-install')).toBeNull();
  });

  it('shows the setup step in words while it runs, with Cancel', async () => {
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    vi.mocked(bridge.status).mockResolvedValue({
      capabilities: [],
      active_session: null,
      installed_model: 'provisioning',
      model_provisioning: {
        operation_id: 'op',
        state: 'importing',
        publication: 'not_published',
        cleanup: 'not_required',
        revision: 1,
      },
    });
    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Models' }));
    const card = await screen.findByTestId('model-install');
    expect(card).toHaveTextContent('Checking and copying the model…');
    expect(card).not.toHaveTextContent(/importing/);
    expect(screen.getByRole('button', { name: 'Cancel' })).toBeEnabled();
  });
});
