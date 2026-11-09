// Minimal message-ID i18n architecture (CONTRACT.md: day one).
// All user-visible strings resolve through t(); missing ids fall back to the id itself.
import en from './en.json';

const MESSAGES = en as Record<string, string>;

export function t(id: string, vars?: Record<string, string | number>): string {
  let out = MESSAGES[id] ?? id;
  if (vars) {
    for (const [k, v] of Object.entries(vars)) {
      out = out.split(`{${k}}`).join(String(v));
    }
  }
  return out;
}

/** Whether a message exists, for copy chosen by an id the service sends. */
export function hasMessage(id: string): boolean {
  return Object.prototype.hasOwnProperty.call(MESSAGES, id);
}
