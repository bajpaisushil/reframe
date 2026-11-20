import type { HTMLAttributes, ReactNode, Ref } from "react";
import { cx } from "@/lib/cx";
import styles from "./Card.module.css";

export interface CardProps extends HTMLAttributes<HTMLDivElement> {
  padding?: "none" | "sm" | "md" | "lg";
  /** Subtle hover state for clickable cards. */
  interactive?: boolean;
  /** `subtle` sits quieter on a white surface. */
  tone?: "surface" | "subtle";
  ref?: Ref<HTMLDivElement>;
}

/** A bordered surface (10px radius, hairline border, no shadow). */
export function Card({ padding = "md", interactive = false, tone = "surface", className, ref, ...rest }: CardProps) {
  return (
    <div
      ref={ref}
      className={cx(styles.card, styles[`pad-${padding}`], styles[tone], interactive && styles.interactive, className)}
      {...rest}
    />
  );
}

export interface PanelProps extends Omit<CardProps, "title"> {
  title?: ReactNode;
  description?: ReactNode;
  /** Controls on the right of the header. */
  actions?: ReactNode;
  children?: ReactNode;
}

/** A card with an optional header row (title, description, actions). */
export function Panel({ title, description, actions, children, padding = "md", className, ...rest }: PanelProps) {
  const hasHeader = title !== undefined || actions !== undefined;
  return (
    <Card padding="none" className={cx(styles.panel, className)} {...rest}>
      {hasHeader && (
        <div className={styles.panelHeader}>
          <div className={styles.panelHeading}>
            {title !== undefined && <h3 className={styles.panelTitle}>{title}</h3>}
            {description !== undefined && <p className={styles.panelDescription}>{description}</p>}
          </div>
          {actions !== undefined && <div className={styles.panelActions}>{actions}</div>}
        </div>
      )}
      <div className={styles[`pad-${padding}`]}>{children}</div>
    </Card>
  );
}
