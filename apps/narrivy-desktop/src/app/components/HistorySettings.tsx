// How long History is kept, and clearing it: on the History screen, where
// the entries are.
import { useState } from "react";
import { bridge } from "../../lib/bridge";
import { t } from "../../i18n";
import { Button } from "../../ui/primitives/Button";
import { useErrorHandler } from "../../ui/primitives/Toast";

const CHOICES = ["7d", "30d", "forever", "off"] as const;

export function HistorySettings({
  retention,
  count,
  onChanged,
}: {
  /** The service's current policy (`history.retention=`), when reported. */
  retention: string | null;
  count: number;
  onChanged: () => void;
}) {
  const report = useErrorHandler();
  const [saving, setSaving] = useState(false);
  const [confirming, setConfirming] = useState(false);

  const choose = async (value: string) => {
    setSaving(true);
    try {
      await bridge.setHistoryRetention(value);
      onChanged();
    } catch (err) {
      report(err, { retry: () => void choose(value) });
    } finally {
      setSaving(false);
    }
  };

  const clear = async () => {
    setSaving(true);
    try {
      await bridge.clearHistory();
      setConfirming(false);
      onChanged();
    } catch (err) {
      report(err, { retry: () => void clear() });
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="history-settings">
      <label className="history-keep">
        <span>{t("history.keep")}</span>
        <select
          className="pipe-select"
          value={retention ?? "7d"}
          disabled={saving || retention === null}
          onChange={(e) => void choose(e.target.value)}
        >
          {CHOICES.map((choice) => (
            <option key={choice} value={choice}>
              {t(`history.keep.${choice}`)}
            </option>
          ))}
        </select>
      </label>
      {confirming ? (
        <span className="history-confirm" role="alert">
          <span>{t("history.clearConfirm", { count })}</span>
          <Button onClick={() => void clear()} disabled={saving}>
            {t("history.clearYes")}
          </Button>
          <Button onClick={() => setConfirming(false)} disabled={saving}>
            {t("history.clearNo")}
          </Button>
        </span>
      ) : (
        <Button
          onClick={() => setConfirming(true)}
          disabled={saving || count === 0}
        >
          {t("history.clear")}
        </Button>
      )}
      {retention === "off" ? (
        <p className="source history-off-note">{t("history.keep.offNote")}</p>
      ) : null}
    </div>
  );
}
