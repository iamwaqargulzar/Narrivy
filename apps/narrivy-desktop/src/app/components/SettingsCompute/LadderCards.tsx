// Visual summaries from the Settings reference design: the automatic compute
// priority and the text-insertion fallback chain. Each step's state comes from
// daemon capabilities; steps that are scheduled say so instead of pretending.
import { Icon, type IconName } from '../../../ui/icons';
import { t } from '../../../i18n';
import { useDaemonStatus } from '../../../state/StatusContext';

function cap(caps: string[], prefix: string): string | null {
  const found = caps.find((c) => c.startsWith(prefix));
  return found ? found.slice(prefix.length) : null;
}

export function ComputePriorityCard() {
  const { status } = useDaemonStatus();
  const caps = status?.capabilities ?? [];
  const device = cap(caps, 'compute.device=');
  const accelerated = device !== null && !device.startsWith('unavailable');
  const cpuInUse = !accelerated && (cap(caps, 'asr.transcribe-cpp.cpu=') === 'available' || caps.includes('asr.transcribe-cpp'));
  const tiers: Array<{ key: string; inUse: boolean }> = [
    { key: 'npu', inUse: false },
    { key: 'dgpu', inUse: false },
    { key: 'igpu', inUse: false },
    { key: 'cpu', inUse: cpuInUse },
  ];
  return (
    <div className="settings-card" data-testid="compute-priority">
      <div className="settings-card-header">
        <Icon name="cpu" />
        {t('ladder.priority.title')}
        <span className="chip chip-warn fm-badge">{t('fm.status.planned', { task: 'T2.15–T2.16' })}</span>
      </div>
      <p className="settings-card-desc">{t('ladder.priority.desc')}</p>
      <ol className="priority-list">
        {tiers.map((tier, index) => (
          <li key={tier.key} data-in-use={tier.inUse ? 'true' : 'false'}>
            <span className="priority-index">{index + 1}.</span>
            <span className="priority-name">{t(`ladder.tier.${tier.key}`)}</span>
            {tier.inUse ? <span className="chip chip-ok fm-badge">{t('models.tier.inUse')}</span> : null}
            <Icon name="grip" className="priority-grip" />
          </li>
        ))}
      </ol>
      <div className="resolved-line">
        <Icon name="checkCircle" />
        <span>
          {t('ladder.priority.current')}{' '}
          <strong>{accelerated ? device : cpuInUse ? t('ladder.tier.cpu') : t('fm.value.notReported')}</strong>
        </span>
      </div>
    </div>
  );
}

export function InsertionChainCard() {
  const { status } = useDaemonStatus();
  const caps = status?.capabilities ?? [];
  const typing = cap(caps, 'delivery.wtype-helper=');
  const inbox = cap(caps, 'delivery.inbox-recovery=');
  const clipboard = caps.includes('delivery.clipboard');
  const steps: Array<{ key: string; icon: IconName; state: 'on' | 'off' | 'planned' }> = [
    { key: 'type', icon: 'keyboard', state: typing?.startsWith('ready') ? 'on' : 'off' },
    { key: 'paste', icon: 'clipboard', state: clipboard ? 'on' : 'planned' },
    { key: 'inbox', icon: 'inbox', state: inbox === 'enabled' ? 'on' : 'off' },
  ];
  return (
    <div className="settings-card" data-testid="insertion-chain">
      <div className="settings-card-header">
        <Icon name="send" />
        {t('ladder.chain.title')}
      </div>
      <p className="settings-card-desc">{t('ladder.chain.desc')}</p>
      <div className="chain">
        {steps.map((step, index) => (
          <div key={step.key} className="chain-part">
            {index > 0 ? <Icon name="arrowRight" className="chain-arrow" /> : null}
            <div className="chain-step" data-state={step.state}>
              <span className="chain-icon" aria-hidden="true">
                <Icon name={step.icon} />
              </span>
              <span className="chain-name">{t(`ladder.chain.${step.key}`)}</span>
              <span className="chain-state">
                {step.state === 'on'
                  ? t('fm.value.ready')
                  : step.state === 'planned'
                    ? t('fm.status.planned', { task: 'T2.4' })
                    : t('fm.value.unavailable')}
              </span>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
