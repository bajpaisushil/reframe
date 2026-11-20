import { describe, expect, it } from "vitest";
import {
  calendarDaysBetween,
  formatAbsoluteTime,
  formatCount,
  formatDuration,
  formatList,
  formatRelativeTime,
  formatShortDate,
  greeting,
  pluralize,
  relativeTimeRefreshInterval,
  toDate,
} from "./format";

// Local-time constructors keep these tests independent of the machine's time zone.
const now = new Date(2026, 10, 20, 15, 30, 0); // Fri Nov 20 2026, 15:30 local
const minutesAgo = (m: number) => new Date(now.getTime() - m * 60_000);

describe("toDate", () => {
  it("parses ISO strings, dates and epoch ms", () => {
    expect(toDate("2026-11-20T10:00:00Z")?.toISOString()).toBe("2026-11-20T10:00:00.000Z");
    expect(toDate(0)?.getTime()).toBe(0);
    expect(toDate(now)?.getTime()).toBe(now.getTime());
  });

  it("returns null for missing or invalid input", () => {
    expect(toDate(null)).toBeNull();
    expect(toDate(undefined)).toBeNull();
    expect(toDate("")).toBeNull();
    expect(toDate("not a date")).toBeNull();
  });
});

describe("formatRelativeTime", () => {
  it("says just now for the last minute and slight future skew", () => {
    expect(formatRelativeTime(now, now)).toBe("just now");
    expect(formatRelativeTime(new Date(now.getTime() - 59_000), now)).toBe("just now");
    expect(formatRelativeTime(new Date(now.getTime() + 90_000), now)).toBe("just now");
  });

  it("uses minutes, then hours", () => {
    expect(formatRelativeTime(minutesAgo(1), now)).toBe("1 min ago");
    expect(formatRelativeTime(minutesAgo(59), now)).toBe("59 min ago");
    expect(formatRelativeTime(minutesAgo(60), now)).toBe("1 h ago");
    expect(formatRelativeTime(minutesAgo(125), now)).toBe("2 h ago");
    expect(formatRelativeTime(minutesAgo(23 * 60 + 59), now)).toBe("23 h ago");
  });

  it("uses calendar days after 24 hours", () => {
    expect(formatRelativeTime(new Date(2026, 10, 19, 9, 0), now)).toBe("yesterday");
    expect(formatRelativeTime(new Date(2026, 10, 18, 23, 0), now)).toBe("2 days ago");
    expect(formatRelativeTime(new Date(2026, 10, 14, 12, 0), now)).toBe("6 days ago");
  });

  it("falls back to a short date after a week", () => {
    expect(formatRelativeTime(new Date(2026, 10, 2, 12, 0), now, "en-US")).toBe("Nov 2");
    expect(formatRelativeTime(new Date(2024, 2, 4, 12, 0), now, "en-US")).toBe("Mar 4, 2024");
  });

  it("shows far-future timestamps as dates and ignores invalid input", () => {
    expect(formatRelativeTime(new Date(2026, 11, 25, 9, 0), now, "en-US")).toBe("Dec 25");
    expect(formatRelativeTime("garbage", now)).toBe("");
    expect(formatRelativeTime(null, now)).toBe("");
  });
});

describe("calendar helpers", () => {
  it("counts calendar days, not 24-hour periods", () => {
    expect(calendarDaysBetween(new Date(2026, 10, 19, 23, 59), new Date(2026, 10, 20, 0, 1))).toBe(1);
    expect(calendarDaysBetween(new Date(2026, 10, 20, 0, 1), new Date(2026, 10, 20, 23, 59))).toBe(0);
  });

  it("formats short dates with the year only when needed", () => {
    expect(formatShortDate(new Date(2026, 0, 5), now, "en-US")).toBe("Jan 5");
    expect(formatShortDate(new Date(2025, 0, 5), now, "en-US")).toBe("Jan 5, 2025");
    expect(formatShortDate(null, now)).toBe("");
  });

  it("formats absolute times for tooltips", () => {
    const text = formatAbsoluteTime(new Date(2026, 10, 20, 9, 41), "en-US");
    expect(text).toContain("Friday, November 20, 2026");
    expect(text).toMatch(/9:41/);
    expect(formatAbsoluteTime("nope")).toBe("");
  });

  it("refreshes young timestamps more often", () => {
    expect(relativeTimeRefreshInterval(minutesAgo(5), now)).toBe(15_000);
    expect(relativeTimeRefreshInterval(minutesAgo(120), now)).toBe(60_000);
    expect(relativeTimeRefreshInterval(minutesAgo(3000), now)).toBe(600_000);
    expect(relativeTimeRefreshInterval(null, now)).toBe(Number.POSITIVE_INFINITY);
  });
});

describe("greeting", () => {
  it("follows the time of day", () => {
    expect(greeting(new Date(2026, 10, 20, 5, 0))).toBe("Good morning");
    expect(greeting(new Date(2026, 10, 20, 11, 59))).toBe("Good morning");
    expect(greeting(new Date(2026, 10, 20, 12, 0))).toBe("Good afternoon");
    expect(greeting(new Date(2026, 10, 20, 17, 59))).toBe("Good afternoon");
    expect(greeting(new Date(2026, 10, 20, 18, 0))).toBe("Good evening");
    expect(greeting(new Date(2026, 10, 20, 2, 0))).toBe("Good evening");
  });
});

describe("counts and lists", () => {
  it("pluralizes with locale-aware numbers", () => {
    expect(pluralize(0, "app", undefined, "en-US")).toBe("0 apps");
    expect(pluralize(1, "app", undefined, "en-US")).toBe("1 app");
    expect(pluralize(1204, "window", undefined, "en-US")).toBe("1,204 windows");
    expect(pluralize(2, "entry", "entries", "en-US")).toBe("2 entries");
    expect(formatCount(Number.NaN)).toBe("0");
  });

  it("joins lists naturally", () => {
    expect(formatList([])).toBe("");
    expect(formatList(["Firefox"])).toBe("Firefox");
    expect(formatList(["Firefox", "Slack"])).toBe("Firefox and Slack");
    expect(formatList(["Firefox", "Slack", "Code"])).toBe("Firefox, Slack and Code");
  });

  it("formats durations", () => {
    expect(formatDuration(400)).toBe("0.4 s");
    expect(formatDuration(9_940)).toBe("9.9 s");
    expect(formatDuration(12_000)).toBe("12 s");
    expect(formatDuration(59_700)).toBe("1 min");
    expect(formatDuration(125_000)).toBe("2 min 5 s");
    expect(formatDuration(3_600_000)).toBe("1 h");
    expect(formatDuration(3_780_000)).toBe("1 h 3 min");
    expect(formatDuration(-1)).toBe("");
  });
});
