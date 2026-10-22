import { describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import App from '../App';
import { bridge, type SessionEvent } from '../lib/bridge';

vi.mock('../lib/bridge', async () => {
  const { createMockBridgeModule } = await import('../test/mockBridge');
  return createMockBridgeModule();
});

const ONBOARD_KEY = 'narrivy.onboarded.v1';

function presetOnboarded() {
  window.localStorage.setItem(ONBOARD_KEY, '1');
}

describe('DictationStateFromEvents', () => {
  it('updates dictation state and transcript from session-event payloads', async () => {
    presetOnboarded();

    let emit: ((state: string) => void) | null = null;
    let seq = 0;
    vi.mocked(bridge.onSessionEvent).mockImplementation(async (handler) => {
      emit = (state: string) => handler({ seq: ++seq, state });
      return () => {};
    });

    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Home' }));

    await waitFor(() => expect(document.querySelector('.topbar-status')?.getAttribute('data-phase')).toBe('idle'));

    await waitFor(() => expect(emit).not.toBeNull());

    act(() => emit!('recording'));
    await waitFor(() => expect(document.querySelector('.topbar-status')?.getAttribute('data-phase')).toBe('recording'));

    act(() => emit!('transcribing'));
    await waitFor(() => expect(document.querySelector('.topbar-status')?.getAttribute('data-phase')).toBe('transcribing'));

    // A legacy event without transaction identity cannot select recovery data.
    await act(async () => {
      emit!('delivered');
    });
    expect(await screen.findByText(/nothing dictated yet/i)).toBeInTheDocument();
  });

  it('renders only fresh identity-matched partials and clears them at terminal state', async () => {
    presetOnboarded();
    let emit: ((event: SessionEvent) => void) | null = null;
    vi.mocked(bridge.onSessionEvent).mockImplementation(async (handler) => {
      emit = handler;
      return () => {};
    });

    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Home' }));
    await waitFor(() => expect(emit).not.toBeNull());

    act(() =>
      emit!({
        event: 'session_state_changed',
        event_seq: 1,
        session_id: '41',
        transaction_id: 'tx-current',
        pipeline_id: 'streaming',
        seq: 1,
        state: 'listening',
      }),
    );
    act(() =>
      emit!({
        event: 'session_state_changed',
        event_seq: 2,
        session_id: '40',
        transaction_id: 'tx-old',
        pipeline_id: 'streaming',
        seq: 99,
        state: 'listening',
      }),
    );
    await waitFor(() => expect(document.querySelector('.topbar-status')?.getAttribute('data-phase')).toBe('listening'));

    act(() =>
      emit!({
        event: 'transcript_partial',
        event_seq: 3,
        session_id: '41',
        transaction_id: 'tx-stale',
        pipeline_id: 'streaming',
        seq: 1,
        text: 'wrong transaction canary',
      }),
    );
    expect(screen.queryByText('wrong transaction canary')).not.toBeInTheDocument();

    act(() =>
      emit!({
        event: 'transcript_partial',
        event_seq: 4,
        session_id: '41',
        transaction_id: 'tx-current',
        pipeline_id: 'streaming',
        seq: 2,
        text: 'fresh tentative words',
      }),
    );
    expect(await screen.findByText('fresh tentative words')).toHaveAttribute('aria-live', 'polite');

    act(() =>
      emit!({
        event: 'transcript_partial',
        event_seq: 5,
        session_id: '41',
        transaction_id: 'tx-current',
        pipeline_id: 'streaming',
        seq: 1,
        text: 'out of order canary',
      }),
    );
    expect(screen.queryByText('out of order canary')).not.toBeInTheDocument();

    vi.mocked(bridge.history).mockResolvedValue([]);
    act(() =>
      emit!({
        event: 'session_state_changed',
        event_seq: 6,
        session_id: '41',
        transaction_id: 'tx-current',
        pipeline_id: 'streaming',
        seq: 2,
        state: 'committed',
      }),
    );
    // The volatile partial is cleared at the terminal state and never stored.
    await waitFor(() => expect(screen.queryByText('fresh tentative words')).not.toBeInTheDocument());
    expect(JSON.stringify(window.localStorage)).not.toContain('fresh tentative words');
  });

  it('drops a session left on screen when the event stream reconnects', async () => {
    presetOnboarded();
    let emit: ((event: SessionEvent) => void) | null = null;
    vi.mocked(bridge.onSessionEvent).mockImplementation(async (handler) => {
      emit = handler;
      return () => {};
    });

    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Home' }));
    await waitFor(() => expect(emit).not.toBeNull());
    const phase = () => document.querySelector('.topbar-status')?.getAttribute('data-phase');

    act(() =>
      emit!({
        event: 'session_state_changed',
        event_seq: 40,
        session_id: '7',
        transaction_id: 'tx-before-restart',
        pipeline_id: 'clean',
        seq: 2,
        state: 'listening',
      }),
    );
    await waitFor(() => expect(phase()).toBe('listening'));

    // The daemon restarted mid-session; the stream says so.
    act(() => emit!({ event: 'events_resubscribed', event_seq: 41 }));
    await waitFor(() => expect(phase()).toBe('idle'));

    // The restarted daemon's next session is followed normally.
    act(() =>
      emit!({
        event: 'session_state_changed',
        event_seq: 42,
        session_id: '1',
        transaction_id: 'tx-after-restart',
        pipeline_id: 'clean',
        seq: 2,
        state: 'listening',
      }),
    );
    await waitFor(() => expect(phase()).toBe('listening'));
  });
});
