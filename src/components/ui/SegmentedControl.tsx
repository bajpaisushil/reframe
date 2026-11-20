import { useRef, type KeyboardEvent } from "react";
import { cx } from "@/lib/cx";
import { Icon, type LucideIcon } from "./Icon";
import styles from "./SegmentedControl.module.css";

export interface SegmentOption<T extends string> {
  value: T;
  label: string;
  icon?: LucideIcon;
  disabled?: boolean;
}

export interface SegmentedControlProps<T extends string> {
  value: T;
  onChange: (value: T) => void;
  options: readonly SegmentOption<T>[];
  /** Accessible name for the group. */
  "aria-label": string;
  size?: "sm" | "md";
  fullWidth?: boolean;
  className?: string;
}

/** A small set of mutually exclusive options (radio group; arrow keys move the selection). */
export function SegmentedControl<T extends string>({
  value,
  onChange,
  options,
  size = "md",
  fullWidth = false,
  className,
  ...rest
}: SegmentedControlProps<T>) {
  const refs = useRef<Array<HTMLButtonElement | null>>([]);
  const enabled = options.map((o, i) => (o.disabled ? -1 : i)).filter((i) => i !== -1);

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const keys = ["ArrowRight", "ArrowDown", "ArrowLeft", "ArrowUp", "Home", "End"];
    if (!keys.includes(event.key) || enabled.length === 0) return;
    event.preventDefault();
    const current = enabled.indexOf(options.findIndex((o) => o.value === value));
    let next: number;
    if (event.key === "Home") next = 0;
    else if (event.key === "End") next = enabled.length - 1;
    else {
      const step = event.key === "ArrowRight" || event.key === "ArrowDown" ? 1 : -1;
      next = (Math.max(current, 0) + step + enabled.length) % enabled.length;
    }
    const index = enabled[next];
    onChange(options[index].value);
    refs.current[index]?.focus();
  };

  return (
    <div
      role="radiogroup"
      aria-label={rest["aria-label"]}
      className={cx(styles.group, styles[size], fullWidth && styles.fullWidth, className)}
      onKeyDown={onKeyDown}
    >
      {options.map((option, index) => {
        const selected = option.value === value;
        return (
          <button
            key={option.value}
            ref={(el) => {
              refs.current[index] = el;
            }}
            type="button"
            role="radio"
            aria-checked={selected}
            tabIndex={selected || (!options.some((o) => o.value === value) && index === enabled[0]) ? 0 : -1}
            disabled={option.disabled}
            className={cx(styles.segment, selected && styles.selected)}
            onClick={() => onChange(option.value)}
          >
            {option.icon && <Icon icon={option.icon} size={size === "sm" ? 14 : 16} />}
            <span>{option.label}</span>
          </button>
        );
      })}
    </div>
  );
}
