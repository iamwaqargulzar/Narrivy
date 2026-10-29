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
import { activeDevice, engineFacts, formatBytes } from "../../lib/overview";
import {
  languageSummary,
  modelName,
  quantBits,
  reportedLanguages,
  usableModels,
} from "../../lib/modelFacts";
import { reportedCaptureLimitSeconds } from "../../lib/capturePolicy";
import { recognitionOptions } from "../../lib/recognition";
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
  type PipelineSummary,
  type ShortcutBinding,
  type ShortcutsInfo,
  type StatusSnapshot,
} from "../../lib/bridge";
import { comboLabel, comboParts } from "../../lib/shortcutKeys";
import {
  KeyChips,
  ShortcutCapture,
  type CaptureTarget,
} from "../components/ShortcutCapture";
import {
  Badge,
  Choices,
  LIVE,
  Option,
  PROPOSED,
  REPORTED,
  planned,
} from "../components/StandingOptions";
import { PipelinesIoButtons } from "../components/PipelinesIo";
import { RecognitionSettingsCard } from "../components/SettingsCompute/RecognitionSettingsCard";
import "../components/SettingsCompute/SettingsCompute.css";
import { EchoCancelCard } from "../components/SettingsCompute/EchoCancelCard";
import { VoiceCard } from "../components/SettingsCompute/VoiceCard";
import {
  bridge as audioBridge,
  type AudioOptions,
  type TextOptions,
} from "../../lib/bridge";
import { VocabularyEditor } from "../components/VocabularyEditor";
import { cleanUpState } from "./LanguageModels";

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
  capture: (target: CaptureTarget) => void;
  remove: (binding: ShortcutBinding) => void;
}

const ShortcutsContext = createContext<ShortcutsContextValue>({
  info: null,
  managed: false,
  capture: () => undefined,
  remove: () => undefined,
});

function cap(status: StatusSnapshot | null, prefix: string): string | null {
  const found = status?.capabilities.find((c) => c.startsWith(prefix));
  return found ? found.slice(prefix.length) : null;
}

function languageName(code: string): string {
  if (code === "auto") return t("pipe.lang.auto");
  try {
    return (
      new Intl.DisplayNames([navigator.language || "en"], {
        type: "language",
      }).of(code) ?? code
    );
  } catch {
    return code;
  }
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
  const report = useErrorHandler();
  const [audio, setAudio] = useState<AudioOptions | null>(null);
  const [saving, setSaving] = useState(false);
  useEffect(() => {
    audioBridge.audioOptions().then(setAudio, () => setAudio(null));
  }, []);
  if (!audio || audio.sounds === undefined) return null;
  const toggle = async () => {
    setSaving(true);
    try {
      setAudio(await audioBridge.setSounds(!audio.sounds));
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
  const report = useErrorHandler();
  const [audio, setAudio] = useState<AudioOptions | null>(null);
  const [saving, setSaving] = useState(false);
  useEffect(() => {
    audioBridge.audioOptions().then(setAudio, () => setAudio(null));
  }, []);
  const choose = async (name: string) => {
    setSaving(true);
    try {
      setAudio(await audioBridge.setMicrophone(name));
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

function MicPanel({ status }: { status: StatusSnapshot | null }) {
  const snap = status ?? { capabilities: [], active_session: null };
  const mic = resolveMic(snap as never, "auto");
  const seconds = reportedCaptureLimitSeconds(status);
  const helper = cap(status, "capture.pipewire-helper=");
  return (
    <>
      <MicrophoneChoice fallback={micChip(mic).label} />
      <SoundsOption />
      <Option
        label={t("pipe.mic.capture")}
        value={
          helper?.startsWith("ready") ? t("pipe.ready") : t("pipe.notReady")
        }
        standing={REPORTED}
      />
      {seconds !== null ? (
        <Option
          label={t("pipe.mic.limit")}
          value={t("home.fact.captureLimit", {
            minutes: Math.max(1, Math.round(seconds / 60)),
          })}
          standing={REPORTED}
        />
      ) : null}
      <Option
        label={t("pipe.mic.gain")}
        hint={t("pipe.mic.gainHint")}
        standing={PROPOSED}
      />
      <Option
        label={t("pipe.mic.echo")}
        hint={t("pipe.mic.echoHint")}
        standing={PROPOSED}
      />
    </>
  );
}

/** How much background noise the microphone's audio processing removes. */
function NoiseOption() {
  const report = useErrorHandler();
  const [audio, setAudio] = useState<AudioOptions | null>(null);
  const [saving, setSaving] = useState(false);
  useEffect(() => {
    audioBridge.audioOptions().then(setAudio, () => setAudio(null));
  }, []);
  const pick = async (level: string) => {
    setSaving(true);
    try {
      setAudio(await audioBridge.setNoiseSuppression(level));
    } catch (err) {
      report(err, { retry: () => void pick(level) });
    } finally {
      setSaving(false);
    }
  };
  const level = audio?.noise_suppression || "standard";
  const usable =
    Boolean(audio?.echo_cancel) &&
    audio?.noise_suppression !== undefined &&
    audio.noise_suppression !== "";
  return (
    <Option
      label={t("pipe.noise.suppression")}
      hint={
        usable
          ? t("pipe.noise.suppressionLive")
          : t("pipe.noise.suppressionNeedsEcho")
      }
      standing={LIVE}
    >
      <Choices
        label={t("pipe.noise.suppression")}
        current={level}
        onPick={(id) => void pick(id)}
        disabled={!usable || saving}
        options={[
          { id: "off", label: t("pipe.off") },
          { id: "standard", label: t("pipe.noise.standard") },
          {
            id: "strong",
            label: t("pipe.noise.strong"),
            note: t("pipe.noise.strongNote"),
          },
        ]}
      />
    </Option>
  );
}

/** This pipeline's own Voice settings: the defaults below, or on, or off. */
function PipelineVoiceChoice({ pipelineId }: { pipelineId: string }) {
  const report = useErrorHandler();
  const [audio, setAudio] = useState<AudioOptions | null>(null);
  const [saving, setSaving] = useState(false);
  useEffect(() => {
    audioBridge.audioOptions().then(setAudio, () => setAudio(null));
  }, []);
  if (!audio || audio.pipelines === undefined) return null;
  const own = audio.pipelines[pipelineId] ?? {};
  const value = (v: boolean | null | undefined) =>
    v === true ? "on" : v === false ? "off" : "default";
  const parse = (v: string) => (v === "on" ? true : v === "off" ? false : null);
  const save = async (echo: boolean | null, onlyMe: boolean | null) => {
    setSaving(true);
    try {
      setAudio(await audioBridge.setPipelineVoice(pipelineId, echo, onlyMe));
    } catch (err) {
      report(err, { retry: () => void save(echo, onlyMe) });
    } finally {
      setSaving(false);
    }
  };
  const echo = own.echo_cancel ?? null;
  const onlyMe = own.only_my_voice ?? null;
  const row = (
    label: string,
    current: boolean | null,
    fallback: boolean,
    onChange: (v: boolean | null) => void,
  ) => (
    <Option label={label} standing={LIVE}>
      <select
        className="pipe-select"
        aria-label={label}
        value={value(current)}
        disabled={saving}
        onChange={(e) => onChange(parse(e.target.value))}
      >
        <option value="default">
          {t("pipe.noise.sameAsDefault", {
            value: fallback ? t("pipe.on") : t("pipe.off"),
          })}
        </option>
        <option value="on">{t("pipe.on")}</option>
        <option value="off">{t("pipe.off")}</option>
      </select>
    </Option>
  );
  return (
    <>
      <p className="pipe-option-label">{t("pipe.noise.thisPipeline")}</p>
      {row(
        t("echo.title"),
        echo,
        audio.echo_cancel,
        (v) => void save(v, onlyMe),
      )}
      {row(
        t("voice.title"),
        onlyMe,
        Boolean(audio.only_my_voice && audio.voice_trained),
        (v) => void save(echo, v),
      )}
    </>
  );
}

function NoisePanel({
  status,
  pipelineId,
}: {
  status: StatusSnapshot | null;
  pipelineId: string;
}) {
  const endpoint = cap(status, "speech.endpoint=");
  const onOff = (value: string | null) =>
    value === null
      ? t("home.status.notReported")
      : value.startsWith("disabled") || value.startsWith("unavailable")
        ? t("pipe.off")
        : t("pipe.on");
  return (
    <>
      <PipelineVoiceChoice pipelineId={pipelineId} />
      <p className="source">{t("pipe.noise.shared")}</p>
      <EchoCancelCard />
      <VoiceCard />
      <NoiseOption />
      <Option
        label={t("pipe.noise.endpoint")}
        hint={t("pipe.noise.endpointHint")}
        value={onOff(endpoint)}
        standing={REPORTED}
      />
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
  onChoose,
}: {
  model: ModelInfo;
  chosen: boolean;
  busy: boolean;
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
          {t("pipe.model.loaded")}
        </span>
      ) : loading ? (
        <span className="chip pipe-model-loading">{t("models.loading")}</span>
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
  const snap = status ?? { capabilities: [], active_session: null };
  const reported = resolveModel(snap as never, "auto");
  const device = activeDevice(status);
  const engine = engineFacts(status);
  const deviceLabel = device.running
    ? t(`strip.device.${device.running}`)
    : t("home.status.notReported");
  const notReported = t("pipe.notReportedYet");
  const usable = usableModels(models ?? []);
  const loaded = usable.find((m) => m.state === "active") ?? null;
  // This pipeline's own model; one that chose none uses the loaded model.
  const chosen = usable.find((m) => m.id === pipeline.model_id) ?? loaded;
  const chosenIsLoaded = chosen !== null && chosen.id === loaded?.id;
  const selection = cap(status, "model.selection=");
  const loading =
    selection === "loading" || usable.some((m) => m.state === "loading");
  const failed = selection?.startsWith("failed:")
    ? selection.slice("failed:".length)
    : null;
  const chosenBits = chosen ? quantBits(chosen.quant) : null;
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
      <div className="pipe-model">
        <div className="pipe-model-head">
          <span className="stage-icon" aria-hidden="true">
            <Icon name="cpu" />
          </span>
          <div>
            <strong>
              {chosen ? modelName(chosen) : modelChip(reported).label}
            </strong>
            <span>
              {chosen
                ? chosenIsLoaded
                  ? t("pipe.model.inUseBy")
                  : t("pipe.model.loadsOnStart")
                : reported.resolved.replace(/^Auto → /, "")}
            </span>
          </div>
        </div>
        <dl className="pipe-facts">
          <div>
            <dt>{t("pipe.model.device")}</dt>
            <dd>
              {chosenIsLoaded
                ? deviceLabel
                : t(`pipe.model.compute.${compute ?? "auto"}`)}
            </dd>
          </div>
          <div>
            <dt>{t("pipe.model.languages")}</dt>
            <dd>
              {chosen?.languages.length
                ? languageSummary(chosen)
                : engine.languages
                  ? reportedLanguages(engine.languages)
                  : notReported}
            </dd>
          </div>
          <div>
            <dt>{t("pipe.model.streaming")}</dt>
            <dd>
              {chosen
                ? chosen.streaming
                  ? t("home.fact.liveText")
                  : t("home.fact.afterStop")
                : notReported}
            </dd>
          </div>
          <div>
            <dt>{t("pipe.model.quant")}</dt>
            <dd>
              {chosen?.quant
                ? `${chosen.quant}${chosenBits ? ` · ${chosenBits}` : ""}`
                : notReported}
            </dd>
          </div>
          <div>
            <dt>{t("pipe.model.size")}</dt>
            <dd>{chosen ? formatBytes(chosen.size_bytes) : notReported}</dd>
          </div>
          <div>
            <dt>{t("pipe.model.runtime")}</dt>
            <dd>{engine.runtime ?? notReported}</dd>
          </div>
        </dl>
      </div>
      <Option
        label={t("pipe.model.choose")}
        hint={t("pipe.model.chooseHint", { pipeline: pipelineName })}
        standing={LIVE}
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
          onClick={() => navigate("models")}
        >
          <Icon name="download" />
          {t("pipe.model.more")}
          <Icon name="arrowRight" />
        </button>
      </Option>
      <Option
        label={t("pipe.model.accel")}
        hint={t("pipe.model.accelHint")}
        standing={compute ? LIVE : planned("T2.15")}
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
        {chosenIsLoaded && device.running ? (
          <p className="pipe-option-hint pipe-model-now">
            {t("pipe.model.runningOn", { device: deviceLabel })}
          </p>
        ) : null}
      </Option>
      <div className="pipe-sub">
        <h5>{t("pipe.lang.title")}</h5>
        <p className="pipe-option-hint">{t("pipe.lang.hint")}</p>
        <RecognitionSettingsCard />
      </div>
      <DictationLanguage status={status} />
      <LanguageShortcuts
        status={status}
        pipelineId={pipeline.id}
        pipelineName={pipelineName}
      />
    </>
  );
}

/** Shortcuts that start this pipeline in a set spoken and written language. */
/** The language dictation writes in, for every pipeline (the daemon keeps
 * it): Automatic or one of the user's languages. The tray's Language menu
 * chooses from the same list. Hidden on a daemon without it. */
function DictationLanguage({ status }: { status: StatusSnapshot | null }) {
  const report = useErrorHandler();
  const options = recognitionOptions(status);
  const [text, setText] = useState<TextOptions | null>(null);
  const [adding, setAdding] = useState("");
  useEffect(() => {
    let alive = true;
    audioBridge.textOptions().then(
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
    <div className="pipe-sub">
      <h5>
        {t("pipe.lang.current")}
        <Badge standing={LIVE} />
      </h5>
      <p className="pipe-option-hint">{t("pipe.lang.currentHint")}</p>
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
                    void save(audioBridge.setDictationLanguage(tag ?? "auto"))
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
    </div>
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
  const { info, managed, capture, remove } = useContext(ShortcutsContext);
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
    <div className="pipe-sub">
      <h5>
        {t("pipe.lang.shortcuts")}
        <Badge standing={LIVE} />
      </h5>
      <p className="pipe-option-hint">{t("pipe.lang.shortcutsHint")}</p>
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
      {!nothingChosen ? (
        <details className="pipe-manual">
          <summary>{t("pipe.shortcut.manual")}</summary>
          <CopyCommand
            command={shortcutCommand(
              pipelineId,
              spoken,
              output,
              info?.manual_command,
            )}
            label={t("pipe.shortcut.copy")}
          />
        </details>
      ) : null}
      {options.languages.length <= 1 ? (
        <p className="pipe-option-hint">{t("pipe.lang.oneLanguage")}</p>
      ) : null}
    </div>
  );
}

/** Paste (then put the clipboard back) or type. */
function DeliveryMethodOption() {
  const report = useErrorHandler();
  const [method, setMethod] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  useEffect(() => {
    audioBridge.textOptions().then(
      (o) => setMethod(o.delivery_method || null),
      () => setMethod(null),
    );
  }, []);
  if (!method) return null;
  const pick = async (next: string) => {
    setSaving(true);
    try {
      setMethod(
        (await audioBridge.setDeliveryMethod(next)).delivery_method ?? null,
      );
    } catch (err) {
      report(err, { retry: () => void pick(next) });
    } finally {
      setSaving(false);
    }
  };
  return (
    <>
      <Option
        label={t("pipe.deliver.method")}
        hint={
          method === "paste-keyboard"
            ? t("pipe.deliver.keyboardHint")
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
      <Option
        label={t("pipe.deliver.restore")}
        hint={
          method !== "type"
            ? t("pipe.deliver.restoreLive")
            : t("pipe.deliver.restoreType")
        }
        value={method !== "type" ? t("pipe.on") : t("pipe.notNeeded")}
        standing={LIVE}
      />
    </>
  );
}

/** "new line" and "new paragraph" said while dictating become line breaks. */
function SpokenBreaksOption() {
  const report = useErrorHandler();
  const [on, setOn] = useState<boolean | null>(null);
  const [saving, setSaving] = useState(false);
  useEffect(() => {
    audioBridge.textOptions().then(
      (o) => setOn(o.spoken_breaks ?? null),
      () => setOn(null),
    );
  }, []);
  if (on === null) return null;
  const toggle = async () => {
    setSaving(true);
    try {
      setOn((await audioBridge.setSpokenBreaks(!on)).spoken_breaks ?? !on);
    } catch (err) {
      report(err, { retry: () => void toggle() });
    } finally {
      setSaving(false);
    }
  };
  return (
    <Option
      label={t("pipe.refine.breaks")}
      hint={t("pipe.refine.breaksHint")}
      standing={LIVE}
    >
      <button
        type="button"
        role="switch"
        className="switch"
        aria-checked={on}
        aria-label={t("pipe.refine.breaks")}
        disabled={saving}
        onClick={() => void toggle()}
      >
        <span className="switch-thumb" />
      </button>
    </Option>
  );
}

/** Free the clean-up model's memory when it has not been used for a while. */
function CleanupIdleOption() {
  const report = useErrorHandler();
  const [minutes, setMinutes] = useState<number | null>(null);
  const [saving, setSaving] = useState(false);
  useEffect(() => {
    audioBridge.textOptions().then(
      (o) => setMinutes(o.cleanup_idle_minutes ?? null),
      () => setMinutes(null),
    );
  }, []);
  if (minutes === null) return null;
  const choose = async (value: number) => {
    setSaving(true);
    try {
      setMinutes(
        (await audioBridge.setCleanupIdle(value)).cleanup_idle_minutes ?? value,
      );
    } catch (err) {
      report(err, { retry: () => void choose(value) });
    } finally {
      setSaving(false);
    }
  };
  return (
    <Option
      label={t("pipe.refine.idle")}
      hint={t("pipe.refine.idleHint")}
      standing={LIVE}
    >
      <select
        className="pipe-select"
        aria-label={t("pipe.refine.idle")}
        value={minutes}
        disabled={saving}
        onChange={(e) => void choose(Number(e.target.value))}
      >
        {[0, 5, 15, 30, 60].map((m) => (
          <option key={m} value={m}>
            {m === 0
              ? t("pipe.refine.idleNever")
              : t("pipe.refine.idleAfter", { minutes: m })}
          </option>
        ))}
      </select>
    </Option>
  );
}

/** Numbers as digits: the Clean pipeline's final text. */
function NumbersOption({ clean }: { clean: boolean }) {
  const report = useErrorHandler();
  const [on, setOn] = useState<boolean | null>(null);
  const [saving, setSaving] = useState(false);
  useEffect(() => {
    audioBridge.textOptions().then(
      (o) => setOn(o.numbers_as_digits),
      () => setOn(null),
    );
  }, []);
  if (on === null) return null;
  const toggle = async () => {
    setSaving(true);
    try {
      setOn((await audioBridge.setNumbersAsDigits(!on)).numbers_as_digits);
    } catch (err) {
      report(err, { retry: () => void toggle() });
    } finally {
      setSaving(false);
    }
  };
  return (
    <Option
      label={t("pipe.refine.numbers")}
      hint={clean ? t("pipe.refine.numbersLive") : t("pipe.refine.numbersRaw")}
      standing={LIVE}
    >
      <button
        type="button"
        role="switch"
        className="switch"
        aria-checked={on}
        aria-label={t("pipe.refine.numbers")}
        disabled={saving}
        onClick={() => void toggle()}
      >
        <span className="switch-thumb" />
      </button>
    </Option>
  );
}

/** The language model that cleans up each dictation in the Clean pipeline
 * (D31), chosen in Models → Correction (D32). */
function CleanupModelOption({ clean }: { clean: boolean }) {
  const { navigate } = useNav();
  const [models, setModels] = useState<LanguageModel[] | null>(null);
  useEffect(() => {
    bridge.languageModels().then(setModels, () => setModels(null));
  }, []);
  if (models === null) return null;
  const current = models.find((m) => m.in_use_for.includes("clean_up"));
  const state = cleanUpState(current?.clean_up_state);
  const value = current
    ? state
      ? `${current.name} · ${state.label}`
      : current.name
    : t("pipe.refine.cleanupNone");
  return (
    <Option
      label={t("pipe.refine.cleanupModel")}
      hint={
        clean ? t("pipe.refine.cleanupModelHint") : t("pipe.refine.numbersRaw")
      }
      value={value}
      standing={LIVE}
    >
      <button
        type="button"
        className="link-action link-quiet"
        onClick={() => navigate("models")}
      >
        {t("pipe.refine.chooseModel")}
        <Icon name="arrowRight" />
      </button>
    </Option>
  );
}

/** The deterministic corrector's rules, in the order it applies them; the
 * last one, numbers, is the live NumbersOption. */
const REFINE_RULES = ["spacing", "punctuation", "capitals", "ending"] as const;

/** Refine (D23, D38): the deterministic rules, which always run on this
 * computer, then the optional clean-up language model. */
function RefinePanel({ pipelineId }: { pipelineId: string }) {
  const clean = pipelineId === "clean";
  const rulesValue = clean ? t("pipe.on") : t("pipe.off");
  return (
    <>
      <Option
        label={t("pipe.refine.mode")}
        hint={
          clean ? t("dictationPage.mode.clean") : t("dictationPage.mode.raw")
        }
        value={clean ? t("pipe.refine.clean") : t("pipe.refine.raw")}
        standing={REPORTED}
      />
      <div className="pipe-sub">
        <h5>{t("refine.rules.title")}</h5>
        <p className="pipe-option-hint">{t("refine.rules.lead")}</p>
      </div>
      {REFINE_RULES.map((rule) => (
        <Option
          key={rule}
          label={t(`refine.rule.${rule}`)}
          hint={t(`refine.rule.${rule}Hint`)}
          value={rulesValue}
          standing={REPORTED}
        />
      ))}
      <NumbersOption clean={clean} />
      {clean ? <SpokenBreaksOption /> : null}
      <Option
        label={t("refine.rules.perRule")}
        hint={t("refine.rules.perRuleHint")}
        standing={planned("T2.20")}
      />
      <Option
        label={t("pipe.refine.hints")}
        hint={t("pipe.refine.hintsHint")}
        standing={LIVE}
      />
      <div className="pipe-sub">
        <h5>{t("refine.model.title")}</h5>
        <p className="pipe-option-hint">{t("refine.model.lead")}</p>
      </div>
      <CleanupModelOption clean={clean} />
      <Option
        label={t("pipe.model.accel")}
        hint={t("refine.model.whereHint")}
        standing={planned("T2.20")}
      >
        <Choices
          current="auto"
          options={[
            { id: "auto", label: t("pipe.model.compute.auto") },
            { id: "npu", label: "NPU" },
            { id: "gpu", label: "GPU" },
            { id: "cpu", label: "CPU" },
          ]}
        />
      </Option>
      <Option
        label={t("refine.level.title")}
        hint={t("refine.level.hint")}
        standing={planned("T2.20")}
      >
        <Choices
          current="off"
          options={[
            { id: "off", label: t("pipe.off") },
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
      <Option
        label={t("pipe.refine.guard")}
        hint={t("pipe.refine.guardLive")}
        standing={LIVE}
      />
      <Option
        label={t("refine.privacy")}
        hint={t("refine.privacyHint")}
        value={t("refine.localOnly")}
        standing={LIVE}
      />
      {clean ? <CleanupIdleOption /> : null}
    </>
  );
}

function DeliverPanel({ status }: { status: StatusSnapshot | null }) {
  const snap = status ?? { capabilities: [], active_session: null };
  const delivery = resolveDelivery(snap as never, "auto");
  const inbox = cap(status, "delivery.inbox-recovery=");
  const verified = cap(status, "delivery.verified=");
  return (
    <>
      <Option
        label={t("pipe.deliver.current")}
        hint={delivery.resolved.replace(/^Auto → /, "")}
        value={deliveryChip(delivery).label}
        standing={REPORTED}
      />
      <DeliveryMethodOption />
      <Option
        label={t("pipe.deliver.inbox")}
        hint={t("pipe.deliver.inboxHint")}
        value={inbox === "enabled" ? t("pipe.on") : t("pipe.off")}
        standing={REPORTED}
      />
      <Option
        label={t("pipe.deliver.verified")}
        hint={t("pipe.deliver.verifiedHint")}
        value={
          verified && !verified.startsWith("unavailable")
            ? t("pipe.on")
            : t("pipe.notYet")
        }
        standing={REPORTED}
      />
      <Option
        label={t("pipe.deliver.space")}
        hint={t("pipe.deliver.spaceHint")}
        standing={PROPOSED}
      />
    </>
  );
}

// -------------------------------------------------------------- the screen

export function PipelinesScreen() {
  const { catalog, select } = usePipelines();
  const { status } = useDaemonStatus();
  const [prefs, setPrefs] = useState<PipelinePrefs>(loadPipelinePrefs);
  const [open, setOpen] = useState<{ pipeline: string; stage: StageId } | null>(
    null,
  );
  const [editing, setEditing] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [dragging, setDragging] = useState<string | null>(null);
  const [dropIndex, setDropIndex] = useState<number | null>(null);

  const report = useErrorHandler();
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
      capture: setCaptureTarget,
      remove: (binding) => void removeShortcut(binding),
    }),
    [shortcutsInfo, managed, removeShortcut],
  );

  const pipelines = useMemo(
    () => orderPipelines(catalog.pipelines, prefs.order),
    [catalog.pipelines, prefs.order],
  );
  const ids = pipelines.map((p) => p.id);
  const snap = status ?? { capabilities: [], active_session: null };
  const [audio, setAudio] = useState<AudioOptions | null>(null);
  useEffect(() => {
    audioBridge.audioOptions().then(setAudio, () => setAudio(null));
  }, [open]);
  const summary: Record<StageId, string> = {
    mic: micChip(resolveMic(snap as never, "auto")).label,
    noise: audio
      ? [
          audio.echo_cancel ? t("pipe.noise.chipEcho") : null,
          audio.voice_trained && audio.only_my_voice
            ? t("pipe.noise.chipVoice")
            : null,
        ]
          .filter(Boolean)
          .join(" · ") || t("pipe.noise.chipSilence")
      : t("pipe.off"),
    transcribe: modelChip(resolveModel(snap as never, "auto")).label,
    refine: "",
    deliver: deliveryChip(resolveDelivery(snap as never, "auto")).label.replace(
      / · Inbox backup$/,
      "",
    ),
  };

  const persist = (next: PipelinePrefs) => {
    savePipelinePrefs(next);
    setPrefs(next);
  };
  const displayName = (id: string, fallback: string) =>
    prefs.names[id] ?? fallback;

  const move = (id: string, index: number) =>
    persist({ ...prefs, order: moveInOrder(ids, id, index) });

  const startRename = (id: string, current: string) => {
    setEditing(id);
    setDraft(current);
  };
  const commitRename = (id: string, daemonName: string) => {
    const name = draft.trim().slice(0, MAX_NAME_LENGTH);
    const names = { ...prefs.names };
    if (!name || name === daemonName) delete names[id];
    else names[id] = name;
    persist({ ...prefs, names });
    setEditing(null);
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
            <PipelinesIoButtons />
            <button
              type="button"
              className="btn btn-default"
              disabled
              title={t("fm.status.proposedTitle")}
            >
              <Icon name="plus" />
              {t("pipelinesPage.new")}
            </button>
          </div>
        </div>

        <ol className="pipe-list" onDragLeave={() => setDropIndex(null)}>
          {pipelines.map((pipeline, index) => {
            const active = pipeline.id === catalog.selectedId;
            const name = displayName(pipeline.id, pipeline.name);
            const openStage =
              open?.pipeline === pipeline.id ? open.stage : null;
            const stageValue = (stage: StageId) =>
              stage === "refine" ? pipeline.name : summary[stage];
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
                      disabled={index === 0}
                      onClick={() => move(pipeline.id, index - 1)}
                      aria-label={t("pipe.moveUp", { name })}
                    >
                      <Icon name="chevronDown" className="icon-flip" />
                    </button>
                    <button
                      type="button"
                      className="icon-btn"
                      disabled={index === pipelines.length - 1}
                      onClick={() => move(pipeline.id, index + 1)}
                      aria-label={t("pipe.moveDown", { name })}
                    >
                      <Icon name="chevronDown" />
                    </button>
                  </span>
                  {editing === pipeline.id ? (
                    <input
                      className="pipe-name-input"
                      value={draft}
                      maxLength={MAX_NAME_LENGTH}
                      autoFocus
                      aria-label={t("pipe.rename", { name })}
                      onChange={(e) => setDraft(e.target.value)}
                      onBlur={() => commitRename(pipeline.id, pipeline.name)}
                      onKeyDown={(e) => {
                        if (e.key === "Enter")
                          commitRename(pipeline.id, pipeline.name);
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
                  {name !== pipeline.name ? (
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
                      <Icon name="chevronDown" className="pipe-stage-chevron" />
                    </button>
                  ))}
                </div>

                {openStage ? (
                  <div className="pipe-panel" role="tabpanel">
                    {openStage === "mic" ? <MicPanel status={status} /> : null}
                    {openStage === "noise" ? (
                      <NoisePanel status={status} pipelineId={pipeline.id} />
                    ) : null}
                    {openStage === "transcribe" ? (
                      <>
                        <TranscribePanel
                          status={status}
                          pipeline={pipeline}
                          pipelineName={name}
                        />
                        <VocabularyEditor />
                      </>
                    ) : null}
                    {openStage === "refine" ? (
                      <RefinePanel pipelineId={pipeline.id} />
                    ) : null}
                    {openStage === "deliver" ? (
                      <DeliverPanel status={status} />
                    ) : null}
                  </div>
                ) : null}

                {!managed ? (
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
                          shortcutsInfo?.manual_command,
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
        <p className="pipe-footnote">{t("pipe.footnote")}</p>

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
