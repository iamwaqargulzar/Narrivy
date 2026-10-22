import { describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import App from '../App';
import { bridge } from '../lib/bridge';

vi.mock('../lib/bridge', async () => {
  const { createMockBridgeModule } = await import('../test/mockBridge');
  return createMockBridgeModule();
});

const ONBOARD_KEY = 'narrivy.onboarded.v1';

function presetOnboarded() {
  window.localStorage.setItem(ONBOARD_KEY, '1');
}

describe('ErrorToastHasAction', () => {
  it('renders an error toast with a Retry action on daemon failure', async () => {
    presetOnboarded();
    vi.mocked(bridge.status).mockRejectedValue({
      code: 'daemon_unavailable',
      message: 'daemon unreachable',
    });

    render(<App />);

    const toast = await screen.findByTestId('toast');
    expect(toast).toHaveTextContent(/daemon unreachable/i);

    const callsBefore = vi.mocked(bridge.status).mock.calls.length;
    fireEvent.click(within(toast).getByRole('button', { name: /^retry$/i }));
    await waitFor(() =>
      expect(vi.mocked(bridge.status).mock.calls.length).toBeGreaterThan(callsBefore),
    );
  });

  it('offers Open Diagnostics when no retry is available', async () => {
    presetOnboarded();

    // isolate from the previous test's rejecting status mock
    vi.mocked(bridge.status).mockReset();
    vi.mocked(bridge.status).mockResolvedValue({
      capabilities: ['mic.available'],
      active_session: { state: 'idle', pipeline_id: null },
      daemon_build: 'narrivyd 0.3.0-dev',
    });

    let emit: ((state: string) => void) | null = null;
    vi.mocked(bridge.onSessionEvent).mockImplementation(async (handler) => {
      emit = (state: string) =>
        handler({
          seq: 1,
          state,
          session_id: '41',
          transaction_id: 'tx-error',
          pipeline_id: 'clean',
        });
      return () => {};
    });
    // the exact post-delivery recovery-result fetch fails
    vi.mocked(bridge.recoveryEntry).mockRejectedValueOnce({
      code: 'daemon_unavailable',
      message: 'recovery result read failed',
    });

    render(<App />);
    await waitFor(() => expect(emit).not.toBeNull());

    await act(async () => {
      emit!('delivered');
    });

    const message = await screen.findByText(/recovery result read failed/i);
    const toast = message.closest('[data-testid="toast"]') as HTMLElement;
    expect(toast).not.toBeNull();

    fireEvent.click(within(toast).getByRole('button', { name: /open diagnostics/i }));
    expect(await screen.findByRole('heading', { level: 1, name: 'Diagnostics' })).toBeInTheDocument();
  });

  it('rejects recovery text whose transaction identity does not match the request', async () => {
    presetOnboarded();
    const requestedTxid = 'tx-requested-private';
    const returnedTxid = 'tx-returned-private';
    const payloadCanary = 'mismatched recovery payload must stay hidden';

    vi.mocked(bridge.status).mockResolvedValue({
      capabilities: ['mic.available'],
      active_session: { state: 'idle', pipeline_id: null },
      daemon_build: 'narrivyd 0.3.0-dev',
    });
    let emit: ((state: string) => void) | null = null;
    vi.mocked(bridge.onSessionEvent).mockImplementation(async (handler) => {
      emit = (state: string) =>
        handler({
          seq: 1,
          state,
          session_id: '51',
          transaction_id: requestedTxid,
          pipeline_id: 'clean',
        });
      return () => {};
    });
    vi.mocked(bridge.recoveryEntry).mockResolvedValue({
      txid: returnedTxid,
      state: 'delivered',
      raw: payloadCanary,
      delivered: payloadCanary,
      text: payloadCanary,
    });

    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Home' }));
    await waitFor(() => expect(emit).not.toBeNull());
    await act(async () => {
      emit!('delivered');
    });

    expect(bridge.recoveryEntry).toHaveBeenCalledWith(requestedTxid);
    const message = await screen.findByText(/completed result could not be verified/i);
    const toast = message.closest('[data-testid="toast"]') as HTMLElement;
    expect(toast).not.toBeNull();
    expect(screen.getByText(/nothing dictated yet/i)).toBeInTheDocument();
    expect(document.body.textContent).not.toContain(payloadCanary);
    expect(document.body.textContent).not.toContain(requestedTxid);
    expect(document.body.textContent).not.toContain(returnedTxid);
    expect(within(toast).getByRole('button', { name: /open diagnostics/i })).toBeInTheDocument();
  });

  it('rejects recovery text from a nonterminal journal state', async () => {
    presetOnboarded();
    const txid = 'tx-nonterminal-private';
    const payloadCanary = 'nonterminal recovery payload must stay hidden';

    vi.mocked(bridge.status).mockResolvedValue({
      capabilities: ['mic.available'],
      active_session: { state: 'idle', pipeline_id: null },
      daemon_build: 'narrivyd 0.3.0-dev',
    });
    let emit: ((state: string) => void) | null = null;
    vi.mocked(bridge.onSessionEvent).mockImplementation(async (handler) => {
      emit = (state: string) =>
        handler({
          seq: 1,
          state,
          session_id: '52',
          transaction_id: txid,
          pipeline_id: 'clean',
        });
      return () => {};
    });
    vi.mocked(bridge.recoveryEntry).mockResolvedValue({
      txid,
      state: 'committed',
      raw: payloadCanary,
      delivered: payloadCanary,
      text: payloadCanary,
    });

    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Home' }));
    await waitFor(() => expect(emit).not.toBeNull());
    await act(async () => {
      emit!('committed');
    });

    const message = await screen.findByText(/completed result could not be verified/i);
    const toast = message.closest('[data-testid="toast"]') as HTMLElement;
    expect(toast).not.toBeNull();
    expect(screen.getByText(/nothing dictated yet/i)).toBeInTheDocument();
    expect(document.body.textContent).not.toContain(payloadCanary);
    expect(document.body.textContent).not.toContain(txid);
    expect(screen.queryByRole('button', { name: 'Undo — show raw' })).not.toBeInTheDocument();
    expect(within(toast).getByRole('button', { name: /open diagnostics/i })).toBeInTheDocument();
  });
});
