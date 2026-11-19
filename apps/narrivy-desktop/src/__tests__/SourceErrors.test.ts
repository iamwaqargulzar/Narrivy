import { describe, expect, it } from 'vitest';
import { failureCode, liveTranscriptText, sourceErrorText } from '../lib/sourceErrors';

// rc33: switching the screen on in the Assistant's Live panel showed
// "The assistant could not answer (bad_request)". The service had said why
// (`screen_capture_unavailable`), in the message the app dropped.
describe('source and live-transcript failures', () => {
  it("reads the service's reason behind bad_request", () => {
    expect(failureCode({ code: 'bad_request', message: 'screen_capture_unavailable' })).toBe('screen_capture_unavailable');
    expect(failureCode({ code: 'bad_request', message: 'Something: with text' })).toBe('bad_request');
    expect(failureCode({ code: 'daemon_unavailable', message: 'meeting_not_recording' })).toBe('daemon_unavailable');
    expect(failureCode(null)).toBe('unavailable');
  });

  it('says what to do, never a raw code, for the reasons the service gives', () => {
    for (const code of ['screen_capture_unavailable', 'meeting_not_recording', 'meeting_no_microphone', 'meeting_recorder_missing']) {
      const text = sourceErrorText(code);
      expect(text).not.toContain(code);
      expect(text).not.toContain('could not answer');
    }
    expect(sourceErrorText('screen_capture_unavailable')).toContain('gstreamer1.0-pipewire');
    // Unknown reasons still show their code, for a report.
    expect(sourceErrorText('meeting_xyz')).toContain('meeting_xyz');
  });

  it('explains a live session whose sound is not transcribed', () => {
    // Nothing says a feature needs an add-on that no package ships (D57).
    expect(liveTranscriptText('meeting_worker_missing')).not.toContain('add-on');
    expect(liveTranscriptText('meeting_model_missing')).toContain('speech model');
    expect(liveTranscriptText('odd')).toContain('odd');
  });
});
