import { describe, expect, it } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import App from '../App';

vi.mock('../lib/bridge', async () => {
  const { createMockBridgeModule } = await import('../test/mockBridge');
  return createMockBridgeModule();
});

const ONBOARD_KEY = 'narrivy.onboarded.v1';

function presetOnboarded() {
  window.localStorage.setItem(ONBOARD_KEY, '1');
}

describe('AppRendersAllScreens', () => {
  it('renders every screen from primary navigation', async () => {
    presetOnboarded();
    render(<App />);

    const cases: Array<[string, string]> = [
      ['Home', 'Home'],
      ['Dictation', 'Dictation'],
      ['Pipelines', 'Pipelines'],
      ['Models', 'Models'],
      ['Refine', 'Refine'],
      ['Applications', 'Applications'],
      ['History', 'History'],
      ['Settings', 'Settings'],
      ['Diagnostics', 'Diagnostics'],
      ['About', 'About'],
    ];

    for (const [navLabel, heading] of cases) {
      fireEvent.click(screen.getByRole('button', { name: navLabel }));
      expect(await screen.findByRole('heading', { level: 1, name: heading })).toBeInTheDocument();
    }
  });
});
