import { useState } from "react";
import { Trash } from "lucide-react";
import { Button, TabPanel, Tabs, type TabItem } from "@/components/ui";
import type { ApplicationEntry, EntryHealth, MonitorInfo, RestoreConfiguration } from "@/types";
import { stateSummary } from "../../model/entries";
import { ContentTab } from "./ContentTab";
import { LaunchTab } from "./LaunchTab";
import { TimingTab } from "./TimingTab";
import { WindowsTab } from "./WindowsTab";
import styles from "./editor.module.css";

type EditorTab = "content" | "launch" | "timing" | "windows";

export interface ApplicationEditorProps {
  entry: ApplicationEntry;
  entries: readonly ApplicationEntry[];
  monitors: readonly MonitorInfo[];
  restore: RestoreConfiguration;
  health: EntryHealth | undefined;
  onUpdate: (update: (entry: ApplicationEntry) => ApplicationEntry) => void;
  onRemove: () => void;
}

/** Everything about one app in the workspace, in tabs: what it reopens, launch, order, windows. */
export function ApplicationEditor({ entry, entries, monitors, restore, health, onUpdate, onRemove }: ApplicationEditorProps) {
  const content = stateSummary(entry.state);
  const [tab, setTab] = useState<EditorTab>(content ? "content" : "launch");
  const current: EditorTab = tab === "content" && !content ? "launch" : tab;

  const items: TabItem<EditorTab>[] = [
    ...(content ? [{ value: "content" as const, label: content.label, count: content.count ?? undefined }] : []),
    { value: "launch", label: "Launch" },
    { value: "timing", label: "Order & timing" },
    { value: "windows", label: "Windows", count: entry.windows.length },
  ];

  return (
    <div className={styles.editor}>
      <Tabs value={current} onChange={setTab} items={items} aria-label={`${entry.displayName} settings`}>
        {content && (
          <TabPanel value="content">
            <div className={styles.tabContent}>
              <ContentTab entry={entry} health={health} onStateChange={(state) => onUpdate((e) => ({ ...e, state }))} />
            </div>
          </TabPanel>
        )}
        <TabPanel value="launch">
          <div className={styles.tabContent}>
            <LaunchTab entry={entry} health={health} onUpdate={onUpdate} />
          </div>
        </TabPanel>
        <TabPanel value="timing">
          <div className={styles.tabContent}>
            <TimingTab entry={entry} entries={entries} restore={restore} onUpdate={onUpdate} />
          </div>
        </TabPanel>
        <TabPanel value="windows">
          <div className={styles.tabContent}>
            <WindowsTab entry={entry} monitors={monitors} onUpdate={onUpdate} />
          </div>
        </TabPanel>
      </Tabs>
      <div className={styles.footer}>
        <p className={styles.footnote}>Changes apply the next time this workspace is restored.</p>
        <Button variant="ghost" size="sm" icon={Trash} className={styles.remove} onClick={onRemove}>
          Remove from workspace
        </Button>
      </div>
    </div>
  );
}
