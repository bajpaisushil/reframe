import { describe, expect, it } from "vitest";
import type { Capability, PlatformCapabilities, PlatformInfo } from "@/types";
import { desktopNotes, missingPermissions, plainWarning } from "./notices";

const full: Capability = { level: "full", note: null };

function platform(overrides: Partial<PlatformCapabilities> = {}): PlatformInfo {
  return {
    os: "linux",
    osVersion: "Ubuntu 24.04",
    displayServer: "x11",
    desktopEnvironment: null,
    capabilities: {
      listWindows: full,
      readWindowGeometry: full,
      moveWindows: full,
      changeWindowState: full,
      closeWindows: full,
      focusWindows: full,
      listMonitors: full,
      processDetails: full,
      applicationIcons: full,
      ...overrides,
    },
  };
}

describe("plainWarning", () => {
  it("rewords failures of a part of the scan", () => {
    expect(plainWarning("Monitors: RandR query failed")).toBe("Couldn't read your display layout — RandR query failed.");
    expect(plainWarning("Windows:")).toBe("Couldn't list the open windows.");
  });

  it("splits a summary from its reason", () => {
    expect(plainWarning("Some open windows may be missing: Wayland session: only XWayland apps are visible.")).toBe(
      "Some open windows may be missing. Wayland session: only XWayland apps are visible.",
    );
  });

  it("keeps other warnings, as sentences", () => {
    expect(plainWarning("the VS Code adapter failed; showing basic details")).toBe(
      "The VS Code adapter failed; showing basic details.",
    );
    expect(plainWarning("Some open windows may be missing.")).toBe("Some open windows may be missing.");
  });
});

describe("desktopNotes", () => {
  it("has nothing to say about a fully supported, clean scan", () => {
    expect(desktopNotes({ warnings: [], platform: platform() })).toEqual({ blocked: null, notes: [] });
  });

  it("explains a platform that can't list windows once", () => {
    const reason = "Wayland compositors do not allow applications to inspect or arrange other applications' windows.";
    const notes = desktopNotes({
      warnings: [
        `Windows: Not supported on this system: ${reason}`,
        `Reframe cannot list open windows on this system: ${reason}`,
        "Processes: permission denied",
      ],
      platform: platform({
        listWindows: { level: "unsupported", note: reason },
        listMonitors: { level: "unsupported", note: "Monitor layout is only available through X11." },
      }),
    });
    expect(notes.blocked).toBe(reason);
    expect(notes.notes).toEqual([
      "Couldn't read the running programs — Permission denied.",
      "Monitor layout is only available through X11.",
    ]);
  });

  it("falls back to a generic reason", () => {
    const notes = desktopNotes({ warnings: [], platform: platform({ listWindows: { level: "unsupported", note: null } }) });
    expect(notes.blocked).toMatch(/doesn't let Reframe see/);
  });

  it("adds partial monitor support unless a warning already says it", () => {
    const xwayland = "Monitors are reported by XWayland: names may be generic.";
    expect(
      desktopNotes({ warnings: [], platform: platform({ listMonitors: { level: "partial", note: xwayland } }) }).notes,
    ).toEqual([xwayland]);
    expect(
      desktopNotes({
        warnings: [`Note: ${xwayland}`],
        platform: platform({ listMonitors: { level: "partial", note: xwayland } }),
      }).notes,
    ).toEqual([`Note: ${xwayland}`]);
  });

  it("drops duplicate warnings", () => {
    expect(desktopNotes({ warnings: ["Some windows may be missing", "Some windows may be missing."], platform: platform() }).notes).toEqual([
      "Some windows may be missing.",
    ]);
  });
});

describe("missingPermissions", () => {
  it("keeps the permissions that are not granted", () => {
    const list = [
      { kind: "accessibility" as const, granted: false, purpose: "Move windows", instructions: "System Settings" },
      { kind: "screenRecording" as const, granted: true, purpose: "Window titles", instructions: "System Settings" },
    ];
    expect(missingPermissions(list).map((p) => p.kind)).toEqual(["accessibility"]);
    expect(missingPermissions(undefined)).toEqual([]);
  });
});
