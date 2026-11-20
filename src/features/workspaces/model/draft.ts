// Editing model of the workspace page: a local draft of the workspace that the page edits freely,
// compared with the last saved version to know whether there are unsaved changes. Pure (tested in
// draft.test.ts).

import type { ApplicationEntry, AppState, MonitorInfo, RestoreConfiguration, Workspace } from "@/types";

/** The parts of a workspace the user can change on the workspace page. */
export type EditableWorkspace = Pick<
  Workspace,
  "name" | "description" | "icon" | "color" | "applications" | "monitors" | "restore"
>;

export function editableFields(workspace: Workspace): EditableWorkspace {
  const { name, description, icon, color, applications, monitors, restore } = workspace;
  return { name, description, icon, color, applications, monitors, restore };
}

/** Structural equality for JSON-like values (object key order is ignored). */
export function deepEqual(a: unknown, b: unknown): boolean {
  if (Object.is(a, b)) return true;
  if (typeof a !== "object" || typeof b !== "object" || a === null || b === null) return false;
  if (Array.isArray(a) !== Array.isArray(b)) return false;
  if (Array.isArray(a) && Array.isArray(b)) {
    return a.length === b.length && a.every((item, index) => deepEqual(item, b[index]));
  }
  const left = a as Record<string, unknown>;
  const right = b as Record<string, unknown>;
  // `undefined` members are treated as absent, like JSON does.
  const keys = new Set([...Object.keys(left), ...Object.keys(right)]);
  for (const key of keys) {
    if (!deepEqual(left[key], right[key])) return false;
  }
  return true;
}

export interface DraftState {
  /** The last version known to be saved. */
  saved: Workspace;
  /** The working copy the page shows and edits. */
  draft: Workspace;
}

export type DetailsPatch = Partial<Pick<Workspace, "name" | "description" | "icon" | "color">>;

export type DraftAction =
  /** Fresh data from the backend: always the new saved version; adopted as the draft when clean. */
  | { type: "load"; workspace: Workspace }
  /** A save finished. `submitted` is the draft that was sent (edits made meanwhile are kept). */
  | { type: "saved"; workspace: Workspace; submitted: Workspace }
  | { type: "discard" }
  | { type: "setDetails"; patch: DetailsPatch }
  | { type: "setRestore"; patch: Partial<RestoreConfiguration> }
  | { type: "moveEntry"; from: number; to: number }
  | { type: "updateEntry"; entryId: string; update: (entry: ApplicationEntry) => ApplicationEntry }
  | { type: "removeEntry"; entryId: string }
  | { type: "addEntries"; entries: readonly ApplicationEntry[]; monitors?: readonly MonitorInfo[] };

export function initDraft(workspace: Workspace): DraftState {
  return { saved: workspace, draft: workspace };
}

/** Same entries with the same content, in any order (order doesn't affect restore readiness). */
export function sameEntriesIgnoringOrder(a: readonly ApplicationEntry[], b: readonly ApplicationEntry[]): boolean {
  if (a.length !== b.length) return false;
  const byId = new Map(b.map((entry) => [entry.id, entry]));
  return a.every((entry) => deepEqual(entry, byId.get(entry.id)));
}

export function hasChanges(draft: Workspace, saved: Workspace): boolean {
  return !deepEqual(editableFields(draft), editableFields(saved));
}

export function isDirty(state: DraftState): boolean {
  return hasChanges(state.draft, state.saved);
}

/** Move one item (a new array; out-of-range indexes leave the list unchanged). */
export function moveItem<T>(list: readonly T[], from: number, to: number): T[] {
  if (from === to || from < 0 || to < 0 || from >= list.length || to >= list.length) return list.slice();
  const next = list.slice();
  const [item] = next.splice(from, 1);
  next.splice(to, 0, item);
  return next;
}

function withDraft(state: DraftState, draft: Workspace): DraftState {
  return draft === state.draft ? state : { ...state, draft };
}

export function draftReducer(state: DraftState, action: DraftAction): DraftState {
  const { draft } = state;
  switch (action.type) {
    case "load":
      if (action.workspace === state.saved) return state;
      return { saved: action.workspace, draft: isDirty(state) ? draft : action.workspace };
    case "saved": {
      const untouched = deepEqual(editableFields(draft), editableFields(action.submitted));
      return { saved: action.workspace, draft: untouched ? action.workspace : draft };
    }
    case "discard":
      return withDraft(state, state.saved);
    case "setDetails":
      return withDraft(state, { ...draft, ...action.patch });
    case "setRestore":
      return withDraft(state, { ...draft, restore: { ...draft.restore, ...action.patch } });
    case "moveEntry":
      return withDraft(state, { ...draft, applications: moveItem(draft.applications, action.from, action.to) });
    case "updateEntry": {
      let changed = false;
      const applications = draft.applications.map((entry) => {
        if (entry.id !== action.entryId) return entry;
        const next = action.update(entry);
        if (next !== entry) changed = true;
        return next;
      });
      return changed ? withDraft(state, { ...draft, applications }) : state;
    }
    case "removeEntry": {
      if (!draft.applications.some((entry) => entry.id === action.entryId)) return state;
      const applications = draft.applications
        .filter((entry) => entry.id !== action.entryId)
        .map((entry) =>
          entry.dependsOn.includes(action.entryId)
            ? { ...entry, dependsOn: entry.dependsOn.filter((id) => id !== action.entryId) }
            : entry,
        );
      return withDraft(state, { ...draft, applications });
    }
    case "addEntries": {
      const known = new Set(draft.applications.map((entry) => entry.id));
      const added = action.entries.filter((entry) => !known.has(entry.id));
      const knownMonitors = new Set(draft.monitors.map((monitor) => monitor.id));
      const newMonitors = (action.monitors ?? []).filter((monitor) => !knownMonitors.has(monitor.id));
      if (added.length === 0 && newMonitors.length === 0) return state;
      return withDraft(state, {
        ...draft,
        applications: [...draft.applications, ...added],
        monitors: [...draft.monitors, ...newMonitors],
      });
    }
    default:
      return state;
  }
}

function blankToNull(value: string | null): string | null {
  return value === null || value.trim() === "" ? null : value;
}

function cleanState(state: AppState): AppState {
  switch (state.kind) {
    case "terminal":
      return {
        ...state,
        sessions: state.sessions.map((session) => {
          const startupCommand = blankToNull(session.startupCommand);
          return {
            ...session,
            profile: blankToNull(session.profile),
            startupCommand,
            startupCommandApproved: startupCommand !== null && session.startupCommandApproved,
          };
        }),
      };
    case "media":
      return { ...state, uri: blankToNull(state.uri), label: blankToNull(state.label) };
    case "browser":
      return { ...state, windows: state.windows.filter((window) => window.tabs.length > 0) };
    default:
      return state;
  }
}

/**
 * Tidy a draft before it is sent: drop blank arguments and blank optional text, browser windows
 * without tabs, and dependencies on applications that are no longer in the workspace.
 */
export function prepareForSave(workspace: Workspace): Workspace {
  const ids = new Set(workspace.applications.map((entry) => entry.id));
  return {
    ...workspace,
    applications: workspace.applications.map((entry) => ({
      ...entry,
      dependsOn: entry.dependsOn.filter((id) => ids.has(id) && id !== entry.id),
      launch: {
        ...entry.launch,
        arguments: entry.launch.arguments.filter((argument) => argument.trim() !== ""),
        workingDirectory: blankToNull(entry.launch.workingDirectory),
      },
      state: cleanState(entry.state),
    })),
  };
}
