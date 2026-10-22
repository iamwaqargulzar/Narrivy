// The Compute tab must render daemon truth only: the same resolveCompute() /
// resolveDelivery() values Home renders, inventories taken from the advertised
// capability strings, and no control that looks functional while changing
// nothing. Unavailable controls keep role="switch" + aria-checked and add
// aria-disabled with an accessible reason derived from the daemon's own report.
import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, within } from '@testing-library/react';
import { AutomationCard } from '../app/components/SettingsCompute/AutomationCard';
import { ComputeCard } from '../app/components/SettingsCompute/ComputeCard';
import { TextDeliveryCard } from '../app/components/SettingsCompute/TextDeliveryCard';
import type { StatusSnapshot } from '../lib/bridge';
import { resolveCompute, resolveDelivery } from '../lib/resolved';
import { t } from '../i18n';
import {
  INSTALLED_RECOVERY_AWAITING_STATUS,
  ROUTED_RECOVERY_CAPABILITIES,
  routedRecoveryStatus,
} from '../test/installedRecoveryFixtures';

const controls = vi.hoisted(() => ({
  status: null as StatusSnapshot | null,
}));

vi.mock('../state/StatusContext', () => ({
  useDaemonStatus: () => ({ status: controls.status }),
}));

const SWITCHES = [
  'compute-vulkan',
  'compute-auto-fallback',
  'delivery-restore-clipboard',
  'delivery-detect-clipboard-changes',
  'delivery-keep-transcript',
];

const FABRICATED = /NVIDIA|Intel\s+NPU|Intel\s+Arc|OpenVINO|Whisper|ydotool|uinput|libei|XWayland|VS\s*Code|Firefox|Discord|Terminal Paste|Clipboard Only|Direct Insert/;

/** Automation-policy tokens the awaiting-model daemon really advertises. */
const AUTOMATION_TOKENS = [
  'policy.local-only=enabled',
  'cancellation.shared=available',
  'recovery.behavioral_qualification=unqualified',
  'recovery.static-readiness=ready',
];

function computeCard() {
  return screen.getByTestId('compute-card');
}

function deliveryCard() {
  return screen.getByTestId('text-delivery-card');
}

function tokenNamesIn(id: string): string[] {
  return within(document.getElementById(id) as HTMLElement)
    .getAllByTestId(`${id}-item`)
    .map((item) => item.querySelector('.token-name')?.textContent ?? '');
}

describe('Compute tab reports the daemon', () => {
  it('renders the exact resolveCompute value and its capability source', () => {
    controls.status = {
      capabilities: [
        'asr.transcribe-cpp.cpu=available',
        'asr.transcribe-cpp.runtime=resident-worker',
      ],
      active_session: null,
    };
    const expected = resolveCompute(controls.status, 'auto');
    expect(expected.resolved).toBe('Auto → CPU (transcribe.cpp, resident-worker)');

    render(<ComputeCard />);
    expect(within(computeCard()).getByTestId('compute-resolved')).toHaveTextContent(
      t('settings.compute.currentResolved', { value: expected.resolved }),
    );
    expect(within(computeCard()).getByTestId('compute-source')).toHaveTextContent(
      t('settings.compute.derivedFrom', { source: expected.source }),
    );
    expect(computeCard().textContent).not.toMatch(FABRICATED);
  });

  it('reports an unresolvable compute engine as unavailable instead of naming one', () => {
    controls.status = INSTALLED_RECOVERY_AWAITING_STATUS;
    const expected = resolveCompute(controls.status, 'auto');
    expect(expected.resolved).toBe('Auto → Unavailable (installed_model_missing)');

    render(<ComputeCard />);
    expect(within(computeCard()).getByTestId('compute-resolved')).toHaveTextContent(
      t('settings.compute.currentResolved', { value: expected.resolved }),
    );
    expect(tokenNamesIn('compute-reported-tokens')).toEqual([
      'asr.transcribe-cpp.cpu=unavailable:installed_model_missing',
      'asr.transcribe-cpp.runtime=resident-worker',
    ]);
  });

  it('lists only capability strings the daemon advertised, marking the cited one', () => {
    const status = routedRecoveryStatus();
    controls.status = status;
    const cited = resolveCompute(status, 'auto').source.replace('capability: ', '');

    render(<ComputeCard />);
    const tokens = tokenNamesIn('compute-reported-tokens');
    expect(tokens.length).toBeGreaterThan(0);
    expect(tokens.every((token) => status.capabilities.includes(token))).toBe(true);
    expect(cited).toBe('asr.transcribe-cpp.cpu=available');
    expect(within(document.getElementById('compute-reported-tokens') as HTMLElement).getAllByTestId('compute-reported-tokens-item')).toHaveLength(tokens.length);
    expect(
      within(document.getElementById('compute-reported-tokens') as HTMLElement)
        .getAllByTestId('compute-reported-tokens-item')
        .filter((item) => item.getAttribute('data-cited') === 'true')
        .map((item) => item.querySelector('.token-name')?.textContent),
    ).toEqual([cited]);
  });

  it('never reorders or selects a device the daemon did not advertise', () => {
    controls.status = routedRecoveryStatus();
    render(<ComputeCard />);

    const group = within(computeCard()).getByRole('radiogroup', { name: t('settings.compute.deviceSelection') });
    for (const radio of within(group).getAllByRole('radio')) {
      expect(radio).toBeDisabled();
      expect(radio).toHaveAttribute('aria-disabled', 'true');
    }
    const reason = document.getElementById('compute-device-reason') as HTMLElement;
    expect(group).toHaveAttribute('aria-describedby', reason.id);
    expect(reason.textContent).toBe(t('settings.compute.deviceUnavailable'));
    expect(computeCard().querySelectorAll('ol,ul')).toHaveLength(1);
    expect(computeCard().textContent).not.toMatch(/Move .* (up|down)|Drag to reorder/);
    expect(within(computeCard()).queryByRole('list', { name: /priority|fallback/i })).not.toBeInTheDocument();
  });
});

describe('Delivery tab reports the daemon', () => {
  it('renders the exact resolveDelivery value instead of a fallback chain', () => {
    const status = routedRecoveryStatus();
    controls.status = status;
    const expected = resolveDelivery(status, 'auto');
    expect(expected.resolved).toContain('target-aware routing');

    render(<TextDeliveryCard />);
    expect(within(deliveryCard()).getByTestId('delivery-resolved')).toHaveTextContent(expected.resolved);
    expect(within(deliveryCard()).getByTestId('delivery-source')).toHaveTextContent(
      t('settings.compute.derivedFrom', { source: expected.source }),
    );
    expect(deliveryCard().textContent).not.toMatch(FABRICATED);
  });

  it('lists only advertised delivery tokens, in the order the daemon reported them', () => {
    const status = routedRecoveryStatus();
    controls.status = status;
    render(<TextDeliveryCard />);

    const tokens = tokenNamesIn('delivery-reported-tokens');
    expect(tokens.every((token) => status.capabilities.includes(token))).toBe(true);
    expect(tokens).toEqual(ROUTED_RECOVERY_CAPABILITIES.filter((token) => token.startsWith('delivery.')));
  });

  it('names no application and offers no way to invent one', () => {
    controls.status = routedRecoveryStatus();
    render(<TextDeliveryCard />);

    expect(within(deliveryCard()).getByTestId('delivery-per-app')).toHaveTextContent(
      t('settings.delivery.perAppUnavailable'),
    );
    expect(within(deliveryCard()).queryByRole('table')).not.toBeInTheDocument();
    expect(within(deliveryCard()).queryByRole('combobox')).not.toBeInTheDocument();
    expect(deliveryCard().textContent).not.toMatch(/Add Override|Per-application overrides\s*VS/m);
  });
});

describe('unavailable controls', () => {
  it('keeps every switch role, aria-checked and disabled with a reason', () => {
    controls.status = routedRecoveryStatus();
    render(
      <>
        <ComputeCard />
        <TextDeliveryCard />
        <AutomationCard />
      </>,
    );

    for (const id of SWITCHES) {
      const control = screen.getByTestId(id);
      expect(control, id).toHaveAttribute('role', 'switch');
      expect(control, id).toHaveAttribute('aria-checked');
      expect(control, id).toBeDisabled();
      expect(control, id).toHaveAttribute('aria-disabled', 'true');
      const reason = document.getElementById(`${id}-reason`) as HTMLElement;
      expect(control, id).toHaveAttribute('aria-describedby', reason.id);
      expect(reason.textContent?.trim().length ?? 0, id).toBeGreaterThan(0);
      expect(reason.textContent, id).toContain('Unavailable');
    }
  });

  it('never changes a switch state when a disabled control is activated', () => {
    controls.status = routedRecoveryStatus();
    render(<TextDeliveryCard />);
    const control = screen.getByTestId('delivery-restore-clipboard');
    const before = control.getAttribute('aria-checked');
    fireEvent.click(control);
    fireEvent.keyDown(control, { key: ' ' });
    expect(control.getAttribute('aria-checked')).toBe(before);
  });

  it('states that no capability is reported when the daemon says nothing', () => {
    controls.status = INSTALLED_RECOVERY_AWAITING_STATUS;
    render(<TextDeliveryCard />);

    expect(document.getElementById('delivery-detect-clipboard-changes-reason')).toHaveTextContent(
      t('settings.compute.controlNoCapability'),
    );
    expect(document.getElementById('delivery-keep-transcript-reason')).toHaveTextContent(
      t('settings.compute.controlNoCapability'),
    );
    // The one clipboard behaviour the daemon does report is quoted verbatim.
    expect(document.getElementById('delivery-restore-clipboard-reason')).toHaveTextContent(
      t('settings.compute.controlNoCapability'),
    );
  });

  it('derives both the reason and the state from the token the daemon reported', () => {
    const restore = 'delivery.clipboard-restore=unavailable:clipboard_not_authorized';
    controls.status = {
      capabilities: [restore, 'compute.vulkan=unavailable:not_supported'],
      active_session: null,
    };
    render(
      <>
        <ComputeCard />
        <TextDeliveryCard />
      </>,
    );

    expect(document.getElementById('delivery-restore-clipboard-reason')).toHaveTextContent(
      t('settings.compute.controlUnavailable', { reported: restore }),
    );
    expect(document.getElementById('compute-vulkan-reason')).toHaveTextContent(
      t('settings.compute.controlUnavailable', { reported: 'compute.vulkan=unavailable:not_supported' }),
    );
    expect(screen.getByTestId('delivery-restore-clipboard')).toHaveAttribute('aria-checked', 'false');

    controls.status = { capabilities: ['delivery.clipboard-restore=available'], active_session: null };
    render(<TextDeliveryCard />);
    expect(screen.getAllByTestId('delivery-restore-clipboard')[1]).toHaveAttribute('aria-checked', 'true');
  });
});

describe('automation tab reports the daemon', () => {
  it('renders mode and optimization as unavailable and lists only policy tokens', () => {
    const status = INSTALLED_RECOVERY_AWAITING_STATUS;
    controls.status = status;
    render(<AutomationCard />);

    const card = screen.getByTestId('automation-card');
    for (const radio of within(card).getAllByRole('radio')) {
      expect(radio).toBeDisabled();
    }
    for (const pill of within(card).getAllByRole('button')) {
      expect(pill).toBeDisabled();
      expect(pill).toHaveAttribute('aria-disabled', 'true');
      expect(pill).toHaveAttribute('aria-pressed', 'false');
    }
    expect(document.getElementById('automation-mode-reason')).toHaveTextContent(
      t('settings.compute.controlUnavailable', {
        reported: AUTOMATION_TOKENS.join(', '),
      }),
    );
    const tokens = tokenNamesIn('automation-reported-tokens');
    expect(tokens).toEqual(AUTOMATION_TOKENS);
    expect(tokens.every((token) => status.capabilities.includes(token))).toBe(true);
  });

  it('reports an empty capability group instead of an empty control', () => {
    controls.status = { capabilities: ['mic.available'], active_session: null };
    render(<AutomationCard />);
    expect(screen.getByTestId('automation-reported-tokens-empty')).toHaveTextContent(
      t('settings.compute.noCapabilitiesReported'),
    );
    expect(document.getElementById('automation-mode-reason')).toHaveTextContent(
      t('settings.compute.controlNoCapability'),
    );
  });
});
