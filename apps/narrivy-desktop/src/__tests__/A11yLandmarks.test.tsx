import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import App from '../App';
import * as fs from 'node:fs';
import * as path from 'node:path';

vi.mock('../lib/bridge', async () => {
  const { createMockBridgeModule } = await import('../test/mockBridge');
  return createMockBridgeModule();
});

const ONBOARD_KEY = 'narrivy.onboarded.v1';

function readCss(): string {
  const base = process.cwd();
  const appCssPath = path.resolve(base, 'src/theme/app.css');
  const tokensPath = path.resolve(base, 'src/theme/tokens.css');
  let out = '';
  try { out += fs.readFileSync(appCssPath, 'utf-8'); } catch {}
  try { out += '\n' + fs.readFileSync(tokensPath, 'utf-8'); } catch {}
  return out;
}

describe('A11yLandmarks', () => {
  it('icon buttons have aria-label, nav has role, focus-visible ring present, skip link exists', async () => {
    window.localStorage.setItem(ONBOARD_KEY, '1');
    render(<App />);

    const nav = await screen.findByRole('navigation');
    expect(nav).toBeInTheDocument();
    expect(nav.getAttribute('aria-label')).toBeTruthy();

    const main = await screen.findByRole('main');
    expect(main).toBeInTheDocument();
    expect(main.id).toBe('main-content');

    expect(screen.getByText('Skip to main content')).toBeInTheDocument();
    expect(screen.getByLabelText('Processing mode: Local, private')).toBeInTheDocument();

    const themeBtn = screen.getByRole('button', { name: /use light theme/i });
    expect(themeBtn).toHaveAttribute('aria-label');

    const allButtons = Array.from(document.querySelectorAll('button'));
    for (const btn of allButtons) {
      const text = (btn.textContent ?? '').trim();
      const hasAriaLabel = btn.hasAttribute('aria-label') && btn.getAttribute('aria-label')!.trim().length > 0;
      const hasVisibleText = text.length > 1 || /[a-zA-Z]{2,}/.test(text);
      const isIconOnly = !hasVisibleText && text.length > 0 && text.length <= 2;
      if (isIconOnly) {
        expect(hasAriaLabel, `icon button "${text}" missing aria-label`).toBe(true);
      }
    }

    const navButtons = nav.querySelectorAll('button');
    expect(navButtons.length).toBeGreaterThan(0);
    navButtons.forEach((b) => {
      const label = b.getAttribute('aria-label') ?? b.textContent ?? '';
      expect(label.trim().length).toBeGreaterThan(0);
    });

    const cssText = readCss();
    expect(cssText).toMatch(/:focus-visible/);
    expect(cssText).toMatch(/--focus-ring/);
    expect(cssText).toMatch(/outline.*var\(--focus-ring\)/);

    const dictBtn = screen.getByRole('button', { name: 'Home' });
    fireEvent.click(dictBtn);
    const startBtn = await screen.findByTestId('dictation-start');
    expect(startBtn).toHaveAttribute('aria-label');
  });
});
