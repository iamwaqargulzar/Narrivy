import { describe, expect, it } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import App from '../App';
import { bridge } from '../lib/bridge';
import { OverlayPositionCard } from '../app/components/SettingsCompute/OverlayPositionCard';
import { ThemeProvider } from '../theme/ThemeProvider';

vi.mock('../lib/bridge', async () => {
  const { createMockBridgeModule } = await import('../test/mockBridge');
  return createMockBridgeModule();
});

const ONBOARD_KEY = 'narrivy.onboarded.v1';

function presetOnboarded() {
  window.localStorage.setItem(ONBOARD_KEY, '1');
}

describe('ThemePersistsAndAppliesClass', () => {
  it('applies the resolved theme class and persists the choice', async () => {
    presetOnboarded();

    const first = render(<App />);
    // First run is deliberately dark, independent of the host preference.
    expect(document.documentElement.classList.contains('dark')).toBe(true);

    fireEvent.click(screen.getByRole('button', { name: /use light theme/i }));
    expect(document.documentElement.classList.contains('light')).toBe(true);
    expect(window.localStorage.getItem('narrivy.theme')).toBe('light');
    // The host draws the overlay: it is told the theme and accent to match.
    expect(bridge.setOverlayAppearance).toHaveBeenLastCalledWith('light', 'mono', 'bottom-center');

    first.unmount();
    document.documentElement.classList.remove('light', 'dark');

    // fresh mount must restore the persisted choice onto <html>
    render(<App />);
    await waitFor(() =>
      expect(document.documentElement.classList.contains('light')).toBe(true),
    );
    expect(window.localStorage.getItem('narrivy.theme')).toBe('light');
  });

  it('places the overlay at the bottom centre by default and where the user picks', () => {
    window.localStorage.clear();
    const first = render(
      <ThemeProvider>
        <OverlayPositionCard />
      </ThemeProvider>,
    );
    expect(screen.getByRole('radio', { name: 'Bottom centre' })).toHaveAttribute('aria-checked', 'true');
    expect(bridge.setOverlayAppearance).toHaveBeenLastCalledWith('dark', 'mono', 'bottom-center');

    fireEvent.click(screen.getByRole('radio', { name: 'Top right' }));
    expect(screen.getByRole('radio', { name: 'Top right' })).toHaveAttribute('aria-checked', 'true');
    expect(bridge.setOverlayAppearance).toHaveBeenLastCalledWith('dark', 'mono', 'top-right');
    expect(window.localStorage.getItem('narrivy.overlay.position')).toBe('top-right');

    // The choice is restored on the next launch.
    first.unmount();
    render(
      <ThemeProvider>
        <OverlayPositionCard />
      </ThemeProvider>,
    );
    expect(screen.getByRole('radio', { name: 'Top right' })).toHaveAttribute('aria-checked', 'true');
  });
});
