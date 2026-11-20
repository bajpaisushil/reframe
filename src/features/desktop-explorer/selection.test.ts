import { describe, expect, it } from "vitest";
import {
  checkState,
  clearChoice,
  countSelection,
  formatCounts,
  isWindowSelected,
  resolveSelection,
  setAppSelected,
  setAppsSelected,
  setWindowSelected,
  toCaptureSelections,
  type Choices,
  type SelectableApp,
} from "./selection";

const app = (key: string, windowIds: string[], suggested = true): SelectableApp => ({
  key,
  suggested,
  windows: windowIds.map((id) => ({ id })),
});

const none: Choices = new Map();

describe("defaults", () => {
  it("selects suggested apps with all their windows and leaves the rest", () => {
    const code = app("exe:/usr/bin/code", ["1", "2"]);
    const tray = app("exe:/usr/bin/tray", ["3"], false);
    expect(resolveSelection(code, none)).toEqual({ windowIds: ["1", "2"], all: true });
    expect(resolveSelection(tray, none)).toBeNull();
    expect(checkState(code, none)).toBe("checked");
    expect(checkState(tray, none)).toBe("unchecked");
  });

  it("an explicit choice wins over the default and can be cleared again", () => {
    const code = app("code", ["1"]);
    const off = setAppSelected(none, code, false);
    expect(checkState(code, off)).toBe("unchecked");
    expect(checkState(code, clearChoice(off, "code"))).toBe("checked");
  });

  it("never mutates the choices it is given", () => {
    const code = app("code", ["1", "2"]);
    const before = new Map(none);
    setAppSelected(none, code, false);
    setWindowSelected(none, code, "1", false);
    setAppsSelected(none, [code], false);
    expect(none).toEqual(before);
  });
});

describe("window subsets", () => {
  const term = app("xterm", ["a", "b", "c"]);

  it("unchecking one window leaves a subset (indeterminate)", () => {
    const choices = setWindowSelected(none, term, "b", false);
    expect(resolveSelection(term, choices)).toEqual({ windowIds: ["a", "c"], all: false });
    expect(checkState(term, choices)).toBe("indeterminate");
    expect(isWindowSelected(term, choices, "b")).toBe(false);
    expect(isWindowSelected(term, choices, "a")).toBe(true);
  });

  it("unchecking the last window deselects the app", () => {
    let choices = setWindowSelected(none, term, "a", false);
    choices = setWindowSelected(choices, term, "b", false);
    choices = setWindowSelected(choices, term, "c", false);
    expect(checkState(term, choices)).toBe("unchecked");
    expect(choices.get("xterm")).toEqual({ kind: "none" });
  });

  it("checking a window of an unselected app selects just that window", () => {
    const off = setAppSelected(none, term, false);
    const choices = setWindowSelected(off, term, "c", true);
    expect(resolveSelection(term, choices)).toEqual({ windowIds: ["c"], all: false });
  });

  it("selecting every window again becomes 'all windows'", () => {
    let choices = setWindowSelected(none, term, "b", false);
    choices = setWindowSelected(choices, term, "b", true);
    expect(choices.get("xterm")).toEqual({ kind: "all" });
  });

  it("keeps window order regardless of click order", () => {
    let choices = setAppSelected(none, term, false);
    choices = setWindowSelected(choices, term, "c", true);
    choices = setWindowSelected(choices, term, "a", true);
    expect(choices.get("xterm")).toEqual({ kind: "some", windowIds: ["a", "c"] });
  });

  it("ignores window ids the app doesn't have", () => {
    const choices = setWindowSelected(none, term, "zzz", false);
    expect(choices.has("xterm")).toBe(false);
  });
});

describe("after a rescan", () => {
  it("drops windows that closed and treats a now-complete subset as all", () => {
    const before = app("xterm", ["a", "b", "c"]);
    const choices = setWindowSelected(none, before, "c", false); // a, b
    const after = app("xterm", ["a", "b"]); // c closed
    expect(resolveSelection(after, choices)).toEqual({ windowIds: ["a", "b"], all: true });
  });

  it("an app whose chosen windows all closed is no longer selected", () => {
    const before = app("xterm", ["a", "b"]);
    const choices = setWindowSelected(none, before, "b", false); // a
    const after = app("xterm", ["b", "d"]);
    expect(resolveSelection(after, choices)).toBeNull();
  });

  it("new windows join apps chosen as a whole, not subsets", () => {
    const whole = app("code", ["1"]);
    const part = app("term", ["a", "b"]);
    const choices = setWindowSelected(none, part, "b", false);
    expect(resolveSelection(app("code", ["1", "2"]), choices)?.windowIds).toEqual(["1", "2"]);
    expect(resolveSelection(app("term", ["a", "b", "x"]), choices)?.windowIds).toEqual(["a"]);
    expect(resolveSelection(whole, choices)?.all).toBe(true);
  });
});

describe("capture selections and counts", () => {
  const code = app("code", ["1", "2"]);
  const term = app("term", ["a", "b", "c"]);
  const tray = app("tray", ["t"], false);
  const apps = [code, term, tray];

  it("sends 'all windows' as an empty list and keeps snapshot order", () => {
    let choices = setWindowSelected(none, term, "b", false);
    choices = setAppSelected(choices, tray, true);
    expect(toCaptureSelections(apps, choices)).toEqual([
      { appKey: "code", windowIds: [] },
      { appKey: "term", windowIds: ["a", "c"] },
      { appKey: "tray", windowIds: [] },
    ]);
  });

  it("counts selected apps and windows", () => {
    const choices = setWindowSelected(none, term, "b", false);
    expect(countSelection(apps, choices)).toEqual({ apps: 2, windows: 4 });
    expect(countSelection(apps, setAppsSelected(choices, apps, false))).toEqual({ apps: 0, windows: 0 });
  });

  it("formats counts with singulars and plurals", () => {
    expect(formatCounts({ apps: 3, windows: 7 })).toBe("3 apps · 7 windows");
    expect(formatCounts({ apps: 1, windows: 1 })).toBe("1 app · 1 window");
  });
});
