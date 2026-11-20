import {
  CircleCheck,
  CircleExclamationPoint,
  CircleX,
  FileDown,
  FileUp,
  Clock,
  Pencil,
  Plus,
  RefreshCw,
  Scan,
  Trash,
} from "lucide-react";
import {
  Button,
  EmptyState,
  Icon,
  RelativeTime,
  SectionHeader,
  SectionLink,
  Skeleton,
  type LucideIcon,
} from "@/components/ui";
import { useActivity } from "@/hooks/useActivity";
import { splitActivityMessage } from "@/lib/activity";
import { errorMessage } from "@/lib/api";
import { cx } from "@/lib/cx";
import { navigate } from "@/lib/router";
import type { ActivityEntry, ActivityKind } from "@/types";
import styles from "./RecentActivity.module.css";

export const RECENT_ACTIVITY_LIMIT = 6;

type Tone = "accent" | "success" | "warning" | "danger" | "neutral";

/** Icon and tone for each kind of activity (exported for the full activity page). */
export const ACTIVITY_KIND: Record<ActivityKind, { icon: LucideIcon; tone: Tone; label: string }> = {
  created: { icon: Plus, tone: "accent", label: "Created" },
  captured: { icon: Scan, tone: "accent", label: "Captured" },
  updated: { icon: Pencil, tone: "neutral", label: "Edited" },
  deleted: { icon: Trash, tone: "neutral", label: "Deleted" },
  restored: { icon: CircleCheck, tone: "success", label: "Restored" },
  restorePartial: { icon: CircleExclamationPoint, tone: "warning", label: "Partly restored" },
  restoreFailed: { icon: CircleX, tone: "danger", label: "Restore failed" },
  imported: { icon: FileDown, tone: "neutral", label: "Imported" },
  exported: { icon: FileUp, tone: "neutral", label: "Exported" },
};

export interface RecentActivityProps {
  /** Ids of workspaces that still exist (rows for deleted ones are not links). */
  existingWorkspaceIds: ReadonlySet<string>;
}

/** The latest few activity entries. */
export function RecentActivity({ existingWorkspaceIds }: RecentActivityProps) {
  const activity = useActivity(RECENT_ACTIVITY_LIMIT);

  return (
    <section aria-labelledby="dashboard-activity" className={styles.section}>
      <SectionHeader
        id="dashboard-activity"
        title="Recent activity"
        action={<SectionLink to={{ name: "activity" }}>View all</SectionLink>}
      />
      <div className={styles.panel}>
        {activity.data ? (
          activity.data.length > 0 ? (
            <ul className={styles.list}>
              {activity.data.map((entry) => (
                <ActivityRow
                  key={entry.id}
                  entry={entry}
                  linked={entry.workspaceId !== null && existingWorkspaceIds.has(entry.workspaceId)}
                />
              ))}
            </ul>
          ) : (
            <EmptyState
              className={styles.centered}
              size="compact"
              icon={Clock}
              title="No activity yet"
              text="Captures, restores and changes to your workspaces will show up here."
            />
          )
        ) : activity.error ? (
          <EmptyState
            className={styles.centered}
            size="compact"
            tone="danger"
            icon={CircleX}
            title="Couldn't load recent activity"
            text={errorMessage(activity.error)}
            action={
              <Button size="sm" variant="secondary" icon={RefreshCw} onClick={() => void activity.refresh()}>
                Try again
              </Button>
            }
          />
        ) : (
          <ActivitySkeleton />
        )}
      </div>
    </section>
  );
}

function ActivityRow({ entry, linked }: { entry: ActivityEntry; linked: boolean }) {
  const kind = ACTIVITY_KIND[entry.kind] ?? ACTIVITY_KIND.updated;
  // Show the workspace once: inline when the message names it, otherwise on a second line.
  const parts = splitActivityMessage(entry.message, entry.workspaceName);
  const workspace = (
    <>
      {entry.workspaceIcon && (
        <span className="emoji" aria-hidden="true">
          {entry.workspaceIcon}
        </span>
      )}
      <span className={styles.workspaceName}>{entry.workspaceName}</span>
    </>
  );
  const content = (
    <>
      <span className={cx(styles.kind, styles[kind.tone])} aria-hidden="true">
        <Icon icon={kind.icon} size={14} />
      </span>
      <span className={styles.body}>
        <span className={styles.line}>
          <span className={cx(styles.message, parts && styles.messageSplit)}>
            {parts ? (
              <>
                {parts.before}
                <span className={styles.inlineWorkspace}>{workspace}</span>
                {parts.after}
              </>
            ) : (
              entry.message
            )}
          </span>
          <RelativeTime className={styles.time} value={entry.createdAt} />
        </span>
        {!parts && entry.workspaceName && <span className={styles.workspace}>{workspace}</span>}
      </span>
    </>
  );
  return (
    <li>
      {linked && entry.workspaceId ? (
        <button
          type="button"
          className={cx(styles.row, styles.linked)}
          onClick={() => entry.workspaceId && navigate({ name: "workspace", id: entry.workspaceId })}
        >
          {content}
        </button>
      ) : (
        <div className={styles.row}>{content}</div>
      )}
    </li>
  );
}

function ActivitySkeleton() {
  return (
    <ul className={styles.list} aria-hidden="true">
      {Array.from({ length: 4 }, (_, i) => (
        <li key={i} className={styles.row}>
          <Skeleton width={28} height={28} radius={14} />
          <span className={styles.body}>
            <Skeleton width={`${50 + ((i * 13) % 30)}%`} height={12} text />
            <Skeleton width={90} height={10} text style={{ marginTop: 6 }} />
          </span>
        </li>
      ))}
    </ul>
  );
}
