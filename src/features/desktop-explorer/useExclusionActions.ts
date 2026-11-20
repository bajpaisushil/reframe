import { useCallback, useState } from "react";
import { useToast } from "@/components/ui";
import { updateSettings } from "@/hooks/useSettings";
import { errorMessage } from "@/lib/api";
import { queryClient, queryKeys } from "@/lib/queryClient";
import type { DesktopSnapshot, DiscoveredApplication } from "@/types";
import { addExclusion, removeExclusion } from "./exclusions";

export interface ExclusionActions {
  /** Add the app to the "never suggest" list. */
  exclude: (app: DiscoveredApplication) => Promise<void>;
  /** Remove the entries that exclude the app. */
  stopExcluding: (app: DiscoveredApplication) => Promise<void>;
  /** Key of the app whose exclusion is being saved. */
  pendingKey: string | null;
}

/** Show an app's new exclusion state in the cached snapshot until the next scan replaces it. */
function patchSnapshot(appKey: string, excluded: boolean): void {
  const current = queryClient.getState<DesktopSnapshot>(queryKeys.desktopSnapshot).data;
  if (!current) return;
  queryClient.setData<DesktopSnapshot>(queryKeys.desktopSnapshot, {
    ...current,
    applications: current.applications.map((app) =>
      app.key === appKey ? { ...app, excluded, suggested: !excluded && app.category === "application" } : app,
    ),
  });
}

/**
 * "Always exclude" / "Stop excluding" from Explore: saves `excludedApps`, shows the new state
 * right away, then rescans so the backend's own matching has the final word.
 *
 * `rescan` is the snapshot's refresh; `resetChoice` makes the app follow its new default
 * (excluded apps start unchecked, apps that are no longer excluded are suggested again).
 */
export function useExclusionActions(
  rescan: () => Promise<DesktopSnapshot | undefined>,
  resetChoice: (appKey: string) => void,
): ExclusionActions {
  const toast = useToast();
  const [pendingKey, setPendingKey] = useState<string | null>(null);

  const change = useCallback(
    async (app: DiscoveredApplication, exclude: boolean) => {
      const name = app.displayName;
      setPendingKey(app.key);
      try {
        await updateSettings((current) => ({
          ...current,
          excludedApps: exclude ? addExclusion(current.excludedApps, app) : removeExclusion(current.excludedApps, app),
        }));
      } catch (error) {
        toast.error(exclude ? `Couldn't exclude ${name}` : `Couldn't stop excluding ${name}`, errorMessage(error));
        setPendingKey(null);
        return;
      }

      patchSnapshot(app.key, exclude);
      resetChoice(app.key);
      if (exclude) {
        toast.success(`${name} won't be suggested anymore`, "It stays in the list, unchecked. You can change this in Settings.");
      } else {
        toast.success(`${name} will be suggested again`);
      }

      const fresh = await rescan();
      setPendingKey(null);
      const after = fresh?.applications.find((candidate) => candidate.key === app.key);
      if (!exclude && after?.excluded) {
        toast.info(`${name} is still excluded`, "Another entry in your excluded apps matches it. You can review the list in Settings.");
      }
    },
    [rescan, resetChoice, toast],
  );

  const exclude = useCallback((app: DiscoveredApplication) => change(app, true), [change]);
  const stopExcluding = useCallback((app: DiscoveredApplication) => change(app, false), [change]);
  return { exclude, stopExcluding, pendingKey };
}
