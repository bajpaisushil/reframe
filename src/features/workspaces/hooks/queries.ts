import { useQuery, type QueryResult } from "@/hooks/useQuery";
import { api } from "@/lib/api";
import { queryKeys } from "@/lib/queryClient";
import type { RestoreRun, WorkspaceHealth } from "@/types";

/**
 * Health is keyed under the workspace (`workspace:<id>:health`), so everything that invalidates
 * the workspace — a save, a finished restore — refreshes its readiness report too.
 */
export const healthKey = (id: string) => `${queryKeys.workspace(id)}:health`;

/** Under `restoreHistory`, which finished restores invalidate. */
export const historyKey = (id: string, limit: number) => `${queryKeys.restoreHistory}:${id}:${limit}`;

/** Restore readiness on this computer (scans the desktop, so it is cached briefly). */
export function useWorkspaceHealth(id: string): QueryResult<WorkspaceHealth> {
  return useQuery(healthKey(id), () => api.workspaceHealth(id), { staleTime: 5_000, refetchOnFocus: true });
}

/** The workspace's most recent restores, newest first. */
export function useRestoreHistory(id: string, limit: number): QueryResult<RestoreRun[]> {
  return useQuery(historyKey(id, limit), () => api.restoreHistory(id, limit), { staleTime: 10_000 });
}
