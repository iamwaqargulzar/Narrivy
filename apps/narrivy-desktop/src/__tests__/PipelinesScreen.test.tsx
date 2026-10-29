// The Pipelines screen: rename, reorder, stage panels that state where each
// option stands, and shortcut commands the CLI actually accepts.
import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import App from '../App';
import { bridge } from '../lib/bridge';
import { moveInOrder, orderPipelines, shortcutCommand } from '../lib/pipelinePrefs';

vi.mock('../lib/bridge', async () => {
  const { createMockBridgeModule } = await import('../test/mockBridge');
  return createMockBridgeModule();
});

async function openPipelines(
  pipelines: Array<Record<string, unknown>> = [
    { id: 'clean', name: 'Clean', version: 1, enabled: true },
    { id: 'raw', name: 'Raw', version: 1, enabled: true },
  ],
) {
  window.localStorage.setItem('narrivy.onboarded.v1', '1');
  vi.mocked(bridge.listPipelines).mockResolvedValue(pipelines as never);
  render(<App />);
  fireEvent.click(await screen.findByRole('button', { name: 'Pipelines' }));
  await screen.findByRole('heading', { level: 1, name: 'Pipelines' });
}

async function waitForTwo(names: () => Array<string | null>): Promise<Array<string | null>> {
  await screen.findAllByRole('heading', { level: 3, name: /Raw|Clean/ });
  await waitFor(() => expect(names().length).toBe(2));
  return names();
}

function cards(): HTMLElement[] {
  return [...document.querySelectorAll<HTMLElement>('.pipe-card')];
}

describe('pipelinePrefs', () => {
  it('orders, moves and builds only flags the CLI accepts', () => {
    expect(orderPipelines([{ id: 'a' }, { id: 'b' }, { id: 'c' }], ['c', 'a']).map((p) => p.id)).toEqual(['c', 'a', 'b']);
    expect(moveInOrder(['a', 'b', 'c'], 'c', 0)).toEqual(['c', 'a', 'b']);
    expect(shortcutCommand('clean')).toBe('/usr/bin/narrivy-cli toggle --pipeline clean');
    expect(shortcutCommand('raw', 'ur', 'en')).toBe('/usr/bin/narrivy-cli toggle --pipeline raw --language ur --output-language en');
    // A malformed tag is never pasted into a command.
    expect(shortcutCommand('raw', 'en; rm -rf ~', 'auto')).toBe('/usr/bin/narrivy-cli toggle --pipeline raw');
  });
});

describe('PipelinesScreen', () => {
  it('renames a pipeline and restores the original name when cleared', async () => {
    await openPipelines();
    const first = cards()[0];
    const original = within(first).getByRole('heading', { level: 3 }).textContent ?? '';
    fireEvent.click(within(first).getByRole('button', { name: original }));
    const input = within(first).getByRole('textbox', { name: `Rename ${original}` });
    fireEvent.change(input, { target: { value: 'Emails' } });
    fireEvent.keyDown(input, { key: 'Enter' });
    expect(within(cards()[0]).getByRole('heading', { level: 3, name: 'Emails' })).toBeInTheDocument();
    expect(JSON.parse(window.localStorage.getItem('narrivy.pipelines.prefs.v1') ?? '{}').names).toBeTruthy();

    fireEvent.click(within(cards()[0]).getByRole('button', { name: 'Emails' }));
    const again = within(cards()[0]).getByRole('textbox', { name: 'Rename Emails' });
    fireEvent.change(again, { target: { value: '' } });
    fireEvent.keyDown(again, { key: 'Enter' });
    expect(within(cards()[0]).getByRole('heading', { level: 3, name: original })).toBeInTheDocument();
  });

  it('reorders with the keyboard buttons and keeps the order', async () => {
    await openPipelines();
    const names = () => cards().map((card) => within(card).getByRole('heading', { level: 3 }).textContent);
    const before = await waitForTwo(names);
    fireEvent.click(screen.getByRole('button', { name: `Move ${before[1]} up` }));
    expect(names()).toEqual([before[1], before[0], ...before.slice(2)]);
  });

  it('expands a stage and marks unconnected options as coming, never as working', async () => {
    await openPipelines();
    const first = cards()[0];
    fireEvent.click(within(first).getByRole('tab', { name: /^Voice/ }));
    const panel = within(first).getByRole('tabpanel');
    // The microphone's live options sit in this stage, noise level included.
    expect(await within(panel).findByRole('switch', { name: "Ignore this computer's sound" })).toBeInTheDocument();
    const noise = await within(panel).findByRole('radiogroup', { name: 'Noise suppression' });
    expect(noise).not.toHaveAttribute('aria-disabled');

    fireEvent.click(within(first).getByRole('tab', { name: /Deliver/ }));
    const method = await within(within(first).getByRole('tabpanel')).findByRole('radiogroup', { name: 'Method' });
    expect(method).not.toHaveAttribute('aria-disabled');

    // An option not connected yet says so, and is never shown as working.
    fireEvent.click(within(first).getByRole('tab', { name: /Refine/ }));
    expect(within(first).getAllByText(/Coming · T2\.20/).length).toBeGreaterThan(0);
  });

  it('gives each pipeline its own model and device, chosen from the downloaded models', async () => {
    const model = (id: string, state: string, extra: Record<string, unknown> = {}) => ({
      id,
      family: 'sample',
      variant: id,
      quant: 'Q8_0',
      size_bytes: 886_000_000,
      license: 'Apache-2.0',
      engine: 'built-in',
      languages: ['en'],
      streaming: false,
      source_repo: `narrivy/${id}`,
      source_revision: '0000000000000000000000000000000000000000',
      qualified: false,
      state,
      ...extra,
    });
    vi.mocked(bridge.status).mockResolvedValue({
      capabilities: ['asr.transcribe-cpp.cpu=available', 'asr.transcribe-cpp.device=igpu', 'model.selection=idle'],
      active_session: { state: 'idle', pipeline_id: null },
      daemon_build: 'narrivyd test',
      protocol_minor: 15,
      installed_model: 'ready',
    });
    vi.mocked(bridge.models).mockResolvedValue([
      model('small', 'active'),
      model('streaming', 'installed', { streaming: true, quant: 'Q5_K_M' }),
      model('published', 'available'),
      model('pack', 'needs_engine'),
    ] as never);
    // Clean chose nothing (it uses the loaded model); Raw chose its own.
    await openPipelines([
      { id: 'clean', name: 'Clean', version: 1, enabled: true, compute: 'auto' },
      { id: 'raw', name: 'Raw', version: 1, enabled: true, model_id: 'streaming', compute: 'cpu' },
    ]);
    const card = (id: string) => cards().find((c) => c.getAttribute('data-pipeline') === id) ?? cards()[id === 'clean' ? 0 : 1];

    const clean = card('clean');
    fireEvent.click(within(clean).getByRole('tab', { name: /Transcribe/ }));
    const panel = within(clean).getByRole('tabpanel');
    // Only models on this computer can be chosen.
    const chooser = await within(panel).findByRole('radiogroup', { name: 'Model' });
    const choices = within(chooser).getAllByRole('radio');
    expect(choices.map((choice) => choice.getAttribute('aria-label'))).toEqual(['Sample small', 'Sample streaming']);
    expect(choices[0]).toHaveAttribute('aria-checked', 'true');
    expect(within(choices[0]).getByText('Loaded')).toBeInTheDocument();
    expect(within(panel).getByText('Q8_0 · 8-bit')).toBeInTheDocument();
    expect(within(panel).getByText('Running on the integrated GPU now.')).toBeInTheDocument();

    // Choosing a model sets it for this pipeline only, keeping its device.
    fireEvent.click(choices[1]);
    expect(bridge.setPipelineEngine).toHaveBeenLastCalledWith('clean', 'streaming', 'auto');
    expect(await within(chooser).findByText('Loading…')).toBeInTheDocument();
    expect(within(panel).getByRole('button', { name: /Download more models/ })).toBeInTheDocument();

    // The device is a working choice. This model cannot run on the NPU, so
    // the NPU is not a choice; it points to the Store instead.
    const devices = within(panel).getByRole('radiogroup', { name: 'Device' });
    expect(within(devices).getByRole('radio', { name: 'Automatic' })).toHaveAttribute('aria-checked', 'true');
    expect(within(devices).queryByRole('radio', { name: /NPU/ })).toBeNull();
    expect(within(devices).getByRole('button', { name: /NPU/ })).toBeInTheDocument();
  });

  it('shows another pipeline its own model, which loads when that pipeline starts', async () => {
    const model = (id: string, state: string) => ({
      id,
      family: 'sample',
      variant: id,
      quant: 'Q8_0',
      size_bytes: 46_000_000,
      license: 'Apache-2.0',
      engine: 'built-in',
      languages: ['en'],
      streaming: id === 'streaming',
      source_repo: `narrivy/${id}`,
      source_revision: '0000000000000000000000000000000000000000',
      qualified: false,
      state,
    });
    vi.mocked(bridge.status).mockResolvedValue({
      capabilities: ['asr.transcribe-cpp.cpu=available', 'asr.transcribe-cpp.device=igpu', 'model.selection=idle'],
      active_session: { state: 'idle', pipeline_id: null },
      daemon_build: 'narrivyd test',
      protocol_minor: 15,
      installed_model: 'ready',
    });
    vi.mocked(bridge.models).mockResolvedValue([model('small', 'active'), model('streaming', 'installed')] as never);
    await openPipelines([
      { id: 'clean', name: 'Clean', version: 1, enabled: true, compute: 'auto' },
      { id: 'raw', name: 'Raw', version: 1, enabled: true, model_id: 'streaming', compute: 'cpu' },
    ]);
    const raw = cards().find((c) => within(c).queryByRole('heading', { level: 3, name: 'Raw' })) as HTMLElement;
    fireEvent.click(within(raw).getByRole('tab', { name: /Transcribe/ }));
    const panel = within(raw).getByRole('tabpanel');
    const chooser = await within(panel).findByRole('radiogroup', { name: 'Model' });
    // Raw's own model is selected although another model is loaded now.
    expect(within(chooser).getByRole('radio', { name: 'Sample streaming' })).toHaveAttribute('aria-checked', 'true');
    expect(within(within(chooser).getByRole('radio', { name: 'Sample small' })).getByText('Loaded')).toBeInTheDocument();
    expect(within(panel).getByText('Loads when this pipeline starts')).toBeInTheDocument();

    const devices = within(panel).getByRole('radiogroup', { name: 'Device' });
    expect(within(devices).getByRole('radio', { name: 'CPU' })).toHaveAttribute('aria-checked', 'true');
    fireEvent.click(within(devices).getByRole('radio', { name: 'GPU' }));
    expect(bridge.setPipelineEngine).toHaveBeenLastCalledWith('raw', 'streaming', 'gpu');
  });

  it('offers the NPU only for a model the service lists as able to run there', async () => {
    const model = (id: string, state: string, devices: string[]) => ({
      id,
      family: 'sample',
      variant: id,
      quant: 'INT8',
      size_bytes: 46_000_000,
      license: 'Apache-2.0',
      engine: 'add-on',
      languages: ['en'],
      streaming: false,
      devices,
      source_repo: `narrivy/${id}`,
      source_revision: '0000000000000000000000000000000000000000',
      qualified: false,
      state,
    });
    vi.mocked(bridge.status).mockResolvedValue({
      capabilities: ['asr.transcribe-cpp.cpu=available', 'asr.transcribe-cpp.device=npu', 'model.selection=idle'],
      active_session: { state: 'idle', pipeline_id: null },
      daemon_build: 'narrivyd test',
      protocol_minor: 17,
      installed_model: 'ready',
    });
    vi.mocked(bridge.models).mockResolvedValue([model('npu-model', 'active', ['CPU', 'GPU', 'NPU'])] as never);
    vi.mocked(bridge.setPipelineEngine).mockClear();
    await openPipelines([{ id: 'raw', name: 'Raw', version: 1, enabled: true, model_id: 'npu-model', compute: 'auto' }]);
    const raw = cards()[0];
    fireEvent.click(within(raw).getByRole('tab', { name: /Transcribe/ }));
    const panel = within(raw).getByRole('tabpanel');
    const devices = await within(panel).findByRole('radiogroup', { name: 'Device' });
    fireEvent.click(within(devices).getByRole('radio', { name: 'NPU' }));
    await waitFor(() => expect(bridge.setPipelineEngine).toHaveBeenLastCalledWith('raw', 'npu-model', 'npu'));
    expect(within(panel).getByText('Running on the NPU now.')).toBeInTheDocument();
  });

  it('imports and exports pipelines from buttons beside New pipeline, asking for the file', async () => {
    await openPipelines();
    fireEvent.click(screen.getByRole('button', { name: 'Export' }));
    await waitFor(() => expect(bridge.pipelinesExport).toHaveBeenCalledWith('/home/you/pipelines.json'));
    expect(bridge.pickFile).toHaveBeenCalledWith('Export pipelines', true, 'narrivy-pipelines-export.json');
    fireEvent.click(screen.getByRole('button', { name: 'Import' }));
    await waitFor(() => expect(bridge.pipelinesImport).toHaveBeenCalledWith('/home/you/shared.json', false));
    expect(screen.queryByLabelText('Pipeline file')).toBeNull();
  });

  it('puts each pipeline\'s shortcut in its title row, without the manual command, when Narrivy adds shortcuts', async () => {
    await openPipelines();
    for (const card of cards()) {
      expect(within(card.querySelector('.pipe-head') as HTMLElement).getByRole('button', { name: 'Add shortcut' })).toBeInTheDocument();
      expect(within(card).queryByText(/narrivy-cli toggle/)).toBeNull();
    }
  });
});

describe('Shortcut capture', () => {
  function serviceAt(minor: number) {
    vi.mocked(bridge.status).mockResolvedValue({
      capabilities: [],
      active_session: { state: 'idle', pipeline_id: null },
      daemon_build: 'narrivyd test',
      protocol_minor: minor,
      installed_model: 'ready',
    });
  }

  async function pressInDialog(init: KeyboardEventInit) {
    const dialog = await screen.findByRole('dialog', { name: /^Shortcut for / });
    fireEvent.keyDown(dialog, init);
    return dialog;
  }

  it('turns a key press into a combination in the service\'s key names', async () => {
    const { pressedKeys, comboLabel } = await import('../lib/shortcutKeys');
    expect(pressedKeys({ code: 'KeyD', ctrlKey: true, altKey: true, shiftKey: false, metaKey: false })).toEqual({
      modifiers: ['Ctrl', 'Alt'],
      key: 'd',
    });
    expect(pressedKeys({ code: 'ControlLeft', ctrlKey: true, altKey: false, shiftKey: false, metaKey: false }).key).toBeNull();
    expect(comboLabel({ modifiers: ['Shift', 'Super'], key: 'Prior' })).toBe('Super + Shift + Page Up');
  });

  it('captures keys and binds them to the pipeline', async () => {
    serviceAt(11);
    await openPipelines();
    const card = (await screen.findAllByRole('button', { name: 'Add shortcut' }))[0];
    fireEvent.click(card);
    const dialog = await pressInDialog({ code: 'KeyD', key: 'd', ctrlKey: true, altKey: true });
    expect(within(dialog).getByText('Ctrl')).toBeInTheDocument();
    expect(within(dialog).getByText('D')).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole('button', { name: 'Save shortcut' }));
    await waitFor(() =>
      expect(bridge.setShortcut).toHaveBeenCalledWith(
        expect.objectContaining({ modifiers: ['Ctrl', 'Alt'], key: 'd', pipeline_id: expect.stringMatching(/^(clean|raw)$/) }),
        false,
      ),
    );
    await waitFor(() => expect(screen.queryByRole('dialog', { name: /^Shortcut for / })).toBeNull());
  });

  it('captures a single key, or modifiers alone when they are released', async () => {
    serviceAt(11);
    vi.mocked(bridge.setShortcut).mockReset();
    vi.mocked(bridge.setShortcut).mockResolvedValue({ outcome: 'created' });
    await openPipelines();
    fireEvent.click((await screen.findAllByRole('button', { name: 'Add shortcut' }))[0]);
    let dialog = await pressInDialog({ code: 'F13', key: 'F13' });
    expect(within(dialog).getByText('F13')).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole('button', { name: 'Save shortcut' }));
    await waitFor(() =>
      expect(bridge.setShortcut).toHaveBeenLastCalledWith(expect.objectContaining({ modifiers: [], key: 'F13' }), false),
    );

    fireEvent.click((await screen.findAllByRole('button', { name: 'Add shortcut' }))[0]);
    dialog = await pressInDialog({ code: 'ControlRight', key: 'Control', ctrlKey: true });
    fireEvent.keyDown(dialog, { code: 'AltRight', key: 'Alt', ctrlKey: true, altKey: true });
    fireEvent.keyUp(dialog, { code: 'AltRight', key: 'Alt', ctrlKey: true });
    fireEvent.keyUp(dialog, { code: 'ControlRight', key: 'Control' });
    expect(within(dialog).getByText('Ctrl')).toBeInTheDocument();
    expect(within(dialog).getByText('Alt')).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole('button', { name: 'Save shortcut' }));
    await waitFor(() =>
      expect(bridge.setShortcut).toHaveBeenLastCalledWith(expect.objectContaining({ modifiers: ['Ctrl', 'Alt'], key: '' }), false),
    );
  });

  it('explains keys in use and replaces only after the user says so', async () => {
    serviceAt(11);
    vi.mocked(bridge.setShortcut).mockReset();
    vi.mocked(bridge.setShortcut)
      .mockResolvedValueOnce({ outcome: 'conflict', conflict_kind: 'shortcut', conflict_name: 'other-app' })
      .mockResolvedValueOnce({ outcome: 'replaced' });
    await openPipelines();
    fireEvent.click((await screen.findAllByRole('button', { name: 'Add shortcut' }))[0]);
    const dialog = await pressInDialog({ code: 'Space', key: ' ', ctrlKey: true });
    fireEvent.click(within(dialog).getByRole('button', { name: 'Save shortcut' }));
    expect(await within(dialog).findByText(/These keys already run other-app/)).toBeInTheDocument();
    expect(bridge.setShortcut).toHaveBeenCalledTimes(1);
    // Both choices are offered; nothing is replaced until the user picks it.
    expect(within(dialog).getByRole('button', { name: 'Try other keys' })).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole('button', { name: 'Replace it' }));
    await waitFor(() => expect(bridge.setShortcut).toHaveBeenLastCalledWith(expect.anything(), true));
  });

  it('lists existing shortcuts as key chips and removes one on request', async () => {
    serviceAt(11);
    vi.mocked(bridge.shortcuts).mockResolvedValue({
      support: 'managed',
      manual_command: '/usr/bin/narrivy-cli toggle',
      bindings: [{ modifiers: ['Super'], key: 'F9', pipeline_id: 'clean' }],
    });
    await openPipelines();
    const remove = await screen.findByRole('button', { name: 'Remove shortcut Super + F9' });
    // On the right of the pipeline's title row, with a button for another.
    const head = remove.closest('.pipe-head') as HTMLElement;
    expect(head).not.toBeNull();
    expect(within(head).getByRole('button', { name: 'Add another shortcut' })).toBeInTheDocument();
    fireEvent.click(remove);
    await waitFor(() => expect(bridge.removeShortcut).toHaveBeenCalledWith(['Super'], 'F9'));
  });

  it('offers only the command when the service cannot manage shortcuts', async () => {
    serviceAt(11);
    vi.mocked(bridge.shortcuts).mockResolvedValue({
      support: 'manual:desktop_not_supported',
      manual_command: '/usr/bin/narrivy-cli toggle',
      bindings: [],
    });
    await openPipelines();
    await screen.findAllByText(/Narrivy cannot add shortcuts on this desktop yet/);
    expect(screen.queryByRole('button', { name: 'Add shortcut' })).toBeNull();
  });

  it('does not offer capture against a service older than protocol minor 11', async () => {
    serviceAt(10);
    vi.mocked(bridge.shortcuts).mockClear();
    await openPipelines();
    await screen.findAllByText(/^\/usr\/bin\/narrivy-cli toggle --pipeline \S+$/);
    expect(screen.queryByRole('button', { name: 'Add shortcut' })).toBeNull();
    expect(bridge.shortcuts).not.toHaveBeenCalled();
  });
});

describe('Refine stage', () => {
  it('lists the rules, the live clean-up model and the scheduled level, with no Refine screen of its own', async () => {
    vi.mocked(bridge.languageModels).mockResolvedValue([
      {
        id: 's1-mini',
        name: 'S1-mini',
        publisher: 'sample',
        size_bytes: 600_000_000,
        license: 'Apache-2.0',
        roles: ['clean_up'],
        languages: ['en'],
        recommended_for: ['clean_up'],
        in_use_for: ['clean_up'],
        clean_up_state: 'ready:npu',
      },
    ]);
    await openPipelines();
    // Refine left the side menu (D38).
    expect(screen.queryByRole('button', { name: 'Refine' })).toBeNull();
    const first = cards()[0];
    fireEvent.click(within(first).getByRole('tab', { name: /Refine/ }));
    const panel = within(within(first).getByRole('tabpanel'));
    for (const rule of ['Tidy spacing', 'Space after punctuation', 'Capitals', 'Closing punctuation']) {
      expect(panel.getByText(rule)).toBeInTheDocument();
    }
    // The clean-up model is read from the service, never claimed absent.
    expect(await panel.findByText(/^S1-mini · .*NPU/)).toBeInTheDocument();
    expect(panel.queryByText('None installed')).toBeNull();
    // The level is scheduled: its choices are not selectable.
    const level = panel.getByText('Faithful cleanup').closest('[role="radiogroup"]');
    expect(level).toHaveAttribute('aria-disabled', 'true');
    expect(panel.getAllByText('Coming · T2.20').length).toBeGreaterThan(0);
  });
});

describe('ModelsScreen', () => {
  const model = (id: string, state: string, extra: Record<string, unknown> = {}) => ({
    id,
    family: 'whisper',
    variant: id,
    quant: 'Q8_0',
    size_bytes: 886_000_000,
    license: 'Apache-2.0',
    engine: 'transcribe.cpp',
    languages: ['multilingual'],
    streaming: false,
    source_repo: `narrivy/${id}`,
    source_revision: '4b27e85dc1da4758c83a7631c69ff4a3cfa8fb0e',
    qualified: false,
    state,
    ...extra,
  });

  async function openModels(minor: number) {
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    vi.mocked(bridge.status).mockResolvedValue({
      capabilities: ['asr.transcribe-cpp.cpu=available'],
      active_session: { state: 'idle', pipeline_id: null },
      daemon_build: 'narrivyd test',
      protocol_minor: minor,
      installed_model: 'ready',
    });
    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Models' }));
    await screen.findByRole('heading', { level: 1, name: 'Models' });
  }

  it('groups the daemon\'s models by state and states each quantization', async () => {
    vi.mocked(bridge.models).mockResolvedValue([
      model('loaded', 'active', { qualified: true }),
      model('here', 'installed', { quant: 'Q5_K_M', streaming: true }),
      model('published', 'available', { downloadable: true }),
      model('bundled', 'available'),
      model('ov-pack', 'needs_engine', { quant: 'INT8', engine: 'openvino' }),
    ] as never);
    await openModels(13);

    const loaded = await screen.findByTestId('model-loaded');
    expect(within(loaded).getByText('In use')).toBeInTheDocument();
    expect(within(loaded).getByText('Qualified')).toBeInTheDocument();
    expect(within(loaded).queryByRole('button')).toBeNull();

    const here = screen.getByTestId('model-here');
    expect(within(here).getByText('Q5_K_M')).toBeInTheDocument();
    expect(within(here).getByText('Live text while speaking')).toBeInTheDocument();
    // This page is for downloading: a model is chosen in a pipeline.
    expect(within(here).queryByRole('button')).toBeNull();
    const download = within(screen.getByTestId('model-published')).getByRole('button', { name: 'Download' });
    expect(download).toBeEnabled();
    fireEvent.click(download);
    expect(bridge.downloadModel).toHaveBeenCalledWith('published');
    // A model with no published download is listed apart and says so.
    expect(within(screen.getByTestId('model-bundled')).getByText('Not available yet')).toBeInTheDocument();
    expect(within(screen.getByTestId('model-bundled')).queryByRole('button')).toBeNull();

    const pack = screen.getByTestId('model-ov-pack');
    expect(within(pack).getByText('INT8')).toBeInTheDocument();
    // A model that needs an add-on runtime leads to the Store.
    expect(within(pack).getByRole('button', { name: /Get the add-on/ })).toBeInTheDocument();
    expect(screen.getByText('Need an add-on')).toBeInTheDocument();
  });

  it('shows download progress, stops a download, and says why one did not finish', async () => {
    vi.mocked(bridge.models).mockResolvedValue([
      model('loaded', 'active'),
      model('coming', 'downloading', { downloadable: true, size_bytes: 1_000_000_000, downloaded_bytes: 250_000_000 }),
      model('half', 'available', { downloadable: true, size_bytes: 1_000_000_000, downloaded_bytes: 500_000_000 }),
    ] as never);
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    vi.mocked(bridge.status).mockResolvedValue({
      capabilities: ['asr.transcribe-cpp.cpu=available', 'model.download=failed:integrity_mismatch'],
      active_session: { state: 'idle', pipeline_id: null },
      daemon_build: 'narrivyd test',
      protocol_minor: 16,
      installed_model: 'ready',
    });
    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Models' }));

    const coming = await screen.findByTestId('model-coming');
    expect(within(coming).getByRole('progressbar')).toHaveAttribute('aria-valuenow', '25');
    fireEvent.click(within(coming).getByRole('button', { name: 'Stop download' }));
    expect(bridge.cancelModelDownload).toHaveBeenCalledWith('coming');

    // An unfinished download resumes; another cannot start while one runs.
    const half = screen.getByTestId('model-half');
    expect(within(half).getByRole('progressbar')).toHaveAttribute('aria-valuenow', '50');
    expect(within(half).getByRole('button', { name: 'Resume download' })).toBeDisabled();

    expect(screen.getByRole('alert')).toHaveTextContent('did not match its checksum');
  });

  it('falls back to the single reported model against an older service', async () => {
    vi.mocked(bridge.models).mockClear();
    await openModels(12);
    expect(await screen.findByText('Local speech model')).toBeInTheDocument();
    expect(bridge.models).not.toHaveBeenCalled();
  });
});

describe('Model filters', () => {
  const base = {
    quant: 'Q8_0', license: 'Apache-2.0', engine: 'e', source_repo: 'r', source_revision: 'x', qualified: false, state: 'available' as const,
  };
  const models = [
    { ...base, id: 'a', family: 'alpha', variant: 'small', size_bytes: 50, languages: ['en'], streaming: false, devices: ['CPU', 'GPU'], qualified: true },
    { ...base, id: 'b', family: 'beta', variant: 'live', size_bytes: 700, languages: ['en'], streaming: true, devices: ['CPU', 'GPU'] },
    { ...base, id: 'c', family: 'gamma', variant: 'large', size_bytes: 1500, languages: ['multilingual'], streaming: false, devices: ['CPU', 'GPU', 'NPU'] },
  ];
  const none = { query: '', language: 'all', device: 'all', text: 'all', sort: 'recommended' } as const;

  it('filters by language, device and live text, and sorts', async () => {
    const { filterModels } = await import('../app/screens/Models');
    const ids = (filters: object) => filterModels(models, { ...none, ...filters } as never).map((m) => m.id);
    expect(ids({})).toEqual(['a', 'b', 'c']);
    expect(ids({ language: 'english' })).toEqual(['a', 'b']);
    expect(ids({ language: 'multilingual' })).toEqual(['c']);
    expect(ids({ device: 'NPU' })).toEqual(['c']);
    expect(ids({ text: 'live' })).toEqual(['b']);
    expect(ids({ text: 'final' })).toEqual(['a', 'c']);
    expect(ids({ query: 'gam' })).toEqual(['c']);
    expect(ids({ sort: 'largest' })).toEqual(['c', 'b', 'a']);
    expect(ids({ sort: 'smallest' })).toEqual(['a', 'b', 'c']);
    expect(ids({ language: 'english', text: 'live', device: 'NPU' })).toEqual([]);
  });

  it('filters the page from the toolbar and says when nothing matches', async () => {
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    vi.mocked(bridge.status).mockResolvedValue({
      capabilities: [], active_session: { state: 'idle', pipeline_id: null }, daemon_build: 't', protocol_minor: 13, installed_model: 'ready',
    });
    vi.mocked(bridge.models).mockResolvedValue(models as never);
    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Models' }));
    await screen.findByTestId('model-a');
    expect(screen.getByText('Showing 3 of 3')).toBeInTheDocument();

    fireEvent.change(screen.getByRole('combobox', { name: 'Text' }), { target: { value: 'live' } });
    expect(screen.queryByTestId('model-a')).toBeNull();
    expect(screen.getByTestId('model-b')).toBeInTheDocument();
    expect(screen.getByText('Showing 1 of 3')).toBeInTheDocument();

    fireEvent.change(screen.getByRole('combobox', { name: 'Runs on' }), { target: { value: 'NPU' } });
    expect(screen.getByText('No model matches')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Clear filters' }));
    expect(screen.getByText('Showing 3 of 3')).toBeInTheDocument();
  });
});
