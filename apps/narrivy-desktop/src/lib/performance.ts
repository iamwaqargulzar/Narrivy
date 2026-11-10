// Performance marks for Dictation start -> Listening pill (PERF-004).
// Target: <100ms UI feedback after user intent. Backend baselined 0.014ms;
// this module proves the frontend renders the listening state quickly.

export const PERF_MARKS = {
  DICTATION_START_CLICK: 'dictation:start-click',
  DICTATION_LISTENING_VISIBLE: 'dictation:listening-visible',
  DICTATION_MEASURE: 'dictation:start-to-listening',
} as const;

function safeMark(name: string): void {
  try {
    // performance may be unavailable in some test sandboxes
    if (typeof performance !== 'undefined' && typeof performance.mark === 'function') {
      performance.mark(name);
    }
  } catch {
    // no-op
  }
}

function safeMeasure(name: string, start: string, end: string): number | null {
  try {
    if (typeof performance !== 'undefined' && typeof performance.measure === 'function') {
      performance.measure(name, start, end);
      const entries = performance.getEntriesByName(name);
      const last = entries[entries.length - 1] as PerformanceMeasure | undefined;
      return last ? last.duration : null;
    }
  } catch {
    // ignore missing marks
  }
  return null;
}

export function markStartClick(): void {
  safeMark(PERF_MARKS.DICTATION_START_CLICK);
}

export function markListeningVisible(): void {
  safeMark(PERF_MARKS.DICTATION_LISTENING_VISIBLE);
}

export function measureStartToListening(): number | null {
  return safeMeasure(
    PERF_MARKS.DICTATION_MEASURE,
    PERF_MARKS.DICTATION_START_CLICK,
    PERF_MARKS.DICTATION_LISTENING_VISIBLE,
  );
}

export function clearPerf(): void {
  try {
    if (typeof performance !== 'undefined') {
      if (typeof performance.clearMarks === 'function') performance.clearMarks();
      if (typeof performance.clearMeasures === 'function') performance.clearMeasures();
    }
  } catch {
    // no-op
  }
}

export function getLastMeasureDuration(name: string = PERF_MARKS.DICTATION_MEASURE): number | null {
  try {
    const entries = performance.getEntriesByName(name);
    const last = entries[entries.length - 1] as PerformanceMeasure | undefined;
    return last ? last.duration : null;
  } catch {
    return null;
  }
}
