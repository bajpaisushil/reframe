import { useId, type ReactNode } from "react";
import { cx } from "@/lib/cx";
import styles from "./RadioGroup.module.css";

export interface RadioOption<T extends string> {
  value: T;
  label: string;
  description?: ReactNode;
  disabled?: boolean;
  /** Control shown next to the option (e.g. a select that belongs to it). */
  extra?: ReactNode;
}

export interface RadioGroupProps<T extends string> {
  value: T;
  onChange: (value: T) => void;
  options: readonly RadioOption<T>[];
  /** Id of the visible label of the group. */
  "aria-labelledby"?: string;
  "aria-label"?: string;
  className?: string;
}

/** A vertical list of native radio buttons (arrow keys move between them). */
export function RadioGroup<T extends string>({ value, onChange, options, className, ...rest }: RadioGroupProps<T>) {
  const name = useId();
  return (
    <div role="radiogroup" className={cx(styles.group, className)} {...rest}>
      {options.map((option) => {
        const checked = option.value === value;
        return (
          <div key={option.value} className={cx(styles.option, option.disabled && styles.disabled)}>
            <label className={styles.label}>
              <input
                type="radio"
                className={styles.input}
                name={name}
                value={option.value}
                checked={checked}
                disabled={option.disabled}
                onChange={() => onChange(option.value)}
              />
              <span className={styles.dot} aria-hidden="true" />
              <span className={styles.text}>
                <span className={styles.title}>{option.label}</span>
                {option.description && <span className={styles.description}>{option.description}</span>}
              </span>
            </label>
            {option.extra && <div className={styles.extra}>{option.extra}</div>}
          </div>
        );
      })}
    </div>
  );
}
