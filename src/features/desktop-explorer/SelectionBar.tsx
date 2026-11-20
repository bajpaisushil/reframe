import { Button } from "@/components/ui";
import { formatCounts, type SelectionCounts } from "./selection";
import styles from "./SelectionBar.module.css";

export interface SelectionBarProps {
  counts: SelectionCounts;
  onSave: () => void;
  onClear: () => void;
}

/** Floats at the bottom of the page while something is selected. */
export function SelectionBar({ counts, onSave, onClear }: SelectionBarProps) {
  return (
    <div className={styles.dock}>
      <div className={styles.bar} role="region" aria-label="Selection">
        <span className={styles.count} aria-live="polite">
          {formatCounts(counts)} selected
        </span>
        <span className={styles.actions}>
          <Button variant="ghost" size="sm" onClick={onClear}>
            Clear
          </Button>
          <Button variant="primary" size="sm" onClick={onSave}>
            Save as workspace…
          </Button>
        </span>
      </div>
    </div>
  );
}
