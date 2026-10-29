// Meetings (F22): record a meeting, then read its transcript with every
// speaker kept apart. The daemon records the microphone (people in the room)
// and the computer's sound (people on a call) separately, tells voices apart
// and transcribes each turn, all on this computer.
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type FormEvent,
} from "react";
import {
  bridge,
  type Meeting,
  type MeetingDetail,
  type MeetingSlide,
  type MeetingTurn,
  type SessionEvent,
} from "../../lib/bridge";
import { Button } from "../../ui/primitives/Button";
import { useErrorHandler, useToasts } from "../../ui/primitives/Toast";
import { Icon } from "../../ui/icons";
import { hasMessage, t } from "../../i18n";
import { useNav } from "../../state/NavContext";
import { useAssistant } from "../../state/AssistantContext";
import { Markdown } from "./Assistant";
import { clock, meetingContext } from "../../lib/meetingContext";
import { ScreenLive } from "../components/ScreenLive";
import { SourceSwitches } from "../components/SourceSwitches";
import {
  AutoNotesSwitch,
  ProcessOnStopSwitch,
} from "../components/AutoNotesSwitch";
import { MeetingDevices } from "../components/MeetingDevices";

// Speaker colours: blue, green and yellow first, as light tints behind
// each speaker's bubble and as the name colour.
const COLORS = [
  "#3e7bfa",
  "#2f9e44",
  "#e0a100",
  "#8e4ec6",
  "#0f9fb3",
  "#d6409f",
  "#f08c00",
  "#7c6f5b",
];

const MODEL_LABEL: Record<string, string> = {
  "large-v3": t("meetings.modelLarge"),
  turbo: t("meetings.modelTurbo"),
};

function when(ms: number): string {
  return new Date(ms).toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function stageText(meeting: Meeting): string {
  const pct = Math.round(meeting.progress / 10);
  switch (meeting.stage) {
    case "load":
      return t("meetings.stage.load");
    case "speakers":
      return t("meetings.stage.speakers");
    case "words":
      return t("meetings.stage.words", { pct });
    case "finish":
      return t("meetings.stage.finish");
    case "import":
      return t("meetings.stage.import");
    default:
      return t("meetings.stage.start");
  }
}

/** While a meeting is gone over (transcribed, its pictures described, its
 * notes written): what is happening, with Pause, Resume and Stop. */
function WorkControls({ meeting }: { meeting: Meeting }) {
  const [error, setError] = useState<string | null>(null);
  const stage = meeting.stage ?? "";
  const working =
    meeting.state === "processing" ||
    (meeting.state === "ready" && /^(slides|notes):|^paused$/.test(stage));
  if (!working) return null;
  const paused = stage === "paused" || Boolean(meeting.paused);
  const [, kind, count] = /^(slides|notes):(.*)$/.exec(stage) ?? [];
  const act = (action: "pause" | "resume" | "stop") => {
    setError(null);
    bridge
      .meetingControl(meeting.id, action)
      .catch((e: unknown) =>
        setError((e as { code?: string })?.code ?? "unavailable"),
      );
  };
  return (
    <div className="meeting-work">
      <span className="source" role="status">
        {paused
          ? t("meetings.work.paused")
          : kind
            ? t(`meetings.work.${kind}`, { count: count ?? "" })
            : t("meetings.work.transcribing")}
      </span>
      <button
        type="button"
        className="btn btn-default"
        onClick={() => act(paused ? "resume" : "pause")}
      >
        <Icon name={paused ? "play" : "pause"} />
        {paused ? t("assistant.watch.resume") : t("assistant.watch.pause")}
      </button>
      <button
        type="button"
        className="btn btn-default"
        title={t("meetings.work.stopHint")}
        onClick={() => act("stop")}
      >
        <Icon name="stop" />
        {t("assistant.watch.cancel")}
      </button>
      {error ? (
        <span className="meeting-error">
          {t("meetings.error.other", { code: error })}
        </span>
      ) : null}
    </div>
  );
}

function failureText(state: string): string {
  const code = state.startsWith("failed:") ? state.slice(7) : state;
  switch (code) {
    case "meeting_stopped":
      return t("meetings.error.stopped");
    case "interrupted":
      return t("meetings.error.interrupted");
    case "meeting_worker_missing":
      return t("meetings.error.worker");
    case "meeting_model_missing":
      return t("meetings.error.model");
    case "meeting_import_no_audio":
      return t("meetings.error.importNoAudio");
    case "meeting_import_timeout":
      return t("meetings.error.importTimeout");
    default:
      return t("meetings.error.other", { code });
  }
}

function StateChip({ meeting }: { meeting: Meeting }) {
  if (meeting.state === "recording")
    return (
      <span className="chip chip-err meeting-live">
        {t("meetings.state.recording")}
      </span>
    );
  if (meeting.state === "processing")
    return <span className="chip">{Math.round(meeting.progress / 10)}%</span>;
  if (meeting.state === "ready")
    return (
      <span className="chip chip-ok">
        {meeting.speakers === 1
          ? t("meetings.state.speaker")
          : t("meetings.state.speakers", { count: meeting.speakers })}
      </span>
    );
  return <span className="chip chip-warn">{t("meetings.state.failed")}</span>;
}

export function MeetingsScreen() {
  const onError = useErrorHandler();
  const { push } = useToasts();
  const [meetings, setMeetings] = useState<Meeting[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [detail, setDetail] = useState<MeetingDetail | null>(null);
  const [title, setTitle] = useState("");
  const [busy, setBusy] = useState(false);
  // "New meeting" shows the recorder in place of the open meeting.
  const [composing, setComposing] = useState(false);
  const [query, setQuery] = useState("");
  const [now, setNow] = useState(Date.now());
  const selectedRef = useRef<string | null>(null);
  selectedRef.current = selected;

  const recording = meetings.find((m) => m.state === "recording") ?? null;

  const refresh = useCallback(async () => {
    try {
      const list = await bridge.meetings();
      setMeetings(list);
      setSelected((current) => current ?? list[0]?.id ?? null);
    } catch (err) {
      onError(err);
    }
  }, [onError]);

  const loadDetail = useCallback(async (id: string | null) => {
    if (!id) {
      setDetail(null);
      return;
    }
    try {
      setDetail(await bridge.meeting(id));
    } catch {
      setDetail(null);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    void loadDetail(selected);
  }, [selected, loadDetail]);

  // Progress and state arrive as events; the transcript is read once ready.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void bridge
      .onSessionEvent((ev: SessionEvent) => {
        if (ev.event !== "meeting_changed" || !ev.id) return;
        const id = ev.id;
        if (ev.state === "deleted") {
          setMeetings((list) => list.filter((m) => m.id !== id));
          return;
        }
        setMeetings((list) => {
          if (!list.some((m) => m.id === id)) {
            void refresh();
            return list;
          }
          return list.map((m) =>
            m.id === id
              ? {
                  ...m,
                  state: ev.state ?? m.state,
                  progress: ev.progress ?? m.progress,
                  stage: ev.stage ?? undefined,
                }
              : m,
          );
        });
        // A new live turn arrives as a change while recording, or while the
        // last words are written after it.
        if (
          id === selectedRef.current &&
          (ev.state !== "processing" || ev.stage === "finish")
        ) {
          void refresh();
          void loadDetail(id);
        }
      })
      .then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [refresh, loadDetail]);

  // The recording clock.
  useEffect(() => {
    if (!recording) return;
    const timer = setInterval(() => setNow(Date.now()), 500);
    return () => clearInterval(timer);
  }, [recording]);

  const start = async (event: FormEvent) => {
    event.preventDefault();
    setBusy(true);
    try {
      const meeting = await bridge.meetingStart(title.trim() || null);
      setTitle("");
      setMeetings((list) => [
        meeting,
        ...list.filter((m) => m.id !== meeting.id),
      ]);
      setSelected(meeting.id);
      setComposing(false);
    } catch (err) {
      onError(err);
    } finally {
      setBusy(false);
    }
  };

  const stop = async () => {
    setBusy(true);
    try {
      await bridge.meetingStop();
      await refresh();
    } catch (err) {
      onError(err);
    } finally {
      setBusy(false);
    }
  };

  const importFile = async (path: string) => {
    const trimmed = path.trim();
    if (!trimmed) return;
    setBusy(true);
    try {
      const meeting = await bridge.meetingImport(trimmed);
      setMeetings((list) => [
        meeting,
        ...list.filter((m) => m.id !== meeting.id),
      ]);
      setSelected(meeting.id);
      setComposing(false);
    } catch (err) {
      onError(err);
    } finally {
      setBusy(false);
    }
  };

  const chooseRecording = async () => {
    try {
      const path = await bridge.pickRecording(t("meetings.importChooseTitle"));
      if (path) await importFile(path);
    } catch (err) {
      onError(err);
    }
  };

  // A file dropped on the window is imported (while not recording).
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void bridge
      .onFileDrop((paths) => {
        if (paths[0]) void importFile(paths[0]);
      })
      .then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      });
    return () => {
      cancelled = true;
      unlisten?.();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const shown = meetings.find((m) => m.id === selected) ?? null;

  const needle = query.trim().toLowerCase();
  const listed = needle
    ? meetings.filter((m) => m.title.toLowerCase().includes(needle))
    : meetings;
  const showRecorder = composing || recording !== null || !shown;

  return (
    <div className="settings-page meetings-shell">
      <aside className="settings-side" aria-label={t("meetings.listLabel")}>
        <button
          type="button"
          className="assistant-new"
          aria-pressed={composing}
          onClick={() => setComposing(true)}
        >
          <Icon name="plus" />
          {t("meetings.new")}
        </button>
        <label className="settings-search">
          <Icon name="search" />
          <input
            type="search"
            value={query}
            placeholder={t("meetings.search")}
            aria-label={t("meetings.search")}
            onChange={(e) => setQuery(e.target.value)}
          />
        </label>
        <nav className="meeting-list" aria-label={t("meetings.listLabel")}>
          {meetings.length === 0 ? (
            <p className="source meeting-empty">{t("meetings.empty")}</p>
          ) : null}
          {needle && listed.length === 0 ? (
            <p className="source meeting-empty">{t("meetings.noneFound")}</p>
          ) : null}
          {listed.map((m) => {
            const current = !composing && m.id === selected;
            return (
              <button
                key={m.id}
                type="button"
                className={`meeting-item${current ? " is-selected" : ""}`}
                aria-current={current ? "true" : undefined}
                onClick={() => {
                  setComposing(false);
                  setSelected(m.id);
                }}
              >
                <span className="meeting-item-title">{m.title}</span>
                <span className="meeting-item-meta">
                  {when(m.started_unix_ms)}
                  {m.state !== "recording" && m.duration_ms > 0
                    ? ` · ${clock(m.duration_ms)}`
                    : ""}
                  {m.watch ? ` · ${t("meetings.screenRecording")}` : ""}
                </span>
                <StateChip meeting={m} />
              </button>
            );
          })}
        </nav>
      </aside>
      <div className="settings-main">
        <div className="meetings-page">
          <div className="page-head">
            <h2 className="page-title">{t("nav.meetings")}</h2>
            <p className="page-lead">{t("meetings.lead")}</p>
          </div>

          {showRecorder ? (
            <section className="card meeting-recorder" aria-live="polite">
              {recording ? (
                <div className="meeting-recording">
                  <span className="meeting-dot" aria-hidden="true" />
                  <div className="meeting-recording-text">
                    <strong>{recording.title}</strong>
                    <span className="source">
                      {t("meetings.recordingNote")}
                    </span>
                  </div>
                  <span className="meeting-clock">
                    {clock(now - recording.started_unix_ms)}
                  </span>
                  <Button
                    className="btn-live meeting-stop"
                    onClick={stop}
                    disabled={busy}
                  >
                    <Icon name="stop" />
                    {t("meetings.stop")}
                  </Button>
                </div>
              ) : (
                <form className="meeting-start" onSubmit={start}>
                  <input
                    className="meeting-title-input"
                    value={title}
                    maxLength={120}
                    placeholder={t("meetings.titlePlaceholder")}
                    aria-label={t("meetings.titleLabel")}
                    title={t("meetings.hint")}
                    onChange={(e) => setTitle(e.target.value)}
                  />
                  <Button
                    variant="primary"
                    type="submit"
                    className="meeting-record"
                    disabled={busy}
                  >
                    <span className="meeting-record-dot" aria-hidden="true" />
                    {t("meetings.start")}
                  </Button>
                </form>
              )}
              <MeetingDevices
                recording={Boolean(recording)}
                extra={
                  recording ? null : (
                    <>
                      <ProcessOnStopSwitch />
                      <AutoNotesSwitch />
                    </>
                  )
                }
                trailing={
                  recording ? null : (
                    <button
                      type="button"
                      className="meeting-import-link"
                      title={t("meetings.importHint")}
                      disabled={busy}
                      onClick={() => void chooseRecording()}
                    >
                      <Icon name="folder" />
                      {t("meetings.importChoose")}
                    </button>
                  )
                }
              />
            </section>
          ) : null}

          <div className="meeting-detail">
            {shown && !composing ? (
              <MeetingView
                key={shown.id}
                meeting={shown}
                detail={
                  detail && detail.meeting.id === shown.id ? detail : null
                }
                onChanged={() => {
                  void refresh();
                  void loadDetail(shown.id);
                }}
                onDeleted={() => {
                  setSelected(null);
                  setDetail(null);
                  void refresh();
                  push(t("meetings.deleted"));
                }}
              />
            ) : null}
          </div>
        </div>
      </div>
    </div>
  );
}

function MeetingView({
  meeting,
  detail,
  onChanged,
  onDeleted,
}: {
  meeting: Meeting;
  detail: MeetingDetail | null;
  onChanged: () => void;
  onDeleted: () => void;
}) {
  const onError = useErrorHandler();
  const { push } = useToasts();
  const { navigate } = useNav();
  const { askAboutMeeting, busy: assistantBusy } = useAssistant();
  const [showRedo, setShowRedo] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [tab, setTab] = useState<"transcript" | "notes" | "screens">(
    "transcript",
  );

  // Words still being said (while recording) are not part of the transcript.
  const turns = (detail?.turns ?? []).filter((turn) => !turn.partial);
  const names = detail?.names ?? {};
  const nameOf = (speaker: string) => names[speaker] ?? speaker;
  // One entry per person: voices with the same name are one person.
  const people = useMemo(() => {
    const order: Array<{ name: string; labels: string[] }> = [];
    for (const turn of turns) {
      const name = names[turn.speaker] ?? turn.speaker;
      const person = order.find((p) => p.name === name);
      if (!person) order.push({ name, labels: [turn.speaker] });
      else if (!person.labels.includes(turn.speaker))
        person.labels.push(turn.speaker);
    }
    return order;
  }, [turns, names]);
  const colorOf = (speaker: string) =>
    COLORS[
      Math.max(
        0,
        people.findIndex((p) => p.name === nameOf(speaker)),
      ) % COLORS.length
    ];

  const plainText = () =>
    [
      meeting.title,
      "",
      ...groupTurns(turns, nameOf).map(
        (group) =>
          `${nameOf(group.speaker)} [${clock(group.start_ms)}]\n${group.texts.join("\n")}`,
      ),
    ].join("\n\n");

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(plainText());
      push(t("meetings.copied"));
    } catch (err) {
      onError(err);
    }
  };

  const askAssistant = () => {
    if (!detail) return;
    askAboutMeeting({
      id: meeting.id,
      title: meeting.title,
      context: meetingContext(detail, nameOf),
    });
    navigate("assistant");
  };

  const remove = async () => {
    try {
      await bridge.meetingDelete(meeting.id);
      onDeleted();
    } catch (err) {
      onError(err);
    }
  };

  return (
    <article className="card meeting-view">
      <header className="meeting-view-head">
        <div>
          <h3 className="meeting-view-title">{meeting.title}</h3>
          <p className="source">
            {when(meeting.started_unix_ms)}
            {meeting.duration_ms > 0 && meeting.state !== "recording"
              ? ` · ${clock(meeting.duration_ms)}`
              : ""}
            {meeting.model
              ? ` · ${MODEL_LABEL[meeting.model] ?? meeting.model}`
              : ""}
          </p>
        </div>
        {meeting.state === "ready" || meeting.state.startsWith("failed") ? (
          <div className="meeting-actions">
            {meeting.state === "ready" ? (
              <>
                <Button onClick={copy}>
                  <Icon name="copy" />
                  {t("meetings.copy")}
                </Button>
                <Button
                  onClick={askAssistant}
                  disabled={!detail || assistantBusy}
                >
                  <Icon name="spark" />
                  {t("meetings.askAssistant")}
                </Button>
                {detail?.recording ? (
                  <Button
                    onClick={() =>
                      bridge
                        .openMeetingFolder(detail.folder, "recording.wav")
                        .catch(onError)
                    }
                  >
                    <Icon name="play" />
                    {t("meetings.play")}
                  </Button>
                ) : null}
                {detail && detail.turns.length > 0 ? (
                  <Button
                    title={t("meetings.subtitlesHint")}
                    onClick={() =>
                      bridge
                        .openMeetingFolder(detail.folder, "transcript.srt")
                        .catch(onError)
                    }
                  >
                    <Icon name="copy" />
                    {t("meetings.subtitles")}
                  </Button>
                ) : null}
                {detail ? (
                  <Button
                    onClick={() =>
                      bridge.openMeetingFolder(detail.folder).catch(onError)
                    }
                  >
                    <Icon name="folder" />
                    {t("meetings.openFolder")}
                  </Button>
                ) : null}
              </>
            ) : null}
            <Button
              onClick={() => setShowRedo((v) => !v)}
              aria-expanded={showRedo}
            >
              <Icon name="refresh" />
              {t("meetings.redo")}
            </Button>
            {confirmDelete ? (
              <>
                <Button className="btn-danger" onClick={remove}>
                  {t("meetings.deleteConfirm")}
                </Button>
                <Button variant="ghost" onClick={() => setConfirmDelete(false)}>
                  {t("meetings.cancel")}
                </Button>
              </>
            ) : (
              <Button
                variant="ghost"
                onClick={() => setConfirmDelete(true)}
                aria-label={t("meetings.delete")}
              >
                <Icon name="trash" />
              </Button>
            )}
          </div>
        ) : null}
      </header>

      {showRedo && detail ? (
        <RedoForm
          meeting={meeting}
          models={detail.models}
          onDone={() => {
            setShowRedo(false);
            onChanged();
          }}
        />
      ) : null}

      {meeting.state === "recording" ? (
        <p className="meeting-note">{t("meetings.recordingDetail")}</p>
      ) : null}
      {meeting.state === "recording" ? (
        <SourceSwitches meetingId={meeting.id} />
      ) : null}
      {meeting.state === "recording" &&
      (meeting.watch ||
        detail?.screen_sources?.length ||
        detail?.screen_error) ? (
        <ScreenLive meetingId={meeting.id} />
      ) : null}

      {(meeting.state === "recording" || meeting.state === "processing") &&
      turns.length > 0 ? (
        <section
          className="meeting-live-transcript"
          aria-live="polite"
          aria-label={t("meetings.liveLabel")}
        >
          <p className="source">{t("meetings.liveNote")}</p>
          <TurnList
            turns={turns}
            nameOf={nameOf}
            colorOf={colorOf}
            follow={meeting.state === "recording"}
          />
        </section>
      ) : null}

      {meeting.state === "processing" ? (
        <div
          className="meeting-progress"
          role="progressbar"
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={Math.round(meeting.progress / 10)}
        >
          <div className="meeting-progress-bar">
            <span style={{ width: `${Math.max(2, meeting.progress / 10)}%` }} />
          </div>
          <p className="source">{stageText(meeting)}</p>
        </div>
      ) : null}
      <WorkControls meeting={meeting} />

      {meeting.state.startsWith("failed") ? (
        <p className="meeting-note meeting-error">
          {failureText(meeting.state)}
        </p>
      ) : null}

      {meeting.state === "ready" && meeting.raw ? (
        <div className="meeting-raw" role="status">
          <p className="meeting-note">{t("meetings.raw")}</p>
          <Button
            onClick={() =>
              bridge
                .meetingProcess(meeting.id, null, null, null)
                .then(onChanged, onError)
            }
          >
            <Icon name="refresh" />
            {t("meetings.goOver")}
          </Button>
        </div>
      ) : null}

      {meeting.state === "ready" && detail ? (
        <div className="meeting-tabs" role="tablist">
          <button
            type="button"
            role="tab"
            aria-selected={tab === "transcript"}
            onClick={() => setTab("transcript")}
          >
            {t("meetings.tab.transcript")}
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={tab === "notes"}
            onClick={() => setTab("notes")}
          >
            {t("meetings.tab.notes")}
            {detail.notes && Object.keys(detail.notes).length ? (
              <span className="meeting-tab-dot" aria-hidden="true" />
            ) : null}
          </button>
          {detail.slides?.length ? (
            <button
              type="button"
              role="tab"
              aria-selected={tab === "screens"}
              onClick={() => setTab("screens")}
            >
              {t("meetings.tab.screens", { count: detail.slides.length })}
            </button>
          ) : null}
        </div>
      ) : null}

      {meeting.state === "ready" && meeting.stage?.startsWith("slides:") ? (
        <p className="meeting-note" role="status">
          {t("meetings.screen.describing", {
            part: meeting.stage.slice("slides:".length).split("/")[0],
            parts: meeting.stage.split("/")[1] ?? "",
          })}
        </p>
      ) : null}
      {detail?.screen_error ? (
        <p className="meeting-note meeting-error">
          {hasMessage(`meetings.screen.error.${detail.screen_error}`)
            ? t(`meetings.screen.error.${detail.screen_error}`)
            : t("meetings.screen.error.other", { code: detail.screen_error })}
        </p>
      ) : null}

      {meeting.state === "ready" && detail && tab === "screens" ? (
        <ScreensPanel meetingId={meeting.id} slides={detail.slides ?? []} />
      ) : null}

      {meeting.state === "ready" && detail && tab === "notes" ? (
        <NotesPanel meeting={meeting} detail={detail} onSaved={onChanged} />
      ) : null}

      {meeting.state === "ready" && detail && tab === "transcript" ? (
        <>
          <div className="meeting-speakers">
            {people.map((person) => (
              <SpeakerChip
                key={person.name}
                meetingId={meeting.id}
                speakers={person.labels}
                name={person.name}
                color={colorOf(person.labels[0])}
                onRenamed={onChanged}
              />
            ))}
          </div>
          <p className="source meeting-sources">
            {t("meetings.sourcesLegend")}
          </p>
          {turns.length === 0 ? (
            <p className="meeting-note">{t("meetings.noSpeech")}</p>
          ) : (
            <TurnList turns={turns} nameOf={nameOf} colorOf={colorOf} />
          )}
        </>
      ) : null}
    </article>
  );
}

/** What was shown on screen: each kept picture, when, and what the model
 * saw on it. */
function ScreensPanel({
  meetingId,
  slides,
}: {
  meetingId: string;
  slides: MeetingSlide[];
}) {
  return (
    <ol className="meeting-screens">
      {slides.map((slide) => (
        <li key={slide.file} className="meeting-screen">
          <SlidePicture meetingId={meetingId} file={slide.file} />
          <div>
            <span className="source">{clock(slide.at_ms)}</span>
            {slide.description ? (
              <p dir="auto">{slide.description}</p>
            ) : (
              <p className="source">{t("meetings.screen.notDescribed")}</p>
            )}
          </div>
        </li>
      ))}
    </ol>
  );
}

function SlidePicture({
  meetingId,
  file,
}: {
  meetingId: string;
  file: string;
}) {
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    bridge.meetingSlide(meetingId, file).then(
      (png) => alive && png && setUrl(`data:image/png;base64,${png}`),
      () => {},
    );
    return () => {
      alive = false;
    };
  }, [meetingId, file]);
  return url ? (
    <img
      src={url}
      alt={t("meetings.screen.alt", { time: file })}
      loading="lazy"
    />
  ) : (
    <span className="meeting-screen-placeholder" aria-hidden="true" />
  );
}

/** The transcript, a speaker's consecutive turns together. With `follow`,
 * the newest turn is kept in view as it arrives. */
function TurnList({
  turns,
  nameOf,
  colorOf,
  follow = false,
}: {
  turns: MeetingTurn[];
  nameOf: (speaker: string) => string;
  colorOf: (speaker: string) => string;
  follow?: boolean;
}) {
  const end = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (follow) end.current?.scrollIntoView?.({ block: "nearest" });
  }, [follow, turns.length]);
  return (
    <>
      <ol className="meeting-transcript">
        {groupTurns(turns, nameOf).map((group, i) => (
          <li
            key={i}
            className={`meeting-turn meeting-turn-${group.source === "call" ? "call" : "room"}`}
            style={{ "--speaker": colorOf(group.speaker) } as CSSProperties}
          >
            <div className="meeting-turn-head">
              <span
                className="meeting-turn-name"
                style={{ color: colorOf(group.speaker) }}
              >
                {nameOf(group.speaker)}
              </span>
              <span className="source">{clock(group.start_ms)}</span>
              {group.language && group.language !== "en" ? (
                <span className="chip meeting-lang">
                  {group.language.toUpperCase()}
                </span>
              ) : null}
            </div>
            {group.texts.map((text, j) => (
              <p key={j} className="meeting-turn-text" dir="auto">
                {text}
              </p>
            ))}
          </li>
        ))}
      </ol>
      <div ref={end} />
    </>
  );
}

const NOTE_ACTIONS: Array<{ kind: "notes" | "email"; label: string }> = [
  { kind: "notes", label: "meetings.notes.detailed" },
  { kind: "email", label: "meetings.notes.email" },
];

function NotesPanel({
  meeting,
  detail,
  onSaved,
}: {
  meeting: Meeting;
  detail: MeetingDetail;
  onSaved: () => void;
}) {
  const onError = useErrorHandler();
  const { push } = useToasts();
  const [running, setRunning] = useState<{
    turnId: string;
    kind: string;
    text: string;
  } | null>(null);
  const [instruction, setInstruction] = useState("");
  const [failure, setFailure] = useState<string | null>(null);
  const runningRef = useRef(running);
  runningRef.current = running;

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void bridge
      .onSessionEvent((ev: SessionEvent) => {
        const current = runningRef.current;
        if (!current || ev.turn_id !== current.turnId) return;
        if (ev.event === "assistant_delta" && typeof ev.text === "string") {
          const piece = ev.text;
          setRunning((r) =>
            r && r.turnId === current.turnId
              ? { ...r, text: r.text + piece }
              : r,
          );
        } else if (ev.event === "assistant_done") {
          const outcome = ev.outcome ?? "failed:unknown";
          setRunning(null);
          if (outcome.startsWith("failed:")) setFailure(outcome.slice(7));
          // The daemon saves the notes; read them back shortly after.
          window.setTimeout(onSaved, 300);
        }
      })
      .then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [onSaved]);

  const start = async (kind: string, text: string | null) => {
    const turnId = `meeting-${meeting.id}-${Date.now().toString(36)}`;
    setFailure(null);
    setRunning({ turnId, kind, text: "" });
    try {
      await bridge.meetingSummarize(meeting.id, turnId, kind, text);
    } catch (err) {
      setRunning(null);
      onError(err);
    }
  };

  const copy = async (text: string) => {
    try {
      await navigator.clipboard.writeText(text);
      push(t("meetings.copied"));
    } catch (err) {
      onError(err);
    }
  };

  const saved = detail.notes ?? {};
  const partStage = meeting.stage?.startsWith("notes:")
    ? meeting.stage.slice(6)
    : null;
  const order = ["notes", "email", "custom"].filter(
    (kind) => saved[kind] && kind !== running?.kind,
  );

  return (
    <div className="meeting-notes">
      <div className="meeting-notes-actions">
        {NOTE_ACTIONS.map((action) => (
          <Button
            key={action.kind}
            variant={action.kind === "notes" ? "primary" : "default"}
            disabled={running !== null}
            onClick={() => start(action.kind, null)}
          >
            <Icon name="spark" />
            {t(action.label)}
          </Button>
        ))}
        <form
          className="meeting-ask"
          onSubmit={(event) => {
            event.preventDefault();
            if (instruction.trim()) void start("custom", instruction.trim());
          }}
        >
          <input
            value={instruction}
            maxLength={500}
            placeholder={t("meetings.notes.askPlaceholder")}
            aria-label={t("meetings.notes.askPlaceholder")}
            onChange={(e) => setInstruction(e.target.value)}
          />
          <Button
            type="submit"
            disabled={running !== null || !instruction.trim()}
          >
            {t("meetings.notes.ask")}
          </Button>
        </form>
      </div>
      <p className="source meeting-notes-hint">{t("meetings.notes.hint")}</p>

      {failure ? (
        <p className="meeting-note meeting-error">
          {t("meetings.notes.failed", { code: failure })}
        </p>
      ) : null}

      {running ? (
        <section className="meeting-note-card is-live">
          <h4>{t(`meetings.notes.kind.${running.kind}`)}</h4>
          {running.text ? (
            <Markdown text={running.text} />
          ) : (
            <p className="source">
              {partStage
                ? t("meetings.notes.parts", { part: partStage })
                : t("meetings.notes.starting")}
            </p>
          )}
        </section>
      ) : null}

      {order.map((kind) => (
        <section key={kind} className="meeting-note-card">
          <div className="meeting-note-head">
            <h4>{t(`meetings.notes.kind.${kind}`)}</h4>
            <Button variant="ghost" onClick={() => copy(saved[kind])}>
              <Icon name="copy" />
              {t("meetings.copy")}
            </Button>
          </div>
          <Markdown text={saved[kind]} />
        </section>
      ))}

      {!running && order.length === 0 ? (
        <p className="meeting-note">{t("meetings.notes.empty")}</p>
      ) : null}
    </div>
  );
}

interface TurnGroup {
  speaker: string;
  /** `room` (the microphone) or `call` (this computer's sound). */
  source: string;
  start_ms: number;
  language: string;
  texts: string[];
}

/** One speaker's consecutive turns read as one block. */
/** Consecutive turns of one person together. People are told apart by
 * name, so two voices the user gave the same name are one person. */
function groupTurns(
  turns: MeetingTurn[],
  nameOf: (speaker: string) => string = (s) => s,
): TurnGroup[] {
  const groups: TurnGroup[] = [];
  for (const turn of turns) {
    const last = groups[groups.length - 1];
    if (
      last &&
      nameOf(last.speaker) === nameOf(turn.speaker) &&
      last.source === turn.source
    ) {
      last.texts.push(turn.text);
    } else {
      groups.push({
        speaker: turn.speaker,
        source: turn.source,
        start_ms: turn.start_ms,
        language: turn.language,
        texts: [turn.text],
      });
    }
  }
  return groups;
}

function SpeakerChip({
  meetingId,
  speakers,
  name,
  color,
  onRenamed,
}: {
  meetingId: string;
  /** The voices this person is (more than one once merged by name). */
  speakers: string[];
  name: string;
  color: string;
  onRenamed: () => void;
}) {
  const speaker = speakers[0] ?? name;
  const onError = useErrorHandler();
  const [editing, setEditing] = useState(false);
  const [value, setValue] = useState(name === speaker ? "" : name);

  const save = async (event?: FormEvent) => {
    event?.preventDefault();
    setEditing(false);
    try {
      for (const label of speakers) {
        await bridge.meetingRename(meetingId, label, value.trim());
      }
      onRenamed();
    } catch (err) {
      onError(err);
    }
  };

  if (editing) {
    return (
      <form className="meeting-speaker is-editing" onSubmit={save}>
        <span
          className="meeting-speaker-dot"
          style={{ background: color }}
          aria-hidden="true"
        />
        <input
          autoFocus
          value={value}
          maxLength={60}
          placeholder={speaker}
          aria-label={t("meetings.renameLabel", { speaker })}
          onChange={(e) => setValue(e.target.value)}
          onBlur={() => void save()}
          onKeyDown={(e) => {
            if (e.key === "Escape") setEditing(false);
          }}
        />
      </form>
    );
  }
  return (
    <button
      type="button"
      className="meeting-speaker"
      title={t("meetings.renameHint")}
      onClick={() => setEditing(true)}
    >
      <span
        className="meeting-speaker-dot"
        style={{ background: color }}
        aria-hidden="true"
      />
      {name}
      {name !== speaker ? <span className="source"> · {speaker}</span> : null}
      <Icon name="pencil" />
    </button>
  );
}

function RedoForm({
  meeting,
  models,
  onDone,
}: {
  meeting: Meeting;
  models: string[];
  onDone: () => void;
}) {
  const onError = useErrorHandler();
  const [model, setModel] = useState(meeting.model || models[0] || "");
  const [room, setRoom] = useState("");
  const [call, setCall] = useState("");
  const count = (v: string) => {
    const n = Number.parseInt(v, 10);
    return Number.isFinite(n) && n > 0 ? n : null;
  };
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    try {
      await bridge.meetingProcess(
        meeting.id,
        model || null,
        count(room),
        count(call),
      );
      onDone();
    } catch (err) {
      onError(err);
    }
  };
  return (
    <form className="meeting-redo" onSubmit={submit}>
      <label>
        {t("meetings.model")}
        <select value={model} onChange={(e) => setModel(e.target.value)}>
          {models.map((m) => (
            <option key={m} value={m}>
              {MODEL_LABEL[m] ?? m}
            </option>
          ))}
        </select>
      </label>
      <label>
        {t("meetings.roomSpeakers")}
        <input
          type="number"
          min={1}
          max={12}
          value={room}
          placeholder={t("meetings.auto")}
          onChange={(e) => setRoom(e.target.value)}
        />
      </label>
      <label>
        {t("meetings.callSpeakers")}
        <input
          type="number"
          min={1}
          max={12}
          value={call}
          placeholder={t("meetings.auto")}
          onChange={(e) => setCall(e.target.value)}
        />
      </label>
      <Button variant="primary" type="submit">
        {t("meetings.redoGo")}
      </Button>
    </form>
  );
}
