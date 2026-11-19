import { describe, expect, it } from 'vitest';
import type { MeetingDetail } from '../lib/bridge';
import { CONTEXT_CHARS, meetingContext } from '../lib/meetingContext';

function detail(lines: number, notes?: string): MeetingDetail {
  return {
    turns: Array.from({ length: lines }, (_, i) => ({
      speaker: 'Person 1',
      start_ms: i * 10_000,
      end_ms: i * 10_000 + 9_000,
      text: `line ${i} ${'word '.repeat(30)}`,
      partial: false,
    })),
    slides: [],
    notes: notes ? { notes } : undefined,
  } as unknown as MeetingDetail;
}

describe('meetingContext', () => {
  it('gives an hour of transcript whole', () => {
    // 360 lines of about 170 characters: an hour, about 61,000 characters.
    const text = meetingContext(detail(360), (s) => s);
    expect(text).toContain('line 0 ');
    expect(text).toContain('line 359 ');
    expect(text).not.toContain('left out');
  });

  it('keeps the start and the end of a longer one, within the budget', () => {
    const text = meetingContext(detail(2000, 'All of it in short.'), (s) => s);
    expect(text.length).toBeLessThanOrEqual(CONTEXT_CHARS + 200);
    expect(text).toContain('line 0 ');
    expect(text).toContain('line 1999 ');
    expect(text).toContain('the notes above cover all of it');
    expect(text).toMatch(/lines in the middle are left out/);
  });
});
