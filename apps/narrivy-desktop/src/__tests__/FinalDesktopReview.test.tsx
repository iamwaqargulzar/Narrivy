import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { StatusProvider, useDaemonStatus } from '../state/StatusContext';
import { PipelinesProvider, usePipelines } from '../state/PipelinesContext';
import { bridge, type PipelineSummary } from '../lib/bridge';
import { installedRecoveryStatus } from '../test/installedRecoveryFixtures';

vi.mock('../lib/bridge', async () => {
  const { createMockBridgeModule } = await import('../test/mockBridge');
  return createMockBridgeModule();
});
const report = vi.hoisted(() => vi.fn());
vi.mock('../ui/primitives/Toast', () => ({ useErrorHandler: () => report }));
beforeEach(() => {
  report.mockReset();
  vi.mocked(bridge.status).mockReset();
  vi.mocked(bridge.listPipelines).mockReset();
});
afterEach(() => { vi.useRealTimers(); vi.restoreAllMocks(); });

it('exhausts the operation budget on the first late scheduler wake', async () => {
  vi.useFakeTimers();
  let monotonicMs = 0;
  vi.spyOn(performance, 'now').mockImplementation(() => monotonicMs);
  vi.mocked(bridge.status).mockResolvedValue(installedRecoveryStatus({
    model_provisioning: {
      operation_id: 'late-wake', state: 'activating',
      publication: 'not_published', cleanup: 'not_required', revision: 1,
    },
  }));
  renderHook(() => useDaemonStatus(), {
    wrapper: ({ children }) => <StatusProvider>{children}</StatusProvider>,
  });
  await act(async () => { await vi.advanceTimersByTimeAsync(0); });
  expect(bridge.status).toHaveBeenCalledTimes(1);
  // A throttled/background scheduler wakes after the wall budget expired;
  // it has not executed 120 timer callbacks in the meantime.
  monotonicMs = 95_000;
  await act(async () => { await vi.advanceTimersByTimeAsync(750); });
  expect(report).toHaveBeenCalledTimes(1);
  expect(bridge.status).toHaveBeenCalledTimes(1);
});

it('does not substitute a newly selected mode into an already pending Start', async () => {
  vi.mocked(bridge.status).mockReturnValue(new Promise(() => {}));
  const modes: PipelineSummary[] = [
    { id: 'clean', name: 'Clean', version: 1, enabled: true },
    { id: 'raw', name: 'Raw', version: 1, enabled: true },
  ];
  vi.mocked(bridge.listPipelines).mockResolvedValue(modes);
  const { result } = renderHook(() => usePipelines(), {
    wrapper: ({ children }) => <StatusProvider><PipelinesProvider>{children}</PipelinesProvider></StatusProvider>,
  });
  await waitFor(() => expect(result.current.catalog.selectedId).toBe('clean'));
  let settle!: (value: PipelineSummary[]) => void;
  vi.mocked(bridge.listPipelines).mockReturnValue(new Promise(done => { settle = done; }));
  let refreshing!: Promise<void>;
  let resolving!: Promise<string | null>;
  act(() => { refreshing = result.current.refresh(); resolving = result.current.resolveStartId(); });
  act(() => { result.current.select('raw'); });
  let chosen: string | null = null;
  await act(async () => { settle(modes); await refreshing; chosen = await resolving; });
  expect(chosen).not.toBe('raw');
  expect(result.current.catalog.selectedId).toBe('raw');
  await expect(result.current.resolveStartId()).resolves.toBe('raw');
});
