// Shared dictation commands (Start/Stop/Cancel) used by Home, Dictation, the
// command palette and onboarding. Start always resolves the exact daemon
// advertised pipeline id selected in PipelinesProvider and fails closed when
// that id is no longer enabled: it never substitutes a default or mock lane.
//
// Stop and Cancel are separate authenticated IPC commands and are never gated
// behind an in-flight Start. A Cancel (or Stop) issued while the advertisement
// is still resolving latches a pending cancellation intent, so the late
// StartSession is never issued after the user cancelled.
import { useCallback, useRef, useState } from 'react';
import { t } from '../i18n';
import { bridge } from '../lib/bridge';
import { buildRecognitionSettings } from '../lib/recognition';
import { loadSettings } from '../lib/settings';
import { markStartClick } from '../lib/performance';
import { useErrorHandler } from '../ui/primitives/Toast';
import { usePipelines } from '../state/PipelinesContext';
import { useSession } from '../state/SessionContext';
import { useDaemonStatus } from '../state/StatusContext';

export const BUSY_SESSION_STATES: ReadonlySet<string> = new Set([
  'preparing',
  'recording',
  'listening',
  'finalizing_audio',
  'transcribing',
  'transforming',
  'delivering',
]);

const STOPPABLE_SESSION_STATES: ReadonlySet<string> = new Set(['recording', 'listening']);

export interface DictationActions {
  start: () => Promise<boolean>;
  stop: () => Promise<boolean>;
  cancel: () => Promise<boolean>;
  /** A command request is in flight. */
  working: boolean;
  /** Session is active or a command is in flight. */
  busy: boolean;
  active: boolean;
  canStart: boolean;
  canStop: boolean;
  canCancel: boolean;
}

export function useDictationActions(): DictationActions {
  const { catalog, refresh, resolveStartId } = usePipelines();
  const session = useSession();
  const report = useErrorHandler();
  const { status } = useDaemonStatus();
  const statusRef = useRef(status);
  statusRef.current = status;
  const [commandWorking, setCommandWorking] = useState(false);
  // Latched by Stop/Cancel while a Start is resolving, waiting for the
  // advertisement or awaiting its own Start acknowledgement. That Start
  // consumes it and never leaves a live session the user already stopped.
  // Shared app-wide through the session context (F-M11).
  const latch = session.dictationLatch;
  const starting = session.startPending;
  const setStarting = session.setStartPending;

  const active = BUSY_SESSION_STATES.has(session.phase || '');
  const working = starting || commandWorking;
  const busy = active || working;

  const start = useCallback(
    async function start(): Promise<boolean> {
      if (latch.current.startInFlight) return false;
      latch.current.startInFlight = true;
      // A fresh explicit Start is the user's intent; any earlier cancellation
      // intent has been answered by it.
      latch.current.cancelIntent = null;
      setStarting(true);
      try {
        const pipelineId = await resolveStartId();
        if (latch.current.cancelIntent) {
          // Stop/Cancel arrived while the advertisement was resolving: never
          // issue the late StartSession.
          return false;
        }
        if (!pipelineId) {
          // The advertised selection disappeared/disabled, or the newest
          // listing failed. Tell the user and re-read the advertisement
          // instead of starting any other lane (the retry action refreshes
          // first, then re-resolves).
          void refresh();
          report(
            catalog.availability === 'error'
              ? { code: 'pipelines_unavailable', message: t('pipelines.error') }
              : { code: 'pipeline_unavailable', message: t('dictation.noStartablePipeline') },
            {
              retry: () => {
                void refresh();
                void start();
              },
            },
          );
          return false;
        }
        const recognition = buildRecognitionSettings(statusRef.current, loadSettings());
        markStartClick();
        if (recognition) {
          await bridge.start(pipelineId, recognition);
        } else {
          await bridge.start(pipelineId);
        }
        if (latch.current.cancelIntent) {
          // Separate Start/Cancel IPC connections can be admitted out of
          // order: this Start acknowledgement proves admission, but it may
          // have landed after the cancellation acknowledgement. Reconcile on
          // the separate authenticated command path before reporting a start
          // the user already cancelled.
          const intent = latch.current.cancelIntent;
          latch.current.cancelIntent = null;
          try {
            if (intent === 'stop') await bridge.stop();
            else await bridge.cancel();
          } catch (err) {
            report(err, { retry: () => void cancel() });
          }
          return false;
        }
        return true;
      } catch (err) {
        report(err, { retry: () => void start() });
        // The daemon may have changed which lanes it accepts; refresh the
        // advertisement so the next attempt uses current truth.
        void refresh();
        return false;
      } finally {
        latch.current.startInFlight = false;
        latch.current.cancelIntent = null;
        setStarting(false);
      }
    },
    [catalog.availability, report, refresh, resolveStartId, latch, setStarting],
  );

  const stop = useCallback(
    async function stop(): Promise<boolean> {
      // A pending Start must not start recording after the user stopped it.
      latch.current.cancelIntent = 'stop';
      setCommandWorking(true);
      try {
        // Stop/cancel never consult the pipeline listing: an active session
        // stays controllable through transient listing failures.
        await bridge.stop();
        return true;
      } catch (err) {
        report(err, { retry: () => void stop() });
        return false;
      } finally {
        setCommandWorking(false);
      }
    },
    [report, latch],
  );

  const cancel = useCallback(
    async function cancel(): Promise<boolean> {
      // Latch the intent before the IPC round-trip: even when the daemon
      // rejects the request, a Start that is still resolving must not fire
      // after the user cancelled.
      latch.current.cancelIntent = 'cancel';
      setCommandWorking(true);
      try {
        await bridge.cancel();
        return true;
      } catch (err) {
        report(err, { retry: () => void cancel() });
        // Intent stays latched so the pending Start still aborts safely.
        return false;
      } finally {
        setCommandWorking(false);
      }
    },
    [report, latch],
  );

  return {
    start,
    stop,
    cancel,
    working,
    busy,
    active,
    canStart: catalog.availability === 'ready' && !busy,
    canStop: STOPPABLE_SESSION_STATES.has(session.phase || ''),
    canCancel: busy,
  };
}
