// Pure helpers for the import mapping screen: editing folder mappings, describing launch targets,
// tracking the user's per-app choices and building the ImportResolution sent to the backend.
// Tested in importModel.test.ts.

import type {
  AppResolution,
  ImportApplication,
  ImportPreview,
  ImportResolution,
  LaunchTarget,
  PackageKind,
  PathMapping,
} from "@/types";

/** Same limit as workspace names in the engine. */
export const MAX_NAME_LENGTH = 80;

// ---------------------------------------------------------------------------------------------
// Folder mappings
// ---------------------------------------------------------------------------------------------

/** One editable row of the folder mapping editor (`key` keeps React state stable). */
export interface MappingRow extends PathMapping {
  key: string;
}

export type KeyFactory = () => string;

let rowCounter = 0;
/** Default key factory for the app (tests pass their own). */
export const nextRowKey: KeyFactory = () => `row-${++rowCounter}`;

export function rowsFromMappings(mappings: readonly PathMapping[], makeKey: KeyFactory = nextRowKey): MappingRow[] {
  return mappings.map((m) => ({ key: makeKey(), fromPrefix: m.fromPrefix, toPrefix: m.toPrefix }));
}

export function addMappingRow(rows: readonly MappingRow[], makeKey: KeyFactory = nextRowKey, initial: Partial<PathMapping> = {}): MappingRow[] {
  return [...rows, { key: makeKey(), fromPrefix: initial.fromPrefix ?? "", toPrefix: initial.toPrefix ?? "" }];
}

export function updateMappingRow(rows: readonly MappingRow[], key: string, patch: Partial<PathMapping>): MappingRow[] {
  return rows.map((row) => (row.key === key ? { ...row, ...patch } : row));
}

export function removeMappingRow(rows: readonly MappingRow[], key: string): MappingRow[] {
  return rows.filter((row) => row.key !== key);
}

const looksLikeWindowsPath = (path: string) => /^[a-zA-Z]:/.test(path) || path.includes("\\");

/** Trimmed, without trailing separators (but keeping a bare root such as "/" or "C:\"). */
export function normalizeFolder(path: string): string {
  const trimmed = path.trim();
  const stripped = trimmed.replace(/[\\/]+$/, "");
  if (stripped === "" || /^[a-zA-Z]:$/.test(stripped)) return trimmed;
  return stripped;
}

function folderKey(path: string): string {
  const normalized = normalizeFolder(path);
  return looksLikeWindowsPath(normalized) ? normalized.toLowerCase().replace(/\//g, "\\") : normalized;
}

export interface MappingProblem {
  /** `incomplete`: only one side filled in; `duplicate`: the folder is already mapped above. */
  kind: "incomplete" | "duplicate";
  message: string;
}

/**
 * Problems by row key. Empty rows are fine (they are left out); a row with only one side, or a
 * source folder already mapped by an earlier row, is not.
 */
export function mappingProblems(rows: readonly MappingRow[]): Map<string, MappingProblem> {
  const problems = new Map<string, MappingProblem>();
  const seen = new Set<string>();
  for (const row of rows) {
    const from = row.fromPrefix.trim();
    const to = row.toPrefix.trim();
    if (from === "" && to === "") continue;
    if (from === "") {
      problems.set(row.key, { kind: "incomplete", message: "Enter the folder from the other computer." });
      continue;
    }
    if (to === "") {
      problems.set(row.key, { kind: "incomplete", message: "Enter where that folder is on this computer." });
      continue;
    }
    const key = folderKey(from);
    if (seen.has(key)) problems.set(row.key, { kind: "duplicate", message: "This folder is already mapped above." });
    seen.add(key);
  }
  return problems;
}

/** The mappings to send: complete rows only, trimmed, first row wins for a repeated folder. */
export function mappingsForImport(rows: readonly MappingRow[]): PathMapping[] {
  const problems = mappingProblems(rows);
  return rows
    .filter((row) => !problems.has(row.key) && row.fromPrefix.trim() !== "" && row.toPrefix.trim() !== "")
    .map((row) => ({ fromPrefix: normalizeFolder(row.fromPrefix), toPrefix: normalizeFolder(row.toPrefix) }));
}

/** A path from the other computer falls under one of the mappings' source folders. */
export function isPathMapped(path: string, mappings: readonly PathMapping[]): boolean {
  const target = folderKey(path);
  return mappings.some((mapping) => {
    const from = folderKey(mapping.fromPrefix);
    if (from === "") return false;
    if (target === from) return true;
    const separator = looksLikeWindowsPath(from) ? "\\" : "/";
    const prefix = from.endsWith(separator) ? from : `${from}${separator}`;
    return target.startsWith(prefix);
  });
}

// ---------------------------------------------------------------------------------------------
// Applications
// ---------------------------------------------------------------------------------------------

const PACKAGE_KIND_LABEL: Record<PackageKind, string> = {
  macBundle: "App",
  windowsPackage: "Windows app",
  flatpak: "Flatpak",
  snap: "Snap",
  appImage: "AppImage",
};

/** What a launch target points at, for display in mono. */
export function targetLabel(target: LaunchTarget): string {
  switch (target.kind) {
    case "executable":
      return target.path;
    case "package":
      return target.package.path ?? `${PACKAGE_KIND_LABEL[target.package.kind]}: ${target.package.id}`;
    case "open":
      return target.target;
  }
}

export function resolutionTarget(resolution: AppResolution): LaunchTarget | null {
  return resolution.kind === "unresolved" ? null : resolution.target;
}

export function sameTarget(a: LaunchTarget | null, b: LaunchTarget | null): boolean {
  if (!a || !b) return a === b;
  return targetLabel(a) === targetLabel(b) && a.kind === b.kind;
}

/** The user's decision for one app: `target: null` keeps Reframe's automatic match. */
export interface AppChoice {
  target: LaunchTarget | null;
  skip: boolean;
}

export type AppChoices = Readonly<Record<string, AppChoice>>;

const NO_CHOICE: AppChoice = { target: null, skip: false };

export function choiceFor(choices: AppChoices, entryId: string): AppChoice {
  return choices[entryId] ?? NO_CHOICE;
}

export function withChoice(choices: AppChoices, entryId: string, patch: Partial<AppChoice>): AppChoices {
  return { ...choices, [entryId]: { ...choiceFor(choices, entryId), ...patch } };
}

/** What will be launched for an app (the user's pick, else the automatic match). */
export function effectiveTarget(app: Pick<ImportApplication, "resolution">, choice: AppChoice): LaunchTarget | null {
  return choice.target ?? resolutionTarget(app.resolution);
}

/** Still needs a decision: no app to start and not skipped. */
export function needsAttention(app: Pick<ImportApplication, "resolution">, choice: AppChoice): boolean {
  return !choice.skip && effectiveTarget(app, choice) === null;
}

export interface TargetOption {
  /** Stable value for a select: "auto", "alt:<n>" or "custom". */
  value: string;
  label: string;
  target: LaunchTarget | null;
}

/**
 * Choices for an app's target select: the automatic match, the other candidates found on this
 * computer, and a program the user picked themselves. Duplicates are listed once.
 */
export function targetOptions(app: Pick<ImportApplication, "resolution" | "alternatives">, choice: AppChoice): TargetOption[] {
  const options: TargetOption[] = [];
  const automatic = resolutionTarget(app.resolution);
  if (automatic) options.push({ value: "auto", label: targetLabel(automatic), target: automatic });
  app.alternatives.forEach((alternative, index) => {
    if (options.some((o) => sameTarget(o.target, alternative))) return;
    options.push({ value: `alt:${index}`, label: targetLabel(alternative), target: alternative });
  });
  if (choice.target && !options.some((o) => sameTarget(o.target, choice.target))) {
    options.push({ value: "custom", label: targetLabel(choice.target), target: choice.target });
  }
  return options;
}

/** The select value matching the current choice. */
export function selectedTargetValue(options: readonly TargetOption[], app: Pick<ImportApplication, "resolution">, choice: AppChoice): string {
  const current = effectiveTarget(app, choice);
  return options.find((o) => sameTarget(o.target, current))?.value ?? "";
}

export function validateImportName(name: string): string | null {
  const trimmed = name.trim();
  if (trimmed === "") return "Give the workspace a name.";
  if ([...trimmed].length > MAX_NAME_LENGTH) return `Use at most ${MAX_NAME_LENGTH} characters.`;
  return null;
}

/** The user's decisions, as the backend expects them. */
export function buildResolution(preview: ImportPreview, name: string, choices: AppChoices, rows: readonly MappingRow[]): ImportResolution {
  const trimmed = name.trim();
  return {
    filePath: preview.filePath,
    name: trimmed !== "" && trimmed !== preview.name ? trimmed : null,
    appMappings: preview.applications.map((app) => {
      const choice = choiceFor(choices, app.entryId);
      // Picking the automatic match again is the same as not choosing.
      const target = choice.target && !sameTarget(choice.target, resolutionTarget(app.resolution)) ? choice.target : null;
      return { entryId: app.entryId, target, skip: choice.skip };
    }),
    pathMappings: mappingsForImport(rows),
  };
}

export interface ImportCounts {
  total: number;
  included: number;
  skipped: number;
  attention: number;
}

export function importCounts(preview: Pick<ImportPreview, "applications">, choices: AppChoices): ImportCounts {
  let skipped = 0;
  let attention = 0;
  for (const app of preview.applications) {
    const choice = choiceFor(choices, app.entryId);
    if (choice.skip) skipped += 1;
    else if (needsAttention(app, choice)) attention += 1;
  }
  const total = preview.applications.length;
  return { total, included: total - skipped, skipped, attention };
}
