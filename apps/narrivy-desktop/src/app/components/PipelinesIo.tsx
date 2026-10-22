// Versioned pipeline export/import (P7 FUN-033/UX-011), shared by Settings
// and the Pipelines screen.
import { useState } from 'react';
import { Button } from '../../ui/primitives/Button';
import { useErrorHandler } from '../../ui/primitives/Toast';
import { bridge } from '../../lib/bridge';
import { t } from '../../i18n';

const DEFAULT_PIPELINE_EXPORT_PATH = 'narrivy-pipelines-export.json';

export function PipelinesIoSection() {
  const report = useErrorHandler();
  const [path, setPath] = useState(DEFAULT_PIPELINE_EXPORT_PATH);
  const [statusLine, setStatusLine] = useState<string | null>(null);
  const [working, setWorking] = useState(false);
  const doExport = async () => {
    setWorking(true);
    try {
      const rep = await bridge.pipelinesExport(path);
      setStatusLine(t('settings.pipelines.exported', { count: rep.count, path: rep.path }));
    } catch (err) {
      report(err, { retry: () => void doExport() });
    } finally {
      setWorking(false);
    }
  };
  const doImport = async (dryRun: boolean) => {
    setWorking(true);
    try {
      if (dryRun) {
        const preview = await bridge.pipelinesImport(path, true);
        setStatusLine(t('settings.pipelines.preview', { deps: preview.required_dependencies.join(', ') || '—' }));
        return;
      }
      const rep = await bridge.pipelinesImport(path, false);
      setStatusLine(t('settings.pipelines.imported', { imported: rep.imported, skipped: rep.skipped_duplicates }));
    } catch (err) {
      report(err, { retry: () => void doImport(dryRun) });
    } finally {
      setWorking(false);
    }
  };
  return (
    <div className="pipelines-io" data-testid="pipelines-io">
      <label htmlFor="pipelines-path">{t('settings.pipelines.pathLabel')}</label>
      <input id="pipelines-path" aria-label={t('settings.pipelines.pathLabel')} value={path} onChange={(e) => setPath(e.target.value)} />
      <div style={{ display: 'flex', gap: 'var(--space-2)', marginTop: 'var(--space-2)' }}>
        <Button onClick={() => void doExport()} disabled={working}>{t('settings.pipelines.export')}</Button>
        <Button onClick={() => void doImport(true)} disabled={working}>{t('settings.pipelines.previewBtn')}</Button>
        <Button variant="primary" onClick={() => void doImport(false)} disabled={working}>{t('settings.pipelines.import')}</Button>
      </div>
      {statusLine ? (
        <p className="setting-resolved" role="status">{statusLine}</p>
      ) : null}
    </div>
  );
}

