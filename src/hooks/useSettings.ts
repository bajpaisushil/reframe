import { useCallback } from "react";
import { api } from "@/lib/api";
import { queryClient, queryKeys } from "@/lib/queryClient";
import type { AppSettings } from "@/types";
import { useQuery, type QueryResult } from "./useQuery";

export type SettingsPatch = Partial<AppSettings> | ((current: AppSettings) => AppSettings);

let queue: Promise<unknown> = Promise.resolve();

/**
 * Save a settings change. Updates are serialized and each one is applied to the latest known
 * settings, so concurrent edits from different places (theme menu, settings page) never overwrite
 * each other. The cache updates optimistically; on failure it is reloaded from the backend (the
 * backend may have saved part of the change, e.g. when only the login item failed) and the error
 * is rethrown for the caller to show.
 */
export function updateSettings(patch: SettingsPatch): Promise<AppSettings> {
  const run = async (): Promise<AppSettings> => {
    const base = queryClient.getState<AppSettings>(queryKeys.settings).data ?? (await api.settings());
    const next = typeof patch === "function" ? patch(base) : { ...base, ...patch };
    queryClient.setData(queryKeys.settings, next);
    try {
      const saved = await api.updateSettings(next);
      queryClient.setData(queryKeys.settings, saved);
      return saved;
    } catch (error) {
      void queryClient.fetch(queryKeys.settings, api.settings, { force: true });
      throw error;
    }
  };
  const result = queue.then(run, run);
  queue = result.catch(() => undefined);
  return result;
}

export interface SettingsResult extends QueryResult<AppSettings> {
  settings: AppSettings | undefined;
  update: (patch: SettingsPatch) => Promise<AppSettings>;
}

/** App settings plus a serialized `update`. */
export function useSettings(): SettingsResult {
  const query = useQuery(queryKeys.settings, api.settings, { staleTime: 60_000 });
  const update = useCallback((patch: SettingsPatch) => updateSettings(patch), []);
  return { ...query, settings: query.data, update };
}
