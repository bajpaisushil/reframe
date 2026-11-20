import { CircleAlert } from "lucide-react";
import { Button, Icon, Kbd } from "@/components/ui";
import { cx } from "@/lib/cx";
import { primaryModifierLabel } from "@/lib/platform";
import type { ApiError } from "@/types";
import styles from "./UnsavedBar.module.css";

export interface UnsavedBarProps {
  dirty: boolean;
  saving: boolean;
  error: ApiError | null;
  onSave: () => void;
  onDiscard: () => void;
}

/**
 * Floating bar while the workspace has unsaved changes: Discard / Save, plus the reasons when a
 * save was refused.
 */
export function UnsavedBar({ dirty, saving, error, onSave, onDiscard }: UnsavedBarProps) {
  const problems = error ? (error.details.length > 0 ? error.details : [error.message]) : [];
  return (
    <div className={styles.dock}>
      <div className={cx(styles.bar, error && styles.withError)} role="region" aria-label="Unsaved changes">
        {error && (
          <div className={styles.error} role="alert">
            <Icon icon={CircleAlert} size={16} className={styles.errorIcon} />
            <div className={styles.errorText}>
              <p className={styles.errorTitle}>
                {problems.length > 1 ? "Couldn't save — fix these first:" : "Couldn't save the changes"}
              </p>
              {problems.length > 1 ? (
                <ul className={styles.errorList}>
                  {problems.map((problem) => (
                    <li key={problem}>{problem}</li>
                  ))}
                </ul>
              ) : (
                <p>{problems[0]}</p>
              )}
            </div>
          </div>
        )}
        <div className={styles.main}>
          <span className={styles.status}>
            <span className={styles.dot} aria-hidden="true" />
            {dirty ? "Unsaved changes" : "Changes not saved"}
          </span>
          <span className={styles.shortcut} aria-hidden="true">
            <Kbd>{primaryModifierLabel()}</Kbd>
            <Kbd>S</Kbd>
          </span>
          <Button variant="ghost" size="sm" onClick={onDiscard} disabled={saving || !dirty}>
            Discard
          </Button>
          <Button variant="primary" size="sm" onClick={onSave} loading={saving} disabled={!dirty}>
            Save changes
          </Button>
        </div>
      </div>
    </div>
  );
}
