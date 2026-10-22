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
  const styleTags = Array.from(document.querySelectorAll('style')).map(s => s.textContent ?? '').join('\n');
  out += '\n' + styleTags;
  return out;
}

describe('MicroInteractions', () => {
  it('Dictation Start button has 150ms press animation', async () => {
    window.localStorage.setItem(ONBOARD_KEY, '1');
    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'Home' }));
    const btn = await screen.findByTestId('dictation-start');
    expect(btn.classList.contains('btn-press')).toBe(true);
    const css = readCss();
    expect(css).toMatch(/\.btn-press\s*\{[^}]*150ms/);
    expect(css).toMatch(/\.btn-press:active/);
  });

  it('respects prefers-reduced-motion: reduce -> 0ms', async () => {
    const original = window.matchMedia;
    window.matchMedia = ((query: string) => ({
      media: query,
      matches: query.includes('prefers-reduced-motion'),
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    })) as unknown as typeof window.matchMedia;

    const css = readCss();
    expect(css).toContain('prefers-reduced-motion');
    expect(css).toMatch(/0ms\s*!important/);
    expect(css).toMatch(/120ms/);

    window.matchMedia = original;
  });

  it('overlay fades in 120ms', async () => {
    const css = readCss();
    expect(css).toMatch(/palette-overlay[^}]*120ms/);
    expect(css).toMatch(/overlayFadeIn/);
    expect(css).toMatch(/pill[^}]*120ms/);
  });
});
