// Versioned pipeline export/import (P7 FUN-033/UX-011), shared by Settings
// and the Pipelines screen: two buttons, each asking for the file with the
// desktop's own chooser.
import { useState } from 'react';
import { Button } from '../../ui/primitives/Button';
import { Icon } from '../../ui/icons';
import { useErrorHandler, useToasts } from '../../ui/primitives/Toast';
import { bridge } from '../../lib/bridge';
import { t } from '../../i18n';

const DEFAULT_PIPELINE_EXPORT_NAME = 'narrivy-pipelines-export.json';

export function PipelinesIoButtons() {
  const report = useErrorHandler();
  const { push } = useToasts();
  const [working, setWorking] = useState(false);
  const run = async (task: () => Promise<void>, retry: () => void) => {
    setWorking(true);
    try {
      await task();
    } catch (err) {
      report(err, { retry });
    } finally {
      setWorking(false);
    }
  };
  const doExport = () =>
    run(async () => {
      const path = await bridge.pickFile(t('settings.pipelines.exportTitle'), true, DEFAULT_PIPELINE_EXPORT_NAME);
      if (!path) return;
      const rep = await bridge.pipelinesExport(path);
      push(t('settings.pipelines.exported', { count: rep.count, path: rep.path }));
    }, () => void doExport());
  const doImport = () =>
    run(async () => {
      const path = await bridge.pickFile(t('settings.pipelines.importTitle'), false);
      if (!path) return;
      const rep = await bridge.pipelinesImport(path, false);
      const deps = rep.required_dependencies.join(', ');
      push(
        t('settings.pipelines.imported', { imported: rep.imported, skipped: rep.skipped_duplicates }) +
          (deps ? ` ${t('settings.pipelines.preview', { deps })}` : ''),
      );
    }, () => void doImport());
  return (
    <>
      <Button onClick={() => void doImport()} disabled={working}>
        <Icon name="doc" />
        {t('settings.pipelines.import')}
      </Button>
      <Button onClick={() => void doExport()} disabled={working}>
        <Icon name="download" />
        {t('settings.pipelines.export')}
      </Button>
    </>
  );
}
