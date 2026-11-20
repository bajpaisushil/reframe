import { Ban, CircleCheck, CircleDashed, CircleExclamationPoint, CircleMinus, CircleX } from "lucide-react";
import { cx } from "@/lib/cx";
import type { EntryStatus, RunStatus } from "@/types";
import type { BadgeTone } from "./Badge";
import { Icon, type LucideIcon } from "./Icon";
import { Spinner } from "./Spinner";
import styles from "./StatusPill.module.css";

interface StatusCopy {
  label: string;
  tone: BadgeTone;
  /** `null` = in progress (spinner). */
  icon: LucideIcon | null;
}

export const ENTRY_STATUS_COPY: Record<EntryStatus, StatusCopy> = {
  pending: { label: "Waiting", tone: "neutral", icon: CircleDashed },
  validating: { label: "Checking", tone: "accent", icon: null },
  launching: { label: "Launching", tone: "accent", icon: null },
  restoring: { label: "Arranging", tone: "accent", icon: null },
  completed: { label: "Restored", tone: "success", icon: CircleCheck },
  partiallyCompleted: { label: "Partly restored", tone: "warning", icon: CircleExclamationPoint },
  failed: { label: "Failed", tone: "danger", icon: CircleX },
  skipped: { label: "Skipped", tone: "neutral", icon: CircleMinus },
};

export const RUN_STATUS_COPY: Record<RunStatus, StatusCopy> = {
  running: { label: "Restoring", tone: "accent", icon: null },
  completed: { label: "Restored", tone: "success", icon: CircleCheck },
  partiallyCompleted: { label: "Partly restored", tone: "warning", icon: CircleExclamationPoint },
  failed: { label: "Failed", tone: "danger", icon: CircleX },
  cancelled: { label: "Cancelled", tone: "neutral", icon: Ban },
};

function Pill({ copy, className, label }: { copy: StatusCopy; className?: string; label?: string }) {
  return (
    <span className={cx(styles.pill, styles[copy.tone], className)}>
      {copy.icon ? <Icon icon={copy.icon} size={14} className={styles.icon} /> : <Spinner size={12} className={styles.icon} />}
      {label ?? copy.label}
    </span>
  );
}

export interface StatusPillProps {
  status: EntryStatus;
  /** Override the label (e.g. "Restored 2 of 3 windows"). */
  label?: string;
  className?: string;
}

/** Status of one application during a restore: icon + label, spinner while in progress. */
export function StatusPill({ status, label, className }: StatusPillProps) {
  return <Pill copy={ENTRY_STATUS_COPY[status] ?? ENTRY_STATUS_COPY.pending} label={label} className={className} />;
}

export interface RunStatusPillProps {
  status: RunStatus;
  label?: string;
  className?: string;
}

/** Status of a whole restore run. */
export function RunStatusPill({ status, label, className }: RunStatusPillProps) {
  return <Pill copy={RUN_STATUS_COPY[status] ?? RUN_STATUS_COPY.running} label={label} className={className} />;
}
