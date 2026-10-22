// Development-only showcase: the preview bridge with believable, invented
// content (a meeting, a researched chat, the real model line-up) for the
// README screenshots. Opened with `?showcase` on `npm run dev`; production
// builds never include it. No real person's data is in here.
import type { LanguageModel, Meeting, MeetingDetail, ModelInfo, StatusSnapshot, StoreItem } from '../lib/bridge';
import { PreviewBridge } from '../lib/previewBridge';

const MINUTE = 60_000;
const now = Date.now();

const DICTATIONS: Array<[string, string, number, number]> = [
  ['tx-7', 'Can you move our design review to Thursday afternoon and invite the platform team?', 3, 520_000],
  ['tx-6', 'The release notes are ready. Please check the install section before we publish.', 18, 610_000],
  ['tx-5', 'Remind me to send the invoice to Sara before Friday.', 41, 430_000],
  ['tx-4', 'Add a test for the empty folder case, then run the whole suite again.', 75, 700_000],
  ['tx-3', 'Thanks for the quick turnaround. The new build fixed the issue on my laptop.', 140, 480_000],
  ['tx-2', 'Let us keep the first release small: dictation, meetings and the assistant.', 26 * 60, 650_000],
];

const meeting: Meeting = {
  id: 'm-sync',
  title: 'Weekly product sync',
  started_unix_ms: now - 3 * 60 * MINUTE,
  duration_ms: 31 * MINUTE + 12_000,
  state: 'ready',
  progress: 1000,
  model: 'large-v3',
  speakers: 3,
};

const screenSession: Meeting = {
  id: 'm-talk',
  title: 'Conference talk: local AI on laptops',
  started_unix_ms: now - 27 * 60 * MINUTE,
  duration_ms: 18 * MINUTE + 40_000,
  state: 'ready',
  progress: 1000,
  model: 'turbo',
  speakers: 1,
  watch: true,
};

const turn = (speaker: string, minute: number, text: string) => ({
  speaker,
  source: 'room',
  start_ms: minute * MINUTE,
  end_ms: minute * MINUTE + 9_000,
  text,
  language: 'en',
});

const meetingDetail: MeetingDetail = {
  meeting,
  names: { 'Person 1': 'You', 'Person 2': 'Amira', 'Person 3': 'Daniel' },
  folder: '~/.local/share/narrivy/meetings/m-sync',
  models: ['large-v3', 'turbo'],
  recording: true,
  turns: [
    turn('Person 1', 0, 'Morning everyone. Three things today: the beta date, the Windows build and the onboarding feedback.'),
    turn('Person 2', 1, 'The beta is in good shape. The installer works on a clean Pop!_OS machine, and dictation is under a second on the NPU.'),
    turn('Person 3', 2, 'Windows compiles now. Capture works, typing into apps is next. I would not promise it for the beta.'),
    turn('Person 1', 3, 'Agreed. Linux ships first, Windows follows. What did people say about onboarding?'),
    turn('Person 2', 4, 'Mostly positive. Two testers missed the shortcut step, so we should make it the first thing they see.'),
    turn('Person 3', 5, 'I can take that. A short animation showing the key, and a test dictation right after.'),
    turn('Person 1', 6, 'Great. Let us aim for the beta on the twenty-eighth and review again on Monday.'),
  ],
  notes: {
    notes: [
      '## Summary',
      'The beta is on track for **28 October**. Linux ships first; Windows follows after the beta.',
      '',
      '## Decisions',
      '- Beta date: 28 October',
      '- Windows is not part of the beta',
      '',
      '## Action items',
      '- **Daniel**: make the shortcut step the first onboarding screen, with a test dictation',
      '- **Amira**: run the installer on two more clean machines',
      '- **You**: review progress on Monday',
    ].join('\n'),
  },
};

const talkDetail: MeetingDetail = {
  meeting: screenSession,
  names: {},
  folder: '~/.local/share/narrivy/meetings/m-talk',
  models: ['large-v3', 'turbo'],
  turns: [
    turn('Person 1', 0, 'Today I want to show that a laptop NPU can run speech recognition faster than real time.'),
    turn('Person 1', 2, 'The trick is to keep the model resident and compile it once for the device.'),
  ],
  slides: [],
};

const chatTurns = [
  {
    id: 'q1',
    role: 'user',
    text: 'Find the three best-reviewed quiet mechanical keyboards under $120 and compare them.',
    at: now - 9 * MINUTE,
  },
  {
    id: 'a1',
    role: 'assistant',
    state: 'done',
    at: now - 8 * MINUTE,
    tools: [
      {
        server: 'web',
        tool: 'web_search',
        arguments: JSON.stringify({ query: 'quiet mechanical keyboard under $120 review 2026' }),
        state: 'done',
        detail: JSON.stringify({
          kind: 'search',
          query: 'quiet mechanical keyboard under $120 review 2026',
          results: [
            { title: 'The best quiet mechanical keyboards', url: 'https://example.com/quiet-keyboards', snippet: 'Silent switches, foam dampening and a solid case make the biggest difference…' },
            { title: 'Budget keyboard roundup', url: 'https://example.org/budget-boards', snippet: 'Five boards under $120 tested for sound, typing feel and build…' },
          ],
        }),
      },
      {
        server: 'web',
        tool: 'open_page',
        arguments: JSON.stringify({ url: 'https://example.com/quiet-keyboards' }),
        state: 'done',
        detail: JSON.stringify({ kind: 'page', url: 'https://example.com/quiet-keyboards', title: 'The best quiet mechanical keyboards', excerpt: 'We measured each board at 30 cm with a sound meter' }),
      },
    ],
    text: [
      'Here are three well-reviewed quiet boards under $120:',
      '',
      '| Keyboard | Switches | Sound | Price |',
      '|---|---|---|---|',
      '| Board A | Silent linear | Quietest tested | $99 |',
      '| Board B | Silent tactile | Very quiet, foam-filled | $115 |',
      '| Board C | Linear, lubed | Soft "thock" | $89 |',
      '',
      '**Pick Board A** if silence matters most, Board B if you like a tactile bump.',
      '',
      'Sources: example.com, example.org',
    ].join('\n'),
  },
];

const chats = {
  'chat-keyboards': {
    id: 'chat-keyboards',
    title: 'Quiet keyboards under $120',
    updatedAt: now - 8 * MINUTE,
    tools: ['web'],
    turns: chatTurns,
  },
  'chat-sync': {
    id: 'chat-sync',
    title: 'Questions about the product sync',
    updatedAt: now - 2 * 60 * MINUTE,
    turns: [{ id: 's1', role: 'user', text: 'What did Daniel agree to do?', at: now - 2 * 60 * MINUTE }],
  },
  'chat-site': {
    id: 'chat-site',
    title: 'One-page site for the launch',
    updatedAt: now - 26 * 60 * MINUTE,
    turns: [{ id: 'w1', role: 'user', text: 'Make a one-page launch site and check it in the browser.', at: now - 26 * 60 * MINUTE }],
  },
  'chat-email': {
    id: 'chat-email',
    title: 'Reply to the hosting quote',
    updatedAt: now - 3 * 24 * 60 * MINUTE,
    turns: [{ id: 'e1', role: 'user', text: 'Draft a polite reply asking for a yearly price.', at: now - 3 * 24 * 60 * MINUTE }],
  },
};

const rev = '7c1e4a9d02b35f6e81c47a0d9b2e3f5a6c8d1e04';
const model = (
  id: string,
  family: string,
  variant: string,
  quant: string,
  mb: number,
  engine: string,
  state: ModelInfo['state'],
  extra: Partial<ModelInfo> = {},
): ModelInfo => ({
  id, family, variant, quant, size_bytes: mb * 1_000_000, license: 'MIT', engine, languages: ['multilingual'],
  streaming: false, devices: engine === 'add-on' ? ['CPU', 'GPU', 'NPU'] : ['CPU', 'GPU'],
  source_repo: `narrivy/${id}`, source_revision: rev, qualified: false, state, ...extra,
});

const MODELS: ModelInfo[] = [
  model('whisper-large-v3-turbo-int8', 'whisper', 'large-v3-turbo', 'INT8', 828, 'add-on', 'active'),
  model('whisper-large-v3-int8', 'whisper', 'large-v3', 'INT8', 1568, 'add-on', 'installed'),
  model('whisper-base-en-q8_0', 'whisper', 'base.en', 'Q8_0', 82, 'built-in', 'installed', { languages: ['en'] }),
  { ...model('whisper-large-v3-turbo-q8_0', 'whisper', 'large-v3-turbo', 'Q8_0', 874, 'built-in', 'available'), downloadable: true },
  { ...model('moonshine-base-en', 'moonshine', 'base', 'FP16', 120, 'built-in', 'available', { streaming: true, languages: ['en'] }), downloadable: true, downloaded_bytes: 46_000_000 },
];

const LANGUAGE_MODELS: LanguageModel[] = [
  {
    id: 's1-mini', name: 'S1-mini (clean-up)', publisher: 'Superwhisper', size_bytes: 623_000_000,
    license: 'Apache-2.0 with a naming term', roles: ['clean_up'], languages: ['en'],
    recommended_for: ['clean_up'], in_use_for: ['clean_up'], clean_up_state: 'ready:NPU',
  },
  {
    id: 'qwen3.5-4b', name: 'Qwen3.5 4B', publisher: 'Qwen', size_bytes: 2_500_000_000,
    license: 'Apache-2.0', roles: ['assistant', 'clean_up'], languages: ['multilingual'],
    recommended_for: ['assistant'], in_use_for: ['assistant'], assistant_state: 'loaded:GPU',
  },
];

const item = (id: string, kind: StoreItem['kind'], category: string, name: string, mb: number, extra: Partial<StoreItem> = {}): StoreItem => ({
  id, kind, category, name, size_bytes: mb * 1_000_000, license: 'Apache-2.0', devices: ['CPU', 'GPU', 'NPU'],
  state: 'available', fit: 'fits', fit_reason: 'npu_found', source: 'published', ...extra,
});

const STORE: StoreItem[] = [
  item('vulkan', 'runtime', 'vulkan', 'Vulkan (built in)', 0, { devices: ['GPU'], license: '', state: 'in_use', fit_reason: 'built_in', source: 'built_in' }),
  item('openvino-runtime-2026.4', 'runtime', 'openvino', 'Intel OpenVINO runtime', 292, { state: 'in_use' }),
  item('browser-chromium', 'module', 'agent_browser', "Assistant's browser (Chromium)", 272, { license: 'BSD-3-Clause', devices: ['CPU'], fit: 'fits', fit_reason: 'any_computer', state: 'installed' }),
  item('cuda', 'runtime', 'cuda', 'CUDA', 0, { devices: ['GPU'], fit: 'does_not_fit', fit_reason: 'planned', source: 'planned', license: '' }),
  item('denoise', 'module', 'denoise', 'Strong noise suppression', 0, { devices: ['CPU'], fit: 'unknown', fit_reason: 'planned', source: 'planned', license: '' }),
];

export class ShowcaseBridge extends PreviewBridge {
  private polls = 0;

  constructor() {
    super();
    this.entries = DICTATIONS.map(([txid, , minutes, us]) => ({
      txid,
      state: 'delivered',
      pipeline_id: 'clean',
      has_delivered_text: true,
      updated_unix_ms: now - minutes * MINUTE,
      transcribe_us: us,
    }));
    this.fullText = Object.fromEntries(DICTATIONS.map(([txid, text]) => [txid, text]));
    this.modelList = MODELS;
    this.storeList = STORE;
    this.languageModelList = LANGUAGE_MODELS;
    this.meetingList = [meeting, screenSession];
    this.meetingDetails = { [meeting.id]: meetingDetail, [screenSession.id]: talkDetail };
    this.library.chat = chats as unknown as Record<string, Record<string, unknown>>;
  }

  // Usage that moves a little, so the status bar's graphs have a shape.
  override status(): Promise<StatusSnapshot> {
    this.polls += 1;
    const wave = (base: number, swing: number, phase: number) =>
      Math.max(0, Math.round(base + swing * Math.sin(this.polls / 3 + phase)));
    this.statusSnapshot.capabilities = [
      ...this.statusSnapshot.capabilities.filter((c) => !/_permille=/.test(c)),
      `diagnostics.resources.cpu_permille=${wave(60, 40, 0)}`,
      `diagnostics.resources.gpu_permille=${wave(120, 70, 1)}`,
      `diagnostics.resources.gpu_own_permille=${wave(30, 25, 1)}`,
      `diagnostics.resources.npu_permille=${wave(90, 90, 2)}`,
    ];
    return super.status();
  }
}

/** Ten minutes of usage before the window opened: a dictation, a reply. */
export function usageHistory() {
  const samples = [];
  for (let i = 200; i > 0; i -= 1) {
    const t = 200 - i;
    const burst = (centre: number, width: number) => Math.exp(-(((t - centre) / width) ** 2));
    samples.push({
      at: Date.now() - i * 3000,
      cpu: Math.round(40 + 25 * Math.sin(t / 7) ** 2 + 220 * burst(60, 6) + 140 * burst(150, 10)),
      gpu: Math.round(90 + 30 * Math.sin(t / 11) ** 2 + 520 * burst(150, 12)),
      npu: Math.round(5 + 640 * burst(60, 4) + 380 * burst(110, 3)),
    });
  }
  return samples;
}

export function createShowcaseBridge() {
  return new ShowcaseBridge();
}
