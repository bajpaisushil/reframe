import { ArrowUpRight, Copy, Ellipsis, FileUp, RotateCcw, SlidersHorizontal, Trash } from "lucide-react";
import {
  AppIconStack,
  Badge,
  Button,
  IconButton,
  Menu,
  RelativeTime,
  Skeleton,
  WorkspaceIcon,
  type MenuEntry,
} from "@/components/ui";
import { cx } from "@/lib/cx";
import { pluralize } from "@/lib/format";
import { navigate } from "@/lib/router";
import { summaryApps } from "@/lib/workspaces";
import type { WorkspaceSummary } from "@/types";
import styles from "./WorkspaceCard.module.css";

export interface WorkspaceCardProps {
  workspace: WorkspaceSummary;
  /** One-click restore with the default mode. */
  onRestore: () => void;
  /** Restore with a choice of mode (the switch dialog). */
  onRestoreWithOptions: () => void;
  onDuplicate: () => void;
  onExport: () => void;
  onDelete: () => void;
  /** The restore is being started (button spinner). */
  starting: boolean;
  /** A restore of this workspace is running. */
  inProgress: boolean;
  /** Duplicate/export in progress. */
  busy: boolean;
}

/** One saved workspace in the library: identity, apps, last use and quick actions. */
export function WorkspaceCard({
  workspace,
  onRestore,
  onRestoreWithOptions,
  onDuplicate,
  onExport,
  onDelete,
  starting,
  inProgress,
  busy,
}: WorkspaceCardProps) {
  const empty = workspace.applicationCount === 0;
  const open = () => navigate({ name: "workspace", id: workspace.id });

  const items: MenuEntry[] = [
    { label: "Open", icon: ArrowUpRight, onSelect: open },
    { label: "Restore with options…", icon: SlidersHorizontal, onSelect: onRestoreWithOptions, disabled: empty },
    { type: "separator" },
    { label: "Duplicate", icon: Copy, onSelect: onDuplicate, disabled: busy },
    { label: "Export…", icon: FileUp, onSelect: onExport, disabled: busy },
    { type: "separator" },
    { label: "Delete…", icon: Trash, onSelect: onDelete, danger: true },
  ];

  return (
    <article className={cx(styles.card, (starting || inProgress) && styles.busy)} aria-label={workspace.name}>
      {/* The whole card opens the workspace; the actions are siblings, never nested in it. */}
      <button type="button" className={styles.open} onClick={open} aria-label={`Open ${workspace.name}`}>
        <span className={styles.head}>
          <WorkspaceIcon emoji={workspace.icon} color={workspace.color} size={36} />
          <span className={styles.titles}>
            <span className={styles.nameRow}>
              <span className={styles.name}>{workspace.name}</span>
              {workspace.isActive && (
                <Badge tone="accent" dot className={styles.active}>
                  Active
                </Badge>
              )}
            </span>
            <span className={styles.when}>
              {inProgress ? (
                "Restoring now…"
              ) : (
                <RelativeTime value={workspace.lastRestoredAt} prefix="Restored " fallback="Never restored" />
              )}
            </span>
          </span>
        </span>
        {workspace.description.trim() !== "" && <span className={styles.description}>{workspace.description}</span>}
        <span className={styles.footer}>
          {empty ? (
            <span className={styles.noApps}>No apps yet</span>
          ) : (
            <>
              <AppIconStack apps={summaryApps(workspace)} total={workspace.applicationCount} max={5} size={20} />
              <span className={styles.count}>{pluralize(workspace.applicationCount, "app")}</span>
            </>
          )}
        </span>
      </button>

      <div className={styles.menu}>
        <Menu
          aria-label={`${workspace.name} actions`}
          items={items}
          trigger={(props) => (
            <IconButton {...props} icon={Ellipsis} size="sm" aria-label={`More actions for ${workspace.name}`} loading={busy} />
          )}
        />
      </div>
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
    </article>
  );
}

/** Placeholder card while the library loads. */
export function WorkspaceCardSkeleton() {
  return (
    <div className={cx(styles.card, styles.skeleton)} aria-hidden="true">
      <span className={styles.head}>
        <Skeleton width={36} height={36} radius={9} />
        <span className={styles.titles}>
          <Skeleton width="55%" height={12} text />
          <Skeleton width="35%" height={10} text style={{ marginTop: 6 }} />
        </span>
      </span>
      <Skeleton width="80%" height={10} text />
      <span className={styles.footer}>
        <Skeleton width={104} height={20} radius={5} />
      </span>
    </div>
  );
}
