import { describe, expect, it } from 'vitest';
import { learnedWords } from '../lib/correctionLearning';

describe('learning from corrections', () => {
  it('offers a misheard name the user fixed', () => {
    expect(learnedWords('Send the build to Wakar today.', 'Send the build to Waqar today.')).toEqual(['Waqar']);
    expect(learnedWords('It runs on open vino now', 'It runs on OpenVINO now')).toEqual(['OpenVINO']);
    expect(learnedWords('I built narivi', 'I built Narrivy')).toEqual(['Narrivy']);
  });

  it('ignores new text, everyday swaps and known words', () => {
    expect(learnedWords('Send it.', 'Send it. Also call Lahore office.')).toEqual([]);
    expect(learnedWords('I think so', 'I thought so')).toEqual([]);
    expect(learnedWords('Meet Ahmad', 'Meet Ahmed', ['ahmed'])).toEqual([]);
    expect(learnedWords('the report is late', 'the invoice is late')).toEqual([]);
  });
});
