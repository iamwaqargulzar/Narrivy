// Capture window + typed capture-end regressions.
//
// The daemon/platform boundary owns the bounded capture window and reports it
// as capabilities; the frontend only renders the reported limit and the typed,
// content-free reason a session's capture ended. No OS-side capture policy is
// duplicated here.
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, render, screen, waitFor } from '@testing-library/react';
import App from '../App';
import { bridge, type SessionEvent, type StatusSnapshot } from '../lib/bridge';
import {
  CAPTURE_END_REASONS,
  captureEndNotice,
  documentedCaptureMaxSeconds,
  isCaptureEndReason,
  reportedCaptureLimitSeconds,
} from '../lib/capturePolicy';

vi.mock('../lib/bridge', async () => {
  const { createMockBridgeModule } = await import('../test/mockBridge');
  return createMockBridgeModule();
});

const ONBOARD_KEY = 'narrivy.onboarded.v1';

function statusWith(capabilities: string[]): StatusSnapshot {
  return {
    capabilities,
    active_session: { state: 'idle', pipeline_id: null },
    daemon_build: 'narrivyd test',
    installed_model: 'ready',
  };
}

const PRODUCTION_CAPTURE_CAPABILITIES = [
  'mic.available',
  'mock.asr',
  'capture.window_seconds=300',
  'capture.max_seconds=300',
  'pipeline.clean=enabled',
];

const PROBE_CAPTURE_CAPABILITIES = [
  'mic.available',
  'mock.asr',
  'capture.window_seconds=3',
  'capture.max_seconds=300',
  'pipeline.clean=enabled',
];

const LEGACY_CAPTURE_CAPABILITIES = ['mic.available', 'mock.asr', 'pipeline.clean=enabled'];

beforeEach(() => {
  window.localStorage.setItem(ONBOARD_KEY, '1');
  vi.mocked(bridge.status).mockReset();
  vi.mocked(bridge.listPipelines)
    .mockReset()
    .mockResolvedValue([{ id: 'clean', name: 'Clean dictation', version: 1, enabled: true }]);
  vi.mocked(bridge.recoveryEntry).mockReset();
  vi.mocked(bridge.onSessionEvent).mockReset();
});

async function openHome() {
  render(<App />);
  return screen.findByTestId('dictation-start');
}

describe('capturePolicy', () => {
  it('parses only daemon-reported finite positive capture limits', () => {
    const status = statusWith([
      'capture.window_seconds=300',
      'capture.max_seconds=300',
    ]);
    expect(reportedCaptureLimitSeconds(status)).toBe(300);
    expect(documentedCaptureMaxSeconds(status)).toBe(300);

    for (const invalid of [
      'capture.window_seconds=0',
      'capture.window_seconds=-5',
      'capture.window_seconds=abc',
      'capture.window_seconds=Infinity',
      'capture.window_seconds=',
    ]) {
      expect(reportedCaptureLimitSeconds(statusWith([invalid]))).toBeNull();
    }
    // A daemon that reports no capture window renders no invented limit.
    expect(reportedCaptureLimitSeconds(statusWith(['mic.available']))).toBeNull();
    expect(documentedCaptureMaxSeconds(null)).toBeNull();
  });

  it('maps known wire tags and never invents an end reason', () => {
    for (const reason of CAPTURE_END_REASONS) {
      expect(isCaptureEndReason(reason)).toBe(true);
    }
    expect(isCaptureEndReason('invented_reason')).toBe(false);
    expect(isCaptureEndReason(undefined)).toBe(false);
    expect(isCaptureEndReason(7)).toBe(false);

    // A graceful Stop or an endpoint is the ordinary outcome: no extra notice.
    expect(captureEndNotice(null, 'finalization_requested')).toBeNull();
    expect(captureEndNotice(null, 'endpoint_detected')).toBeNull();
    expect(captureEndNotice(null, null)).toBeNull();
    expect(captureEndNotice(null, undefined)).toBeNull();
    expect(captureEndNotice(null, 'invented_reason')).toBeNull();

    expect(
      captureEndNotice(statusWith(['capture.max_seconds=300']), 'safety_limit'),
    ).toEqual({ key: 'dictation.endSafetyLimit', values: { seconds: 300 } });
    expect(captureEndNotice(null, 'safety_limit')).toEqual({
      key: 'dictation.endSafetyLimitUnknown',
      values: {},
    });
    expect(captureEndNotice(null, 'source_ended')).toEqual({
      key: 'dictation.endSourceEnded',
      values: {},
    });
  });
});

describe('CapturePolicyRendering', () => {
  it('renders the reported production window and the explicit maximum end reason', async () => {
    vi.mocked(bridge.status).mockImplementation(async () =>
      statusWith(PRODUCTION_CAPTURE_CAPABILITIES),
    );
    vi.mocked(bridge.recoveryEntry).mockImplementation(async (txid: string) => ({
      txid,
      state: 'delivered',
      delivered: 'a long dictated passage',
      raw: 'a long dictated passage',
      capture_end: 'safety_limit',
    }));
    let emit: ((event: SessionEvent) => void) | null = null;
    vi.mocked(bridge.onSessionEvent).mockImplementation(async (handler) => {
      emit = handler as (event: SessionEvent) => void;
      return () => {};
    });

    await openHome();
    expect(await screen.findByTestId('dictation-capture-limit')).toHaveTextContent('300 s');
    expect(screen.queryByTestId('dictation-capture-end')).not.toBeInTheDocument();

    await waitFor(() => expect(emit).not.toBeNull());
    await act(async () => {
      emit!({
        event: 'session_state_changed',
        event_seq: 1,
        session_id: '41',
        transaction_id: 'tx-long',
        pipeline_id: 'clean',
        seq: 1,
        state: 'committed',
      });
    });

    const notice = await screen.findByTestId('dictation-capture-end');
    // The explicit bound is surfaced with the reported maximum and the text
    // captured before the limit is retained.
    expect(notice).toHaveTextContent(/300 s capture limit/i);
    expect(notice).toHaveTextContent(/transcribed/i);
    expect(await screen.findByText('a long dictated passage')).toBeInTheDocument();
  });

  it('renders a shortened developer probe window against the documented maximum', async () => {
    vi.mocked(bridge.status).mockImplementation(async () =>
      statusWith(PROBE_CAPTURE_CAPABILITIES),
    );
    vi.mocked(bridge.onSessionEvent).mockImplementation(async () => () => {});
    await openHome();
    const limit = await screen.findByTestId('dictation-capture-limit');
    expect(limit).toHaveTextContent(/Developer probe window 3 s/);
    expect(limit).toHaveTextContent(/maximum stays 300 s/);
  });

  it('never invents a limit or an end reason for a legacy daemon report', async () => {
    vi.mocked(bridge.status).mockImplementation(async () =>
      statusWith(LEGACY_CAPTURE_CAPABILITIES),
    );
    vi.mocked(bridge.recoveryEntry).mockImplementation(async (txid: string) => ({
      txid,
      state: 'delivered',
      delivered: 'legacy text',
      capture_end: null,
    }));
    let emit: ((event: SessionEvent) => void) | null = null;
    vi.mocked(bridge.onSessionEvent).mockImplementation(async (handler) => {
      emit = handler as (event: SessionEvent) => void;
      return () => {};
    });

    await openHome();
    expect(screen.queryByTestId('dictation-capture-limit')).not.toBeInTheDocument();

    await waitFor(() => expect(emit).not.toBeNull());
    await act(async () => {
      emit!({
        event: 'session_state_changed',
        event_seq: 1,
        session_id: '42',
        transaction_id: 'tx-legacy',
        pipeline_id: 'clean',
        seq: 1,
        state: 'committed',
      });
    });
    expect(await screen.findByText('legacy text')).toBeInTheDocument();
    expect(screen.queryByTestId('dictation-capture-end')).not.toBeInTheDocument();
  });

  it('surfaces how capture ended even when the session failed without text', async () => {
    vi.mocked(bridge.status).mockImplementation(async () =>
      statusWith(PRODUCTION_CAPTURE_CAPABILITIES),
    );
    vi.mocked(bridge.recoveryEntry).mockImplementation(async (txid: string) => ({
      txid,
      state: 'failed_terminal',
      delivered: null,
      capture_end: 'source_ended',
    }));
    let emit: ((event: SessionEvent) => void) | null = null;
    vi.mocked(bridge.onSessionEvent).mockImplementation(async (handler) => {
      emit = handler as (event: SessionEvent) => void;
      return () => {};
    });

    await openHome();
    await waitFor(() => expect(emit).not.toBeNull());
    await act(async () => {
      emit!({
        event: 'session_state_changed',
        event_seq: 1,
        session_id: '43',
        transaction_id: 'tx-source-ended',
        pipeline_id: 'clean',
        seq: 1,
        state: 'failed',
      });
    });

    const notice = await screen.findByTestId('dictation-capture-end');
    expect(notice).toHaveTextContent(/microphone disconnected/i);
    expect(notice).toHaveTextContent(/saved in History/i);
    // No transcript-bearing content is fabricated for the failed session.
    expect(screen.queryByText(/full text/)).not.toBeInTheDocument();
  });
});
