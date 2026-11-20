// The user's "never suggest" list (`AppSettings.excludedApps`) as seen from Explore
// (tested in exclusions.test.ts).
//
// The backend matches entries case-insensitively against an application's id and the executable
// stem, file name, full path, process name and package id of its processes; entries written as
// app ids (`exe:slack`) also match the bare stem. Explore knows the app id, the main executable
// and the package, so removing entries is best effort: Explore rescans afterwards and reports an
// app that is still excluded by an entry it could not attribute (e.g. a process name).

import type { DiscoveredApplication } from "@/types";

export type ExclusionTarget = Pick<DiscoveredApplication, "appId" | "executablePath" | "package">;

function fileName(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}

/** `name.ext` → `name`; names starting with a dot and names without one are kept. */
function stripExtension(name: string): string {
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(0, dot) : name;
}

function bare(entry: string): string {
  if (entry.startsWith("exe:")) return entry.slice(4);
  if (entry.startsWith("pkg:")) return entry.slice(4);
  return entry;
}

/** Lower-cased names an exclusion entry can use to refer to this app. */
export function exclusionCandidates(app: ExclusionTarget): string[] {
  const candidates = new Set<string>();
  const add = (value: string | null | undefined) => {
    const v = value?.trim().toLowerCase();
    if (v) candidates.add(v);
  };
  add(app.appId);
  if (app.executablePath) {
    const name = fileName(app.executablePath.trim());
    add(app.executablePath);
    add(name);
    add(stripExtension(name));
  }
  add(app.package?.id);
  return [...candidates];
}

/** True when `entry` refers to this app (as far as Explore can tell). */
export function entryMatches(entry: string, app: ExclusionTarget): boolean {
  const normalized = entry.trim().toLowerCase();
  if (!normalized) return false;
  const candidates = exclusionCandidates(app);
  return candidates.includes(normalized) || candidates.includes(bare(normalized));
}

/** The list with this app added (by app id) unless an entry already covers it. */
export function addExclusion(list: readonly string[], app: ExclusionTarget): string[] {
  if (list.some((entry) => entryMatches(entry, app))) return [...list];
  return [...list, app.appId];
}

/** The list without the entries that refer to this app. */
export function removeExclusion(list: readonly string[], app: ExclusionTarget): string[] {
  return list.filter((entry) => !entryMatches(entry, app));
}
