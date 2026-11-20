// Selection model for capturing applications (tested in selection.test.ts).
//
// The user's decisions are stored as *choices* keyed by `DiscoveredApplication::key`. An app
// without a choice follows its default (selected when the backend suggests it), so a rescan
// selects newly opened suggested apps while keeping everything the user decided. Choices are
// resolved against the current snapshot: windows that closed drop out of a subset, and a subset
// that covers every current window counts as "all windows".

import { pluralize } from "@/lib/format";
import type { CaptureSelection, DiscoveredApplication, WindowId } from "@/types";

/** What the user chose for one application. */
export type AppChoice =
  | { kind: "none" }
  | { kind: "all" }
  /** A strict subset of the app's windows (in the app's window order). */
  | { kind: "some"; windowIds: readonly WindowId[] };

/** Explicit choices by app key. Missing keys follow the app's default. */
export type Choices = ReadonlyMap<string, AppChoice>;

export type CheckState = "checked" | "unchecked" | "indeterminate";

/** The fields of a discovered application the selection model needs. */
export type SelectableApp = Pick<DiscoveredApplication, "key" | "suggested"> & {
  windows: ReadonlyArray<{ id: WindowId }>;
};

const NONE: AppChoice = { kind: "none" };
const ALL: AppChoice = { kind: "all" };

export interface ResolvedSelection {
  /** Selected windows that are still open, in the app's window order. */
  windowIds: WindowId[];
  /** Every current window is selected (captured as "all windows", which also covers new ones). */
  all: boolean;
}

/** The choice in effect for an app: the explicit one, else its default. */
export function choiceFor(app: SelectableApp, choices: Choices): AppChoice {
  return choices.get(app.key) ?? (app.suggested ? ALL : NONE);
}

/** What is selected of an app right now; `null` when nothing is. */
export function resolveSelection(app: SelectableApp, choices: Choices): ResolvedSelection | null {
  const choice = choiceFor(app, choices);
  const ids = app.windows.map((w) => w.id);
  switch (choice.kind) {
    case "none":
      return null;
    case "all":
      return { windowIds: ids, all: true };
    case "some": {
      const chosen = new Set(choice.windowIds);
      const windowIds = ids.filter((id) => chosen.has(id));
      if (windowIds.length === 0) return null;
      return { windowIds, all: windowIds.length === ids.length };
    }
  }
}

export function checkState(app: SelectableApp, choices: Choices): CheckState {
  const resolved = resolveSelection(app, choices);
  if (!resolved) return "unchecked";
  return resolved.all ? "checked" : "indeterminate";
}

export function isWindowSelected(app: SelectableApp, choices: Choices, windowId: WindowId): boolean {
  return resolveSelection(app, choices)?.windowIds.includes(windowId) ?? false;
}

function withChoice(choices: Choices, key: string, choice: AppChoice | undefined): Map<string, AppChoice> {
  const next = new Map(choices);
  if (choice) next.set(key, choice);
  else next.delete(key);
  return next;
}

/** Select every window of an app, or none of them. */
export function setAppSelected(choices: Choices, app: SelectableApp, selected: boolean): Map<string, AppChoice> {
  return withChoice(choices, app.key, selected ? ALL : NONE);
}

/** Same as {@link setAppSelected} for several apps at once. */
export function setAppsSelected(
  choices: Choices,
  apps: readonly SelectableApp[],
  selected: boolean,
): Map<string, AppChoice> {
  const next = new Map(choices);
  for (const app of apps) next.set(app.key, selected ? ALL : NONE);
  return next;
}

/** Forget the user's choice for an app so its default applies again. */
export function clearChoice(choices: Choices, key: string): Map<string, AppChoice> {
  return withChoice(choices, key, undefined);
}

/**
 * Add or remove one window. Removing the last window deselects the app; selecting every window
 * turns the choice back into "all windows".
 */
export function setWindowSelected(
  choices: Choices,
  app: SelectableApp,
  windowId: WindowId,
  selected: boolean,
): Map<string, AppChoice> {
  const ids = app.windows.map((w) => w.id);
  if (!ids.includes(windowId)) return new Map(choices);
  const current = new Set(resolveSelection(app, choices)?.windowIds ?? []);
  if (selected) current.add(windowId);
  else current.delete(windowId);
  const windowIds = ids.filter((id) => current.has(id));
  const choice: AppChoice =
    windowIds.length === 0 ? NONE : windowIds.length === ids.length ? ALL : { kind: "some", windowIds };
  return withChoice(choices, app.key, choice);
}

/**
 * Capture selections in the given (snapshot) order. An app whose every window is selected is sent
 * with an empty window list, which the backend reads as "all windows".
 */
export function toCaptureSelections(apps: readonly SelectableApp[], choices: Choices): CaptureSelection[] {
  const selections: CaptureSelection[] = [];
  for (const app of apps) {
    const resolved = resolveSelection(app, choices);
    if (!resolved) continue;
    selections.push({ appKey: app.key, windowIds: resolved.all ? [] : resolved.windowIds });
  }
  return selections;
}

export interface SelectionCounts {
  apps: number;
  windows: number;
}

export function countSelection(apps: readonly SelectableApp[], choices: Choices): SelectionCounts {
  let appCount = 0;
  let windowCount = 0;
  for (const app of apps) {
    const resolved = resolveSelection(app, choices);
    if (!resolved) continue;
    appCount += 1;
    windowCount += resolved.windowIds.length;
  }
  return { apps: appCount, windows: windowCount };
}

/** "3 apps · 7 windows" (or "1 app · 1 window"). */
export function formatCounts(counts: SelectionCounts): string {
  return `${pluralize(counts.apps, "app")} · ${pluralize(counts.windows, "window")}`;
}
