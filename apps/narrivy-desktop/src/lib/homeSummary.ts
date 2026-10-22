// Short, friendly labels for the Home summary chips. Each label is derived
// from the same resolved value the Setup details show in full, so a chip can
// never claim more than the daemon reported.
import type { ResolvedCardValue } from './resolved';

export type ChipTone = 'neutral' | 'ok' | 'warn';

export interface SummaryChip {
  label: string;
  /** The full resolved value, shown as the chip's tooltip. */
  detail: string;
  tone: ChipTone;
}

function plain(value: ResolvedCardValue): string {
  return value.resolved.replace(/^Auto → /, '');
}

export function micChip(value: ResolvedCardValue): SummaryChip {
  const text = plain(value);
  if (/^No microphone/i.test(text)) return { label: 'No microphone', detail: text, tone: 'warn' };
  if (/^Chosen/i.test(text)) return { label: 'Chosen microphone', detail: text, tone: 'neutral' };
  return { label: 'Default microphone', detail: text, tone: 'neutral' };
}

export function modelChip(value: ResolvedCardValue): SummaryChip {
  const text = plain(value);
  if (/not installed/i.test(text)) return { label: 'Model not installed', detail: text, tone: 'warn' };
  if (/installing/i.test(text)) return { label: 'Installing model…', detail: text, tone: 'warn' };
  if (/failed|unavailable|not available/i.test(text)) return { label: 'Model unavailable', detail: text, tone: 'warn' };
  if (/test model/i.test(text)) return { label: 'Test model', detail: text, tone: 'neutral' };
  if (/manually/i.test(text)) return { label: 'Chosen model', detail: text, tone: 'neutral' };
  return { label: 'Local model', detail: text, tone: 'neutral' };
}

export function computeChip(value: ResolvedCardValue): SummaryChip {
  const text = plain(value);
  if (/unavailable|not available/i.test(text)) return { label: 'No compute', detail: text, tone: 'warn' };
  const device = text.split(' (')[0] ?? text;
  return { label: device, detail: text, tone: 'neutral' };
}

export function deliveryChip(value: ResolvedCardValue): SummaryChip {
  const text = plain(value);
  if (/null sink|^unavailable/i.test(text)) return { label: 'No text delivery', detail: text, tone: 'warn' };
  if (/clipboard paste/i.test(text)) return { label: 'Pastes text', detail: text, tone: 'neutral' };
  if (/typing helper unavailable|inbox recovery only|no keyboard paste/i.test(text)) {
    return { label: 'Saves to Inbox', detail: text, tone: 'neutral' };
  }
  if (/inbox recovery/i.test(text)) return { label: 'Types text · Inbox backup', detail: text, tone: 'neutral' };
  if (/router/i.test(text)) return { label: 'Types text', detail: text, tone: 'neutral' };
  return { label: 'Text delivery', detail: text, tone: 'neutral' };
}
