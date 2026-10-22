// Home truthfulness: readiness, delivery, model and mode claims must come from
// what the daemon actually reports. No hotkey/PTT, no local-only mode switch,
// no fabricated telemetry.
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import App from '../App';
import { bridge } from '../lib/bridge';
import {
  CLEAN_MODE_ID,
  STRICT_MODES_DISABLED,
  STRICT_MODES_ENABLED,
  installedRecoveryStatus,
  readyRecoveryStatus,
} from '../test/installedRecoveryFixtures';

vi.mock('../lib/bridge', async () => {
  const { createMockBridgeModule } = await import('../test/mockBridge');
  return createMockBridgeModule();
});

beforeEach(() => {
  window.localStorage.setItem('narrivy.onboarded.v1', '1');
  vi.mocked(bridge.status).mockReset();
  vi.mocked(bridge.listPipelines).mockReset();
  vi.mocked(bridge.start).mockReset().mockResolvedValue(undefined);
});

describe('HomeTruthfulReadiness', () => {
  it('claims readiness only when the daemon advertises a startable mode, and starts that mode', async () => {
    vi.mocked(bridge.status).mockImplementation(async () => readyRecoveryStatus());
    vi.mocked(bridge.listPipelines).mockImplementation(async () => STRICT_MODES_ENABLED);

    render(<App />);

    await waitFor(() =>
      expect(screen.getByTestId('home-readiness')).toHaveTextContent('Ready when you are'),
    );
    const start = await screen.findByTestId('dictation-start');
    await waitFor(() => expect(start).toBeEnabled());
    expect(screen.getByTestId('home-mode-value')).toHaveTextContent(CLEAN_MODE_ID);

    fireEvent.click(start);
    await waitFor(() => expect(bridge.start).toHaveBeenCalledWith(CLEAN_MODE_ID));

    // No unsupported hotkey/PTT and no local-only mode switch.
    expect(document.body.textContent).not.toMatch(/ctrl/i);
    expect(document.body.textContent).not.toMatch(/hold/i);
    expect(document.body.textContent).not.toMatch(/push.to.talk|ptt/i);
    expect(screen.queryByRole('group', { name: /mode toggle/i })).toBeNull();
    expect(screen.queryByRole('button', { name: /^raw$/i })).toBeNull();
    expect(screen.queryByRole('button', { name: /^clean$/i })).toBeNull();

    // No fabricated telemetry values.
    expect(screen.queryByText('NPU')).toBeNull();
    expect(screen.queryByText('Latency')).toBeNull();
    expect(screen.queryByText('Model memory')).toBeNull();
    expect(document.body.textContent).not.toMatch(/2\.1\s*GB/i);
  });

  it('reports an awaiting model instead of ready and links to model provisioning', async () => {
    vi.mocked(bridge.status).mockImplementation(async () => installedRecoveryStatus());
    vi.mocked(bridge.listPipelines).mockImplementation(async () => STRICT_MODES_DISABLED);

    render(<App />);

    await waitFor(() =>
      expect(screen.getByTestId('home-readiness')).toHaveTextContent(
        /speech model not installed/i,
      ),
    );
    expect(screen.queryByText('Narrivy is ready')).toBeNull();
    const start = await screen.findByTestId('dictation-start');
    await waitFor(() => expect(start).toBeDisabled());

    // Delivery/model text reflects the reported capabilities and lifecycle:
    // an Inbox-only daemon is never labelled as clipboard copy.
    expect(screen.getByTestId('home-delivery-value')).toHaveTextContent(/inbox recovery only/i);
    expect(screen.getByTestId('home-delivery-value')).not.toHaveTextContent(/clipboard copy/i);
    expect(screen.getByTestId('home-model-status')).toHaveTextContent('Not installed');

    fireEvent.click(screen.getByTestId('home-open-models'));
    expect(await screen.findByRole('heading', { level: 1, name: 'Models' })).toBeInTheDocument();
  });

  it('does not claim ready when the daemon is unavailable', async () => {
    vi.mocked(bridge.status).mockRejectedValue({
      code: 'daemon_unavailable',
      message: 'daemon unreachable',
    });
    vi.mocked(bridge.listPipelines).mockRejectedValue({
      code: 'daemon_unavailable',
      message: 'daemon unreachable',
    });

    render(<App />);

    await waitFor(() =>
      expect(screen.getByTestId('home-readiness')).toHaveTextContent(/daemon is unavailable/i),
    );
    expect(screen.queryByText('Narrivy is ready')).toBeNull();
    expect(screen.getByTestId('dictation-start')).toBeDisabled();
    expect(screen.getByTestId('home-retry')).toBeInTheDocument();
  });
});
