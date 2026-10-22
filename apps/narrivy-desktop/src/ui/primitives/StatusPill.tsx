// Live session-state pill. The tone and label follow the daemon state string;
// unknown states are shown as reported, never guessed.
const LABELS: Record<string, string> = {
  '': 'Unknown',
  idle: 'Idle',
  preparing: 'Starting',
  recording: 'Listening',
  listening: 'Listening',
  finalizing_audio: 'Processing',
  transcribing: 'Processing',
  transforming: 'Processing',
  delivering: 'Delivering',
  processing: 'Processing',
  committed: 'Processing',
  finalized: 'Processing',
  delivered: 'Delivered',
  inbox_recovered: 'Saved to Inbox',
  cancelled: 'Cancelled',
  failed: 'Failed',
  error: 'Error',
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
  const shown = label ?? LABELS[phase] ?? phase;
  return (
    <span className={`pill ${kind}`} data-phase={phase || 'unknown'}>
      <span className="pill-dot" aria-hidden="true" />
      {shown}
    </span>
  );
}
