// The live status bar along the bottom of the window: what is active right
// now (the model and the mode, and the models as switches), that everything
// stays on this computer, and what is in use: the Narrivy service's memory,
// and small graphs of the last ten minutes of CPU (the service's share), GPU
// and NPU (the whole computer). Every value is read from daemon status, which
// the status provider refreshes every few seconds; anything the daemon does
// not report is shown as a dash, never guessed.
import { resolveDelivery, resolveMic, resolveModel } from '../lib/resolved';
import { deliveryChip, micChip, modelChip } from '../lib/homeSummary';
import { useEffect, useState } from 'react';
import { activeDevice, formatBytes, formatPermille, resourceFacts } from '../lib/overview';
import { useDaemonStatus } from '../state/StatusContext';
import type { StatusSnapshot } from '../lib/bridge';
import { useAssistant } from '../state/AssistantContext';
import { usePipelines } from '../state/PipelinesContext';
import { Icon } from '../ui/icons';
import { t } from '../i18n';

type Tone = 'neutral' | 'ok' | 'warn';

interface StripItem {
  key: string;
  label: string;
  title?: string;
  tone: Tone;
}

function Items({ items, className }: { items: StripItem[]; className: string }) {
  return (
    <span className={className}>
      {items.map((item, index) => (
        <span key={item.key} className="strip-item" data-tone={item.tone} data-item={item.key} title={item.title}>
          {index > 0 ? <span className="strip-dot" aria-hidden="true" /> : null}
          {item.label}
        </span>
      ))}
    </span>
  );
}

/** The models, as switches where they can be switched: the speech model
 * stays loaded for dictation (a status only), the assistant's model loads
 * and unloads, and voice mode turns on and off. */
function ModelSwitches({ status }: { status: StatusSnapshot }) {
  const assistant = useAssistant();
  const worker = status.capabilities.find((c) => c.startsWith('asr.transcribe-cpp.worker='))?.split('=')[1];
  const device = status.capabilities.find((c) => c.startsWith('asr.transcribe-cpp.device='))?.split('=')[1];
  const { modelState, loadingModel, modelName, voice, load, unload, busy } = assistant;
  const loaded = modelState?.startsWith('loaded:') ?? false;
  const voiceLoaded = voice.state !== 'off';
  return (
    <span className="strip-group strip-models">
      {worker !== undefined ? (
        <span
          className="strip-model"
          data-item="speech"
          data-on={worker.startsWith('ready') ? 'true' : 'false'}
          title={t('strip.speechAlways')}
        >
          <span className="strip-led" aria-hidden="true" />
          {worker.startsWith('ready')
            ? device
              ? t('strip.speechReady', { device: device.toUpperCase() })
              : t('strip.speechLoaded')
            : t('strip.speechNotLoaded')}
        </span>
      ) : null}
      {modelState || loadingModel ? (
        <button
          type="button"
          role="switch"
          className="strip-model strip-switch"
          data-item="assistant"
          aria-checked={loaded}
          aria-label={t('strip.assistantSwitch')}
          aria-busy={loadingModel}
          disabled={loadingModel || (loaded && busy)}
          title={t('strip.assistantSwitchHint', { name: modelName ?? '' })}
          onClick={() => (loaded ? unload() : load())}
        >
          <span className="switch" aria-hidden="true">
            <span className="switch-thumb" />
          </span>
          {loadingModel
            ? t('strip.assistantLoadingShort')
            : loaded
              ? t('strip.assistantOn', { device: (modelState ?? '').slice('loaded:'.length).toUpperCase() })
              : t('strip.assistantOff')}
        </button>
      ) : null}
      {voice.prefs.available || voiceLoaded ? (
        <button
          type="button"
          role="switch"
          className="strip-model strip-switch"
          data-item="voice"
          aria-checked={voiceLoaded}
          aria-busy={voice.state === 'starting'}
          aria-label={t('strip.voiceSwitch')}
          title={t('strip.voiceSwitchHint')}
          onClick={() => (voiceLoaded ? voice.stop() : voice.start({ standby: true }))}
        >
          <span className="switch" aria-hidden="true">
            <span className="switch-thumb" />
          </span>
          {!voiceLoaded
            ? t('strip.voiceOff')
            : voice.state === 'starting'
              ? t('strip.voiceLoading')
              : voice.standby
                ? t('strip.voiceReady')
                : t('strip.voiceOnShort')}
        </button>
      ) : null}
    </span>
  );
}

/** How far back the graphs go. */
const HISTORY_MS = 10 * 60_000;

export interface Sample {
  at: number;
  cpu: number | null;
  gpu: number | null;
  npu: number | null;
}

/** The last ten minutes of samples, kept while the window is open (the
 * status bar is on every screen). */
const history: Sample[] = [];

function remember(sample: Sample): void {
  const last = history[history.length - 1];
  if (last && sample.at - last.at < 1000) return;
  history.push(sample);
  while (history.length && history[0].at < sample.at - HISTORY_MS) history.shift();
}

/** Fill the graphs with earlier samples (the dev showcase, for screenshots). */
export function seedUsageHistory(samples: Sample[]): void {
  samples.forEach(remember);
}

/** A small graph of one share over the last ten minutes, and its value now. */
function Meter({ name, label, title, values, now }: { name: string; label: string; title: string; values: Array<number | null>; now: number | null }) {
  const width = 44;
  const height = 14;
  const at = Date.now();
  const points = history
    .map((sample, i) => ({ x: width - ((at - sample.at) / HISTORY_MS) * width, y: values[i] }))
    .filter((point): point is { x: number; y: number } => point.y !== null)
    .map(({ x, y }) => `${Math.max(0, x).toFixed(1)},${(height - (Math.min(1000, y) / 1000) * (height - 1) - 0.5).toFixed(1)}`);
  return (
    <span className="strip-item strip-meter" data-item={name} title={title}>
      <span className="strip-meter-name">{label}</span>
      <svg className="strip-graph" width={width} height={height} viewBox={`0 0 ${width} ${height}`} aria-hidden="true">
        {points.length > 1 ? <polyline points={points.join(' ')} /> : null}
      </svg>
      <span className="strip-meter-value">{formatPermille(now) ?? t('strip.none')}</span>
    </span>
  );
}

export function StatusStrip() {
  const { status } = useDaemonStatus();
  const { catalog } = usePipelines();
  if (!status) {
    return (
      <footer className="status-strip" aria-label={t('strip.label')}>
        <Icon name="wave" className="strip-mark" />
        <span>{t('shell.status.connecting')}</span>
      </footer>
    );
  }

  const device = activeDevice(status);
  const model = modelChip(resolveModel(status, 'auto'));
  const mode = catalog.pipelines.find((p) => p.id === catalog.selectedId)?.name;
  const mic = micChip(resolveMic(status, 'auto'));
  const delivery = deliveryChip(resolveDelivery(status, 'auto'));

  const active: StripItem[] = [
    // Only when something is off: where it runs is under each model.
    ...(device.testEngine
      ? [{ key: 'device', label: t('strip.testEngine'), tone: 'neutral' as const }]
      : device.running
        ? []
        : [{ key: 'device', label: t('strip.noEngine'), tone: 'warn' as const }]),
    { key: 'model', label: model.label, title: model.detail, tone: model.tone },
    ...(mode ? [{ key: 'mode', label: mode, tone: 'neutral' as const }] : []),
    // Only problems are worth a slot here; a working mic and delivery are the norm.
    ...(mic.tone === 'warn' ? [{ key: 'mic', label: mic.label, title: mic.detail, tone: 'warn' as const }] : []),
    ...(delivery.tone === 'warn' ? [{ key: 'delivery', label: delivery.label, title: delivery.detail, tone: 'warn' as const }] : []),
  ];

  return (
    <footer className="status-strip" aria-label={t('strip.label')}>
      <Icon name="wave" className="strip-mark" />
      <Items items={active} className="strip-group" />
      <ModelSwitches status={status} />
      <span className="strip-spacer" />
      <span className="strip-item strip-private" title={t('strip.privateHint')}>
        <Icon name="lock" />
        {t('strip.private')}
      </span>
      <Usage status={status} />
    </footer>
  );
}

/** Memory as a number; CPU, GPU and NPU as graphs of the last minutes. */
function Usage({ status }: { status: StatusSnapshot }) {
  const resources = resourceFacts(status);
  const [, redraw] = useState(0);
  useEffect(() => {
    remember({ at: Date.now(), cpu: resources.cpuPermille, gpu: resources.gpuPermille, npu: resources.npuPermille });
    redraw((n) => n + 1);
  }, [status, resources.cpuPermille, resources.gpuPermille, resources.npuPermille]);
  const dash = t('strip.none');
  return (
    <span className="strip-group strip-stats">
      <span
        className="strip-item"
        data-item="ram"
        title={resources.processes !== null ? t('strip.ramTitle', { count: resources.processes }) : t('strip.notReported')}
      >
        {t('strip.ram', { value: formatBytes(resources.rssBytes) ?? dash })}
      </span>
      <Meter
        name="cpu"
        label={t('strip.cpuShort')}
        title={resources.reported && resources.cpuPermille === null ? t('strip.measuring') : t('strip.cpuTitle')}
        values={history.map((s) => s.cpu)}
        now={resources.cpuPermille}
      />
      {resources.hasGpu ? (
        <Meter
          name="gpu"
          label={t('strip.gpuShort')}
          title={
            resources.gpuOwnPermille !== null
              ? t('strip.gpuTitleOwn', { own: formatPermille(resources.gpuOwnPermille) ?? '' })
              : t('strip.gpuTitle')
          }
          values={history.map((s) => s.gpu)}
          now={resources.gpuPermille}
        />
      ) : null}
      {resources.hasNpu ? (
        <Meter name="npu" label={t('strip.npuShort')} title={t('strip.npuTitle')} values={history.map((s) => s.npu)} now={resources.npuPermille} />
      ) : null}
    </span>
  );
}
