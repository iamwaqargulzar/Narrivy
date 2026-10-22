// Development-only preview bridge: lets `npm run dev` render the whole app in
// an ordinary browser with a believable daemon. It replays the same event
// shapes narrivyd sends (identity, sequence numbers, volatile partials, a
// terminal state, then a recovery fetch), so screens behave as they do in the
// Tauri shell. It is selected only when Vite runs in dev mode outside Tauri;
// production builds never include it.
import {
  FakeBridge,
  type HistoryEntryFull,
  type JournalEntryDto,
  type ModelInfo,
  type RecognitionSettings,
  type StoreItem,
} from './bridge';

const CAPABILITIES = [
  'capture.pipewire-helper=ready:system-owned-revalidated',
  'capture.window_seconds=300',
  'capture.max_seconds=300',
  'model.gguf=available',
  'asr.transcribe-cpp.cpu=available',
  'asr.transcribe-cpp.runtime=resident-worker',
  'asr.transcribe-cpp.worker=ready',
  'asr.transcribe-cpp.device=igpu',
  'model.selection=idle',
  'asr.transcribe-cpp.streaming=disabled:model_unsupported',
  'compute.device=unavailable:no_qualified_plan',
  'delivery.wtype-helper=ready:system-owned-revalidated',
  'delivery.inbox-recovery=enabled',
  'policy.local-only=enabled',
  'pipeline.clean=enabled',
  'pipeline.raw=enabled',
  'history.optional=enabled',
  'history.retention=7d',
  'history.max_entries=200',
  'activation.atomic-command=available',
  'activation.portal-global-shortcuts=unavailable:interface_absent',
  'diagnostics.support-report=available',
  'diagnostics.health.overall=ok',
  'diagnostics.latency.sessions=12',
  'diagnostics.latency.p50_us=412000',
  'diagnostics.latency.p95_us=880000',
  'diagnostics.resources.rss_bytes=412000000',
  'diagnostics.resources.processes=3',
  'diagnostics.resources.cpu_permille=34',
  'asr.recognition-settings=available',
  'asr.context-bias=available',
  'asr.languages=en',
];

const SAMPLE =
  'Let us move the design review to Thursday afternoon and invite the platform team as well.';

const now = Date.now();
const SEEDED: Array<[string, string, number]> = [
  ['tx-3', 'Remind me to send the invoice to Sara before Friday.', now - 6 * 60_000],
  ['tx-2', 'The new build fixes the overlay focus issue on COSMIC.', now - 52 * 60_000],
  ['tx-1', 'Draft a short reply thanking them for the quick turnaround.', now - 26 * 3_600_000],
];

export class PreviewBridge extends FakeBridge {
  private eventSeq = 0;
  private stateSeq = 0;
  private session = 0;
  private timers: number[] = [];

  constructor() {
    super({
      status: {
        capabilities: CAPABILITIES,
        daemon_build: 'narrivyd 0.3.0-dev',
        protocol_minor: 26,
        installed_model: 'ready',
      },
      pipelines: [
        { id: 'clean', name: 'Clean', version: 1, enabled: true, compute: 'auto' },
        { id: 'raw', name: 'Raw', version: 1, enabled: true, compute: 'auto' },
      ],
      entries: SEEDED.map(([txid, , at], index) => ({
        txid,
        // One seeded dictation could not be inserted and went to the Inbox.
        state: index === 1 ? 'committed' : 'delivered',
        pipeline_id: 'clean',
        has_delivered_text: index !== 1,
        updated_unix_ms: at,
        // The oldest seeded entry predates timing, to show the unmeasured case.
        ...(index < 2 ? { transcribe_us: 640_000 + index * 910_000 } : {}),
      })),
      fullText: Object.fromEntries(SEEDED.map(([txid, text]) => [txid, text])),
      checks: {
        mic: { id: 'mic', pass: true, detail: 'PipeWire capture helper is ready' },
        delivery: { id: 'delivery', pass: true, detail: 'Typing helper ready; Inbox recovery enabled' },
        shortcuts: { id: 'shortcuts', pass: false, disabled: true, detail: 'No global shortcut is configured' },
        'theme-store': { id: 'theme-store', pass: true, detail: 'Theme preference is readable and writable' },
      },
    });
    const rev = '0000000000000000000000000000000000000000';
    const model = (id: string, family: string, variant: string, quant: string, mb: number, engine: string, state: ModelInfo['state'], streaming = false, languages = ['multilingual']): ModelInfo => ({
      id, family, variant, quant, size_bytes: mb * 1_000_000, license: 'Apache-2.0',
      engine, languages, streaming, devices: engine === 'add-on' ? ['CPU', 'GPU', 'NPU'] : ['CPU', 'GPU'], source_repo: `narrivy/${id.toLowerCase()}`, source_revision: rev, qualified: id === 'sample-small-Q8_0', state,
    });
    // Placeholder names only: the preview never names a real model or engine.
    this.modelList = [
      model('sample-small-Q8_0', 'sample', 'small', 'Q8_0', 46, 'built-in', 'active', false, ['en']),
      model('sample-streaming-Q8_0', 'sample', 'streaming', 'Q8_0', 731, 'built-in', 'installed', true, ['en']),
      { ...model('sample-large-fast-Q8_0', 'sample', 'large-fast', 'Q8_0', 886, 'built-in', 'available'), downloadable: true },
      { ...model('sample-large-Q5_K_M', 'sample', 'large', 'Q5_K_M', 1161, 'built-in', 'available'), downloadable: true, downloaded_bytes: 240_000_000 },
      model('sample-large-fast-int8', 'sample', 'large-fast', 'INT8', 828, 'add-on', 'needs_engine'),
      model('sample-large-int8', 'sample', 'large', 'INT8', 1568, 'add-on', 'needs_engine'),
    ];
    // The Store: accelerators (one built in, one to add, one planned) and
    // planned modules. Speech models are on the Models screen.
    const item = (id: string, kind: StoreItem['kind'], category: string, name: string, mb: number, extra: Partial<StoreItem> = {}): StoreItem => ({
      id, kind, category, name, size_bytes: mb * 1_000_000,
      license: 'Apache-2.0', devices: ['CPU', 'GPU', 'NPU'], state: 'available', fit: 'fits', fit_reason: 'npu_found',
      source: 'published', ...extra,
    });
    const planned: Partial<StoreItem> = { size_bytes: 0, license: '', fit: 'unknown', fit_reason: 'planned', source: 'planned' };
    this.storeList = [
      item('sample-builtin', 'runtime', 'sample', 'Built-in GPU runtime', 0, { devices: ['GPU'], license: '', state: 'in_use', fit_reason: 'built_in', source: 'built_in' }),
      item('sample-runtime', 'runtime', 'sample', 'Sample accelerator runtime', 292),
      item('sample-planned-runtime', 'runtime', 'sample', 'Sample planned runtime', 0, { ...planned, devices: ['GPU'], fit: 'does_not_fit' }),
      item('sample-refine', 'module', 'refine', 'Sample refinement model', 0, planned),
      item('sample-grammar', 'module', 'refine', 'Sample grammar check', 0, { ...planned, devices: ['CPU'] }),
      item('sample-denoise', 'module', 'denoise', 'Sample noise suppression', 0, { ...planned, devices: ['CPU'] }),
    ];
  }

  private send(event: Record<string, unknown>) {
    this.eventSeq += 1;
    this.dispatch({ ...event, event_seq: this.eventSeq });
  }

  private state(state: string, pipeline: string) {
    this.stateSeq += 1;
    this.send({
      event: 'session_state_changed',
      session_id: `s-${this.session}`,
      transaction_id: `tx-live-${this.session}`,
      pipeline_id: pipeline,
      seq: this.stateSeq,
      state,
    });
    this.statusSnapshot.active_session =
      state === 'delivered' || state === 'cancelled' ? null : { state, pipeline_id: pipeline };
  }

  private later(ms: number, run: () => void) {
    this.timers.push(window.setTimeout(run, ms));
  }

  private clearTimers() {
    this.timers.forEach((id) => window.clearTimeout(id));
    this.timers = [];
  }

  override start(pipelineId?: string | null, _recognition?: RecognitionSettings | null) {
    this.clearTimers();
    this.session += 1;
    this.stateSeq = 0;
    const pipeline = pipelineId ?? 'clean';
    this.state('preparing', pipeline);
    this.later(250, () => this.state('recording', pipeline));
    const words = SAMPLE.split(' ');
    words.forEach((_, index) => {
      this.later(900 + index * 260, () =>
        this.send({
          event: 'transcript_partial',
          session_id: `s-${this.session}`,
          transaction_id: `tx-live-${this.session}`,
          pipeline_id: pipeline,
          seq: index,
          text: words.slice(0, index + 1).join(' '),
        }),
      );
    });
    return Promise.resolve();
  }

  override stop() {
    this.clearTimers();
    const pipeline = this.statusSnapshot.active_session?.pipeline_id ?? 'clean';
    const txid = `tx-live-${this.session}`;
    this.state('processing', pipeline);
    this.later(900, () => {
      const entry: JournalEntryDto = {
        txid,
        state: 'delivered',
        pipeline_id: pipeline,
        has_delivered_text: true,
        updated_unix_ms: Date.now(),
      };
      this.entries = [entry, ...this.entries.filter((e) => e.txid !== txid)];
      this.fullText[txid] = SAMPLE;
      this.state('delivered', pipeline);
    });
    return Promise.resolve();
  }

  override cancel() {
    this.clearTimers();
    this.state('cancelled', this.statusSnapshot.active_session?.pipeline_id ?? 'clean');
    return Promise.resolve();
  }

  override recoveryEntry(txid: string): Promise<HistoryEntryFull> {
    const text = this.fullText[txid] ?? null;
    return Promise.resolve({
      txid,
      state: 'delivered',
      text,
      delivered: text,
      raw: text ? text.replace(/\.$/, '').toLowerCase() : null,
      capture_end: 'user_stopped',
    });
  }
}

export function createPreviewBridge() {
  return new PreviewBridge();
}
