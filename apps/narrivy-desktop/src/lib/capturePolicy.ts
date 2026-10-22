// Render the capture window and typed capture end reason the daemon reports.
//
// The daemon/platform boundary owns the real capture policy: it enforces the
// bounded window and reports both the effective limit (`capture.window_seconds`)
// and the documented production maximum (`capture.max_seconds`) as
// capabilities. This module only parses those reported numbers and maps the
// typed, content-free `capture_end` wire tag to a notice key. The frontend
// never invents, lengthens or duplicates an OS-side capture limit.
import type { StatusSnapshot } from './bridge';

export const CAPTURE_WINDOW_CAPABILITY_PREFIX = 'capture.window_seconds=';
export const CAPTURE_MAX_CAPABILITY_PREFIX = 'capture.max_seconds=';

function reportedSeconds(status: StatusSnapshot | null, prefix: string): number | null {
  const token = (status?.capabilities ?? []).find((cap) => cap.startsWith(prefix));
  if (!token) return null;
  const seconds = Number(token.slice(prefix.length));
  // Reject an absent, non-numeric, non-finite or non-positive report rather
  // than rendering a made-up limit.
  return Number.isFinite(seconds) && seconds > 0 ? seconds : null;
}

/** Effective capture window the daemon will enforce, when it reports one. */
export function reportedCaptureLimitSeconds(status: StatusSnapshot | null): number | null {
  return reportedSeconds(status, CAPTURE_WINDOW_CAPABILITY_PREFIX);
}

/** Documented production maximum, reported independently of a short probe. */
export function documentedCaptureMaxSeconds(status: StatusSnapshot | null): number | null {
  return reportedSeconds(status, CAPTURE_MAX_CAPABILITY_PREFIX);
}

/** Closed wire tags of `narrivy_types::CaptureEnd`. */
export const CAPTURE_END_REASONS = [
  'finalization_requested',
  'endpoint_detected',
  'safety_limit',
  'source_ended',
] as const;

export type CaptureEndReason = (typeof CAPTURE_END_REASONS)[number];

export function isCaptureEndReason(value: unknown): value is CaptureEndReason {
  return (
    typeof value === 'string' && (CAPTURE_END_REASONS as readonly string[]).includes(value)
  );
}

export interface CaptureEndNotice {
  key: string;
  values: Record<string, string | number>;
}

/**
 * Truthful notice for how a finished session's capture ended. A graceful Stop
 * (`finalization_requested`) or an independent endpoint (`endpoint_detected`)
 * is the ordinary user-driven outcome and needs no extra explanation; the
 * documented maximum and a source that ended on its own are surfaced.
 */
export function captureEndNotice(
  status: StatusSnapshot | null,
  captureEnd: string | null | undefined,
): CaptureEndNotice | null {
  switch (captureEnd) {
    case 'safety_limit': {
      const seconds = documentedCaptureMaxSeconds(status) ?? reportedCaptureLimitSeconds(status);
      return seconds === null
        ? { key: 'dictation.endSafetyLimitUnknown', values: {} }
        : { key: 'dictation.endSafetyLimit', values: { seconds } };
    }
    case 'source_ended':
      return { key: 'dictation.endSourceEnded', values: {} };
    default:
      // Includes `finalization_requested`, `endpoint_detected`, absent and
      // unrecognized tags: never invent an end reason.
      return null;
  }
}
