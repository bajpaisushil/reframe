import { useId, type ReactNode } from "react";
import { cx } from "@/lib/cx";
import styles from "./Switch.module.css";

export interface SwitchProps {
  checked: boolean;
  onChange: (checked: boolean) => void;
  /** Visible label (clicking it toggles). Without it, pass `aria-label`. */
  label?: ReactNode;
  description?: ReactNode;
  disabled?: boolean;
  "aria-label"?: string;
  id?: string;
  className?: string;
}

/** On/off toggle (`role="switch"`). */
export function Switch({ checked, onChange, label, description, disabled = false, id, className, ...rest }: SwitchProps) {
  const autoId = useId();
  const switchId = id ?? autoId;
  const labelId = `${switchId}-label`;
  const descriptionId = `${switchId}-description`;
  const control = (
    <button
      id={switchId}
      type="button"
      role="switch"
      aria-checked={checked}
      aria-labelledby={label ? labelId : undefined}
      aria-label={label ? undefined : rest["aria-label"]}
      aria-describedby={description ? descriptionId : undefined}
      disabled={disabled}
      className={styles.switch}
      onClick={() => onChange(!checked)}
    >
      <span className={styles.thumb} />
    </button>
  );
  if (!label) return <span className={className}>{control}</span>;
  return (
    <div className={cx(styles.row, disabled && styles.disabled, className)}>
      <div className={styles.text}>
        <label id={labelId} htmlFor={switchId} className={styles.label}>
          {label}
        </label>
        {description && (
          <p id={descriptionId} className={styles.description}>
            {description}
          </p>
        )}
      </div>
      {control}
    </div>
  );
}
