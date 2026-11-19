// What an assistant chat about a meeting (or a video watched together) is
// given: the notes, then the transcript with what was on screen at its
// place, cut to what the model can hold.
import type { MeetingDetail } from './bridge';

/** About 24,000 tokens, two hours of speech. Language models run on the
 * GPU (D58), which reads this in well under a minute; the service's prompt
 * holds about 50,000 tokens with the chat besides. */
export const CONTEXT_CHARS = 96000;

/** When a transcript is longer than that: this share of the room goes to its
 * start, the rest to its end, and the notes cover the middle. */
const HEAD_SHARE = 1 / 3;

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
  const room = CONTEXT_CHARS - parts.join('\n\n').length;
  const size = (list: Array<{ text: string }>) => list.reduce((sum, line) => sum + line.text.length + 1, 0);
  if (lines.length && size(lines) <= room) {
    parts.push(`What was said and shown:\n${lines.map((line) => line.text).join('\n')}`);
  } else if (lines.length) {
    // The start and the newest part word for word; the middle is left out.
    const head: string[] = [];
    let used = 0;
    for (const line of lines) {
      if (used + line.text.length + 1 > room * HEAD_SHARE) break;
      head.push(line.text);
      used += line.text.length + 1;
    }
    const tail: string[] = [];
    for (let i = lines.length - 1; i >= head.length; i -= 1) {
      if (used + lines[i].text.length + 1 > room) break;
      tail.unshift(lines[i].text);
      used += lines[i].text.length + 1;
    }
    const skipped = lines.length - head.length - tail.length;
    const gap = `(… ${skipped} lines in the middle are left out here${notes ? '; the notes above cover all of it' : ''} …)`;
    parts.push(`What was said and shown (the first ${head.length} and last ${tail.length} of ${lines.length} lines):\n${[...head, gap, ...tail].join('\n')}`);
  }
  return parts.join('\n\n');
}
