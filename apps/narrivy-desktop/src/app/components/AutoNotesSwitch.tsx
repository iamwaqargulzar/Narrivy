// Meeting options as switches. "Write notes automatically": the meeting's
// detailed notes start by themselves, so they are there when the user opens
// the meeting. "Go over a recording when it stops": off, a stopped
// recording is kept as heard (quick and light) until the user asks.
import { useEffect, useId, useState } from "react";
import { bridge, type MeetingOptions } from "../../lib/bridge";
import { t } from "../../i18n";
import { useErrorHandler } from "../../ui/primitives/Toast";

function OptionSwitch({
  label,
  hint,
  read,
  write,
}: {
  label: string;
  hint?: string;
  read: (o: MeetingOptions) => boolean | undefined;
  write: (on: boolean) => Promise<MeetingOptions>;
}) {
  const report = useErrorHandler();
  const labelId = useId();
  const [on, setOn] = useState<boolean | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    let alive = true;
    bridge.meetingOptions().then(
      (o) => alive && setOn(read(o) ?? null),
      () => alive && setOn(null),
    );
    return () => {
      alive = false;
    };
    // `read` is fixed for each switch.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Not known (an older service): no switch.
  if (on === null) return null;
  const toggle = async () => {
    setSaving(true);
    try {
      setOn(read(await write(!on)) ?? !on);
    } catch (err) {
      report(err, { retry: () => void toggle() });
    } finally {
      setSaving(false);
    }
  };
  return (
    <div className="meeting-auto-notes" title={hint}>
      <span id={labelId}>{label}</span>
      <button
        type="button"
        role="switch"
        className="switch"
        aria-checked={on}
        aria-labelledby={labelId}
        disabled={saving}
        onClick={() => void toggle()}
      >
        <span className="switch-thumb" />
      </button>
    </div>
  );
}

export function AutoNotesSwitch() {
  return (
    <OptionSwitch
      label={t("meetings.autoNotes")}
      read={(o) => o.auto_notes}
      write={(on) => bridge.setAutoNotes(on)}
    />
  );
}

export function ProcessOnStopSwitch() {
  return (
    <OptionSwitch
      label={t("meetings.processOnStop")}
      hint={t("meetings.processOnStopHint")}
      read={(o) => o.process_on_stop}
      write={(on) => bridge.setProcessOnStop(on)}
    />
  );
}
