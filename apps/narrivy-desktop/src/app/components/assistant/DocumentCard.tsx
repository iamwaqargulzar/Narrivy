// A document the assistant made in the chat (D55): Word, PDF, slides or
// Markdown, written by Narrivy itself. The card previews its content and
// downloads (a copy in the Downloads folder) or opens it. The file is kept
// with the chat by the service.
import { useState, type ReactNode } from 'react';
import { bridge } from '../../../lib/bridge';
import { failureCode } from '../../../lib/sourceErrors';
import { hasMessage, t } from '../../../i18n';
import { Icon } from '../../../ui/icons';

export interface DocumentInfo {
  id: string;
  chat: string;
  title: string;
  format: 'docx' | 'pdf' | 'pptx' | 'md' | string;
  name: string;
  size: number;
  /** Pages (PDF) or slides (pptx) the service counted; 0 when it reflows. */
  units: number;
}

/** A tool use's detail, when it is a document that was made. */
export function documentOf(detail?: string): DocumentInfo | null {
  try {
    const value = JSON.parse(detail ?? '{}') as Partial<DocumentInfo> & { kind?: string };
    if (value.kind !== 'document' || typeof value.id !== 'string' || typeof value.chat !== 'string') return null;
    return {
      id: value.id,
      chat: value.chat,
      title: typeof value.title === 'string' ? value.title : '',
      format: typeof value.format === 'string' ? value.format : '',
      name: typeof value.name === 'string' ? value.name : value.id,
      size: typeof value.size === 'number' ? value.size : 0,
      units: typeof value.units === 'number' ? value.units : 0,
    };
  } catch {
    return null;
  }
}

function size(bytes: number): string {
  return bytes >= 1024 * 1024 ? `${(bytes / 1024 / 1024).toFixed(1)} MB` : `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

function errorText(code: string): string {
  return hasMessage(`assistant.document.error.${code}`)
    ? t(`assistant.document.error.${code}`)
    : t('assistant.document.error.other', { code });
}

export function DocumentCard({ doc, renderPreview }: { doc: DocumentInfo; renderPreview: (markdown: string) => ReactNode }) {
  const key = `${doc.chat}/${doc.id}`;
  const [preview, setPreview] = useState<string | null>(null);
  const [open, setOpen] = useState(false);
  const [saved, setSaved] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const kind = hasMessage(`assistant.document.kind.${doc.format}`)
    ? t(`assistant.document.kind.${doc.format}`)
    : doc.format.toUpperCase();
  const run = (work: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    work()
      .catch((e: unknown) => setError(failureCode(e)))
      .finally(() => setBusy(false));
  };
  const togglePreview = () => {
    if (open) {
      setOpen(false);
      return;
    }
    if (preview !== null) {
      setOpen(true);
      return;
    }
    run(async () => {
      const value = JSON.parse(await bridge.assistantLibrary('get', 'document', key)) as { markdown?: string };
      setPreview(value.markdown ?? '');
      setOpen(true);
    });
  };
  return (
    <div className="document-card" data-format={doc.format}>
      <div className="document-card-head">
        <span className="document-icon" aria-hidden="true">
          <Icon name="doc" />
          <span className="document-ext">{doc.format.toUpperCase()}</span>
        </span>
        <span className="document-name">
          <strong title={doc.name}>{doc.title || doc.name}</strong>
          <span className="source">
            {kind}
            {doc.units ? ` · ${t(doc.format === 'pptx' ? 'assistant.document.slides' : 'assistant.document.pages', { n: doc.units })}` : ''} ·{' '}
            {size(doc.size)}
          </span>
        </span>
        <span className="document-actions">
          <button type="button" className="btn btn-default" aria-expanded={open} disabled={busy} onClick={togglePreview}>
            {open ? t('assistant.document.hide') : t('assistant.document.preview')}
          </button>
          <button
            type="button"
            className="btn btn-default"
            disabled={busy}
            onClick={() =>
              run(async () => {
                const value = JSON.parse(await bridge.assistantLibrary('download', 'document', key)) as { path?: string };
                setSaved(value.path ?? null);
              })
            }
          >
            <Icon name="download" />
            {t('assistant.document.download')}
          </button>
          <button
            type="button"
            className="btn btn-primary"
            disabled={busy}
            onClick={() => run(async () => void (await bridge.assistantLibrary('open', 'document', key)))}
          >
            {t('assistant.document.open')}
          </button>
        </span>
      </div>
      {saved ? (
        <p className="document-saved" role="status">
          {t('assistant.document.saved', { path: saved })}{' '}
          <button
            type="button"
            className="btn btn-ghost"
            onClick={() => run(async () => void (await bridge.assistantLibrary('show', 'document', key, saved)))}
          >
            {t('assistant.document.show')}
          </button>
        </p>
      ) : null}
      {error ? (
        <p className="assistant-error" role="alert">
          {errorText(error)}
        </p>
      ) : null}
      {open && preview !== null ? (
        <div className="document-preview" aria-label={t('assistant.document.previewOf', { name: doc.name })}>
          {renderPreview(preview)}
        </div>
      ) : null}
    </div>
  );
}
