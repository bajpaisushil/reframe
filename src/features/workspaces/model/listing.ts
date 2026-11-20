// Searching and sorting the workspace library. Pure (tested in listing.test.ts).

import { lastUsedAt } from "@/lib/workspaces";
import type { WorkspaceSummary } from "@/types";

export type WorkspaceSort = "recent" | "name" | "created";

export const SORT_OPTIONS: ReadonlyArray<{ value: WorkspaceSort; label: string }> = [
  { value: "recent", label: "Recently used" },
  { value: "name", label: "Name" },
  { value: "created", label: "Recently created" },
];

export function isWorkspaceSort(value: unknown): value is WorkspaceSort {
  return SORT_OPTIONS.some((option) => option.value === value);
}

/** Lowercase without accents, so "resume" finds "Résumé". */
export function foldText(value: string): string {
  return value.normalize("NFD").replace(/\p{M}/gu, "").toLocaleLowerCase();
}

type Searchable = Pick<WorkspaceSummary, "name" | "description" | "applicationNames">;

/** Every word of the query appears in the name, the description or an app name. */
export function matchesQuery(workspace: Searchable, query: string): boolean {
  const terms = foldText(query).split(/\s+/).filter(Boolean);
  if (terms.length === 0) return true;
  const haystack = foldText([workspace.name, workspace.description, ...workspace.applicationNames].join("\n"));
  return terms.every((term) => haystack.includes(term));
}

function time(value: string | null | undefined): number {
  if (!value) return 0;
  const ms = Date.parse(value);
  return Number.isNaN(ms) ? 0 : ms;
}

const collator = new Intl.Collator(undefined, { sensitivity: "base", numeric: true });

type Sortable = Searchable & Pick<WorkspaceSummary, "createdAt" | "updatedAt" | "lastRestoredAt">;

export function sortWorkspaces<T extends Sortable>(list: readonly T[], sort: WorkspaceSort): T[] {
  const sorted = [...list];
  switch (sort) {
    case "name":
      return sorted.sort((a, b) => collator.compare(a.name, b.name) || time(b.createdAt) - time(a.createdAt));
    case "created":
      return sorted.sort((a, b) => time(b.createdAt) - time(a.createdAt) || collator.compare(a.name, b.name));
    case "recent":
    default:
      // Stable: ties keep the backend order.
      return sorted.sort((a, b) => lastUsedAt(b) - lastUsedAt(a));
  }
}

export function filterAndSortWorkspaces<T extends Sortable>(list: readonly T[], query: string, sort: WorkspaceSort): T[] {
  return sortWorkspaces(
    list.filter((workspace) => matchesQuery(workspace, query)),
    sort,
  );
}
