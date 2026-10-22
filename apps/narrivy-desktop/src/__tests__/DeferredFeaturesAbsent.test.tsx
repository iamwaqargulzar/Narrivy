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

describe('DeferredFeaturesAbsent', () => {
  it('never renders Snippets or Integrations on any screen', async () => {
    presetOnboarded();
    render(<App />);

    fireEvent.click(screen.getByRole('button', { name: 'Home' }));
    await screen.findByRole('heading', { level: 1, name: 'Home' });
    expect(document.body.textContent?.toLowerCase()).not.toContain('snippet');
    expect(document.body.textContent?.toLowerCase()).not.toContain('integration');
    fireEvent.click(screen.getByRole('button', { name: 'Home' }));
    await screen.findByRole('heading', { level: 1, name: 'Home' });
    expect(document.body.textContent?.toLowerCase()).not.toContain('snippet');
    expect(document.body.textContent?.toLowerCase()).not.toContain('integration');
    fireEvent.click(screen.getByRole('button', { name: 'Models' }));
    await screen.findByRole('heading', { level: 1, name: 'Models' });
    expect(document.body.textContent?.toLowerCase()).not.toContain('snippet');
    expect(document.body.textContent?.toLowerCase()).not.toContain('integration');
    fireEvent.click(screen.getByRole('button', { name: 'History' }));
    await screen.findByRole('heading', { level: 1, name: 'History' });
    expect(document.body.textContent?.toLowerCase()).not.toContain('snippet');
    expect(document.body.textContent?.toLowerCase()).not.toContain('integration');
    fireEvent.click(screen.getByRole('button', { name: 'Settings' }));
    await screen.findByRole('heading', { level: 1, name: 'Settings' });
    expect(document.body.textContent?.toLowerCase()).not.toContain('snippet');
    expect(document.body.textContent?.toLowerCase()).not.toContain('integration');
    fireEvent.click(screen.getByRole('button', { name: 'Diagnostics' }));
    await screen.findByRole('heading', { level: 1, name: 'Diagnostics' });
    expect(document.body.textContent?.toLowerCase()).not.toContain('snippet');
    expect(document.body.textContent?.toLowerCase()).not.toContain('integration');
    fireEvent.click(screen.getByRole('button', { name: 'About' }));
    await screen.findByRole('heading', { level: 1, name: 'About' });
    expect(document.body.textContent?.toLowerCase()).not.toContain('snippet');
    expect(document.body.textContent?.toLowerCase()).not.toContain('integration');

    expect(screen.queryByText(/snippets/i)).toBeNull();
    expect(screen.queryByText(/integrations/i)).toBeNull();
  });
});
