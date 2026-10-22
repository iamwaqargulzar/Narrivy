// The Dictation selector and Start command use only the daemon-advertised
// pipeline ids: the UI must never offer or send a hardcoded raw/clean pair.
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import App from '../App';
import { bridge } from '../lib/bridge';

vi.mock('../lib/bridge', async () => {
  const { createMockBridgeModule } = await import('../test/mockBridge');
  return createMockBridgeModule();
});

const ONBOARD_KEY = 'narrivy.onboarded.v1';

beforeEach(() => {
  window.localStorage.setItem(ONBOARD_KEY, '1');
  vi.mocked(bridge.status).mockReset();
  vi.mocked(bridge.listPipelines).mockReset();
  vi.mocked(bridge.start).mockReset().mockResolvedValue(undefined);
});

describe('DictationStartsAdvertisedPipelineId', () => {
  it('offers only the advertised lanes and starts the selected exact id', async () => {
    vi.mocked(bridge.status).mockResolvedValue({
      capabilities: ['mock.asr', 'null.delivery'],
      active_session: { state: 'idle', pipeline_id: null },
      daemon_build: 'narrivyd 0.3.0-dev',
    });
    vi.mocked(bridge.listPipelines).mockResolvedValue([
      { id: 'raw', name: 'Raw dictation', version: 1, enabled: true },
      { id: 'clean', name: 'Clean dictation', version: 1, enabled: false },
    ]);

    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Dictation' }));

    const select = (await screen.findByTestId('dictation-mode-select')) as HTMLSelectElement;
    await waitFor(() => expect(select.value).toBe('raw'));
    // The disabled lane is not offered as a startable choice.
    expect(Array.from(select.options).map((o) => o.value)).toEqual(['raw']);

    const start = screen.getByTestId('dictation-start');
    await waitFor(() => expect(start).toBeEnabled());
    fireEvent.click(start);
    await waitFor(() => expect(bridge.start).toHaveBeenCalledWith('raw'));
  });

  it('switches the start command when the user selects another advertised id', async () => {
    vi.mocked(bridge.status).mockResolvedValue({
      capabilities: ['mock.asr', 'null.delivery'],
      active_session: { state: 'idle', pipeline_id: null },
      daemon_build: 'narrivyd 0.3.0-dev',
    });
    vi.mocked(bridge.listPipelines).mockResolvedValue([
      { id: 'mock-dictation', name: 'Mock dictation pipeline', version: 1, enabled: true },
      { id: 'clean', name: 'Clean dictation', version: 1, enabled: true },
    ]);

    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Dictation' }));

    const select = (await screen.findByTestId('dictation-mode-select')) as HTMLSelectElement;
    await waitFor(() => expect(select.value).toBe('mock-dictation'));

    const start = screen.getByTestId('dictation-start');
    await waitFor(() => expect(start).toBeEnabled());
    fireEvent.click(start);
    await waitFor(() => expect(bridge.start).toHaveBeenLastCalledWith('mock-dictation'));

    await act(async () => {
      fireEvent.change(select, { target: { value: 'clean' } });
    });
    expect(select.value).toBe('clean');
    fireEvent.click(start);
    await waitFor(() => expect(bridge.start).toHaveBeenLastCalledWith('clean'));
  });
});
