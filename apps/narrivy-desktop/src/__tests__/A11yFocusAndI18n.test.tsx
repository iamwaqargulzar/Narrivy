// WCAG 2.1 AA behaviour that is easy to lose: dialogs take, keep and give
// back focus; popovers return focus on Escape; the palette tells assistive
// tech which option is selected; and .tsx files carry no hard-coded words.
import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import * as fs from 'node:fs';
import * as path from 'node:path';
import App from '../App';
import { ToastProvider } from '../ui/primitives/Toast';
import { ShortcutCapture } from '../app/components/ShortcutCapture';
import { StatusPill } from '../ui/primitives/StatusPill';
import { ThemeProvider } from '../theme/ThemeProvider';
import { ThemeCard } from '../app/components/SettingsCompute/ThemeCard';
import { t } from '../i18n';
import { NavProvider } from '../state/NavContext';

vi.mock('../lib/bridge', async () => {
  const { createMockBridgeModule } = await import('../test/mockBridge');
  return createMockBridgeModule();
});

const ONBOARD_KEY = 'narrivy.onboarded.v1';

function tab(target: Element, shiftKey = false): KeyboardEvent {
  const event = new KeyboardEvent('keydown', { key: 'Tab', shiftKey, bubbles: true, cancelable: true });
  target.dispatchEvent(event);
  return event;
}

describe('dialog focus', () => {
  it('onboarding takes focus, keeps Tab inside and moves focus to each new step', async () => {
    window.localStorage.removeItem(ONBOARD_KEY);
    render(<App />);
    const dialog = await screen.findByRole('dialog', { name: t('onboard.title') });
    await waitFor(() => expect(dialog).toHaveFocus());

    const buttons = within(dialog).getAllByRole('button');
    const last = buttons[buttons.length - 1];
    last.focus();
    expect(tab(last).defaultPrevented).toBe(true);
    expect(buttons[0]).toHaveFocus();
    expect(tab(buttons[0], true).defaultPrevented).toBe(true);
    expect(last).toHaveFocus();

    fireEvent.click(within(dialog).getByRole('button', { name: t('action.next') }));
    await waitFor(() =>
      expect(within(dialog).getByRole('heading', { name: t('onboard.mic.title') })).toHaveFocus(),
    );
  });

  it('the shortcut dialog keeps Tab inside and gives focus back when it closes', async () => {
    const opener = document.createElement('button');
    document.body.appendChild(opener);
    opener.focus();
    const onClose = vi.fn();
    const { unmount } = render(
      <NavProvider>
        <ToastProvider>
          <ShortcutCapture target={{ pipelineId: 'p', pipelineName: 'Notes' }} onClose={onClose} onSaved={() => {}} />
        </ToastProvider>
      </NavProvider>,
    );
    const dialog = screen.getByRole('dialog', { name: t('capture.title', { name: 'Notes' }) });
    expect(dialog).toHaveFocus();

    fireEvent.keyDown(dialog, { key: 'a', code: 'KeyA', ctrlKey: true });
    const again = await screen.findByRole('button', { name: t('capture.again') });
    const cancel = screen.getByRole('button', { name: t('capture.cancel') });
    const save = screen.getByRole('button', { name: t('capture.save') });
    save.focus();
    expect(tab(save).defaultPrevented).toBe(true);
    expect(cancel).toHaveFocus();
    expect(again).toBeInTheDocument();

    fireEvent.keyDown(cancel, { key: 'Escape' });
    unmount();
    expect(opener).toHaveFocus();
    opener.remove();
  });
});

describe('popovers and the palette', () => {
  it('Escape closes Tips and puts focus back on its button', async () => {
    window.localStorage.setItem(ONBOARD_KEY, '1');
    render(<App />);
    const trigger = await screen.findByRole('button', { name: t('home.tipsTitle') });
    fireEvent.click(trigger);
    expect(screen.getByRole('dialog', { name: t('home.tipsTitle') })).toBeInTheDocument();
    trigger.focus();
    fireEvent.keyDown(document, { key: 'Escape' });
    await waitFor(() => expect(screen.queryByRole('dialog', { name: t('home.tipsTitle') })).toBeNull());
    expect(trigger).toHaveFocus();
    expect(trigger).toHaveAttribute('aria-expanded', 'false');
  });

  it('the palette search box points at the selected option', async () => {
    window.localStorage.setItem(ONBOARD_KEY, '1');
    render(<App />);
    fireEvent.keyDown(window, { key: 'k', ctrlKey: true });
    const box = await screen.findByRole('combobox', { name: t('palette.placeholder') });
    const first = screen.getAllByRole('option')[0];
    expect(box).toHaveAttribute('aria-activedescendant', first.id);
    fireEvent.keyDown(box, { key: 'ArrowDown' });
    const second = screen.getAllByRole('option')[1];
    expect(second).toHaveAttribute('aria-selected', 'true');
    expect(box).toHaveAttribute('aria-activedescendant', second.id);
  });
});

describe('strings come from en.json', () => {
  it('theme choices are named from the message catalogue', () => {
    render(
      <ThemeProvider>
        <ThemeCard />
      </ThemeProvider>,
    );
    const group = screen.getByRole('group', { name: t('settings.theme.title') });
    expect(
      within(group).getByRole('button', {
        name: t('settings.theme.optionAria', { label: t('settings.theme.dark'), hint: t('settings.theme.darkHint') }),
      }),
    ).toBeInTheDocument();
  });

  it('the status pill reads its label from the catalogue and shows unknown states as reported', () => {
    const { rerender } = render(<StatusPill phase="inbox_recovered" />);
    expect(screen.getByText(t('pill.inboxRecovered'))).toBeInTheDocument();
    rerender(<StatusPill phase="warming_up" />);
    expect(screen.getByText('warming_up')).toBeInTheDocument();
  });

  it('no .tsx file has plain words between JSX tags', () => {
    // Words outside {t(...)}: a crude but cheap guard. Key caps, product ids
    // and file paths shown as examples are not copy and are allowed.
    const allowed = new Set(['K', 'asr.transcribe-cpp', 'mock.asr']);
    const root = path.resolve(process.cwd(), 'src');
    const offenders: string[] = [];
    const walk = (dir: string) => {
      for (const name of fs.readdirSync(dir)) {
        const full = path.join(dir, name);
        if (fs.statSync(full).isDirectory()) {
          if (name !== '__tests__' && name !== 'test') walk(full);
        } else if (name.endsWith('.tsx')) {
          const source = fs.readFileSync(full, 'utf-8');
          for (const match of source.matchAll(/>\s*([A-Za-z][A-Za-z .,'’!?-]*[A-Za-z.!?])\s*<\//g)) {
            if (!allowed.has(match[1])) offenders.push(`${path.relative(root, full)}: ${match[1]}`);
          }
          for (const match of source.matchAll(/\b(aria-label|title|placeholder|alt)="([^"]*[A-Za-z]{2,}[^"]*)"/g)) {
            if (!match[2].startsWith('/') && !match[2].startsWith('http')) offenders.push(`${path.relative(root, full)}: ${match[1]}="${match[2]}"`);
          }
        }
      }
    };
    walk(root);
    expect(offenders).toEqual([]);
  });
});
