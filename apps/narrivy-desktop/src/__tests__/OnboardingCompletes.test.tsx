import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import App from '../App';
import { bridge } from '../lib/bridge';
import { DEFAULT_CAPABILITIES } from '../test/mockBridge';

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

  it('offers the bundled model on a clean install (F-M11)', async () => {
    vi.mocked(bridge.status).mockResolvedValue({
      capabilities: [],
      active_session: null,
      installed_model: 'awaiting_model',
    });
    render(<App />);
    const dialog = await screen.findByRole('dialog', { name: /getting started/i });
    fireEvent.click(within(dialog).getByRole('button', { name: 'Next' }));
    fireEvent.click(within(dialog).getByRole('button', { name: /run microphone check/i }));
    expect(await within(dialog).findByText('Pass')).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole('button', { name: 'Next' }));
    const panel = await within(dialog).findByTestId('model-install');
    fireEvent.click(within(panel).getByRole('button', { name: 'Set up' }));
    await waitFor(() => expect(bridge.installBundledModel).toHaveBeenCalledTimes(1));
  });

  it('shows no model step when a model is ready', async () => {
    vi.mocked(bridge.status).mockResolvedValue({
      capabilities: [...DEFAULT_CAPABILITIES],
      active_session: null,
      installed_model: 'ready',
    });
    render(<App />);
    const dialog = await screen.findByRole('dialog', { name: /getting started/i });
    fireEvent.click(within(dialog).getByRole('button', { name: 'Next' }));
    fireEvent.click(within(dialog).getByRole('button', { name: /run microphone check/i }));
    expect(await within(dialog).findByText('Pass')).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole('button', { name: 'Next' }));
    await within(dialog).findByRole('button', { name: /start first dictation/i });
    expect(within(dialog).queryByTestId('model-install')).toBeNull();
  });

  it('offers to set the dictation shortcut when the service manages shortcuts (F-M20)', async () => {
    vi.mocked(bridge.status).mockResolvedValue({
      capabilities: [...DEFAULT_CAPABILITIES, 'activation.shortcuts=managed'],
      active_session: null,
      installed_model: 'ready',
    });
    render(<App />);
    const dialog = await screen.findByRole('dialog', { name: /getting started/i });
    const panel = await within(dialog).findByTestId('onboard-set-shortcut');
    fireEvent.click(within(panel).getByRole('button', { name: /set a shortcut/i }));
    expect(await screen.findByText(/press the keys/i)).toBeInTheDocument();
  });

  it('does not offer it when shortcuts are manual', async () => {
    vi.mocked(bridge.status).mockResolvedValue({
      capabilities: [...DEFAULT_CAPABILITIES, 'activation.shortcuts=manual:desktop_not_supported'],
      active_session: null,
      installed_model: 'ready',
    });
    render(<App />);
    const dialog = await screen.findByRole('dialog', { name: /getting started/i });
    await within(dialog).findByRole('button', { name: 'Next' });
    expect(within(dialog).queryByTestId('onboard-set-shortcut')).toBeNull();
  });
});
