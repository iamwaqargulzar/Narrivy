// Installed strict-recovery pipeline selection regressions.
//
// The packaged daemon advertises its two real built-in dictation modes
// (`clean` first as the configured default, then `raw`) and accepts exactly the
// advertised id. These tests drive the real React screens with fixtures shaped
// like that installed daemon; the frontend holds no mode policy of its own.
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import App from '../App';
import { bridge, type PipelineSummary } from '../lib/bridge';
import {
  CLEAN_MODE_ID,
  RAW_MODE_ID,
  STRICT_MODES_DISABLED,
  STRICT_MODES_ENABLED,
  installedRecoveryStatus,
  readyRecoveryStatus,
} from '../test/installedRecoveryFixtures';

vi.mock('../lib/bridge', async () => {
  const { createMockBridgeModule } = await import('../test/mockBridge');
  return createMockBridgeModule();
});

const ONBOARD_KEY = 'narrivy.onboarded.v1';
const OTHER_ENABLED: PipelineSummary = {
  id: 'other-lane',
  name: 'Other advertised lane',
  version: 1,
  enabled: true,
};

beforeEach(() => {
  window.localStorage.setItem(ONBOARD_KEY, '1');
  vi.mocked(bridge.status).mockReset();
  vi.mocked(bridge.listPipelines).mockReset();
  vi.mocked(bridge.start).mockReset().mockResolvedValue(undefined);
  vi.mocked(bridge.stop).mockReset().mockResolvedValue(undefined);
  vi.mocked(bridge.cancel).mockReset().mockResolvedValue(undefined);
});

async function openDictation() {
  render(<App />);
  fireEvent.click(await screen.findByRole('button', { name: 'Dictation' }));
  return screen.findByTestId('dictation-start');
}

describe('InstalledRecoveryPipelineSelection', () => {
  it('offers exactly the advertised real modes and starts the selected exact id', async () => {
    vi.mocked(bridge.status).mockImplementation(async () => readyRecoveryStatus());
    vi.mocked(bridge.listPipelines).mockImplementation(async () => STRICT_MODES_ENABLED);

    const start = await openDictation();
    await waitFor(() => expect(start).toBeEnabled());

    const select = (await screen.findByTestId('dictation-mode-select')) as HTMLSelectElement;
    // The daemon's default mode is the first advertised entry.
    expect(select.value).toBe(CLEAN_MODE_ID);
    expect(Array.from(select.options).map((o) => o.value)).toEqual([
      CLEAN_MODE_ID,
      RAW_MODE_ID,
    ]);
    expect(screen.getByTestId('dictation-selected-id')).toHaveTextContent(CLEAN_MODE_ID);

    // Selecting the other real mode and starting sends that exact advertised id.
    fireEvent.change(select, { target: { value: RAW_MODE_ID } });
    await waitFor(() => expect(select.value).toBe(RAW_MODE_ID));
    fireEvent.click(start);
    await waitFor(() => expect(bridge.start).toHaveBeenCalledTimes(1));
    expect(bridge.start).toHaveBeenCalledWith(RAW_MODE_ID);
  });

  it('blocks start and guides model provisioning while the strict modes are awaiting the model', async () => {
    vi.mocked(bridge.status).mockImplementation(async () => installedRecoveryStatus());
    vi.mocked(bridge.listPipelines).mockImplementation(async () => STRICT_MODES_DISABLED);

    const start = await openDictation();
    const notice = await screen.findByTestId('dictation-availability');
    expect(notice).toHaveTextContent(/speech model is not installed/i);
    expect(start).toBeDisabled();
    expect(screen.getByTestId('home-open-models')).toBeInTheDocument();

    fireEvent.click(start);
    expect(bridge.start).not.toHaveBeenCalled();
  });

  it('blocks start and guides provisioning when the daemon advertises no pipeline at all', async () => {
    vi.mocked(bridge.status).mockImplementation(async () => installedRecoveryStatus());
    vi.mocked(bridge.listPipelines).mockImplementation(async () => []);

    const start = await openDictation();
    const notice = await screen.findByTestId('dictation-availability');
    expect(notice).toHaveTextContent(/advertises no dictation modes/i);
    expect(start).toBeDisabled();

    fireEvent.click(start);
    expect(bridge.start).not.toHaveBeenCalled();
  });

  it('shows a failed listing with a working retry and keeps Stop/Cancel usable for an active session', async () => {
    vi.mocked(bridge.status).mockImplementation(async () =>
      installedRecoveryStatus({
        active_session: { state: 'recording', pipeline_id: CLEAN_MODE_ID },
      }),
    );
    vi.mocked(bridge.listPipelines).mockRejectedValue({
      code: 'daemon_unavailable',
      message: 'listing failed',
    });

    const start = await openDictation();
    const notice = await screen.findByTestId('dictation-availability');
    expect(notice).toHaveTextContent(/could not load available modes/i);

    const stop = screen.getByTestId('dictation-stop');
    const cancel = screen.getByTestId('dictation-cancel');
    await waitFor(() => expect(stop).toBeEnabled());
    expect(cancel).toBeEnabled();

    fireEvent.click(stop);
    await waitFor(() => expect(bridge.stop).toHaveBeenCalledTimes(1));
    fireEvent.click(cancel);
    await waitFor(() => expect(bridge.cancel).toHaveBeenCalledTimes(1));
    expect(start).toBeDisabled();

    // Retry is explicit and recovers once the daemon answers again.
    vi.mocked(bridge.listPipelines).mockResolvedValue(STRICT_MODES_ENABLED);
    fireEvent.click(screen.getByTestId('dictation-pipelines-refresh'));
    await waitFor(() =>
      expect(screen.queryByTestId('dictation-availability')).not.toBeInTheDocument(),
    );
  });

  it('cannot issue a stale start after the selected lane disappears from the advertisement', async () => {
    vi.mocked(bridge.status).mockImplementation(async () => readyRecoveryStatus());
    vi.mocked(bridge.listPipelines).mockResolvedValue(STRICT_MODES_ENABLED);

    const start = await openDictation();
    await waitFor(() => expect(start).toBeEnabled());

    vi.mocked(bridge.listPipelines).mockResolvedValue([]);
    fireEvent.click(screen.getByTestId('dictation-pipelines-refresh'));
    await waitFor(() =>
      expect(screen.getByTestId('dictation-availability')).toHaveTextContent(
        /advertises no dictation modes/i,
      ),
    );

    fireEvent.click(screen.getByTestId('dictation-start'));
    await act(async () => {
      await Promise.resolve();
    });
    expect(bridge.start).not.toHaveBeenCalled();
    expect(screen.getByTestId('dictation-start')).toBeDisabled();
  });

  it('cannot start from an in-flight refresh that removes the selection before it settles', async () => {
    vi.mocked(bridge.status).mockImplementation(async () => readyRecoveryStatus());
    vi.mocked(bridge.listPipelines).mockResolvedValue(STRICT_MODES_ENABLED);

    const start = await openDictation();
    await waitFor(() => expect(start).toBeEnabled());

    let resolveRefresh: ((pipelines: PipelineSummary[]) => void) | null = null;
    vi.mocked(bridge.listPipelines).mockImplementation(
      () =>
        new Promise<PipelineSummary[]>((resolve) => {
          resolveRefresh = resolve;
        }),
    );

    fireEvent.click(screen.getByTestId('dictation-pipelines-refresh'));
    fireEvent.click(start);
    await act(async () => {
      await Promise.resolve();
    });
    // Start waits for the in-flight advertisement instead of racing it.
    expect(bridge.start).not.toHaveBeenCalled();

    await act(async () => {
      resolveRefresh?.([]);
      await Promise.resolve();
    });
    await waitFor(() => expect(screen.getByTestId('dictation-start')).toBeDisabled());
    expect(bridge.start).not.toHaveBeenCalled();
  });

  it('preserves the selection across refresh while the daemon still advertises it', async () => {
    vi.mocked(bridge.status).mockImplementation(async () => readyRecoveryStatus());
    vi.mocked(bridge.listPipelines).mockResolvedValue(STRICT_MODES_ENABLED);

    const start = await openDictation();
    await waitFor(() => expect(start).toBeEnabled());

    vi.mocked(bridge.listPipelines).mockResolvedValue([OTHER_ENABLED, ...STRICT_MODES_ENABLED]);
    fireEvent.click(screen.getByTestId('dictation-pipelines-refresh'));
    await waitFor(() => {
      const select = screen.getByTestId('dictation-mode-select') as HTMLSelectElement;
      expect(Array.from(select.options).map((o) => o.value)).toEqual([
        'other-lane',
        CLEAN_MODE_ID,
        RAW_MODE_ID,
      ]);
      expect(select.value).toBe(CLEAN_MODE_ID);
    });
  });
});
