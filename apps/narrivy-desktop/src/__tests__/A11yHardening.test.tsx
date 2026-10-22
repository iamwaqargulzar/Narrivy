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
  try {
    out += fs.readFileSync(appCssPath, 'utf-8');
  } catch {}
  try {
    out += '\n' + fs.readFileSync(tokensPath, 'utf-8');
  } catch {}
  return out;
}

describe('A11yHardening', () => {
  it('focus-visible ring, nav role, skip link, icon aria-label, color contrast token presence', async () => {
    window.localStorage.setItem(ONBOARD_KEY, '1');
    render(<App />);

    // 1) nav role with accessible name
    const nav = await screen.findByRole('navigation');
    expect(nav).toBeInTheDocument();
    expect(nav.getAttribute('aria-label')).toBeTruthy();
    // must have role="navigation" explicitly (AppShell sets it) or implicit nav role passes
    expect(nav.tagName.toLowerCase()).toBe('nav');

    // 2) skip link points to main-content
    const skip = screen.getByText('Skip to main content');
    expect(skip).toBeInTheDocument();
    expect(skip.getAttribute('href')).toBe('#main-content');
    const main = await screen.findByRole('main');
    expect(main).toBeInTheDocument();
    expect(main.id).toBe('main-content');
    // main must be focusable via skip link (tabIndex -1)
    expect(main.getAttribute('tabIndex')).toBe('-1');

    // 3) icon aria-label — theme toggle + nav items + icon-only buttons
    const themeBtn = screen.getByRole('button', { name: /use light theme/i });
    expect(themeBtn).toHaveAttribute('aria-label');
    expect(themeBtn.getAttribute('aria-label')!.trim().length).toBeGreaterThan(0);

    const navButtons = nav.querySelectorAll('button');
    expect(navButtons.length).toBeGreaterThan(0);
    navButtons.forEach((b) => {
      const label = b.getAttribute('aria-label') ?? b.textContent ?? '';
      expect(label.trim().length, `nav button missing accessible name: ${b.outerHTML.slice(0, 120)}`).toBeGreaterThan(0);
    });

    // Generic icon-only guard: any button whose visible text is <=2 chars must have aria-label
    const allButtons = Array.from(document.querySelectorAll('button')) as HTMLButtonElement[];
    for (const btn of allButtons) {
      const text = (btn.textContent ?? '').trim();
      const hasAriaLabel = btn.hasAttribute('aria-label') && btn.getAttribute('aria-label')!.trim().length > 0;
      const hasVisibleText = text.length > 1 || /[a-zA-Z]{2,}/.test(text);
      const isIconOnly = !hasVisibleText && text.length > 0 && text.length <= 2;
      if (isIconOnly) {
        expect(hasAriaLabel, `icon button "${text}" missing aria-label`).toBe(true);
      }
    }

    // 4) focus-visible ring presence via tokens.css / app.css
    const cssText = readCss();
    // must define focus-visible selector
    expect(cssText, 'missing :focus-visible selector').toMatch(/:focus-visible/);
    // must define --focus-ring token
    expect(cssText, 'missing --focus-ring token').toMatch(/--focus-ring/);
    // must use outline with var(--focus-ring) and offset
    expect(cssText, 'missing outline var(--focus-ring)').toMatch(/outline.*var\(--focus-ring\)/);
    expect(cssText, 'missing outline-offset for focus ring').toMatch(/outline-offset/);

    // 5) color contrast token presence (high-contrast semantic tokens)
    // light palette + dark override must both be present
    expect(cssText, 'missing --bg token').toMatch(/--bg:/);
    expect(cssText, 'missing --surface token').toMatch(/--surface:/);
    expect(cssText, 'missing --text token').toMatch(/--text:/);
    expect(cssText, 'missing --muted token').toMatch(/--muted:/);
    expect(cssText, 'missing --border token').toMatch(/--border:/);
    expect(cssText, 'missing --accent token').toMatch(/--accent:/);
    // Each explicit theme declares its matching browser color scheme.
    expect(cssText, 'missing dark color-scheme').toMatch(/(?:\:root|html\.dark)[\s\S]*?color-scheme:\s*dark/);
    expect(cssText, 'missing light color-scheme').toMatch(/html\.light[\s\S]*?color-scheme:\s*light/);
    // Both explicit theme blocks must exist.
    expect(cssText, 'missing html.dark block').toMatch(/html\.dark/);
    expect(cssText, 'missing html.light block').toMatch(/html\.light/);
    expect(cssText, 'missing dark accent token').toMatch(/html\.dark[\s\S]*--accent:/);
    expect(cssText, 'missing light accent token').toMatch(/html\.light[\s\S]*--accent:/);
    expect(cssText, 'type tokens must scale with the root font size').not.toMatch(/--font-(?:ui|small|heading):[^;]*\bpx\b/);
    expect(cssText, 'missing increased-contrast palette').toMatch(/prefers-contrast:\s*more/);
    expect(cssText, 'missing forced-colors focus treatment').toMatch(/forced-colors:\s*active[\s\S]*Highlight/);

    // nav active state must not be color-only: check aria-current + textual heading change
    // clicking nav changes heading text, which is non-color state signal
    const dictBtn = screen.getByRole('button', { name: 'History' });
    expect(dictBtn).toHaveAttribute('aria-current', 'false');
    fireEvent.click(dictBtn);
    const heading = await screen.findByRole('heading', { level: 1, name: 'History' });
    expect(heading).toBeInTheDocument();
    // after navigation, dictation button should be aria-current true
    expect(dictBtn.getAttribute('aria-current')).toBe('true');
  });
});
