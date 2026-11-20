import { cx } from "@/lib/cx";
import styles from "./Divider.module.css";

export interface DividerProps {
  orientation?: "horizontal" | "vertical";
  /** Vertical breathing room for horizontal dividers. */
  spacing?: "none" | "sm" | "md" | "lg";
  className?: string;
}

/** A hairline separator. */
export function Divider({ orientation = "horizontal", spacing = "none", className }: DividerProps) {
  if (orientation === "vertical") {
    return <span role="separator" aria-orientation="vertical" className={cx(styles.vertical, className)} />;
  }
  return <hr className={cx(styles.horizontal, styles[spacing], className)} />;
}
