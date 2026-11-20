import { cx } from "@/lib/cx";
import styles from "./Spinner.module.css";

export interface SpinnerProps {
  size?: 12 | 14 | 16 | 20 | 24;
  /** Accessible label; without it the spinner is decorative (e.g. inside a labelled button). */
  label?: string;
  className?: string;
}

/** Circular activity indicator in the current text color. */
export function Spinner({ size = 16, label, className }: SpinnerProps) {
  const a11y = label ? { role: "status", "aria-label": label } : { "aria-hidden": true };
  return (
    <span className={cx(styles.spinner, className)} style={{ width: size, height: size }} {...a11y}>
      <svg viewBox="0 0 16 16" width={size} height={size} fill="none">
        <circle className={styles.track} cx="8" cy="8" r="6.25" strokeWidth="1.75" />
        <circle className={styles.arc} cx="8" cy="8" r="6.25" strokeWidth="1.75" strokeLinecap="round" />
      </svg>
    </span>
  );
}
