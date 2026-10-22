import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import App from '../App';
import { bridge } from '../lib/bridge';

vi.mock('../lib/bridge', async () => {
  const { createMockBridgeModule } = await import('../test/mockBridge');
  return createMockBridgeModule();
});

const ONBOARD_KEY = 'narrivy.onboarded.v1';

describe('OnboardingCompletes', () => {
  it('walks Welcome → Mic check → First dictation and sets the flag', async () => {
    // NOTE: deliberately NOT presetting narrivy.onboarded.v1
    render(<App />);

    const dialog = await screen.findByRole('dialog', { name: /getting started/i });

    // Step 1: Welcome
    fireEvent.click(within(dialog).getByRole('button', { name: 'Next' }));

    // Step 2: Mic check
    fireEvent.click(within(dialog).getByRole('button', { name: /run microphone check/i }));
    expect(await within(dialog).findByText('Pass')).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole('button', { name: 'Next' }));

    // Step 3: First dictation — start once, stop once. Start waits for the
    // daemon-advertised pipeline selection to load.
    const startBtn = within(dialog).getByRole('button', { name: /start first dictation/i });
    await waitFor(() => expect(startBtn).toBeEnabled());
    fireEvent.click(startBtn);
    const stopBtn = await within(dialog).findByRole('button', { name: /stop first dictation/i });
    await waitFor(() => expect(stopBtn).toBeEnabled());
    fireEvent.click(stopBtn);

    const finishBtn = await within(dialog).findByRole('button', { name: 'Finish' });
    await waitFor(() => expect(finishBtn).toBeEnabled());
    fireEvent.click(finishBtn);

    await waitFor(() =>
      expect(screen.queryByRole('dialog', { name: /getting started/i })).toBeNull(),
    );
    expect(window.localStorage.getItem(ONBOARD_KEY)).toBe('1');
    expect(bridge.start).toHaveBeenCalledTimes(1);
    expect(bridge.stop).toHaveBeenCalledTimes(1);
  });
});
