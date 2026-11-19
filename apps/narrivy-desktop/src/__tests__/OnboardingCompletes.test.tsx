import { describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import App from '../App';
import { bridge, type ModelInfo, type SessionEvent } from '../lib/bridge';
import { offeredModels } from '../app/screens/Models';
import { DEFAULT_CAPABILITIES } from '../test/mockBridge';

vi.mock('../lib/bridge', async () => {
  const { createMockBridgeModule } = await import('../test/mockBridge');
  return createMockBridgeModule();
});

const ONBOARD_KEY = 'narrivy.onboarded.v1';

function model(id: string, state: ModelInfo['state'], extra: Partial<ModelInfo> = {}): ModelInfo {
  return {
    id,
    family: 'whisper',
    variant: id,
    quant: 'Q8_0',
    size_bytes: 83_000_000,
    license: 'MIT',
    engine: 'transcribe.cpp',
    languages: ['en'],
    streaming: false,
    source_repo: 'narrivy/store',
    source_revision: 'abc',
    qualified: false,
    state,
    downloadable: true,
    ...extra,
  };
}

function ready() {
  vi.mocked(bridge.status).mockResolvedValue({
    capabilities: [...DEFAULT_CAPABILITIES],
    active_session: null,
    installed_model: 'ready',
  });
}

function sessionEvents() {
  let handler: ((event: SessionEvent) => void) | null = null;
  let seq = 0;
  vi.mocked(bridge.onSessionEvent).mockImplementation(async (h) => {
    handler = h;
    return () => {};
  });
  return (state: string) => act(() => handler!({ seq: ++seq, state }));
}

async function openAt(step: number) {
  render(<App />);
  const dialog = await screen.findByRole('dialog', { name: /getting started/i });
  for (let i = 0; i < step; i += 1) {
    const next = within(dialog).getByRole('button', { name: 'Next' });
    await waitFor(() => expect(next).toBeEnabled());
    fireEvent.click(next);
  }
  return dialog;
}

describe('OnboardingCompletes', () => {
  it('walks every step, dictates once, and sets the flag', async () => {
    ready();
    const emit = sessionEvents();
    const dialog = await openAt(4);
    expect(within(dialog).getByText('Step 5 of 5')).toBeInTheDocument();
    const startBtn = within(dialog).getByRole('button', { name: 'Start' });
    await waitFor(() => expect(startBtn).toBeEnabled());
    fireEvent.click(startBtn);
    await waitFor(() => expect(bridge.start).toHaveBeenCalled());
    emit('listening');
    const stopBtn = within(dialog).getByRole('button', { name: 'Stop' });
    await waitFor(() => expect(stopBtn).toBeEnabled());
    fireEvent.click(stopBtn);
    await waitFor(() => expect(bridge.stop).toHaveBeenCalled());
    emit('transcribing');
    expect(await within(dialog).findByText(/Turning your words into text/)).toBeInTheDocument();
    emit('idle');
    fireEvent.click(within(dialog).getByRole('button', { name: 'Finish' }));

    await waitFor(() =>
      expect(screen.queryByRole('dialog', { name: /getting started/i })).toBeNull(),
    );
    expect(window.localStorage.getItem(ONBOARD_KEY)).toBe('1');
    expect(bridge.start).toHaveBeenCalledTimes(1);
    expect(bridge.stop).toHaveBeenCalledTimes(1);
  });

  it('follows the service when a dictation is stopped elsewhere', async () => {
    ready();
    const emit = sessionEvents();
    const dialog = await openAt(4);
    const startBtn = within(dialog).getByRole('button', { name: 'Start' });
    const stopBtn = within(dialog).getByRole('button', { name: 'Stop' });
    expect(stopBtn).toBeDisabled();
    await waitFor(() => expect(startBtn).toBeEnabled());
    fireEvent.click(startBtn);
    await waitFor(() => expect(bridge.start).toHaveBeenCalled());
    emit('listening');
    await waitFor(() => expect(stopBtn).toBeEnabled());
    expect(startBtn).toBeDisabled();
    // Stopped by the shortcut or another client: Start comes back, Stop goes.
    emit('idle');
    await waitFor(() => expect(within(dialog).getByRole('button', { name: /Start|Try again/ })).toBeEnabled());
    expect(stopBtn).toBeDisabled();
    expect(within(dialog).queryByText(/Listening/)).toBeNull();
    expect(bridge.stop).not.toHaveBeenCalled();
  });

  it('tests the microphone and says in words whether it heard the user', async () => {
    ready();
    vi.mocked(bridge.audioOptions).mockResolvedValue({
      echo_cancel: true,
      echo_cancel_active: true,
      microphone: '',
      microphones: [{ name: 'usb-mic', description: 'USB Microphone' }],
    });
    const dialog = await openAt(1);
    const picker = await within(dialog).findByRole('combobox', { name: 'Microphone' });
    fireEvent.change(picker, { target: { value: 'usb-mic' } });
    await waitFor(() => expect(bridge.setMicrophone).toHaveBeenCalledWith('usb-mic'));

    vi.mocked(bridge.capabilityCheck).mockResolvedValueOnce({ id: 'mic', pass: false, detail: 'silent: loudest -120 dBFS' });
    fireEvent.click(within(dialog).getByRole('button', { name: 'Test microphone' }));
    expect(await within(dialog).findByText(/Nothing was heard/)).toBeInTheDocument();
    expect(within(dialog).queryByText(/dBFS/)).toBeNull();

    vi.mocked(bridge.capabilityCheck).mockResolvedValueOnce({ id: 'mic', pass: true, detail: 'heard: loudest -12 dBFS' });
    fireEvent.click(within(dialog).getByRole('button', { name: 'Test again' }));
    expect(await within(dialog).findByText(/Narrivy heard you/)).toBeInTheDocument();
    expect(bridge.capabilityCheck).toHaveBeenCalledWith('mic');
  });

  it('downloads a speech model in place on a clean install and loads it (D40)', async () => {
    vi.mocked(bridge.status).mockResolvedValue({
      capabilities: [],
      active_session: null,
      installed_model: 'awaiting_model',
    });
    vi.mocked(bridge.models).mockResolvedValue([
      model('whisper-tiny.en-Q8_0', 'available'),
      model('whisper-base.en-Q8_0', 'available', { first_run_rank: 1 }),
    ]);
    const dialog = await openAt(2);
    const group = await within(dialog).findByRole('radiogroup');
    // The suggested model comes first and is picked.
    expect(within(group).getAllByRole('radio')[0]).toBeChecked();
    expect(within(group).getByText('Suggested')).toBeInTheDocument();
    expect(within(dialog).getByRole('button', { name: 'Next' })).toBeDisabled();

    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      vi.mocked(bridge.models).mockResolvedValue([
        model('whisper-tiny.en-Q8_0', 'available'),
        model('whisper-base.en-Q8_0', 'downloading', { downloaded_bytes: 41_500_000, first_run_rank: 1 }),
      ]);
      fireEvent.click(within(dialog).getByRole('button', { name: 'Download' }));
      await waitFor(() => expect(bridge.downloadModel).toHaveBeenCalledWith('whisper-base.en-Q8_0'));
      expect(await within(dialog).findByRole('progressbar')).toHaveAttribute('aria-valuenow', '50');

      vi.mocked(bridge.models).mockResolvedValue([
        model('whisper-tiny.en-Q8_0', 'available'),
        model('whisper-base.en-Q8_0', 'installed'),
      ]);
      await act(async () => {
        await vi.advanceTimersByTimeAsync(1100);
      });
      await waitFor(() => expect(bridge.selectModel).toHaveBeenCalledWith('whisper-base.en-Q8_0'));
      expect(await within(dialog).findByText(/Loading the model/)).toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
    expect(bridge.installBundledModel).not.toHaveBeenCalled();
  });

  it('says the model is ready when one is in use', async () => {
    ready();
    vi.mocked(bridge.models).mockResolvedValue([model('whisper-base.en-Q8_0', 'active')]);
    const dialog = await openAt(2);
    expect(await within(dialog).findByText(/is ready/)).toBeInTheDocument();
    expect(within(dialog).queryByRole('radiogroup')).toBeNull();
    expect(within(dialog).getByRole('button', { name: 'Next' })).toBeEnabled();
  });

  it('offers the suggested models first, else whatever can be downloaded', () => {
    const suggested = offeredModels([
      model('parakeet-tdt-0.6b-v3-Q8_0', 'available', { first_run_rank: 2 }),
      model('whisper-tiny.en-Q8_0', 'available'),
      model('whisper-base.en-Q8_0', 'available', { first_run_rank: 1 }),
      model('whisper-small.en-Q8_0', 'available', { first_run_rank: 3, downloadable: false }),
    ]);
    expect(suggested.map((m) => m.id)).toEqual(['whisper-base.en-Q8_0', 'parakeet-tdt-0.6b-v3-Q8_0']);
    const other = offeredModels([
      model('a', 'available'),
      model('b', 'available', { downloadable: false }),
      model('c', 'needs_engine'),
    ]);
    expect(other.map((m) => m.id)).toEqual(['a']);
  });

  it('offers to set the dictation shortcut when the service manages shortcuts (F-M20)', async () => {
    vi.mocked(bridge.status).mockResolvedValue({
      capabilities: [...DEFAULT_CAPABILITIES, 'activation.shortcuts=managed'],
      active_session: null,
      installed_model: 'ready',
    });
    const dialog = await openAt(3);
    const panel = await within(dialog).findByTestId('onboard-set-shortcut');
    fireEvent.click(within(panel).getByRole('button', { name: /choose shortcut keys/i }));
    expect(await screen.findByText(/press the keys/i)).toBeInTheDocument();
  });

  it('does not offer it when shortcuts are manual', async () => {
    vi.mocked(bridge.status).mockResolvedValue({
      capabilities: [...DEFAULT_CAPABILITIES, 'activation.shortcuts=manual:desktop_not_supported'],
      active_session: null,
      installed_model: 'ready',
    });
    const dialog = await openAt(3);
    await within(dialog).findByRole('button', { name: 'Next' });
    expect(within(dialog).queryByTestId('onboard-set-shortcut')).toBeNull();
  });
});
