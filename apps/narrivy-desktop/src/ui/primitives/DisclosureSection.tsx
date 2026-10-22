import type { ReactNode } from 'react';
import { t } from '../../i18n';

export function DisclosureSection({
  title,
  meta,
  defaultOpen = false,
  children,
}: {
  title: ReactNode;
  meta?: ReactNode;
  defaultOpen?: boolean;
  children: ReactNode;
}) {
  return (
    <details className="disclosure" open={defaultOpen}>
      <summary>
        {title}
        {meta ? <span className="source"> — {meta}</span> : null}
      </summary>
      <div className="disclosure-body">{children}</div>
    </details>
  );
}

export function DisclosureFallbackNote(): string {
  return t('common.loading');
}
