import { describe, expect, it } from "vitest";
import type { BrowserWindow, TerminalSession } from "@/types";
import {
  addBrowserTab,
  launchFromTarget,
  launchTargetText,
  ordinal,
  pathLeaf,
  removeBrowserTab,
  stateLine,
  stateSummary,
  unapprovedCommands,
  updateTerminalSession,
} from "./entries";

const tab = (url: string) => ({ url, title: url, pinned: false });

describe("browser tabs", () => {
  const windows: BrowserWindow[] = [
    { tabs: [tab("a"), tab("b"), tab("c")], activeTab: 2, windowTitle: null },
    { tabs: [tab("d")], activeTab: 0, windowTitle: null },
  ];

  it("keeps the selected tab selected when an earlier tab is removed", () => {
    expect(removeBrowserTab(windows, 0, 0)[0]).toEqual({ tabs: [tab("b"), tab("c")], activeTab: 1, windowTitle: null });
  });

  it("selects a neighbour when the selected tab is removed", () => {
    expect(removeBrowserTab(windows, 0, 2)[0].activeTab).toBe(1);
  });

  it("removes a window with no tabs left", () => {
    expect(removeBrowserTab(windows, 1, 0)).toHaveLength(1);
  });

  it("adds tabs, creating a first window when needed", () => {
    expect(addBrowserTab(windows, 1, { url: "e", title: "E" })[1].tabs.map((t) => t.url)).toEqual(["d", "e"]);
    expect(addBrowserTab([], 0, { url: "e", title: "E" })).toEqual([
      { tabs: [{ url: "e", title: "E", pinned: false }], activeTab: 0, windowTitle: null },
    ]);
  });
});

describe("terminal sessions", () => {
  const approved: TerminalSession = {
    workingDirectory: "/home/me",
    profile: null,
    startupCommand: "npm run dev",
    startupCommandApproved: true,
    windowTitle: null,
  };

  it("withdraws approval when the command changes", () => {
    expect(updateTerminalSession(approved, { startupCommand: "npm run dev -- --host" }).startupCommandApproved).toBe(false);
    expect(updateTerminalSession(approved, { workingDirectory: "/tmp" }).startupCommandApproved).toBe(true);
  });

  it("never approves an empty command", () => {
    expect(updateTerminalSession({ ...approved, startupCommand: null, startupCommandApproved: false }, { startupCommandApproved: true }).startupCommandApproved).toBe(false);
  });

  it("counts commands that will not run", () => {
    expect(
      unapprovedCommands({
        kind: "terminal",
        sessions: [approved, { ...approved, startupCommandApproved: false }, { ...approved, startupCommand: " ", startupCommandApproved: false }],
      }),
    ).toBe(1);
  });
});

describe("labels", () => {
  it("summarises saved content", () => {
    expect(stateSummary({ kind: "browser", profile: null, windows: [{ tabs: [tab("a"), tab("b")], activeTab: 0, windowTitle: null }] })).toEqual({
      label: "Tabs",
      count: 2,
    });
    expect(stateSummary({ kind: "none" })).toBeNull();
  });

  it("describes what an entry reopens in one line", () => {
    expect(
      stateLine({
        kind: "browser",
        profile: null,
        windows: [
          { tabs: [tab("a"), tab("b")], activeTab: 0, windowTitle: null },
          { tabs: [tab("c")], activeTab: 0, windowTitle: null },
        ],
      }),
    ).toBe("3 tabs");
    expect(
      stateLine({
        kind: "projects",
        projects: ["/code/reframe", "/code/site", "/code/api"].map((path) => ({ path, isWorkspaceFile: false, windowTitle: null })),
      }),
    ).toBe("reframe, site +1");
    expect(stateLine({ kind: "media", uri: "spotify:playlist:x", label: null })).toBe("Playlist");
    expect(stateLine({ kind: "none" })).toBe("");
  });

  it("describes launch targets", () => {
    expect(launchTargetText({ executable: "/usr/bin/xterm", package: null })).toBe("/usr/bin/xterm");
    expect(launchTargetText({ executable: null, package: { kind: "flatpak", id: "org.gimp.GIMP", path: null } })).toBe("org.gimp.GIMP");
    expect(launchTargetText({ executable: " ", package: null })).toBeNull();
    expect(launchFromTarget({ kind: "executable", path: "/opt/app" })).toEqual({ executable: "/opt/app", package: null });
    expect(launchFromTarget({ kind: "open", target: "https://example.com" })).toBeNull();
  });

  it("writes ordinals", () => {
    expect([1, 2, 3, 4, 11, 12, 13, 21, 22, 101].map(ordinal)).toEqual([
      "1st",
      "2nd",
      "3rd",
      "4th",
      "11th",
      "12th",
      "13th",
      "21st",
      "22nd",
      "101st",
    ]);
  });

  it("takes the last path segment", () => {
    expect(pathLeaf("/home/me/code/reframe/")).toBe("reframe");
    expect(pathLeaf("C:\\Users\\me\\Docs")).toBe("Docs");
  });
});
