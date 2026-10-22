import { act, renderHook } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { StatusProvider, useDaemonStatus } from '../state/StatusContext';
import { bridge, type StatusSnapshot } from '../lib/bridge';
import { installedRecoveryStatus, readyRecoveryStatus } from '../test/installedRecoveryFixtures';

vi.mock('../lib/bridge', async () => {
  const { createMockBridgeModule } = await import('../test/mockBridge');
  return createMockBridgeModule();
});
const report = vi.hoisted(() => vi.fn());
vi.mock('../ui/primitives/Toast', () => ({ useErrorHandler: () => report }));

beforeEach(() => { vi.mocked(bridge.status).mockReset(); report.mockReset(); });

it('does not let an older awaiting-model response overwrite a newer ready response', async () => {
  let resolveOld!: (snapshot: StatusSnapshot) => void;
  let resolveNew!: (snapshot: StatusSnapshot) => void;
  vi.mocked(bridge.status)
    .mockReturnValueOnce(new Promise(done => { resolveOld = done; }))
    .mockReturnValueOnce(new Promise(done => { resolveNew = done; }));
  const { result } = renderHook(() => useDaemonStatus(), {
    wrapper: ({ children }) => <StatusProvider>{children}</StatusProvider>,
  });
  expect(bridge.status).toHaveBeenCalledTimes(1);
  let reload!: Promise<void>;
  act(() => { reload = result.current.reload(); });
  expect(bridge.status).toHaveBeenCalledTimes(2);
  const ready = readyRecoveryStatus();
  await act(async () => { resolveNew(ready); await reload; });
  expect(result.current.status?.installed_model).toEqual(ready.installed_model);
  await act(async () => { resolveOld(installedRecoveryStatus()); });
  expect(result.current.status?.installed_model).toEqual(ready.installed_model);
});
