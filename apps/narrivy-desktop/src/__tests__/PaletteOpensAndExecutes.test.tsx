import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
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

describe('PaletteOpensAndExecutes', () => {
  it('opens with Ctrl+K, filters, executes on Enter, closes on Escape', async () => {
    presetOnboarded();
    render(<App />);

    fireEvent.keyDown(window, { key: 'k', ctrlKey: true });
    const dialog = await screen.findByRole('dialog', { name: /command palette/i });
    expect(dialog).toBeInTheDocument();

    const input = screen.getByPlaceholderText(/type a command/i);
    fireEvent.change(input, { target: { value: 'about' } });
    fireEvent.keyDown(input, { key: 'Enter' });

    expect(await screen.findByRole('heading', { level: 1, name: 'About' })).toBeInTheDocument();

    // Esc closes the overlay
    fireEvent.keyDown(window, { key: 'k', ctrlKey: true });
    const reopened = await screen.findByRole('dialog', { name: /command palette/i });
    const reopenedInput = reopened.querySelector('input') as HTMLInputElement;
    fireEvent.keyDown(reopenedInput, { key: 'Escape' });
    await waitFor(() =>
      expect(screen.queryByRole('dialog', { name: /command palette/i })).toBeNull(),
    );
  });

  it('executes the run-all-diagnostics action', async () => {
    presetOnboarded();
    render(<App />);

    fireEvent.keyDown(window, { key: 'k', metaKey: true });
    const input = await screen.findByPlaceholderText(/type a command/i);
    fireEvent.change(input, { target: { value: 'diagnostics' } });
    fireEvent.keyDown(input, { key: 'ArrowDown' }); // skip "Go to Diagnostics"
    fireEvent.keyDown(input, { key: 'Enter' });     // run all diagnostics

    expect(await screen.findByRole('heading', { level: 1, name: 'Diagnostics' })).toBeInTheDocument();
    await waitFor(() => expect(bridge.capabilityCheck).toHaveBeenCalledWith('mic'));
    await waitFor(() => expect(bridge.capabilityCheck).toHaveBeenCalledWith('theme-store'));
  });
});
