// Deterministic timer/promise-controlled regressions for status freshness:
// one bounded deadline per provision operation (even across active-state
// transitions) with an explicit retry, and no publication after unmount.
import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ReactNode } from 'react';
import { StatusProvider, useDaemonStatus } from '../state/StatusContext';
import { bridge, type StatusSnapshot } from '../lib/bridge';
import { installedRecoveryStatus } from '../test/installedRecoveryFixtures';

vi.mock('../lib/bridge', async () => {
  const { createMockBridgeModule } = await import('../test/mockBridge');
  return createMockBridgeModule();
});
const report = vi.hoisted(() => vi.fn());
vi.mock('../ui/primitives/Toast', () => ({ useErrorHandler: () => report }));

const wrapper = ({ children }: { children: ReactNode }) => (
  <StatusProvider>{children}</StatusProvider>
);

const ACTIVE_PROVISION = {
  operation_id: 'op-deadline',
  state: 'activating',
  publication: 'not_published',
  cleanup: 'not_required',
  revision: 1,
};

function activeStatus(state = 'activating'): StatusSnapshot {
  return installedRecoveryStatus({
    model_provisioning: { ...ACTIVE_PROVISION, state },
  });
}

function flushMicrotasks(): Promise<void> {
  return Promise.resolve();
}

/** The provision freshness window is one absolute monotonic elapsed-time
 * deadline, so advancing the fake scheduler must advance `performance.now()`
 * with it exactly like the real wall clock. Faking only the timer callbacks
 * would freeze the monotonic clock and let a tick-counting implementation
 * pass. */
const FAKE_CLOCK_METHODS = [
  'setTimeout',
  'clearTimeout',
  'setInterval',
  'clearInterval',
  'setImmediate',
  'clearImmediate',
  'Date',
  'performance',
] as const;

function installDeadlineClock(): void {
  vi.useFakeTimers({ toFake: [...FAKE_CLOCK_METHODS] });
}

beforeEach(() => {
  report.mockReset();
  vi.mocked(bridge.status).mockReset();
});

afterEach(() => {
  vi.useRealTimers();
});

describe('StatusProvisionDeadline', () => {
  it('bounds an active provision poll by one operation deadline and surfaces a retry', async () => {
    installDeadlineClock();
    vi.mocked(bridge.status).mockResolvedValue(activeStatus());
    const { result } = renderHook(() => useDaemonStatus(), { wrapper });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(bridge.status).toHaveBeenCalledTimes(1);
    expect(result.current.status?.model_provisioning?.state).toBe('activating');

    await act(async () => {
      await vi.advanceTimersByTimeAsync(90_000);
    });
    await act(async () => {
      await flushMicrotasks();
    });

    const callsAtDeadline = vi.mocked(bridge.status).mock.calls.length;
    expect(report).toHaveBeenCalledTimes(1);
    const [err, opts] = report.mock.calls[0] as [
      { code: string; message: string },
      { retry?: () => void },
    ];
    expect(err.code).toBe('provision_status_stale');
    expect(err.message).toMatch(/did not report completion/i);
    expect(typeof opts.retry).toBe('function');

    // Polling stops at the deadline instead of silently retrying forever.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10_000);
    });
    expect(vi.mocked(bridge.status).mock.calls.length).toBe(callsAtDeadline);

    // The surfaced retry performs one immediate read and re-arms the window.
    await act(async () => {
      opts.retry?.();
      await flushMicrotasks();
    });
    const callsAtRetry = vi.mocked(bridge.status).mock.calls.length;
    expect(callsAtRetry).toBe(callsAtDeadline + 1);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(750);
    });
    expect(vi.mocked(bridge.status).mock.calls.length).toBeGreaterThan(callsAtRetry);
  });

  it('keeps one deadline across active state transitions of the same operation', async () => {
    installDeadlineClock();
    // The operation moves activating -> importing partway through; that must
    // not hand it a second full deadline.
    let calls = 0;
    vi.mocked(bridge.status).mockImplementation(async () => {
      calls += 1;
      return calls > 81 ? activeStatus('importing') : activeStatus('activating');
    });
    renderHook(() => useDaemonStatus(), { wrapper });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(91_000);
    });
    await act(async () => {
      await flushMicrotasks();
    });

    // Exhausted within the original ~90 s window...
    expect(report).toHaveBeenCalledTimes(1);
    const callsAtDeadline = vi.mocked(bridge.status).mock.calls.length;

    // ...and the state transition did not re-arm another window or re-report.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10_000);
    });
    expect(report).toHaveBeenCalledTimes(1);
    expect(vi.mocked(bridge.status).mock.calls.length).toBe(callsAtDeadline);
  });

  it('does not publish or report when an in-flight status read settles after unmount', async () => {
    let rejectStatus!: (err: unknown) => void;
    vi.mocked(bridge.status).mockReturnValueOnce(
      new Promise((_done, reject) => {
        rejectStatus = reject;
      }),
    );
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
    const { unmount } = renderHook(() => useDaemonStatus(), { wrapper });
    expect(bridge.status).toHaveBeenCalledTimes(1);

    unmount();
    await act(async () => {
      rejectStatus({ code: 'daemon_unavailable', message: 'daemon unreachable' });
      await flushMicrotasks();
    });

    expect(report).not.toHaveBeenCalled();
    expect(errorSpy).not.toHaveBeenCalled();
    errorSpy.mockRestore();
  });
});
