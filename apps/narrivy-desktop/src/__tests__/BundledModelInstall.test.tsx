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

    const install = await screen.findByRole('button', { name: /install bundled model/i });
    expect(bridge.installBundledModel).not.toHaveBeenCalled();
    fireEvent.click(install);

    await waitFor(() => expect(bridge.installBundledModel).toHaveBeenCalledTimes(1));
    expect(await screen.findByText(/model installation: ready/i)).toBeInTheDocument();
  });
});
