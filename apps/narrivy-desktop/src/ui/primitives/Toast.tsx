// Toast system with mandatory recovery action (UX-007):
// every error toast carries Retry or Open Diagnostics.
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react';
import { t } from '../../i18n';
import { toBridgeError } from '../../lib/bridge';
import { useNav } from '../../state/NavContext';
import { Button } from './Button';

export interface ToastAction {
  label: string;
  onClick: () => void;
}

interface ToastItem {
  id: number;
  message: string;
  action?: ToastAction;
}

interface ToastContextValue {
  push: (message: string, action?: ToastAction) => void;
}

const ToastContext = createContext<ToastContextValue | null>(null);

const TOAST_TTL_MS = 8000;

export function ToastProvider({ children }: { children: ReactNode }) {
  const [items, setItems] = useState<ToastItem[]>([]);
  const nextId = useRef(1);
  const timers = useRef<ReturnType<typeof setTimeout>[]>([]);

  useEffect(() => () => timers.current.forEach(clearTimeout), []);

  const dismiss = useCallback((id: number) => {
    setItems((prev) => prev.filter((item) => item.id !== id));
  }, []);

  const push = useCallback(
    (message: string, action?: ToastAction) => {
      const id = nextId.current++;
      setItems((prev) => [...prev, { id, message, action }]);
      timers.current.push(setTimeout(() => dismiss(id), TOAST_TTL_MS));
    },
    [dismiss],
  );

  const value = useMemo(() => ({ push }), [push]);

  return (
    <ToastContext.Provider value={value}>
      {children}
      <div className="toast-stack" role="status" aria-live="polite">
        {items.map((item) => (
          <div key={item.id} className="toast" data-testid="toast">
            <span>{item.message}</span>
            {item.action ? (
              <Button onClick={item.action.onClick}>{item.action.label}</Button>
            ) : null}
            <Button variant="ghost" aria-label={t('toast.dismiss')} onClick={() => dismiss(item.id)}>
              ✕
            </Button>
          </div>
        ))}
      </div>
    </ToastContext.Provider>
  );
}

export function useToasts(): ToastContextValue {
  const ctx = useContext(ToastContext);
  if (!ctx) throw new Error('useToasts must be used within ToastProvider');
  return ctx;
}

/**
 * Report any caught error as a toast WITH a recovery action:
 * a retry closure when provided, otherwise navigation to Diagnostics.
 */
export function useErrorHandler() {
  const { push } = useToasts();
  const { navigate } = useNav();
  return useCallback(
    (err: unknown, opts?: { retry?: () => void }) => {
      const be = toBridgeError(err);
      const message = be.message && be.message.trim().length > 0 ? be.message : be.code;
      const action: ToastAction = opts?.retry
        ? { label: t('action.retry'), onClick: opts.retry }
        : { label: t('action.openDiagnostics'), onClick: () => navigate('diagnostics') };
      push(message, action);
    },
    [push, navigate],
  );
}
