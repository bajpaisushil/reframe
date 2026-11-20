import { useMemo, useSyncExternalStore } from "react";
import { ActiveRunsStore, type ActiveRunsSnapshot } from "@/lib/activeRunsStore";
import { api } from "@/lib/api";
import { onRestoreEvent } from "@/lib/events";
import { queryClient, queryKeys } from "@/lib/queryClient";
import type { RestoreRun } from "@/types";

let store: ActiveRunsStore | null = null;

function getStore(): ActiveRunsStore {
  if (!store) {
    store = new ActiveRunsStore({
      listen: (handler) => onRestoreEvent(handler),
      fetchActive: api.activeRuns,
      onRunFinished: (run) => {
        // A finished restore changes last-restored times, the active workspace and the feed.
        queryClient.invalidate(
          queryKeys.workspaces,
          queryKeys.activeWorkspace,
          queryKeys.activityAll,
          queryKeys.workspace(run.workspaceId),
          queryKeys.restoreHistory,
          queryKeys.restoreRun(run.id),
        );
      },
    });
  }
  return store;
}

export interface ActiveRunsResult extends ActiveRunsSnapshot {
  refresh: () => Promise<void>;
}

/**
 * Live restore runs: everything `api.activeRuns()` reports, kept up to date by restore events.
 * `running` lists runs in progress; `runs` also keeps recently finished runs of this session.
 */
export function useActiveRuns(): ActiveRunsResult {
  const runsStore = getStore();
  const snapshot = useSyncExternalStore(runsStore.subscribe, runsStore.getSnapshot, runsStore.getSnapshot);
  return useMemo(() => ({ ...snapshot, refresh: runsStore.refresh }), [snapshot, runsStore]);
}

/** The live state of one run if this session has seen it (null otherwise). */
export function useLiveRun(runId: string): RestoreRun | null {
  const { runs } = useActiveRuns();
  return runs.get(runId) ?? null;
}
