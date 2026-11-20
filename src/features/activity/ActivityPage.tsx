import { useMemo, useState } from "react";
import { ChevronRight, CirclePause, CircleX, Clock, RefreshCw, RotateCcw } from "lucide-react";
import { Page, PageHeader } from "@/components/layout/Page";
import {
  Badge,
  Button,
  EmptyState,
  Icon,
  RunStatusPill,
  Skeleton,
  TabPanel,
  Tabs,
  WorkspaceIcon,
} from "@/components/ui";
import { useActiveRuns } from "@/hooks/useActiveRuns";
import { useActivity } from "@/hooks/useActivity";
import { useNow } from "@/hooks/useNow";
import { useQuery } from "@/hooks/useQuery";
import { useWorkspaces } from "@/hooks/useWorkspaces";
import { splitActivityMessage } from "@/lib/activity";
import { api, errorMessage } from "@/lib/api";
import { cx } from "@/lib/cx";
import { formatAbsoluteTime, formatDuration } from "@/lib/format";
import { queryKeys } from "@/lib/queryClient";
import { navigate } from "@/lib/router";
import { ACTIVITY_KIND } from "@/features/dashboard/RecentActivity";
import { MODE_LABEL } from "@/features/restoration/modes";
import { isInterruptedRun } from "@/features/restoration/runState";
import type { ActivityEntry, RestoreRun, WorkspaceSummary } from "@/types";
import { formatTimeOfDay, groupByDay, runAppsLabel, runDurationMs } from "./activityModel";
import styles from "./ActivityPage.module.css";

type Tab = "activity" | "restores";

const ACTIVITY_LIMIT = 200;
const HISTORY_LIMIT = 50;

/** Remembered while the app runs, so coming back shows the same tab. */
let lastTab: Tab = "activity";

/** Everything Reframe has done (route `activity`): the activity feed and past restores. */
export function ActivityPage() {
  const [tab, setTabState] = useState<Tab>(lastTab);
  const setTab = (next: Tab) => {
    lastTab = next;
    setTabState(next);
  };
  const activity = useActivity(ACTIVITY_LIMIT);
  const history = useQuery(`${queryKeys.restoreHistory}:${HISTORY_LIMIT}`, () => api.restoreHistory(null, HISTORY_LIMIT), {
    staleTime: 10_000,
    refetchOnFocus: true,
  });
  const workspaces = useWorkspaces();
  const byId = useMemo(() => new Map((workspaces.data ?? []).map((w) => [w.id, w])), [workspaces.data]);

  return (
    <Page>
      <PageHeader title="Activity" subtitle="Everything Reframe has done, newest first." />
      <Tabs
        aria-label="Activity"
        value={tab}
        onChange={setTab}
        items={[
          { value: "activity", label: "Activity", count: activity.data?.length },
          { value: "restores", label: "Restores", count: history.data?.length },
        ]}
      >
        <TabPanel value="activity" className={styles.panel}>
          {activity.data ? (
            activity.data.length > 0 ? (
              <ActivityFeed entries={activity.data} workspaces={byId} />
            ) : (
              <EmptyState
                icon={Clock}
                title="No activity yet"
                text="Captures, restores and changes to your workspaces will show up here."
              />
            )
          ) : activity.error ? (
            <LoadError title="Couldn't load activity" error={activity.error} onRetry={() => void activity.refresh()} />
          ) : (
            <ListSkeleton />
          )}
        </TabPanel>
        <TabPanel value="restores" className={styles.panel}>
          {history.data ? (
            history.data.length > 0 ? (
              <RestoreHistory runs={history.data} workspaces={byId} />
            ) : (
              <EmptyState
                icon={RotateCcw}
                title="No restores yet"
                text="Each time you restore a workspace, you'll find how it went here."
              />
            )
          ) : history.error ? (
            <LoadError title="Couldn't load past restores" error={history.error} onRetry={() => void history.refresh()} />
          ) : (
            <ListSkeleton />
          )}
        </TabPanel>
      </Tabs>
    </Page>
  );
}

// ---------------------------------------------------------------------------------------------
// Activity feed
// ---------------------------------------------------------------------------------------------

function ActivityFeed({ entries, workspaces }: { entries: readonly ActivityEntry[]; workspaces: ReadonlyMap<string, WorkspaceSummary> }) {
  const now = useNow();
  const groups = useMemo(() => groupByDay(entries, (e) => e.createdAt, new Date(now)), [entries, now]);
  return (
    <div className={styles.groups}>
      {groups.map((group) => (
        <section key={group.key} aria-label={group.label} className={styles.group}>
          <h2 className={styles.day}>{group.label}</h2>
          <ul className={styles.list}>
            {group.items.map((entry) => (
              <ActivityRow
                key={entry.id}
                entry={entry}
                linked={entry.workspaceId !== null && workspaces.has(entry.workspaceId)}
              />
            ))}
          </ul>
        </section>
      ))}
    </div>
  );
}

function ActivityRow({ entry, linked }: { entry: ActivityEntry; linked: boolean }) {
  const kind = ACTIVITY_KIND[entry.kind] ?? ACTIVITY_KIND.updated;
  const parts = splitActivityMessage(entry.message, entry.workspaceName);
  const workspace = (
    <>
      {entry.workspaceIcon && (
        <span className="emoji" aria-hidden="true">
          {entry.workspaceIcon}
        </span>
      )}
      <span>{entry.workspaceName}</span>
    </>
  );
  const content = (
    <>
      <span className={cx(styles.kind, styles[kind.tone])} aria-hidden="true">
        <Icon icon={kind.icon} size={14} />
      </span>
      <span className={styles.body}>
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
        {!parts && entry.workspaceName && <span className={styles.workspaceLine}>{workspace}</span>}
      </span>
      <time className={styles.time} dateTime={entry.createdAt} title={formatAbsoluteTime(entry.createdAt)}>
        {formatTimeOfDay(entry.createdAt)}
      </time>
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

// ---------------------------------------------------------------------------------------------
// Past restores
// ---------------------------------------------------------------------------------------------

function RestoreHistory({ runs, workspaces }: { runs: readonly RestoreRun[]; workspaces: ReadonlyMap<string, WorkspaceSummary> }) {
  const now = useNow();
  const { running, ready, error } = useActiveRuns();
  const activeIds = useMemo(() => new Set(running.map((r) => r.id)), [running]);
  const groups = useMemo(() => groupByDay(runs, (r) => r.startedAt, new Date(now)), [runs, now]);
  return (
    <div className={styles.groups}>
      {groups.map((group) => (
        <section key={group.key} aria-label={group.label} className={styles.group}>
          <h2 className={styles.day}>{group.label}</h2>
          <ul className={styles.list}>
            {group.items.map((run) => (
              <RunRow
                key={run.id}
                run={run}
                workspace={workspaces.get(run.workspaceId) ?? null}
                interrupted={isInterruptedRun(run, activeIds, ready && !error, now)}
              />
            ))}
          </ul>
        </section>
      ))}
    </div>
  );
}

function RunRow({ run, workspace, interrupted }: { run: RestoreRun; workspace: WorkspaceSummary | null; interrupted: boolean }) {
  const duration = runDurationMs(run);
  const facts = [runAppsLabel(run), MODE_LABEL[run.mode], duration !== null ? formatDuration(duration) : null].filter(
    (f): f is string => !!f,
  );
  return (
    <li>
      <button
        type="button"
        className={cx(styles.row, styles.linked, styles.runRow)}
        onClick={() => navigate({ name: "restore", runId: run.id })}
        aria-label={`${run.workspaceName}, ${interrupted ? "interrupted" : run.status}, ${facts.join(", ")}. View details`}
      >
        <WorkspaceIcon emoji={workspace?.icon ?? ""} color={workspace?.color ?? null} size={28} />
        <span className={styles.body}>
          <span className={styles.runName}>{run.workspaceName}</span>
          <span className={styles.runFacts}>{facts.join(" · ")}</span>
        </span>
        {interrupted ? (
          <Badge tone="neutral" icon={CirclePause}>
            Interrupted
          </Badge>
        ) : (
          <RunStatusPill status={run.status} />
        )}
        <time className={styles.time} dateTime={run.startedAt} title={formatAbsoluteTime(run.startedAt)}>
          {formatTimeOfDay(run.startedAt)}
        </time>
        <Icon icon={ChevronRight} size={16} className={styles.chevron} />
      </button>
    </li>
  );
}

// ---------------------------------------------------------------------------------------------
// States
// ---------------------------------------------------------------------------------------------

function LoadError({ title, error, onRetry }: { title: string; error: unknown; onRetry: () => void }) {
  return (
    <EmptyState
      icon={CircleX}
      tone="danger"
      title={title}
      text={errorMessage(error)}
      action={
        <Button variant="secondary" icon={RefreshCw} onClick={onRetry}>
          Try again
        </Button>
      }
    />
  );
}

function ListSkeleton() {
  return (
    <div className={styles.groups} aria-hidden="true">
      <div className={styles.group}>
        <Skeleton width={72} height={12} text />
        <ul className={styles.list}>
          {[58, 42, 66, 50].map((width, index) => (
            <li key={index} className={styles.row}>
              <Skeleton width={28} height={28} radius={14} />
              <span className={styles.body}>
                <Skeleton width={`${width}%`} height={12} text />
              </span>
              <Skeleton width={48} height={10} text />
            </li>
          ))}
        </ul>
      </div>
    </div>
  );
}
