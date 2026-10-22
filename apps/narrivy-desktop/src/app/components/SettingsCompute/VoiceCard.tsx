// "Only my voice": train Narrivy on the user's voice once, then dictation
// keeps only speech that sounds like them. People talking nearby are left
// out. The profile is a voice fingerprint kept on this computer, not audio.
import { useCallback, useEffect, useRef, useState } from "react";
import { bridge, type AudioOptions } from "../../../lib/bridge";
import { t } from "../../../i18n";
import { Button } from "../../../ui/primitives/Button";
import { useErrorHandler } from "../../../ui/primitives/Toast";
import { Choices } from "../StandingOptions";
import "./SettingsCompute.css";

/** How long training records, matching the daemon. */
export const TRAINING_SECONDS = 20;

function failureText(state: string): string {
  const code = state.slice("failed:".length);
  if (code === "voice_training_too_short") return t("voice.failed.short");
  return t("voice.failed.other", { code });
}

export function VoiceCard() {
  const report = useErrorHandler();
  const [options, setOptions] = useState<AudioOptions | null>(null);
  const [left, setLeft] = useState(0);
  const [busy, setBusy] = useState(false);
  const timer = useRef<ReturnType<typeof setInterval> | null>(null);

  const read = useCallback(async () => {
    try {
      const found = await bridge.audioOptions();
      setOptions(found);
      return found;
    } catch {
      setOptions(null);
      return null;
    }
  }, []);

  useEffect(() => {
    void read();
    return () => {
      if (timer.current) clearInterval(timer.current);
    };
  }, [read]);

  const recording = options?.voice_training === "recording";

  // While training records, count down and watch for the end.
  useEffect(() => {
    if (!recording) return;
    const started = Date.now();
    timer.current = setInterval(() => {
      setLeft(
        Math.max(
          0,
          TRAINING_SECONDS - Math.floor((Date.now() - started) / 1000),
        ),
      );
      void read();
    }, 1000);
    return () => {
      if (timer.current) clearInterval(timer.current);
      timer.current = null;
    };
  }, [recording, read]);

  const run = async (action: () => Promise<AudioOptions>) => {
    setBusy(true);
    try {
      setOptions(await action());
    } catch (err) {
      report(err, { retry: () => void run(action) });
    } finally {
      setBusy(false);
    }
  };

  const train = () => {
    setLeft(TRAINING_SECONDS);
    void run(() => bridge.trainVoice());
  };

  const available = Boolean(options?.voice_filter_available);
  const trained = Boolean(options?.voice_trained);
  const on = Boolean(options?.only_my_voice) && trained;
  const failed = options?.voice_training?.startsWith("failed:")
    ? options.voice_training
    : null;

  return (
    <section className="settings-card" aria-labelledby="voice-heading">
      <div className="settings-card-header settings-card-header-row">
        <span id="voice-heading">{t("voice.title")}</span>
        <button
          type="button"
          role="switch"
          className="switch"
          aria-checked={on}
          aria-labelledby="voice-heading"
          disabled={!available || !trained || busy || recording}
          onClick={() => void run(() => bridge.setOnlyMyVoice(!on))}
        >
          <span className="switch-thumb" />
        </button>
      </div>
      <p className="settings-card-desc">{t("voice.desc")}</p>

      {!options || !available ? (
        <p className="source">{t("voice.unavailable")}</p>
      ) : recording ? (
        <div className="voice-training" aria-live="polite">
          <p className="voice-training-count">
            {t("voice.reading", { seconds: left })}
          </p>
          <blockquote className="voice-passage">
            <p>{t("voice.passage")}</p>
            <p dir="rtl" lang="ur">
              {t("voice.passageUrdu")}
            </p>
          </blockquote>
        </div>
      ) : (
        <>
          <p className="source" aria-live="polite">
            {failed
              ? failureText(failed)
              : trained
                ? on
                  ? t("voice.active")
                  : t("voice.off")
                : t("voice.untrained")}
          </p>
          {trained && options.voice_strictness ? (
            <div className="voice-strictness">
              <span className="source">{t("voice.strictness")}</span>
              <Choices
                label={t("voice.strictness")}
                current={options.voice_strictness}
                disabled={busy}
                onPick={(level) =>
                  void run(() => bridge.setVoiceStrictness(level))
                }
                options={[
                  {
                    id: "relaxed",
                    label: t("voice.strict.relaxed"),
                    note: t("voice.strict.relaxedNote"),
                  },
                  { id: "normal", label: t("voice.strict.normal") },
                  {
                    id: "strict",
                    label: t("voice.strict.strict"),
                    note: t("voice.strict.strictNote"),
                  },
                ]}
              />
            </div>
          ) : null}
          <div className="voice-actions">
            <Button onClick={train} disabled={busy}>
              {trained ? t("voice.retrain") : t("voice.train")}
            </Button>
            {trained ? (
              <Button
                onClick={() => void run(() => bridge.forgetVoice())}
                disabled={busy}
              >
                {t("voice.forget")}
              </Button>
            ) : null}
          </div>
        </>
      )}
    </section>
  );
}
