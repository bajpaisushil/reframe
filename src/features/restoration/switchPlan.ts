// Pure helpers for the switch dialog: which windows are selected for closing and icon lookups for
// the plan's rows. Tested in switchPlan.test.ts.

import type { CloseCandidate, DesktopSnapshot, IconLookup, SwitchPlan, Workspace } from "@/types";

/** The user's explicit choices, by window id; anything not listed uses `defaultSelected`. */
export type CloseOverrides = Readonly<Record<string, boolean>>;

export function isCloseSelected(candidate: CloseCandidate, overrides: CloseOverrides): boolean {
  return overrides[candidate.windowId] ?? candidate.defaultSelected;
}

/** Window ids the user confirmed may be closed (only ever from the current plan). */
export function selectedCloseIds(plan: Pick<SwitchPlan, "mode" | "close"> | null | undefined, overrides: CloseOverrides): string[] {
  if (!plan || plan.mode !== "replace") return [];
  return plan.close.filter((c) => isCloseSelected(c, overrides)).map((c) => c.windowId);
}

/** Select or clear every candidate. */
export function setAllClose(candidates: readonly CloseCandidate[], selected: boolean): Record<string, boolean> {
  return Object.fromEntries(candidates.map((c) => [c.windowId, selected]));
}

/** Suggested windows (from the previous workspace) first, then by app name and title. */
export function sortCloseCandidates(candidates: readonly CloseCandidate[]): CloseCandidate[] {
  return [...candidates].sort(
    (a, b) =>
      Number(b.defaultSelected) - Number(a.defaultSelected) ||
      a.appName.localeCompare(b.appName) ||
      a.title.localeCompare(b.title),
  );
}

/** Nothing would be opened or arranged (every app is turned off). */
export function isEmptyPlan(plan: Pick<SwitchPlan, "launch" | "reuse">): boolean {
  return plan.launch.length === 0 && plan.reuse.length === 0;
}

/** Icon lookup for a workspace entry, from its saved launch information. */
export function workspaceEntryLookup(workspace: Pick<Workspace, "applications"> | null | undefined, entryId: string, appId: string): IconLookup {
  const entry = workspace?.applications.find((a) => a.id === entryId);
  return { appId, executablePath: entry?.launch.executable ?? null, package: entry?.launch.package ?? null };
}

export interface WindowLookups {
  /** By window id, with the app name the scan saw (window ids can be reused by the system). */
  byWindow: ReadonlyMap<string, { lookup: IconLookup; name: string }>;
  /** By app name (lowercase), for windows that have closed since the scan. */
  byName: ReadonlyMap<string, IconLookup>;
}

/**
 * Icon lookups for running windows, from a desktop scan the app already has (no new scan is
 * started for icons).
 */
export function windowLookups(snapshot: Pick<DesktopSnapshot, "applications"> | null | undefined): WindowLookups {
  const byWindow = new Map<string, { lookup: IconLookup; name: string }>();
  const byName = new Map<string, IconLookup>();
  for (const app of snapshot?.applications ?? []) {
    const lookup: IconLookup = { appId: app.appId, executablePath: app.executablePath, package: app.package };
    const name = app.displayName.trim().toLowerCase();
    if (name && !byName.has(name)) byName.set(name, lookup);
    for (const window of app.windows) byWindow.set(window.id, { lookup, name });
  }
  return { byWindow, byName };
}

/**
 * The icon for a window: the scanned window when it still belongs to the same app, else any
 * scanned window of an app with that name, else initials from the app id.
 */
export function windowLookup(lookups: WindowLookups, windowId: string, appId: string, appName?: string): IconLookup {
  const name = appName?.trim().toLowerCase();
  const scanned = lookups.byWindow.get(windowId);
  if (scanned && (!name || scanned.name === name)) return scanned.lookup;
  return (name ? lookups.byName.get(name) : undefined) ?? { appId, executablePath: null, package: null };
}
