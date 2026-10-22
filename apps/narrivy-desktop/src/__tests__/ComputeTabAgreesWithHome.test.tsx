// One app, one resolved compute claim. The Compute tab used to render a second,
// fabricated string next to Home's, so the two screens could contradict each
// other; this asserts they render the identical resolveCompute() value for the
// same daemon status.
import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import App from '../App';
import { bridge } from '../lib/bridge';

vi.mock('../lib/bridge', async () => {
  const { createMockBridgeModule } = await import('../test/mockBridge');
  return createMockBridgeModule();
});

const ONBOARD_KEY = 'narrivy.onboarded.v1';

describe('ComputeTabAgreesWithHome', () => {
  it('shows the same resolved compute string on Home and in Settings', async () => {
    window.localStorage.setItem(ONBOARD_KEY, '1');
    render(<App />);

    const home = await screen.findByText('Auto → CPU (transcribe.cpp)');
    expect(home).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Settings' }));
    fireEvent.click(await screen.findByRole('tab', { name: 'Compute' }));
    const tab = await screen.findByTestId('compute-card');
    expect(tab).toHaveTextContent('Current resolved selection: Auto → CPU (transcribe.cpp)');
    expect(tab).toHaveTextContent('Derived from: capability: asr.transcribe-cpp');
    expect(tab.textContent).not.toMatch(
      /NVIDIA|Intel NPU|Intel Arc|OpenVINO|Whisper|ydotool|uinput|libei|XWayland|VS Code|Firefox|Discord/,
    );
  });

  it('follows the daemon report when the strict resident engine is advertised', async () => {
    window.localStorage.setItem(ONBOARD_KEY, '1');
    vi.mocked(bridge.status).mockResolvedValue({
      capabilities: [
        'mic.available',
        'asr.transcribe-cpp.cpu=available',
        'asr.transcribe-cpp.runtime=resident-worker',
        'delivery.inbox-only=available',
      ],
      active_session: { state: 'idle', pipeline_id: null },
      installed_model: 'ready',
    });
    render(<App />);

    expect(await screen.findByText('Auto → CPU (transcribe.cpp, resident-worker)')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Settings' }));
    fireEvent.click(await screen.findByRole('tab', { name: 'Compute' }));
    const tab = await screen.findByTestId('compute-card');
    expect(tab).toHaveTextContent('Current resolved selection: Auto → CPU (transcribe.cpp, resident-worker)');
    expect(tab).toHaveTextContent('Derived from: capability: asr.transcribe-cpp.cpu=available');
    expect(tab).toHaveTextContent('Unavailable: the daemon reports no capability for this setting');
  });
});
