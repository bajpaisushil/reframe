import { useId, type ReactNode } from "react";
import { cx } from "@/lib/cx";
import styles from "./Field.module.css";

export interface FieldControlProps {
  id: string;
  "aria-describedby": string | undefined;
  "aria-invalid": true | undefined;
  "aria-required": true | undefined;
}

export interface FieldProps {
  label: ReactNode;
  /** Help text under the control. */
  hint?: ReactNode;
  /** Error text (replaces the hint and marks the control invalid). */
  error?: ReactNode;
  /** Shows "Optional" next to the label (Reframe marks optional fields, not required ones). */
  optional?: boolean;
  required?: boolean;
  /** Character counter, e.g. `{ value: name.length, max: 80 }`; turns red past the limit. */
  counter?: { value: number; max: number };
  /** Render the control with the wiring it needs (id, description, invalid state). */
  children: (control: FieldControlProps) => ReactNode;
  className?: string;
}

/** Label + control + hint/error, wired up for assistive technology. */
export function Field({ label, hint, error, optional, required, counter, children, className }: FieldProps) {
  const id = useId();
  const messageId = `${id}-message`;
  const hasError = error !== undefined && error !== null && error !== false && error !== "";
  const message = hasError ? error : hint;
  const over = counter ? counter.value > counter.max : false;
  return (
    <div className={cx(styles.field, className)}>
      <div className={styles.labelRow}>
        <label htmlFor={id} className={styles.label}>
          {label}
          {optional && <span className={styles.optional}>Optional</span>}
        </label>
        {counter && (
          <span className={cx(styles.counter, over && styles.counterOver)} aria-live={over ? "polite" : undefined}>
            {counter.value}/{counter.max}
          </span>
        )}
      </div>
      {children({
        id,
        "aria-describedby": message ? messageId : undefined,
        "aria-invalid": hasError || over || undefined,
        "aria-required": required || undefined,
      })}
      {message && (
        <p id={messageId} className={cx(styles.message, hasError && styles.error)}>
          {message}
        </p>
      )}
    </div>
  );
}
