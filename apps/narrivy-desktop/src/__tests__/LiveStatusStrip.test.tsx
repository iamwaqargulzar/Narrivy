// The bottom status bar shows what is active and what is in use, read only
// from daemon status and refreshed in the background.
import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, render, screen, within } from '@testing-library/react';
import App from '../App';
import { bridge } from '../lib/bridge';
import { DEFAULT_CAPABILITIES } from '../test/mockBridge';
import { LIVE_REFRESH_MS } from '../state/StatusContext';

vi.mock('../lib/bridge', async () => {
  const { createMockBridgeModule } = await import('../test/mockBridge');
  return createMockBridgeModule();
});

function statusWith(extra: string[]) {
  return {
    capabilities: [
      ...DEFAULT_CAPABILITIES.filter((c) => c !== 'mock.asr'),
      'asr.transcribe-cpp.cpu=available',
      'asr.transcribe-cpp.runtime=resident-worker',
      ...extra,
    ],
    active_session: { state: 'idle', pipeline_id: null },
    daemon_build: 'narrivyd 0.3.0-dev',
    protocol_minor: 10,
    installed_model: 'ready',
  };
}

function strip(): HTMLElement {
  return screen.getByRole('contentinfo', { name: 'Live status' });
}

afterEach(() => {
  vi.useRealTimers();
});

describe('LiveStatusStrip', () => {
  it('shows the memory, graphs of CPU, GPU and NPU, and no privacy note (owner, 2026-10-25)', async () => {
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    vi.mocked(bridge.status).mockResolvedValue(
      statusWith([
        'diagnostics.resources.rss_bytes=89161728',
        'diagnostics.resources.processes=2',
        'diagnostics.resources.cpu_permille=31',
        'diagnostics.resources.gpu_permille=250',
        'diagnostics.resources.npu_permille=600',
        'diagnostics.latency.p50_us=412000',
      ]),
    );
    const { container } = render(<App />);
    expect(await within(strip()).findByText('RAM 85 MB')).toBeInTheDocument();
    const meter = (name: string) => container.querySelector(`.strip-meter[data-item="${name}"]`);
    expect(meter('cpu')).toHaveTextContent('CPU3%');
    expect(meter('gpu')).toHaveTextContent('GPU25%');
    expect(meter('npu')).toHaveTextContent('NPU60%');
    expect(meter('cpu')?.querySelector('svg.strip-graph')).not.toBeNull();
    expect(within(strip()).queryByText('Private to this computer')).toBeNull();
    // Where recognition runs is shown under the models, not repeated here;
    // the dictation latency is no longer shown.
    expect(within(strip()).queryByText(/Runs on/)).toBeNull();
    expect(within(strip()).queryByText(/Latency/)).toBeNull();
  });

  it('shows the GPU and NPU only when the computer has them', async () => {
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    vi.mocked(bridge.status).mockResolvedValue(
      statusWith([
        'diagnostics.resources.cpu_permille=31',
        'diagnostics.resources.gpu_permille=unavailable:warming_up',
        'diagnostics.resources.npu_permille=unavailable:no_device',
      ]),
    );
    const { container } = render(<App />);
    await within(strip()).findByText(/RAM/);
    expect(container.querySelector('.strip-meter[data-item="gpu"]')).toHaveTextContent('GPU—');
    expect(container.querySelector('.strip-meter[data-item="npu"]')).toBeNull();
  });

  it('shows a dash for values the daemon does not report, and refreshes in the background', async () => {
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval', 'setTimeout', 'clearTimeout'] });
    vi.mocked(bridge.status).mockResolvedValue(
      statusWith(['diagnostics.resources.rss_bytes=unavailable:warming_up', 'diagnostics.latency.p50_us=unavailable:no_samples']),
    );
    render(<App />);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(within(strip()).getByText('RAM —')).toBeInTheDocument();

    vi.mocked(bridge.status).mockResolvedValue(statusWith(['diagnostics.resources.rss_bytes=209715200']));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(LIVE_REFRESH_MS);
    });
    expect(within(strip()).getByText('RAM 200 MB')).toBeInTheDocument();
  });

  it('says the service failed when the first read is superseded by a failing background read', async () => {
    // Seen natively: the first read waited out the on-demand start while the
    // 3 s poll replaced it, and the app said "Connecting" forever.
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval', 'setTimeout', 'clearTimeout'] });
    const failed = {
      code: 'daemon_failed',
      message: "Narrivy's background service could not start. For details run: journalctl --user -u narrivy-daemon",
    };
    vi.mocked(bridge.status)
      .mockImplementationOnce(() => new Promise(() => {}))
      .mockRejectedValue(failed);
    render(<App />);
    expect(within(strip()).getByText('Connecting…')).toBeInTheDocument();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(LIVE_REFRESH_MS);
    });
    expect(within(strip()).getByText(failed.message)).toBeInTheDocument();
    expect(screen.getAllByText('Service failed').length).toBeGreaterThan(0);
  });

  it('keeps the last status for one failed refresh and goes offline after two', async () => {
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval', 'setTimeout', 'clearTimeout'] });
    vi.mocked(bridge.status).mockResolvedValue(statusWith(['diagnostics.resources.rss_bytes=209715200']));
    render(<App />);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(within(strip()).getByText('RAM 200 MB')).toBeInTheDocument();

    vi.mocked(bridge.status).mockRejectedValue({ code: 'daemon_unavailable', message: 'gone' });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(LIVE_REFRESH_MS);
    });
    expect(within(strip()).getByText('RAM 200 MB')).toBeInTheDocument();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(LIVE_REFRESH_MS);
    });
    expect(within(strip()).queryByText('RAM 200 MB')).toBeNull();
    expect(within(strip()).getByText('Offline')).toBeInTheDocument();
  });
});
