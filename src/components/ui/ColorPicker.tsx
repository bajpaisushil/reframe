import { useRef, type KeyboardEvent } from "react";
import { WORKSPACE_COLORS, normalizeHexColor } from "@/lib/color";
import { cx } from "@/lib/cx";
import { Tooltip } from "./Tooltip";
import styles from "./ColorPicker.module.css";

export interface ColorPickerProps {
  /** `#RRGGBB` or null for "no color". */
  value: string | null;
  onChange: (value: string | null) => void;
  "aria-label"?: string;
  className?: string;
}

const OPTIONS: Array<{ value: string | null; name: string }> = [
  { value: null, name: "No color" },
  ...WORKSPACE_COLORS.map((c) => ({ value: c.value, name: c.name })),
];

/** Eight curated workspace colors plus "no color" (radio group; arrow keys move). */
export function ColorPicker({ value, onChange, className, ...rest }: ColorPickerProps) {
  const refs = useRef<Array<HTMLButtonElement | null>>([]);
  const normalized = normalizeHexColor(value);
  const selectedIndex = Math.max(
    0,
    OPTIONS.findIndex((o) => o.value === normalized),
  );

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const step = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[event.key];
    let next: number | undefined;
    if (step !== undefined) next = (selectedIndex + step + OPTIONS.length) % OPTIONS.length;
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = OPTIONS.length - 1;
    if (next === undefined) return;
    event.preventDefault();
    onChange(OPTIONS[next].value);
    refs.current[next]?.focus();
  };

  return (
    <div role="radiogroup" aria-label={rest["aria-label"] ?? "Color"} className={cx(styles.group, className)} onKeyDown={onKeyDown}>
      {OPTIONS.map((option, index) => {
        const selected = index === selectedIndex;
        return (
          <Tooltip key={option.name} content={option.name} delay={300}>
            <button
              ref={(el) => {
                refs.current[index] = el;
              }}
              type="button"
              role="radio"
              aria-checked={selected}
              aria-label={option.name}
              tabIndex={selected ? 0 : -1}
              className={cx(styles.swatch, option.value === null && styles.none, selected && styles.selected)}
              style={option.value ? { ["--swatch" as string]: option.value } : undefined}
              onClick={() => onChange(option.value)}
            />
          </Tooltip>
        );
      })}
    </div>
  );
}
