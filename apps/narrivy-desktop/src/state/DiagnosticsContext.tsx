import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useState,
  type ReactNode,
} from 'react';
import { bridge, type BridgeError, type CapabilityCheckResult } from '../lib/bridge';
import { useErrorHandler } from '../ui/primitives/Toast';

export const DIAGNOSTIC_CHECKS = [
  { id: 'mic', titleKey: 'diag.mic.title' },
  { id: 'delivery', titleKey: 'diag.delivery.title' },
  { id: 'shortcuts', titleKey: 'diag.shortcuts.title' },
  { id: 'theme-store', titleKey: 'diag.theme.title' },
] as const;

export type DiagnosticId = (typeof DIAGNOSTIC_CHECKS)[number]['id'];

export interface CheckState {
  status: 'idle' | 'running' | 'done' | 'error';
  result?: CapabilityCheckResult;
  error?: BridgeError;
}

interface DiagnosticsContextValue {
  states: Record<string, CheckState>;
  runCheck: (id: DiagnosticId) => Promise<void>;
  runAll: () => Promise<void>;
  runningAll: boolean;
}

const DiagnosticsContext = createContext<DiagnosticsContextValue | null>(null);

export function DiagnosticsProvider({ children }: { children: ReactNode }) {
  const report = useErrorHandler();
  const [states, setStates] = useState<Record<string, CheckState>>({});
  const [runningAll, setRunningAll] = useState(false);

  const runCheck = useCallback(
    async (id: DiagnosticId) => {
      setStates((prev) => ({ ...prev, [id]: { status: 'running' } }));
      try {
        const result = await bridge.capabilityCheck(id);
        setStates((prev) => ({ ...prev, [id]: { status: 'done', result } }));
      } catch (err) {
        setStates((prev) => ({
          ...prev,
          [id]: { status: 'error', error: { code: 'unknown', message: String(err) } },
        }));
        report(err, { retry: () => void runCheck(id) });
      }
    },
    [report],
  );

  const runAll = useCallback(async () => {
    setRunningAll(true);
    try {
      for (const check of DIAGNOSTIC_CHECKS) {
        // sequential keeps result cards stable and avoids overlapping checks
        await runCheck(check.id);
      }
    } finally {
      setRunningAll(false);
    }
  }, [runCheck]);

  const value = useMemo<DiagnosticsContextValue>(
    () => ({ states, runCheck, runAll, runningAll }),
    [states, runCheck, runAll, runningAll],
  );

  return <DiagnosticsContext.Provider value={value}>{children}</DiagnosticsContext.Provider>;
}

export function useDiagnostics(): DiagnosticsContextValue {
  const ctx = useContext(DiagnosticsContext);
  if (!ctx) throw new Error('useDiagnostics must be used within DiagnosticsProvider');
  return ctx;
}
