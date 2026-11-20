import { useMemo, useState } from "react";
import { LayoutGrid, Plus, RefreshCw, Scan } from "lucide-react";
import { Page, PageHeader } from "@/components/layout/Page";
import { Button, EmptyState } from "@/components/ui";
import { useActiveRuns } from "@/hooks/useActiveRuns";
import { useNow } from "@/hooks/useNow";
import { useRestoreWorkspace } from "@/hooks/useRestoreWorkspace";
import { useActiveWorkspace, useWorkspaces } from "@/hooks/useWorkspaces";
import { errorMessage } from "@/lib/api";
import { greeting } from "@/lib/format";
import { navigate } from "@/lib/router";
import { sortByRecentUse } from "@/lib/workspaces";
import { ActiveWorkspaceCard } from "./ActiveWorkspaceCard";
import { FirstRunHero } from "./FirstRunHero";
import { LiveRestoreBanner } from "./LiveRestoreBanner";
import { NewWorkspaceDialog } from "./NewWorkspaceDialog";
import { RecentActivity } from "./RecentActivity";
import { RunningNow } from "./RunningNow";
import { WorkspaceTiles, WorkspaceTilesSkeleton } from "./WorkspaceTiles";
import styles from "./DashboardPage.module.css";

export const TAGLINE = "Your workspace, exactly where you left it.";

/** Home: greeting, live restores, the active workspace, recent workspaces, desktop and activity. */
export function DashboardPage() {
  const now = useNow();
  const workspaces = useWorkspaces();
  const active = useActiveWorkspace();
  const { running } = useActiveRuns();
  const { restore, pendingId } = useRestoreWorkspace();
  const [creating, setCreating] = useState(false);

  const list = useMemo(() => sortByRecentUse(workspaces.data ?? []), [workspaces.data]);
  const existingIds = useMemo(() => new Set(list.map((w) => w.id)), [list]);
  const restoringIds = useMemo(() => new Set(running.map((r) => r.workspaceId)), [running]);
  const activeWorkspace = active.workspace;
  const others = activeWorkspace ? list.filter((w) => w.id !== activeWorkspace.id) : list;
  const firstRun = workspaces.data !== undefined && list.length === 0;

  return (
    <Page>
      <PageHeader
        title={greeting(new Date(now))}
        subtitle={TAGLINE}
        actions={
          <>
            <Button variant="secondary" icon={Plus} onClick={() => setCreating(true)}>
              New workspace
            </Button>
            {/* On first run the hero's "Explore your desktop" is the one primary action. */}
            <Button
              variant={firstRun ? "secondary" : "primary"}
              icon={Scan}
              onClick={() => navigate({ name: "explore" })}
            >
              Capture desktop
            </Button>
          </>
        }
      />

      <div className={styles.sections}>
        <LiveRestoreBanner />

        {workspaces.data === undefined ? (
          workspaces.error ? (
            <EmptyState
              size="compact"
              tone="danger"
              icon={LayoutGrid}
              title="Couldn't load your workspaces"
              text={errorMessage(workspaces.error)}
              action={
                <Button size="sm" variant="secondary" icon={RefreshCw} onClick={() => void workspaces.refresh()}>
                  Try again
                </Button>
              }
            />
          ) : (
            <WorkspaceTilesSkeleton />
          )
        ) : firstRun ? (
          <FirstRunHero />
        ) : (
          <>
            {activeWorkspace && (
              <ActiveWorkspaceCard
                workspace={activeWorkspace}
                onRestore={() => void restore(activeWorkspace)}
                restoring={pendingId === activeWorkspace.id}
                inProgress={restoringIds.has(activeWorkspace.id)}
              />
            )}
            {others.length > 0 && (
              <WorkspaceTiles
                workspaces={others}
                total={list.length}
                onRestore={(workspace) => void restore(workspace)}
                pendingId={pendingId}
                restoringIds={restoringIds}
              />
            )}
          </>
        )}

        <div className={styles.columns}>
          <RunningNow />
          <RecentActivity existingWorkspaceIds={existingIds} />
        </div>
      </div>

      <NewWorkspaceDialog open={creating} onClose={() => setCreating(false)} />
    </Page>
  );
}
