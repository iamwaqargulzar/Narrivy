// Press and hold to dictate (opt-in): hold one chosen key to talk, let go
// to finish. It needs Narrivy to read the keyboards, which the user is told
// before they choose a key.
import { useEffect, useState } from "react";
import { bridge, type HoldKey } from "../../../lib/bridge";
import { t } from "../../../i18n";
import { useErrorHandler } from "../../../ui/primitives/Toast";
import "./SettingsCompute.css";

function stateText(hold: HoldKey): string {
  switch (hold.state) {
    case "reading":
      return t("hold.reading");
    case "denied":
      return t("hold.denied");
    case "starting":
      return t("hold.starting");
    case "off":
      return t("hold.off");
    default:
      return t("hold.unavailable");
  }
}

export function HoldKeyCard() {
  const report = useErrorHandler();
  const [hold, setHold] = useState<HoldKey | null>(null);
  const [failed, setFailed] = useState(false);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    let alive = true;
    bridge.holdKey().then(
      (found) => alive && setHold(found),
      () => alive && setFailed(true),
    );
    return () => {
      alive = false;
    };
  }, []);

  const choose = async (value: string) => {
    setSaving(true);
    try {
      const next = await bridge.setHoldKey(value === "" ? null : Number(value));
      setHold(next);
      // The keyboards open in the background: read the state again.
      window.setTimeout(
        () => bridge.holdKey().then(setHold, () => undefined),
        1500,
      );
    } catch (err) {
      report(err, { retry: () => void choose(value) });
    } finally {
      setSaving(false);
    }
  };

  return (
    <section className="settings-card" aria-labelledby="hold-heading">
      <div className="settings-card-header" id="hold-heading">
        {t("hold.title")}
      </div>
      <p className="settings-card-desc">{t("hold.desc")}</p>
      <p className="settings-card-desc hold-privacy">{t("hold.privacy")}</p>
      {failed ? (
        <p className="source">{t("hold.unavailableService")}</p>
      ) : (
        <>
          <select
            className="pipe-select"
            aria-label={t("hold.key")}
            value={hold?.key ?? ""}
            disabled={!hold || saving}
            onChange={(e) => void choose(e.target.value)}
          >
            <option value="">{t("hold.none")}</option>
            {hold?.keys.map((k) => (
              <option key={k.code} value={k.code}>
                {k.name}
              </option>
            ))}
          </select>
          {hold ? (
            <p className="source" aria-live="polite">
              {stateText(hold)}
            </p>
          ) : null}
        </>
      )}
    </section>
  );
}
