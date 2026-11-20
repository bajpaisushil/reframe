// Display helpers for discovered applications and windows (tested in describe.test.ts).

import { formatCount, pluralize } from "@/lib/format";
import type { AppCategory, DiscoveredApplication, IconLookup, MonitorInfo, Rect, StateDetail, WindowState } from "@/types";

/** Badge text for window states worth pointing out (normal windows get none). */
export const WINDOW_STATE_LABEL: Partial<Record<WindowState, string>> = {
  maximized: "Maximized",
  minimized: "Minimized",
  fullscreen: "Full screen",
};

export function windowTitle(title: string): string {
  return title.trim() || "Untitled window";
}

/** "1280 × 720" */
export function windowSize(bounds: Pick<Rect, "width" | "height">): string {
  return `${formatCount(Math.round(bounds.width))} × ${formatCount(Math.round(bounds.height))}`;
}

/** "Project: reframe" */
export function detailText(detail: StateDetail): string {
  return detail.value ? `${detail.label}: ${detail.value}` : detail.label;
}

/** "2 windows", or "1 of 3 windows" when only some are selected. */
export function windowCountLabel(total: number, selected: number | null): string {
  if (selected === null || selected === total || selected === 0) return pluralize(total, "window");
  return `${formatCount(selected)} of ${pluralize(total, "window")}`;
}

/** Footnote about processes Explore doesn't list; empty when there are none. */
export function backgroundNote(count: number): string {
  if (!(count > 0)) return "";
  return count === 1
    ? "1 background process isn't shown — it has no windows."
    : `${formatCount(count)} background processes aren't shown — they have no windows.`;
}

export function iconLookup(app: Pick<DiscoveredApplication, "appId" | "executablePath" | "package">): IconLookup {
  return { appId: app.appId, executablePath: app.executablePath, package: app.package };
}

/** Apps split by category, keeping the snapshot order (frontmost first) inside each group. */
export function groupByCategory<T extends Pick<DiscoveredApplication, "category">>(apps: readonly T[]): Record<AppCategory, T[]> {
  const groups: Record<AppCategory, T[]> = { application: [], utility: [], system: [] };
  for (const app of apps) (groups[app.category] ?? groups.application).push(app);
  return groups;
}

/** Monitor names by id, only when there is more than one monitor (one name says nothing). */
export function monitorNames(monitors: readonly Pick<MonitorInfo, "id" | "name">[]): Map<string, string> {
  if (monitors.length < 2) return new Map();
  return new Map(monitors.map((m) => [m.id, m.name]));
}
