import { describe, expect, it } from "vitest";
import {
  dependencyLabels,
  dependencyOptions,
  dependencyPath,
  dependents,
  launchOrder,
  toggleDependency,
  wouldCreateCycle,
} from "./dependencies";

const app = (id: string, dependsOn: string[] = []) => ({ id, displayName: id.toUpperCase(), dependsOn });

// code <- terminal <- browser ; docs is independent
const entries = [app("code"), app("terminal", ["code"]), app("browser", ["terminal"]), app("docs")];

describe("dependencyPath", () => {
  it("follows starts-after links transitively", () => {
    expect(dependencyPath(entries, "browser", "code")).toEqual(["browser", "terminal", "code"]);
    expect(dependencyPath(entries, "code", "browser")).toBeNull();
    expect(dependencyPath(entries, "docs", "code")).toBeNull();
  });

  it("terminates on existing loops and ignores unknown ids", () => {
    const looped = [app("a", ["b", "ghost"]), app("b", ["a"]), app("c")];
    expect(dependencyPath(looped, "a", "c")).toBeNull();
    expect(dependencyPath(looped, "a", "b")).toEqual(["a", "b"]);
  });
});

describe("wouldCreateCycle", () => {
  it("rejects self-dependencies and choices that lead back", () => {
    expect(wouldCreateCycle(entries, "code", "code")).toBe(true);
    expect(wouldCreateCycle(entries, "code", "terminal")).toBe(true);
    expect(wouldCreateCycle(entries, "code", "browser")).toBe(true);
    expect(wouldCreateCycle(entries, "browser", "code")).toBe(false);
    expect(wouldCreateCycle(entries, "code", "docs")).toBe(false);
  });
});

describe("dependencyOptions", () => {
  it("lists the other apps and explains why loops can't be chosen", () => {
    const options = dependencyOptions(entries, "code");
    expect(options.map((o) => [o.entry.id, o.selected, o.disabledReason])).toEqual([
      ["terminal", false, "Already starts after CODE"],
      ["browser", false, "Already starts after CODE (via TERMINAL)"],
      ["docs", false, null],
    ]);
  });

  it("keeps selected choices enabled so an existing loop can be undone", () => {
    const looped = [app("a", ["b"]), app("b", ["a"])];
    expect(dependencyOptions(looped, "a")).toEqual([{ entry: looped[1], selected: true, disabledReason: null }]);
  });

  it("returns nothing for an unknown entry", () => {
    expect(dependencyOptions(entries, "nope")).toEqual([]);
  });
});

describe("toggleDependency", () => {
  it("adds in workspace order and removes", () => {
    const list = [app("a"), app("b"), app("c", ["b"])];
    expect(toggleDependency(list, "c", "a", true)).toEqual(["a", "b"]);
    expect(toggleDependency(list, "c", "b", false)).toEqual([]);
  });

  it("refuses a choice that would loop", () => {
    expect(toggleDependency(entries, "code", "browser", true)).toEqual([]);
  });
});

describe("launchOrder", () => {
  const on = (id: string, dependsOn: string[] = [], enabled = true) => ({ ...app(id, dependsOn), enabled });

  it("follows list order when nothing waits", () => {
    expect(launchOrder([on("a"), on("b"), on("c")])).toEqual(["a", "b", "c"]);
  });

  it("starts an app after the ones it waits for, even when it is listed first", () => {
    expect(launchOrder([on("eyes", ["term"]), on("term")])).toEqual(["term", "eyes"]);
    // Independent apps keep their list order around it.
    expect(launchOrder([on("x"), on("eyes", ["term"]), on("y"), on("term")])).toEqual(["x", "y", "term", "eyes"]);
  });

  it("skips apps that are off and treats waiting for them as satisfied", () => {
    expect(launchOrder([on("a", ["b"]), on("b", [], false), on("c")])).toEqual(["a", "c"]);
  });

  it("leaves out apps caught in a loop", () => {
    expect(launchOrder([on("a", ["b"]), on("b", ["a"]), on("c"), on("d", ["d"])])).toEqual(["c"]);
  });
});

describe("labels and dependents", () => {
  it("names dependencies and flags references to removed apps", () => {
    expect(dependencyLabels(entries, app("x", ["code", "gone"]))).toEqual([
      { id: "code", name: "CODE" },
      { id: "gone", name: null },
    ]);
  });

  it("finds the apps waiting for one", () => {
    expect(dependents(entries, "code").map((e) => e.id)).toEqual(["terminal"]);
    expect(dependents(entries, "docs")).toEqual([]);
  });
});
