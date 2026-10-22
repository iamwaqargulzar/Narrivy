// "Ignore this computer's sound": the microphone without what the speakers
// play (videos, people on a call), so other voices stay out of dictation and
// off the microphone side of a meeting.
import { useEffect, useState } from "react";
import { bridge, type AudioOptions } from "../../../lib/bridge";
import { t } from "../../../i18n";
import { useErrorHandler } from "../../../ui/primitives/Toast";
import "./SettingsCompute.css";

export function EchoCancelCard() {
  const report = useErrorHandler();
  const [options, setOptions] = useState<AudioOptions | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    let alive = true;
    bridge
      .audioOptions()
      .then((found) => {
        if (alive) setOptions(found);
      })
      .catch(() => {
        if (alive) setOptions(null);
      });
    return () => {
      alive = false;
    };
  }, []);

  const toggle = async () => {
    if (!options) return;
    setSaving(true);
    try {
      setOptions(await bridge.setEchoCancel(!options.echo_cancel));
      // Starting takes a moment: read again for whether it is working.
      setTimeout(() => {
        bridge.audioOptions().then(setOptions, () => undefined);
      }, 2500);
    } catch (err) {
      report(err, { retry: () => void toggle() });
    } finally {
      setSaving(false);
    }
  };

  const on = options?.echo_cancel ?? false;
  const state = !options
    ? t("echo.unavailable")
    : !on
      ? t("echo.off")
      : options.echo_cancel_active
        ? t("echo.active")
        : t("echo.starting");
  return (
    <section className="settings-card" aria-labelledby="echo-heading">
      <div className="settings-card-header settings-card-header-row">
        <span id="echo-heading">{t("echo.title")}</span>
        <button
          type="button"
          role="switch"
          className="switch"
          aria-checked={on}
          aria-labelledby="echo-heading"
          disabled={!options || saving}
          onClick={() => void toggle()}
        >
          <span className="switch-thumb" />
        </button>
      </div>
      <p className="settings-card-desc">{t("echo.desc")}</p>
      <p className="source" aria-live="polite">
        {state}
      </p>
    </section>
  );
}
