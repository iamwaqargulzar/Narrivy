// First run: Welcome → Microphone → Speech model → Try it → Shortcut. A new
// install has no speech model (D40), so the wizard downloads one in place
// and loads it; the microphone test records a few seconds and says whether
// the user was heard. Skippable at every step; completing or skipping sets
// narrivy.onboarded.v1.
import { useCallback, useEffect, useRef, useState } from 'react';
import { Button } from '../ui/primitives/Button';
import { useErrorHandler } from '../ui/primitives/Toast';
import { t } from '../i18n';
import { bridge, type AudioOptions, type ModelInfo } from '../lib/bridge';
import { languageSummary, modelName } from '../lib/modelFacts';
import { formatBytes } from '../lib/overview';
import { availabilityMessageKey } from '../lib/pipelines';
import { usePipelines } from '../state/PipelinesContext';
import { useSession } from '../state/SessionContext';
import { useActivation, useDaemonStatus } from '../state/StatusContext';
import { useDictationActions } from '../hooks/useDictationActions';
import { keepTabInside, useDialogFocus } from '../hooks/useDialogFocus';
import { BrandMark } from '../ui/icons';
import { ShortcutCapture } from './components/ShortcutCapture';
import { DownloadProgress, offeredModels } from './screens/Models';

const STEPS = 5;
type Step = 0 | 1 | 2 | 3 | 4;

const DOWNLOAD_POLL_MS = 1000;

type MicResult = 'heard' | 'quiet' | 'silent' | 'blocked' | 'failed';
type MicState = { kind: 'idle' } | { kind: 'running' } | { kind: 'done'; result: MicResult; detail: string };

/** The check's detail starts with a stable word (see the service's `mic_check`). */
function micResult(pass: boolean, detail: string): MicResult {
  if (pass) return 'heard';
  if (detail.startsWith('quiet:')) return 'quiet';
  if (detail.startsWith('silent:')) return 'silent';
  if (detail.startsWith('blocked:')) return 'blocked';
  return 'failed';
}

function MicrophoneStep() {
  const report = useErrorHandler();
  const [audio, setAudio] = useState<AudioOptions | null>(null);
  const [saving, setSaving] = useState(false);
  const [mic, setMic] = useState<MicState>({ kind: 'idle' });
  useEffect(() => {
    bridge.audioOptions().then(setAudio, () => setAudio(null));
  }, []);
  const choose = async (name: string) => {
    setSaving(true);
    setMic({ kind: 'idle' });
    try {
      setAudio(await bridge.setMicrophone(name));
    } catch (err) {
      report(err, { retry: () => void choose(name) });
    } finally {
      setSaving(false);
    }
  };
  const test = async () => {
    setMic({ kind: 'running' });
    try {
      const result = await bridge.capabilityCheck('mic');
      setMic({ kind: 'done', result: micResult(result.pass, result.detail ?? ''), detail: result.detail ?? '' });
    } catch (err) {
      setMic({ kind: 'idle' });
      report(err, { retry: () => void test() });
    }
  };
  const chosen = audio?.microphone ?? '';
  const microphones = audio?.microphones ?? [];
  return (
    <>
      {microphones.length > 0 ? (
        <label className="onboard-field">
          <span>{t('onboard.mic.choose')}</span>
          <select
            className="pipe-select"
            value={chosen}
            disabled={saving || mic.kind === 'running'}
            onChange={(event) => void choose(event.target.value)}
          >
            <option value="">{t('pipe.mic.systemDefault')}</option>
            {microphones.map((m) => (
              <option key={m.name} value={m.name}>
                {m.description}
              </option>
            ))}
          </select>
        </label>
      ) : null}
      <div className="onboard-try">
        <Button onClick={() => void test()} disabled={saving || mic.kind === 'running'}>
          {mic.kind === 'done' ? t('onboard.mic.again') : t('onboard.mic.run')}
        </Button>
      </div>
      <div aria-live="polite">
        {mic.kind === 'running' ? (
          <p className="onboard-result" data-tone="busy">{t('onboard.mic.listening')}</p>
        ) : mic.kind === 'done' ? (
          <p className="onboard-result" data-tone={mic.result === 'heard' ? 'ok' : 'warn'}>
            {t(`onboard.mic.${mic.result}`)}
            {mic.result === 'failed' ? <span className="diag-detail"> {mic.detail}</span> : null}
          </p>
        ) : null}
      </div>
    </>
  );
}

function ModelStep({ ready }: { ready: boolean }) {
  const report = useErrorHandler();
  const [models, setModels] = useState<ModelInfo[] | null>(null);
  const [picked, setPicked] = useState<string | null>(null);
  const [loading, setLoading] = useState<string | null>(null);
  const reload = useCallback(() => bridge.models().then(setModels), []);
  useEffect(() => {
    reload().catch(() => setModels([]));
  }, [reload]);

  const downloading = models?.find((m) => m.state === 'downloading') ?? null;
  // While a download runs, follow it; once it lands, load it.
  useEffect(() => {
    if (!downloading) return;
    const id = downloading.id;
    const timer = window.setInterval(() => {
      bridge
        .models()
        .then((list) => {
          setModels(list);
          const done = list.find((m) => m.id === id);
          if (done && done.state === 'installed') {
            setLoading(id);
            bridge.selectModel(id).catch((err) => {
              setLoading(null);
              report(err);
            });
          }
        })
        .catch(() => {});
    }, DOWNLOAD_POLL_MS);
    return () => window.clearInterval(timer);
  }, [downloading, report]);

  const active = models?.find((m) => m.state === 'active') ?? null;
  if (ready || active) {
    return (
      <p className="onboard-result" data-tone="ok" role="status">
        {active ? t('onboard.model.readyNamed', { name: modelName(active) }) : t('onboard.model.ready')}
      </p>
    );
  }
  if (loading) {
    return (
      <p className="onboard-result" data-tone="busy" role="status">
        {t('onboard.model.loading')}
      </p>
    );
  }
  if (models === null) return <p className="source">{t('onboard.model.listing')}</p>;
  // Downloaded but not loaded (an earlier run): offer to use it.
  const installed = models.find((m) => m.state === 'installed') ?? null;
  if (installed) {
    return (
      <div className="onboard-try">
        <Button
          variant="primary"
          onClick={() => {
            setLoading(installed.id);
            bridge.selectModel(installed.id).catch((err) => {
              setLoading(null);
              report(err);
            });
          }}
        >
          {t('onboard.model.use', { name: modelName(installed) })}
        </Button>
      </div>
    );
  }
  if (downloading) {
    return (
      <div className="onboard-download" role="status">
        <p>{t('onboard.model.downloading', { name: modelName(downloading) })}</p>
        <DownloadProgress model={downloading} />
        <Button
          variant="ghost"
          onClick={() => {
            bridge
              .cancelModelDownload(downloading.id)
              .then(reload)
              .catch((err) => report(err));
          }}
        >
          {t('action.cancel')}
        </Button>
      </div>
    );
  }
  const offered = offeredModels(models);
  if (offered.length === 0) {
    return <p className="onboard-result" data-tone="warn">{t('onboard.model.none')}</p>;
  }
  const choice = picked ?? offered[0].id;
  return (
    <>
      <div className="onboard-models" role="radiogroup" aria-label={t('onboard.model.choose')}>
        {offered.map((m, index) => (
          <label key={m.id} className="onboard-model" data-picked={m.id === choice}>
            <input type="radio" name="onboard-model" checked={m.id === choice} onChange={() => setPicked(m.id)} />
            <span className="onboard-model-name">
              {modelName(m)}
              {index === 0 && !picked ? <span className="onboard-badge">{t('onboard.model.suggested')}</span> : null}
            </span>
            <span className="onboard-model-facts">
              {[formatBytes(m.size_bytes), languageSummary(m)].filter(Boolean).join(' · ')}
            </span>
          </label>
        ))}
      </div>
      <div className="onboard-try">
        <Button
          variant="primary"
          onClick={() => {
            bridge
              .downloadModel(choice)
              .then(reload)
              .catch((err) => report(err));
          }}
        >
          {t('onboard.model.download')}
        </Button>
      </div>
      <p className="source">{t('onboard.model.note')}</p>
    </>
  );
}

export function Onboarding({ onDone }: { onDone: () => void }) {
  const [step, setStep] = useState<Step>(0);
  const [capturing, setCapturing] = useState(false);
  const [shortcutSaved, setShortcutSaved] = useState(false);
  const dictation = useDictationActions();
  const session = useSession();
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
  const { status } = useDaemonStatus();
  const availabilityKey = availabilityMessageKey(catalog);
  const activation = useActivation();
  const shortcutsManaged = (status?.capabilities ?? []).includes('activation.shortcuts=managed');
  const shortcutPipeline = catalog.pipelines.find((p) => p.id === catalog.selectedId) ?? null;
  const modelState = status?.installed_model ?? null;
  const modelReady = modelState === 'ready';

  // The text shown is the one from a dictation started here, not an older one.
  const [finishedAtStart, setFinishedAtStart] = useState<number | null>(null);
  // Start, Stop and the words under them follow the service's session, not
  // what was clicked here: a dictation stopped elsewhere (shortcut, tray)
  // must not leave Start locked or Stop pointing at nothing.
  const doStart = async () => {
    setFinishedAtStart(session.finishedSessions);
    await dictation.start();
  };
  const finishedHere =
    finishedAtStart !== null && session.finishedSessions !== finishedAtStart && !dictation.active;
  const heard = finishedHere ? session.lastTranscript : null;

  // A shortcut press starts a dictation without the Start button, so the
  // baseline is also taken on entering Try it.
  const go = (next: Step) => {
    if (next === 4) setFinishedAtStart(session.finishedSessions);
    setStep(next);
  };
  const hasShortcut = shortcutSaved || activation.portalListenerReady;
  const skip = (
    <Button variant="ghost" onClick={onDone}>
      {t('action.skip')}
    </Button>
  );
  const back = step > 0 ? <Button onClick={() => go((step - 1) as Step)}>{t('action.back')}</Button> : null;
  const next = (disabled = false) => (
    <Button variant="primary" disabled={disabled} onClick={() => go((step + 1) as Step)}>
      {t('action.next')}
    </Button>
  );

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
            {Array.from({ length: STEPS }, (_, i) => (
              <span key={i} data-state={i < step ? 'done' : i === step ? 'current' : 'next'} />
            ))}
          </div>
          <p className="source">{t('onboard.step', { n: step + 1, total: STEPS })}</p>
        </div>

        {step === 0 ? (
          <>
            <h2 ref={heading} tabIndex={-1}>{t('onboard.welcome.title')}</h2>
            <p>{t('onboard.welcome.body')}</p>
            <ol className="onboard-list">
              <li>{t('onboard.welcome.mic')}</li>
              <li>{t('onboard.welcome.model')}</li>
              <li>{t('onboard.welcome.shortcut')}</li>
              <li>{t('onboard.welcome.try')}</li>
            </ol>
            <p className="source">{t('onboard.welcome.private')}</p>
            <div className="modal-actions">
              {skip}
              {next()}
            </div>
          </>
        ) : null}

        {step === 1 ? (
          <>
            <h2 ref={heading} tabIndex={-1}>{t('onboard.mic.title')}</h2>
            <p>{t('onboard.mic.body')}</p>
            <MicrophoneStep />
            <div className="modal-actions">
              {skip}
              {back}
              {next()}
            </div>
          </>
        ) : null}

        {step === 2 ? (
          <>
            <h2 ref={heading} tabIndex={-1}>{t('onboard.model.title')}</h2>
            <p>{t('onboard.model.body')}</p>
            <ModelStep ready={modelReady} />
            <div className="modal-actions">
              {skip}
              {back}
              {next(!modelReady)}
            </div>
          </>
        ) : null}

        {step === 3 ? (
          <>
            <h2 ref={heading} tabIndex={-1}>{t('onboard.shortcut.title')}</h2>
            {shortcutsManaged && shortcutPipeline ? (
              <div data-testid="onboard-set-shortcut">
                <p>{t('onboard.shortcut.setBody')}</p>
                {shortcutSaved ? (
                  <p className="onboard-result" data-tone="ok" role="status">{t('onboard.shortcut.saved')}</p>
                ) : (
                  <div className="onboard-try">
                    <Button variant="primary" onClick={() => setCapturing(true)}>{t('onboard.shortcut.set')}</Button>
                  </div>
                )}
              </div>
            ) : activation.portalListenerReady ? (
              <p role="status">{t('onboard.shortcut.listenerReady')}</p>
            ) : activation.suggestedCommand ? (
              <div className="onboard-shortcut">
                <p>{t('onboard.shortcut.manualBody')}</p>
                <p className="command" data-testid="activation-shortcut-command">
                  {activation.suggestedCommand}
                </p>
                <p>{t('onboard.shortcut.manualHow')}</p>
              </div>
            ) : (
              <p>{t('onboard.shortcut.later')}</p>
            )}
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
              {skip}
              {back}
              {next()}
            </div>
          </>
        ) : null}
        {step === 4 ? (
          <>
            <h2 ref={heading} tabIndex={-1}>{t('onboard.dictation.title')}</h2>
            <p>{t(hasShortcut ? 'onboard.dictation.bodyShortcut' : 'onboard.dictation.body')}</p>
            {availabilityKey ? (
              <p className="source" role="status">
                {t(availabilityKey)}
              </p>
            ) : null}
            <div className="onboard-try">
              <Button
                variant="primary"
                className="btn-press"
                onClick={() => void doStart()}
                disabled={!dictation.canStart}
              >
                {finishedHere ? t('onboard.dictation.again') : t('onboard.dictation.start')}
              </Button>
              <Button onClick={() => void dictation.stop()} disabled={!dictation.canStop}>
                {t('onboard.dictation.stop')}
              </Button>
            </div>
            <div aria-live="polite">
              {dictation.canStop ? (
                <p className="onboard-result" data-tone="busy">{t('onboard.dictation.listening')}</p>
              ) : dictation.active ? (
                <p className="onboard-result" data-tone="busy">{t('onboard.dictation.working')}</p>
              ) : heard ? (
                <div className="onboard-heard">
                  <p className="source">{t('onboard.dictation.heard')}</p>
                  <p>{heard}</p>
                </div>
              ) : finishedHere ? (
                <p className="onboard-result" data-tone="warn">{t('onboard.dictation.nothing')}</p>
              ) : null}
            </div>
            <div className="modal-actions">
              {back}
              <Button variant="primary" onClick={onDone}>
                {t('onboard.finish')}
              </Button>
            </div>
          </>
        ) : null}

      </div>
    </div>
  );
}
