// F-M11: Home, the command palette and Dictation each use their own
// `useDictationActions`; a Cancel from one must stop a Start another is
// still resolving. The latch lives in the one SessionProvider.
import { act, renderHook } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { useDictationActions } from '../hooks/useDictationActions';
import { SessionProvider } from '../state/SessionContext';

const controls = vi.hoisted(() => ({
  resolveStartId: vi.fn(), refresh: vi.fn(), start: vi.fn(),
  stop: vi.fn(), cancel: vi.fn(), report: vi.fn(),
}));
vi.mock('../state/PipelinesContext', () => ({ usePipelines: () => ({
  catalog: { availability: 'ready' },
  resolveStartId: controls.resolveStartId, refresh: controls.refresh,
}) }));
vi.mock('../state/StatusContext', () => ({
  useDaemonStatus: () => ({ status: null, reload: async () => {} }),
}));
vi.mock('../ui/primitives/Toast', () => ({ useErrorHandler: () => controls.report }));
vi.mock('../lib/bridge', () => ({ bridge: {
  start: controls.start, stop: controls.stop, cancel: controls.cancel,
  onSessionEvent: async () => () => {},
  recoveryEntry: async () => null,
} }));

it('a Cancel from the palette stops a Start that Home is still resolving', async () => {
  controls.cancel.mockResolvedValue(undefined);
  controls.start.mockResolvedValue(undefined);
  let resolve!: (id: string) => void;
  controls.resolveStartId.mockReturnValue(new Promise<string>((done) => { resolve = done; }));
  const { result } = renderHook(
    () => ({ home: useDictationActions(), palette: useDictationActions() }),
    { wrapper: SessionProvider },
  );
  let starting!: Promise<boolean>;
  act(() => { starting = result.current.home.start(); });
  // The palette sees the pending Start and may cancel it.
  expect(result.current.palette.canCancel).toBe(true);
  await act(async () => { await result.current.palette.cancel(); });
  await act(async () => { resolve('clean'); await starting; });
  expect(controls.start).not.toHaveBeenCalled();
  expect(result.current.home.canCancel).toBe(false);
});
