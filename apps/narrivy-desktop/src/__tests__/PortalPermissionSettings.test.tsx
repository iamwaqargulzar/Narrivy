import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import App from '../App';
import { bridge } from '../lib/bridge';

vi.mock('../lib/bridge', async () => {
  const { createMockBridgeModule } = await import('../test/mockBridge');
  return createMockBridgeModule();
});

describe('portal permission settings', () => {
  it('requests permission only after an explicit settings action', async () => {
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    render(<App />);

    fireEvent.click(await screen.findByRole('button', { name: 'Settings' }));
    fireEvent.click(screen.getByRole('tab', { name: 'Text insertion' }));
    const request = await screen.findByRole('button', {
      name: /request text delivery permission/i,
    });
    await waitFor(() => expect(request).toBeEnabled());
    expect(bridge.requestPortalKeyboardPermission).not.toHaveBeenCalled();
    fireEvent.click(request);

    expect(await screen.findByText(/keyboard permission is active/i)).toBeInTheDocument();
    expect(bridge.requestPortalKeyboardPermission).toHaveBeenCalledTimes(1);
  });
});
