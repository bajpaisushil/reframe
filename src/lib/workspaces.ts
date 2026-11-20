// Pure helpers for workspace summaries (tested in workspaces.test.ts).

import type { IconLookup, WorkspaceSummary } from "@/types";

export interface SummaryApp {
  lookup: IconLookup;
  name: string;
}

/** The first few apps of a summary, pairing icon lookups with their display names. */
export function summaryApps(summary: Pick<WorkspaceSummary, "applicationIcons" | "applicationNames">): SummaryApp[] {
  return summary.applicationIcons.map((lookup, index) => ({
    lookup,
    name: summary.applicationNames[index] ?? lookup.appId,
  }));
}

function time(value: string | null | undefined): number {
  if (!value) return 0;
  const ms = Date.parse(value);
  return Number.isNaN(ms) ? 0 : ms;
}

/** When the workspace was last used: restored or edited, whichever is later (epoch ms). */
export function lastUsedAt(summary: Pick<WorkspaceSummary, "lastRestoredAt" | "updatedAt">): number {
  return Math.max(time(summary.lastRestoredAt), time(summary.updatedAt));
}

/** Most recently used first; ties keep the backend order (stable sort). */
export function sortByRecentUse<T extends Pick<WorkspaceSummary, "lastRestoredAt" | "updatedAt">>(list: readonly T[]): T[] {
  return [...list].sort((a, b) => lastUsedAt(b) - lastUsedAt(a));
}
