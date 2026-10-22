import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import App from '../App';

vi.mock('../lib/bridge', async () => {
  const { createMockBridgeModule } = await import('../test/mockBridge');
  return createMockBridgeModule();
});

const ONBOARD_KEY = 'narrivy.onboarded.v1';

function presetOnboarded() {
  window.localStorage.setItem(ONBOARD_KEY, '1');
}

describe('HomeShowsComputedAutoValues', () => {
  it('shows Auto → resolved values derived only from capability strings', async () => {
    presetOnboarded();
    render(<App />);

    // capabilities: mic.available, asr.transcribe-cpp, mock.asr, delivery.clipboard
    expect(await screen.findByText('Auto → CPU (transcribe.cpp)')).toBeInTheDocument();
    expect(screen.getByText('Auto → Local GGUF (transcribe.cpp)')).toBeInTheDocument();
    expect(screen.getByText('Auto → System default microphone')).toBeInTheDocument();
    expect(screen.getByText('Auto → Clipboard paste')).toBeInTheDocument();
  });
});
