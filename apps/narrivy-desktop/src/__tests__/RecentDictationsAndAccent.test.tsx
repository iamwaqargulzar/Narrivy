// Home's Recent list (Copy / Retry / Delete) and the appearance settings
// (accent picker and greeting switch), which live only in Settings.
import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import App from '../App';
import { bridge } from '../lib/bridge';
import { DEFAULT_CAPABILITIES } from '../test/mockBridge';

vi.mock('../lib/bridge', async () => {
  const { createMockBridgeModule } = await import('../test/mockBridge');
  return createMockBridgeModule();
});

function historyStatus(protocolMinor: number) {
  return {
    capabilities: [
      ...DEFAULT_CAPABILITIES.filter((c) => !c.startsWith('history.optional=')),
      'history.optional=enabled',
    ],
    active_session: { state: 'idle', pipeline_id: null },
    daemon_build: 'narrivyd 0.3.0-dev',
    protocol_minor: protocolMinor,
    installed_model: 'ready',
  };
}

function seedHistory() {
  vi.mocked(bridge.history).mockResolvedValue([
    { txid: 'tx-ok', state: 'delivered', pipeline_id: 'clean', has_delivered_text: true, updated_unix_ms: Date.now(), transcribe_us: 1_250_000 },
    { txid: 'tx-inbox', state: 'committed', pipeline_id: 'clean', has_delivered_text: false, updated_unix_ms: Date.now() },
  ]);
  vi.mocked(bridge.historyEntry).mockImplementation(async (txid: string) => ({
    txid,
    delivered: txid === 'tx-ok' ? 'first dictation' : null,
    corrected: txid === 'tx-inbox' ? 'text that went to the inbox' : null,
  }));
}

describe('RecentDictations', () => {
  it('lists recent dictations, flags one that was not inserted, and deletes on request', async () => {
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    vi.mocked(bridge.status).mockResolvedValue(historyStatus(10));
    seedHistory();
    render(<App />);

    const row = (await screen.findByText('first dictation')).closest('li') as HTMLElement;
    const inboxRow = (await screen.findByText('text that went to the inbox')).closest('li') as HTMLElement;
    expect(within(inboxRow).getByText('Not inserted')).toBeInTheDocument();
    // Retry is offered only for the row that was not inserted, and is not
    // enabled until the daemon can re-insert text.
    expect(within(inboxRow).getByRole('button', { name: 'Retry' })).toBeDisabled();
    expect(within(row).queryByRole('button', { name: 'Retry' })).toBeNull();

    fireEvent.click(within(row).getByRole('button', { name: 'Delete' }));
    await waitFor(() => expect(bridge.deleteHistoryEntry).toHaveBeenCalledWith('tx-ok'));
    await waitFor(() => expect(screen.queryByText('first dictation')).toBeNull());
  });

  it('shows a new dictation as soon as it finishes, even when its text repeats', async () => {
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    vi.mocked(bridge.status).mockResolvedValue(historyStatus(10));
    seedHistory();
    let emit: ((event: Record<string, unknown>) => void) | null = null;
    vi.mocked(bridge.onSessionEvent).mockImplementation(async (handler) => {
      emit = handler as (event: Record<string, unknown>) => void;
      return () => {};
    });
    vi.mocked(bridge.recoveryEntry).mockResolvedValue({ txid: 'tx-new', state: 'delivered', text: 'first dictation', delivered: 'first dictation' });
    render(<App />);
    await screen.findByText('first dictation');
    await waitFor(() => expect(emit).not.toBeNull());

    vi.mocked(bridge.history).mockResolvedValue([
      { txid: 'tx-new', state: 'delivered', pipeline_id: 'clean', has_delivered_text: true, updated_unix_ms: Date.now() },
      { txid: 'tx-ok', state: 'delivered', pipeline_id: 'clean', has_delivered_text: true, updated_unix_ms: Date.now() },
    ]);
    const loads = () => vi.mocked(bridge.history).mock.calls.length;
    let before = loads();
    const identity = { session_id: 's-2', transaction_id: 'tx-new', pipeline_id: 'clean' };
    emit!({ event: 'session_state_changed', event_seq: 1, seq: 1, state: 'preparing', ...identity });
    emit!({ event: 'session_state_changed', event_seq: 2, seq: 2, state: 'delivered', ...identity });

    await waitFor(() => expect(loads()).toBeGreaterThan(before));

    // A second dictation with the same words still appears.
    vi.mocked(bridge.history).mockResolvedValue([
      { txid: 'tx-again', state: 'delivered', pipeline_id: 'clean', has_delivered_text: true, updated_unix_ms: Date.now() },
      { txid: 'tx-new', state: 'delivered', pipeline_id: 'clean', has_delivered_text: true, updated_unix_ms: Date.now() },
      { txid: 'tx-ok', state: 'delivered', pipeline_id: 'clean', has_delivered_text: true, updated_unix_ms: Date.now() },
    ]);
    vi.mocked(bridge.recoveryEntry).mockResolvedValue({ txid: 'tx-again', state: 'delivered', text: 'first dictation', delivered: 'first dictation' });
    before = loads();
    const again = { session_id: 's-3', transaction_id: 'tx-again', pipeline_id: 'clean' };
    emit!({ event: 'session_state_changed', event_seq: 3, seq: 1, state: 'preparing', ...again });
    emit!({ event: 'session_state_changed', event_seq: 4, seq: 2, state: 'delivered', ...again });

    await waitFor(() => expect(loads()).toBeGreaterThan(before));
  });

  it('never offers Delete against a daemon older than protocol minor 10', async () => {
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    vi.mocked(bridge.status).mockResolvedValue(historyStatus(9));
    seedHistory();
    render(<App />);

    const row = (await screen.findByText('first dictation')).closest('li') as HTMLElement;
    expect(within(row).getByRole('button', { name: 'Delete' })).toBeDisabled();
  });
});

describe('Appearance settings', () => {
  it('offers the accent picker only in Settings, and applies and persists it', async () => {
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    render(<App />);
    expect(document.documentElement.dataset.accent).toBe('mono');
    expect(screen.queryByRole('radio', { name: 'Gold' })).toBeNull();

    fireEvent.click(await screen.findByRole('button', { name: 'Settings' }));
    fireEvent.click(await screen.findByRole('radio', { name: 'Gold' }));
    expect(document.documentElement.dataset.accent).toBe('gold');
    expect(window.localStorage.getItem('narrivy.accent')).toBe('gold');
  });

  it('hides and restores the Home greeting', async () => {
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    render(<App />);
    expect(await screen.findByRole('heading', { name: /^Good (morning|afternoon|evening)$/ })).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Settings' }));
    const toggle = await screen.findByRole('switch', { name: 'Show greeting' });
    expect(toggle).toHaveAttribute('aria-checked', 'true');
    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute('aria-checked', 'false');
    expect(window.localStorage.getItem('narrivy.greeting')).toBe('off');

    fireEvent.click(screen.getByRole('button', { name: 'Home' }));
    await screen.findByTestId('dictation-start');
    expect(screen.queryByRole('heading', { name: /^Good (morning|afternoon|evening)$/ })).toBeNull();
  });
});

describe('Version and updates', () => {
  it('shows the version in the sidebar, not the header, and never claims an unchecked update state', async () => {
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    render(<App />);
    await screen.findByTestId('dictation-start');
    const header = document.querySelector('.topbar') as HTMLElement;
    expect(within(header).queryByText(/^v\d/)).toBeNull();
    expect(within(header).queryAllByRole('radio')).toHaveLength(0);

    const version = screen.getByRole('button', { name: /^Version .+\. Updates: Not checked\. Open About$/ });
    expect(screen.queryByText(/up to date/i)).toBeNull();
    fireEvent.click(version);
    expect(await screen.findByRole('heading', { level: 1, name: 'About' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Check for updates' })).toBeDisabled();
    expect(screen.queryByText(/up to date/i)).toBeNull();
  });
});

describe('History screen', () => {
  it('names a not-inserted entry in words, hides raw ids, and deletes on request', async () => {
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    vi.mocked(bridge.status).mockResolvedValue(historyStatus(10));
    seedHistory();
    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'History' }));
    const list = await screen.findByTestId('history-virtual-list');
    expect(await within(list).findByText('Not inserted')).toBeInTheDocument();
    expect(within(list).queryByText('committed')).toBeNull();
    expect(within(list).queryByText('tx-ok')).toBeNull();
    // Each row names its pipeline, and its transcription time when measured.
    const timed = list.querySelector('[data-txid="tx-ok"]') as HTMLElement;
    expect(timed.querySelector('.history-pipeline')?.textContent).toMatch(/clean/i);
    expect(within(timed).getByText('Transcribed in 1.3 s')).toBeInTheDocument();
    const untimed = list.querySelector('[data-txid="tx-inbox"]') as HTMLElement;
    expect(within(untimed).queryByText(/Transcribed in/)).toBeNull();
    // Text shows directly, with no reveal step.
    expect(await within(list).findByText('first dictation')).toBeInTheDocument();
    expect(within(list).queryByRole('button', { name: /reveal/i })).toBeNull();

    const row = list.querySelector('[data-txid="tx-ok"]') as HTMLElement;
    fireEvent.click(within(row).getByRole('button', { name: 'Delete' }));
    await waitFor(() => expect(bridge.deleteHistoryEntry).toHaveBeenCalledWith('tx-ok'));
    await waitFor(() => expect(list.querySelector('[data-txid="tx-ok"]')).toBeNull());
  });
});
