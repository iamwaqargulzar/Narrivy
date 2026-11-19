import { describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import App from '../App';
import { bridge } from '../lib/bridge';

vi.mock('../lib/bridge', async () => {
  const { createMockBridgeModule } = await import('../test/mockBridge');
  return createMockBridgeModule();
});

const ONBOARD_KEY = 'narrivy.onboarded.v1';

describe('UndoRestoresRawAfterClean', () => {
  // UX-009: after a Clean delivery the Dictation screen offers ONE-STEP undo
  // that reverts the displayed output to the recovery transaction's Raw
  // variant without depending on optional History.
  it('reverts displayed clean output to the raw variant on Undo', async () => {
    window.localStorage.setItem(ONBOARD_KEY, '1');

    const RAW_TEXT = 'hello   world from mock asr';
    const CLEAN_TEXT = 'Hello world from mock asr.';

    let emit: ((state: string) => void) | null = null;
    vi.mocked(bridge.onSessionEvent).mockImplementation(async (handler) => {
      emit = (state: string) =>
        handler({
          seq: 1,
          state,
          session_id: '41',
          transaction_id: 'tx-clean-1',
          pipeline_id: 'clean',
        });
      return () => {};
    });
    vi.mocked(bridge.recoveryEntry).mockResolvedValue({
      txid: 'tx-clean-1',
      state: 'delivered',
      pipeline_id: 'clean',
      raw: RAW_TEXT,
      corrected: CLEAN_TEXT,
      delivered: CLEAN_TEXT,
      text: CLEAN_TEXT,
    });

    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Home' }));

    // Delivered event fetches exactly its own recovery transaction.
    await waitFor(() => expect(emit).not.toBeNull());
    await act(async () => {
      emit!('delivered');
    });
    expect(bridge.recoveryEntry).toHaveBeenCalledWith('tx-clean-1');
    expect(bridge.history).not.toHaveBeenCalled();
    expect(bridge.historyEntry).not.toHaveBeenCalled();

    // Clean output shown; mode selector and one-step undo present.
    expect(await screen.findByText(CLEAN_TEXT)).toBeInTheDocument();
    // Undo only changes the displayed text; the active pipeline is untouched.
    expect(screen.getByRole('heading', { name: 'Active pipeline' })).toBeInTheDocument();

    const undo = await screen.findByRole('button', { name: 'Undo — show raw' });
    fireEvent.click(undo);

    // One step back: the RAW variant replaces the displayed output...
    const shown = await screen.findByText(
      (_content, element) => element?.classList.contains('transcript-text') ?? false,
    );
    expect(shown.textContent).toBe(RAW_TEXT);
    expect(screen.queryByText(CLEAN_TEXT)).not.toBeInTheDocument();

    // ...and undo is single-shot (no repeated toggling).
    expect(screen.queryByRole('button', { name: 'Undo — show raw' })).not.toBeInTheDocument();
  });

  it('does not offer undo when no raw variant exists in the entry', async () => {
    window.localStorage.setItem(ONBOARD_KEY, '1');

    let emit: ((state: string) => void) | null = null;
    vi.mocked(bridge.onSessionEvent).mockImplementation(async (handler) => {
      emit = (state: string) =>
        handler({
          seq: 2,
          state,
          session_id: '42',
          transaction_id: 'tx-raw-1',
          pipeline_id: 'raw',
        });
      return () => {};
    });
    vi.mocked(bridge.recoveryEntry).mockResolvedValue({
      txid: 'tx-raw-1',
      state: 'delivered',
      pipeline_id: 'raw',
      raw: null,
      corrected: null,
      delivered: null,
      text: 'plain text',
    });

    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Home' }));
    await waitFor(() => expect(emit).not.toBeNull());
    await act(async () => {
      emit!('delivered');
    });

    expect(await screen.findByText('plain text')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Undo — show raw' })).not.toBeInTheDocument();
  });
});
