import { act, fireEvent, render, renderHook, screen, within } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { RecognitionSettingsCard } from '../app/components/SettingsCompute/RecognitionSettingsCard';
import { useDictationActions } from '../hooks/useDictationActions';
import type { StatusSnapshot } from '../lib/bridge';
import { buildRecognitionSettings, recognitionOptions } from '../lib/recognition';
import { DEFAULT_SETTINGS, SETTINGS_STORAGE_KEY } from '../lib/settings';

const controls = vi.hoisted(() => ({
  resolveStartId: vi.fn(),
  refresh: vi.fn(),
  start: vi.fn(),
  stop: vi.fn(),
  cancel: vi.fn(),
  report: vi.fn(),
  status: null as StatusSnapshot | null,
}));

vi.mock('../state/PipelinesContext', () => ({
  usePipelines: () => ({
    catalog: { availability: 'ready' },
    resolveStartId: controls.resolveStartId,
    refresh: controls.refresh,
  }),
}));
vi.mock('../state/SessionContext', async () => {
  const { pinnedSession } = await import('../test/sessionHookMock');
  return { useSession: () => pinnedSession('idle') };
});
vi.mock('../state/StatusContext', () => ({
  useDaemonStatus: () => ({ status: controls.status }),
}));
vi.mock('../ui/primitives/Toast', () => ({ useErrorHandler: () => controls.report }));
vi.mock('../lib/bridge', () => ({
  bridge: {
    start: controls.start,
    stop: controls.stop,
    cancel: controls.cancel,
  },
}));

const CAPABLE = [
  'asr.recognition-settings=available',
  'asr.language-detection=available',
  'asr.context-bias=available',
  'asr.languages=en,fr,zh',
  'asr.translation-targets=en,es',
  'asr.scripts=Latn,Hans',
  'asr.locales=en-US,fr-FR',
];

function snapshot(capabilities: string[], protocol_minor = 9): StatusSnapshot {
  return {
    capabilities,
    active_session: { state: 'idle', pipeline_id: null },
    protocol_minor,
  };
}

function envelope(values: Record<string, string>) {
  return { schema_version: 1 as const, values: { ...DEFAULT_SETTINGS.values, ...values } };
}

function expectUnsupported(status: StatusSnapshot | null, values: Record<string, string>) {
  let caught: unknown;
  try {
    buildRecognitionSettings(status, envelope(values));
  } catch (err) {
    caught = err;
  }
  expect(caught).toMatchObject({ code: 'unsupported_recognition_setting' });
}

describe('recognitionOptions and buildRecognitionSettings', () => {
  it('keeps spoken, written, script, translation, locale, and context fields distinct', () => {
    const status = snapshot(CAPABLE);
    expect(recognitionOptions(status)).toEqual({
      available: true,
      languageDetection: true,
      contextBias: true,
      languages: ['en', 'fr', 'zh'],
      translationTargets: ['en', 'es'],
      scripts: ['Latn', 'Hans'],
      locales: ['en-US', 'fr-FR'],
    });
    expect(
      buildRecognitionSettings(
        status,
        envelope({
          'asr.language': 'fr',
          'asr.output_language': 'en',
          'asr.output_script': 'Hans',
          'asr.translation_target': 'es',
          'asr.locale': 'fr-FR',
          'asr.context_hints': 'narrivy, dictation',
        }),
      ),
    ).toEqual({
      spoken_language: { kind: 'explicit', tag: 'fr' },
      output_language: 'en',
      output_script: 'Hans',
      translation_target: 'es',
      locale: 'fr-FR',
      context_hints: ['narrivy', 'dictation'],
    });
    expect(buildRecognitionSettings(status, envelope({ 'asr.language': 'detect' }))).toEqual({
      spoken_language: { kind: 'auto_detect' },
    });
  });

  it('renders only advertised choices and disables unsupported recognition controls', () => {
    controls.status = snapshot(CAPABLE);
    const view = render(<RecognitionSettingsCard />);
    const spoken = screen.getByRole('combobox', { name: 'Spoken language' });
    expect(spoken).toBeEnabled();
    expect(within(spoken).getByRole('option', { name: 'Auto detect' })).toBeInTheDocument();
    expect(within(spoken).getByRole('option', { name: 'fr' })).toBeInTheDocument();
    fireEvent.change(screen.getByRole('combobox', { name: 'Translation target' }), {
      target: { value: 'es' },
    });
    expect(screen.getByRole('combobox', { name: 'Written language' })).toHaveTextContent('es');

    controls.status = snapshot([]);
    view.rerender(<RecognitionSettingsCard />);
    expect(screen.getByRole('combobox', { name: 'Spoken language' })).toBeDisabled();
    expect(screen.getByText('Recognition settings are unavailable for the loaded model.')).toBeInTheDocument();
  });

  it('fails closed for unsupported stale recognition values', () => {
    const status = snapshot(CAPABLE);
    expectUnsupported(status, { 'asr.language': 'de' });
    expectUnsupported(status, { 'asr.output_language': 'ja' });
    expectUnsupported(status, { 'asr.output_script': 'Cyrl' });
    expectUnsupported(status, { 'asr.translation_target': 'de' });
    expectUnsupported(status, { 'asr.locale': 'de-DE' });
    expectUnsupported(snapshot(CAPABLE.filter((item) => item !== 'asr.language-detection=available')), {
      'asr.language': 'detect',
    });
    expectUnsupported(snapshot(CAPABLE.filter((item) => item !== 'asr.context-bias=available')), {
      'asr.context_hints': 'narrivy',
    });
    expectUnsupported(snapshot(CAPABLE, 8), { 'asr.language': 'fr' });
    expect(buildRecognitionSettings(status, DEFAULT_SETTINGS)).toBeNull();
  });
});

describe('useDictationActions recognition payload', () => {
  beforeEach(() => {
    vi.resetAllMocks();
    controls.refresh.mockResolvedValue(undefined);
    controls.start.mockResolvedValue(undefined);
    controls.cancel.mockResolvedValue(undefined);
    controls.resolveStartId.mockResolvedValue('raw');
    controls.status = snapshot(CAPABLE);
  });

  it('passes explicit saved recognition settings on Start and keeps the one-argument call for Auto', async () => {
    const { result } = renderHook(() => useDictationActions());
    await act(async () => {
      expect(await result.current.start()).toBe(true);
    });
    expect(controls.start.mock.calls).toEqual([['raw']]);

    window.localStorage.setItem(
      SETTINGS_STORAGE_KEY,
      JSON.stringify(
        envelope({
          'asr.language': 'fr',
          'asr.output_language': 'en',
          'asr.output_script': 'Hans',
          'asr.translation_target': 'es',
          'asr.locale': 'fr-FR',
          'asr.context_hints': 'narrivy',
        }),
      ),
    );
    await act(async () => {
      expect(await result.current.start()).toBe(true);
    });
    expect(controls.start.mock.calls[1]).toEqual([
      'raw',
      {
        spoken_language: { kind: 'explicit', tag: 'fr' },
        output_language: 'en',
        output_script: 'Hans',
        translation_target: 'es',
        locale: 'fr-FR',
        context_hints: ['narrivy'],
      },
    ]);
  });
});
