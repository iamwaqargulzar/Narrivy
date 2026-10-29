// Deterministic promise-controlled regressions for the pipeline advertisement:
// overlapping refreshes, failed listings, selection disappearance during an
// in-flight start, capability-driven readiness refresh and cancellation while
// the advertisement is still resolving.
//
// Every listing/status read is a controlled promise so the exact ordering is
// forced by the test, never by timing.
import { act, fireEvent, render, renderHook, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ReactNode } from 'react';
import App from '../App';
import { useDictationActions } from '../hooks/useDictationActions';
import { bridge, type PipelineSummary, type StatusSnapshot } from '../lib/bridge';
import { PipelinesProvider, usePipelines } from '../state/PipelinesContext';
import { StatusProvider, useDaemonStatus } from '../state/StatusContext';
import {
  CLEAN_MODE,
  CLEAN_MODE_ID,
  readyRecoveryStatus,
} from '../test/installedRecoveryFixtures';

vi.mock('../lib/bridge', async () => {
  const { createMockBridgeModule } = await import('../test/mockBridge');
  return createMockBridgeModule();
});
// Keep the real ToastProvider for screen-level renders, but capture the error
// handler so provider-level tests can assert what was surfaced.
const report = vi.hoisted(() => vi.fn());
vi.mock('../ui/primitives/Toast', async (importOriginal) => {
  const actual = (await importOriginal()) as Record<string, unknown>;
  return { ...actual, useErrorHandler: () => report };
});
// Keep the real SessionProvider for screen-level renders, but pin the session
// phase so the action hooks are exercised without session-event coupling.
vi.mock('../state/SessionContext', async (importOriginal) => {
  const actual = (await importOriginal()) as Record<string, unknown>;
  const { pinnedSession } = await import('../test/sessionHookMock');
  return { ...actual, useSession: () => pinnedSession('idle') };
});

const CLEAN_MODE_DISABLED: PipelineSummary = { ...CLEAN_MODE, enabled: false };

const OTHER_ENABLED: PipelineSummary = {
  id: 'other-lane',
  name: 'Other advertised lane',
  version: 1,
  enabled: true,
};

/** A status read that never settles: pipelines-only tests then have exactly
 * one readiness-triggered refresh and no timing coupling to status. */
function pendingStatus(): Promise<StatusSnapshot> {
  return new Promise(() => {});
}

function wrapper({ children }: { children: ReactNode }) {
  return (
    <StatusProvider>
      <PipelinesProvider>{children}</PipelinesProvider>
    </StatusProvider>
  );
}

function ids(list: PipelineSummary[]): string[] {
  return list.map((p) => p.id);
}

beforeEach(() => {
  window.localStorage.setItem('narrivy.onboarded.v1', '1');
  report.mockReset();
  vi.mocked(bridge.status).mockReset().mockResolvedValue(readyRecoveryStatus());
  vi.mocked(bridge.listPipelines).mockReset();
  vi.mocked(bridge.start).mockReset().mockResolvedValue(undefined);
  vi.mocked(bridge.stop).mockReset().mockResolvedValue(undefined);
  vi.mocked(bridge.cancel).mockReset().mockResolvedValue(undefined);
});

describe('PipelineFreshnessRace', () => {
  it('ignores an older listing response that settles after a newer refresh', async () => {
    vi.mocked(bridge.status).mockImplementation(pendingStatus);
    vi.mocked(bridge.listPipelines).mockResolvedValue([CLEAN_MODE]);
    const { result } = renderHook(() => usePipelines(), { wrapper });
    await waitFor(() => expect(result.current.catalog.availability).toBe('ready'));

    let resolveOlder!: (list: PipelineSummary[]) => void;
    let resolveNewer!: (list: PipelineSummary[]) => void;
    let controlled = 0;
    vi.mocked(bridge.listPipelines).mockImplementation(() => {
      controlled += 1;
      return controlled === 1
        ? new Promise((done) => { resolveOlder = done; })
        : new Promise((done) => { resolveNewer = done; });
    });

    let olderRequest!: Promise<void>;
    act(() => {
      olderRequest = result.current.refresh();
    });
    await waitFor(() => expect(bridge.listPipelines).toHaveBeenCalledTimes(2));
    let newerRequest!: Promise<void>;
    act(() => {
      newerRequest = result.current.refresh();
    });
    await waitFor(() => expect(bridge.listPipelines).toHaveBeenCalledTimes(3));
    vi.mocked(bridge.listPipelines).mockResolvedValue([CLEAN_MODE]);

    await act(async () => {
      resolveNewer([OTHER_ENABLED]);
      await newerRequest;
    });
    expect(ids(result.current.catalog.pipelines)).toEqual([OTHER_ENABLED.id]);
    expect(result.current.loading).toBe(false);
    expect(result.current.error).toBeNull();

    // The older read settles last; it must not resurrect its snapshot.
    await act(async () => {
      resolveOlder([CLEAN_MODE]);
      await olderRequest;
    });
    expect(ids(result.current.catalog.pipelines)).toEqual([OTHER_ENABLED.id]);
    expect(result.current.catalog.selectedId).toBe(OTHER_ENABLED.id);
    expect(result.current.loading).toBe(false);
    expect(result.current.error).toBeNull();
  });

  it('keeps last-known lanes on a failed listing but rejects Start on that failed advertisement', async () => {
    vi.mocked(bridge.status).mockImplementation(pendingStatus);
    vi.mocked(bridge.listPipelines).mockResolvedValue([CLEAN_MODE]);
    const { result } = renderHook(
      () => ({ pipelines: usePipelines(), actions: useDictationActions() }),
      { wrapper },
    );
    await waitFor(() => expect(result.current.pipelines.catalog.availability).toBe('ready'));

    // Newest advertisement failed: last-known lanes stay for context...
    vi.mocked(bridge.listPipelines).mockRejectedValue({
      code: 'daemon_unavailable',
      message: 'listing failed',
    });
    await act(async () => {
      await result.current.pipelines.refresh();
    });
    expect(result.current.pipelines.error).not.toBeNull();
    expect(ids(result.current.pipelines.catalog.pipelines)).toEqual([CLEAN_MODE_ID]);

    // ...but Start must not run on stale/failed listing data.
    await expect(result.current.pipelines.resolveStartId()).resolves.toBeNull();
    let started: boolean | null = null;
    await act(async () => {
      started = await result.current.actions.start();
    });
    expect(started).toBe(false);
    expect(bridge.start).not.toHaveBeenCalled();

    // Explicit recovery: the next successful advertisement restores Start.
    vi.mocked(bridge.listPipelines).mockResolvedValue([CLEAN_MODE]);
    await act(async () => {
      await result.current.pipelines.refresh();
    });
    await act(async () => {
      expect(await result.current.pipelines.resolveStartId()).toBe(CLEAN_MODE_ID);
      expect(await result.current.actions.start()).toBe(true);
    });
    expect(bridge.start).toHaveBeenCalledTimes(1);
    expect(bridge.start).toHaveBeenCalledWith(CLEAN_MODE_ID);
  });

  it('never substitutes the fallback lane for an in-flight start and requires an intentional next start', async () => {
    vi.mocked(bridge.status).mockImplementation(pendingStatus);
    vi.mocked(bridge.listPipelines).mockResolvedValue([CLEAN_MODE, OTHER_ENABLED]);
    const { result } = renderHook(
      () => ({ pipelines: usePipelines(), actions: useDictationActions() }),
      { wrapper },
    );
    await waitFor(() =>
      expect(result.current.pipelines.catalog.selectedId).toBe(CLEAN_MODE_ID),
    );

    // The daemon drops the selected lane while a Start is waiting on it.
    let resolveRefresh!: (list: PipelineSummary[]) => void;
    vi.mocked(bridge.listPipelines).mockImplementation(
      () => new Promise((done) => { resolveRefresh = done; }),
    );
    let refreshPromise!: Promise<void>;
    act(() => {
      refreshPromise = result.current.pipelines.refresh();
    });
    let startPromise!: Promise<boolean>;
    act(() => {
      startPromise = result.current.actions.start();
    });
    vi.mocked(bridge.listPipelines).mockResolvedValue([OTHER_ENABLED]);
    await act(async () => {
      resolveRefresh([OTHER_ENABLED]);
      await refreshPromise;
    });

    // The fallback lane was recorded and surfaced, not started.
    expect(bridge.start).not.toHaveBeenCalled();
    expect(result.current.pipelines.selectionChange).toMatchObject({
      from: CLEAN_MODE_ID,
      to: OTHER_ENABLED.id,
    });
    let started: boolean | null = null;
    await act(async () => {
      started = await startPromise;
    });
    expect(started).toBe(false);
    expect(bridge.start).not.toHaveBeenCalled();
    expect(result.current.pipelines.catalog.selectedId).toBe(OTHER_ENABLED.id);

    // The intentional next start uses the lane that is now selected.
    await act(async () => {
      expect(await result.current.actions.start()).toBe(true);
    });
    expect(bridge.start).toHaveBeenCalledTimes(1);
    expect(bridge.start).toHaveBeenCalledWith(OTHER_ENABLED.id);
  });

  it('refreshes the advertisement when only the actual capabilities change', async () => {
    const warming = readyRecoveryStatus({
      capabilities: [
        'model.gguf=available',
        'asr.transcribe-cpp.cpu=available',
        'asr.transcribe-cpp.runtime=warming',
        'pipeline.linux-mic-inbox=warming',
      ],
    });
    const residentReady = readyRecoveryStatus({
      capabilities: [
        'model.gguf=available',
        'asr.transcribe-cpp.cpu=available',
        'asr.transcribe-cpp.runtime=resident-worker',
        'pipeline.linux-mic-inbox=enabled',
      ],
    });
    // Same lifecycle and no provision revision: only the actual capability
    // tokens move (resident warming -> ready).
    expect(warming.installed_model).toBe(residentReady.installed_model);
    expect(warming.model_provisioning).toBeUndefined();
    expect(residentReady.model_provisioning).toBeUndefined();

    let resolveWarming!: (snapshot: StatusSnapshot) => void;
    vi.mocked(bridge.status)
      .mockReset()
      .mockImplementationOnce(() => new Promise((done) => { resolveWarming = done; }))
      .mockResolvedValue(residentReady);
    vi.mocked(bridge.listPipelines).mockResolvedValue([CLEAN_MODE_DISABLED]);

    const { result } = renderHook(
      () => ({ status: useDaemonStatus(), pipelines: usePipelines() }),
      { wrapper },
    );
    await waitFor(() => expect(bridge.listPipelines).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(result.current.pipelines.catalog.availability).toBe('unavailable'));

    await act(async () => {
      resolveWarming(warming);
    });
    await waitFor(() => expect(bridge.listPipelines).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(result.current.status.status?.installed_model).toBe('ready'));
    expect(result.current.pipelines.catalog.availability).toBe('unavailable');

    // Only the capability tokens changed; that alone must re-read the lanes.
    const callsBefore = vi.mocked(bridge.listPipelines).mock.calls.length;
    vi.mocked(bridge.listPipelines).mockResolvedValue([CLEAN_MODE]);
    await act(async () => {
      await result.current.status.reload();
    });

    await waitFor(() => expect(result.current.pipelines.catalog.availability).toBe('ready'));
    expect(bridge.listPipelines).toHaveBeenCalledTimes(callsBefore + 1);
    expect(result.current.pipelines.catalog.selectedId).toBe(CLEAN_MODE_ID);
  });

  it('honors Cancel while the daemon advertisement is still resolving and prevents the late start', async () => {
    vi.mocked(bridge.status).mockImplementation(pendingStatus);
    vi.mocked(bridge.listPipelines).mockResolvedValue([CLEAN_MODE]);
    const { result } = renderHook(
      () => ({ pipelines: usePipelines(), actions: useDictationActions() }),
      { wrapper },
    );
    await waitFor(() => expect(result.current.pipelines.catalog.availability).toBe('ready'));

    let resolveRefresh!: (list: PipelineSummary[]) => void;
    vi.mocked(bridge.listPipelines).mockImplementation(
      () => new Promise((done) => { resolveRefresh = done; }),
    );
    let refreshPromise!: Promise<void>;
    act(() => {
      refreshPromise = result.current.pipelines.refresh();
    });
    let startPromise!: Promise<boolean>;
    act(() => {
      startPromise = result.current.actions.start();
    });
    // Cancel is offered while Start/listing is pending...
    expect(result.current.actions.canCancel).toBe(true);
    let accepted = false;
    await act(async () => {
      accepted = await result.current.actions.cancel();
    });
    expect(accepted).toBe(true);
    expect(bridge.cancel).toHaveBeenCalledTimes(1);

    // ...and once the advertisement resolves, the late Start is not issued.
    await act(async () => {
      resolveRefresh([CLEAN_MODE]);
      await refreshPromise;
    });
    let started: boolean | null = null;
    await act(async () => {
      started = await startPromise;
    });
    expect(started).toBe(false);
    expect(bridge.start).not.toHaveBeenCalled();
  });

  it('surfaces the refresh-driven selection change on Home before an intentional start', async () => {
    vi.mocked(bridge.listPipelines).mockResolvedValue([CLEAN_MODE, OTHER_ENABLED]);
    render(<App />);
    await waitFor(() => expect(screen.getByTestId('dictation-selected-id')).toHaveTextContent(CLEAN_MODE_ID));

    vi.mocked(bridge.listPipelines).mockResolvedValue([OTHER_ENABLED]);
    fireEvent.click(screen.getByTestId('dictation-pipelines-refresh'));

    const notice = await screen.findByTestId('dictation-selection-change');
    expect(notice).toHaveTextContent(/Selected mode is now other-lane/);
    await waitFor(() => expect(screen.getByTestId('dictation-selected-id')).toHaveTextContent(OTHER_ENABLED.id));
    // The refresh alone never starts a lane.
    expect(bridge.start).not.toHaveBeenCalled();

    // The intentional Start answers the surfaced change and uses the new lane.
    fireEvent.click(screen.getByTestId('dictation-start'));
    await waitFor(() => expect(bridge.start).toHaveBeenCalledWith(OTHER_ENABLED.id));
    await waitFor(() =>
      expect(screen.queryByTestId('dictation-selection-change')).not.toBeInTheDocument(),
    );
  });
});
