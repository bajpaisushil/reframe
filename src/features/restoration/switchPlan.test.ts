import { describe, expect, it } from "vitest";
import type { CloseCandidate, DesktopSnapshot, SwitchPlan, Workspace } from "@/types";
import {
  isCloseSelected,
  isEmptyPlan,
  selectedCloseIds,
  setAllClose,
  sortCloseCandidates,
  windowLookup,
  windowLookups,
  workspaceEntryLookup,
} from "./switchPlan";

function candidate(windowId: string, appName: string, defaultSelected: boolean, title = windowId): CloseCandidate {
  return { windowId, pid: 1, title, appId: appName.toLowerCase(), appName, defaultSelected, reason: "" };
}

function plan(overrides: Partial<SwitchPlan> = {}): SwitchPlan {
  return {
    workspaceId: "ws",
    mode: "replace",
    previousWorkspaceId: "prev",
    launch: [],
    reuse: [],
    close: [candidate("w1", "XTerm", true), candidate("w2", "Firefox", false), candidate("w3", "Clock", true)],
    warnings: [],
    ...overrides,
  };
}

describe("close selection", () => {
  it("starts from the suggested windows", () => {
    expect(selectedCloseIds(plan(), {})).toEqual(["w1", "w3"]);
  });

  it("applies the user's choices on top", () => {
    const overrides = { w1: false, w2: true };
    expect(isCloseSelected(plan().close[0], overrides)).toBe(false);
    expect(selectedCloseIds(plan(), overrides)).toEqual(["w2", "w3"]);
  });

  it("never closes anything outside Replace", () => {
    expect(selectedCloseIds(plan({ mode: "merge" }), { w2: true })).toEqual([]);
    expect(selectedCloseIds(null, {})).toEqual([]);
  });

  it("ignores choices for windows no longer in the plan", () => {
    expect(selectedCloseIds(plan(), { gone: true })).toEqual(["w1", "w3"]);
  });

  it("selects or clears everything", () => {
    const all = setAllClose(plan().close, true);
    expect(selectedCloseIds(plan(), all)).toEqual(["w1", "w2", "w3"]);
    expect(selectedCloseIds(plan(), setAllClose(plan().close, false))).toEqual([]);
  });

  it("lists suggested windows first, then by app and title", () => {
    const sorted = sortCloseCandidates([
      candidate("a", "Zed", false),
      candidate("b", "XTerm", true, "server"),
      candidate("c", "XTerm", true, "build"),
      candidate("d", "Clock", false),
    ]);
    expect(sorted.map((c) => c.windowId)).toEqual(["c", "b", "d", "a"]);
  });
});

describe("plan helpers", () => {
  it("detects plans with nothing to do", () => {
    expect(isEmptyPlan(plan())).toBe(true);
    expect(isEmptyPlan(plan({ launch: [{ entryId: "e", displayName: "X", appId: "x", reason: "Not running" }] }))).toBe(false);
  });

  it("finds icon lookups for workspace entries", () => {
    const workspace = {
      applications: [{ id: "e1", launch: { executable: "/usr/bin/xterm", package: null } }],
    } as unknown as Workspace;
    expect(workspaceEntryLookup(workspace, "e1", "xterm")).toEqual({ appId: "xterm", executablePath: "/usr/bin/xterm", package: null });
    expect(workspaceEntryLookup(undefined, "e1", "xterm")).toEqual({ appId: "xterm", executablePath: null, package: null });
  });

  it("maps running windows to icons from a desktop scan", () => {
    const snapshot = {
      applications: [
        { appId: "xterm", displayName: "XTerm", executablePath: "/usr/bin/xterm", package: null, windows: [{ id: "0x1" }, { id: "0x2" }] },
        { appId: "gedit", displayName: "Text Editor", executablePath: "/usr/bin/gedit", package: null, windows: [{ id: "0x5" }] },
      ],
    } as unknown as DesktopSnapshot;
    const lookups = windowLookups(snapshot);
    expect(windowLookup(lookups, "0x2", "ignored").executablePath).toBe("/usr/bin/xterm");
    expect(windowLookup(lookups, "0x9", "clock")).toEqual({ appId: "clock", executablePath: null, package: null });
    // A window that has closed since the scan: matched by app name.
    expect(windowLookup(lookups, "0x9", "xterm", "XTerm").executablePath).toBe("/usr/bin/xterm");
    // A reused window id now belonging to another app is not trusted.
    expect(windowLookup(lookups, "0x5", "xterm", "XTerm").executablePath).toBe("/usr/bin/xterm");
    expect(windowLookup(lookups, "0x5", "clock", "Clock").executablePath).toBeNull();
    expect(windowLookups(undefined).byWindow.size).toBe(0);
  });
});
