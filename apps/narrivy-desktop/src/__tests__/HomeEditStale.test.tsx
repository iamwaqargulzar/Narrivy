// F-M11: saving an edit never overwrites a newer dictation that arrived
// while the user was editing.
import { describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import App from '../App';
import { bridge, type SessionEvent } from '../lib/bridge';

vi.mock('../lib/bridge', async () => {
  const { createMockBridgeModule } = await import('../test/mockBridge');
  return createMockBridgeModule();
});

const TEXT: Record<string, string> = { 'tx-1': 'first dictation', 'tx-2': 'second dictation' };

describe('HomeEditStale', () => {
  it('keeps the newer dictation and says the edit was not saved', async () => {
    window.localStorage.setItem('narrivy.onboarded.v1', '1');
    vi.mocked(bridge.recoveryEntry).mockImplementation(async (txid: string) => ({
      txid,
      state: 'delivered',
      delivered: TEXT[txid],
      raw: TEXT[txid],
    }));
    let emit: ((event: SessionEvent) => void) | null = null;
    vi.mocked(bridge.onSessionEvent).mockImplementation(async (handler) => {
      emit = handler as (event: SessionEvent) => void;
      return () => {};
    });
    render(<App />);
    await waitFor(() => expect(emit).not.toBeNull());
    let eventSeq = 0;
    const commit = async (txid: string, seq: number) => {
      await act(async () => {
        emit!({
          event: 'session_state_changed',
          event_seq: ++eventSeq,
          session_id: String(seq),
          transaction_id: txid,
          pipeline_id: 'clean',
          seq,
          state: 'committed',
        });
        // The daemon returns to idle after every session.
        emit!({
          event: 'session_state_changed',
          event_seq: ++eventSeq,
          session_id: String(seq),
          transaction_id: txid,
          pipeline_id: 'clean',
          seq: seq + 1,
          state: 'idle',
        });
      });
    };
    await commit('tx-1', 1);
    expect(await screen.findByText('first dictation')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Edit last text' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'Edit last text' }), {
      target: { value: 'first dictation, edited' },
    });
    await commit('tx-2', 2);
    fireEvent.click(screen.getByRole('button', { name: 'Save edit' }));

    expect(await screen.findByText(/newer dictation arrived/i)).toBeInTheDocument();
    expect(await screen.findByText('second dictation')).toBeInTheDocument();
    expect(screen.queryByText('first dictation, edited')).toBeNull();
  });
});
