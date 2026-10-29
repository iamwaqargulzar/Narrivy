// F-H13: no control on Settings or Pipelines may look live while changing
// nothing. Every enabled control the screens render is activated twice (so a
// radio or switch that starts on its current value still changes once). It
// must call a bridge method, or be listed below as a control that only acts
// inside the app. Anything else must be disabled or read-only.
import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import App from '../App';
import { bridge, type StatusSnapshot } from '../lib/bridge';
import { routedRecoveryStatus, STRICT_MODES_ENABLED } from '../test/installedRecoveryFixtures';

vi.mock('../lib/bridge', async () => {
  const { createMockBridgeModule } = await import('../test/mockBridge');
  return createMockBridgeModule();
});

/**
 * Controls that act only inside the app, each with its reason. A pattern
 * matches the control's accessible name or its title.
 */
const APP_ONLY: Array<[RegExp, string]> = [
  [/^(System|Light|Dark) theme, /, "changes the app's own look"],
  [/^Show greeting$/, 'shows or hides the greeting on Home'],
  [/^Add shortcut$/, 'opens the shortcut dialog; saving there calls the service'],
  [/^Copy command$/, 'copies a command to the clipboard'],
  [/^Download more models$|^Choose in Models$|needs an NPU model from the Store$/, 'opens Models or the Store'],
  [/^Move .+ (up|down)$/, 'orders the list on this page'],
  [/^Rename |^Click to rename\./, 'a display name kept by the app'],
  [/^Use this pipeline$/, 'chooses the pipeline that Start then sends to the service'],
];

function appOnly(el: HTMLElement, name: string): boolean {
  const title = el.getAttribute('title') ?? '';
  return APP_ONLY.some(([pattern]) => pattern.test(name) || pattern.test(title));
}

/** Bridge reads every screen makes on its own; they do not count as a change. */
const READS = new Set([
  'status',
  'listPipelines',
  'models',
  'pickFile',
  'subscribeEvents',
  'onSessionEvent',
]);

const CONTROLS =
  'button, input, select, textarea, [role="switch"], [role="radio"], [role="checkbox"], [role="slider"]';

function nameOf(el: HTMLElement): string {
  const labelledBy = el.getAttribute('aria-labelledby');
  if (labelledBy) {
    return labelledBy
      .split(/\s+/)
      .map((id) => document.getElementById(id)?.textContent ?? '')
      .join(' ')
      .trim();
  }
  return (el.getAttribute('aria-label') ?? el.getAttribute('placeholder') ?? el.textContent ?? '').trim();
}

function isInert(el: HTMLElement): boolean {
  return (
    (el as HTMLButtonElement).disabled ||
    el.getAttribute('aria-disabled') === 'true' ||
    (el as HTMLInputElement).readOnly ||
    el.getAttribute('aria-readonly') === 'true'
  );
}

/** Navigation inside the screen: section tabs, stage tabs, disclosures, search. */
function isNavigation(el: HTMLElement): boolean {
  return (
    el.getAttribute('role') === 'tab' ||
    el.hasAttribute('aria-expanded') ||
    (el as HTMLInputElement).type === 'search'
  );
}

function liveControls(root: ParentNode): HTMLElement[] {
  return [...root.querySelectorAll<HTMLElement>(CONTROLS)].filter((el) => !isInert(el) && !isNavigation(el));
}

function bridgeWrites(): string[] {
  return Object.entries(bridge)
    .filter(([key, fn]) => !READS.has(key) && vi.isMockFunction(fn) && fn.mock.calls.length > 0)
    .map(([key]) => key);
}

async function settle() {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 20));
  });
}

function closeDialogs() {
  for (const dialog of document.querySelectorAll<HTMLElement>('[role="dialog"], [role="alertdialog"]')) {
    fireEvent.keyDown(dialog, { key: 'Escape' });
  }
}

async function activate(el: HTMLElement) {
  if (el instanceof HTMLSelectElement) {
    const values = [...el.options].map((option) => option.value).filter((value) => value !== el.value);
    if (values.length > 0) fireEvent.change(el, { target: { value: values[values.length - 1] } });
  } else if (el instanceof HTMLTextAreaElement || (el instanceof HTMLInputElement && el.type !== 'checkbox' && el.type !== 'radio')) {
    // A field inside a form counts when the filled-in form sends it.
    const fields = el.form
      ? [...el.form.querySelectorAll<HTMLInputElement>('input, textarea')].filter((field) => !field.value)
      : [el];
    for (const field of new Set([el, ...fields])) {
      fireEvent.change(field, { target: { value: field.type === 'number' ? '2' : 'narrivy' } });
    }
    if (el.form) fireEvent.submit(el.form);
    else fireEvent.keyDown(el, { key: 'Enter' });
    fireEvent.blur(el);
  } else {
    fireEvent.click(el);
  }
  await settle();
  closeDialogs();
  await settle();
}

/** Activate every live control in `root()` and return those that changed nothing. */
async function unwired(root: () => ParentNode): Promise<string[]> {
  const wired = new Set<string>();
  const seen = new Set<string>();
  for (let round = 0; round < 2; round += 1) {
    const counts = new Map<string, number>();
    for (const el of liveControls(root())) {
      const name = nameOf(el);
      const count = (counts.get(name) ?? 0) + 1;
      counts.set(name, count);
      // An earlier control may have hidden or locked this one, and a list
      // with a single entry has nothing to choose.
      if (!el.isConnected || isInert(el)) continue;
      if (el instanceof HTMLSelectElement && el.options.length < 2) continue;
      const key = count > 1 ? `${name} (${count})` : name;
      seen.add(key);
      if (appOnly(el, name)) {
        wired.add(key);
        continue;
      }
      vi.clearAllMocks();
      await activate(el);
      if (bridgeWrites().length > 0) wired.add(key);
    }
  }
  return [...seen].filter((key) => !wired.has(key));
}

async function openApp(status?: StatusSnapshot) {
  window.localStorage.setItem('narrivy.onboarded.v1', '1');
  if (status) vi.mocked(bridge.status).mockResolvedValue(status);
  vi.mocked(bridge.listPipelines).mockResolvedValue(STRICT_MODES_ENABLED as never);
  render(<App />);
}

const STATUSES: Array<[string, StatusSnapshot | undefined]> = [
  ['the default service', undefined],
  ['a routed recovery service', routedRecoveryStatus()],
];

/** Each case drives every control on a screen; allow for a loaded machine. */
const TIMEOUT_MS = 30_000;

afterEach(() => {
  cleanup();
  window.localStorage.clear();
});

describe.each(STATUSES)('controls are wired with %s', (_label, status) => {
  it('every Settings control calls the service, or is disabled or read-only', async () => {
    await openApp(status);
    fireEvent.click(await screen.findByRole('button', { name: 'Settings' }));
    const tabs = await screen.findAllByRole('tab');
    const failures: string[] = [];
    let checked = 0;
    for (const tab of tabs.filter((el) => el.classList.contains('settings-tab'))) {
      fireEvent.click(tab);
      await settle();
      for (const toggle of document.querySelectorAll<HTMLElement>('.settings-main [aria-expanded="false"]')) {
        fireEvent.click(toggle);
      }
      await settle();
      const main = () => document.querySelector('.settings-main') as HTMLElement;
      checked += liveControls(main()).length;
      failures.push(...(await unwired(main)).map((key) => `${tab.textContent}: ${key}`));
    }
    expect(checked).toBeGreaterThan(0);
    expect(failures).toEqual([]);
  }, TIMEOUT_MS);

  it('every Pipelines control calls the service, or is disabled or read-only', async () => {
    await openApp(status);
    fireEvent.click(await screen.findByRole('button', { name: 'Pipelines' }));
    await screen.findByRole('heading', { level: 1, name: 'Pipelines' });
    await settle();
    const page = () => document.querySelector('.page-wide') as HTMLElement;
    const failures = (await unwired(page)).map((key) => `page: ${key}`);
    const stages = [...page().querySelectorAll<HTMLElement>('[role="tab"]')];
    expect(stages.length).toBeGreaterThan(0);
    for (const stage of stages) {
      const label = `${stage.closest('.pipe-card')?.querySelector('h3')?.textContent} / ${stage.textContent}`;
      fireEvent.click(stage);
      await settle();
      const panel = () => (stage.closest('.pipe-card')?.querySelector('[role="tabpanel"]') ?? document.createElement('div')) as HTMLElement;
      failures.push(...(await unwired(panel)).map((key) => `${label}: ${key}`));
    }
    expect(failures).toEqual([]);
  }, TIMEOUT_MS);
});
