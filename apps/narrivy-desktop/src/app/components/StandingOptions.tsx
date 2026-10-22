// Shared building blocks for screens that list options together with where
// each one stands: Live (works here), From service (read from the daemon),
// Coming (scheduled, with its completion-plan task) or Proposed (awaiting an
// owner decision). A choice list for an option that is not connected yet is
// visible but never actionable.
import type { ReactNode } from "react";
import { t } from "../../i18n";

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

/** One option inside an expanded stage: what it is, its current value, where it stands. */
export function Option({
  label,
  hint,
  value,
  standing,
  children,
}: {
  label: string;
  hint?: string;
  value?: ReactNode;
  standing: Standing;
  children?: ReactNode;
}) {
  return (
    <div className="pipe-option" data-standing={standing.kind}>
      <div className="pipe-option-text">
        <span className="pipe-option-label">
          {label}
          <Badge standing={standing} />
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

/** A choice list. Without `onPick` the option is not connected yet:
 * visible, never actionable. With it, a live choice. */
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
  if (onPick) {
    return (
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
            {option.note ? <em>{option.note}</em> : null}
          </button>
        ))}
      </div>
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
