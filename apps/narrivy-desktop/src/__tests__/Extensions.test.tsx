// Extensions live in Settings → Extensions (D48), not the Store: the
// assistant's and meetings' switches hide their screens, the writing
// assistant's registers the browser connector, and Models → Language models
// offers downloads.
import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import App from '../App';
import { bridge, type LanguageModel } from '../lib/bridge';
import { LanguageModelsPanel } from '../app/screens/LanguageModels';
import { ToastProvider } from '../ui/primitives/Toast';
import { NavProvider } from '../state/NavContext';

vi.mock('../lib/bridge', async () => {
  const { createMockBridgeModule } = await import('../test/mockBridge');
  return createMockBridgeModule();
});

async function openExtensions() {
  window.localStorage.setItem('narrivy.onboarded.v1', '1');
  window.localStorage.removeItem('narrivy.extensions');
  vi.mocked(bridge.status).mockResolvedValue({
    capabilities: ['asr.transcribe-cpp.cpu=available'],
    active_session: { state: 'idle', pipeline_id: null },
    daemon_build: 'narrivyd test',
    protocol_minor: 55,
    installed_model: 'ready',
  });
  render(<App />);
  fireEvent.click(await screen.findByRole('button', { name: 'Settings' }));
  fireEvent.click(await screen.findByRole('tab', { name: /Extensions/ }));
  return screen.findByRole('region', { name: 'Extensions' }).catch(() => document.body);
}

describe('Settings → Extensions', () => {
  it('turning the assistant and meetings off hides their screens and tells the service', async () => {
    await openExtensions();
    const nav = screen.getByRole('navigation');
    expect(within(nav).getByRole('button', { name: 'Assistant' })).toBeInTheDocument();
    expect(within(nav).getByRole('button', { name: 'Meetings' })).toBeInTheDocument();

    fireEvent.click(screen.getByRole('switch', { name: 'AI assistant' }));
    await waitFor(() =>
      expect(bridge.assistantLibrary).toHaveBeenCalledWith('put', 'model', null, JSON.stringify({ enabled: false })),
    );
    await waitFor(() => expect(within(nav).queryByRole('button', { name: 'Assistant' })).toBeNull());

    fireEvent.click(screen.getByRole('switch', { name: 'Meeting transcription' }));
    await waitFor(() => expect(within(nav).queryByRole('button', { name: 'Meetings' })).toBeNull());
    expect(JSON.parse(window.localStorage.getItem('narrivy.extensions') ?? '{}')).toEqual({
      assistant: false,
      meetings: false,
    });
  });

  it('the writing assistant registers the connector and shows how to add the extension', async () => {
    await openExtensions();
    const writing = await screen.findByRole('switch', { name: 'Writing assistant for your browser' });
    expect(writing).toHaveAttribute('aria-checked', 'false');
    fireEvent.click(writing);
    await waitFor(() => expect(bridge.setBrowserConnector).toHaveBeenCalledWith(true));
    expect(await screen.findByText(/Connected to Brave/)).toBeInTheDocument();
    expect(screen.getByText('/usr/share/narrivy/browser-extension')).toBeInTheDocument();
    expect(screen.getByText(/Open brave:\/\/extensions/)).toBeInTheDocument();
  });
});

const model = (extra: Partial<LanguageModel>): LanguageModel => ({
  id: 'qwen3.5-4b-openvino',
  name: 'Qwen3.5 4B',
  publisher: 'Qwen',
  size_bytes: 3_577_365_101,
  license: 'Apache-2.0',
  roles: ['assistant', 'clean_up'],
  languages: ['en'],
  recommended_for: ['assistant'],
  in_use_for: [],
  ...extra,
});

describe('Models → Language models downloads', () => {
  it('offers a model that is not here, downloads it and shows progress', async () => {
    vi.mocked(bridge.languageModels).mockResolvedValue([model({ installed: false, download: 'available' })]);
    render(
      <NavProvider>
        <ToastProvider>
          <LanguageModelsPanel />
        </ToastProvider>
      </NavProvider>,
    );
    const row = await screen.findByTestId('lm-offer-qwen3.5-4b-openvino');
    // Not offered as a choice until it is downloaded.
    expect(screen.queryByRole('option', { name: /Qwen3.5 4B/ })).toBeNull();
    fireEvent.click(within(row).getByRole('button', { name: 'Download Qwen3.5 4B' }));
    await waitFor(() => expect(bridge.downloadModel).toHaveBeenCalledWith('qwen3.5-4b-openvino'));
  });

  it('shows progress with a cancel, and says when a model is not published yet', async () => {
    vi.mocked(bridge.languageModels).mockResolvedValue([
      model({ installed: false, download: 'installing', installed_bytes: 1_788_682_551 }),
      model({ id: 's1-mini-openvino', name: 'S1-mini', roles: ['clean_up'], installed: false, download: 'unpublished' }),
    ]);
    render(
      <NavProvider>
        <ToastProvider>
          <LanguageModelsPanel />
        </ToastProvider>
      </NavProvider>,
    );
    const row = await screen.findByTestId('lm-offer-qwen3.5-4b-openvino');
    expect(within(row).getByRole('status')).toHaveTextContent('Downloading 50%');
    fireEvent.click(within(row).getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(bridge.cancelModelDownload).toHaveBeenCalledWith('qwen3.5-4b-openvino'));
    const s1 = screen.getByTestId('lm-offer-s1-mini-openvino');
    expect(within(s1).getByText('Not published for download yet.')).toBeInTheDocument();
    expect(within(s1).queryByRole('button')).toBeNull();
  });
});
