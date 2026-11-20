import type { ReactNode } from "react";
import { cx } from "@/lib/cx";
import styles from "./SettingsSection.module.css";

export interface SettingsSectionProps {
  id: string;
  title: string;
  description?: ReactNode;
  children: ReactNode;
}

/** One settings topic: title and explanation on the left, its options in a card on the right. */
export function SettingsSection({ id, title, description, children }: SettingsSectionProps) {
  return (
    <section className={styles.section} aria-labelledby={id}>
      <div className={styles.intro}>
        <h2 id={id} className={styles.title}>
          {title}
        </h2>
        {description && <p className={styles.description}>{description}</p>}
      </div>
      <div className={styles.card}>{children}</div>
    </section>
  );
}

export interface SettingsRowProps {
  label?: ReactNode;
  description?: ReactNode;
  /** Control on the right of the label. */
  control?: ReactNode;
  /** Content under the label row (lists, radio groups, notes). */
  children?: ReactNode;
  labelId?: string;
  className?: string;
}

/** A row inside a settings card; rows are separated by hairlines. */
export function SettingsRow({ label, description, control, children, labelId, className }: SettingsRowProps) {
  return (
    <div className={cx(styles.row, className)}>
      {(label || control) && (
        <div className={styles.rowHead}>
          {label && (
            <div className={styles.rowText}>
              <div id={labelId} className={styles.rowLabel}>
                {label}
              </div>
              {description && <p className={styles.rowDescription}>{description}</p>}
            </div>
          )}
          {control && <div className={styles.rowControl}>{control}</div>}
        </div>
      )}
      {children && <div className={cx(styles.rowBody, !label && !control && styles.rowBodyOnly)}>{children}</div>}
    </div>
  );
}
