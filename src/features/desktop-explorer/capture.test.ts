import { describe, expect, it } from "vitest";
import type { ApplicationEntry, MonitorInfo, Workspace } from "@/types";
import { appendCapture } from "./capture";

const monitor = (id: string, x = 0): MonitorInfo => ({
  id,
  name: id,
  bounds: { x, y: 0, width: 1920, height: 1080 },
  workArea: { x, y: 0, width: 1920, height: 1040 },
  scaleFactor: 1,
  isPrimary: x === 0,
});

const entry = (id: string, monitorIds: Array<string | null>): ApplicationEntry => ({
  id,
  appId: `exe:${id}`,
  displayName: id,
  adapterId: "generic",
  enabled: true,
  launch: { executable: `/usr/bin/${id}`, package: null, arguments: [], workingDirectory: null },
  dependsOn: [],
  state: { kind: "none" },
  windows: monitorIds.map((monitorId) => ({
    title: id,
    className: null,
    bounds: { x: 0, y: 0, width: 800, height: 600 },
    state: "normal",
    monitorId,
  })),
  options: { delayAfterLaunchMs: null, timeoutMs: null, retryCount: null, restoreWindowPositions: true },
});

const workspace = (applications: ApplicationEntry[], monitors: MonitorInfo[]): Workspace => ({
  id: "ws",
  name: "Deep work",
  description: "",
  icon: "🧠",
  color: null,
  createdAt: "2026-11-01T00:00:00Z",
  updatedAt: "2026-11-01T00:00:00Z",
  lastRestoredAt: null,
  applications,
  monitors,
  restore: {
    defaultMode: "merge",
    launchDelayMs: 0,
    timeoutMs: 10000,
    retryCount: 1,
    restoreWindowPositions: true,
    maxParallel: 3,
  },
  schemaVersion: 1,
});

describe("appendCapture", () => {
  it("appends entries after the existing ones", () => {
    const result = appendCapture(workspace([entry("code", ["a"])], [monitor("a")]), {
      entries: [entry("xterm", ["a"]), entry("xclock", ["a"])],
      monitors: [monitor("a")],
    });
    expect(result.applications.map((e) => e.id)).toEqual(["code", "xterm", "xclock"]);
    expect(result.monitors.map((m) => m.id)).toEqual(["a"]);
  });

  it("adds monitors the new windows use that the layout doesn't know", () => {
    const saved = { ...monitor("a"), bounds: { x: 0, y: 0, width: 2560, height: 1440 } };
    const result = appendCapture(workspace([], [saved]), {
      entries: [entry("xterm", ["b", null])],
      monitors: [monitor("a"), monitor("b", 1920), monitor("c", 3840)],
    });
    expect(result.monitors.map((m) => m.id)).toEqual(["a", "b"]);
    // A known monitor keeps its saved geometry.
    expect(result.monitors[0].bounds.width).toBe(2560);
  });

  it("takes the current layout when the workspace has none yet", () => {
    const result = appendCapture(workspace([], []), {
      entries: [entry("xterm", [null])],
      monitors: [monitor("a"), monitor("b", 1920)],
    });
    expect(result.monitors.map((m) => m.id)).toEqual(["a", "b"]);
  });

  it("doesn't modify the workspace it was given", () => {
    const original = workspace([entry("code", ["a"])], [monitor("a")]);
    appendCapture(original, { entries: [entry("xterm", ["b"])], monitors: [monitor("b", 1920)] });
    expect(original.applications).toHaveLength(1);
    expect(original.monitors).toHaveLength(1);
  });
});
