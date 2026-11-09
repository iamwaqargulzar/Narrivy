// Live session-state pill. The tone and label follow the daemon state string;
// unknown states are shown as reported, never guessed.
import { t } from '../../i18n';

const LABELS: Record<string, string> = {
  '': 'pill.unknown',
  idle: 'pill.idle',
  preparing: 'pill.starting',
  recording: 'pill.listening',
  listening: 'pill.listening',
  finalizing_audio: 'pill.processing',
  transcribing: 'pill.processing',
  transforming: 'pill.processing',
  delivering: 'pill.delivering',
  processing: 'pill.processing',
  committed: 'pill.processing',
  finalized: 'pill.processing',
  delivered: 'pill.delivered',
  inbox_recovered: 'pill.inboxRecovered',
  cancelled: 'pill.cancelled',
  failed: 'pill.failed',
  error: 'pill.error',
};

export function StatusPill({ phase, label }: { phase: string; label?: string }) {
  const kind =
    phase === 'recording' || phase === 'listening' || phase === 'preparing'
      ? 'pill-rec'
      : phase === 'error' || phase === 'failed'
        ? 'pill-err'
        : phase === 'delivered' || phase === 'inbox_recovered'
          ? 'pill-ok'
          : phase === 'idle' || phase === '' || phase === 'cancelled'
            ? ''
            : 'pill-busy';
  const shown = label ?? (phase in LABELS ? t(LABELS[phase]) : phase);
  return (
    <span className={`pill ${kind}`} data-phase={phase || 'unknown'}>
      <span className="pill-dot" aria-hidden="true" />
      {shown}
    </span>
  );
}
