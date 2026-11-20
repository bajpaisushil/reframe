import { describe, expect, it } from "vitest";
import { lastUsedAt, sortByRecentUse, summaryApps } from "./workspaces";

describe("summaryApps", () => {
  it("pairs icon lookups with names and falls back to the app id", () => {
    const apps = summaryApps({
      applicationIcons: [
        { appId: "firefox", executablePath: "/usr/bin/firefox", package: null },
        { appId: "code", executablePath: null, package: null },
      ],
      applicationNames: ["Firefox"],
    });
    expect(apps).toEqual([
      { lookup: { appId: "firefox", executablePath: "/usr/bin/firefox", package: null }, name: "Firefox" },
      { lookup: { appId: "code", executablePath: null, package: null }, name: "code" },
    ]);
  });
});

describe("recent use ordering", () => {
  const ws = (id: string, updatedAt: string, lastRestoredAt: string | null) => ({ id, updatedAt, lastRestoredAt });

  it("uses the later of restore and update times", () => {
    expect(lastUsedAt(ws("a", "2026-11-01T00:00:00Z", "2026-11-10T00:00:00Z"))).toBe(Date.parse("2026-11-10T00:00:00Z"));
    expect(lastUsedAt(ws("a", "2026-11-12T00:00:00Z", null))).toBe(Date.parse("2026-11-12T00:00:00Z"));
    expect(lastUsedAt(ws("a", "garbage", null))).toBe(0);
  });

  it("sorts most recently used first and keeps ties stable", () => {
    const list = [
      ws("old", "2026-10-01T00:00:00Z", null),
      ws("restored", "2026-10-02T00:00:00Z", "2026-11-19T00:00:00Z"),
      ws("edited", "2026-11-18T00:00:00Z", null),
      ws("tie", "2026-10-01T00:00:00Z", null),
    ];
    expect(sortByRecentUse(list).map((w) => w.id)).toEqual(["restored", "edited", "old", "tie"]);
    expect(list[0].id).toBe("old"); // input untouched
  });
});
