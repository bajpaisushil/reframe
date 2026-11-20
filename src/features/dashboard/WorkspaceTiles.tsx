import { RotateCcw } from "lucide-react";
import { AppIconStack, Button, RelativeTime, SectionHeader, SectionLink, Skeleton, WorkspaceIcon } from "@/components/ui";
import { cx } from "@/lib/cx";
import { navigate } from "@/lib/router";
import { summaryApps } from "@/lib/workspaces";
import type { WorkspaceSummary } from "@/types";
import styles from "./WorkspaceTiles.module.css";

export const MAX_TILES = 6;

export interface WorkspaceTilesProps {
  workspaces: readonly WorkspaceSummary[];
  /** Total saved workspaces (for the count and "View all"). */
  total: number;
  onRestore: (workspace: WorkspaceSummary) => void;
  pendingId: string | null;
  /** Ids of workspaces with a restore in progress. */
  restoringIds: ReadonlySet<string>;
}

/** Up to six recently used workspaces as quiet tiles, plus "View all". */
export function WorkspaceTiles({ workspaces, total, onRestore, pendingId, restoringIds }: WorkspaceTilesProps) {
  return (
    <section aria-labelledby="dashboard-workspaces">
      <SectionHeader
        id="dashboard-workspaces"
        title="Workspaces"
        count={total}
        action={<SectionLink to={{ name: "workspaces" }}>View all</SectionLink>}
      />
      <ul className={styles.grid}>
        {workspaces.slice(0, MAX_TILES).map((workspace) => (
          <li key={workspace.id}>
            <WorkspaceTile
              workspace={workspace}
              onRestore={() => onRestore(workspace)}
              starting={pendingId === workspace.id}
              inProgress={restoringIds.has(workspace.id)}
            />
          </li>
        ))}
      </ul>
    </section>
  );
}

function WorkspaceTile({
  workspace,
  onRestore,
  starting,
  inProgress,
}: {
  workspace: WorkspaceSummary;
  onRestore: () => void;
  starting: boolean;
  inProgress: boolean;
}) {
  const empty = workspace.applicationCount === 0;
  return (
    <div className={cx(styles.tile, (starting || inProgress) && styles.busy)}>
      {/* The whole tile opens the workspace; Restore is a sibling, never nested in it. */}
      <button
        type="button"
        className={styles.open}
        onClick={() => navigate({ name: "workspace", id: workspace.id })}
      >
        <WorkspaceIcon emoji={workspace.icon} color={workspace.color} size={36} />
        <span className={styles.text}>
          <span className={styles.name}>{workspace.name}</span>
          <span className={styles.when}>
            {inProgress ? (
              "Restoring now…"
            ) : (
              <RelativeTime value={workspace.lastRestoredAt} prefix="Restored " fallback="Never restored" />
            )}
          </span>
        </span>
        <span className={styles.apps}>
          {empty ? (
            <span className={styles.noApps}>No apps yet</span>
          ) : (
            <AppIconStack apps={summaryApps(workspace)} total={workspace.applicationCount} max={6} size={20} />
          )}
        </span>
      </button>
      {!empty && (
        <Button
          size="sm"
          variant="secondary"
          icon={RotateCcw}
          loading={starting}
          className={styles.restore}
          onClick={onRestore}
          aria-label={inProgress ? `View restore of ${workspace.name}` : `Restore ${workspace.name}`}
        >
          {inProgress ? "View" : "Restore"}
        </Button>
      )}
    </div>
  );
}

/** Placeholder grid while workspaces load. */
export function WorkspaceTilesSkeleton() {
  return (
    <section aria-hidden="true">
      <div className={styles.skeletonHeader}>
        <Skeleton width={120} height={14} text />
      </div>
      <div className={styles.grid}>
        {Array.from({ length: 3 }, (_, i) => (
          <div key={i} className={cx(styles.tile, styles.skeletonTile)}>
            <Skeleton width={36} height={36} radius={9} />
            <Skeleton width="60%" height={12} text />
            <Skeleton width="40%" height={10} text />
            <Skeleton width={110} height={20} radius={5} />
          </div>
        ))}
      </div>
    </section>
  );
}
