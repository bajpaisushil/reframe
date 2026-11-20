import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { Input } from "@/components/ui";
import { cx } from "@/lib/cx";
import { formatSeconds, parseSeconds, parseWholeNumber, type NumberRange } from "../model/validation";
import styles from "./NumberField.module.css";

export interface NumberFieldProps {
  label: ReactNode;
  hint?: ReactNode;
  /** Stored value: milliseconds for `seconds`, the number itself for `count`. */
  value: number | null;
  onChange: (value: number | null) => void;
  unit: "seconds" | "count";
  /** Allowed range in the unit shown (seconds or a count). */
  range: NumberRange;
  /** Shown when empty (e.g. the workspace default). */
  placeholder?: string;
  className?: string;
}

function format(value: number | null, unit: NumberFieldProps["unit"]): string {
  if (value === null) return "";
  return unit === "seconds" ? formatSeconds(value) : String(value);
}

/**
 * A settings row with a small numeric field: valid values are applied as you type, invalid ones
 * are explained, and leaving the field restores the last valid value.
 */
export function NumberField({ label, hint, value, onChange, unit, range, placeholder, className }: NumberFieldProps) {
  const id = useId();
  const hintId = `${id}-hint`;
  const errorId = `${id}-error`;
  const [text, setText] = useState(() => format(value, unit));
  const [error, setError] = useState<string | null>(null);
  const focused = useRef(false);

  // Follow outside changes (discard, a fresh save) unless the user is typing.
  useEffect(() => {
    if (!focused.current) {
      setText(format(value, unit));
      setError(null);
    }
  }, [value, unit]);

  const parse = (input: string) => (unit === "seconds" ? parseSeconds(input, range) : parseWholeNumber(input, range));
  const describedBy = [hint ? hintId : null, error ? errorId : null].filter(Boolean).join(" ") || undefined;

  return (
    <div className={cx(styles.row, className)}>
      <div className={styles.text}>
        <label htmlFor={id} className={styles.label}>
          {label}
        </label>
        {hint && (
          <p id={hintId} className={styles.hint}>
            {hint}
          </p>
        )}
      </div>
      <div className={styles.control}>
        <span className={styles.wrap}>
          <Input
            id={id}
            size="sm"
            inputMode={unit === "seconds" ? "decimal" : "numeric"}
            value={text}
            placeholder={placeholder}
            aria-invalid={error ? true : undefined}
            aria-describedby={describedBy}
            className={cx(styles.input, unit === "seconds" && styles.withSuffix)}
            onFocus={() => {
              focused.current = true;
            }}
            onChange={(event) => {
              const next = event.target.value;
              setText(next);
              const parsed = parse(next);
              if (parsed.ok) {
                setError(null);
                if (parsed.value !== value) onChange(parsed.value);
              } else {
                setError(parsed.error);
              }
            }}
            onBlur={() => {
              focused.current = false;
              setText(format(value, unit));
              setError(null);
            }}
          />
          {unit === "seconds" && (
            <span className={styles.suffix} aria-hidden="true">
              s
            </span>
          )}
        </span>
        {error && (
          <p id={errorId} className={styles.error} role="alert">
            {error}
          </p>
        )}
      </div>
    </div>
  );
}
