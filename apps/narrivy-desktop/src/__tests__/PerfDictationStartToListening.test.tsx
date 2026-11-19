import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import App from '../App';
import { bridge } from '../lib/bridge';
import { PERF_MARKS, clearPerf } from '../lib/performance';

vi.mock('../lib/bridge', async () => {
  const { createMockBridgeModule } = await import('../test/mockBridge');
  return createMockBridgeModule();
});

const ONBOARD_KEY = 'narrivy.onboarded.v1';

describe('PerfDictationStartToListening', () => {
  beforeEach(() => {
    window.localStorage.setItem(ONBOARD_KEY, '1');
    clearPerf();
    try { performance.clearMarks(); performance.clearMeasures(); } catch {}
  });
  afterEach(() => {
    vi.useRealTimers();
    try { performance.clearMarks(); performance.clearMeasures(); } catch {}
  });

  it('marks start-click -> listening-visible in <100ms (PERF-004)', async () => {
    let emit: ((state: string) => void) | null = null;
    vi.mocked(bridge.onSessionEvent).mockImplementation(async (handler) => {
      emit = (state: string) => handler({ seq: 1, state });
      return () => {};
    });

    render(<App />);
    const dictBtn = await screen.findByRole('button', { name: 'Home' });
    fireEvent.click(dictBtn);
    await waitFor(() => expect(document.querySelector('.topbar-status')?.getAttribute('data-phase')).toBe('idle'));

    const startBtn = await screen.findByTestId('dictation-start');
    // The advertised pipeline must be loaded before Start is enabled. Wait
    // with real timers, then switch to fake timers for the perf window.
    await waitFor(() => expect(startBtn).toBeEnabled());
    // Fake the clocks only: the measured marks need the real `performance`.
    vi.useFakeTimers({
      shouldAdvanceTime: true,
      toFake: ['setTimeout', 'clearTimeout', 'setInterval', 'clearInterval', 'Date'],
    });

    await act(async () => {
      fireEvent.click(startBtn);
      await Promise.resolve();
    });

    await vi.advanceTimersByTimeAsync(10);
    await waitFor(() => expect(emit).not.toBeNull());
    await act(async () => {
      emit!('recording');
    });

    await vi.advanceTimersByTimeAsync(0);
    vi.useRealTimers();

    const marks = performance.getEntriesByName(PERF_MARKS.DICTATION_START_CLICK);
    expect(marks.length).toBeGreaterThan(0);

    const measures = performance.getEntriesByName(PERF_MARKS.DICTATION_MEASURE);
    let duration: number | null = null;
    if (measures.length > 0) {
      duration = (measures[measures.length - 1] as PerformanceMeasure).duration;
    } else {
      const startEntries = performance.getEntriesByName(PERF_MARKS.DICTATION_START_CLICK);
      const visEntries = performance.getEntriesByName(PERF_MARKS.DICTATION_LISTENING_VISIBLE);
      const startMark = startEntries[startEntries.length - 1] as PerformanceMark | undefined;
      const visMark = visEntries[visEntries.length - 1] as PerformanceMark | undefined;
      if (startMark && visMark) duration = visMark.startTime - startMark.startTime;
    }
    expect(duration).not.toBeNull();
    expect(duration!).toBeLessThan(100);
  });
});
