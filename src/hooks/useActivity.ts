import { api } from "@/lib/api";
import { queryKeys } from "@/lib/queryClient";
import type { ActivityEntry } from "@/types";
import { useQuery, type QueryResult } from "./useQuery";

/** The most recent activity entries, newest first (`limit` is clamped to 1..200 by the backend). */
export function useActivity(limit: number): QueryResult<ActivityEntry[]> {
  return useQuery(queryKeys.activity(limit), () => api.activity(limit), { staleTime: 10_000, refetchOnFocus: true });
}
