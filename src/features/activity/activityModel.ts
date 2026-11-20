// Pure helpers for the Activity page: grouping by calendar day and describing past restores.
// Tested in activityModel.test.ts.

import { calendarDaysBetween, toDate, type DateInput } from "@/lib/format";
import { runCounts } from "@/features/restoration/runState";
import type { RestoreRun } from "@/types";

export interface DayGroup<T> {
  /** Local calendar day ("2026-11-20"), or "unknown". */
  key: string;
  label: string;
  items: T[];
}

function pad(n: number): string {
  return String(n).padStart(2, "0");
}

export function dayKey(date: Date): string {
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

/** "Today", "Yesterday", "Monday" (this week), "November 2", "November 2, 2025". */
export function dayLabel(date: Date, now: Date = new Date(), locale?: string): string {
  const days = calendarDaysBetween(date, now);
  if (days === 0) return "Today";
  if (days === 1) return "Yesterday";
  if (days > 1 && days < 7) return new Intl.DateTimeFormat(locale, { weekday: "long" }).format(date);
  const sameYear = date.getFullYear() === now.getFullYear();
  return new Intl.DateTimeFormat(locale, { month: "long", day: "numeric", ...(sameYear ? {} : { year: "numeric" }) }).format(date);
}

/**
 * Group items by local calendar day, keeping their order (the API lists newest first). Items
 * without a valid date are collected at the end.
 */
export function groupByDay<T>(items: readonly T[], getDate: (item: T) => DateInput, now: Date = new Date(), locale?: string): DayGroup<T>[] {
  const groups: DayGroup<T>[] = [];
  const byKey = new Map<string, DayGroup<T>>();
  const undated: T[] = [];
  for (const item of items) {
    const date = toDate(getDate(item));
    if (!date) {
      undated.push(item);
      continue;
    }
    const key = dayKey(date);
    let group = byKey.get(key);
    if (!group) {
      group = { key, label: dayLabel(date, now, locale), items: [] };
      byKey.set(key, group);
      groups.push(group);
    }
    group.items.push(item);
  }
  if (undated.length > 0) groups.push({ key: "unknown", label: "Date unknown", items: undated });
  return groups;
}

/** "9:41 AM" in the user's regional format. */
export function formatTimeOfDay(value: DateInput, locale?: string): string {
  const date = toDate(value);
  if (!date) return "";
  return new Intl.DateTimeFormat(locale, { hour: "numeric", minute: "2-digit" }).format(date);
}

/** "6 apps" for a complete restore, "5 of 6 apps" otherwise (turned-off apps are not counted). */
export function runAppsLabel(run: Pick<RestoreRun, "entries" | "status">): string {
  const { restored, total } = runCounts(run);
  const noun = total === 1 ? "app" : "apps";
  return run.status === "completed" && restored === total ? `${total} ${noun}` : `${restored} of ${total} ${noun}`;
}

/** How long a finished run took (ms), or null while running / when unknown. */
export function runDurationMs(run: Pick<RestoreRun, "startedAt" | "finishedAt">): number | null {
  if (!run.finishedAt) return null;
  const ms = Date.parse(run.finishedAt) - Date.parse(run.startedAt);
  return Number.isFinite(ms) && ms >= 0 ? ms : null;
}
