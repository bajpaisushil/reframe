import { useEffect, useRef, type ReactNode } from "react";
import { Check, Minus } from "lucide-react";
import { cx } from "@/lib/cx";
import { Icon } from "./Icon";
import styles from "./Checkbox.module.css";

export interface CheckboxProps {
  checked: boolean;
  onChange: (checked: boolean) => void;
  /** Mixed state (some children selected). */
  indeterminate?: boolean;
  label?: ReactNode;
  description?: ReactNode;
  disabled?: boolean;
  "aria-label"?: string;
  id?: string;
  className?: string;
}

/** A native checkbox with Reframe styling (keeps native keyboard and form behaviour). */
export function Checkbox({
  checked,
  onChange,
  indeterminate = false,
  label,
  description,
  disabled = false,
  id,
  className,
  ...rest
}: CheckboxProps) {
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (ref.current) ref.current.indeterminate = indeterminate;
  }, [indeterminate]);

  return (
    <label className={cx(styles.wrapper, disabled && styles.disabled, !label && styles.bare, className)}>
      <span className={styles.boxWrap}>
        <input
          ref={ref}
          id={id}
          type="checkbox"
          className={styles.input}
          checked={checked}
          disabled={disabled}
          aria-label={label ? undefined : rest["aria-label"]}
          onChange={(event) => onChange(event.target.checked)}
        />
        <span className={styles.box} aria-hidden="true">
          <Icon icon={indeterminate ? Minus : Check} size={12} strokeWidth={2.5} className={styles.mark} />
        </span>
      </span>
      {label && (
        <span className={styles.text}>
          <span className={styles.label}>{label}</span>
          {description && <span className={styles.description}>{description}</span>}
        </span>
      )}
    </label>
  );
}
