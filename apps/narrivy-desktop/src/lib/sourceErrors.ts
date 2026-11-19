import { hasMessage, t } from '../i18n';

/**
 * The reason a request failed. The service rejects meeting and source
 * requests as `bad_request` with the actual reason (`screen_capture_unavailable`,
 * `meeting_not_recording`, ...) as the message; that reason is what the
 * user needs to hear, not `bad_request`.
 */
export function failureCode(error: unknown): string {
  const { code, message } = (error ?? {}) as { code?: unknown; message?: unknown };
  if (code === 'bad_request' && typeof message === 'string' && /^[a-z][a-z_]{0,63}$/.test(message)) return message;
  return typeof code === 'string' && code ? code : 'unavailable';
}

/** What to tell the user when switching the screen, sound or microphone failed. */
export function sourceErrorText(code: string): string {
  return hasMessage(`sources.error.${code}`) ? t(`sources.error.${code}`) : t('assistant.error.other', { code });
}

/** Why the sound being recorded is not turned into text as it is heard. */
export function liveTranscriptText(code: string): string {
  return hasMessage(`assistant.live.noTranscript.${code}`)
    ? t(`assistant.live.noTranscript.${code}`)
    : t('assistant.live.noTranscript.other', { code });
}
