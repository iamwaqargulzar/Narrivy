import { act, renderHook } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { useDictationActions } from '../hooks/useDictationActions';

const controls = vi.hoisted(() => ({
  resolveStartId: vi.fn(), refresh: vi.fn(), start: vi.fn(),
  stop: vi.fn(), cancel: vi.fn(), report: vi.fn(),
}));
vi.mock('../state/PipelinesContext', () => ({ usePipelines: () => ({
  catalog: { availability: 'ready' },
  resolveStartId: controls.resolveStartId, refresh: controls.refresh,
}) }));
vi.mock('../state/SessionContext', async () => {
  const { pinnedSession } = await import('../test/sessionHookMock');
  return { useSession: () => pinnedSession('idle') };
});
vi.mock('../state/StatusContext', () => ({ useDaemonStatus: () => ({ status: null }) }));
vi.mock('../ui/primitives/Toast', () => ({ useErrorHandler: () => controls.report }));
vi.mock('../lib/bridge', () => ({ bridge: {
  start: controls.start, stop: controls.stop, cancel: controls.cancel,
} }));

beforeEach(() => {
  vi.resetAllMocks();
  controls.refresh.mockResolvedValue(undefined);
  controls.start.mockResolvedValue(undefined);
  controls.cancel.mockResolvedValue(undefined);
});

it('honors Cancel during pending pipeline resolution and prevents a late Start', async () => {
  let resolve!: (id: string) => void;
  controls.resolveStartId.mockReturnValue(new Promise<string>(done => { resolve = done; }));
  const { result } = renderHook(() => useDictationActions());
  let starting!: Promise<boolean>;
  act(() => { starting = result.current.start(); });
  expect(result.current.canCancel).toBe(true);
  let accepted = false;
  await act(async () => { accepted = await result.current.cancel(); });
  // Settle the pending request before asserting so a failure never leaves
  // an unresolved start behind in the test runner.
  await act(async () => { resolve('linux-mic-inbox'); await starting; });
  expect({ accepted, lateStarts: controls.start.mock.calls.length }).toEqual({
    accepted: true, lateStarts: 0,
  });
});

it('reconciles Cancel when Start acknowledgement arrives after the first cancellation', async () => {
  controls.resolveStartId.mockResolvedValue('clean');
  let ownedSessionActive = false;
  controls.cancel.mockImplementation(async () => { ownedSessionActive = false; });
  let acknowledge!: () => void;
  controls.start.mockReturnValue(new Promise<void>(done => { acknowledge = done; }));
  const { result } = renderHook(() => useDictationActions());
  let starting!: Promise<boolean>;
  await act(async () => { starting = result.current.start(); });
  expect(controls.start).toHaveBeenCalledTimes(1);
  await act(async () => { await result.current.cancel(); });
  // Separate Start/Cancel IPC connections can be admitted out of order.
  // A cancellation acknowledgement before Start admission cannot prove the
  // subsequently admitted session was cancelled.
  let started = true;
  await act(async () => {
    ownedSessionActive = true;
    acknowledge();
    started = await starting;
  });
  expect({ started, ownedSessionActive }).toEqual({
    started: false, ownedSessionActive: false,
  });
});
