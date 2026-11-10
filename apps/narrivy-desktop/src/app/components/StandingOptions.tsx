// Shared building blocks for screens that list options together with where
// each one stands: Live (works here), From service (read from the daemon),
// Coming (scheduled, with its completion-plan task) or Proposed (awaiting an
// owner decision). Live is the normal case and carries no badge; the others
// are marked. A choice list for an option that is not connected yet is
// visible but never actionable.
import type { ReactNode } from "react";
import { t } from "../../i18n";
import { Icon, type IconName } from "../../ui/icons";

export type Standing =
  | { kind: "live" }
  | { kind: "reported" }
  | { kind: "planned"; task: string }
  | { kind: "proposed" };

export const LIVE: Standing = { kind: "live" };
export const REPORTED: Standing = { kind: "reported" };
export const PROPOSED: Standing = { kind: "proposed" };
export const planned = (task: string): Standing => ({ kind: "planned", task });

export function Badge({ standing }: { standing: Standing }) {
  switch (standing.kind) {
    case "live":
      return (
        <span className="chip chip-ok fm-badge">{t("fm.status.live")}</span>
      );
    case "reported":
      return <span className="chip fm-badge">{t("fm.status.reported")}</span>;
    case "planned":
      return (
        <span
          className="chip chip-warn fm-badge"
          title={t("fm.status.plannedTitle", { task: standing.task })}
        >
          {t("fm.status.planned", { task: standing.task })}
        </span>
      );
    default:
      return (
        <span
          className="chip fm-badge fm-badge-proposed"
          title={t("fm.status.proposedTitle")}
        >
          {t("fm.status.proposed")}
        </span>
      );
  }
}

/** One option inside an expanded stage: what it is, its current value, where
 * it stands. A small control (a switch, a menu) sits beside the label; a
 * `block` one (a choice list, a model list) gets the whole width below it. */
export function Option({
  label,
  hint,
  value,
  standing,
  block = false,
  children,
}: {
  label: string;
  hint?: string;
  value?: ReactNode;
  standing: Standing;
  block?: boolean;
  children?: ReactNode;
}) {
  return (
    <div
      className="pipe-option"
      data-standing={standing.kind}
      data-layout={block ? "block" : "inline"}
    >
      <div className="pipe-option-text">
        <span className="pipe-option-label">
          {label}
          {standing.kind === "live" ? null : <Badge standing={standing} />}
        </span>
        {hint ? <span className="pipe-option-hint">{hint}</span> : null}
      </div>
      {value !== undefined ? (
        <span className="pipe-option-value">{value}</span>
      ) : null}
      {children ? <div className="pipe-option-control">{children}</div> : null}
    </div>
  );
}

/** A group of options in an expanded stage, as one card. `wide` cards take
 * the whole row. */
export function Section({
  icon,
  title,
  lead,
  wide = false,
  children,
}: {
  icon?: IconName;
  title: string;
  lead?: string;
  wide?: boolean;
  children: ReactNode;
}) {
  return (
    <section className="pipe-section" data-wide={wide ? "true" : undefined}>
      <header className="pipe-section-head">
        {icon ? (
          <span className="pipe-section-icon" aria-hidden="true">
            <Icon name={icon} />
          </span>
        ) : null}
        <div>
          <h4>{title}</h4>
          {lead ? <p>{lead}</p> : null}
        </div>
      </header>
      <div className="pipe-section-body">{children}</div>
    </section>
  );
}

/** What the service reports about a stage, read-only, in one quiet row. */
export function Facts({
  items,
}: {
  items: Array<{ label: string; value: ReactNode } | null>;
}) {
  const shown = items.filter(
    (item): item is { label: string; value: ReactNode } => item !== null,
  );
  if (!shown.length) return null;
  return (
    <dl className="pipe-status" aria-label={t("pipe.statusTitle")}>
      <span className="pipe-status-title">{t("pipe.statusTitle")}</span>
      {shown.map((item) => (
        <div key={item.label}>
          <dt>{item.label}</dt>
          <dd>{item.value}</dd>
        </div>
      ))}
    </dl>
  );
}

/** A choice list, drawn as one segmented control. Without `onPick` the
 * option is not connected yet: visible, never actionable. With it, a live
 * choice; the chosen option's `note` is shown under the control. */
export function Choices({
  options,
  current,
  onPick,
  disabled = false,
  label,
}: {
  options: Array<{ id: string; label: string; note?: string }>;
  current: string;
  onPick?: (id: string) => void;
  disabled?: boolean;
  label?: string;
}) {
  const note = options.find((option) => option.id === current)?.note;
  if (onPick) {
    return (
      <>
      <div
        className="pipe-choices"
        role="radiogroup"
        aria-label={label}
        aria-disabled={disabled || undefined}
      >
        {options.map((option) => (
          <button
            key={option.id}
            type="button"
            role="radio"
            aria-checked={option.id === current}
            className="pipe-choice pipe-choice-live"
            title={option.note}
            disabled={disabled}
            onClick={() => option.id !== current && onPick(option.id)}
          >
            {option.label}
          </button>
        ))}
      </div>
      {note ? <p className="pipe-choice-note">{note}</p> : null}
      </>
    );
  }
  return (
    <div className="pipe-choices" role="radiogroup" aria-disabled="true">
      {options.map((option) => (
        <span
          key={option.id}
          role="radio"
          aria-checked={option.id === current}
          aria-disabled="true"
          className="pipe-choice"
          title={option.note}
        >
          {option.label}
          {option.note ? <em>{option.note}</em> : null}
        </span>
      ))}
    </div>
  );
}
