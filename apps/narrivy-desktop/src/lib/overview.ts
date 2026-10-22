// Overview facts for Home, read only from daemon capability strings. A value
// the daemon does not report is `null`, and the UI says "not reported".
import type { StatusSnapshot } from './bridge';

function cap(status: StatusSnapshot | null, prefix: string): string | null {
  const found = status?.capabilities.find((c) => c.startsWith(prefix));
  return found ? found.slice(prefix.length) : null;
}

function micros(value: string | null): number | null {
  if (value === null || !/^\d+$/.test(value)) return null;
  return Number(value);
}

export interface LatencyFacts {
  /** The daemon reports latency at all (even "no samples yet"). */
  reported: boolean;
  sessions: number | null;
  p50Ms: number | null;
  p95Ms: number | null;
}

export function latencyFacts(status: StatusSnapshot | null): LatencyFacts {
  const sessions = cap(status, 'diagnostics.latency.sessions=');
  const p50 = micros(cap(status, 'diagnostics.latency.p50_us='));
  const p95 = micros(cap(status, 'diagnostics.latency.p95_us='));
  return {
    reported: (status?.capabilities ?? []).some((c) => c.startsWith('diagnostics.latency.')),
    sessions: sessions !== null && /^\d+$/.test(sessions) ? Number(sessions) : null,
    p50Ms: p50 === null ? null : Math.round(p50 / 1000),
    p95Ms: p95 === null ? null : Math.round(p95 / 1000),
  };
}

export function formatMs(ms: number | null): string | null {
  if (ms === null) return null;
  return ms >= 1000 ? `${(ms / 1000).toFixed(ms >= 10_000 ? 0 : 1)} s` : `${ms} ms`;
}

export interface HealthFacts {
  overall: string | null;
  subsystems: number | null;
}

export function healthFacts(status: StatusSnapshot | null): HealthFacts {
  const subsystems = cap(status, 'diagnostics.health.subsystems=');
  return {
    overall: cap(status, 'diagnostics.health.overall='),
    subsystems: subsystems !== null && /^\d+$/.test(subsystems) ? Number(subsystems) : null,
  };
}

export interface HistoryFacts {
  enabled: boolean;
  maxEntries: string | null;
  retention: string | null;
}

export function historyFacts(status: StatusSnapshot | null): HistoryFacts {
  return {
    enabled: cap(status, 'history.optional=') === 'enabled',
    maxEntries: cap(status, 'history.max_entries='),
    retention: cap(status, 'history.retention='),
  };
}

export interface EngineFacts {
  runtime: string | null;
  languages: string | null;
  streaming: boolean | null;
  maxSeconds: string | null;
}

export function engineFacts(status: StatusSnapshot | null): EngineFacts {
  const streaming = cap(status, 'asr.transcribe-cpp.streaming=');
  return {
    runtime: cap(status, 'asr.transcribe-cpp.runtime='),
    languages: cap(status, 'asr.languages='),
    streaming:
      streaming === null ? null : !(streaming.startsWith('disabled') || streaming.startsWith('unavailable')),
    maxSeconds: cap(status, 'capture.max_seconds='),
  };
}

/** The daemon reports a compute plan, so the device ladder can be shown. */
export function computeReported(status: StatusSnapshot | null): boolean {
  return cap(status, 'compute.device=') !== null;
}

export interface ResourceFacts {
  /** The daemon reports resource usage at all. */
  reported: boolean;
  rssBytes: number | null;
  processes: number | null;
  /** Whole-machine CPU share in thousandths; null while warming up. */
  cpuPermille: number | null;
  /** How busy the computer's GPU and NPU are, in thousandths; null while
   * warming up. `has…` is false when there is no such device. */
  gpuPermille: number | null;
  npuPermille: number | null;
  hasGpu: boolean;
  hasNpu: boolean;
  /** Of the GPU's use, Narrivy's own, when the daemon can tell. */
  gpuOwnPermille: number | null;
}

function integer(value: string | null): number | null {
  return value !== null && /^\d+$/.test(value) ? Number(value) : null;
}

/** Reported and not `unavailable:no_device`. */
function present(value: string | null): boolean {
  return value !== null && value !== 'unavailable:no_device';
}

export function resourceFacts(status: StatusSnapshot | null): ResourceFacts {
  return {
    reported: (status?.capabilities ?? []).some((c) => c.startsWith('diagnostics.resources.')),
    rssBytes: integer(cap(status, 'diagnostics.resources.rss_bytes=')),
    processes: integer(cap(status, 'diagnostics.resources.processes=')),
    cpuPermille: integer(cap(status, 'diagnostics.resources.cpu_permille=')),
    gpuPermille: integer(cap(status, 'diagnostics.resources.gpu_permille=')),
    npuPermille: integer(cap(status, 'diagnostics.resources.npu_permille=')),
    hasGpu: present(cap(status, 'diagnostics.resources.gpu_permille=')),
    hasNpu: present(cap(status, 'diagnostics.resources.npu_permille=')),
    gpuOwnPermille: integer(cap(status, 'diagnostics.resources.gpu_own_permille=')),
  };
}

export function formatBytes(bytes: number | null): string | null {
  if (bytes === null) return null;
  if (bytes >= 1024 ** 3) return `${(bytes / 1024 ** 3).toFixed(1)} GB`;
  return `${Math.round(bytes / 1024 ** 2)} MB`;
}

export function formatPermille(permille: number | null): string | null {
  if (permille === null) return null;
  return permille < 10 ? `${(permille / 10).toFixed(1)}%` : `${Math.round(permille / 10)}%`;
}

export type ServiceState = 'ok' | 'off' | 'warn' | 'unknown';

export interface ServiceFact {
  /** i18n key suffix under `services.` */
  id: string;
  state: ServiceState;
  /** i18n key or raw detail for the state line. */
  detail: string;
  /** Raw values interpolated into the detail. */
  values?: Record<string, string>;
}

function readiness(value: string | null): ServiceState {
  if (value === null) return 'unknown';
  if (value.startsWith('ready') || value === 'available' || value === 'enabled') return 'ok';
  if (value.startsWith('disabled')) return 'off';
  return 'warn';
}

/**
 * Every service the pipeline depends on, from daemon capabilities only.
 * The accelerator row names a backend only when the planner resolved one.
 */
export function serviceFacts(status: StatusSnapshot | null): ServiceFact[] {
  if (!status) return [{ id: 'daemon', state: 'warn', detail: 'services.detail.unreachable' }];
  const caps = status.capabilities;
  const facts: ServiceFact[] = [
    { id: 'daemon', state: 'ok', detail: 'services.detail.running', values: { build: status.daemon_build ?? '' } },
  ];
  const worker = cap(status, 'asr.transcribe-cpp.worker=');
  if (worker !== null) {
    facts.push({ id: 'engine', state: readiness(worker), detail: worker.startsWith('ready') ? 'services.detail.loaded' : 'services.detail.notReady' });
  } else if (caps.includes('mock.asr')) {
    facts.push({ id: 'engine', state: 'warn', detail: 'services.detail.testEngine' });
  } else {
    facts.push({ id: 'engine', state: 'unknown', detail: 'services.detail.notReported' });
  }
  const capture = caps.includes('mic.available') ? 'available' : cap(status, 'capture.pipewire-helper=');
  facts.push({ id: 'capture', state: readiness(capture), detail: capture === null ? 'services.detail.notReported' : readiness(capture) === 'ok' ? 'services.detail.ready' : 'services.detail.unavailable' });
  const typing = cap(status, 'delivery.wtype-helper=');
  facts.push({ id: 'typing', state: readiness(typing), detail: typing === null ? 'services.detail.notReported' : readiness(typing) === 'ok' ? 'services.detail.ready' : 'services.detail.unavailable' });
  const inbox = cap(status, 'delivery.inbox-recovery=');
  facts.push({ id: 'inbox', state: readiness(inbox), detail: inbox === 'enabled' ? 'services.detail.on' : inbox === null ? 'services.detail.notReported' : 'services.detail.off' });
  const device = cap(status, 'compute.device=');
  const backend = cap(status, 'compute.device.backend=');
  const deviceClass = cap(status, 'compute.device.class=');
  const pack = cap(status, 'compute.runtime-pack=');
  if (device === 'resolved' && backend) {
    facts.push({ id: 'accelerator', state: 'ok', detail: 'services.detail.accelerator', values: { backend, device: (deviceClass ?? '').toUpperCase() } });
  } else if (pack) {
    facts.push({ id: 'accelerator', state: 'warn', detail: 'services.detail.installPack', values: { pack } });
  } else if (device !== null) {
    facts.push({ id: 'accelerator', state: 'off', detail: 'services.detail.cpuOnly' });
  }
  const history = cap(status, 'history.optional=');
  if (history !== null) {
    facts.push({ id: 'history', state: readiness(history), detail: history === 'enabled' ? 'services.detail.on' : 'services.detail.off' });
  }
  return facts;
}

/** Overall pipeline verdict from the services the pipeline cannot work without. */
export function pipelineVerdict(facts: ServiceFact[]): 'ok' | 'degraded' | 'down' {
  const required = facts.filter((f) => ['daemon', 'engine', 'capture'].includes(f.id));
  if (required.some((f) => f.state === 'warn' && f.id === 'daemon')) return 'down';
  if (required.every((f) => f.state === 'ok') && facts.every((f) => f.state !== 'warn')) return 'ok';
  return 'degraded';
}

export type DeviceClass = 'npu' | 'dgpu' | 'igpu' | 'cpu';

export interface ActiveDevice {
  /** The device class recognition actually runs on, or null if no engine reports one. */
  running: DeviceClass | null;
  /** The device class the compute planner selected, if it resolved one. */
  planned: DeviceClass | null;
  /** The engine runtime serving recognition (for example a resident worker). */
  runtime: string | null;
  /** The engine is the built-in test engine, not a real model. */
  testEngine: boolean;
}

function deviceClass(value: string | null): DeviceClass | null {
  return value === 'npu' || value === 'dgpu' || value === 'igpu' || value === 'cpu' ? value : null;
}

/**
 * Where recognition runs, read only from engine tokens. The planner's choice
 * is reported separately: a planned accelerator is not claimed as the running
 * device until an engine reports that it executes there.
 */
export function activeDevice(status: StatusSnapshot | null): ActiveDevice {
  const caps = status?.capabilities ?? [];
  const cpuEngine = caps.includes('asr.transcribe-cpp') || cap(status, 'asr.transcribe-cpp.cpu=') === 'available';
  // A ready engine attests the device its model is loaded on. Without that
  // token (an older service) an available engine is the CPU lane.
  const attested = deviceClass(cap(status, 'asr.transcribe-cpp.device='));
  return {
    running: attested ?? (cpuEngine ? 'cpu' : null),
    planned: cap(status, 'compute.device=') === 'resolved' ? deviceClass(cap(status, 'compute.device.class=')) : null,
    runtime: cap(status, 'asr.transcribe-cpp.runtime='),
    testEngine: caps.includes('mock.asr'),
  };
}
