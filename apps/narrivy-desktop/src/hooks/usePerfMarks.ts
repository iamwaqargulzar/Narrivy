// Hook: trigger a listening-visible mark when a phase becomes 'recording'.
// Separation keeps Dictation.tsx free of low-level performance boilerplate.

import { useEffect } from 'react';
import { markListeningVisible, measureStartToListening } from '../lib/performance';

export function usePerfMarks(phase: string): void {
  useEffect(() => {
    if (phase === 'recording') {
      markListeningVisible();
      // eagerly compute measure so duration is available for any assertion
      measureStartToListening();
    }
  }, [phase]);
}

export function useListeningPerfMark(isListening: boolean): void {
  useEffect(() => {
    if (isListening) {
      markListeningVisible();
      measureStartToListening();
    }
  }, [isListening]);
}
