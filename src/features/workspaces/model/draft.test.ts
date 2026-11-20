import { describe, expect, it } from "vitest";
import type { ApplicationEntry, Workspace } from "@/types";
import {
  deepEqual,
  draftReducer,
  hasChanges,
  initDraft,
  isDirty,
  moveItem,
  prepareForSave,
  sameEntriesIgnoringOrder,
  type DraftState,
} from "./draft";

function entry(id: string, overrides: Partial<ApplicationEntry> = {}): ApplicationEntry {
  return {
    id,
    appId: `exe:${id}`,
    displayName: id.toUpperCase(),
    adapterId: "generic",
    enabled: true,
    launch: { executable: `/usr/bin/${id}`, package: null, arguments: [], workingDirectory: null },
    dependsOn: [],
    state: { kind: "none" },
    windows: [],
    options: { delayAfterLaunchMs: null, timeoutMs: null, retryCount: null, restoreWindowPositions: true },
    ...overrides,
  };
}

function workspace(applications: ApplicationEntry[] = [], overrides: Partial<Workspace> = {}): Workspace {
  return {
    id: "ws-1",
    name: "Deep work",
    description: "",
    icon: "🧠",
    color: null,
    createdAt: "2026-11-01T09:00:00Z",
    updatedAt: "2026-11-01T09:00:00Z",
    lastRestoredAt: null,
    applications,
    monitors: [],
    restore: {
      defaultMode: "merge",
      launchDelayMs: 300,
      timeoutMs: 20000,
      retryCount: 1,
      restoreWindowPositions: true,
      maxParallel: 3,
    },
    schemaVersion: 1,
    ...overrides,
  };
}

const run = (state: DraftState, ...actions: Parameters<typeof draftReducer>[1][]) => actions.reduce(draftReducer, state);

describe("deepEqual", () => {
  it("ignores key order and treats undefined members as absent", () => {
    expect(deepEqual({ a: 1, b: [1, { c: 2 }] }, { b: [1, { c: 2 }], a: 1 })).toBe(true);
    expect(deepEqual({ a: 1, b: undefined }, { a: 1 })).toBe(true);
    expect(deepEqual([1, 2], [2, 1])).toBe(false);
    expect(deepEqual({ a: null }, { a: 0 })).toBe(false);
    expect(deepEqual([], {})).toBe(false);
  });
});

describe("sameEntriesIgnoringOrder", () => {
  it("ignores order but not content", () => {
    const a = entry("a");
    const b = entry("b");
    expect(sameEntriesIgnoringOrder([a, b], [b, a])).toBe(true);
    expect(sameEntriesIgnoringOrder([a, b], [a, { ...b, enabled: false }])).toBe(false);
    expect(sameEntriesIgnoringOrder([a], [a, b])).toBe(false);
  });
});

describe("moveItem", () => {
  it("moves forwards and backwards without mutating", () => {
    const list = ["a", "b", "c", "d"];
    expect(moveItem(list, 0, 2)).toEqual(["b", "c", "a", "d"]);
    expect(moveItem(list, 3, 1)).toEqual(["a", "d", "b", "c"]);
    expect(list).toEqual(["a", "b", "c", "d"]);
  });

  it("ignores out-of-range indexes", () => {
    expect(moveItem(["a", "b"], 0, 5)).toEqual(["a", "b"]);
    expect(moveItem(["a", "b"], -1, 0)).toEqual(["a", "b"]);
  });
});

describe("draftReducer", () => {
  it("starts clean and becomes dirty on edits", () => {
    const state = initDraft(workspace([entry("a")]));
    expect(isDirty(state)).toBe(false);
    const edited = run(state, { type: "setDetails", patch: { name: "Focus" } });
    expect(isDirty(edited)).toBe(true);
    expect(edited.saved.name).toBe("Deep work");
  });

  it("is clean again when an edit is undone by hand", () => {
    const state = initDraft(workspace([entry("a")]));
    const edited = run(state, { type: "setRestore", patch: { retryCount: 3 } }, { type: "setRestore", patch: { retryCount: 1 } });
    expect(isDirty(edited)).toBe(false);
  });

  it("reorders entries (the launch order)", () => {
    const state = initDraft(workspace([entry("a"), entry("b"), entry("c")]));
    const moved = run(state, { type: "moveEntry", from: 2, to: 0 });
    expect(moved.draft.applications.map((e) => e.id)).toEqual(["c", "a", "b"]);
    expect(isDirty(moved)).toBe(true);
    const back = run(moved, { type: "moveEntry", from: 0, to: 2 });
    expect(isDirty(back)).toBe(false);
  });

  it("updates one entry and keeps the state object when nothing changed", () => {
    const state = initDraft(workspace([entry("a"), entry("b")]));
    const same = run(state, { type: "updateEntry", entryId: "a", update: (e) => e });
    expect(same).toBe(state);
    const off = run(state, { type: "updateEntry", entryId: "b", update: (e) => ({ ...e, enabled: false }) });
    expect(off.draft.applications[1].enabled).toBe(false);
    expect(off.draft.applications[0]).toBe(state.draft.applications[0]);
  });

  it("removing an entry also removes it from other entries' dependencies", () => {
    const state = initDraft(workspace([entry("a"), entry("b", { dependsOn: ["a"] }), entry("c", { dependsOn: ["a", "b"] })]));
    const removed = run(state, { type: "removeEntry", entryId: "a" });
    expect(removed.draft.applications.map((e) => [e.id, e.dependsOn])).toEqual([
      ["b", []],
      ["c", ["b"]],
    ]);
  });

  it("appends added entries once and merges display layouts by id", () => {
    const monitor = { id: "HDMI-1", name: "HDMI-1", bounds: { x: 0, y: 0, width: 1920, height: 1080 }, workArea: { x: 0, y: 0, width: 1920, height: 1050 }, scaleFactor: 1, isPrimary: true };
    const state = initDraft(workspace([entry("a")], { monitors: [monitor] }));
    const added = run(state, { type: "addEntries", entries: [entry("a"), entry("b")], monitors: [monitor, { ...monitor, id: "DP-1", isPrimary: false }] });
    expect(added.draft.applications.map((e) => e.id)).toEqual(["a", "b"]);
    expect(added.draft.monitors.map((m) => m.id)).toEqual(["HDMI-1", "DP-1"]);
  });

  it("discard returns to the saved version", () => {
    const state = run(initDraft(workspace([entry("a")])), { type: "setDetails", patch: { icon: "🚀" } }, { type: "discard" });
    expect(isDirty(state)).toBe(false);
    expect(state.draft.icon).toBe("🧠");
  });

  it("adopts fresh backend data only while there are no unsaved changes", () => {
    const base = workspace([entry("a")]);
    const restored = { ...base, lastRestoredAt: "2026-11-20T10:00:00Z", updatedAt: "2026-11-20T10:00:00Z" };
    const clean = run(initDraft(base), { type: "load", workspace: restored });
    expect(clean.draft).toBe(restored);

    const dirty = run(initDraft(base), { type: "setDetails", patch: { name: "Focus" } }, { type: "load", workspace: restored });
    expect(dirty.saved).toBe(restored);
    expect(dirty.draft.name).toBe("Focus");
    // Server-owned fields alone never count as unsaved changes.
    expect(hasChanges({ ...base, updatedAt: "2030-01-01T00:00:00Z" }, base)).toBe(false);
  });

  it("after saving, takes the saved version unless the draft changed meanwhile", () => {
    const state = run(initDraft(workspace([entry("a")])), { type: "setDetails", patch: { name: " Focus " } });
    const submitted = state.draft;
    const serverCopy = { ...submitted, name: "Focus", updatedAt: "2026-11-20T11:00:00Z" };
    const saved = run(state, { type: "saved", workspace: serverCopy, submitted });
    expect(saved.draft).toBe(serverCopy);
    expect(isDirty(saved)).toBe(false);

    const editedMeanwhile = run(state, { type: "setDetails", patch: { description: "Writing" } }, { type: "saved", workspace: serverCopy, submitted });
    expect(editedMeanwhile.saved).toBe(serverCopy);
    expect(editedMeanwhile.draft.description).toBe("Writing");
    expect(isDirty(editedMeanwhile)).toBe(true);
  });
});

describe("prepareForSave", () => {
  it("drops blank arguments, stale dependencies and empty browser windows", () => {
    const ws = workspace([
      entry("a", {
        launch: { executable: "/usr/bin/a", package: null, arguments: ["--new", " ", ""], workingDirectory: "  " },
        dependsOn: ["gone", "b", "a"],
        state: {
          kind: "browser",
          profile: null,
          windows: [
            { tabs: [], activeTab: null, windowTitle: null },
            { tabs: [{ url: "https://example.com/", title: "example.com", pinned: false }], activeTab: 0, windowTitle: null },
          ],
        },
      }),
      entry("b"),
    ]);
    const prepared = prepareForSave(ws).applications[0];
    expect(prepared.launch.arguments).toEqual(["--new"]);
    expect(prepared.launch.workingDirectory).toBeNull();
    expect(prepared.dependsOn).toEqual(["b"]);
    expect(prepared.state.kind === "browser" && prepared.state.windows.length).toBe(1);
  });

  it("never keeps an approval for a blank startup command", () => {
    const ws = workspace([
      entry("t", {
        state: {
          kind: "terminal",
          sessions: [{ workingDirectory: null, profile: " ", startupCommand: "   ", startupCommandApproved: true, windowTitle: null }],
        },
      }),
    ]);
    const state = prepareForSave(ws).applications[0].state;
    expect(state).toEqual({
      kind: "terminal",
      sessions: [{ workingDirectory: null, profile: null, startupCommand: null, startupCommandApproved: false, windowTitle: null }],
    });
  });
});
