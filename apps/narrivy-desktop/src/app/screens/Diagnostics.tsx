import { allCapabilityStates } from '../../lib/capabilityStates';
// Isolated capability checks, each with its own Run action. A result shows
// pass/fail, Disabled when the capability is present but turned off, and the
// daemon's detail line.
import { Button } from '../../ui/primitives/Button';
import { Icon, type IconName } from '../../ui/icons';
import { t } from '../../i18n';
import { DIAGNOSTIC_CHECKS, useDiagnostics } from '../../state/DiagnosticsContext';

type Verdict = 'idle' | 'running' | 'pass' | 'fail' | 'disabled' | 'error';

const GLYPHS: Record<Verdict, IconName> = {
  idle: 'info',
  running: 'refresh',
  pass: 'check',
  fail: 'close',
  disabled: 'alert',
  error: 'alert',
};

function CheckRow({ id, title }: { id: (typeof DIAGNOSTIC_CHECKS)[number]['id']; title: string }) {
  const { states, runCheck } = useDiagnostics();
  const state = states[id];
  let verdict: Verdict = 'idle';
  let line = t('diag.idle');
  let detail: string | undefined;
  if (state?.status === 'running') {
    verdict = 'running';
    line = t('diag.running');
  } else if (state?.status === 'error') {
    verdict = 'error';
    line = t('diag.error');
    detail = state.error?.message;
  } else if (state?.status === 'done') {
    const r = state.result;
    verdict = r?.disabled ? 'disabled' : r?.pass ? 'pass' : 'fail';
    line = r?.disabled ? t('diag.disabled') : r?.pass ? t('diag.pass') : t('diag.fail');
    detail = r?.detail;
  }
  return (
    <div className="check-row">
      <span className="check-glyph" data-verdict={verdict} aria-hidden="true">
        <Icon name={GLYPHS[verdict]} />
      </span>
      <div>
        <div className="check-title">
          {title} <span className="check-id">{id}</span>
        </div>
        <div className={`diag-result${verdict === 'error' ? ' diag-error' : ''}`} role={verdict === 'error' ? 'alert' : undefined}>
          {line}
        </div>
        {detail ? <div className="diag-detail">{detail}</div> : null}
      </div>
      <Button variant="ghost" onClick={() => void runCheck(id)} aria-label={t('diag.runAria', { title })}>
        {t('diag.run')}
      </Button>
    </div>
  );
}

export function DiagnosticsScreen() {
  const { runAll, runningAll } = useDiagnostics();

  return (
    <div className="page">
      <div className="page-head" style={{ display: 'flex', alignItems: 'flex-end', gap: 16 }}>
        <div style={{ flex: 1 }}>
          <h2 className="page-title">{t('nav.diagnostics')}</h2>
          <p className="page-lead">{t('diag.lead')}</p>
        </div>
        <Button variant="primary" onClick={() => void runAll()} disabled={runningAll}>
          <Icon name="refresh" />
          {t('diag.runAll')}
        </Button>
      </div>
      <div className="panel check-list">
        {DIAGNOSTIC_CHECKS.map((check) => (
          <CheckRow key={check.id} id={check.id} title={t(check.titleKey)} />
        ))}
      </div>
    </div>
  );
}

// State-token enumeration (mirrors src-tauri vocabulary; consumed by tests).
export const CAPABILITY_STATE_TOKENS = allCapabilityStates();
