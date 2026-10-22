import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import App from '../App';
import { bridge } from '../lib/bridge';

vi.mock('../lib/bridge', async () => {
  const { createMockBridgeModule } = await import('../test/mockBridge');
  return createMockBridgeModule();
});

const ONBOARD_KEY = 'narrivy.onboarded.v1';

function presetOnboarded() {
  window.localStorage.setItem(ONBOARD_KEY, '1');
}

describe('CapabilityDisabledState', () => {
  it('renders Disabled with detail for a disabled capability check', async () => {
    presetOnboarded();
    vi.mocked(bridge.capabilityCheck).mockImplementation(async (id: string) => {
      if (id === 'shortcuts') {
        return { id, pass: false, disabled: true, detail: 'Accessibility permission not granted' };
      }
      return { id, pass: true, detail: 'ok' };
    });

    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Diagnostics' }));

    fireEvent.click(await screen.findByRole('button', { name: /run shortcuts check/i }));

    expect(await screen.findByText('Disabled')).toBeInTheDocument();
    expect(screen.getByText('Accessibility permission not granted')).toBeInTheDocument();
  });

  it('renders Fail for a failing non-disabled check', async () => {
    presetOnboarded();
    vi.mocked(bridge.capabilityCheck).mockImplementation(async (id: string) => ({
      id,
      pass: false,
      detail: 'no input device',
    }));

    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Diagnostics' }));

    fireEvent.click(await screen.findByRole('button', { name: /run microphone check/i }));

    expect(await screen.findByText('Fail')).toBeInTheDocument();
    expect(screen.getByText('no input device')).toBeInTheDocument();
  });
});
