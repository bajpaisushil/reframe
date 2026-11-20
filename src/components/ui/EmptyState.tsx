import type { ReactNode } from "react";
import { cx } from "@/lib/cx";
import { Icon, type LucideIcon } from "./Icon";
import styles from "./EmptyState.module.css";

export interface EmptyStateProps {
  icon?: LucideIcon;
  title: ReactNode;
  text?: ReactNode;
  /** Buttons or links. */
  action?: ReactNode;
  /** `page` fills a page area; `compact` sits inside a section. */
  size?: "page" | "compact";
  /** `danger` for error states. */
  tone?: "default" | "danger";
  className?: string;
}

/** A calm explanation of why something is empty (or failed) and what to do next. */
export function EmptyState({ icon, title, text, action, size = "page", tone = "default", className }: EmptyStateProps) {
  return (
    <div className={cx(styles.empty, styles[size], tone === "danger" && styles.danger, className)}>
      {icon && (
        <div className={styles.iconWrap}>
          <Icon icon={icon} size={size === "page" ? 20 : 18} />
        </div>
      )}
      <div className={styles.copy}>
        <h3 className={styles.title}>{title}</h3>
        {text !== undefined && <p className={styles.text}>{text}</p>}
      </div>
      {action !== undefined && <div className={styles.action}>{action}</div>}
    </div>
  );
}
