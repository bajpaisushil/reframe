import { describe, expect, it } from "vitest";
import type { EntryProgress, RestoreRun } from "@/types";
import { dayKey, dayLabel, formatTimeOfDay, groupByDay, runAppsLabel, runDurationMs } from "./activityModel";

// Local times, so the tests do not depend on the machine's time zone.
const now = new Date(2026, 10, 20, 15, 30); // Friday, November 20, 2026

describe("dayLabel", () => {
  it("uses friendly words for recent days", () => {
    expect(dayLabel(new Date(2026, 10, 20, 0, 5), now, "en-US")).toBe("Today");
    expect(dayLabel(new Date(2026, 10, 19, 23, 59), now, "en-US")).toBe("Yesterday");
    expect(dayLabel(new Date(2026, 10, 16, 9, 0), now, "en-US")).toBe("Monday");
  });

  it("falls back to dates", () => {
    expect(dayLabel(new Date(2026, 10, 2, 9, 0), now, "en-US")).toBe("November 2");
    expect(dayLabel(new Date(2025, 11, 31, 9, 0), now, "en-US")).toBe("December 31, 2025");
  });
});

describe("groupByDay", () => {
  it("groups by local day and keeps the order", () => {
    const items = [
      { id: 1, at: new Date(2026, 10, 20, 14, 0).toISOString() },
      { id: 2, at: new Date(2026, 10, 20, 9, 0).toISOString() },
      { id: 3, at: new Date(2026, 10, 19, 18, 0).toISOString() },
      { id: 4, at: "not a date" },
      { id: 5, at: new Date(2026, 9, 1, 8, 0).toISOString() },
    ];
    const groups = groupByDay(items, (i) => i.at, now, "en-US");
    expect(groups.map((g) => [g.label, g.items.map((i) => i.id)])).toEqual([
      ["Today", [1, 2]],
      ["Yesterday", [3]],
      ["October 1", [5]],
      ["Date unknown", [4]],
    ]);
    expect(groups[0].key).toBe(dayKey(new Date(2026, 10, 20)));
  });

  it("handles an empty list", () => {
    expect(groupByDay([], () => null, now)).toEqual([]);
  });
});

describe("restore history helpers", () => {
  const entry = (status: EntryProgress["status"], message: string | null = null): EntryProgress => ({
    entryId: Math.random().toString(36),
    displayName: "App",
    appId: "app",
    status,
    message,
    attempt: 1,
    windowsExpected: 1,
    windowsRestored: 1,
    startedAt: null,
    finishedAt: null,
  });

  const run = (overrides: Partial<RestoreRun>): RestoreRun => ({
    id: "r",
    workspaceId: "w",
    workspaceName: "Dev",
    mode: "merge",
    status: "completed",
    startedAt: "2026-11-20T10:00:00Z",
    finishedAt: "2026-11-20T10:00:14Z",
    entries: [],
    closedWindows: [],
    messages: [],
    ...overrides,
  });

  it("counts restored apps", () => {
    const r = run({
      status: "partiallyCompleted",
      entries: [entry("completed"), entry("partiallyCompleted"), entry("failed"), entry("skipped", "Disabled")],
    });
    expect(runAppsLabel(r)).toBe("2 of 3 apps");
    expect(runAppsLabel(run({ entries: [entry("completed")] }))).toBe("1 app");
    expect(runAppsLabel(run({ entries: [entry("completed"), entry("completed")] }))).toBe("2 apps");
    expect(runAppsLabel(run({ status: "cancelled", entries: [entry("completed"), entry("skipped", "Restore cancelled")] }))).toBe(
      "1 of 2 apps",
    );
  });

  it("measures finished runs only", () => {
    expect(runDurationMs(run({}))).toBe(14_000);
    expect(runDurationMs(run({ finishedAt: null }))).toBeNull();
  });

  it("formats the time of day", () => {
    expect(formatTimeOfDay(new Date(2026, 10, 20, 9, 41), "en-US")).toBe("9:41 AM");
    expect(formatTimeOfDay(null)).toBe("");
  });
});
