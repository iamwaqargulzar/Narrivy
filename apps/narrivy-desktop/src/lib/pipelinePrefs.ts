// Per-computer presentation preferences for the Pipelines screen: the order
// the user dragged pipelines into and the names they gave them. These are
// this app's own preferences (shortcuts live in the desktop's settings); the
// daemon still owns each pipeline's id, stages and version. Stored in
// localStorage; a missing or unreadable store falls back to the daemon order
// and names.
import { USER_MANAGED_SHORTCUT_COMMAND } from './activation';

export const PIPELINE_PREFS_KEY = 'narrivy.pipelines.prefs.v1';

export interface PipelinePrefs {
  order: string[];
  names: Record<string, string>;
}

const EMPTY: PipelinePrefs = { order: [], names: {} };
export const MAX_NAME_LENGTH = 40;
const TAG = /^[A-Za-z]{2,8}(-[A-Za-z0-9]{1,8})*$/;

export function loadPipelinePrefs(): PipelinePrefs {
  try {
    const raw = window.localStorage.getItem(PIPELINE_PREFS_KEY);
    if (!raw) return EMPTY;
    const parsed = JSON.parse(raw) as Partial<PipelinePrefs>;
    const order = Array.isArray(parsed.order) ? parsed.order.filter((id): id is string => typeof id === 'string') : [];
    const names: Record<string, string> = {};
    for (const [id, name] of Object.entries(parsed.names ?? {})) {
      if (typeof name === 'string' && name.trim()) names[id] = name.trim().slice(0, MAX_NAME_LENGTH);
    }
    return { order, names };
  } catch {
    return EMPTY;
  }
}

export function savePipelinePrefs(prefs: PipelinePrefs): void {
  try {
    window.localStorage.setItem(PIPELINE_PREFS_KEY, JSON.stringify(prefs));
  } catch {
    // storage unavailable; the preference lasts for this session only
  }
}

/** Pipelines in the user's order; ones the user never placed keep daemon order at the end. */
export function orderPipelines<T extends { id: string }>(pipelines: T[], order: string[]): T[] {
  const rank = new Map(order.map((id, index) => [id, index]));
  return [...pipelines].sort((a, b) => {
    const ra = rank.get(a.id) ?? Number.MAX_SAFE_INTEGER;
    const rb = rank.get(b.id) ?? Number.MAX_SAFE_INTEGER;
    return ra - rb;
  });
}

/** Move `id` to `target`'s position; returns the new full order. */
export function moveInOrder(ids: string[], id: string, targetIndex: number): string[] {
  const from = ids.indexOf(id);
  if (from < 0) return ids;
  const next = ids.filter((item) => item !== id);
  const to = Math.max(0, Math.min(next.length, targetIndex));
  next.splice(to, 0, id);
  return next;
}

/**
 * The command a desktop keyboard shortcut should run to toggle dictation with
 * this pipeline (and, optionally, these languages). Every flag here is one the
 * CLI accepts; tags are validated so the command is always safe to paste.
 */
export function shortcutCommand(
  pipelineId: string,
  spoken = 'auto',
  output = 'auto',
  base: string = USER_MANAGED_SHORTCUT_COMMAND,
): string {
  const parts = [base, '--pipeline', pipelineId];
  if (spoken !== 'auto' && TAG.test(spoken)) parts.push('--language', spoken);
  if (output !== 'auto' && TAG.test(output)) parts.push('--output-language', output);
  return parts.join(' ');
}
