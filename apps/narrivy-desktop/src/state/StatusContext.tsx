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
import { bridge, toBridgeError, type BridgeError, type StatusSnapshot } from '../lib/bridge';
import { t } from '../i18n';
import { useErrorHandler } from '../ui/primitives/Toast';

interface StatusContextValue {
  status: StatusSnapshot | null;
  error: BridgeError | null;
  loading: boolean;
  reload: () => Promise<void>;
}

const StatusContext = createContext<StatusContextValue | null>(null);

const PROVISION_ACTIVE_STATES = new Set(['importing', 'cancel_requested', 'activating']);
/** How often the visible window re-reads daemon status in the background. */
export const LIVE_REFRESH_MS = 3000;
const PROVISION_POLL_INTERVAL_MS = 750;
/** One bounded deadline per daemon-owned provision operation (~90 s worst
 * case), shared across its active-state transitions. */
const PROVISION_POLL_DEADLINE_MS = 90_000;
/** Bounded follow-up reads after a provision reports ready, to observe the
 * resident worker becoming startable; never continuous polling. */
const TERMINAL_FOLLOW_UP_MS = [400, 1200, 2500];

export function StatusProvider({ children }: { children: ReactNode }) {
  const [status, setStatus] = useState<StatusSnapshot | null>(null);
  const [error, setError] = useState<BridgeError | null>(null);
  const [loading, setLoading] = useState(true);
  const [pollEpoch, setPollEpoch] = useState(0);
  const report = useErrorHandler();

  // Request ownership: every read gets a generation and only the newest one
  // may publish. A slow awaiting-model answer can therefore never overwrite a
  // newer ready snapshot, and nothing publishes after unmount.
  const generation = useRef(0);
  const mounted = useRef(true);
  // Whether any read has succeeded: a quiet read may hide a failure only
  // when there is a last known status to keep.
  const connected = useRef(false);
  // Consecutive failed quiet reads; one is a blip, two mean the daemon is gone.
  const quietFailures = useRef(0);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      // Invalidate every read owned by this lifetime.
      generation.current += 1;
    };
  }, []);

  const fetchStatus = useCallback(
    async (quiet: boolean) => {
      const gen = ++generation.current;
      if (!quiet) setLoading(true);
      try {
        const snap = await bridge.status();
        if (!mounted.current || gen !== generation.current) return;
        connected.current = true;
        quietFailures.current = 0;
        setStatus(snap);
        setError(null);
      } catch (err) {
        if (!mounted.current || gen !== generation.current) return;
        if (quiet && connected.current) {
          // One failed background read keeps the last known daemon truth
          // instead of flapping the UI. A second in a row means the daemon
          // is gone: stop showing its old status as current (F-M11: Home
          // kept saying Ready after the daemon stopped).
          quietFailures.current += 1;
          if (quietFailures.current < 2) return;
          connected.current = false;
          setStatus(null);
        }
        // Never connected: a quiet read that supersedes the first one must
        // still surface its failure, or the app says "Connecting" forever
        // (seen natively 2026-10-20). Only an explicit read raises a toast.
        setError(toBridgeError(err));
        if (!quiet) report(err, { retry: () => void fetchStatus(false) });
      } finally {
        // The newest read has settled, so the loading state is over even when
        // a quiet freshness read superseded an explicit reload.
        if (mounted.current && gen === generation.current) setLoading(false);
      }
    },
    [report],
  );

  const reload = useCallback(async () => {
    await fetchStatus(false);
  }, [fetchStatus]);

  useEffect(() => {
    void fetchStatus(false);
  }, [fetchStatus]);

  // Explicit bounded freshness for the daemon-owned offline model provision:
  // while an operation is actually running, re-read status on a bounded
  // interval so a completed provision replaces the initial awaiting snapshot.
  const provisionId = status?.model_provisioning?.operation_id ?? null;
  const provisionState = status?.model_provisioning?.state ?? null;
  // Live freshness for the always-visible status (bottom bar, Home): a quiet
  // re-read every few seconds while the window is visible. Quiet reads keep
  // the last known values on failure instead of flapping or raising toasts.
  // It pauses while a model provision is active: that operation has its own
  // faster, deadline-bounded poll, and once that deadline is spent reads stop
  // until the surfaced retry. Owned by this provider; unmounting stops it.
  const provisionActive = provisionState !== null && PROVISION_ACTIVE_STATES.has(provisionState);
  // Single flight: a tick while the previous read is still out is skipped,
  // so reads never pile up behind a slow daemon (F-M10).
  const livePolling = useRef(false);
  useEffect(() => {
    if (provisionActive) return;
    const id = window.setInterval(() => {
      if (document.visibilityState !== 'visible' || livePolling.current) return;
      livePolling.current = true;
      void fetchStatus(true).finally(() => {
        livePolling.current = false;
      });
    }, LIVE_REFRESH_MS);
    return () => window.clearInterval(id);
  }, [fetchStatus, provisionActive]);

  const provisionBudget = useRef<{
    operationId: string;
    /** Absolute monotonic instant (performance.now()) at which this
     * operation's freshness window is over. Anchored once per operation and
     * retained across active-state transitions, so a throttled or background
     * scheduler that wakes late sees the elapsed window rather than a tick
     * count that assumed every callback ran on time. */
    deadlineMs: number;
    exhausted: boolean;
  } | null>(null);
  useEffect(() => {
    if (!provisionId || !provisionState || !PROVISION_ACTIVE_STATES.has(provisionState)) return;
    // One deadline per operation, also while its active state transitions
    // (importing -> activating -> ready). A different operation starts fresh.
    if (provisionBudget.current?.operationId !== provisionId) {
      provisionBudget.current = {
        operationId: provisionId,
        deadlineMs: performance.now() + PROVISION_POLL_DEADLINE_MS,
        exhausted: false,
      };
    }
    // An exhausted operation stays stopped until the surfaced retry re-arms
    // it; a later state transition must not restart polling or re-report.
    if (provisionBudget.current.exhausted) return;
    let cancelled = false;
    let timer: ReturnType<typeof setInterval> | undefined;
    const tick = () => {
      if (cancelled) return;
      const budget = provisionBudget.current;
      if (!budget || budget.operationId !== provisionId || budget.exhausted) return;
      // Compare against the monotonic clock at the moment this callback
      // actually runs. A throttled/background scheduler may wake long after
      // the window passed; that late wake must exhaust the operation instead
      // of issuing one more read just because this is its first callback.
      if (performance.now() < budget.deadlineMs) {
        void fetchStatus(true);
        return;
      }
      // Exhausted: stop polling and surface one explicit retry instead of
      // silently freezing on a stale provisioning snapshot.
      budget.exhausted = true;
      if (timer) clearInterval(timer);
      report(
        { code: 'provision_status_stale', message: t('status.provisionStalled') },
        {
          retry: () => {
            provisionBudget.current = {
              operationId: provisionId,
              // An explicit retry re-arms a fresh full window for the
              // operation; a new operation always gets its own deadline.
              deadlineMs: performance.now() + PROVISION_POLL_DEADLINE_MS,
              exhausted: false,
            };
            setPollEpoch((n) => n + 1);
            void fetchStatus(false);
          },
        },
      );
    };
    timer = setInterval(tick, PROVISION_POLL_INTERVAL_MS);
    return () => {
      cancelled = true;
      if (timer) clearInterval(timer);
    };
  }, [provisionId, provisionState, fetchStatus, pollEpoch, report]);

  // A provision that reports ready is followed by resident-worker readiness,
  // which can flip the advertised pipeline to enabled. Three bounded reads
  // cover that transition without polling forever.
  const followedUpOperation = useRef<string | null>(null);
  useEffect(() => {
    if (!provisionId || provisionState !== 'ready') return;
    if (followedUpOperation.current === provisionId) return;
    followedUpOperation.current = provisionId;
    const timers = TERMINAL_FOLLOW_UP_MS.map((delay) =>
      setTimeout(() => void fetchStatus(true), delay),
    );
    return () => timers.forEach(clearTimeout);
  }, [provisionId, provisionState, fetchStatus]);

  const value = useMemo<StatusContextValue>(
    () => ({ status, error, loading, reload }),
    [status, error, loading, reload],
  );

  return <StatusContext.Provider value={value}>{children}</StatusContext.Provider>;
}

export function useDaemonStatus(): StatusContextValue {
  const ctx = useContext(StatusContext);
  if (!ctx) throw new Error('useDaemonStatus must be used within StatusProvider');
  return ctx;
}
