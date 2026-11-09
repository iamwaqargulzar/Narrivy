// A pinned-phase `useSession` for hook-level tests: the dictation latch and
// the pending-start flag behave as in SessionProvider, per component.
import { useRef, useState } from 'react';
import type { DictationLatch } from '../state/SessionContext';

export function pinnedSession(phase: string) {
  const dictationLatch = useRef<DictationLatch>({ startInFlight: false, cancelIntent: null });
  const [startPending, setStartPending] = useState(false);
  return { phase, dictationLatch, startPending, setStartPending };
}
