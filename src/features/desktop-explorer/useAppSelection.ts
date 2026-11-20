import { useCallback, useMemo, useState } from "react";
import type { CaptureSelection, DiscoveredApplication, WindowId } from "@/types";
import {
  clearChoice,
  countSelection,
  setAppSelected,
  setAppsSelected,
  setWindowSelected,
  toCaptureSelections,
  type Choices,
  type SelectionCounts,
} from "./selection";

export interface AppSelection {
  choices: Choices;
  /** Capture selections for the current apps, in snapshot order. */
  selections: CaptureSelection[];
  counts: SelectionCounts;
  setApp: (app: DiscoveredApplication, selected: boolean) => void;
  setApps: (apps: readonly DiscoveredApplication[], selected: boolean) => void;
  setWindow: (app: DiscoveredApplication, windowId: WindowId, selected: boolean) => void;
  /** Let the app's default (suggested or not) apply again. */
  clear: (appKey: string) => void;
  /** Forget every choice (back to the suggestions). */
  reset: () => void;
}

/**
 * The user's capture selection over the apps of the current snapshot. Choices survive rescans:
 * apps opened since follow their suggestion, closed windows simply drop out.
 */
export function useAppSelection(apps: readonly DiscoveredApplication[]): AppSelection {
  const [choices, setChoices] = useState<Choices>(() => new Map());

  const setApp = useCallback((app: DiscoveredApplication, selected: boolean) => {
    setChoices((current) => setAppSelected(current, app, selected));
  }, []);
  const setApps = useCallback((list: readonly DiscoveredApplication[], selected: boolean) => {
    setChoices((current) => setAppsSelected(current, list, selected));
  }, []);
  const setWindow = useCallback((app: DiscoveredApplication, windowId: WindowId, selected: boolean) => {
    setChoices((current) => setWindowSelected(current, app, windowId, selected));
  }, []);
  const clear = useCallback((appKey: string) => {
    setChoices((current) => clearChoice(current, appKey));
  }, []);
  const reset = useCallback(() => setChoices(new Map()), []);

  const selections = useMemo(() => toCaptureSelections(apps, choices), [apps, choices]);
  const counts = useMemo(() => countSelection(apps, choices), [apps, choices]);

  return { choices, selections, counts, setApp, setApps, setWindow, clear, reset };
}
