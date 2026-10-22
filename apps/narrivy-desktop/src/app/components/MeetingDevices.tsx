// Which microphone and which output a meeting records. The microphone hears
// the people in the room; the output's sound is the other side of an online
// call. Shows what is recording now, or what a meeting started now would
// record, so the user never has to guess.
import { useEffect, useState, type ReactNode } from "react";
import { bridge, type MeetingOptions } from "../../lib/bridge";
import { t } from "../../i18n";
import { useErrorHandler } from "../../ui/primitives/Toast";

type Device = { name: string; description: string };

function DeviceSelect({
  label,
  chosen,
  devices,
  defaultLabel,
  disabled,
  onPick,
  first,
}: {
  label: string;
  chosen: string;
  devices: Device[];
  defaultLabel: string;
  disabled: boolean;
  onPick: (name: string) => void;
  /** An option listed before the default. */
  first?: { value: string; label: string };
}) {
  const listed = devices.some((d) => d.name === chosen);
  return (
    <label className="meeting-device">
      <span>{label}</span>
      <select
        className="pipe-select"
        value={chosen}
        disabled={disabled}
        onChange={(e) => onPick(e.target.value)}
      >
        {first ? <option value={first.value}>{first.label}</option> : null}
        <option value="">{defaultLabel}</option>
        {devices.map((d) => (
          <option key={d.name} value={d.name}>
            {d.description}
          </option>
        ))}
        {chosen && !listed && chosen !== first?.value ? (
          <option value={chosen}>{t("meetings.devices.unplugged")}</option>
        ) : null}
      </select>
    </label>
  );
}

/** Not a PipeWire name (`!` is never in one): that side is off. */
const CALL_OFF = "!off";

export function MeetingDevices({
  recording,
  extra,
}: {
  recording: boolean;
  /** Shown at the end of the row (the notes switch). */
  extra?: ReactNode;
}) {
  const report = useErrorHandler();
  const [options, setOptions] = useState<MeetingOptions | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    let alive = true;
    const load = () =>
      bridge.meetingOptions().then(
        (o) => alive && setOptions(o),
        () => alive && setOptions(null),
      );
    void load();
    // Devices come and go (a headset plugged in): keep the lists current.
    const timer = window.setInterval(() => void load(), 5000);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, [recording]);

  // An older service lists no devices.
  if (!options || options.microphones === undefined) return null;
  const pick = async (choice: {
    microphone?: string;
    speaker?: string;
    call_audio?: boolean;
    room_audio?: boolean;
  }) => {
    setSaving(true);
    try {
      setOptions(await bridge.setMeetingDevices(choice));
    } catch (err) {
      report(err, { retry: () => void pick(choice) });
    } finally {
      setSaving(false);
    }
  };
  const mic = options.microphone_in_use || t("meetings.devices.none");
  const callOff = options.call_audio === false;
  const micOff = options.room_audio === false;
  const now = micOff
    ? t(
        recording
          ? "meetings.devices.recordingCallOnly"
          : "meetings.devices.willRecordCallOnly",
        { out: options.speaker_in_use || t("meetings.devices.none") },
      )
    : callOff
      ? t(
          recording
            ? "meetings.devices.recordingMicOnly"
            : "meetings.devices.willRecordMicOnly",
          { mic },
        )
      : t(
          recording
            ? "meetings.devices.recording"
            : "meetings.devices.willRecord",
          { mic, out: options.speaker_in_use || t("meetings.devices.none") },
        );
  return (
    <div className="meeting-devices">
      <div className="meeting-devices-pick">
        <DeviceSelect
          label={t("meetings.devices.microphone")}
          chosen={micOff ? CALL_OFF : (options.microphone ?? "")}
          devices={options.microphones}
          defaultLabel={t("meetings.devices.micDefault")}
          disabled={recording || saving}
          // Not both off: a meeting needs something to record.
          first={
            callOff
              ? undefined
              : { value: CALL_OFF, label: t("meetings.devices.micOff") }
          }
          onPick={(microphone) =>
            void pick(
              microphone === CALL_OFF
                ? { room_audio: false }
                : { microphone, room_audio: true },
            )
          }
        />
        <DeviceSelect
          label={t("meetings.devices.speaker")}
          chosen={callOff ? CALL_OFF : (options.speaker ?? "")}
          devices={options.speakers ?? []}
          defaultLabel={t("meetings.devices.speakerDefault")}
          disabled={recording || saving}
          first={
            micOff
              ? undefined
              : { value: CALL_OFF, label: t("meetings.devices.callOff") }
          }
          onPick={(speaker) =>
            void pick(
              speaker === CALL_OFF
                ? { call_audio: false }
                : { speaker, call_audio: true },
            )
          }
        />
        <ScreenSwitch
          on={Boolean(options.screen)}
          available={Boolean(options.screen_available)}
          disabled={recording || saving}
          onToggle={async (on) => {
            setSaving(true);
            try {
              setOptions(await bridge.setMeetingScreen(on));
            } catch (err) {
              report(err);
            } finally {
              setSaving(false);
            }
          }}
        />
        {extra}
      </div>
      <p className="source meeting-devices-now" role="status">
        {now}
        {recording ? ` ${t("meetings.devices.locked")}` : null}
      </p>
    </div>
  );
}

/** "Keep what is on screen": pictures of the shared screen, described by
 * the assistant's model after the meeting (D34). */
function ScreenSwitch({
  on,
  available,
  disabled,
  onToggle,
}: {
  on: boolean;
  available: boolean;
  disabled: boolean;
  onToggle: (on: boolean) => void;
}) {
  return (
    <div
      className="meeting-auto-notes"
      title={available ? t("meetings.screen.hint") : t("meetings.screen.unavailable")}
    >
      <span id="meeting-screen-label">{t("meetings.screen.label")}</span>
      <button
        type="button"
        role="switch"
        className="switch"
        aria-checked={on && available}
        aria-labelledby="meeting-screen-label"
        disabled={disabled || !available}
        onClick={() => onToggle(!on)}
      >
        <span className="switch-thumb" />
      </button>
    </div>
  );
}
