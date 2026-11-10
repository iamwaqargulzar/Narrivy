// Pipelines: every pipeline the daemon advertises, in the user's order and
// with the user's names, each with its five stages. Clicking a stage expands
// that stage's options. Every option says where it stands: Live (works here),
// From service (read from the daemon), Coming (scheduled, with its task) or
// Proposed (awaiting an owner decision). Nothing is shown as working unless
// it does. Order, names and language shortcuts are this app's preferences
// (lib/pipelinePrefs.ts); the daemon owns ids, stages and versions.
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type DragEvent,
} from "react";
import { Icon, type IconName } from "../../ui/icons";
import { t } from "../../i18n";
import { usePipelines } from "../../state/PipelinesContext";
import { useDaemonStatus } from "../../state/StatusContext";
import { useNav } from "../../state/NavContext";
import { useErrorHandler } from "../../ui/primitives/Toast";
import { resolveDelivery, resolveMic, resolveModel } from "../../lib/resolved";
import { deliveryChip, micChip, modelChip } from "../../lib/homeSummary";
import { activeDevice, formatBytes } from "../../lib/overview";
import {
  languageSummary,
  modelName,
  quantBits,
  usableModels,
} from "../../lib/modelFacts";
import { reportedCaptureLimitSeconds } from "../../lib/capturePolicy";
import { recognitionOptions, tagName } from "../../lib/recognition";
import {
  MAX_NAME_LENGTH,
  loadPipelinePrefs,
  moveInOrder,
  orderPipelines,
  savePipelinePrefs,
  shortcutCommand,
  type PipelinePrefs,
} from "../../lib/pipelinePrefs";
import {
  bridge,
  type LanguageModel,
  type ModelInfo,
  type PipelineCompute,
  type PipelineEdit,
  type PipelineRefine,
  type PipelineSummary,
  type ShortcutBinding,
  type ShortcutsInfo,
  type StatusSnapshot,
} from "../../lib/bridge";
import { comboLabel, comboParts } from "../../lib/shortcutKeys";
import { PIPELINE_EDIT_PROTOCOL_MINOR, pipelineRefine } from "../../lib/pipelines";
import {
  KeyChips,
  ShortcutCapture,
  type CaptureTarget,
} from "../components/ShortcutCapture";
import {
  Choices,
  Facts,
  LIVE,
  Option,
  REPORTED,
  Section,
} from "../components/StandingOptions";
import { RecognitionSettingsCard } from "../components/SettingsCompute/RecognitionSettingsCard";
import "../components/SettingsCompute/SettingsCompute.css";
import { VoiceCard } from "../components/SettingsCompute/VoiceCard";
import {
  bridge as audioBridge,
  type AudioOptions,
  type CleanupLevel,
  type TextOptions,
} from "../../lib/bridge";
import { VocabularyEditor } from "../components/VocabularyEditor";
import { cleanUpState } from "./LanguageModels";
import { voiceSummary } from "../../lib/voiceSummary";

type StageId = "mic" | "noise" | "transcribe" | "refine" | "deliver";

const STAGES: Array<{ id: StageId; icon: IconName }> = [
  { id: "mic", icon: "mic" },
  { id: "noise", icon: "wave" },
  { id: "transcribe", icon: "cpu" },
  { id: "refine", icon: "spark" },
  { id: "deliver", icon: "send" },
];

/** Shortcut management needs a daemon at protocol minor 11 or newer. */
const SHORTCUTS_PROTOCOL_MINOR = 11;

interface ShortcutsContextValue {
  /** Null until loaded, or when the service cannot list shortcuts. */
  info: ShortcutsInfo | null;
  managed: boolean;
  /** The installed toggle command the daemon reports; null when unknown. */
  command: string | null;
  capture: (target: CaptureTarget) => void;
  remove: (binding: ShortcutBinding) => void;
}

const ShortcutsContext = createContext<ShortcutsContextValue>({
  info: null,
  managed: false,
  command: null,
  capture: () => undefined,
  remove: () => undefined,
});

/** First service protocol minor where every pipeline has its own options. */
const PIPELINE_OPTIONS_PROTOCOL_MINOR = 57;

/** The pipeline whose stage is open: its options are read and changed (D50). */
const PipelineIdContext = createContext<string | undefined>(undefined);

function usePipelineId(): string | undefined {
  return useContext(PipelineIdContext);
}

function cap(status: StatusSnapshot | null, prefix: string): string | null {
  const found = status?.capabilities.find((c) => c.startsWith(prefix));
  return found ? found.slice(prefix.length) : null;
}

function languageName(code: string): string {
  if (code === "auto") return t("pipe.lang.auto");
  return tagName(code);
}

function CopyCommand({ command, label }: { command: string; label: string }) {
  const report = useErrorHandler();
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const id = window.setTimeout(() => setCopied(false), 1500);
    return () => window.clearTimeout(id);
  }, [copied]);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(command);
      setCopied(true);
    } catch (err) {
      report(err ?? new Error(t("home.copyFailed")), {
        retry: () => void copy(),
      });
    }
  };
  return (
    <div className="pipe-command">
      <code>{command}</code>
      <button
        type="button"
        className="icon-btn"
        onClick={() => void copy()}
        aria-label={label}
        title={label}
      >
        <Icon name={copied ? "check" : "copy"} />
      </button>
    </div>
  );
}

// ------------------------------------------------------------ stage panels

/** A short sound when listening starts and stops. */
function SoundsOption() {
  const pid = usePipelineId();
  const report = useErrorHandler();
  const [audio, setAudio] = useState<AudioOptions | null>(null);
  const [saving, setSaving] = useState(false);
  useEffect(() => {
    audioBridge.audioOptions(pid).then(setAudio, () => setAudio(null));
  }, []);
  if (!audio || audio.sounds === undefined) return null;
  const toggle = async () => {
    setSaving(true);
    try {
      setAudio(await audioBridge.setSounds(!audio.sounds, pid));
    } catch (err) {
      report(err, { retry: () => void toggle() });
    } finally {
      setSaving(false);
    }
  };
  return (
    <Option
      label={t("pipe.mic.sounds")}
      hint={t("pipe.mic.soundsHint")}
      standing={LIVE}
    >
      <button
        type="button"
        role="switch"
        className="switch"
        aria-checked={audio.sounds}
        aria-label={t("pipe.mic.sounds")}
        disabled={saving}
        onClick={() => void toggle()}
      >
        <span className="switch-thumb" />
      </button>
    </Option>
  );
}

/** Which microphone Narrivy records; every pipeline uses it. */
function MicrophoneChoice({ fallback }: { fallback: string }) {
  const pid = usePipelineId();
  const report = useErrorHandler();
  const [audio, setAudio] = useState<AudioOptions | null>(null);
  const [saving, setSaving] = useState(false);
  useEffect(() => {
    audioBridge.audioOptions(pid).then(setAudio, () => setAudio(null));
  }, []);
  const choose = async (name: string) => {
    setSaving(true);
    try {
      setAudio(await audioBridge.setMicrophone(name, pid));
    } catch (err) {
      report(err, { retry: () => void choose(name) });
    } finally {
      setSaving(false);
    }
  };
  if (!audio || audio.microphones === undefined) {
    // An older service: show what it reports.
    return (
      <Option
        label={t("pipe.mic.device")}
        hint={t("pipe.mic.deviceHint")}
        value={fallback}
        standing={REPORTED}
      />
    );
  }
  const chosen = audio.microphone ?? "";
  const listed = audio.microphones.some((m) => m.name === chosen);
  return (
    <Option
      label={t("pipe.mic.choose")}
      hint={
        audio.microphone_missing
          ? t("pipe.mic.missing")
          : t("pipe.mic.chooseLive")
      }
      standing={LIVE}
    >
      <select
        className="pipe-select"
        aria-label={t("pipe.mic.choose")}
        value={chosen}
        disabled={saving}
        onChange={(e) => void choose(e.target.value)}
      >
        <option value="">{t("pipe.mic.systemDefault")}</option>
        {audio.microphones.map((m) => (
          <option key={m.name} value={m.name}>
            {m.description}
          </option>
        ))}
        {chosen && !listed ? (
          <option value={chosen}>{t("pipe.mic.unplugged")}</option>
        ) : null}
      </select>
    </Option>
  );
}

/** Automatic gain: WebRTC's gain control in the echo canceller, so it
 * works while "Ignore this computer's sound" is on. Hidden on a service
 * without it. */
function AutoGainOption() {
  const pid = usePipelineId();
  const report = useErrorHandler();
  const [audio, setAudio] = useState<AudioOptions | null>(null);
  const [saving, setSaving] = useState(false);
  useEffect(() => {
    audioBridge.audioOptions(pid).then(setAudio, () => setAudio(null));
  }, []);
  if (!audio || audio.auto_gain === undefined) return null;
  const toggle = async () => {
    setSaving(true);
    try {
      setAudio(await audioBridge.setAutoGain(!audio.auto_gain, pid));
    } catch (err) {
      report(err, { retry: () => void toggle() });
    } finally {
      setSaving(false);
    }
  };
  return (
    <Option
      label={t("pipe.mic.gain")}
      hint={audio.echo_cancel ? t("pipe.mic.gainLive") : t("pipe.mic.gainNeedsEcho")}
      standing={LIVE}
    >
      <button
        type="button"
        role="switch"
        className="switch"
        aria-checked={audio.auto_gain}
        aria-label={t("pipe.mic.gain")}
        disabled={saving || !audio.echo_cancel}
        onClick={() => void toggle()}
      >
        <span className="switch-thumb" />
      </button>
    </Option>
  );
}

function MicPanel({ status }: { status: StatusSnapshot | null }) {
  const snap = status ?? { capabilities: [], active_session: null };
  const mic = resolveMic(snap as never, "auto");
  const seconds = reportedCaptureLimitSeconds(status);
  const helper = cap(status, "capture.pipewire-helper=");
  return (
    <>
      <Section title={t("pipe.sec.microphone")}>
        <MicrophoneChoice fallback={micChip(mic).label} />
        <AutoGainOption />
        <SoundsOption />
      </Section>
      <Facts
        items={[
          {
            label: t("pipe.mic.capture"),
            value: helper?.startsWith("ready")
              ? t("pipe.ready")
              : t("pipe.notReady"),
          },
          seconds !== null
            ? {
                label: t("pipe.mic.limit"),
                value: t("home.fact.captureLimit", {
                  minutes: Math.max(1, Math.round(seconds / 60)),
                }),
              }
            : null,
        ]}
      />
    </>
  );
}

/** Voice: what reaches the speech model from the microphone, for this
 * pipeline. "Ignore this computer's sound" and "Only my voice" are this
 * pipeline's own; noise suppression is set on the echo canceller. */
function VoicePanel({
  status,
  pipelineId,
}: {
  status: StatusSnapshot | null;
  pipelineId: string;
}) {
  const pid = usePipelineId();
  const report = useErrorHandler();
  const [audio, setAudio] = useState<AudioOptions | null>(null);
  const [saving, setSaving] = useState(false);
  useEffect(() => {
    audioBridge.audioOptions(pid).then(setAudio, () => setAudio(null));
  }, []);
  if (!audio) return null;
  const run = async (action: () => Promise<AudioOptions>) => {
    setSaving(true);
    try {
      setAudio(await action());
    } catch (err) {
      report(err, { retry: () => void run(action) });
    } finally {
      setSaving(false);
    }
  };
  const own = audio.pipelines?.[pipelineId] ?? {};
  const perPipeline = audio.pipelines !== undefined;
  const echo = perPipeline
    ? (own.echo_cancel ?? audio.echo_cancel)
    : audio.echo_cancel;
  const toggleEcho = () =>
    void run(() =>
      perPipeline
        ? audioBridge.setPipelineVoice(
            pipelineId,
            !echo,
            own.only_my_voice ?? null,
          )
        : audioBridge.setEchoCancel(!echo, pid),
    );
  const level = audio.noise_suppression || "standard";
  const noiseKnown =
    audio.noise_suppression !== undefined && audio.noise_suppression !== "";
  const endpoint = cap(status, "speech.endpoint=");
  return (
    <>
      <Section title={t("pipe.voice.background")}>
        <SwitchOption
          label={t("echo.title")}
          hint={
            echo && !audio.echo_cancel_active
              ? t("pipe.voice.echoStarting")
              : t("pipe.voice.echoHint")
          }
          on={echo}
          disabled={saving}
          onToggle={toggleEcho}
        />
        {noiseKnown ? (
          <Option
            label={t("pipe.noise.suppression")}
            hint={
              echo
                ? t("pipe.noise.suppressionLive")
                : t("pipe.noise.suppressionNeedsEcho")
            }
            standing={LIVE}
          >
            <Choices
              label={t("pipe.noise.suppression")}
              current={level}
              disabled={!echo || saving}
              onPick={(id) => void run(() => audioBridge.setNoiseSuppression(id, pid))}
              options={[
                { id: "off", label: t("pipe.off") },
                { id: "standard", label: t("pipe.noise.standard") },
                { id: "strong", label: t("pipe.noise.strong") },
              ]}
            />
          </Option>
        ) : null}
      </Section>
      <Section title={t("pipe.voice.people")}>
        <VoiceCard pipelineId={perPipeline ? pipelineId : undefined} embedded />
      </Section>
      {endpoint !== null ? (
        <Facts
          items={[
            {
              label: t("pipe.noise.endpoint"),
              value:
                endpoint.startsWith("disabled") ||
                endpoint.startsWith("unavailable")
                  ? t("pipe.off")
                  : t("pipe.on"),
            },
          ]}
        />
      ) : null}
    </>
  );
}

/** `ListModels` needs a daemon at protocol minor 13 or newer. */
const MODELS_PROTOCOL_MINOR = 13;

/**
 * The daemon's model listing, re-read when the model in use or a model load
 * changes. Null until loaded or when the daemon cannot list models.
 */
function useModelListing(status: StatusSnapshot | null): {
  models: ModelInfo[] | null;
  setModels: (models: ModelInfo[]) => void;
} {
  const [models, setModels] = useState<ModelInfo[] | null>(null);
  const listable = (status?.protocol_minor ?? 0) >= MODELS_PROTOCOL_MINOR;
  const selection = cap(status, "model.selection=");
  useEffect(() => {
    if (!listable) {
      setModels(null);
      return;
    }
    let current = true;
    bridge
      .models()
      .then((list) => {
        if (current) setModels(list);
      })
      .catch(() => {
        if (current) setModels(null);
      });
    return () => {
      current = false;
    };
  }, [listable, selection]);
  return { models, setModels };
}

/** How often, and for how long, a requested model load is followed. */
const MODEL_LOAD_POLL_MS = 1000;
const MODEL_LOAD_POLLS = 120;

/** One downloaded model in the chooser: pick it and this pipeline transcribes with it. */
function ModelChoice({
  model,
  chosen,
  busy,
  runsOn,
  onChoose,
}: {
  model: ModelInfo;
  chosen: boolean;
  busy: boolean;
  /** Where the loaded model runs, when the service says. */
  runsOn: string | null;
  onChoose: (model: ModelInfo) => void;
}) {
  const loaded = model.state === "active";
  const loading = model.state === "loading";
  const bits = quantBits(model.quant);
  return (
    <button
      type="button"
      role="radio"
      className="pipe-model-choice"
      aria-checked={chosen}
      aria-label={modelName(model)}
      data-state={model.state}
      disabled={busy && !chosen}
      onClick={() => {
        if (!chosen && !loading) onChoose(model);
      }}
    >
      <span className="pipe-model-radio" aria-hidden="true" />
      <span className="pipe-model-choice-main">
        <span className="pipe-model-choice-name">
          {modelName(model)}
          {model.quant ? (
            <span className="model-quant" title={t("models.quantTitle")}>
              {model.quant}
              {bits ? <em>{bits}</em> : null}
            </span>
          ) : null}
        </span>
        <span className="pipe-model-choice-facts">
          {model.languages.length ? (
            <span>{languageSummary(model)}</span>
          ) : null}
          <span>
            {model.streaming
              ? t("models.tag.streaming")
              : t("models.tag.finalOnly")}
          </span>
          <span>{formatBytes(model.size_bytes)}</span>
        </span>
      </span>
      {loaded ? (
        <span className="chip chip-ok" title={t("pipe.model.loadedTitle")}>
          <Icon name="check" />
          {runsOn
            ? t("pipe.model.loadedOn", { device: runsOn })
            : t("pipe.model.loaded")}
        </span>
      ) : loading ? (
        <span className="chip pipe-model-loading">{t("models.loading")}</span>
      ) : chosen ? (
        <span className="chip">{t("pipe.model.loadsOnStart")}</span>
      ) : null}
    </button>
  );
}

function isCompute(value: string | undefined): value is PipelineCompute {
  return (
    value === "auto" || value === "gpu" || value === "npu" || value === "cpu"
  );
}

/** The devices a pipeline can ask for with `model`: the NPU only for a model
 * the service lists as able to run there. */
function computeChoices(model: ModelInfo | null): PipelineCompute[] {
  return model?.devices?.includes("NPU")
    ? ["auto", "npu", "gpu", "cpu"]
    : ["auto", "gpu", "cpu"];
}

function TranscribePanel({
  status,
  pipeline,
  pipelineName,
}: {
  status: StatusSnapshot | null;
  pipeline: PipelineSummary;
  pipelineName: string;
}) {
  const { navigate } = useNav();
  const onError = useErrorHandler();
  const { reload: reloadStatus } = useDaemonStatus();
  const { refresh: refreshPipelines } = usePipelines();
  const { models, setModels } = useModelListing(status);
  const [requested, setRequested] = useState<string | null>(null);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  const device = activeDevice(status);
  const deviceLabel = device.running
    ? t(`strip.device.${device.running}`)
    : t("home.status.notReported");
  const usable = usableModels(models ?? []);
  const loaded = usable.find((m) => m.state === "active") ?? null;
  // This pipeline's own model; one that chose none uses the loaded model.
  const chosen = usable.find((m) => m.id === pipeline.model_id) ?? loaded;
  const selection = cap(status, "model.selection=");
  const loading =
    selection === "loading" || usable.some((m) => m.state === "loading");
  const failed = selection?.startsWith("failed:")
    ? selection.slice("failed:".length)
    : null;
  // A service that reports no device choice cannot set one.
  const compute = isCompute(pipeline.compute) ? pipeline.compute : null;
  const choices = computeChoices(chosen);
  const npuOffered = choices.includes("npu");

  // Store the pipeline's choice, then follow the service's listing until the
  // model is loaded or the load is over. The service loads it at once when
  // nothing is dictating, keeps the previous model if the new one does not
  // load, and otherwise loads it when this pipeline next starts.
  const apply = async (
    modelId: string | null,
    nextCompute: PipelineCompute,
  ) => {
    setRequested(modelId ?? chosen?.id ?? "");
    try {
      await bridge.setPipelineEngine(pipeline.id, modelId, nextCompute);
      await refreshPipelines();
      const follow = modelId ?? chosen?.id ?? null;
      let sawLoading = false;
      for (let poll = 0; poll < MODEL_LOAD_POLLS; poll += 1) {
        const list = await bridge.models();
        if (!mounted.current) return;
        setModels(list);
        const anyLoading = list.some((m) => m.state === "loading");
        const state = list.find((m) => m.id === follow)?.state;
        if (anyLoading) sawLoading = true;
        else if (
          sawLoading ||
          poll >= 2 ||
          (state === "active" && modelId !== null && modelId !== loaded?.id)
        )
          break;
        await new Promise((resolve) =>
          window.setTimeout(resolve, MODEL_LOAD_POLL_MS),
        );
        if (!mounted.current) return;
      }
    } catch (error) {
      if (mounted.current) onError(error);
    } finally {
      if (mounted.current) {
        setRequested(null);
        // The device, languages and live-text facts follow the new engine.
        void reloadStatus().catch(() => {});
      }
    }
  };

  return (
    <>
      <Section title={t("pipe.sec.model")}>
      <Option
        label={t("pipe.model.choose")}
        hint={t("pipe.model.chooseHint", { pipeline: pipelineName })}
        standing={LIVE}
        block
      >
        {usable.length ? (
          <div
            className="pipe-model-choices"
            role="radiogroup"
            aria-label={t("pipe.model.choose")}
          >
            {usable.map((model) => (
              <ModelChoice
                key={model.id}
                model={
                  requested === model.id && model.state === "installed"
                    ? { ...model, state: "loading" }
                    : model
                }
                chosen={chosen?.id === model.id}
                busy={loading || requested !== null}
                runsOn={device.running ? deviceLabel : null}
                onChoose={(picked) => void apply(picked.id, compute ?? "auto")}
              />
            ))}
          </div>
        ) : (
          <p className="pipe-option-hint">{t("pipe.model.noneListed")}</p>
        )}
        {failed ? (
          <p className="pipe-model-error" role="alert">
            {t("pipe.model.failed", { reason: failed })}
          </p>
        ) : null}
        <button
          type="button"
          className="link-action link-quiet pipe-model-more"
          onClick={() => navigate("models", "speech")}
        >
          <Icon name="download" />
          {t("pipe.model.more")}
          <Icon name="arrowRight" />
        </button>
      </Option>
      <Option
        label={t("pipe.model.accel")}
        hint={
          npuOffered ? t("pipe.model.accelHint") : t("pipe.model.accelNoNpu")
        }
        standing={compute ? LIVE : REPORTED}
      >
        <div
          className="pipe-choices"
          role="radiogroup"
          aria-label={t("pipe.model.accel")}
        >
          {choices.map((choice) => (
            <button
              key={choice}
              type="button"
              role="radio"
              className="pipe-choice pipe-choice-live"
              aria-checked={(compute ?? "auto") === choice}
              disabled={compute === null || loading || requested !== null}
              onClick={() => {
                if (compute !== choice)
                  void apply(pipeline.model_id ?? null, choice);
              }}
            >
              {t(`pipe.model.compute.${choice}`)}
            </button>
          ))}
          {npuOffered ? null : (
            <button
              type="button"
              className="pipe-choice pipe-choice-link"
              title={t("pipe.model.npuNote")}
              onClick={() => navigate("store")}
            >
              {t("pipe.model.compute.npu")}
              <em>{t("pipe.model.npuNote")}</em>
            </button>
          )}
        </div>
      </Option>
      </Section>
      <DictationLanguage status={status} />
      <LanguageShortcuts
        status={status}
        pipelineId={pipeline.id}
        pipelineName={pipelineName}
      />
      <Section title={t("pipe.sec.vocabulary")}>
        <VocabularyEditor />
      </Section>
      <details className="pipe-more">
        <summary>
          <Icon name="sliders" />
          {t("pipe.lang.more")}
        </summary>
        <RecognitionSettingsCard embedded />
      </details>
    </>
  );
}

/** Shortcuts that start this pipeline in a set spoken and written language. */
/** The language dictation writes in, for every pipeline (the daemon keeps
 * it): Automatic or one of the user's languages. The tray's Language menu
 * chooses from the same list. Hidden on a daemon without it. */
function DictationLanguage({ status }: { status: StatusSnapshot | null }) {
  const pid = usePipelineId();
  const report = useErrorHandler();
  const options = recognitionOptions(status);
  const [text, setText] = useState<TextOptions | null>(null);
  const [adding, setAdding] = useState("");
  useEffect(() => {
    let alive = true;
    audioBridge.textOptions(pid).then(
      (o) => alive && setText(o),
      () => alive && setText(null),
    );
    return () => {
      alive = false;
    };
  }, []);
  if (!text || text.dictation_languages === undefined) {
    return null;
  }
  const current = text.dictation_language ?? null;
  const listed = text.dictation_languages;
  const addable = options.languages.filter((tag) => !listed.includes(tag));
  const save = (change: Promise<TextOptions>) =>
    change.then(setText, (error: unknown) => report(error));

  return (
    <Section icon="globe" title={t("pipe.lang.current")} lead={t("pipe.lang.currentHint")}>
      <div role="radiogroup" aria-label={t("pipe.lang.current")}>
        {[null, ...listed].map((tag) => {
          const name = tag ? languageName(tag) : t("pipe.lang.auto");
          return (
            <div key={tag ?? "auto"} className="pipe-lang-row">
              <label className="pipe-lang-pair">
                <input
                  type="radio"
                  name="dictation-language"
                  checked={current === tag}
                  onChange={() =>
                    void save(audioBridge.setDictationLanguage(tag ?? "auto", pid))
                  }
                />{" "}
                {name}
              </label>
              <span className="card-head-spacer" />
              {tag ? (
                <button
                  type="button"
                  className="icon-btn icon-btn-danger"
                  aria-label={t("pipe.lang.removeLanguage", { name })}
                  title={t("pipe.lang.removeLanguage", { name })}
                  onClick={() =>
                    void save(
                      audioBridge.setDictationLanguages(
                        listed.filter((other) => other !== tag),
                      ),
                    )
                  }
                >
                  <Icon name="trash" />
                </button>
              ) : null}
            </div>
          );
        })}
      </div>
      {listed.length === 0 ? (
        <p className="pipe-option-hint">{t("pipe.lang.noLanguages")}</p>
      ) : null}
      {addable.length > 0 && options.languages.length > 1 ? (
        <div className="pipe-lang-add">
          <select
            value={adding}
            onChange={(e) => setAdding(e.target.value)}
            aria-label={t("pipe.lang.pick")}
          >
            <option value="">{t("pipe.lang.pick")}</option>
            {addable.map((tag) => (
              <option key={tag} value={tag}>
                {languageName(tag)}
              </option>
            ))}
          </select>
          <button
            type="button"
            className="btn"
            disabled={!adding}
            onClick={() => {
              const tag = adding;
              setAdding("");
              void save(audioBridge.setDictationLanguages([...listed, tag]));
            }}
          >
            <Icon name="plus" />
            {t("pipe.lang.addLanguage")}
          </button>
        </div>
      ) : null}
    </Section>
  );
}

function LanguageShortcuts({
  status,
  pipelineId,
  pipelineName,
}: {
  status: StatusSnapshot | null;
  pipelineId: string;
  pipelineName: string;
}) {
  const { info, managed, command, capture, remove } = useContext(ShortcutsContext);
  const options = recognitionOptions(status);
  const [spoken, setSpoken] = useState("auto");
  const [output, setOutput] = useState("auto");
  const outputChoices = options.translationTargets.length
    ? options.translationTargets
    : options.languages;
  const list = (info?.bindings ?? []).filter(
    (b) =>
      b.pipeline_id === pipelineId &&
      (b.spoken_language !== undefined || b.output_language !== undefined),
  );
  const nothingChosen = spoken === "auto" && output === "auto";

  return (
    <Section icon="keyboard" title={t("pipe.lang.shortcuts")} lead={t("pipe.lang.shortcutsHint")}>
      {list.map((binding) => (
        <div key={comboLabel(binding)} className="pipe-lang-row">
          <span className="pipe-lang-pair">
            {languageName(binding.spoken_language ?? "auto")}{" "}
            <Icon name="arrowRight" />{" "}
            {languageName(binding.output_language ?? "auto")}
          </span>
          <KeyChips parts={comboParts(binding)} />
          <span className="card-head-spacer" />
          <button
            type="button"
            className="icon-btn icon-btn-danger"
            aria-label={t("pipe.shortcut.remove", {
              keys: comboLabel(binding),
            })}
            title={t("pipe.shortcut.remove", { keys: comboLabel(binding) })}
            onClick={() => remove(binding)}
          >
            <Icon name="trash" />
          </button>
        </div>
      ))}
      <div className="pipe-lang-add">
        <label>
          {t("pipe.lang.spoken")}
          <select
            value={spoken}
            onChange={(e) => setSpoken(e.target.value)}
            aria-label={t("pipe.lang.spoken")}
          >
            <option value="auto">{t("pipe.lang.auto")}</option>
            {options.languages.map((tag) => (
              <option key={tag} value={tag}>
                {languageName(tag)}
              </option>
            ))}
          </select>
        </label>
        <label>
          {t("pipe.lang.written")}
          <select
            value={output}
            onChange={(e) => setOutput(e.target.value)}
            aria-label={t("pipe.lang.written")}
          >
            <option value="auto">{t("pipe.lang.auto")}</option>
            {outputChoices.map((tag) => (
              <option key={tag} value={tag}>
                {languageName(tag)}
              </option>
            ))}
          </select>
        </label>
        {managed ? (
          <button
            type="button"
            className="btn"
            disabled={nothingChosen}
            onClick={() =>
              capture({
                pipelineId,
                pipelineName,
                spokenLanguage: spoken === "auto" ? undefined : spoken,
                outputLanguage: output === "auto" ? undefined : output,
              })
            }
          >
            <Icon name="plus" />
            {t("pipe.lang.add")}
          </button>
        ) : null}
      </div>
      {!nothingChosen && command ? (
        <details className="pipe-manual">
          <summary>{t("pipe.shortcut.manual")}</summary>
          <CopyCommand
            command={shortcutCommand(pipelineId, spoken, output, command)}
            label={t("pipe.shortcut.copy")}
          />
        </details>
      ) : null}
      {options.languages.length <= 1 ? (
        <p className="pipe-option-hint">{t("pipe.lang.oneLanguage")}</p>
      ) : null}
    </Section>
  );
}

/** Paste (then put the clipboard back) or type. */
function DeliveryMethodOption() {
  const pid = usePipelineId();
  const report = useErrorHandler();
  const [method, setMethod] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  useEffect(() => {
    audioBridge.textOptions(pid).then(
      (o) => setMethod(o.delivery_method || null),
      () => setMethod(null),
    );
  }, []);
  if (!method) return null;
  const pick = async (next: string) => {
    setSaving(true);
    try {
      setMethod(
        (await audioBridge.setDeliveryMethod(next, pid)).delivery_method ?? null,
      );
    } catch (err) {
      report(err, { retry: () => void pick(next) });
    } finally {
      setSaving(false);
    }
  };
  return (
    <Option
      label={t("pipe.deliver.method")}
      hint={
        method === "paste-keyboard"
          ? t("pipe.deliver.keyboardHint")
          : method === "type"
            ? t("pipe.deliver.typeHint")
            : t("pipe.deliver.methodLive")
      }
      standing={LIVE}
    >
      <Choices
        label={t("pipe.deliver.method")}
        current={method}
        onPick={(id) => void pick(id)}
        disabled={saving}
        options={[
          { id: "paste", label: t("pipe.deliver.paste") },
          { id: "paste-keyboard", label: t("pipe.deliver.pasteKeyboard") },
          { id: "type", label: t("pipe.deliver.type") },
        ]}
      />
    </Option>
  );
}

/** The text options, read once; `save` sends a change and keeps the reply. */
function useTextOptions(): {
  text: TextOptions | null;
  saving: boolean;
  save: (change: () => Promise<TextOptions>) => Promise<void>;
} {
  const report = useErrorHandler();
  const pid = usePipelineId();
  const [text, setText] = useState<TextOptions | null>(null);
  const [saving, setSaving] = useState(false);
  useEffect(() => {
    let alive = true;
    audioBridge.textOptions(pid).then(
      (o) => alive && setText(o),
      () => alive && setText(null),
    );
    return () => {
      alive = false;
    };
  }, []);
  const save = async (change: () => Promise<TextOptions>) => {
    setSaving(true);
    try {
      setText(await change());
    } catch (err) {
      report(err, { retry: () => void save(change) });
    } finally {
      setSaving(false);
    }
  };
  return { text, saving, save };
}

/** An on/off option: the label and its one-line hint, a switch beside. */
function SwitchOption({
  label,
  hint,
  on,
  disabled = false,
  onToggle,
}: {
  label: string;
  hint?: string;
  on: boolean;
  disabled?: boolean;
  onToggle: () => void;
}) {
  return (
    <Option label={label} hint={hint} standing={LIVE}>
      <button
        type="button"
        role="switch"
        className="switch"
        aria-checked={on}
        aria-label={label}
        disabled={disabled}
        onClick={onToggle}
      >
        <span className="switch-thumb" />
      </button>
    </Option>
  );
}

/** The deterministic corrector's rules, in the order it applies them. */
const REFINE_RULES = ["spacing", "punctuation", "capitals", "ending"] as const;

/** Idle minutes after which the clean-up model is unloaded; 0 never. */
const IDLE_MINUTES = [0, 5, 15, 30, 60];

type TextSave = (change: () => Promise<TextOptions>) => Promise<void>;

/** The rules: always on this computer, the same result for the same words. */
function RulesGroup({
  text,
  saving,
  save,
}: {
  text: TextOptions;
  saving: boolean;
  save: TextSave;
}) {
  const pid = usePipelineId();
  const rules = text.refine_rules;
  return (
    <Section title={t("refine.rules.title")} lead={t("refine.rules.lead")}>
      {REFINE_RULES.map((rule) =>
        rules ? (
          <SwitchOption
            key={rule}
            label={t(`refine.rule.${rule}`)}
            hint={t(`refine.rule.${rule}Hint`)}
            on={rules[rule]}
            disabled={saving}
            onToggle={() =>
              void save(() =>
                audioBridge.setRefineOptions({
                  refineRules: { ...rules, [rule]: !rules[rule] },
                }, pid),
              )
            }
          />
        ) : (
          <Option
            key={rule}
            label={t(`refine.rule.${rule}`)}
            hint={t(`refine.rule.${rule}Hint`)}
            value={t("pipe.on")}
            standing={REPORTED}
          />
        ),
      )}
    </Section>
  );
}

/** How numbers and spoken line breaks are written. */
function FormattingGroup({
  text,
  saving,
  save,
}: {
  text: TextOptions;
  saving: boolean;
  save: TextSave;
}) {
  const pid = usePipelineId();
  const typed = text.delivery_method === "type";
  return (
    <Section title={t("refine.format.title")}>
      <SwitchOption
        label={t("pipe.refine.numbers")}
        hint={t("pipe.refine.numbersLive")}
        on={text.numbers_as_digits}
        disabled={saving}
        onToggle={() =>
          void save(() =>
            audioBridge.setNumbersAsDigits(!text.numbers_as_digits, pid),
          )
        }
      />
      {text.spoken_breaks !== undefined ? (
        <SwitchOption
          label={t("pipe.refine.breaks")}
          hint={
            typed && text.spoken_breaks
              ? t("pipe.refine.breaksTyped")
              : t("pipe.refine.breaksHint")
          }
          on={text.spoken_breaks}
          disabled={saving}
          onToggle={() =>
            void save(() => audioBridge.setSpokenBreaks(!text.spoken_breaks, pid))
          }
        />
      ) : null}
    </Section>
  );
}

/** The optional clean-up language model (D31): chosen here, and its
 * settings shown only while one is in use. "None" is the rules alone. */
function CleanupGroup({
  text,
  saving,
  save,
}: {
  text: TextOptions;
  saving: boolean;
  save: TextSave;
}) {
  const pid = usePipelineId();
  const report = useErrorHandler();
  const { navigate } = useNav();
  const [models, setModels] = useState<LanguageModel[] | null>(null);
  const [choosing, setChoosing] = useState(false);
  useEffect(() => {
    bridge.languageModels().then(setModels, () => setModels(null));
  }, []);
  if (models === null) return null;
  const usable = models.filter(
    (m) => m.roles.includes("clean_up") && m.installed !== false,
  );
  const current = models.find((m) => m.in_use_for.includes("clean_up"));
  const level = text.cleanup_level;
  const off = !current || level === "off";
  const choose = async (id: string) => {
    setChoosing(true);
    try {
      await bridge.selectLanguageModel("clean_up", id || null);
      if (id && level === "off") {
        await save(() =>
          audioBridge.setRefineOptions({ cleanupLevel: "faithful" }, pid),
        );
      }
      setModels(await bridge.languageModels());
    } catch (err) {
      report(err, { retry: () => void choose(id) });
    } finally {
      setChoosing(false);
    }
  };
  const state = current ? cleanUpState(current.clean_up_state) : null;
  return (
    <>
      <Section title={t("refine.model.title")} lead={t("refine.model.lead")}>
        <Option
          label={t("pipe.refine.cleanupModel")}
          hint={
            usable.length
              ? t("pipe.refine.cleanupModelHint")
              : t("pipe.refine.cleanupNoneHere")
          }
          standing={LIVE}
        >
          {usable.length ? (
            <select
              className="pipe-select"
              aria-label={t("pipe.refine.cleanupModel")}
              value={off ? "" : current.id}
              disabled={choosing || saving}
              onChange={(e) => void choose(e.target.value)}
            >
              <option value="">{t("pipe.refine.cleanupNone")}</option>
              {usable.map((m) => (
                <option key={m.id} value={m.id}>
                  {m.name}
                </option>
              ))}
            </select>
          ) : (
            <button
              type="button"
              className="btn btn-secondary btn-sm"
              onClick={() => navigate("models", "correction")}
            >
              {t("pipe.refine.chooseModel")}
            </button>
          )}
        </Option>
        {!off && level ? (
          <Option
            label={t("refine.level.title")}
            standing={LIVE}
            block
          >
            <Choices
              label={t("refine.level.title")}
              current={level}
              disabled={saving}
              onPick={(id) =>
                void save(() =>
                  audioBridge.setRefineOptions({
                    cleanupLevel: id as CleanupLevel,
                  }, pid),
                )
              }
              options={[
                {
                  id: "faithful",
                  label: t("refine.level.faithful"),
                  note: t("refine.level.faithfulNote"),
                },
                {
                  id: "polish",
                  label: t("refine.level.polish"),
                  note: t("refine.level.polishNote"),
                },
                {
                  id: "rewrite",
                  label: t("refine.level.rewrite"),
                  note: t("refine.level.rewriteNote"),
                },
              ]}
            />
          </Option>
        ) : null}
        {!off && text.cleanup_compute ? (
          <Option
            label={t("pipe.model.accel")}
            hint={t("refine.model.whereLive")}
            standing={LIVE}
          >
            <Choices
              label={t("pipe.model.accel")}
              current={text.cleanup_compute}
              disabled={saving}
              onPick={(id) =>
                void save(() =>
                  audioBridge.setRefineOptions({
                    cleanupCompute: id as PipelineCompute,
                  }, pid),
                )
              }
              options={[
                { id: "auto", label: t("pipe.model.compute.auto") },
                { id: "npu", label: "NPU" },
                { id: "gpu", label: "GPU" },
                { id: "cpu", label: "CPU" },
              ]}
            />
          </Option>
        ) : null}
        {!off && text.cleanup_idle_minutes !== undefined ? (
          <Option
            label={t("pipe.refine.idle")}
            hint={t("pipe.refine.idleHint")}
            standing={LIVE}
          >
            <select
              className="pipe-select"
              aria-label={t("pipe.refine.idle")}
              value={text.cleanup_idle_minutes}
              disabled={saving}
              onChange={(e) =>
                void save(() =>
                  audioBridge.setCleanupIdle(Number(e.target.value), pid),
                )
              }
            >
              {IDLE_MINUTES.map((m) => (
                <option key={m} value={m}>
                  {m === 0
                    ? t("pipe.refine.idleNever")
                    : t("pipe.refine.idleAfter", { minutes: m })}
                </option>
              ))}
            </select>
          </Option>
        ) : null}
      </Section>
      <Facts
        items={[
          !off && state
            ? { label: t("pipe.refine.cleanupModel"), value: state.label }
            : null,
          !off ? { label: t("pipe.refine.guard"), value: t("pipe.on") } : null,
          { label: t("refine.privacy"), value: t("refine.localOnly") },
        ]}
      />
    </>
  );
}

/** Refine (D23, D38): how this pipeline writes; for Clean, the rules, the
 * formatting and the optional clean-up model. */
function RefinePanel({
  pipeline,
  editable,
  onRefine,
}: {
  pipeline: PipelineSummary;
  editable: boolean;
  onRefine: (refine: PipelineRefine) => void;
}) {
  const { text, saving, save } = useTextOptions();
  const refine = pipelineRefine(pipeline);
  const clean = refine === "clean";
  return (
    <>
      <Section title={t("pipe.refine.mode")}>
        <Option
          label={t("pipe.refine.modeChoose")}
          hint={clean ? t("pipe.refine.cleanHint") : t("pipe.refine.rawHint")}
          value={
            editable
              ? undefined
              : clean
                ? t("pipe.refine.clean")
                : t("pipe.refine.raw")
          }
          standing={editable ? LIVE : REPORTED}
        >
          {editable ? (
            <Choices
              label={t("pipe.refine.mode")}
              current={refine}
              onPick={(id) => onRefine(id as PipelineRefine)}
              options={[
                { id: "clean", label: t("pipe.refine.clean") },
                { id: "raw", label: t("pipe.refine.raw") },
              ]}
            />
          ) : null}
        </Option>
      </Section>
      {clean && text ? (
        <>
          <RulesGroup text={text} saving={saving} save={save} />
          <FormattingGroup text={text} saving={saving} save={save} />
          <CleanupGroup text={text} saving={saving} save={save} />
        </>
      ) : null}
    </>
  );
}

/** A space after each dictation, so the next one does not run on. Hidden on
 * a service without it. */
function SpaceAfterOption() {
  const pid = usePipelineId();
  const { text, saving, save } = useTextOptions();
  if (text?.space_after === undefined) return null;
  const on = text.space_after;
  return (
    <Option
      label={t("pipe.deliver.space")}
      hint={t("pipe.deliver.spaceHint")}
      standing={LIVE}
    >
      <button
        type="button"
        role="switch"
        className="switch"
        aria-checked={on}
        aria-label={t("pipe.deliver.space")}
        disabled={saving}
        onClick={() =>
          void save(() => audioBridge.setRefineOptions({ spaceAfter: !on }, pid))
        }
      >
        <span className="switch-thumb" />
      </button>
    </Option>
  );
}

function DeliverPanel({ status }: { status: StatusSnapshot | null }) {
  const inbox = cap(status, "delivery.inbox-recovery=");
  const verified = cap(status, "delivery.verified=");
  return (
    <>
      <Section title={t("pipe.sec.delivery")}>
        <DeliveryMethodOption />
        <SpaceAfterOption />
      </Section>
      <Facts
        items={[
          {
            label: t("pipe.deliver.inbox"),
            value:
              inbox === "enabled" ? t("pipe.deliver.inboxKept") : t("pipe.off"),
          },
          {
            label: t("pipe.deliver.verified"),
            value:
              verified && !verified.startsWith("unavailable")
                ? t("pipe.on")
                : t("pipe.notYet"),
          },
        ]}
      />
    </>
  );
}

// -------------------------------------------------------------- the screen

export function PipelinesScreen() {
  const { catalog, select, refresh } = usePipelines();
  const { status, manualCommand } = useDaemonStatus();
  const report = useErrorHandler();
  // A service that keeps the pipelines (minor 56) owns their names and
  // order; an older one leaves them to this app's own preferences.
  const editable = (status?.protocol_minor ?? 0) >= PIPELINE_EDIT_PROTOCOL_MINOR;
  const [prefs, setPrefs] = useState<PipelinePrefs>(loadPipelinePrefs);
  const [open, setOpen] = useState<{ pipeline: string; stage: StageId } | null>(
    null,
  );
  const [editing, setEditing] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [dragging, setDragging] = useState<string | null>(null);
  const [dropIndex, setDropIndex] = useState<number | null>(null);
  const [deleting, setDeleting] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const [shortcutsInfo, setShortcutsInfo] = useState<ShortcutsInfo | null>(
    null,
  );
  const [captureTarget, setCaptureTarget] = useState<CaptureTarget | null>(
    null,
  );
  const shortcutsServed =
    (status?.protocol_minor ?? 0) >= SHORTCUTS_PROTOCOL_MINOR;
  const loadShortcuts = useCallback(async () => {
    if (!shortcutsServed) {
      setShortcutsInfo(null);
      return;
    }
    try {
      setShortcutsInfo(await bridge.shortcuts());
    } catch {
      // Without a listing the screen offers the copy-a-command route only.
      setShortcutsInfo(null);
    }
  }, [shortcutsServed]);
  useEffect(() => {
    void loadShortcuts();
  }, [loadShortcuts]);
  const removeShortcut = useCallback(
    async (binding: ShortcutBinding) => {
      try {
        await bridge.removeShortcut(binding.modifiers, binding.key);
      } catch (err) {
        report(err, { retry: () => void removeShortcut(binding) });
      }
      await loadShortcuts();
    },
    [loadShortcuts, report],
  );
  const managed = shortcutsInfo?.support === "managed";
  const shortcutsContext = useMemo<ShortcutsContextValue>(
    () => ({
      info: shortcutsInfo,
      managed,
      command: shortcutsInfo?.manual_command || manualCommand,
      capture: setCaptureTarget,
      remove: (binding) => void removeShortcut(binding),
    }),
    [shortcutsInfo, managed, manualCommand, removeShortcut],
  );

  const pipelines = useMemo(
    () =>
      editable
        ? catalog.pipelines
        : orderPipelines(catalog.pipelines, prefs.order),
    [editable, catalog.pipelines, prefs.order],
  );
  const ids = pipelines.map((p) => p.id);
  // A service from before D50 keeps one set of options for all pipelines.
  const ownOptions =
    (status?.protocol_minor ?? 0) >= PIPELINE_OPTIONS_PROTOCOL_MINOR;
  const recordingWith = status?.active_session?.pipeline_id ?? null;
  const snap = status ?? { capabilities: [], active_session: null };
  // Each pipeline's own Voice and Deliver choices, for its stage chips.
  const idsKey = ids.join(",");
  const [chips, setChips] = useState<
    Record<string, { audio: AudioOptions | null; method: string | null }>
  >({});
  useEffect(() => {
    let alive = true;
    const targets = ownOptions ? idsKey.split(",").filter(Boolean) : [""];
    void Promise.all(
      targets.map(async (id) => {
        const pipeline = id || undefined;
        const [audio, text] = await Promise.all([
          audioBridge.audioOptions(pipeline).catch(() => null),
          audioBridge.textOptions(pipeline).catch(() => null),
        ]);
        return [id, { audio, method: text?.delivery_method || null }] as const;
      }),
    ).then((found) => {
      if (alive) setChips(Object.fromEntries(found));
    });
    return () => {
      alive = false;
    };
  }, [open, idsKey, ownOptions]);
  const chipsFor = (id: string) =>
    chips[ownOptions ? id : ""] ?? { audio: null, method: null };
  const summary: Record<StageId, string> = {
    mic: micChip(resolveMic(snap as never, "auto")).label,
    noise: "",
    transcribe: modelChip(resolveModel(snap as never, "auto")).label,
    refine: "",
    deliver: deliveryChip(resolveDelivery(snap as never, "auto")).label.replace(
      / · Inbox backup$/,
      "",
    ),
  };

  /** One change to the pipelines, then the new listing. */
  const edit = async (change: PipelineEdit): Promise<PipelineSummary[] | null> => {
    setBusy(true);
    try {
      const listed = await bridge.editPipeline(change);
      await refresh();
      return listed;
    } catch (err) {
      report(err, { retry: () => void edit(change) });
      return null;
    } finally {
      setBusy(false);
    }
  };

  const persist = (next: PipelinePrefs) => {
    savePipelinePrefs(next);
    setPrefs(next);
  };
  const displayName = (pipeline: PipelineSummary) =>
    editable ? pipeline.name : (prefs.names[pipeline.id] ?? pipeline.name);

  const move = (id: string, index: number) => {
    const order = moveInOrder(ids, id, index);
    if (editable) void edit({ op: "reorder", order });
    else persist({ ...prefs, order });
  };

  const startRename = (id: string, current: string) => {
    setEditing(id);
    setDraft(current);
  };
  const commitRename = (pipeline: PipelineSummary) => {
    setEditing(null);
    const name = draft.trim().slice(0, MAX_NAME_LENGTH);
    if (editable) {
      if (name && name !== pipeline.name)
        void edit({ op: "rename", pipelineId: pipeline.id, name });
      return;
    }
    const names = { ...prefs.names };
    if (!name || name === pipeline.name) delete names[pipeline.id];
    else names[pipeline.id] = name;
    persist({ ...prefs, names });
  };

  const create = async () => {
    const listed = await edit({ op: "create" });
    const created = listed?.[listed.length - 1];
    if (created) startRename(created.id, created.name);
  };
  const remove = async (pipeline: PipelineSummary) => {
    setDeleting(null);
    if (open?.pipeline === pipeline.id) setOpen(null);
    if ((await edit({ op: "delete", pipelineId: pipeline.id })) !== null)
      void loadShortcuts();
  };

  const onDragStart = (event: DragEvent, id: string) => {
    setDragging(id);
    event.dataTransfer.effectAllowed = "move";
    event.dataTransfer.setData("text/plain", id);
  };
  const onDragOver = (event: DragEvent, index: number) => {
    if (!dragging) return;
    event.preventDefault();
    const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
    setDropIndex(
      event.clientY < rect.top + rect.height / 2 ? index : index + 1,
    );
  };
  const onDrop = (event: DragEvent) => {
    event.preventDefault();
    if (dragging && dropIndex !== null) {
      const from = ids.indexOf(dragging);
      move(dragging, dropIndex > from ? dropIndex - 1 : dropIndex);
    }
    setDragging(null);
    setDropIndex(null);
  };

  return (
    <ShortcutsContext.Provider value={shortcutsContext}>
      <div className="page-wide">
        <div className="page-head page-head-row">
          <div>
            <h2 className="page-title">{t("nav.pipelines")}</h2>
            <p className="page-lead">{t("pipe.lead")}</p>
          </div>
          <div className="page-head-actions">
            <button
              type="button"
              className="btn btn-primary"
              disabled={!editable || busy}
              title={editable ? undefined : t("pipe.needsUpdate")}
              onClick={() => void create()}
            >
              <Icon name="plus" />
              {t("pipelinesPage.new")}
            </button>
          </div>
        </div>

        <ol className="pipe-list" onDragLeave={() => setDropIndex(null)}>
          {pipelines.map((pipeline, index) => {
            const active = pipeline.id === catalog.selectedId;
            const name = displayName(pipeline);
            const refine = pipelineRefine(pipeline);
            const openStage =
              open?.pipeline === pipeline.id ? open.stage : null;
            const stageValue = (stage: StageId) =>
              stage === "refine"
                ? t(refine === "clean" ? "pipe.refine.clean" : "pipe.refine.raw")
                : stage === "mic" && chipsFor(pipeline.id).audio?.microphone
                  ? (chipsFor(pipeline.id).audio?.microphones?.find(
                      (m) => m.name === chipsFor(pipeline.id).audio?.microphone,
                    )?.description ?? summary.mic)
                  : stage === "noise"
                  ? voiceSummary(chipsFor(pipeline.id).audio, pipeline.id)
                  : stage === "deliver" && chipsFor(pipeline.id).method
                    ? t(
                        chipsFor(pipeline.id).method === "type"
                          ? "pipe.deliver.chipType"
                          : "pipe.deliver.chipPaste",
                      )
                    : summary[stage];
            const deletable =
              editable && pipelines.length > 1 && recordingWith !== pipeline.id;
            return (
              <li
                key={pipeline.id}
                className="pipe-card"
                data-active={active ? "true" : "false"}
                data-dragging={dragging === pipeline.id ? "true" : "false"}
                data-drop={
                  dropIndex === index
                    ? "before"
                    : dropIndex === index + 1 && index === pipelines.length - 1
                      ? "after"
                      : undefined
                }
                onDragOver={(event) => onDragOver(event, index)}
                onDrop={onDrop}
              >
                <div className="pipe-head">
                  {pipelines.length > 1 ? (
                    <>
                      <span
                        className="pipe-grip"
                        draggable
                        onDragStart={(event) => onDragStart(event, pipeline.id)}
                        onDragEnd={() => {
                          setDragging(null);
                          setDropIndex(null);
                        }}
                        title={t("pipe.drag")}
                        aria-hidden="true"
                      >
                        <Icon name="grip" />
                      </span>
                      <span
                        className="pipe-order"
                        role="group"
                        aria-label={t("pipe.orderLabel", { name })}
                      >
                        <button
                          type="button"
                          className="icon-btn"
                          disabled={index === 0 || busy}
                          onClick={() => move(pipeline.id, index - 1)}
                          aria-label={t("pipe.moveUp", { name })}
                        >
                          <Icon name="chevronDown" className="icon-flip" />
                        </button>
                        <button
                          type="button"
                          className="icon-btn"
                          disabled={index === pipelines.length - 1 || busy}
                          onClick={() => move(pipeline.id, index + 1)}
                          aria-label={t("pipe.moveDown", { name })}
                        >
                          <Icon name="chevronDown" />
                        </button>
                      </span>
                    </>
                  ) : null}
                  {editing === pipeline.id ? (
                    <input
                      className="pipe-name-input"
                      value={draft}
                      maxLength={MAX_NAME_LENGTH}
                      autoFocus
                      onFocus={(e) => e.currentTarget.select()}
                      aria-label={t("pipe.rename", { name })}
                      onChange={(e) => setDraft(e.target.value)}
                      onBlur={() => commitRename(pipeline)}
                      onKeyDown={(e) => {
                        if (e.key === "Enter") commitRename(pipeline);
                        if (e.key === "Escape") setEditing(null);
                      }}
                    />
                  ) : (
                    <button
                      type="button"
                      className="pipe-name"
                      onClick={() => startRename(pipeline.id, name)}
                      title={t("pipe.renameHint")}
                    >
                      <h3>{name}</h3>
                      <Icon name="edit" />
                    </button>
                  )}
                  {!editable && name !== pipeline.name ? (
                    <span className="pipe-origin">{pipeline.name}</span>
                  ) : null}
                  {active ? (
                    <span className="chip chip-ok">
                      {t("pipelinesPage.active")}
                    </span>
                  ) : null}
                  {!pipeline.enabled ? (
                    <span className="chip chip-warn">
                      {t("dictation.modeUnavailable")}
                    </span>
                  ) : null}
                  <span className="card-head-spacer" />
                  {managed ? (
                    <div
                      className="pipe-keys"
                      role="group"
                      aria-label={t("pipe.shortcut.title")}
                      title={t("pipe.shortcut.hint")}
                    >
                      {(() => {
                        const bindings = (shortcutsInfo?.bindings ?? []).filter(
                          (b) =>
                            b.pipeline_id === pipeline.id &&
                            b.spoken_language === undefined &&
                            b.output_language === undefined,
                        );
                        const add = () =>
                          setCaptureTarget({
                            pipelineId: pipeline.id,
                            pipelineName: name,
                          });
                        return (
                          <>
                            {bindings.map((binding) => (
                              <span
                                key={comboLabel(binding)}
                                className="pipe-key-binding"
                              >
                                <KeyChips parts={comboParts(binding)} />
                                <button
                                  type="button"
                                  className="icon-btn"
                                  aria-label={t("pipe.shortcut.remove", {
                                    keys: comboLabel(binding),
                                  })}
                                  title={t("pipe.shortcut.remove", {
                                    keys: comboLabel(binding),
                                  })}
                                  onClick={() => void removeShortcut(binding)}
                                >
                                  <Icon name="close" />
                                </button>
                              </span>
                            ))}
                            {bindings.length ? (
                              <button
                                type="button"
                                className="icon-btn pipe-key-add"
                                aria-label={t("pipe.shortcut.addAnother")}
                                title={t("pipe.shortcut.addAnother")}
                                onClick={add}
                              >
                                <Icon name="plus" />
                              </button>
                            ) : (
                              <button type="button" className="btn btn-ghost" onClick={add}>
                                <Icon name="keyboard" />
                                {t("pipe.shortcut.add")}
                              </button>
                            )}
                          </>
                        );
                      })()}
                    </div>
                  ) : null}
                  {!active && pipeline.enabled ? (
                    <button
                      type="button"
                      className="btn btn-default"
                      onClick={() => select(pipeline.id)}
                    >
                      {t("pipelinesPage.use")}
                    </button>
                  ) : null}
                  {deleting === pipeline.id ? (
                    <span className="pipe-delete-confirm" role="group">
                      <button
                        type="button"
                        className="btn btn-danger"
                        disabled={busy}
                        onClick={() => void remove(pipeline)}
                      >
                        {t("pipe.delete.confirm")}
                      </button>
                      <button
                        type="button"
                        className="btn btn-ghost"
                        onClick={() => setDeleting(null)}
                      >
                        {t("pipe.delete.cancel")}
                      </button>
                    </span>
                  ) : editable && pipelines.length > 1 ? (
                    <button
                      type="button"
                      className="icon-btn icon-btn-danger"
                      disabled={!deletable || busy}
                      aria-label={t("pipe.delete.label", { name })}
                      title={
                        deletable
                          ? t("pipe.delete.label", { name })
                          : t("pipe.delete.inUse")
                      }
                      onClick={() => setDeleting(pipeline.id)}
                    >
                      <Icon name="trash" />
                    </button>
                  ) : null}
                </div>

                <div
                  className="pipe-stages"
                  role="tablist"
                  aria-label={t("pipe.stagesLabel", { name })}
                >
                  {STAGES.map((stage) => (
                    <button
                      key={stage.id}
                      type="button"
                      role="tab"
                      className="pipe-stage"
                      aria-selected={openStage === stage.id}
                      aria-expanded={openStage === stage.id}
                      onClick={() =>
                        setOpen(
                          openStage === stage.id
                            ? null
                            : { pipeline: pipeline.id, stage: stage.id },
                        )
                      }
                    >
                      <span className="stage-icon" aria-hidden="true">
                        <Icon name={stage.icon} />
                      </span>
                      <span className="pipe-stage-text">
                        <span className="stage-label">
                          {t(`pipe.stage.${stage.id}`)}
                        </span>
                        <span className="pipe-stage-value">
                          {stageValue(stage.id)}
                        </span>
                      </span>
                    </button>
                  ))}
                </div>

                {openStage ? (
                  <PipelineIdContext.Provider
                    value={ownOptions ? pipeline.id : undefined}
                  >
                  <div className="pipe-panel" role="tabpanel">
                    {openStage === "mic" ? <MicPanel status={status} /> : null}
                    {openStage === "noise" ? (
                      <VoicePanel status={status} pipelineId={pipeline.id} />
                    ) : null}
                    {openStage === "transcribe" ? (
                      <TranscribePanel
                        status={status}
                        pipeline={pipeline}
                        pipelineName={name}
                      />
                    ) : null}
                    {openStage === "refine" ? (
                      <RefinePanel
                        pipeline={pipeline}
                        editable={editable}
                        onRefine={(next) =>
                          void edit({
                            op: "refine",
                            pipelineId: pipeline.id,
                            refine: next,
                          })
                        }
                      />
                    ) : null}
                    {openStage === "deliver" ? (
                      <DeliverPanel status={status} />
                    ) : null}
                  </div>
                  </PipelineIdContext.Provider>
                ) : null}

                {!managed && shortcutsContext.command ? (
                  <div className="pipe-shortcut">
                    <span className="pipe-shortcut-label">
                      <Icon name="keyboard" />
                      {t("pipe.shortcut.title")}
                    </span>
                    <span className="pipe-option-hint">
                      {t("pipe.shortcut.manualOnly")}
                    </span>
                    <details className="pipe-manual" open>
                      <summary>{t("pipe.shortcut.manual")}</summary>
                      <CopyCommand
                        command={shortcutCommand(
                          pipeline.id,
                          "auto",
                          "auto",
                          shortcutsContext.command,
                        )}
                        label={t("pipe.shortcut.copy")}
                      />
                    </details>
                  </div>
                ) : null}
              </li>
            );
          })}
          {pipelines.length === 0 ? (
            <li className="card empty-state">
              <Icon name="pipeline" />
              <strong>{t("pipelines.empty")}</strong>
            </li>
          ) : null}
        </ol>
        <p className="pipe-footnote">
          {t(
            (status?.capabilities ?? []).includes("activation.shortcuts=managed")
              ? "pipe.footnoteManaged"
              : "pipe.footnote",
          )}
        </p>

        {captureTarget ? (
          <ShortcutCapture
            target={captureTarget}
            onClose={() => setCaptureTarget(null)}
            onSaved={() => {
              setCaptureTarget(null);
              void loadShortcuts();
            }}
          />
        ) : null}
      </div>
    </ShortcutsContext.Provider>
  );
}
