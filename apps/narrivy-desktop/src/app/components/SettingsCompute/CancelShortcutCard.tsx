// A desktop shortcut that cancels the dictation in progress: nothing is
// written. It runs Narrivy's command line, like the pipelines' shortcuts.
import { useCallback, useEffect, useState } from "react";
import { bridge, type ShortcutBinding } from "../../../lib/bridge";
import { comboParts } from "../../../lib/shortcutKeys";
import { t } from "../../../i18n";
import { Button } from "../../../ui/primitives/Button";
import { useErrorHandler } from "../../../ui/primitives/Toast";
import { KeyChips, ShortcutCapture } from "../ShortcutCapture";
import "./SettingsCompute.css";

export const CANCEL_TARGET = "cancel";

export function CancelShortcutCard() {
  const report = useErrorHandler();
  const [bindings, setBindings] = useState<ShortcutBinding[] | null>(null);
  const [managed, setManaged] = useState(false);
  const [capturing, setCapturing] = useState(false);

  const load = useCallback(async () => {
    try {
      const info = await bridge.shortcuts();
      setManaged(info.support === "managed");
      setBindings(info.bindings.filter((b) => b.pipeline_id === CANCEL_TARGET));
    } catch {
      setBindings([]);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const remove = async (binding: ShortcutBinding) => {
    try {
      await bridge.removeShortcut(binding.modifiers, binding.key);
      await load();
    } catch (err) {
      report(err, { retry: () => void remove(binding) });
    }
  };

  return (
    <section className="settings-card" aria-labelledby="cancel-heading">
      <div className="settings-card-header" id="cancel-heading">
        {t("cancelShortcut.title")}
      </div>
      <p className="settings-card-desc">{t("cancelShortcut.desc")}</p>
      <div className="voice-actions">
        {bindings?.map((b) => (
          <span
            key={`${b.modifiers.join("+")}+${b.key}`}
            className="vocab-word"
          >
            <KeyChips parts={comboParts(b)} />
            <button
              type="button"
              aria-label={t("cancelShortcut.remove")}
              onClick={() => void remove(b)}
            >
              ×
            </button>
          </span>
        ))}
        {managed ? (
          <Button onClick={() => setCapturing(true)}>
            {t("cancelShortcut.add")}
          </Button>
        ) : (
          <p className="source">{t("cancelShortcut.manual")}</p>
        )}
      </div>
      {capturing ? (
        <ShortcutCapture
          target={{
            pipelineId: CANCEL_TARGET,
            pipelineName: t("cancelShortcut.name"),
          }}
          onClose={() => setCapturing(false)}
          onSaved={() => {
            setCapturing(false);
            void load();
          }}
        />
      ) : null}
    </section>
  );
}
