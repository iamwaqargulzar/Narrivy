// Optional History list via daemon_history. Crash-recovery records are never
// presented here; an absent/disabled capability fails closed. Each row shows
// its text, loaded when the row enters the visible window.
// Virtualized: only a window of rows mounts to the DOM (1000 entries -> <50 nodes).
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useErrorHandler } from "../../ui/primitives/Toast";
import { Icon } from "../../ui/icons";
import { t } from "../../i18n";
import { bridge, type JournalEntryDto } from "../../lib/bridge";
import { useDaemonStatus } from "../../state/StatusContext";
import { useSession } from "../../state/SessionContext";
import { usePipelines } from "../../state/PipelinesContext";
import { loadPipelinePrefs } from "../../lib/pipelinePrefs";
import { formatMs } from "../../lib/overview";
import { HistorySettings } from "../components/HistorySettings";

/** A row's text: undefined while loading, null when the entry has none. */
type RowText = string | null | undefined;

const WINDOW_SIZE = 30;
const ROW_HEIGHT_PX = 96;

const STATE_LABELS: Record<
  string,
  { label: string; tone: "" | " chip-ok" | " chip-warn" }
> = {
  delivered: { label: "history.state.delivered", tone: " chip-ok" },
  inbox_recovered: { label: "history.state.inboxRecovered", tone: " chip-warn" },
  failed: { label: "history.state.failed", tone: " chip-warn" },
  cancelled: { label: "history.state.cancelled", tone: "" },
  // Text was transcribed but not typed into the app (same rule as Home's Recent).
  committed: { label: "history.state.notInserted", tone: " chip-warn" },
};

/** Delete needs a daemon at protocol minor 10 or newer. */
const DELETE_PROTOCOL_MINOR = 10;

/** An unmapped daemon state reads as words, not as a wire token. */
function humanState(state: string | null | undefined): string {
  if (!state) return "—";
  const words = state.replace(/[_-]+/g, " ");
  return words.charAt(0).toUpperCase() + words.slice(1);
}

function friendlyTime(ms: number): string {
  const when = new Date(ms);
  const now = new Date();
  const time = when.toLocaleTimeString(undefined, {
    hour: "2-digit",
    minute: "2-digit",
  });
  const startOfDay = (d: Date) =>
    new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const days = Math.round((startOfDay(now) - startOfDay(when)) / 86_400_000);
  if (days === 0) return `${t("history.today")}, ${time}`;
  if (days === 1) return `${t("history.yesterday")}, ${time}`;
  return `${when.toLocaleDateString(undefined, { month: "short", day: "numeric" })}, ${time}`;
}

function PageHead({ count }: { count?: number }) {
  return (
    <div className="page-head">
      <h2 className="page-title">{t("nav.history")}</h2>
      <p className="page-lead">
        {typeof count === "number" && count > 0
          ? t("history.leadCount", { count })
          : t("history.lead")}
      </p>
    </div>
  );
}

export function HistoryScreen() {
  const [entries, setEntries] = useState<JournalEntryDto[] | null>(null);
  const [texts, setTexts] = useState<Record<string, RowText>>({});
  const requested = useRef<Set<string>>(new Set());
  const [copiedId, setCopiedId] = useState<string | null>(null);
  const [offset, setOffset] = useState(0);
  const containerRef = useRef<HTMLDivElement>(null);
  const report = useErrorHandler();
  const {
    status,
    loading: statusLoading,
    reload: reloadStatus,
  } = useDaemonStatus();
  const { finishedSessions } = useSession();
  const { catalog } = usePipelines();
  // The pipeline as the user named it on the Pipelines screen, else its own name.
  const pipelineNames = useMemo(() => loadPipelinePrefs().names, []);
  const modeName = (id: string | null | undefined) =>
    (id
      ? (pipelineNames[id] ?? catalog.pipelines.find((p) => p.id === id)?.name)
      : undefined) ??
    id ??
    "—";
  const historyEnabled =
    status?.capabilities.some(
      (capability) => capability === "history.optional=enabled",
    ) ?? false;
  const canDelete = (status?.protocol_minor ?? 0) >= DELETE_PROTOCOL_MINOR;
  const retention =
    status?.capabilities
      .find((c) => c.startsWith("history.retention="))
      ?.slice("history.retention=".length) ?? null;
  // Keep-for and Clear need a service at protocol minor 34 or newer.
  const settings = (count: number) =>
    (status?.protocol_minor ?? 0) >= 34 ? (
      <HistorySettings
        retention={retention}
        count={count}
        onChanged={() => {
          void reloadStatus();
          void load();
        }}
      />
    ) : null;

  const load = useCallback(async () => {
    try {
      const list = await bridge.history();
      setEntries(list);
    } catch (err) {
      setEntries([]);
      report(err, { retry: () => void load() });
    }
  }, [report]);

  // Reload after every finished dictation, so the list stays current while open.
  useEffect(() => {
    if (historyEnabled) void load();
  }, [historyEnabled, load, finishedSessions]);

  useEffect(() => {
    if (!copiedId) return;
    const id = window.setTimeout(() => setCopiedId(null), 1600);
    return () => window.clearTimeout(id);
  }, [copiedId]);

  const copy = async (txid: string, text: string) => {
    try {
      await navigator.clipboard.writeText(text);
      setCopiedId(txid);
    } catch (err) {
      report(err ?? new Error(t("home.copyFailed")), {
        retry: () => void copy(txid, text),
      });
    }
  };

  const remove = async (txid: string) => {
    try {
      await bridge.deleteHistoryEntry(txid);
      setEntries((current) =>
        (current ?? []).filter((entry) => entry.txid !== txid),
      );
      setTexts((prev) => {
        const next = { ...prev };
        delete next[txid];
        return next;
      });
    } catch (err) {
      report(err, { retry: () => void remove(txid) });
    }
  };

  const onScroll = useCallback(() => {
    const el = containerRef.current;
    if (!el) return;
    const raw = Math.floor(el.scrollTop / ROW_HEIGHT_PX);
    const maxOffset = Math.max(0, (entries?.length ?? 0) - WINDOW_SIZE);
    setOffset(Math.min(maxOffset, Math.max(0, raw)));
  }, [entries]);

  const visible = useMemo(
    () => (entries ? entries.slice(offset, offset + WINDOW_SIZE) : []),
    [entries, offset],
  );

  // Load the text of each visible row once. A failed read shows as "no text"
  // for that row instead of interrupting the list with an error per row.
  useEffect(() => {
    for (const entry of visible) {
      if (requested.current.has(entry.txid)) continue;
      requested.current.add(entry.txid);
      void bridge
        .historyEntry(entry.txid)
        .then((full) => full.delivered ?? full.corrected ?? full.text ?? null)
        .catch(() => null)
        .then((text) => setTexts((prev) => ({ ...prev, [entry.txid]: text })));
    }
  }, [visible]);

  const totalHeight = (entries?.length ?? 0) * ROW_HEIGHT_PX;
  const topSpacer = offset * ROW_HEIGHT_PX;
  const bottomSpacer = Math.max(
    0,
    totalHeight - topSpacer - visible.length * ROW_HEIGHT_PX,
  );

  if (statusLoading && !status) {
    return (
      <div className="page">
        <PageHead />
        <p className="screen-empty-note">{t("common.loading")}</p>
      </div>
    );
  }
  if (!historyEnabled) {
    return (
      <div className="page">
        <PageHead />
        <div className="panel empty-state">
          <Icon name="shield" />
          <strong>{t("history.offTitle")}</strong>
          <span className="screen-empty-note" role="status" aria-live="polite">
            {t("history.disabled")}
          </span>
        </div>
      </div>
    );
  }
  if (!entries) {
    return (
      <div className="page">
        <PageHead />
        <p className="screen-empty-note">{t("common.loading")}</p>
      </div>
    );
  }
  if (entries.length === 0) {
    return (
      <div className="page">
        <PageHead />
        {settings(0)}
        <div className="panel empty-state">
          <Icon name="history" />
          <strong>{t("history.emptyTitle")}</strong>
          <span className="screen-empty-note">{t("history.empty")}</span>
        </div>
      </div>
    );
  }

  return (
    <div className="page">
      <PageHead count={entries.length} />
      {settings(entries.length)}
      <div
        ref={containerRef}
        className="history-scroll"
        data-testid="history-virtual-container"
        onScroll={onScroll}
      >
        <div
          style={{ height: `${topSpacer}px`, pointerEvents: "none" }}
          aria-hidden="true"
        />
        <ul className="history-list" data-testid="history-virtual-list">
          {visible.map((entry) => {
            const text = texts[entry.txid];
            const known = STATE_LABELS[entry.state ?? ""];
            const state = known
              ? { label: t(known.label), tone: known.tone }
              : { label: humanState(entry.state), tone: "" as const };
            return (
              <li
                key={entry.txid}
                className="history-item"
                data-txid={entry.txid}
              >
                <div className="history-meta">
                  <span className="history-time">
                    {typeof entry.updated_unix_ms === "number"
                      ? friendlyTime(entry.updated_unix_ms)
                      : t("history.timeUnknown")}
                  </span>
                  <span
                    className="chip history-pipeline"
                    title={t("history.pipelineTitle")}
                  >
                    <Icon name="pipeline" />
                    {modeName(entry.pipeline_id)}
                  </span>
                  <span className={`chip${state.tone}`}>{state.label}</span>
                  {typeof entry.transcribe_us === "number" ? (
                    <span
                      className="history-timing"
                      title={t("history.transcribedTitle")}
                    >
                      <Icon name="gauge" />
                      {t("history.transcribedIn", {
                        time:
                          formatMs(
                            Math.max(1, Math.round(entry.transcribe_us / 1000)),
                          ) ?? "",
                      })}
                    </span>
                  ) : null}
                  <span className="sr-only">
                    {entry.has_delivered_text
                      ? t("history.deliveredYes")
                      : t("history.deliveredNo")}
                  </span>
                </div>
                <div className="history-actions">
                  <button
                    type="button"
                    className="icon-btn"
                    onClick={() => text && void copy(entry.txid, text)}
                    disabled={!text}
                    aria-label={t("history.copy")}
                    title={t("history.copy")}
                  >
                    <Icon name={copiedId === entry.txid ? "check" : "copy"} />
                  </button>
                  <button
                    type="button"
                    className="icon-btn icon-btn-danger"
                    onClick={() => void remove(entry.txid)}
                    disabled={!canDelete}
                    aria-label={t("recent.delete")}
                    title={
                      canDelete
                        ? t("recent.delete")
                        : t("recent.deleteUnsupported")
                    }
                  >
                    <Icon name="trash" />
                  </button>
                </div>
                {text === undefined ? (
                  <p className="history-text history-text-muted">
                    {t("history.loadingText")}
                  </p>
                ) : text ? (
                  <p className="history-text" title={text}>
                    {text}
                  </p>
                ) : (
                  <p className="history-text history-text-muted">
                    {t("history.noText")}
                  </p>
                )}
              </li>
            );
          })}
        </ul>
        <div
          style={{ height: `${bottomSpacer}px`, pointerEvents: "none" }}
          aria-hidden="true"
        />
      </div>
    </div>
  );
}
