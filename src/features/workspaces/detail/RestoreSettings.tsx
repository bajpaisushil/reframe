import { SectionHeader, SegmentedControl, Switch, type SegmentOption } from "@/components/ui";
import type { RestoreConfiguration, SwitchMode } from "@/types";
import { NumberField } from "../components/NumberField";
import styles from "./RestoreSettings.module.css";

export const MODE_OPTIONS: readonly SegmentOption<SwitchMode>[] = [
  { value: "replace", label: "Replace" },
  { value: "merge", label: "Merge" },
  { value: "additive", label: "Additive" },
];

export const MODE_EXPLANATIONS: Record<SwitchMode, string> = {
  replace: "Closes apps that belonged only to your previous workspace — after you confirm which — then opens what's missing.",
  merge: "Keeps everything that's open, reuses apps that are already running and opens what's missing.",
  additive: "Opens every app in this workspace as new windows, without closing or reusing anything.",
};

export interface RestoreSettingsProps {
  restore: RestoreConfiguration;
  onChange: (patch: Partial<RestoreConfiguration>) => void;
}

/** How this workspace restores: the default switch mode and launch timing. */
export function RestoreSettings({ restore, onChange }: RestoreSettingsProps) {
  return (
    <section aria-labelledby="restore-settings-title" className={styles.section}>
      <SectionHeader id="restore-settings-title" title="Restore settings" />
      <div className={styles.panel}>
        <div className={styles.mode}>
          <div className={styles.modeText}>
            <p className={styles.label} id="restore-mode-label">
              When restoring
            </p>
            <p className={styles.hint}>The mode chosen for you each time; you can still pick another.</p>
          </div>
          <SegmentedControl
            aria-label="Default restore mode"
            value={restore.defaultMode}
            options={MODE_OPTIONS}
            onChange={(defaultMode) => onChange({ defaultMode })}
            size="sm"
          />
          <p className={styles.modeExplanation} aria-live="polite">
            <span className={styles.modeName}>{MODE_OPTIONS.find((o) => o.value === restore.defaultMode)?.label}:</span>{" "}
            {MODE_EXPLANATIONS[restore.defaultMode]}
          </p>
        </div>
        <NumberField
          className={styles.row}
          label="Pause between apps"
          hint="Time between starting one app and the next."
          unit="seconds"
          range={{ min: 0, max: 60 }}
          value={restore.launchDelayMs}
          onChange={(value) => onChange({ launchDelayMs: value ?? 0 })}
        />
        <NumberField
          className={styles.row}
          label="Wait for windows"
          hint="How long to wait for an app's windows before moving on."
          unit="seconds"
          range={{ min: 1, max: 300 }}
          value={restore.timeoutMs}
          onChange={(value) => onChange({ timeoutMs: value ?? restore.timeoutMs })}
        />
        <NumberField
          className={styles.row}
          label="Retries"
          hint="Extra attempts when an app doesn't start."
          unit="count"
          range={{ min: 0, max: 5 }}
          value={restore.retryCount}
          onChange={(value) => onChange({ retryCount: value ?? 0 })}
        />
        <NumberField
          className={styles.row}
          label="Apps started at once"
          hint="Apps that don't wait for each other can start together."
          unit="count"
          range={{ min: 1, max: 8 }}
          value={restore.maxParallel}
          onChange={(value) => onChange({ maxParallel: value ?? 1 })}
        />
        <div className={styles.row}>
          <Switch
            className={styles.switchRow}
            checked={restore.restoreWindowPositions}
            onChange={(restoreWindowPositions) => onChange({ restoreWindowPositions })}
            label="Restore window positions"
            description="Put windows back at their saved size and place, adapted to the displays you have now."
          />
        </div>
      </div>
    </section>
  );
}
