// Shared daemon-advertised pipeline catalog and selection state.
//
// The provider owns the only pipeline-selection source of truth for the app
// (Home, Dictation, command palette, onboarding) and validates every start
// against the latest advertisement so no screen can silently start a lane the
// daemon did not offer.
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
import {
  bridge,
  toBridgeError,
  type BridgeError,
  type PipelineSummary,
  type StatusSnapshot,
} from '../lib/bridge';
import {
  buildPipelineCatalog,
  isPipelineAdvertised,
  preserveSelection,
  type PipelineCatalog,
} from '../lib/pipelines';
import { useDaemonStatus } from './StatusContext';

/** A refresh-driven change of the selected lane that the user did not make.
 * Start must never silently adopt the fallback lane: the UI surfaces this and
 * waits for an intentional new Start. */
export interface PipelineSelectionChange {
  /** Lane the user had selected before the daemon stopped advertising it. */
  from: string;
  /** Lane now selected instead, or null when no lane is startable. */
  to: string | null;
  /** Monotonic id so repeated changes stay distinguishable. */
  revision: number;
}

interface PipelinesContextValue {
  catalog: PipelineCatalog;
  error: BridgeError | null;
  loading: boolean;
  /** Select one advertised+enabled pipeline; unknown/disabled ids are ignored. */
  select: (id: string) => void;
  /** Re-read `request.list_pipelines`; keeps last-known data on failure. */
  refresh: () => Promise<void>;
  /** Latest selected id that is still advertised and enabled, or null. */
  resolveStartId: () => Promise<string | null>;
  /** Set when an advertisement refresh replaced the user's selection. */
  selectionChange: PipelineSelectionChange | null;
  acknowledgeSelectionChange: () => void;
}

const PipelinesContext = createContext<PipelinesContextValue | null>(null);

function sanitize(list: unknown): PipelineSummary[] {
  if (!Array.isArray(list)) return [];
  return list.filter(
    (p): p is PipelineSummary =>
      typeof p === 'object' &&
      p !== null &&
      typeof (p as PipelineSummary).id === 'string' &&
      (p as PipelineSummary).id.length > 0 &&
      typeof (p as PipelineSummary).enabled === 'boolean',
  );
}

/** Capability tokens whose change can flip an advertised lane's `enabled`
 * flag: model activation, resident worker warming->ready or restart, capture
 * helper readiness. Bounded by the daemon's own capability list. */
const READINESS_CAPABILITY_PREFIXES = ['model.', 'asr.', 'pipeline.', 'recovery.', 'capture.'];

/** Daemon readiness fields whose change can flip `enabled` on the advertised
 * lanes. Used as an explicit bounded refresh trigger instead of polling.
 * Revision/model state alone is not enough: a resident worker warming to ready
 * or restarting changes only the capability tokens. */
function readinessKey(status: StatusSnapshot | null): string {
  const relevantCapabilities = (status?.capabilities ?? [])
    .filter((cap) => READINESS_CAPABILITY_PREFIXES.some((prefix) => cap.startsWith(prefix)))
    .slice()
    .sort()
    .join(',');
  return [
    status?.installed_model ?? '',
    status?.model_provisioning?.state ?? '',
    status?.model_provisioning?.revision ?? '',
    relevantCapabilities,
  ].join('|');
}

/** True when status reports a lane's `pipeline.<id>=` token with a different
 * enabled state than the held listing. A lane status does not mention is not
 * a disagreement. */
export function listingDisagreesWithStatus(pipelines: PipelineSummary[], status: StatusSnapshot | null): boolean {
  const capabilities = status?.capabilities ?? [];
  return pipelines.some((pipeline) => {
    const prefix = `pipeline.${pipeline.id}=`;
    const token = capabilities.find((cap) => cap.startsWith(prefix));
    return token !== undefined && (token.slice(prefix.length) === 'enabled') !== pipeline.enabled;
  });
}

interface Advertisement {
  /** Request sequence of the read that produced this data. */
  seq: number;
  /** False when the newest listing read failed; Start rejects such data. */
  ok: boolean;
  pipelines: PipelineSummary[];
}

export function PipelinesProvider({ children }: { children: ReactNode }) {
  const { status } = useDaemonStatus();
  const [pipelines, setPipelines] = useState<PipelineSummary[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<BridgeError | null>(null);
  const [selectionChange, setSelectionChange] = useState<PipelineSelectionChange | null>(null);

  const pipelinesRef = useRef<PipelineSummary[]>([]);
  const selectedRef = useRef<string | null>(null);
  const requestSeq = useRef(0);
  const pending = useRef<Promise<void> | null>(null);
  const advertisement = useRef<Advertisement | null>(null);
  const lastReadinessKey = useRef<string | null>(null);
  const selectionRevision = useRef(0);

  const refresh = useCallback(async () => {
    const seq = ++requestSeq.current;
    setLoading(true);
    const request = bridge
      .listPipelines()
      .then((list) => {
        if (seq !== requestSeq.current) return;
        const next = sanitize(list);
        pipelinesRef.current = next;
        // Selection is preserved while the daemon still advertises the lane;
        // it only falls to another lane when the previous one is gone. That
        // fallback is recorded so Start rejects it and the UI can surface the
        // change instead of silently switching modes.
        const previous = selectedRef.current;
        if (isPipelineAdvertised(next, previous)) {
          selectedRef.current = previous;
        } else {
          const fallback = preserveSelection(previous, next);
          selectedRef.current = fallback;
          if (previous) {
            selectionRevision.current += 1;
            setSelectionChange({
              from: previous,
              to: fallback,
              revision: selectionRevision.current,
            });
          }
        }
        advertisement.current = { seq, ok: true, pipelines: next };
        setPipelines(next);
        setSelectedId(selectedRef.current);
        setError(null);
      })
      .catch((err: unknown) => {
        if (seq !== requestSeq.current) return;
        // Keep last-known pipelines/selection for context, but mark the newest
        // advertisement as failed so Start can never run on stale listing data.
        advertisement.current = { seq, ok: false, pipelines: pipelinesRef.current };
        setError(toBridgeError(err));
      })
      .finally(() => {
        if (seq === requestSeq.current) {
          setLoading(false);
          pending.current = null;
        }
      });
    pending.current = request;
    await request;
  }, []);

  // Initial load plus explicit readiness-triggered refresh: a completed model
  // provision or a resident worker warming->ready/restart changes the
  // advertised lanes (or the reportable readiness reason) without any
  // continuous polling.
  useEffect(() => {
    const key = readinessKey(status);
    if (lastReadinessKey.current === null) {
      lastReadinessKey.current = key;
      void refresh();
      return;
    }
    if (lastReadinessKey.current !== key) {
      lastReadinessKey.current = key;
      void refresh();
      return;
    }
    // Self-correction: the listing is read once per readiness change, and
    // that read can land a moment before the daemon finishes becoming ready
    // (or after it stopped being ready). When a fresh status disagrees with
    // the held listing about whether a lane is enabled, read it again. This
    // is bounded by the status cadence and stops as soon as they agree.
    if (!pending.current && listingDisagreesWithStatus(pipelinesRef.current, status)) {
      void refresh();
    }
  }, [status, refresh]);

  const select = useCallback((id: string) => {
    const advertised = pipelinesRef.current.find((p) => p.id === id);
    if (!advertised || !advertised.enabled) return;
    selectedRef.current = id;
    setSelectedId(id);
    // An explicit choice answers any earlier refresh-driven change notice.
    setSelectionChange(null);
  }, []);

  const acknowledgeSelectionChange = useCallback(() => setSelectionChange(null), []);

  const resolveStartId = useCallback(async (): Promise<string | null> => {
    // The id the user chose when Start was pressed. This in-flight Start is
    // frozen to that identity: a lane the user selects while it waits is
    // never substituted into it. That newer choice belongs to the next
    // deliberate Start.
    const requestedId = selectedRef.current;

    // Await the latest advertisement: a newer refresh can begin while this
    // Start waits, so loop until the settled read owns the newest sequence.
    for (;;) {
      const seq = requestSeq.current;
      const inFlight = pending.current;
      if (inFlight) await inFlight;
      if (requestSeq.current === seq) break;
    }

    const latest = advertisement.current;
    if (!latest || latest.seq !== requestSeq.current || !latest.ok) {
      // Unresolved, failed or superseded listing data is never startable.
      return null;
    }
    if (!requestedId) return null;
    // Return the originally captured lane when it is still advertised and
    // enabled; otherwise reject this old Start instead of adopting another
    // id (refresh fallback included). A refresh that replaced the selection
    // already surfaced its change for the user to answer with a new Start.
    const advertised = latest.pipelines.find((p) => p.id === requestedId);
    return advertised?.enabled ? requestedId : null;
  }, []);

  const catalog = useMemo(
    () =>
      buildPipelineCatalog({
        loading,
        failed: error !== null,
        pipelines,
        selectedId,
        status,
      }),
    [loading, error, pipelines, selectedId, status],
  );

  const value = useMemo<PipelinesContextValue>(
    () => ({
      catalog,
      error,
      loading,
      select,
      refresh,
      resolveStartId,
      selectionChange,
      acknowledgeSelectionChange,
    }),
    [
      catalog,
      error,
      loading,
      select,
      refresh,
      resolveStartId,
      selectionChange,
      acknowledgeSelectionChange,
    ],
  );

  return <PipelinesContext.Provider value={value}>{children}</PipelinesContext.Provider>;
}

export function usePipelines(): PipelinesContextValue {
  const ctx = useContext(PipelinesContext);
  if (!ctx) throw new Error('usePipelines must be used within PipelinesProvider');
  return ctx;
}
