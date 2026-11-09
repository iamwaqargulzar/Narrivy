// Truthful activation wording: the exact user-managed shortcut command may be
// suggested only when the daemon reports the atomic command as available and
// no live portal listener; it must never be presented as configured or as
// working. The three activation facts stay separate.
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import type { Mock } from 'vitest';
import App from '../App';
import { bridge, type StatusSnapshot } from '../lib/bridge';
import {
  ATOMIC_COMMAND_CAPABILITY,
  PORTAL_INTERFACE_CAPABILITY,
  PORTAL_LISTENER_CAPABILITY,
  activationPresentation,
} from '../lib/activation';

vi.mock('../lib/bridge', async () => {
  const { createMockBridgeModule } = await import('../test/mockBridge');
  return createMockBridgeModule();
});

const ONBOARD_KEY = 'narrivy.onboarded.v1';
/** The installed command the mock daemon reports (`manual_command`). */
const USER_MANAGED_SHORTCUT_COMMAND = '/usr/bin/narrivy-cli toggle';

function statusWith(capabilities: string[]): StatusSnapshot {
  return { capabilities, active_session: null, daemon_build: 'narrivyd-test', installed_model: 'ready', protocol_minor: 11 };
}

/** First run's shortcut step, just before Try it. */
async function shortcutStep(): Promise<HTMLElement> {
  render(<App />);
  const dialog = await screen.findByRole('dialog', { name: /getting started/i });
  for (let i = 0; i < 3; i += 1) {
    const next = within(dialog).getByRole('button', { name: 'Next' });
    await waitFor(() => expect(next).toBeEnabled());
    fireEvent.click(next);
  }
  await within(dialog).findByText('Step 4 of 5');
  return dialog;
}

describe('ActivationShortcutTruth', () => {
  beforeEach(() => {
    window.localStorage.removeItem(ONBOARD_KEY);
  });

  it('activation_presentation_keeps_the_three_facts_separate', () => {
    const atomicOnly = activationPresentation(
      statusWith([
        `${ATOMIC_COMMAND_CAPABILITY}=available`,
        `${PORTAL_INTERFACE_CAPABILITY}=unavailable:interface_absent`,
        `${PORTAL_LISTENER_CAPABILITY}=unavailable:not_composed`,
      ]),
      USER_MANAGED_SHORTCUT_COMMAND,
    );
    expect(atomicOnly.atomicCommandAvailable).toBe(true);
    expect(atomicOnly.portalInterfaceAvailable).toBe(false);
    expect(atomicOnly.portalListenerReady).toBe(false);
    // An absent portal interface is exactly when the user-managed fallback is
    // suggested.
    expect(atomicOnly.suggestedCommand).toBe(USER_MANAGED_SHORTCUT_COMMAND);

    // A probed-present interface is still not a listener, so the fallback
    // command stays available.
    const interfacePresent = activationPresentation(
      statusWith([
        `${ATOMIC_COMMAND_CAPABILITY}=available`,
        `${PORTAL_INTERFACE_CAPABILITY}=available`,
        `${PORTAL_LISTENER_CAPABILITY}=unavailable:not_composed`,
      ]),
      USER_MANAGED_SHORTCUT_COMMAND,
    );
    expect(interfacePresent.portalInterfaceAvailable).toBe(true);
    expect(interfacePresent.portalListenerReady).toBe(false);
    expect(interfacePresent.suggestedCommand).toBe(USER_MANAGED_SHORTCUT_COMMAND);

    // A live listener means no manual command is suggested.
    const listenerReady = activationPresentation(
      statusWith([
        `${ATOMIC_COMMAND_CAPABILITY}=available`,
        `${PORTAL_INTERFACE_CAPABILITY}=available`,
        `${PORTAL_LISTENER_CAPABILITY}=ready`,
      ]),
      USER_MANAGED_SHORTCUT_COMMAND,
    );
    expect(listenerReady.portalListenerReady).toBe(true);
    expect(listenerReady.suggestedCommand).toBeNull();

    // Missing or unavailable atomic command: nothing is suggested.
    for (const capabilities of [
      [],
      [`${ATOMIC_COMMAND_CAPABILITY}=unavailable:not_composed`],
      [`${PORTAL_LISTENER_CAPABILITY}=unavailable:not_composed`],
      ['mock.asr', 'null.delivery'],
    ]) {
      const facts = activationPresentation(statusWith(capabilities), USER_MANAGED_SHORTCUT_COMMAND);
      expect(facts.atomicCommandAvailable).toBe(false);
      expect(facts.suggestedCommand).toBeNull();
    }

    // No command reported by the daemon: nothing is suggested.
    expect(
      activationPresentation(statusWith([`${ATOMIC_COMMAND_CAPABILITY}=available`])).suggestedCommand,
    ).toBeNull();

    // Unknown/unreported status fails closed.
    expect(activationPresentation(null).suggestedCommand).toBeNull();
    expect(activationPresentation(undefined).suggestedCommand).toBeNull();
  });

  it('onboarding_suggests_the_exact_command_without_claiming_it_is_configured', async () => {
    (bridge.status as Mock).mockResolvedValue(
      statusWith([
        'mock.asr',
        `${ATOMIC_COMMAND_CAPABILITY}=available`,
        `${PORTAL_INTERFACE_CAPABILITY}=unavailable:interface_absent`,
        `${PORTAL_LISTENER_CAPABILITY}=unavailable:not_composed`,
      ]),
    );

    const dialog = await shortcutStep();

    const command = await within(dialog).findByTestId('activation-shortcut-command');
    await waitFor(() =>
      expect(command.textContent).toContain(USER_MANAGED_SHORTCUT_COMMAND),
    );
    const rendered = dialog.textContent ?? '';
    expect(rendered).toContain('/usr/bin/narrivy-cli toggle');
    // Words that would overclaim: the user adds it, never configured/working.
    expect(rendered.toLowerCase()).toContain('does not let narrivy add a shortcut');
    expect(rendered.toLowerCase()).not.toContain('is configured');
    expect(rendered.toLowerCase()).not.toContain('shortcut works');
  });

  it('onboarding_omits_the_command_when_a_live_listener_is_reported', async () => {
    (bridge.status as Mock).mockResolvedValue(
      statusWith([
        'mock.asr',
        `${ATOMIC_COMMAND_CAPABILITY}=available`,
        `${PORTAL_INTERFACE_CAPABILITY}=available`,
        `${PORTAL_LISTENER_CAPABILITY}=ready`,
      ]),
    );

    const dialog = await shortcutStep();
    await waitFor(() =>
      expect(within(dialog).queryByTestId('activation-shortcut-command')).toBeNull(),
    );
    expect(dialog.textContent ?? '').not.toContain('/usr/bin/narrivy-cli toggle');
  });

  it('onboarding_omits_the_command_when_atomic_toggle_is_unavailable', async () => {
    (bridge.status as Mock).mockResolvedValue(
      statusWith([
        'mock.asr',
        `${ATOMIC_COMMAND_CAPABILITY}=unavailable:not_composed`,
        `${PORTAL_LISTENER_CAPABILITY}=unavailable:not_composed`,
      ]),
    );

    const dialog = await shortcutStep();
    await waitFor(() =>
      expect(within(dialog).queryByTestId('activation-shortcut-command')).toBeNull(),
    );
    expect(dialog.textContent ?? '').not.toContain('/usr/bin/narrivy-cli toggle');
  });
});
