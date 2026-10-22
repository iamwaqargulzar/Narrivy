// Subscribes to the `session-event` Tauri event stream and exposes the live
// phase plus the last transcript/delivered text (fetched lazily for the exact
// short-lived recovery transaction when the daemon reports completion).
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
import { t } from '../i18n';
import { bridge } from '../lib/bridge';
import { isCaptureEndReason } from '../lib/capturePolicy';
import { useErrorHandler } from '../ui/primitives/Toast';
import { useDaemonStatus } from './StatusContext';

const SUCCESSFUL_RECOVERY_STATES = new Set(['delivered', 'inbox_recovered']);

function isVerifiedRecoveryResult(value: unknown, expectedTxid: string): value is {
  txid: string;
  state: string;
  text?: string | null;
  raw?: string | null;
  delivered?: string | null;
} {
  if (value === null || typeof value !== 'object') return false;
  const result = value as Record<string, unknown>;
  return (
    result.txid === expectedTxid &&
    typeof result.state === 'string' &&
    SUCCESSFUL_RECOVERY_STATES.has(result.state)
  );
}

/**
 * A recovery response bound to the exact requested transaction. This is the
 * weaker binding used ONLY for the content-free typed capture end reason; any
 * transcript-bearing field still requires {@link isVerifiedRecoveryResult}.
 */
function isBoundToTransaction(
  value: unknown,
  expectedTxid: string,
): value is { txid: string; state: string; capture_end?: unknown } {
  if (value === null || typeof value !== 'object') return false;
  const result = value as Record<string, unknown>;
  return result.txid === expectedTxid && typeof result.state === 'string';
}

interface SessionContextValue {
  phase: string;
  /** Volatile, uncorrected model hypothesis. Never persisted by the UI. */
  tentativeTranscript: string | null;
  lastTranscript: string | null;
  lastDelivered: string | null;
  setLastDelivered: (text: string) => void;
  /** P7 UX-009: raw variant of the last delivered entry, when journaled. */
  lastRaw: string | null;
  /**
   * Typed, content-free reason the daemon recorded for how the last session's
   * capture ended (`safety_limit`, `source_ended`, ...). Null when the daemon
   * did not report one; the UI must render "not reported", never a guess.
   */
  captureEnd: string | null;
  /** True while the displayed output still shows the Clean delivery. */
  undoAvailable: boolean;
  /** One-step undo: revert the displayed output to the Raw variant. */
  undoToRaw: () => void;
  /**
   * Counts sessions that ended, once their outcome is durable. Lists of past
   * dictations reload when it changes, even when two results read the same.
   */
  finishedSessions: number;
}

const SessionContext = createContext<SessionContextValue | null>(null);

export function SessionProvider({ children }: { children: ReactNode }) {
  const { status, reload } = useDaemonStatus();
  const report = useErrorHandler();
  const [phase, setPhase] = useState<string>(() => status?.active_session?.state ?? '');
  const [lastTranscript, setLastTranscript] = useState<string | null>(null);
  const [tentativeTranscript, setTentativeTranscript] = useState<string | null>(null);
  const [lastDelivered, setLastDeliveredState] = useState<string | null>(null);
  // The raw variant rides the exact recovery-result fetch used for the clean
  // result. Optional user History remains independent and may be disabled.
  const [lastRaw, setLastRaw] = useState<string | null>(null);
  const [captureEnd, setCaptureEnd] = useState<string | null>(null);
  const [undoAvailable, setUndoAvailable] = useState(false);
  const [finishedSessions, setFinishedSessions] = useState(0);
  const activeIdentity = useRef<{
    sessionId: string;
    transactionId: string;
    pipelineId: string;
  } | null>(null);
  const lastStateSeq = useRef<number | null>(null);
  const lastPartialSeq = useRef<number | null>(null);
  const lastEnvelopeSeq = useRef<number | null>(null);
  const latestRecoveryRequest = useRef<string | null>(null);

  // Sync initial phase once the first status snapshot arrives.
  useEffect(() => {
    if (status?.active_session?.state) setPhase(status.active_session.state);
    const active = status?.active_session;
    if (active?.session_id && active.transaction_id && active.pipeline_id) {
      const incoming = {
        sessionId: active.session_id,
        transactionId: active.transaction_id,
        pipelineId: active.pipeline_id,
      };
      if (
        activeIdentity.current?.sessionId !== incoming.sessionId ||
        activeIdentity.current?.transactionId !== incoming.transactionId ||
        activeIdentity.current?.pipelineId !== incoming.pipelineId
      ) {
        activeIdentity.current = incoming;
        lastStateSeq.current = null;
        lastPartialSeq.current = null;
        latestRecoveryRequest.current = null;
        setTentativeTranscript(null);
      }
    }
  }, [status]);

  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let cancelled = false;

    const fetchRecoveryOutcome = async (txid: string, expectsDeliveredText: boolean) => {
      latestRecoveryRequest.current = txid;
      try {
        const full = await bridge.recoveryEntry(txid);
        if (cancelled || latestRecoveryRequest.current !== txid) return;
        // The typed capture end reason is content-free and bound to the exact
        // transaction, so a failed session may still report how recording
        // ended even when no transcript reached the user.
        if (isBoundToTransaction(full, txid)) {
          setCaptureEnd(isCaptureEndReason(full.capture_end) ? full.capture_end : null);
        }
        // IPC success is not enough to trust transcript-bearing data. Bind the
        // response to the requested transaction and a successful terminal
        // recovery state before any variant reaches React state. A failed
        // session is expected not to be successful, so only a delivery-expecting
        // fetch reports an unverifiable result.
        if (!isVerifiedRecoveryResult(full, txid)) {
          if (expectsDeliveredText) {
            report({
              code: 'recovery_result_unverified',
              message: t('dictation.recoveryResultUnverified'),
            });
          }
          return;
        }
        // The detail row carries typed variants; `text` stays a fallback for
        // older bridges. Raw is stored separately for one-step undo.
        const deliveredText =
          typeof full.delivered === 'string' && full.delivered.length > 0
            ? full.delivered
            : typeof full.text === 'string'
              ? full.text
              : null;
        if (deliveredText) {
          setLastTranscript(deliveredText);
          setLastDeliveredState(deliveredText);
          setLastRaw(typeof full.raw === 'string' && full.raw.length > 0 ? full.raw : null);
          setUndoAvailable(true);
        }
      } catch (err) {
        if (!cancelled && latestRecoveryRequest.current === txid) {
          report(err); // toast with Open Diagnostics action
        }
      } finally {
        // The outcome was read back, so History already holds this entry.
        if (!cancelled) setFinishedSessions((count) => count + 1);
      }
    };

    bridge
      .onSessionEvent((ev) => {
        if (cancelled) return;

        if (ev.event) {
          if (
            typeof ev.event_seq !== 'number' ||
            !Number.isSafeInteger(ev.event_seq) ||
            ev.event_seq <= 0 ||
            (lastEnvelopeSeq.current !== null && ev.event_seq <= lastEnvelopeSeq.current)
          ) {
            return;
          }
          lastEnvelopeSeq.current = ev.event_seq;
        }

        if (ev.event === 'events_resubscribed') {
          // The event stream reconnected (the daemon restarted, for
          // example), so events may have been missed. Drop the session
          // shown and re-read the status, which restores a session that is
          // still running.
          activeIdentity.current = null;
          lastStateSeq.current = null;
          lastPartialSeq.current = null;
          setTentativeTranscript(null);
          setPhase('idle');
          void reload();
          return;
        }

        if (ev.event === 'transcript_partial') {
          const identity = activeIdentity.current;
          if (
            !identity ||
            ev.session_id !== identity.sessionId ||
            ev.transaction_id !== identity.transactionId ||
            ev.pipeline_id !== identity.pipelineId ||
            typeof ev.seq !== 'number' ||
            !Number.isSafeInteger(ev.seq) ||
            ev.seq < 0 ||
            typeof ev.text !== 'string' ||
            ev.text.length === 0 ||
            new TextEncoder().encode(ev.text).byteLength > 16_384 ||
            (lastPartialSeq.current !== null && ev.seq <= lastPartialSeq.current)
          ) {
            return;
          }
          lastPartialSeq.current = ev.seq;
          setTentativeTranscript(ev.text);
          return;
        }

        // Missing event tags remain readable for protocol-minor compatibility,
        // but current state events carry exact identity and sequence fields.
        if (!ev.state || (ev.event && ev.event !== 'session_state_changed')) return;
        if (typeof ev.seq === 'number') {
          if (!Number.isSafeInteger(ev.seq) || ev.seq < 0) return;
        }
        if (ev.session_id && ev.transaction_id && ev.pipeline_id) {
          const incoming = {
            sessionId: ev.session_id,
            transactionId: ev.transaction_id,
            pipelineId: ev.pipeline_id,
          };
          const active = activeIdentity.current;
          const beginsSession = ev.state === 'preparing';
          if (
            active &&
            (active.sessionId !== incoming.sessionId ||
              active.transactionId !== incoming.transactionId ||
              active.pipelineId !== incoming.pipelineId) &&
            !beginsSession
          ) {
            return;
          }
          if (
            !active ||
            active.sessionId !== incoming.sessionId ||
            active.transactionId !== incoming.transactionId ||
            active.pipelineId !== incoming.pipelineId
          ) {
            activeIdentity.current = incoming;
            lastStateSeq.current = null;
            lastPartialSeq.current = null;
            latestRecoveryRequest.current = null;
            setTentativeTranscript(null);
            // A new transaction cannot inherit how the previous capture ended.
            setCaptureEnd(null);
          }
        } else if (activeIdentity.current && ev.session_id !== activeIdentity.current.sessionId) {
          return;
        }
        if (
          typeof ev.seq === 'number' &&
          lastStateSeq.current !== null &&
          ev.seq <= lastStateSeq.current
        ) {
          return;
        }
        if (typeof ev.seq === 'number') lastStateSeq.current = ev.seq;
        setPhase(ev.state);

        if (['committed', 'cancelled', 'failed', 'idle'].includes(ev.state)) {
          setTentativeTranscript(null);
          lastPartialSeq.current = null;
        }
        if (
          (ev.state === 'committed' ||
            ev.state === 'delivered' ||
            ev.state === 'finalized') &&
          typeof ev.transaction_id === 'string' &&
          ev.transaction_id.length > 0
        ) {
          void fetchRecoveryOutcome(ev.transaction_id, true);
        }
        if (
          ev.state === 'failed' &&
          typeof ev.transaction_id === 'string' &&
          ev.transaction_id.length > 0
        ) {
          // A failed session still has durable transaction truth: fetch it so
          // the typed capture end reason renders even when no text was
          // committed. The text path stays bound to successful states.
          void fetchRecoveryOutcome(ev.transaction_id, false);
        }
        if (['cancelled', 'failed', 'idle'].includes(ev.state)) {
          activeIdentity.current = null;
          lastStateSeq.current = null;
        }
        if (ev.state === 'cancelled') {
          // Cancel discards the transaction; drop any outstanding fetch. The
          // failed path keeps its request bound so its end reason can render.
          latestRecoveryRequest.current = null;
        }
        if (['cancelled', 'failed', 'idle'].includes(ev.state)) {
          // Explicit bounded refresh trigger: session termination can change
          // advertised lane readiness and the reported active session. This is
          // event-driven, never continuous polling.
          void reload();
        }
      })
      .then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      })
      .catch(() => {
        // event subscription is best-effort; degraded state still renders
      });

    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, [report, reload]);

  const setLastDelivered = useCallback((text: string) => {
    setLastDeliveredState(text);
    setLastTranscript(text);
    setUndoAvailable(false);
  }, []);

  // UX-009: destructive AI transformation gets a ONE-STEP undo. Reverting
  // shows the journaled raw variant; a repeated click is a no-op.
  const undoToRaw = useCallback(() => {
    if (!undoAvailable || lastRaw === null) return;
    setLastTranscript(lastRaw);
    setUndoAvailable(false);
  }, [undoAvailable, lastRaw]);

  const value = useMemo<SessionContextValue>(
    () => ({
      phase,
      tentativeTranscript,
      lastTranscript,
      lastDelivered,
      setLastDelivered,
      lastRaw,
      captureEnd,
      undoAvailable,
      undoToRaw,
      finishedSessions,
    }),
    [
      phase,
      tentativeTranscript,
      lastTranscript,
      lastDelivered,
      setLastDelivered,
      lastRaw,
      captureEnd,
      undoAvailable,
      undoToRaw,
      finishedSessions,
    ],
  );

  return <SessionContext.Provider value={value}>{children}</SessionContext.Provider>;
}

export function useSession(): SessionContextValue {
  const ctx = useContext(SessionContext);
  if (!ctx) throw new Error('useSession must be used within SessionProvider');
  return ctx;
}
