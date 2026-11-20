import { cx } from "@/lib/cx";
import styles from "./ProgressBar.module.css";

export interface ProgressBarProps {
  /** Current value; omit for an indeterminate bar. */
  value?: number;
  max?: number;
  /** Accessible name ("Restoring Deep work"). */
  label: string;
  /** Human-readable value ("3 of 7 apps") for assistive tech. */
  valueText?: string;
  size?: "sm" | "md";
  tone?: "accent" | "success" | "warning" | "danger";
  className?: string;
}

export function ProgressBar({ value, max = 100, label, valueText, size = "md", tone = "accent", className }: ProgressBarProps) {
  const determinate = value !== undefined && Number.isFinite(value) && max > 0;
  const fraction = determinate ? Math.min(1, Math.max(0, value / max)) : 0;
  return (
    <div
      role="progressbar"
      aria-label={label}
      aria-valuemin={determinate ? 0 : undefined}
      aria-valuemax={determinate ? max : undefined}
      aria-valuenow={determinate ? value : undefined}
      aria-valuetext={valueText}
      className={cx(styles.track, styles[size], styles[tone], !determinate && styles.indeterminate, className)}
    >
      <span className={styles.fill} style={determinate ? { transform: `scaleX(${fraction})` } : undefined} />
    </div>
  );
}
