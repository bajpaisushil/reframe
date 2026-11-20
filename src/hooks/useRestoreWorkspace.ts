import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { useToast } from "@/components/ui/Toast";
import { api, errorMessage } from "@/lib/api";
import { queryClient, queryKeys } from "@/lib/queryClient";
import { navigate } from "@/lib/router";
import type { AppSettings, SwitchMode } from "@/types";
import { useActiveRuns } from "./useActiveRuns";

export interface RestoreTarget {
  id: string;
  name: string;
}

export interface RestoreWorkspaceResult {
  /** Start restoring (or open the restore already in progress) and go to its progress page. */
  restore: (workspace: RestoreTarget) => Promise<void>;
  /** Workspace id whose restore is being started (for a button spinner). */
  pendingId: string | null;
}

async function defaultSwitchMode(): Promise<SwitchMode> {
  const cached = queryClient.getState<AppSettings>(queryKeys.settings).data;
  if (cached) return cached.defaultSwitchMode;
  const settings = await queryClient.fetch(queryKeys.settings, api.settings);
  // Merge never closes anything, so it is the safe choice if settings cannot be read.
  return settings?.defaultSwitchMode ?? "merge";
}

/**
 * One-click restore with the user's default switch mode, restoring every enabled app and closing
 * nothing (closing windows is only ever done after explicit confirmation on the restore page).
 * Errors are shown as a toast; a restore already running for the workspace is opened instead of
 * starting a second one.
 */
export function useRestoreWorkspace(): RestoreWorkspaceResult {
  const toast = useToast();
  const { running } = useActiveRuns();
  const [pendingId, setPendingId] = useState<string | null>(null);
  const runningRef = useRef(running);
  useLayoutEffect(() => {
    runningRef.current = running;
  }, [running]);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const restore = useCallback(
    async (workspace: RestoreTarget) => {
      const existing = runningRef.current.find((run) => run.workspaceId === workspace.id);
      if (existing) {
        navigate({ name: "restore", runId: existing.id });
        return;
      }
      setPendingId(workspace.id);
      try {
        const mode = await defaultSwitchMode();
        const runId = await api.startRestore({ workspaceId: workspace.id, mode, entryIds: null, closeWindowIds: [] });
        navigate({ name: "restore", runId });
      } catch (error) {
        toast.error(`Couldn't restore ${workspace.name}`, errorMessage(error));
      } finally {
        if (mounted.current) setPendingId(null);
      }
    },
    [toast],
  );

  return { restore, pendingId };
}
