// Status freshness: a daemon-owned model provision that completes after the
// first snapshot must be able to update Home readiness. The provider performs
// a bounded refresh while the operation is active instead of leaving the
// initial awaiting snapshot permanently stale.
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import App from '../App';
import { bridge } from '../lib/bridge';
import {
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
});

describe('StatusFreshness', () => {
  it('updates readiness after an active model provision completes', async () => {
    let statusCalls = 0;
    vi.mocked(bridge.status).mockImplementation(async () => {
      statusCalls += 1;
      if (statusCalls <= 1) {
        return installedRecoveryStatus({
          model_provisioning: {
            operation_id: 'op-1',
            state: 'activating',
            publication: 'not_published',
            cleanup: 'not_required',
            revision: 1,
          },
        });
      }
      return readyRecoveryStatus({
        model_provisioning: {
          operation_id: 'op-1',
          state: 'ready',
          publication: 'verified_installed',
          cleanup: 'confirmed',
          revision: 2,
        },
      });
    });
    vi.mocked(bridge.listPipelines).mockImplementation(async () =>
      statusCalls <= 1 ? STRICT_MODES_DISABLED : STRICT_MODES_ENABLED,
    );

    render(<App />);

    await waitFor(() =>
      expect(screen.getByTestId('home-readiness')).toHaveTextContent(
        /speech model not installed/i,
      ),
    );

    // No navigation, no manual reload: the bounded freshness read observes the
    // completed provision and the advertised lane becomes startable.
    await waitFor(
      () => expect(screen.getByTestId('home-readiness')).toHaveTextContent(/ready when you are/i),
      { timeout: 5000 },
    );
    await waitFor(() => expect(screen.getByTestId('dictation-start')).toBeEnabled());
    await waitFor(() =>
      expect(screen.getByTestId('home-model-status')).toHaveTextContent('Local GGUF (transcribe.cpp)'),
    );
  });
});
