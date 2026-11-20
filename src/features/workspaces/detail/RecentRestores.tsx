import { ChevronRight, History, RefreshCw } from "lucide-react";
import { Button, EmptyState, Icon, RelativeTime, RunStatusPill, SectionHeader, Skeleton } from "@/components/ui";
import { errorMessage } from "@/lib/api";
import { formatDuration } from "@/lib/format";
import { Link } from "@/lib/router";
import type { RestoreRun, SwitchMode } from "@/types";
import { useRestoreHistory } from "../hooks/queries";
import styles from "./RecentRestores.module.css";

export const RECENT_RESTORES_LIMIT = 5;

const MODE_LABEL: Record<SwitchMode, string> = { replace: "Replace", merge: "Merge", additive: "Additive" };

/** "3 of 4 apps" — counted like the activity feed (apps that were turned off don't count). */
export function runOutcome(run: RestoreRun): string {
  const attempted = run.entries.filter((e) => e.message !== "Disabled");
  const restored = attempted.filter((e) => e.status === "completed" || e.status === "partiallyCompleted").length;
  if (attempted.length === 0) return "No apps";
  return `${restored} of ${attempted.length} ${attempted.length === 1 ? "app" : "apps"}`;
}

function runDuration(run: RestoreRun): string {
  if (!run.finishedAt) return "";
  const ms = Date.parse(run.finishedAt) - Date.parse(run.startedAt);
  return Number.isFinite(ms) && ms >= 0 ? formatDuration(ms) : "";
}

/** The workspace's last few restores, each linking to its full report. */
export function RecentRestores({ workspaceId }: { workspaceId: string }) {
  const history = useRestoreHistory(workspaceId, RECENT_RESTORES_LIMIT);
  const runs = history.data;

  return (
    <section aria-labelledby="recent-restores-title" className={styles.section}>
      <SectionHeader id="recent-restores-title" title="Recent restores" />
      <div className={styles.panel}>
        {runs === undefined ? (
          history.error ? (
            <EmptyState
              className={styles.centered}
              size="compact"
              tone="danger"
              icon={History}
              title="Couldn't load restores"
              text={errorMessage(history.error)}
              action={
                <Button size="sm" variant="secondary" icon={RefreshCw} onClick={() => void history.refresh()}>
                  Try again
                </Button>
              }
            />
          ) : (
            <ul className={styles.list} aria-hidden="true">
              {Array.from({ length: 3 }, (_, i) => (
                <li key={i} className={styles.skeletonRow}>
                  <Skeleton width={92} height={20} radius={999} />
                  <Skeleton width="40%" height={10} text />
                </li>
              ))}
            </ul>
          )
        ) : runs.length === 0 ? (
          <EmptyState
            className={styles.centered}
            size="compact"
            icon={History}
            title="Not restored yet"
            text="Each time you restore this workspace, the result shows up here."
          />
        ) : (
          <ul className={styles.list}>
            {runs.map((run) => {
              const duration = runDuration(run);
              return (
                <li key={run.id}>
                  <Link to={{ name: "restore", runId: run.id }} className={styles.row}>
                    <RunStatusPill status={run.status} className={styles.status} />
                    <span className={styles.text}>
                      <span className={styles.primary}>
                        {runOutcome(run)}
                        <span className={styles.mode}>{MODE_LABEL[run.mode] ?? run.mode}</span>
                      </span>
                      <span className={styles.secondary}>
                        <RelativeTime value={run.startedAt} />
                        {duration && <> · took {duration}</>}
                      </span>
                    </span>
                    <Icon icon={ChevronRight} size={16} className={styles.chevron} />
                  </Link>
                </li>
              );
            })}
          </ul>
        )}
      </div>
    </section>
  );
}
