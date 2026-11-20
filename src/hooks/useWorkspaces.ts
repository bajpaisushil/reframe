import { useMemo } from "react";
import { api } from "@/lib/api";
import { queryKeys } from "@/lib/queryClient";
import type { Workspace, WorkspaceSummary } from "@/types";
import { useQuery, type QueryResult } from "./useQuery";

/** All saved workspaces (most recently updated first, as the backend lists them). */
export function useWorkspaces(): QueryResult<WorkspaceSummary[]> {
  return useQuery(queryKeys.workspaces, api.listWorkspaces, { staleTime: 10_000, refetchOnFocus: true });
}

/** One full workspace by id (`enabled` is false for an empty id). */
export function useWorkspace(id: string): QueryResult<Workspace> {
  return useQuery(queryKeys.workspace(id), () => api.getWorkspace(id), {
    enabled: id.length > 0,
    staleTime: 10_000,
    refetchOnFocus: true,
  });
}

/** Id of the workspace restored most recently (the "active" one), or null. */
export function useActiveWorkspaceId(): QueryResult<string | null> {
  return useQuery(queryKeys.activeWorkspace, api.activeWorkspace, { staleTime: 10_000, refetchOnFocus: true });
}

export interface ActiveWorkspaceResult {
  /** Summary of the active workspace; null when there is none (or it no longer exists). */
  workspace: WorkspaceSummary | null;
  isLoading: boolean;
  error: unknown;
}

/** The active workspace's summary, joined from the active id and the workspace list. */
export function useActiveWorkspace(): ActiveWorkspaceResult {
  const workspaces = useWorkspaces();
  const activeId = useActiveWorkspaceId();
  const workspace = useMemo(() => {
    const list = workspaces.data ?? [];
    if (activeId.data) return list.find((w) => w.id === activeId.data) ?? null;
    // The id query failed or is loading: fall back to the flag on the summaries.
    return activeId.data === undefined ? (list.find((w) => w.isActive) ?? null) : null;
  }, [workspaces.data, activeId.data]);
  return {
    workspace,
    isLoading: workspaces.isLoading || activeId.isLoading,
    error: workspaces.error ?? activeId.error,
  };
}
