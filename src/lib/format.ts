// Formatting helpers for dates, counts and durations. Pure functions: every helper takes the
// current time and locale as optional parameters so it can be tested deterministically.
//
// UI copy is English; absolute dates use the user's regional format (the webview locale).

const SECOND = 1000;
const MINUTE = 60 * SECOND;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/** Anything the API hands us as a timestamp (ISO strings), or a Date / epoch milliseconds. */
export type DateInput = string | number | Date | null | undefined;

/** Parse a timestamp; `null` for missing or invalid input (never throws). */
export function toDate(value: DateInput): Date | null {
  if (value === null || value === undefined || value === "") return null;
  const date = value instanceof Date ? new Date(value.getTime()) : new Date(value);
  return Number.isNaN(date.getTime()) ? null : date;
}

function startOfDay(date: Date): number {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();
}

/** Whole calendar days between two dates in local time (DST-safe). */
export function calendarDaysBetween(earlier: Date, later: Date): number {
  return Math.round((startOfDay(later) - startOfDay(earlier)) / DAY);
}

/** "Nov 4" in the current year, "Nov 4, 2024" otherwise. */
export function formatShortDate(value: DateInput, now: Date = new Date(), locale?: string): string {
  const date = toDate(value);
  if (!date) return "";
  const sameYear = date.getFullYear() === now.getFullYear();
  return new Intl.DateTimeFormat(locale, {
    month: "short",
    day: "numeric",
    ...(sameYear ? {} : { year: "numeric" }),
  }).format(date);
}

/**
 * Compact, friendly relative time: "just now", "5 min ago", "2 h ago", "yesterday",
 * "3 days ago", then a short date. Timestamps slightly in the future (clock skew between
 * processes) read as "just now"; anything further ahead falls back to the date.
 */
export function formatRelativeTime(value: DateInput, now: Date = new Date(), locale?: string): string {
  const date = toDate(value);
  if (!date) return "";
  const diff = now.getTime() - date.getTime();
  if (diff < -5 * MINUTE) return formatShortDate(date, now, locale);
  if (diff < MINUTE) return "just now";
  if (diff < HOUR) return `${Math.floor(diff / MINUTE)} min ago`;
  if (diff < DAY) return `${Math.floor(diff / HOUR)} h ago`;
  const days = calendarDaysBetween(date, now);
  if (days <= 1) return "yesterday";
  if (days < 7) return `${days} days ago`;
  return formatShortDate(date, now, locale);
}

/** "Friday, November 20, 2026 at 9:41 AM" (regional format) for tooltips and titles. */
export function formatAbsoluteTime(value: DateInput, locale?: string): string {
  const date = toDate(value);
  if (!date) return "";
  return new Intl.DateTimeFormat(locale, { dateStyle: "full", timeStyle: "short" }).format(date);
}

/** How often a relative label for this timestamp needs re-rendering to stay accurate. */
export function relativeTimeRefreshInterval(value: DateInput, now: Date = new Date()): number {
  const date = toDate(value);
  if (!date) return Number.POSITIVE_INFINITY;
  const age = now.getTime() - date.getTime();
  if (age < HOUR) return 15 * SECOND;
  if (age < DAY) return MINUTE;
  return 10 * MINUTE;
}

/** Time-aware greeting for the dashboard header. */
export function greeting(now: Date = new Date()): string {
  const hour = now.getHours();
  if (hour >= 5 && hour < 12) return "Good morning";
  if (hour >= 12 && hour < 18) return "Good afternoon";
  return "Good evening";
}

/** Locale-aware integer formatting ("1,204"). */
export function formatCount(value: number, locale?: string): string {
  if (!Number.isFinite(value)) return "0";
  return new Intl.NumberFormat(locale, { maximumFractionDigits: 0 }).format(value);
}

/** "1 app", "3 apps", "0 apps". The plural defaults to `singular + "s"`. */
export function pluralize(count: number, singular: string, plural = `${singular}s`, locale?: string): string {
  return `${formatCount(count, locale)} ${count === 1 ? singular : plural}`;
}

/** Human duration: "0.4 s", "12 s", "2 min 5 s", "1 h 3 min". */
export function formatDuration(ms: number): string {
  if (!Number.isFinite(ms) || ms < 0) return "";
  if (ms < 9950) return `${Math.round(ms / 100) / 10} s`;
  const totalSeconds = Math.round(ms / SECOND);
  if (totalSeconds < 60) return `${totalSeconds} s`;
  const totalMinutes = Math.floor(totalSeconds / 60);
  if (totalMinutes < 60) {
    const seconds = totalSeconds % 60;
    return seconds ? `${totalMinutes} min ${seconds} s` : `${totalMinutes} min`;
  }
  const hours = Math.floor(totalMinutes / 60);
  const minutes = totalMinutes % 60;
  return minutes ? `${hours} h ${minutes} min` : `${hours} h`;
}

/** Join a list as "A, B and C". */
export function formatList(items: readonly string[], conjunction = "and"): string {
  if (items.length === 0) return "";
  if (items.length === 1) return items[0];
  return `${items.slice(0, -1).join(", ")} ${conjunction} ${items[items.length - 1]}`;
}
