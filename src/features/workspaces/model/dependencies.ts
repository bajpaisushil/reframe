// "Starts after" relationships between the applications of a workspace. An application waits
// for the ones it depends on, so a dependency that leads back to itself (a cycle) could never
// start; the editor never offers such a choice. Pure (tested in dependencies.test.ts).

import type { ApplicationEntry } from "@/types";

type Entry = Pick<ApplicationEntry, "id" | "displayName" | "dependsOn">;

/**
 * The chain of "starts after" links from `fromId` to `toId` (ids, both ends included), or null
 * when `fromId` does not wait for `toId`, directly or indirectly. Breadth-first, so the chain is
 * one of the shortest.
 */
export function dependencyPath(entries: readonly Entry[], fromId: string, toId: string): string[] | null {
  const byId = new Map(entries.map((entry) => [entry.id, entry]));
  const previous = new Map<string, string | null>([[fromId, null]]);
  const queue = [fromId];
  while (queue.length > 0) {
    const current = queue.shift() as string;
    if (current === toId) {
      const path: string[] = [];
      for (let id: string | null = current; id !== null; id = previous.get(id) ?? null) path.unshift(id);
      return path;
    }
    for (const next of byId.get(current)?.dependsOn ?? []) {
      if (!previous.has(next) && byId.has(next)) {
        previous.set(next, current);
        queue.push(next);
      }
    }
  }
  return null;
}

/** Would making `entryId` start after `candidateId` create a loop (or depend on itself)? */
export function wouldCreateCycle(entries: readonly Entry[], entryId: string, candidateId: string): boolean {
  return entryId === candidateId || dependencyPath(entries, candidateId, entryId) !== null;
}

export interface DependencyOption<T extends Entry = Entry> {
  entry: T;
  selected: boolean;
  /** Why it can't be chosen (it already waits for this application); null when it can. */
  disabledReason: string | null;
}

/**
 * The applications `entryId` could start after, in workspace order. Choices that would create a
 * loop are disabled with a plain explanation; a selected choice is never disabled, so a loop that
 * already exists in saved data can still be undone.
 */
export function dependencyOptions<T extends Entry>(entries: readonly T[], entryId: string): DependencyOption<T>[] {
  const self = entries.find((entry) => entry.id === entryId);
  if (!self) return [];
  const names = new Map(entries.map((entry) => [entry.id, entry.displayName]));
  return entries
    .filter((entry) => entry.id !== entryId)
    .map((entry) => {
      const selected = self.dependsOn.includes(entry.id);
      let disabledReason: string | null = null;
      if (!selected) {
        const path = dependencyPath(entries, entry.id, entryId);
        if (path) {
          const via = path.slice(1, -1).map((id) => names.get(id) ?? "another app");
          disabledReason =
            via.length === 0
              ? `Already starts after ${self.displayName}`
              : `Already starts after ${self.displayName} (via ${via.join(", ")})`;
        }
      }
      return { entry, selected, disabledReason };
    });
}

/** Toggle one dependency, keeping workspace order and refusing choices that would loop. */
export function toggleDependency(
  entries: readonly Entry[],
  entryId: string,
  dependencyId: string,
  selected: boolean,
): string[] {
  const self = entries.find((entry) => entry.id === entryId);
  if (!self) return [];
  const current = new Set(self.dependsOn);
  if (selected) {
    if (current.has(dependencyId) || wouldCreateCycle(entries, entryId, dependencyId)) return [...self.dependsOn];
    current.add(dependencyId);
  } else {
    current.delete(dependencyId);
  }
  const order = entries.map((entry) => entry.id);
  // Known ids in workspace order; unknown ones (stale references) stay at the end.
  return [...current].sort((a, b) => {
    const ia = order.indexOf(a);
    const ib = order.indexOf(b);
    return (ia === -1 ? Number.MAX_SAFE_INTEGER : ia) - (ib === -1 ? Number.MAX_SAFE_INTEGER : ib);
  });
}

export interface DependencyLabel {
  id: string;
  /** Display name, or null when the referenced application is no longer in the workspace. */
  name: string | null;
}

/** What an application waits for, for "After VS Code" chips. */
export function dependencyLabels(entries: readonly Entry[], entry: Entry): DependencyLabel[] {
  const names = new Map(entries.map((e) => [e.id, e.displayName]));
  return entry.dependsOn.map((id) => ({ id, name: names.get(id) ?? null }));
}

/**
 * The order a restore starts the apps that are on, mirroring the backend planner: list order,
 * except that an app waits for the apps it starts after. Dependencies on apps that are off count
 * as satisfied. Apps caught in a loop (or depending on themselves) can't start and are left out.
 */
export function launchOrder(entries: readonly (Entry & { enabled: boolean })[]): string[] {
  const planned = entries.filter((entry) => entry.enabled);
  const plannedIds = new Set(planned.map((entry) => entry.id));
  const position = new Map(entries.map((entry, index) => [entry.id, index]));
  const selfDependent = new Set(planned.filter((entry) => entry.dependsOn.includes(entry.id)).map((entry) => entry.id));
  const waitingFor = new Map(
    planned.map((entry) => [entry.id, new Set(entry.dependsOn.filter((id) => id !== entry.id && plannedIds.has(id)))]),
  );
  const ready = planned.filter((e) => !selfDependent.has(e.id) && waitingFor.get(e.id)?.size === 0).map((e) => e.id);
  const order: string[] = [];
  while (ready.length > 0) {
    ready.sort((a, b) => (position.get(a) ?? 0) - (position.get(b) ?? 0));
    const next = ready.shift() as string;
    order.push(next);
    for (const entry of planned) {
      const waits = waitingFor.get(entry.id);
      if (waits?.delete(next) && waits.size === 0 && !selfDependent.has(entry.id)) ready.push(entry.id);
    }
  }
  return order;
}

/** Applications that wait for `entryId` (to warn before removing it). */
export function dependents<T extends Entry>(entries: readonly T[], entryId: string): T[] {
  return entries.filter((entry) => entry.id !== entryId && entry.dependsOn.includes(entryId));
}
