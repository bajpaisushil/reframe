import { ArrowUpRight, RotateCcw } from "lucide-react";
import { AppIconStack, Button, RelativeTime, WorkspaceIcon } from "@/components/ui";
import { pluralize } from "@/lib/format";
import { navigate } from "@/lib/router";
import { summaryApps } from "@/lib/workspaces";
import type { WorkspaceSummary } from "@/types";
import styles from "./ActiveWorkspaceCard.module.css";

export interface ActiveWorkspaceCardProps {
  workspace: WorkspaceSummary;
  onRestore: () => void;
  restoring: boolean;
  /** A restore of this workspace is already running. */
  inProgress: boolean;
}

/** The workspace restored most recently, with quick Restore / Open actions. */
export function ActiveWorkspaceCard({ workspace, onRestore, restoring, inProgress }: ActiveWorkspaceCardProps) {
  const empty = workspace.applicationCount === 0;
  return (
    <section className={styles.card} aria-labelledby="active-workspace-name">
      <WorkspaceIcon emoji={workspace.icon} color={workspace.color} size={56} />
      <div className={styles.body}>
        <p className="overline">Active workspace</p>
        <h2 id="active-workspace-name" className={styles.name}>
          {workspace.name}
        </h2>
        <div className={styles.meta}>
          {!empty && <AppIconStack apps={summaryApps(workspace)} total={workspace.applicationCount} size={20} showNames />}
          <span className={styles.facts}>
            <span>{empty ? "No apps yet" : pluralize(workspace.applicationCount, "app")}</span>
            <span aria-hidden="true" className={styles.dot}>
              ·
            </span>
            <RelativeTime value={workspace.lastRestoredAt} prefix="Restored " fallback="Never restored" />
          </span>
        </div>
      </div>
      <div className={styles.actions}>
        <Button
          variant="secondary"
          icon={RotateCcw}
          loading={restoring}
          disabled={empty && !inProgress}
          onClick={onRestore}
        >
          {inProgress ? "View restore" : "Restore"}
        </Button>
        <Button variant="ghost" iconEnd={ArrowUpRight} onClick={() => navigate({ name: "workspace", id: workspace.id })}>
          Open
        </Button>
      </div>
    </section>
  );
}
