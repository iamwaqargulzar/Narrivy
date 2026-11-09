// What an assistant chat about a meeting (or a video watched together) is
// given: the notes, then the transcript with what was on screen at its
// place, cut to what the model can hold.
import type { MeetingDetail } from './bridge';

/** About 6,000 tokens: room for the question and the answer besides. */
export const CONTEXT_CHARS = 24000;

export function clock(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  const two = (n: number) => String(n).padStart(2, '0');
  return h > 0 ? `${h}:${two(m)}:${two(sec)}` : `${two(m)}:${two(sec)}`;
}

export function meetingContext(detail: MeetingDetail, nameOf: (speaker: string) => string): string {
  const parts: string[] = [];
  const notes = detail.notes?.notes;
  if (notes) parts.push(`Notes:\n${notes.trim()}`);
  const lines: Array<{ at: number; text: string }> = detail.turns.filter((turn) => !turn.partial).map((turn) => ({
    at: turn.start_ms,
    text: `[${clock(turn.start_ms)}] ${nameOf(turn.speaker)}: ${turn.text}`,
  }));
  for (const slide of detail.slides ?? []) {
    if (slide.description && slide.description !== 'Video call.') {
      lines.push({
        at: slide.at_ms,
        text: `[${clock(slide.at_ms)}] (On screen) ${slide.description.replace(/\n+/g, ' ')}`,
      });
    }
  }
  lines.sort((a, b) => a.at - b.at);
  let used = parts.join('\n\n').length;
  const kept: string[] = [];
  for (const line of lines) {
    if (used + line.text.length + 1 > CONTEXT_CHARS) break;
    kept.push(line.text);
    used += line.text.length + 1;
  }
  if (kept.length) {
    const cut = kept.length < lines.length;
    parts.push(
      `What was said and shown${cut ? ` (the first ${kept.length} of ${lines.length} lines${notes ? '; the notes above cover all of it' : ''})` : ''}:\n${kept.join('\n')}`,
    );
  }
  return parts.join('\n\n');
}
