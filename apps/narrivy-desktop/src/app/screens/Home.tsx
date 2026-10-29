// Home: three sections, per the owner's direction and sketch (2026-09-29).
//
// 1. Dictation: the mic button with its state, a text area showing live words
//    while listening and the last result afterwards, and the Recent list with
//    Copy, Retry and Delete.
// 2. Active pipeline: a read-only line of each stage and whether it is
//    active. Changes happen in Pipelines.
// 3. System: latency, the service's own memory and CPU, and every service the
//    pipeline depends on.
//
// Every value comes from the daemon status snapshot, the advertised pipeline
// list and session events. A value the daemon does not report is not shown.
import { useEffect, useState } from "react";
import { Icon, type IconName } from "../../ui/icons";
import {
  capabilityState,
  UNKNOWN_CAPABILITY_STATE,
} from "../../lib/capabilityStates";
import {
  markListeningVisible,
  measureStartToListening,
} from "../../lib/performance";
import {
  resolveCompute,
  resolveDelivery,
  resolveMic,
  resolveModel,
} from "../../lib/resolved";
import {
  computeChip,
  deliveryChip,
  micChip,
  modelChip,
} from "../../lib/homeSummary";
import {
  activeDevice,
  engineFacts,
  formatBytes,
  formatMs,
  formatPermille,
  latencyFacts,
  pipelineVerdict,
  resourceFacts,
  serviceFacts,
} from "../../lib/overview";
import { loadSettings } from "../../lib/settings";
import { reportedCaptureLimitSeconds } from "../../lib/capturePolicy";
import { useErrorHandler, useToasts } from "../../ui/primitives/Toast";
import { t } from "../../i18n";
import { reportedLanguages } from "../../lib/modelFacts";
import { useDaemonStatus } from "../../state/StatusContext";
import { usePipelines } from "../../state/PipelinesContext";
import { useSession } from "../../state/SessionContext";
import { useDictationActions } from "../../hooks/useDictationActions";
import { useNav } from "../../state/NavContext";
import { useTheme } from "../../theme/ThemeProvider";
import {
  DictationNotices,
  SessionDetails,
} from "../components/DictationShared";
import { RecentDictations } from "../components/RecentDictations";
import { bridge } from "../../lib/bridge";
import { LearnWords } from "../components/LearnWords";
import { learnedWords } from "../../lib/correctionLearning";

const LIVE = new Set(["preparing", "recording", "listening"]);
const BUSY = new Set([
  "finalizing_audio",
  "transcribing",
  "transforming",
  "delivering",
  "processing",
  "stopping",
]);

type StageState = "active" | "ready" | "off" | "unavailable" | "working";

function greetingKey(): string {
  const hour = new Date().getHours();
  if (hour < 12) return "home.greeting.morning";
  if (hour < 18) return "home.greeting.afternoon";
  return "home.greeting.evening";
}

const MODE_DESCRIPTION_KEYS: Record<string, string> = {
  clean: "dictationPage.mode.clean",
  raw: "dictationPage.mode.raw",
};

/** A reported language code as a word ("en" → "English"), or the code itself. */
interface StageProps {
  icon: IconName;
  label: string;
  value: string;
  facts: string[];
  detail?: string;
  state: StageState;
}

/** One read-only pipeline stage: what it is, what is active, and its state. */
function Stage({ icon, label, value, facts, detail, state }: StageProps) {
  return (
    <li className="stage-tile" data-state={state} title={detail}>
      <span className="stage-icon" aria-hidden="true">
        <Icon name={icon} />
      </span>
      <span className="stage-label">{label}</span>
      <span className="stage-value">{value}</span>
      {facts.length > 0 ? (
        <ul className="stage-facts">
          {facts.map((fact) => (
            <li key={fact}>{fact}</li>
          ))}
        </ul>
      ) : null}
      <span className="stage-state">
        <span className="dot" aria-hidden="true" />
        {t(`home.stageState.${state}`)}
      </span>
    </li>
  );
}

export function HomeScreen() {
  const { status, error: statusError } = useDaemonStatus();
  const { catalog, acknowledgeSelectionChange } = usePipelines();
  const actions = useDictationActions();
  const session = useSession();
  const { navigate } = useNav();
  const { showGreeting } = useTheme();
  const report = useErrorHandler();
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  // The transcript the edit started from: Save never overwrites a newer
  // one that arrived meanwhile (F-M11).
  const [editBase, setEditBase] = useState("");
  const { push } = useToasts();
  const [copied, setCopied] = useState(false);
  // Words the user corrected, offered for their vocabulary.
  const [learn, setLearn] = useState<string[]>([]);

  const live = LIVE.has(session.phase);
  const busy = BUSY.has(session.phase);

  // PERF-004: mark the instant the listening state becomes visible.
  useEffect(() => {
    if (session.phase !== "recording") return;
    try {
      markListeningVisible();
      measureStartToListening();
    } catch {
      // performance marks are best-effort
    }
  }, [session.phase]);

  useEffect(() => {
    if (!copied) return;
    const id = window.setTimeout(() => setCopied(false), 1500);
    return () => window.clearTimeout(id);
  }, [copied]);

  const snap = status ?? { capabilities: [], active_session: null };
  const settingMode = (key: string): "auto" | "custom" =>
    loadSettings().values[key] === "auto" ? "auto" : "custom";
  const micC = micChip(
    resolveMic(snap as never, settingMode("audio.input_device")),
  );
  const modelC = modelChip(
    resolveModel(snap as never, settingMode("asr.model")),
  );
  const computeC = computeChip(
    resolveCompute(snap as never, settingMode("compute.device")),
  );
  const deliveryC = deliveryChip(
    resolveDelivery(snap as never, settingMode("delivery.mode")),
  );
  const latency = latencyFacts(status);
  const resources = resourceFacts(status);
  const verdict = pipelineVerdict(serviceFacts(status));
  const selected =
    catalog.pipelines.find((p) => p.id === catalog.selectedId) ?? null;
  const localOnly =
    status?.capabilities.includes("policy.local-only=enabled") ?? false;

  // Capability state token rendered for styling only.
  const advertisedCapabilityState = (() => {
    for (const cap of snap.capabilities ?? []) {
      const state = capabilityState(cap);
      if (state !== UNKNOWN_CAPABILITY_STATE) return state;
    }
    return UNKNOWN_CAPABILITY_STATE;
  })();

  const readinessTitle = (() => {
    if (!status && statusError) return t("home.readiness.daemonUnavailable");
    if (!status) return t("home.readiness.checkingDaemon");
    if (actions.active) return t("home.readiness.active");
    switch (catalog.availability) {
      case "loading":
        return t("home.readiness.modesLoading");
      case "error":
        return t("home.readiness.modesUnavailable");
      case "empty":
        return t("home.readiness.modesEmpty");
      case "awaiting_model":
        return t("home.readiness.awaitingModel");
      case "unavailable":
        return t("home.readiness.unavailable");
      case "ready":
        return t("home.readiness.ready");
    }
  })();
  const stateLine = live
    ? t("home.phase.live")
    : busy
      ? t("home.phase.busy")
      : readinessTitle;
  const sessionControls = actions.canStop || actions.canCancel || live;
  const phase = live ? "live" : busy ? "busy" : "idle";

  const startNow = () => {
    acknowledgeSelectionChange();
    void actions.start();
  };

  const shown = session.lastTranscript;
  const copyLast = async () => {
    if (!shown) return;
    try {
      await navigator.clipboard.writeText(shown);
      setCopied(true);
    } catch (err) {
      report(err ?? new Error(t("home.copyFailed")), {
        retry: () => void copyLast(),
      });
    }
  };
  const liveText = session.tentativeTranscript;
  const showLive = live || busy;
  const undoOffered =
    !showLive &&
    shown &&
    session.undoAvailable &&
    session.lastRaw !== null &&
    session.lastRaw !== shown;

  const micState: StageState =
    micC.tone === "warn" ? "unavailable" : live ? "active" : "ready";
  const modelState: StageState =
    modelC.tone === "warn" ? "unavailable" : busy ? "working" : "ready";
  const refineState: StageState = selected
    ? busy
      ? "working"
      : "ready"
    : "unavailable";
  const deliverState: StageState =
    deliveryC.tone === "warn"
      ? "unavailable"
      : session.phase === "delivering"
        ? "working"
        : "ready";

  // Read-only facts per stage, each from a daemon-reported value.
  const device = activeDevice(status);
  const engine = engineFacts(status);
  const captureSeconds = reportedCaptureLimitSeconds(status);
  const captureFacts =
    captureSeconds !== null
      ? [
          t("home.fact.captureLimit", {
            minutes: Math.max(1, Math.round(captureSeconds / 60)),
          }),
        ]
      : [];
  const transcribeFacts = [
    device.running
      ? t("strip.runsOn", { device: t(`strip.device.${device.running}`) })
      : null,
    engine.languages ? reportedLanguages(engine.languages) : null,
    engine.streaming === null
      ? null
      : engine.streaming
        ? t("home.fact.liveText")
        : t("home.fact.afterStop"),
  ].filter((fact): fact is string => fact !== null);
  const modeKey = selected ? MODE_DESCRIPTION_KEYS[selected.id] : undefined;
  const refineFacts = modeKey ? [t(modeKey)] : [];
  const inboxBackup = snap.capabilities.includes(
    "delivery.inbox-recovery=enabled",
  );
  const deliverFacts = inboxBackup ? [t("home.fact.inboxBackup")] : [];

  return (
    <div className="home" data-capability-state={advertisedCapabilityState}>
      {showGreeting ? (
        <header className="greeting">
          <h2 className="display">{t(greetingKey())}</h2>
          <p className="greeting-sub">
            {localOnly ? t("home.sub.private") : t("home.sub.ready")}
          </p>
        </header>
      ) : null}

      <DictationNotices />

      {/* 1. Dictation */}
      <section
        className="dictate"
        data-phase={phase}
        aria-label={t("home.section.dictation")}
      >
        <div className="dictate-top">
          <div className="mic-column">
            <button
              type="button"
              className="mic-button btn-press"
              data-phase={phase}
              onClick={startNow}
              disabled={!actions.canStart}
              aria-label={t("home.start")}
              data-testid="dictation-start"
            >
              <Icon name="mic" />
            </button>
            <span
              className="mic-state"
              data-testid="home-readiness"
              role="status"
            >
              {stateLine}
            </span>
            <span className="session-controls" hidden={!sessionControls}>
              <button
                type="button"
                className="text-button text-button-live"
                onClick={() => void actions.stop()}
                disabled={!actions.canStop}
                aria-label={t("home.stopLabel")}
                data-testid="dictation-stop"
              >
                {t("dictation.stop")}
              </button>
              <button
                type="button"
                className="text-button"
                onClick={() => void actions.cancel()}
                disabled={!actions.canCancel}
                aria-label={t("home.cancelLabel")}
                data-testid="dictation-cancel"
              >
                {t("dictation.cancel")}
              </button>
            </span>
          </div>

          <div className="text-panel">
            {showLive ? (
              <p
                className={`transcript-text${liveText ? " is-live" : " transcript-empty"}`}
                role="status"
                aria-live="polite"
                aria-atomic="true"
              >
                {liveText ?? t("dictation.liveNone")}
              </p>
            ) : editing ? (
              <textarea
                className="text-edit"
                aria-label={t("home.recent.edit")}
                value={draft}
                onChange={(e) => setDraft(e.target.value)}
                rows={4}
              />
            ) : shown ? (
              <p className="transcript-text">{shown}</p>
            ) : (
              <p className="transcript-text transcript-empty">
                {t("home.transcript.emptyOverview")}
              </p>
            )}
            {learn.length && !editing ? (
              <LearnWords words={learn} onDone={() => setLearn([])} />
            ) : null}
            <div className="text-panel-actions">
              {editing ? (
                <>
                  <button
                    type="button"
                    className="text-button text-button-strong"
                    onClick={() => {
                      const before = shown ?? "";
                      setEditing(false);
                      if (before !== editBase) {
                        push(t("home.recent.editStale"));
                        return;
                      }
                      session.setLastDelivered(draft);
                      bridge
                        .vocabulary()
                        .then(
                          (v) => v.words,
                          () => null,
                        )
                        .then((known) => {
                          // No vocabulary (an older service): nothing to offer.
                          if (known)
                            setLearn(learnedWords(before, draft, known));
                        });
                    }}
                  >
                    {t("home.recent.save")}
                  </button>
                  <button
                    type="button"
                    className="text-button"
                    onClick={() => setEditing(false)}
                  >
                    {t("home.recent.cancelEdit")}
                  </button>
                </>
              ) : (
                <>
                  <button
                    type="button"
                    className="text-button"
                    onClick={() => void copyLast()}
                    disabled={!shown || showLive}
                    aria-label={t("home.recent.copy")}
                  >
                    {copied ? t("home.copied") : t("home.copy")}
                  </button>
                  <button
                    type="button"
                    className="text-button"
                    onClick={() => {
                      if (!shown) return;
                      setDraft(shown);
                      setEditBase(shown);
                      setEditing(true);
                    }}
                    disabled={!shown || showLive}
                    aria-label={t("home.recent.edit")}
                  >
                    {t("home.edit")}
                  </button>
                  {/* UX-009: one-step undo after a Clean delivery reverts the
                      shown output to the journaled Raw variant. */}
                  {undoOffered ? (
                    <button
                      type="button"
                      className="text-button"
                      aria-label={t("dictation.undo")}
                      onClick={session.undoToRaw}
                    >
                      {t("dictation.undo")}
                    </button>
                  ) : null}
                </>
              )}
            </div>
          </div>
        </div>
        <RecentDictations />
      </section>

      {/* 2. Active pipeline (read-only; changed only on the Pipelines screen) */}
      <section className="pipeline-card" aria-labelledby="pipeline-title">
        <div className="pipeline-card-head">
          <h3 id="pipeline-title" className="section-label">
            {t("home.tile.pipeline")}
          </h3>
          {selected ? (
            <span className="section-meta">{selected.name}</span>
          ) : null}
          <span className="card-head-spacer" />
          <button
            type="button"
            className="link-action link-quiet"
            onClick={() => navigate("pipelines")}
          >
            {t("home.managePipeline")}
            <Icon name="arrowRight" />
          </button>
        </div>
        <ol className="pipeline-stages">
          <Stage
            icon="mic"
            label={t("home.stage.microphone")}
            value={micC.label}
            facts={captureFacts}
            detail={micC.detail}
            state={micState}
          />
          <Stage
            icon="wave"
            label={t("home.stage.noise")}
            value={t("home.stage.noiseValue")}
            facts={[t("home.fact.asCaptured")]}
            detail={t("home.stage.cleanDetail")}
            state="off"
          />
          <Stage
            icon="cpu"
            label={t("home.stage.transcribe")}
            value={modelC.label}
            facts={transcribeFacts}
            detail={`${modelC.detail} · ${computeC.detail}`}
            state={modelState}
          />
          <Stage
            icon="spark"
            label={t("home.stage.refine")}
            value={selected ? selected.name : t("home.mode.none")}
            facts={refineFacts}
            state={refineState}
          />
          <Stage
            icon="send"
            label={t("home.stage.deliver")}
            value={
              inboxBackup
                ? deliveryC.label.replace(/ · Inbox backup$/, "")
                : deliveryC.label
            }
            facts={deliverFacts}
            detail={deliveryC.detail}
            state={deliverState}
          />
        </ol>
        <SessionDetails />
      </section>

      {/* 3. System */}
      <section className="pipeline-card" aria-label={t("home.metricsTitle")}>
        <div className="pipeline-card-head">
          <h3 className="section-label">{t("home.systemTitle")}</h3>
          <span className="section-meta verdict" data-verdict={verdict}>
            <span className="dot" aria-hidden="true" />
            {t(`home.services.verdict.${verdict}`)}
          </span>
        </div>
        <div className="stats">
          {latency.reported ? (
            <div className="stat">
              <span className="stage-icon" aria-hidden="true">
                <Icon name="gauge" />
              </span>
              <span className="stat-label">{t("home.tile.latency")}</span>
              <span className="stat-value">
                {formatMs(latency.p50Ms) ?? "—"}
              </span>
              <span className="stat-note">
                {latency.p50Ms === null
                  ? t("home.tile.noSamples")
                  : t("home.latency.note", {
                      p95: formatMs(latency.p95Ms) ?? "—",
                      count: String(latency.sessions ?? 0),
                    })}
              </span>
            </div>
          ) : null}
          {resources.reported ? (
            <>
              <div className="stat">
                <span className="stage-icon" aria-hidden="true">
                  <Icon name="memory" />
                </span>
                <span className="stat-label">{t("home.resources.ram")}</span>
                <span className="stat-value">
                  {formatBytes(resources.rssBytes) ?? "—"}
                </span>
                <span className="stat-note">
                  {t("home.resources.caption", {
                    count: String(resources.processes ?? "—"),
                  })}
                </span>
              </div>
              <div className="stat">
                <span className="stage-icon" aria-hidden="true">
                  <Icon name="cpu" />
                </span>
                <span className="stat-label">{t("home.resources.cpu")}</span>
                <span className="stat-value">
                  {formatPermille(resources.cpuPermille) ?? "…"}
                </span>
                <span className="stat-note">{t("home.resources.cpuNote")}</span>
              </div>
            </>
          ) : (
            <div className="stat">
              <span className="stat-label">{t("home.resources.title")}</span>
              <span className="stat-note">
                {t("home.resources.unreported")}
              </span>
            </div>
          )}
        </div>
      </section>
    </div>
  );
}
