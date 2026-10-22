// Three-step first-run wizard: Welcome → Mic check → First dictation.
// Skippable at every step; completing or skipping sets narrivy.onboarded.v1.
import { useState } from 'react';
import { Button } from '../ui/primitives/Button';
import { useErrorHandler } from '../ui/primitives/Toast';
import { t } from '../i18n';
import { activationPresentation } from '../lib/activation';
import { bridge, type CapabilityCheckResult } from '../lib/bridge';
import { availabilityMessageKey } from '../lib/pipelines';
import { usePipelines } from '../state/PipelinesContext';
import { useDaemonStatus } from '../state/StatusContext';
import { useDictationActions } from '../hooks/useDictationActions';
import { BrandMark } from '../ui/icons';

type MicState =
  | { kind: 'idle' }
  | { kind: 'running' }
  | { kind: 'done'; result: CapabilityCheckResult };

export function Onboarding({ onDone }: { onDone: () => void }) {
  const [step, setStep] = useState<0 | 1 | 2>(0);
  const [mic, setMic] = useState<MicState>({ kind: 'idle' });
  const [started, setStarted] = useState(false);
  const [stopped, setStopped] = useState(false);
  const report = useErrorHandler();
  const dictation = useDictationActions();
  const { catalog } = usePipelines();
  const { status } = useDaemonStatus();
  const availabilityKey = availabilityMessageKey(catalog);
  // Truthful activation wording: the exact command is offered only when the
  // daemon reports the atomic toggle and no live portal listener, and it is
  // always labelled as a user-managed suggestion, never as configured.
  const activation = activationPresentation(status);

  const finish = () => onDone();

  const runMicCheck = async () => {
    setMic({ kind: 'running' });
    try {
      const result = await bridge.capabilityCheck('mic');
      setMic({ kind: 'done', result });
    } catch (err) {
      setMic({ kind: 'idle' });
      report(err, { retry: () => void runMicCheck() });
    }
  };

  const doStart = async () => {
    if (await dictation.start()) setStarted(true);
  };

  const doStop = async () => {
    if (await dictation.stop()) setStopped(true);
  };

  return (
    <div className="modal-overlay onboarding">
      <div className="modal onboard-card" role="dialog" aria-modal="true" aria-label={t('onboard.title')}>
        <div className="onboard-top">
          <BrandMark />
          <div className="onboard-progress" aria-hidden="true">
            {[0, 1, 2].map((i) => (
              <span key={i} data-state={i < step ? 'done' : i === step ? 'current' : 'next'} />
            ))}
          </div>
          <p className="source">{t('onboard.step', { n: step + 1 })}</p>
        </div>

        {step === 0 ? (
          <>
            <h2>{t('onboard.welcome.title')}</h2>
            <p>{t('onboard.welcome.body')}</p>
            {activation.suggestedCommand ? (
              <div className="onboard-shortcut" role="status">
                <p>{t('onboard.shortcut.title')}</p>
                <p>{t('onboard.shortcut.body')}</p>
                <p className="command" data-testid="activation-shortcut-command">
                  {t('onboard.shortcut.suggested', { command: activation.suggestedCommand })}
                </p>
                <p>{t('onboard.shortcut.notConfigured')}</p>
              </div>
            ) : activation.portalListenerReady ? (
              <p className="source" role="status">
                {t('onboard.shortcut.listenerReady')}
              </p>
            ) : null}
            <div className="modal-actions">
              <Button variant="ghost" onClick={finish}>
                {t('action.skip')}
              </Button>
              <Button variant="primary" onClick={() => setStep(1)}>
                {t('action.next')}
              </Button>
            </div>
          </>
        ) : null}

        {step === 1 ? (
          <>
            <h2>{t('onboard.mic.title')}</h2>
            <p>{t('onboard.mic.body')}</p>
            <Button onClick={() => void runMicCheck()} disabled={mic.kind === 'running'}>
              {t('onboard.mic.run')}
            </Button>
            {mic.kind === 'done' ? (
              <p className="diag-result" aria-live="polite">
                {mic.result.disabled ? t('diag.disabled') : mic.result.pass ? t('diag.pass') : t('diag.fail')}
                {mic.result.detail ? (
                  <span className="diag-detail">
                    {' '}
                    {t('onboard.mic.detail')} {mic.result.detail}
                  </span>
                ) : null}
              </p>
            ) : mic.kind === 'running' ? (
              <p className="source">{t('diag.running')}</p>
            ) : null}
            <div className="modal-actions">
              <Button variant="ghost" onClick={finish}>
                {t('action.skip')}
              </Button>
              <Button onClick={() => setStep(0)}>{t('action.back')}</Button>
              <Button variant="primary" disabled={mic.kind !== 'done'} onClick={() => setStep(2)}>
                {t('action.next')}
              </Button>
            </div>
          </>
        ) : null}

        {step === 2 ? (
          <>
            <h2>{t('onboard.dictation.title')}</h2>
            <p>{t('onboard.dictation.body')}</p>
            {availabilityKey ? (
              <p className="source" role="status">
                {t(availabilityKey)}
              </p>
            ) : null}
            <div className="onboard-try">
              <Button variant="primary" className="btn-press" onClick={() => void doStart()} disabled={started || !dictation.canStart}>
                {t('onboard.dictation.start')}
              </Button>
              <Button onClick={() => void doStop()} disabled={!started || stopped}>
                {t('onboard.dictation.stop')}
              </Button>
            </div>
            <div className="modal-actions">
              <Button variant="ghost" onClick={finish}>
                {t('action.skip')}
              </Button>
              <Button variant="primary" disabled={!stopped} onClick={finish}>
                {t('onboard.finish')}
              </Button>
            </div>
          </>
        ) : null}
      </div>
    </div>
  );
}
