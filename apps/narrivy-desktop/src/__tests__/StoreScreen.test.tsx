// The Store shows what the service offers and its verdicts, installs through
// the verified download, and asks before removing anything. Speech models
// are never shown here: they belong on the Models screen.
import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import App from '../App';
import { bridge, type StoreItem } from '../lib/bridge';

vi.mock('../lib/bridge', async () => {
  const { createMockBridgeModule } = await import('../test/mockBridge');
  return createMockBridgeModule();
});

const item = (id: string, kind: StoreItem['kind'], name: string, extra: Partial<StoreItem> = {}): StoreItem => ({
  id,
  kind,
  category: kind === 'runtime' ? 'openvino' : 'refine',
  name,
  size_bytes: 292_000_000,
  license: 'Apache-2.0',
  devices: ['CPU', 'GPU', 'NPU'],
  state: 'available',
  fit: 'fits',
  fit_reason: 'npu_found',
  source: 'published',
  ...extra,
});

async function openStore(items: StoreItem[], minor = 17) {
  window.localStorage.setItem('narrivy.onboarded.v1', '1');
  vi.mocked(bridge.status).mockResolvedValue({
    capabilities: ['asr.transcribe-cpp.cpu=available'],
    active_session: { state: 'idle', pipeline_id: null },
    daemon_build: 'narrivyd test',
    protocol_minor: minor,
    installed_model: 'ready',
  });
  vi.mocked(bridge.storeItems).mockClear().mockResolvedValue(items);
  render(<App />);
  fireEvent.click(await screen.findByRole('button', { name: 'Store' }));
  await screen.findByRole('heading', { level: 1, name: 'Store' });
}

describe('StoreScreen', () => {
  it('shows each verdict as the service reports it and installs only what fits', async () => {
    await openStore([
      item('rt', 'runtime', 'Sample runtime'),
      item('rt2', 'runtime', 'Second runtime', { fit: 'needs_runtime', fit_reason: 'needs_runtime', requires: 'rt' }),
    ]);
    const runtime = await screen.findByTestId('store-rt');
    expect(within(runtime).getByText('Fits this computer: an NPU was found')).toBeInTheDocument();
    const second = screen.getByTestId('store-rt2');
    expect(within(second).getByText('Install Sample runtime first')).toBeInTheDocument();
    expect(within(second).getByRole('button', { name: /Install/ })).toBeDisabled();

    fireEvent.click(within(runtime).getByRole('button', { name: /Install/ }));
    await waitFor(() => expect(bridge.downloadModel).toHaveBeenCalledWith('rt'));
  });

  it('shows what is built in and what is coming, without offering to install either', async () => {
    await openStore([
      item('vulkan', 'runtime', 'Vulkan GPU', { category: 'vulkan', source: 'built_in', state: 'in_use', fit_reason: 'built_in', size_bytes: 0 }),
      item('cuda', 'runtime', 'NVIDIA CUDA', { category: 'cuda', source: 'planned', fit: 'does_not_fit', fit_reason: 'no_nvidia', size_bytes: 0 }),
      item('refine-llm-openvino', 'module', 'Clean-up', { source: 'planned', fit: 'unknown', fit_reason: 'planned', size_bytes: 0 }),
      item('denoise-deepfilternet', 'module', 'DeepFilterNet noise suppression', { category: 'denoise', source: 'planned', fit: 'unknown', fit_reason: 'planned', size_bytes: 0 }),
      item('voice-changer', 'extension', 'Voice changer', { category: 'voice', source: 'planned', fit: 'unknown', fit_reason: 'planned', size_bytes: 0 }),
      item('m1', 'model', 'A speech model from an older service'),
    ]);
    const vulkan = await screen.findByTestId('store-vulkan');
    expect(within(vulkan).getByText('In use')).toBeInTheDocument();
    expect(within(vulkan).getByText(/Nothing to install/)).toBeInTheDocument();
    expect(within(vulkan).queryByRole('button')).toBeNull();

    const cuda = screen.getByTestId('store-cuda');
    expect(within(cuda).getByText('Coming later')).toBeInTheDocument();
    expect(within(cuda).getByText('No NVIDIA GPU found on this computer')).toBeInTheDocument();
    expect(within(cuda).queryByRole('button')).toBeNull();

    expect(screen.getByRole('heading', { name: 'Refine' })).toBeInTheDocument();
    expect(within(screen.getByTestId('store-refine-llm-openvino')).getByText('Coming later')).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: 'Noise suppression' })).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: 'Extensions' })).toBeInTheDocument();
    const voice = screen.getByTestId('store-voice-changer');
    expect(within(voice).getByText('Coming later')).toBeInTheDocument();
    expect(within(voice).getByText(/Changes your voice live/)).toBeInTheDocument();
    expect(screen.queryByTestId('store-m1')).toBeNull();
    expect(screen.getByRole('button', { name: /Speech models are on the Models screen/ })).toBeInTheDocument();
  });

  it('narrows to one section from the side panel, and searches by name and description', async () => {
    await openStore([
      item('rt', 'runtime', 'Sample runtime'),
      item('denoise-deepfilternet', 'module', 'DeepFilterNet noise suppression', { category: 'denoise' }),
      item('voice-changer', 'extension', 'Voice changer', { category: 'voice' }),
    ]);
    await screen.findByTestId('store-rt');
    const sections = screen.getByRole('tablist', { name: 'Store sections' });
    // Sections with nothing in them are not offered.
    expect(within(sections).queryByRole('tab', { name: /Refine/ })).toBeNull();

    fireEvent.click(within(sections).getByRole('tab', { name: /Noise suppression/ }));
    expect(screen.getByTestId('store-denoise-deepfilternet')).toBeInTheDocument();
    expect(screen.queryByTestId('store-rt')).toBeNull();

    fireEvent.click(within(sections).getByRole('tab', { name: /Everything/ }));
    fireEvent.change(screen.getByRole('searchbox', { name: 'Search the store' }), { target: { value: 'calls and games' } });
    expect(screen.getByTestId('store-voice-changer')).toBeInTheDocument();
    expect(screen.queryByTestId('store-rt')).toBeNull();

    fireEvent.change(screen.getByRole('searchbox', { name: 'Search the store' }), { target: { value: 'nothing like this' } });
    expect(screen.getByText('Nothing in the store matches that search.')).toBeInTheDocument();
  });

  it('asks before removing, and never offers removal of an item in use', async () => {
    await openStore([
      item('rt', 'runtime', 'Sample runtime', { state: 'in_use' }),
      item('rt2', 'runtime', 'Second runtime', { state: 'installed' }),
    ]);
    const runtime = await screen.findByTestId('store-rt');
    expect(within(runtime).getByText('In use')).toBeInTheDocument();
    expect(within(runtime).queryByRole('button', { name: 'Remove' })).toBeNull();

    const second = screen.getByTestId('store-rt2');
    fireEvent.click(within(second).getByRole('button', { name: 'Remove' }));
    expect(bridge.removeStoreItem).not.toHaveBeenCalled();
    const confirm = within(second).getByRole('group', { name: 'Remove Second runtime?' });
    fireEvent.click(within(confirm).getByRole('button', { name: 'Remove' }));
    await waitFor(() => expect(bridge.removeStoreItem).toHaveBeenCalledWith('rt2'));
  });

  it('shows install progress and the reason an install failed', async () => {
    await openStore([
      item('rt', 'runtime', 'Sample runtime', { state: 'installing', installed_bytes: 146_000_000 }),
      item('rt2', 'runtime', 'Second runtime', { state: 'failed', failure: 'integrity_mismatch' }),
    ]);
    const bar = await screen.findByRole('progressbar', { name: 'Install of Sample runtime' });
    expect(bar).toHaveAttribute('aria-valuenow', '50');
    expect(within(screen.getByTestId('store-rt2')).getByRole('alert')).toHaveTextContent(/did not match its checksum/);
    expect(within(screen.getByTestId('store-rt2')).getByRole('button', { name: /Try again/ })).toBeInTheDocument();
  });

  it('says so when the service is too old to have a Store', async () => {
    await openStore([], 16);
    expect(await screen.findByText(/older than this app and has no Store/)).toBeInTheDocument();
    expect(bridge.storeItems).not.toHaveBeenCalled();
  });
});
