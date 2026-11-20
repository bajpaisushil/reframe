import { useEffect, useMemo, useRef, useState } from "react";
import { ArrowDownUp, ChevronDown, Compass, FileDown, LayoutGrid, Plus, RefreshCw, Search, SearchX, X } from "lucide-react";
import { Page, PageHeader } from "@/components/layout/Page";
import { Button, EmptyState, IconButton, Input, Menu } from "@/components/ui";
import { NewWorkspaceDialog } from "@/features/dashboard/NewWorkspaceDialog";
import { SwitchDialog } from "@/features/restoration/SwitchDialog";
import { useActiveRuns } from "@/hooks/useActiveRuns";
import { useRestoreWorkspace } from "@/hooks/useRestoreWorkspace";
import { useWorkspaces } from "@/hooks/useWorkspaces";
import { errorMessage } from "@/lib/api";
import { formatCount, pluralize } from "@/lib/format";
import { hasPrimaryModifier } from "@/lib/platform";
import { navigate } from "@/lib/router";
import type { WorkspaceSummary } from "@/types";
import { useWorkspaceActions } from "./hooks/useWorkspaceActions";
import { filterAndSortWorkspaces, isWorkspaceSort, SORT_OPTIONS, type WorkspaceSort } from "./model/listing";
import { WorkspaceCard, WorkspaceCardSkeleton } from "./WorkspaceCard";
import styles from "./WorkspacesPage.module.css";

const SORT_STORAGE_KEY = "reframe.workspaces.sort";

function readStoredSort(): WorkspaceSort {
  try {
    const value = window.localStorage.getItem(SORT_STORAGE_KEY);
    return isWorkspaceSort(value) ? value : "recent";
  } catch {
    return "recent";
  }
}

function storeSort(sort: WorkspaceSort): void {
  try {
    window.localStorage.setItem(SORT_STORAGE_KEY, sort);
  } catch {
    // A remembered sort order is a convenience; nothing to do when storage is unavailable.
  }
}

/** The workspace library: search, sort and act on every saved workspace. */
export function WorkspacesPage() {
  const workspaces = useWorkspaces();
  const { running } = useActiveRuns();
  const { restore, pendingId } = useRestoreWorkspace();
  const actions = useWorkspaceActions();
  const [creating, setCreating] = useState(false);
  // The target outlives `open` so the dialog can animate out.
  const [switchTarget, setSwitchTarget] = useState<WorkspaceSummary | null>(null);
  const [switchOpen, setSwitchOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<WorkspaceSort>(readStoredSort);
  const searchRef = useRef<HTMLInputElement>(null);

  const all = workspaces.data;
  const visible = useMemo(() => filterAndSortWorkspaces(all ?? [], query, sort), [all, query, sort]);
  const restoringIds = useMemo(() => new Set(running.map((run) => run.workspaceId)), [running]);
  const total = all?.length ?? 0;
  const searching = query.trim() !== "";

  // Ctrl/⌘+F focuses the search field.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented || document.getElementById("root")?.inert) return;
      if (hasPrimaryModifier(event) && !event.shiftKey && !event.altKey && event.key.toLowerCase() === "f" && searchRef.current) {
        event.preventDefault();
        searchRef.current.focus();
        searchRef.current.select();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  const changeSort = (next: WorkspaceSort) => {
    setSort(next);
    storeSort(next);
  };

  const sortLabel = SORT_OPTIONS.find((option) => option.value === sort)?.label ?? "Recently used";

  return (
    <Page>
      <PageHeader
        title={
          <>
            Workspaces
            {total > 0 && <span className={styles.titleCount}>{formatCount(total)}</span>}
          </>
        }
        subtitle="Everything you've saved, ready to restore."
        actions={
          <>
            <Button variant="secondary" icon={FileDown} onClick={() => navigate({ name: "import" })}>
              Import
            </Button>
            {/* With nothing saved yet, "Explore your desktop" below is the one primary action. */}
            <Button variant={all !== undefined && total === 0 ? "secondary" : "primary"} icon={Plus} onClick={() => setCreating(true)}>
              New workspace
            </Button>
          </>
        }
      />

      {all === undefined ? (
        workspaces.error ? (
          <EmptyState
            tone="danger"
            icon={LayoutGrid}
            title="Couldn't load your workspaces"
            text={errorMessage(workspaces.error)}
            action={
              <Button variant="secondary" icon={RefreshCw} onClick={() => void workspaces.refresh()}>
                Try again
              </Button>
            }
          />
        ) : (
          <ul className={styles.grid} aria-busy="true" aria-label="Loading workspaces">
            {Array.from({ length: 6 }, (_, index) => (
              <li key={index}>
                <WorkspaceCardSkeleton />
              </li>
            ))}
          </ul>
        )
      ) : total === 0 ? (
        <EmptyState
          icon={LayoutGrid}
          title="No workspaces yet"
          text="Capture the apps and windows you have open, and Reframe will bring them back exactly as they were whenever you need them."
          action={
            <>
              <Button variant="primary" icon={Compass} onClick={() => navigate({ name: "explore" })}>
                Explore your desktop
              </Button>
              <Button variant="secondary" icon={Plus} onClick={() => setCreating(true)}>
                New workspace
              </Button>
            </>
          }
        />
      ) : (
        <>
          <div className={styles.toolbar}>
            <div className={styles.search}>
              <Input
                ref={searchRef}
                role="searchbox"
                inputMode="search"
                enterKeyHint="search"
                icon={Search}
                value={query}
                placeholder="Search workspaces and apps"
                aria-label="Search workspaces"
                className={styles.searchInput}
                onChange={(event) => setQuery(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Escape" && query !== "") {
                    event.preventDefault();
                    event.stopPropagation();
                    setQuery("");
                  }
                }}
              />
              {searching && (
                <IconButton
                  icon={X}
                  size="sm"
                  aria-label="Clear search"
                  className={styles.clear}
                  onClick={() => {
                    setQuery("");
                    searchRef.current?.focus();
                  }}
                />
              )}
            </div>
            <span className={styles.resultCount} aria-live="polite">
              {searching ? `${pluralize(visible.length, "result")}` : ""}
            </span>
            <Menu
              aria-label="Sort workspaces"
              items={SORT_OPTIONS.map((option) => ({
                label: option.label,
                checked: option.value === sort,
                onSelect: () => changeSort(option.value),
              }))}
              trigger={(props) => (
                <Button {...props} variant="ghost" size="sm" icon={ArrowDownUp} iconEnd={ChevronDown} aria-label={`Sort: ${sortLabel}`}>
                  {sortLabel}
                </Button>
              )}
            />
          </div>

          {visible.length === 0 ? (
            <EmptyState
              size="compact"
              icon={SearchX}
              title={`No workspaces match “${query.trim()}”`}
              text="Try another name, or search for an app in the workspace."
              action={
                <Button size="sm" variant="secondary" onClick={() => setQuery("")}>
                  Clear search
                </Button>
              }
            />
          ) : (
            <ul className={styles.grid}>
              {visible.map((workspace) => (
                <li key={workspace.id}>
                  <WorkspaceCard
                    workspace={workspace}
                    onRestore={() => void restore(workspace)}
                    onRestoreWithOptions={() => {
                      setSwitchTarget(workspace);
                      setSwitchOpen(true);
                    }}
                    onDuplicate={() => void actions.duplicate(workspace)}
                    onExport={() => void actions.exportToFile(workspace)}
                    onDelete={() => actions.requestDelete(workspace)}
                    starting={pendingId === workspace.id}
                    inProgress={restoringIds.has(workspace.id)}
                    busy={actions.busy?.id === workspace.id}
                  />
                </li>
              ))}
            </ul>
          )}
        </>
      )}

      <NewWorkspaceDialog open={creating} onClose={() => setCreating(false)} />
      {switchTarget && (
        <SwitchDialog
          open={switchOpen}
          onClose={() => setSwitchOpen(false)}
          workspace={{ id: switchTarget.id, name: switchTarget.name, icon: switchTarget.icon, color: switchTarget.color }}
        />
      )}
      {actions.dialogs}
    </Page>
  );
}
