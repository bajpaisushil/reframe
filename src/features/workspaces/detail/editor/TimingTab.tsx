import { AppIcon, Checkbox, Switch } from "@/components/ui";
import { cx } from "@/lib/cx";
import { pluralize } from "@/lib/format";
import type { ApplicationEntry, EntryOptions, RestoreConfiguration } from "@/types";
import { NumberField } from "../../components/NumberField";
import { dependencyOptions, toggleDependency } from "../../model/dependencies";
import { formatSeconds, secondsLabel } from "../../model/validation";
import styles from "./editor.module.css";

export interface TimingTabProps {
  entry: ApplicationEntry;
  entries: readonly ApplicationEntry[];
  restore: RestoreConfiguration;
  onUpdate: (update: (entry: ApplicationEntry) => ApplicationEntry) => void;
}

/** What the app waits for, and per-app overrides of the workspace's timing. */
export function TimingTab({ entry, entries, restore, onUpdate }: TimingTabProps) {
  const options = dependencyOptions(entries, entry.id);
  const name = entry.displayName;
  const setOptions = (patch: Partial<EntryOptions>) => onUpdate((current) => ({ ...current, options: { ...current.options, ...patch } }));
  const selectedCount = entry.dependsOn.length;

  return (
    <div className={styles.panel}>
      <div className={styles.group}>
        <div className={styles.groupHeader}>
          <h4 className={styles.groupTitle}>Starts after</h4>
          <span className={styles.groupMeta}>{selectedCount > 0 ? `${pluralize(selectedCount, "app")} selected` : "Starts in list order"}</span>
        </div>
        <p className={styles.hint}>{name} waits until the apps you pick here are open before it starts.</p>
        {options.length === 0 ? (
          <div className={styles.empty}>Add more apps to this workspace to make {name} wait for them.</div>
        ) : (
          <div className={styles.dependencyList} role="group" aria-label={`Apps ${name} starts after`}>
            {options.map((option) => (
              <Checkbox
                key={option.entry.id}
                checked={option.selected}
                disabled={option.disabledReason !== null}
                className={cx(styles.dependency, option.disabledReason !== null && styles.dependencyDisabled)}
                onChange={(checked) =>
                  onUpdate((current) => ({ ...current, dependsOn: toggleDependency(entries, current.id, option.entry.id, checked) }))
                }
                label={
                  <span className={styles.dependencyName}>
                    <AppIcon
                      lookup={{
                        appId: option.entry.appId,
                        executablePath: option.entry.launch.executable,
                        package: option.entry.launch.package,
                      }}
                      name={option.entry.displayName}
                      size={20}
                    />
                    <span className={styles.dependencyLabel}>{option.entry.displayName}</span>
                  </span>
                }
                description={option.disabledReason ?? undefined}
              />
            ))}
          </div>
        )}
      </div>

      <div className={styles.group}>
        <h4 className={styles.groupTitle}>Timing</h4>
        <div className={styles.settings}>
          <NumberField
            className={styles.setting}
            label="Wait after it opens"
            hint="An extra pause before the apps that start after this one."
            unit="seconds"
            range={{ min: 0, max: 120, optional: true }}
            value={entry.options.delayAfterLaunchMs}
            placeholder="0"
            onChange={(delayAfterLaunchMs) => setOptions({ delayAfterLaunchMs })}
          />
          <NumberField
            className={styles.setting}
            label="Wait for its windows"
            hint={`How long to wait for ${name}'s windows. Empty uses the workspace setting (${secondsLabel(restore.timeoutMs)}).`}
            unit="seconds"
            range={{ min: 1, max: 300, optional: true }}
            value={entry.options.timeoutMs}
            placeholder={formatSeconds(restore.timeoutMs)}
            onChange={(timeoutMs) => setOptions({ timeoutMs })}
          />
          <NumberField
            className={styles.setting}
            label="Retries"
            hint={`Extra attempts if ${name} fails to start. Empty uses the workspace setting (${restore.retryCount}).`}
            unit="count"
            range={{ min: 0, max: 5, optional: true }}
            value={entry.options.retryCount}
            placeholder={String(restore.retryCount)}
            onChange={(retryCount) => setOptions({ retryCount })}
          />
          <div className={styles.setting}>
            <Switch
              className={styles.switchRow}
              checked={entry.options.restoreWindowPositions}
              onChange={(restoreWindowPositions) => setOptions({ restoreWindowPositions })}
              disabled={!restore.restoreWindowPositions}
              label="Restore window positions"
              description={
                restore.restoreWindowPositions
                  ? `Put ${name}'s windows back where they were saved.`
                  : "Turned off for the whole workspace in Restore settings."
              }
            />
          </div>
        </div>
      </div>
    </div>
  );
}
