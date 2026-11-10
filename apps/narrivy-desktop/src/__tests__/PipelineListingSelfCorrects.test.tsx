// The mode listing is read once per readiness change. If that read lands just
// before the daemon becomes ready, the app must notice that a later status
// disagrees and read the listing again, instead of staying "unavailable".
import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, render, screen } from '@testing-library/react';
import App from '../App';
import { bridge } from '../lib/bridge';
import { LIVE_REFRESH_MS } from '../state/StatusContext';
import { listingDisagreesWithStatus } from '../state/PipelinesContext';

vi.mock('../lib/bridge', async () => {
  const { createMockBridgeModule } = await import('../test/mockBridge');
  return createMockBridgeModule();
});

const READY_STATUS = {
  capabilities: ['model.gguf=available', 'asr.transcribe-cpp.worker=ready', 'pipeline.clean=enabled', 'pipeline.raw=enabled'],
  active_session: { state: 'idle', pipeline_id: null },
  daemon_build: 'narrivyd test',
  protocol_minor: 12,
  installed_model: null,
};
const lanes = (enabled: boolean) => [
  { id: 'clean', name: 'Clean', version: 1, enabled },
  { id: 'raw', name: 'Raw', version: 1, enabled },
];

afterEach(() => {
  vi.useRealTimers();
});

describe('PipelineListingSelfCorrects', () => {
  it('detects a listing that disagrees with status, and ignores lanes status does not mention', () => {
    expect(listingDisagreesWithStatus(lanes(false), READY_STATUS as never)).toBe(true);
    expect(listingDisagreesWithStatus(lanes(true), READY_STATUS as never)).toBe(false);
    expect(listingDisagreesWithStatus(lanes(false), { ...READY_STATUS, capabilities: [] } as never)).toBe(false);
  });

  it('re-reads a stale "disabled" listing once status says the modes are enabled', async () => {
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval', 'setTimeout', 'clearTimeout'] });
    // A fresh object per read, as the real bridge returns.
    vi.mocked(bridge.status).mockImplementation(async () => ({ ...READY_STATUS }) as never);
    // Every read so far landed before the worker was ready.
    vi.mocked(bridge.listPipelines).mockResolvedValue(lanes(false));
    render(<App />);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(screen.getAllByText(/No dictation mode is currently available/).length).toBeGreaterThan(0);

    // The daemon now answers "enabled"; the status tokens have not changed.
    vi.mocked(bridge.listPipelines).mockResolvedValue(lanes(true));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(LIVE_REFRESH_MS);
    });
    expect(screen.queryByText(/No dictation mode is currently available/)).toBeNull();
    expect(screen.getByTestId('dictation-start')).not.toBeDisabled();
  });
});
