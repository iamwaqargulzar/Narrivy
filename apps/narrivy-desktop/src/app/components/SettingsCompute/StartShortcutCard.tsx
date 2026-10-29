// How to start dictation from any app: the daemon's suggested desktop
// command when no shortcut listener is live, and the tray menu. The keys
// themselves are set per pipeline on the Pipelines page.
import { t } from "../../../i18n";
import { activationPresentation } from "../../../lib/activation";
import { useDaemonStatus } from "../../../state/StatusContext";
import "./SettingsCompute.css";

export function StartShortcutCard() {
  const { status } = useDaemonStatus();
  const activation = activationPresentation(status);

  return (
    <section className="settings-card" aria-labelledby="start-shortcut-heading" data-testid="start-shortcut">
      <div className="settings-card-header" id="start-shortcut-heading">
        {t("startShortcut.title")}
      </div>
      {activation.suggestedCommand ? (
        <>
          <p className="settings-card-desc">{t("onboard.shortcut.body")}</p>
          <code className="code-chip code-block">{activation.suggestedCommand}</code>
          <p className="source">{t("onboard.shortcut.notConfigured")}</p>
        </>
      ) : activation.portalListenerReady ? (
        <p className="settings-card-desc">{t("onboard.shortcut.listenerReady")}</p>
      ) : (
        <p className="settings-card-desc">{t("fm.value.notReported")}</p>
      )}
      <p className="source">{t("startShortcut.tray")}</p>
    </section>
  );
}
