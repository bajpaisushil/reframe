import { useId, useRef, useState, type FormEvent, type KeyboardEvent } from "react";
import { cx } from "@/lib/cx";
import { CURATED_EMOJI, normalizeEmojiInput } from "@/lib/emoji";
import { Button } from "./Button";
import { Input } from "./Input";
import styles from "./EmojiPicker.module.css";

export interface EmojiPickerProps {
  value: string;
  onChange: (emoji: string) => void;
  /** Columns in the grid (keyboard navigation follows it). */
  columns?: number;
  className?: string;
}

/**
 * Curated grid of work/life emoji (arrow keys move, Enter picks) plus a field for any other emoji.
 * Renders inline; put it in a Popover for a compact trigger.
 */
export function EmojiPicker({ value, onChange, columns = 8, className }: EmojiPickerProps) {
  const inputId = useId();
  const refs = useRef<Array<HTMLButtonElement | null>>([]);
  const selectedIndex = CURATED_EMOJI.findIndex((e) => e.emoji === value);
  const [focusIndex, setFocusIndex] = useState(Math.max(0, selectedIndex));
  const [custom, setCustom] = useState("");
  const [customError, setCustomError] = useState<string | null>(null);

  const move = (index: number) => {
    const clamped = Math.min(CURATED_EMOJI.length - 1, Math.max(0, index));
    setFocusIndex(clamped);
    refs.current[clamped]?.focus();
  };

  const onGridKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const deltas: Record<string, number> = { ArrowRight: 1, ArrowLeft: -1, ArrowDown: columns, ArrowUp: -columns };
    if (event.key in deltas) {
      event.preventDefault();
      move(focusIndex + deltas[event.key]);
    } else if (event.key === "Home") {
      event.preventDefault();
      move(0);
    } else if (event.key === "End") {
      event.preventDefault();
      move(CURATED_EMOJI.length - 1);
    }
  };

  const submitCustom = (event: FormEvent) => {
    event.preventDefault();
    // React events bubble through portals: keep this submit away from an enclosing form.
    event.stopPropagation();
    const emoji = normalizeEmojiInput(custom);
    if (!emoji) {
      setCustomError("Type or paste a single emoji.");
      return;
    }
    setCustomError(null);
    setCustom("");
    onChange(emoji);
  };

  return (
    <div className={cx(styles.picker, className)}>
      <div
        role="group"
        aria-label="Suggested icons"
        className={styles.grid}
        style={{ gridTemplateColumns: `repeat(${columns}, 32px)` }}
        onKeyDown={onGridKeyDown}
      >
        {CURATED_EMOJI.map((item, index) => (
          <button
            key={item.emoji}
            ref={(el) => {
              refs.current[index] = el;
            }}
            type="button"
            className={cx(styles.cell, item.emoji === value && styles.selected)}
            aria-label={item.label}
            aria-pressed={item.emoji === value}
            tabIndex={index === focusIndex ? 0 : -1}
            data-autofocus={index === focusIndex ? "true" : undefined}
            onFocus={() => setFocusIndex(index)}
            onClick={() => onChange(item.emoji)}
          >
            <span className="emoji" aria-hidden="true">
              {item.emoji}
            </span>
          </button>
        ))}
      </div>
      {/* A form so Enter in the field submits without bubbling to an enclosing dialog form. */}
      <form className={styles.custom} onSubmit={submitCustom} noValidate>
        <label htmlFor={inputId} className={styles.customLabel}>
          Any other emoji
        </label>
        <div className={styles.customRow}>
          <Input
            id={inputId}
            size="sm"
            value={custom}
            placeholder="Type or paste one"
            aria-invalid={customError ? true : undefined}
            aria-describedby={customError ? `${inputId}-error` : undefined}
            onChange={(event) => {
              setCustom(event.target.value);
              if (customError) setCustomError(null);
            }}
          />
          <Button type="submit" size="sm" variant="secondary" disabled={custom.trim() === ""}>
            Use
          </Button>
        </div>
        {customError && (
          <p id={`${inputId}-error`} className={styles.error}>
            {customError}
          </p>
        )}
      </form>
    </div>
  );
}
