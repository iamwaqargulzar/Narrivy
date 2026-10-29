// Meeting options as switches. "Write notes automatically": the meeting's
// detailed notes start by themselves, so they are there when the user opens
// the meeting. "Go over a recording when it stops": off, a stopped
// recording is kept as heard (quick and light) until the user asks.
import { useEffect, useState } from "react";
import { bridge, type MeetingOptions } from "../../lib/bridge";
import { t } from "../../i18n";
import { useErrorHandler } from "../../ui/primitives/Toast";
import { Icon, type IconName } from "../../ui/icons";

function OptionSwitch({
  icon,
  label,
  short,
  hint,
  read,
  write,
}: {
  icon: IconName;
  label: string;
  /** The pill's own words; `label` names it for screen readers. */
  short: string;
  hint?: string;
  read: (o: MeetingOptions) => boolean | undefined;
  write: (on: boolean) => Promise<MeetingOptions>;
}) {
  const report = useErrorHandler();
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
    <span className="meeting-option" title={hint ?? label}>
      <button
        type="button"
        role="switch"
        className="meeting-pill"
        aria-checked={on}
        aria-label={label}
        disabled={saving}
        onClick={() => void toggle()}
      >
        <Icon name={icon} />
        {short}
      </button>
    </span>
  );
}

export function AutoNotesSwitch() {
  return (
    <OptionSwitch
      icon="pencil"
      label={t("meetings.autoNotes")}
      short={t("meetings.autoNotesShort")}
      read={(o) => o.auto_notes}
      write={(on) => bridge.setAutoNotes(on)}
    />
  );
}

export function ProcessOnStopSwitch() {
  return (
    <OptionSwitch
      icon="refresh"
      label={t("meetings.processOnStop")}
      short={t("meetings.processOnStopShort")}
      hint={t("meetings.processOnStopHint")}
      read={(o) => o.process_on_stop}
      write={(on) => bridge.setProcessOnStop(on)}
    />
  );
}
