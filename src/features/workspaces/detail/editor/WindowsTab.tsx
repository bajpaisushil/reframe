import { AppWindow, X } from "lucide-react";
import { Icon, IconButton } from "@/components/ui";
import type { ApplicationEntry, MonitorInfo } from "@/types";
import { monitorName, removeAt, windowStateLabel } from "../../model/entries";
import styles from "./editor.module.css";

export interface WindowsTabProps {
  entry: ApplicationEntry;
  monitors: readonly MonitorInfo[];
  onUpdate: (update: (entry: ApplicationEntry) => ApplicationEntry) => void;
}

/** The window arrangement Reframe puts back: one saved position per window. */
export function WindowsTab({ entry, monitors, onUpdate }: WindowsTabProps) {
  const name = entry.displayName;
  if (entry.windows.length === 0) {
    return (
      <div className={styles.empty}>
        No saved windows. Reframe starts {name} and leaves its windows wherever the app opens them.
      </div>
    );
  }
  return (
    <div className={styles.group}>
      <p className={styles.hint}>
        {entry.options.restoreWindowPositions
          ? "Reframe moves each window back to its saved size and place. Removing one here only forgets its position."
          : `Window positions are turned off for ${name} (see Order & timing), so these are kept but not used.`}
      </p>
      <ul className={styles.list}>
        {entry.windows.map((window, index) => {
          const display = monitorName(monitors, window.monitorId);
          const title = window.title.trim() || "Untitled window";
          return (
            <li key={`${window.title}-${index}`} className={styles.listRow}>
              <Icon icon={AppWindow} size={16} className={styles.listIcon} />
              <div className={styles.listText}>
                <span className={styles.listTitle}>{title}</span>
                <span className={styles.listMeta}>
                  <span className="tabular">
                    {window.bounds.width} × {window.bounds.height}
                  </span>
                  {" · "}
                  {display ?? "Unknown display"}
                  {" · "}
                  {windowStateLabel(window.state)}
                </span>
              </div>
              <IconButton
                icon={X}
                size="sm"
                aria-label={`Forget the position of ${title}`}
                className={styles.removeButton}
                onClick={() => onUpdate((current) => ({ ...current, windows: removeAt(current.windows, index) }))}
              />
            </li>
          );
        })}
      </ul>
    </div>
  );
}
