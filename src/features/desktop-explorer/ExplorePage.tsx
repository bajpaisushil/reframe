import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { AppWindow, LayoutGrid, MonitorOff, RefreshCw } from "lucide-react";
import { Page, PageHeader } from "@/components/layout/Page";
import { Button, EmptyState, Skeleton, useToast } from "@/components/ui";
import { useDesktopSnapshot } from "@/hooks/useDesktopSnapshot";
import { errorMessage } from "@/lib/api";
import { queryClient, queryKeys } from "@/lib/queryClient";
import { navigate } from "@/lib/router";
import type { DesktopSnapshot, DiscoveredApplication } from "@/types";
import { AppList, AppListSkeleton } from "./AppList";
import { backgroundNote } from "./describe";
import { PermissionNotices, ScanNotes } from "./DesktopNotices";
import { MonitorMap, type HoverTarget } from "./MonitorMap";
import { desktopNotes } from "./notices";
import { SaveWorkspaceDialog } from "./SaveWorkspaceDialog";
import { SelectionBar } from "./SelectionBar";
import { checkState } from "./selection";
import { useAppSelection } from "./useAppSelection";
import { useExclusionActions } from "./useExclusionActions";
import { usePermissions } from "./usePermissions";
import styles from "./ExplorePage.module.css";

const NO_APPS: DiscoveredApplication[] = [];

/** Explore: what is open on the desktop right now, and what to capture into a workspace. */
export function ExplorePage() {
  const toast = useToast();
  const snapshot = useDesktopSnapshot();
  const { refresh } = snapshot;
  const data = snapshot.data;
  const apps = data?.applications ?? NO_APPS;
  const selection = useAppSelection(apps);
  const exclusions = useExclusionActions(refresh, selection.clear);
  const permissions = usePermissions();
  const [hovered, setHovered] = useState<HoverTarget | null>(null);
  const [saveOpen, setSaveOpen] = useState(false);

  const notes = useMemo(() => (data ? desktopNotes(data) : null), [data]);
  const selectedApps = useMemo(() => {
    const keys = new Set(selection.selections.map((s) => s.appKey));
    return apps.filter((app) => keys.has(app.key));
  }, [apps, selection.selections]);
  const hasSelection = selection.counts.apps > 0;
  const scanning = snapshot.isFetching;

  const rescan = useCallback(async () => {
    const result = await refresh();
    const { error } = queryClient.getState<DesktopSnapshot>(queryKeys.desktopSnapshot);
    if (!result && error) toast.error("Couldn't look at your desktop", errorMessage(error));
  }, [refresh, toast]);

  // A newly granted permission changes what Reframe can see: scan again.
  const missingCount = permissions.missing.length;
  const previousMissing = useRef(missingCount);
  useEffect(() => {
    if (missingCount < previousMissing.current) void refresh();
    previousMissing.current = missingCount;
  }, [missingCount, refresh]);

  const { setApp, setApps } = selection;
  const toggleFromMap = useCallback(
    (app: DiscoveredApplication) => setApp(app, checkState(app, selection.choices) !== "checked"),
    [setApp, selection.choices],
  );

  let content;
  if (!data || !notes) {
    content = snapshot.error ? (
      <EmptyState
        tone="danger"
        icon={MonitorOff}
        title="Couldn't look at your desktop"
        text={errorMessage(snapshot.error)}
        action={
          <Button variant="secondary" icon={RefreshCw} loading={scanning} onClick={() => void rescan()}>
            Try again
          </Button>
        }
      />
    ) : (
      <ExploreSkeleton />
    );
  } else if (notes.blocked) {
    content = (
      <div className={styles.content}>
        <EmptyState
          icon={MonitorOff}
          title="Reframe can't see app windows here"
          text={
            <>
              {notes.blocked}
              <span className={styles.blockedHint}>
                You can still build workspaces: open one and add apps to it by choosing them manually.
              </span>
            </>
          }
          action={
            <Button variant="secondary" icon={LayoutGrid} onClick={() => navigate({ name: "workspaces" })}>
              Go to Workspaces
            </Button>
          }
        />
        <ScanNotes notes={notes.notes} />
      </div>
    );
  } else {
    content = (
      <div className={styles.content}>
        {data.monitors.length > 0 && (
          <MonitorMap
            monitors={data.monitors}
            apps={apps}
            choices={selection.choices}
            hovered={hovered}
            onHover={setHovered}
            onToggleApp={toggleFromMap}
            capturedAt={data.capturedAt}
            scanning={scanning}
          />
        )}
        <ScanNotes notes={notes.notes} />
        {apps.length === 0 ? (
          <EmptyState
            size="compact"
            className={styles.nothingOpen}
            icon={AppWindow}
            title="Nothing's open right now"
            text="Open the apps you work with, then refresh. Reframe lists every app that has a window."
            action={
              <Button variant="secondary" size="sm" icon={RefreshCw} loading={scanning} onClick={() => void rescan()}>
                Refresh
              </Button>
            }
          />
        ) : (
          <div aria-busy={scanning || undefined}>
            <AppList
              apps={apps}
              monitors={data.monitors}
              selection={selection}
              hovered={hovered}
              onHover={setHovered}
              exclusions={exclusions}
            />
          </div>
        )}
        {data.backgroundProcessCount > 0 && <p className={styles.footnote}>{backgroundNote(data.backgroundProcessCount)}</p>}
      </div>
    );
  }

  return (
    <Page>
      <PageHeader
        title="Explore"
        subtitle="What's open on your desktop right now"
        actions={
          <>
            <Button variant="secondary" icon={RefreshCw} loading={scanning} onClick={() => void rescan()}>
              Refresh
            </Button>
            <Button variant="primary" disabled={!hasSelection} onClick={() => setSaveOpen(true)}>
              Save as workspace…
            </Button>
          </>
        }
      />

      <div className={styles.sections}>
        <PermissionNotices missing={permissions.missing} onGrant={permissions.grant} granting={permissions.granting} />
        {content}
      </div>

      {hasSelection && !saveOpen && (
        <SelectionBar counts={selection.counts} onSave={() => setSaveOpen(true)} onClear={() => setApps(apps, false)} />
      )}

      <SaveWorkspaceDialog
        open={saveOpen}
        onClose={() => setSaveOpen(false)}
        selections={selection.selections}
        apps={selectedApps}
        counts={selection.counts}
        // The desktop may have changed while saving; keep the list current for "Stay here".
        onSaved={() => void refresh()}
      />
    </Page>
  );
}

/** Placeholder for the first scan (it can take a moment while adapters look inside apps). */
function ExploreSkeleton() {
  return (
    <div className={styles.content} role="status" aria-label="Looking at your desktop…">
      <div className={styles.deskSkeleton} aria-hidden="true">
        <Skeleton width={344} height={194} radius={5} />
      </div>
      <AppListSkeleton rows={5} />
    </div>
  );
}
