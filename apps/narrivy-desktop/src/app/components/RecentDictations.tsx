// The Recent list on Home: the latest retained dictations with Copy, Retry
// (only for a dictation whose text could not be inserted) and Delete.
//
// Rows come from optional History; nothing is shown when History is off.
// Delete needs a daemon at protocol minor 10 or newer. Retry re-inserting
// text is not served by the daemon yet, so it is shown disabled with the
// reason rather than pretending to work.
import { useCallback, useEffect, useState } from 'react';
import { Icon } from '../../ui/icons';
import { t } from '../../i18n';
import { bridge, type JournalEntryDto } from '../../lib/bridge';
import { useDaemonStatus } from '../../state/StatusContext';
import { useSession } from '../../state/SessionContext';
import { useErrorHandler } from '../../ui/primitives/Toast';
import { useNav } from '../../state/NavContext';

const RECENT_LIMIT = 5;
const DELETE_PROTOCOL_MINOR = 10;

interface RecentRow {
  entry: JournalEntryDto;
  text: string | null;
}

function timeLabel(ms: number | undefined): string {
  if (typeof ms !== 'number') return '';
  const when = new Date(ms);
  const sameDay = new Date().toDateString() === when.toDateString();
  return sameDay
    ? when.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' })
    : when.toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
}

export function RecentDictations() {
  const { status } = useDaemonStatus();
  const session = useSession();
  const report = useErrorHandler();
  const { navigate } = useNav();
  const [rows, setRows] = useState<RecentRow[] | null>(null);
  const [copied, setCopied] = useState<string | null>(null);
  const historyEnabled = status?.capabilities.includes('history.optional=enabled') ?? false;
  const canDelete = (status?.protocol_minor ?? 0) >= DELETE_PROTOCOL_MINOR;

  const load = useCallback(async () => {
    try {
      const entries = (await bridge.history()).slice(0, RECENT_LIMIT);
      const withText = await Promise.all(
        entries.map(async (entry) => {
          if (!entry.has_delivered_text && entry.state !== 'committed') return { entry, text: null };
          try {
            const full = await bridge.historyEntry(entry.txid);
            return { entry, text: full.delivered ?? full.corrected ?? full.text ?? null };
          } catch {
            return { entry, text: null };
          }
        }),
      );
      setRows(withText);
    } catch {
      setRows([]);
    }
  }, []);

  // Reload when History becomes available and after every finished dictation.
  useEffect(() => {
    if (historyEnabled) void load();
  }, [historyEnabled, load, session.finishedSessions]);

  useEffect(() => {
    if (!copied) return;
    const id = window.setTimeout(() => setCopied(null), 1500);
    return () => window.clearTimeout(id);
  }, [copied]);

  const copy = async (row: RecentRow) => {
    if (!row.text) return;
    try {
      await navigator.clipboard.writeText(row.text);
      setCopied(row.entry.txid);
    } catch (err) {
      report(err ?? new Error(t('home.copyFailed')), { retry: () => void copy(row) });
    }
  };

  const remove = async (row: RecentRow) => {
    try {
      await bridge.deleteHistoryEntry(row.entry.txid);
      setRows((current) => (current ?? []).filter((r) => r.entry.txid !== row.entry.txid));
    } catch (err) {
      report(err, { retry: () => void remove(row) });
    }
  };

  return (
    <div className="recent">
      <div className="recent-head">
        <span className="recent-title">{t('recent.title')}</span>
        <span className="card-head-spacer" />
        <button type="button" className="link-action link-quiet" onClick={() => navigate('history')}>
          {t('recent.viewAll')}
          <Icon name="arrowRight" />
        </button>
      </div>
      {!historyEnabled ? (
        <p className="recent-empty">{t('recent.historyOff')}</p>
      ) : rows === null ? (
        <p className="recent-empty">{t('common.loading')}</p>
      ) : rows.length === 0 ? (
        <p className="recent-empty">{t('recent.empty')}</p>
      ) : (
        <ul className="recent-list">
          {rows.map((row) => {
            const notInserted = !row.entry.has_delivered_text || row.entry.state === 'committed';
            return (
              <li key={row.entry.txid} className="recent-row" data-failed={notInserted ? 'true' : 'false'}>
                <span className="recent-time">{timeLabel(row.entry.updated_unix_ms)}</span>
                <span className="recent-text" title={row.text ?? undefined}>
                  {row.text ?? t('recent.noText')}
                </span>
                {notInserted ? <span className="chip chip-warn recent-flag">{t('recent.notInserted')}</span> : null}
                <span className="recent-actions">
                  <button
                    type="button"
                    className="icon-btn"
                    onClick={() => void copy(row)}
                    disabled={!row.text}
                    aria-label={t('recent.copy')}
                    title={t('recent.copy')}
                  >
                    <Icon name={copied === row.entry.txid ? 'check' : 'copy'} />
                  </button>
                  {notInserted ? (
                    <button
                      type="button"
                      className="icon-btn"
                      disabled
                      aria-label={t('recent.retry')}
                      title={t('recent.retryComing')}
                    >
                      <Icon name="refresh" />
                    </button>
                  ) : null}
                  <button
                    type="button"
                    className="icon-btn icon-btn-danger"
                    onClick={() => void remove(row)}
                    disabled={!canDelete}
                    aria-label={t('recent.delete')}
                    title={canDelete ? t('recent.delete') : t('recent.deleteUnsupported')}
                  >
                    <Icon name="trash" />
                  </button>
                </span>
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}
