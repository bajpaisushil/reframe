import type { HTMLAttributes, ReactNode } from "react";
import { cx } from "@/lib/cx";
import { Icon, type LucideIcon } from "./Icon";
import styles from "./Badge.module.css";

export type BadgeTone = "neutral" | "accent" | "success" | "warning" | "danger";

export interface BadgeProps extends HTMLAttributes<HTMLSpanElement> {
  tone?: BadgeTone;
  icon?: LucideIcon;
  /** A small colored dot instead of an icon. */
  dot?: boolean;
  children: ReactNode;
}

/** Compact label for status and metadata ("Active", "3 apps"). */
export function Badge({ tone = "neutral", icon, dot = false, className, children, ...rest }: BadgeProps) {
  return (
    <span className={cx(styles.badge, styles[tone], className)} {...rest}>
      {icon ? <Icon icon={icon} size={12} className={styles.icon} /> : dot && <span className={styles.dot} aria-hidden />}
      {children}
    </span>
  );
}
