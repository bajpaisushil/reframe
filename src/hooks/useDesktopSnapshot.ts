import { api } from "@/lib/api";
import { queryKeys } from "@/lib/queryClient";
import type { DesktopSnapshot } from "@/types";
import { useQuery, type QueryOptions, type QueryResult } from "./useQuery";

/**
 * The current desktop (applications with visible windows). Scanning can take a moment, so a
 * result younger than 30 s is reused when navigating between pages; `refresh()` scans again.
 */
export function useDesktopSnapshot(options: Pick<QueryOptions, "enabled"> = {}): QueryResult<DesktopSnapshot> {
  return useQuery(queryKeys.desktopSnapshot, api.scanDesktop, { staleTime: 30_000, ...options });
}
