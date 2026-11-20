import { getVersion } from "@tauri-apps/api/app";
import { api } from "@/lib/api";
import { queryKeys } from "@/lib/queryClient";
import type { PlatformInfo } from "@/types";
import { useQuery, type QueryResult } from "./useQuery";

/** OS, display server and what the platform layer can do (does not change while running). */
export function usePlatformInfo(): QueryResult<PlatformInfo> {
  return useQuery(queryKeys.platformInfo, api.platformInfo, { staleTime: Number.POSITIVE_INFINITY });
}

/** Reframe's version from the app bundle. */
export function useAppVersion(): QueryResult<string> {
  return useQuery(queryKeys.appVersion, getVersion, { staleTime: Number.POSITIVE_INFINITY });
}
