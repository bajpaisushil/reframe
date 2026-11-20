import { describe, expect, it } from "vitest";
import type { AppSettings, PlatformCapabilities } from "@/types";
import {
  addExcludedApp,
  capabilityRows,
  capabilitySummary,
  parseCloseWait,
  removeExcludedApp,
  startupRestoreNeedsLogin,
  startupWorkspaceMissing,
  systemDescription,
  withStartupAction,
  withStartupWorkspace,
} from "./settingsModel";

function settings(startup: Partial<AppSettings["startup"]> = {}): AppSettings {
  return {
    theme: "system",
    startup: { launchAtLogin: false, action: "doNothing", workspaceId: null, askBeforeRestoring: true, ...startup },
    excludedApps: [],
    browserTabCapture: true,
    defaultSwitchMode: "merge",
    closeWaitSeconds: 15,
  };
}

const workspaces = [
  { id: "a", isActive: false },
  { id: "b", isActive: true },
];

describe("startup choices", () => {
  it("picks the active workspace when choosing to restore a workspace", () => {
    const next = withStartupAction(settings(), "restoreWorkspace", workspaces);
    expect(next.startup).toMatchObject({ action: "restoreWorkspace", workspaceId: "b" });
  });

  it("falls back to the first workspace, or none", () => {
    expect(withStartupAction(settings(), "restoreWorkspace", [{ id: "a", isActive: false }]).startup.workspaceId).toBe("a");
    expect(withStartupAction(settings(), "restoreWorkspace", []).startup.workspaceId).toBeNull();
  });

  it("keeps a valid choice and replaces a deleted one", () => {
    expect(withStartupAction(settings({ workspaceId: "a" }), "restoreWorkspace", workspaces).startup.workspaceId).toBe("a");
    expect(withStartupAction(settings({ workspaceId: "gone" }), "restoreWorkspace", workspaces).startup.workspaceId).toBe("b");
  });

  it("remembers the workspace when switching to another choice", () => {
    const next = withStartupAction(settings({ action: "restoreWorkspace", workspaceId: "a" }), "doNothing", workspaces);
    expect(next.startup).toMatchObject({ action: "doNothing", workspaceId: "a" });
  });

  it("does not mutate the input", () => {
    const original = settings();
    withStartupAction(original, "restoreLastActive", workspaces);
    withStartupWorkspace(original, "a");
    expect(original.startup).toMatchObject({ action: "doNothing", workspaceId: null });
  });

  it("flags a missing startup workspace", () => {
    expect(startupWorkspaceMissing(settings({ action: "restoreWorkspace", workspaceId: "gone" }), workspaces)).toBe(true);
    expect(startupWorkspaceMissing(settings({ action: "restoreWorkspace", workspaceId: "a" }), workspaces)).toBe(false);
    expect(startupWorkspaceMissing(settings({ action: "restoreLastActive" }), workspaces)).toBe(false);
  });

  it("notes when startup restore cannot happen without the login item", () => {
    expect(startupRestoreNeedsLogin(settings({ action: "restoreLastActive" }))).toBe(true);
    expect(startupRestoreNeedsLogin(settings({ action: "restoreLastActive", launchAtLogin: true }))).toBe(false);
    expect(startupRestoreNeedsLogin(settings())).toBe(false);
  });
});

describe("parseCloseWait", () => {
  it("accepts whole seconds from 1 to 120", () => {
    expect(parseCloseWait(" 15 ")).toEqual({ ok: true, value: 15 });
    expect(parseCloseWait("1")).toEqual({ ok: true, value: 1 });
    expect(parseCloseWait("120")).toEqual({ ok: true, value: 120 });
  });

  it("explains what is wrong", () => {
    expect(parseCloseWait("")).toMatchObject({ ok: false, error: "Enter a number of seconds." });
    expect(parseCloseWait("0")).toMatchObject({ ok: false });
    expect(parseCloseWait("121")).toMatchObject({ ok: false, error: "Choose between 1 and 120 seconds." });
    expect(parseCloseWait("2.5")).toMatchObject({ ok: false, error: "Use whole seconds, from 1 to 120." });
    expect(parseCloseWait("-3")).toMatchObject({ ok: false });
  });
});

describe("excluded apps", () => {
  it("adds trimmed entries", () => {
    expect(addExcludedApp(["slack"], "  exe:gimp ")).toEqual({ ok: true, value: ["slack", "exe:gimp"] });
  });

  it("rejects empty and duplicate entries, ignoring case", () => {
    expect(addExcludedApp([], "   ")).toMatchObject({ ok: false });
    expect(addExcludedApp(["Slack"], "SLACK")).toMatchObject({ ok: false, error: "“SLACK” is already on the list." });
  });

  it("enforces the limit", () => {
    const full = Array.from({ length: 500 }, (_, i) => `app-${i}`);
    expect(addExcludedApp(full, "one-more")).toMatchObject({ ok: false });
  });

  it("removes one entry", () => {
    expect(removeExcludedApp(["a", "b", "c"], "b")).toEqual(["a", "c"]);
    expect(removeExcludedApp(["a"], "zzz")).toEqual(["a"]);
  });
});

describe("platform description", () => {
  const capabilities: PlatformCapabilities = {
    listWindows: { level: "full", note: null },
    readWindowGeometry: { level: "full", note: null },
    moveWindows: { level: "full", note: null },
    changeWindowState: { level: "full", note: null },
    closeWindows: { level: "full", note: null },
    focusWindows: { level: "full", note: null },
    listMonitors: { level: "partial", note: " The X server has no RandR extension. " },
    processDetails: { level: "full", note: null },
    applicationIcons: { level: "unsupported", note: "" },
  };

  it("lists every capability in order with plain labels", () => {
    const rows = capabilityRows(capabilities);
    expect(rows).toHaveLength(9);
    expect(rows[0]).toEqual({ key: "listWindows", label: "See open windows", level: "full", levelLabel: "Full", note: null });
    expect(rows[6]).toMatchObject({ levelLabel: "Partial", note: "The X server has no RandR extension." });
    expect(rows[8]).toMatchObject({ levelLabel: "Not available", note: null });
    expect(capabilitySummary(rows)).toBe("7 of 9 work fully here.");
  });

  it("describes the system in one line", () => {
    expect(systemDescription({ os: "linux", osVersion: "Ubuntu 24.04", displayServer: "x11", desktopEnvironment: "GNOME" })).toBe(
      "Ubuntu 24.04 · X11 · GNOME",
    );
    expect(systemDescription({ os: "windows", osVersion: "", displayServer: "unknown", desktopEnvironment: null })).toBe("Windows");
  });
});
