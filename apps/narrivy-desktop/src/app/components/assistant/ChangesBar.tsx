// The files the assistant changed in this chat (the coding harness, D55),
// with Undo all and Keep. The service copied each file before its first
// change; undo puts them back and removes the files it made. What commands
// changed is not tracked, and the bar says so.
import { useCallback, useEffect, useState } from 'react';
import { bridge } from '../../../lib/bridge';
import { failureCode } from '../../../lib/sourceErrors';
import { t } from '../../../i18n';
import { Icon } from '../../../ui/icons';

interface Changed {
  path: string;
  new: boolean;
  undoable: boolean;
}

export function ChangesBar({ chatId, busy }: { chatId: string | null; busy: boolean }) {
  const [files, setFiles] = useState<Changed[]>([]);
  const [open, setOpen] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [result, setResult] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    if (!chatId) {
      setFiles([]);
      return;
    }
    try {
      const value = JSON.parse(await bridge.assistantLibrary('list', 'changes', chatId)) as { files?: Changed[] };
      setFiles(Array.isArray(value.files) ? value.files : []);
    } catch {
      setFiles([]);
    }
  }, [chatId]);

  // Looked at when the chat opens and after each reply.
  useEffect(() => {
    setResult(null);
    setConfirming(false);
    if (!busy) void load();
  }, [load, busy]);

  if (!files.length) {
    return result ? (
      <p className="changes-result" role="status">
        {result}
      </p>
    ) : null;
  }
  const act = (op: 'undo' | 'keep') => {
    if (!chatId) return;
    setError(null);
    bridge
      .assistantLibrary(op, 'changes', chatId)
      .then((raw) => {
        if (op === 'undo') {
          const done = JSON.parse(raw) as { restored?: number; removed?: number; failed?: number };
          setResult(
            done.failed
              ? t('assistant.changes.undonePartly', { n: (done.restored ?? 0) + (done.removed ?? 0), failed: done.failed })
              : t('assistant.changes.undone', { n: (done.restored ?? 0) + (done.removed ?? 0) }),
          );
        } else {
          setResult(null);
        }
        setConfirming(false);
        return load();
      })
      .catch((e: unknown) => setError(failureCode(e)));
  };
  return (
    <section className="changes-bar" aria-label={t('assistant.changes.title')}>
      <div className="changes-head">
        <button type="button" className="btn btn-ghost changes-count" aria-expanded={open} onClick={() => setOpen(!open)}>
          <Icon name="pencil" />
          {t(files.length === 1 ? 'assistant.changes.countOne' : 'assistant.changes.count', { n: files.length })}
        </button>
        {confirming ? (
          <span className="changes-confirm" role="group" aria-label={t('assistant.changes.confirmTitle')}>
            <span>{t('assistant.changes.confirm', { n: files.length })}</span>
            <button type="button" className="btn btn-primary" disabled={busy} onClick={() => act('undo')}>
              {t('assistant.changes.undoNow')}
            </button>
            <button type="button" className="btn btn-default" onClick={() => setConfirming(false)}>
              {t('assistant.changes.cancel')}
            </button>
          </span>
        ) : (
          <span className="changes-actions">
            <button type="button" className="btn btn-default" disabled={busy} onClick={() => setConfirming(true)}>
              <Icon name="undo" />
              {t('assistant.changes.undo')}
            </button>
            <button type="button" className="btn btn-ghost" disabled={busy} title={t('assistant.changes.keepHint')} onClick={() => act('keep')}>
              {t('assistant.changes.keep')}
            </button>
          </span>
        )}
      </div>
      {open ? (
        <ul className="changes-files">
          {files.map((file) => (
            <li key={file.path} title={file.path}>
              <span className="chip">{file.new ? t('assistant.changes.new') : t('assistant.changes.changed')}</span>
              {file.path}
              {!file.undoable ? <span className="source"> · {t('assistant.changes.tooLarge')}</span> : null}
            </li>
          ))}
          <li className="source">{t('assistant.changes.commandsNote')}</li>
        </ul>
      ) : null}
      {error ? (
        <p className="assistant-error" role="alert">
          {t('assistant.changes.failed', { code: error })}
        </p>
      ) : null}
    </section>
  );
}
