import type { ReactNode } from "react";
import { cx } from "@/lib/cx";
import styles from "./KeyValue.module.css";

export interface KeyValueListProps {
  children: ReactNode;
  /** Width of the label column. */
  labelWidth?: number;
  className?: string;
}

/** A list of label/value rows (`<dl>`). */
export function KeyValueList({ children, labelWidth = 160, className }: KeyValueListProps) {
  return (
    <dl className={cx(styles.list, className)} style={{ ["--kv-label-width" as string]: `${labelWidth}px` }}>
      {children}
    </dl>
  );
}

export interface KeyValueProps {
  label: ReactNode;
  value: ReactNode;
  /** Technical values (paths, ids) in Geist Mono and selectable for copying. */
  mono?: boolean;
}

export function KeyValue({ label, value, mono = false }: KeyValueProps) {
  return (
    <div className={styles.row}>
      <dt className={styles.label}>{label}</dt>
      <dd className={cx(styles.value, mono && "mono selectable")}>{value}</dd>
    </div>
  );
}
