// Three-step first-run wizard: Welcome → Mic check → First dictation.
// The last step installs the bundled speech model first when the daemon has
// none (F-M11: on a clean install Start could never be enabled).
// Skippable at every step; completing or skipping sets narrivy.onboarded.v1.
import { useEffect, useRef, useState } from 'react';
import { Button } from '../ui/primitives/Button';
import { useErrorHandler } from '../ui/primitives/Toast';
import { t } from '../i18n';
import { activationPresentation } from '../lib/activation';
import { bridge, type CapabilityCheckResult } from '../lib/bridge';
import { availabilityMessageKey } from '../lib/pipelines';
import { usePipelines } from '../state/PipelinesContext';
import { useDaemonStatus } from '../state/StatusContext';
import { useDictationActions } from '../hooks/useDictationActions';
import { keepTabInside, useDialogFocus } from '../hooks/useDialogFocus';
import { BrandMark } from '../ui/icons';
import { provisionFailureText } from './screens/Models';
import { ShortcutCapture } from './components/ShortcutCapture';
import { BundledModelSetup } from './components/BundledModelSetup';

type MicState =
  | { kind: 'idle' }
  | { kind: 'running' }
  | { kind: 'done'; result: CapabilityCheckResult };

export function Onboarding({ onDone }: { onDone: () => void }) {
  const [step, setStep] = useState<0 | 1 | 2>(0);
  const [mic, setMic] = useState<MicState>({ kind: 'idle' });
  const [started, setStarted] = useState(false);
  const [stopped, setStopped] = useState(false);
  // F-M20: set the dictation shortcut here when the service manages them.
  const [capturing, setCapturing] = useState(false);
  const [shortcutSaved, setShortcutSaved] = useState(false);
  const report = useErrorHandler();
  const dictation = useDictationActions();
  const dialog = useRef<HTMLDivElement>(null);
  const heading = useRef<HTMLHeadingElement>(null);
  const firstStep = useRef(true);
  useDialogFocus(dialog);
  // Next and Back remove the button that had focus: move it to the new
  // step's heading so it is read and Tab starts from the top of the step.
  useEffect(() => {
    if (firstStep.current) {
      firstStep.current = false;
      return;
    }
    heading.current?.focus();
  }, [step]);
  const { catalog } = usePipelines();
  const { status, reload } = useDaemonStatus();
  const availabilityKey = availabilityMessageKey(catalog);
  // Truthful activation wording: the exact command is offered only when the
  // daemon reports the atomic toggle and no live portal listener, and it is
  // always labelled as a user-managed suggestion, never as configured.
  const activation = activationPresentation(status);

  const finish = () => onDone();

  const shortcutsManaged = (status?.capabilities ?? []).includes('activation.shortcuts=managed');
  const shortcutPipeline = catalog.pipelines.find((p) => p.id === catalog.selectedId) ?? null;

  const modelState = status?.installed_model ?? null;
  const needsModel =
    modelState === 'awaiting_model' ||
    modelState === 'provisioning' ||
    modelState === 'activation_failed';
  const provisionState = status?.model_provisioning?.state ?? null;

  const cancelModel = async () => {
    const operationId = status?.model_provisioning?.operation_id;
    if (!operationId) return;
    try {
      await bridge.cancelModelInstall(operationId);
      await reload();
    } catch (err) {
      report(err);
    }
  };

  const installModel = async () => {
    try {
      await bridge.installBundledModel();
      await reload();
    } catch (err) {
      report(err, { retry: () => void installModel() });
    }
  };

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
      <div
        ref={dialog}
        className="modal onboard-card"
        role="dialog"
        aria-modal="true"
        aria-label={t('onboard.title')}
        tabIndex={-1}
        onKeyDown={(event) => keepTabInside(event, dialog.current)}
      >
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
            <h2 ref={heading} tabIndex={-1}>{t('onboard.welcome.title')}</h2>
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
            {shortcutsManaged && shortcutPipeline ? (
              <div className="onboard-shortcut" data-testid="onboard-set-shortcut">
                <p>{t('onboard.shortcut.setBody')}</p>
                {shortcutSaved ? (
                  <p className="source" role="status">{t('onboard.shortcut.saved')}</p>
                ) : (
                  <Button onClick={() => setCapturing(true)}>{t('onboard.shortcut.set')}</Button>
                )}
              </div>
            ) : null}
            {capturing && shortcutPipeline ? (
              <ShortcutCapture
                target={{ pipelineId: shortcutPipeline.id, pipelineName: shortcutPipeline.name }}
                onClose={() => setCapturing(false)}
                onSaved={() => {
                  setCapturing(false);
                  setShortcutSaved(true);
                }}
              />
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
            <h2 ref={heading} tabIndex={-1}>{t('onboard.mic.title')}</h2>
            <p>{t('onboard.mic.body')}</p>
            <Button onClick={() => void runMicCheck()} disabled={mic.kind === 'running'}>
              {t('onboard.mic.run')}
            </Button>
            <div aria-live="polite">
            {mic.kind === 'done' ? (
              <p className="diag-result">
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
            </div>
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
            <h2 ref={heading} tabIndex={-1}>{t('onboard.dictation.title')}</h2>
            <p>{t('onboard.dictation.body')}</p>
            {needsModel ? (
              <BundledModelSetup
                provisionState={provisionState}
                failureText={provisionFailureText(status?.model_provisioning?.failure)}
                onInstall={() => void installModel()}
                onCancel={() => void cancelModel()}
              />
            ) : null}
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
