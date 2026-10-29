// Pipeline export/import (P7 FUN-033/UX-011), shared by Settings and the
// Pipelines screen. Planned, not in 1.0: the service lists only its built-in
// pipelines and never reads saved pipeline files (F-H13), so an import would
// change nothing. The buttons stay visible, disabled, with a Planned label.
import { Button } from '../../ui/primitives/Button';
import { Icon } from '../../ui/icons';
import { t } from '../../i18n';

export function PipelinesIoButtons() {
  const title = t('settings.pipelines.plannedTitle');
  return (
    <>
      <span className="chip chip-warn fm-badge" title={title}>
        {t('settings.pipelines.planned')}
      </span>
      <Button disabled title={title}>
        <Icon name="doc" />
        {t('settings.pipelines.import')}
      </Button>
      <Button disabled title={title}>
        <Icon name="download" />
        {t('settings.pipelines.export')}
      </Button>
    </>
  );
}
