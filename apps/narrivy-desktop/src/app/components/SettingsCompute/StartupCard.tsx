// Start at login, and whether Narrivy then opens its window or waits in the
// tray. Turning it on also starts the Narrivy service at login.
import { useEffect, useState } from "react";
import { bridge, type Autostart } from "../../../lib/bridge";
import { t } from "../../../i18n";
import { useErrorHandler } from "../../../ui/primitives/Toast";
import "./SettingsCompute.css";

export function StartupCard() {
  const report = useErrorHandler();
  const [state, setState] = useState<Autostart | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    let alive = true;
    bridge.autostart().then(
      (found) => alive && setState(found),
      () => alive && setState(null),
    );
    return () => {
      alive = false;
    };
  }, []);

  const change = async (enabled: boolean, hidden: boolean) => {
    setSaving(true);
    try {
      setState(await bridge.setAutostart(enabled, hidden));
    } catch (err) {
      report(err, { retry: () => void change(enabled, hidden) });
    } finally {
      setSaving(false);
    }
  };

  const enabled = state?.enabled ?? false;
  const hidden = state?.hidden ?? false;
  return (
    <section className="settings-card" aria-labelledby="startup-heading">
      <div className="settings-card-header settings-card-header-row">
        <span id="startup-heading">{t("startup.title")}</span>
        <button
          type="button"
          role="switch"
          className="switch"
          aria-checked={enabled}
          aria-labelledby="startup-heading"
          disabled={!state || saving}
          onClick={() => void change(!enabled, hidden)}
        >
          <span className="switch-thumb" />
        </button>
      </div>
      <p className="settings-card-desc">
        {state && !state.service_installed
          ? t("startup.descNoService")
          : t("startup.desc")}
      </p>
      <div className="settings-card-header settings-card-header-row startup-row">
        <span id="startup-hidden">{t("startup.hidden")}</span>
        <button
          type="button"
          role="switch"
          className="switch"
          aria-checked={hidden}
          aria-labelledby="startup-hidden"
          disabled={!state || !enabled || saving}
          onClick={() => void change(true, !hidden)}
        >
          <span className="switch-thumb" />
        </button>
      </div>
      <p className="settings-card-desc">{t("startup.hiddenDesc")}</p>
    </section>
  );
}
