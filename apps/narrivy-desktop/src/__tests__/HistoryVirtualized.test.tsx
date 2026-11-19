import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import App from '../App';
import { bridge } from '../lib/bridge';

vi.mock('../lib/bridge', async () => {
  const { createMockBridgeModule } = await import('../test/mockBridge');
  return createMockBridgeModule();
});

const ONBOARD_KEY = 'narrivy.onboarded.v1';

function makeEntries(n: number) {
  return Array.from({ length: n }, (_, i) => ({
    txid: `tx-${String(i).padStart(4, '0')}`,
    state: 'delivered',
    pipeline_id: i % 2 === 0 ? 'raw' : 'clean',
    has_delivered_text: true,
    updated_unix_ms: 1_700_000_000_000 + i,
  }));
}

describe('HistoryVirtualized', () => {
  it('renders 1000 entries without DOM explosion (<50 visible nodes)', async () => {
    window.localStorage.setItem(ONBOARD_KEY, '1');
    vi.mocked(bridge.status).mockResolvedValue({
      capabilities: ['history.optional=enabled'],
      active_session: { state: 'idle', pipeline_id: null },
    });
    const entries = makeEntries(1000);
    vi.mocked(bridge.history).mockResolvedValue(entries);

    render(<App />);
    // navigate to History
    const histBtn = await screen.findByRole('button', { name: 'History' });
    fireEvent.click(histBtn);

    const container = await screen.findByTestId('history-virtual-container');
    expect(container).toBeInTheDocument();

    const list = await screen.findByTestId('history-virtual-list');
    // visible children must be <50 (windowed)
    expect(list.children.length).toBeLessThan(50);
    expect(list.children.length).toBeGreaterThan(0);
    // but virtualization still reports total via spacers or container — ensure container exists
    // Ensure we did NOT render all 1000
    expect(list.children.length).toBeLessThan(1000);
    // And that at least the first txid appears
    expect(list.querySelector('[data-txid="tx-0000"]')).not.toBeNull();
    // The last txid should NOT be in the visible window initially (virtualized off-screen)
    expect(list.querySelector('[data-txid="tx-0999"]')).toBeNull();
  });

  it('still renders small lists correctly', async () => {
    window.localStorage.setItem(ONBOARD_KEY, '1');
    vi.mocked(bridge.status).mockResolvedValue({
      capabilities: ['history.optional=enabled'],
      active_session: { state: 'idle', pipeline_id: null },
    });
    vi.mocked(bridge.history).mockResolvedValue([
      { txid: 'tx-a', state: 'delivered', pipeline_id: 'clean', has_delivered_text: true },
      { txid: 'tx-b', state: 'delivered', pipeline_id: 'raw', has_delivered_text: false },
    ]);
    const { unmount } = render(<App />);
    const histBtn = await screen.findByRole('button', { name: 'History' });
    fireEvent.click(histBtn);
    const list = await screen.findByTestId('history-virtual-list');
    expect(list.children.length).toBe(2);
    unmount();
  });

  it('fails closed when optional History is absent and never queries recovery data', async () => {
    window.localStorage.setItem(ONBOARD_KEY, '1');
    vi.mocked(bridge.status).mockResolvedValue({
      capabilities: ['history.optional=disabled:not_composed'],
      active_session: { state: 'idle', pipeline_id: null },
    });
    vi.mocked(bridge.history).mockClear();
    vi.mocked(bridge.historyEntry).mockClear();

    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'History' }));

    const disabled = await screen.findByText(/History is unavailable/i);
    expect(disabled).toHaveAttribute('role', 'status');
    expect(bridge.history).not.toHaveBeenCalled();
    expect(bridge.historyEntry).not.toHaveBeenCalled();
  });
});
