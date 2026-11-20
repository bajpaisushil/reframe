// Plain-language wording for the restore readiness report (api.workspaceHealth). Pure (tested in
// readiness.test.ts).

import { pluralize } from "@/lib/format";
import type { EntryHealth, MonitorMapping, RestoreEstimate } from "@/types";

export type ReadinessTone = "success" | "neutral" | "warning";

export interface ReadinessHeadline {
  title: string;
  /** A second sentence when something needs attention. */
  detail: string | null;
  tone: ReadinessTone;
}

export function estimateTotal(estimate: RestoreEstimate): number {
  return estimate.full + estimate.partial + estimate.launchOnly + estimate.unavailable;
}

/**
 * "All 5 apps can be fully restored", "4 of 5 apps can be fully restored", "3 of 5 apps are
 * ready to restore". Counts only apps that are switched on (as the estimate does).
 */
export function readinessHeadline(estimate: RestoreEstimate): ReadinessHeadline {
  const total = estimateTotal(estimate);
  const ready = total - estimate.unavailable;
  if (total === 0) {
    return { title: "Nothing to restore yet", detail: null, tone: "neutral" };
  }
  if (estimate.unavailable === 0) {
    const allFull = estimate.full === total;
    if (total === 1) {
      return { title: allFull ? "The app can be fully restored" : "The app is ready to restore", detail: null, tone: "success" };
    }
    return {
      title: allFull ? `All ${total} apps can be fully restored` : `All ${total} apps are ready to restore`,
      detail: null,
      tone: "success",
    };
  }
  const title =
    ready > 0 && estimate.full === ready
      ? `${ready} of ${pluralize(total, "app")} can be fully restored`
      : `${ready} of ${pluralize(total, "app")} ${ready === 1 ? "is" : "are"} ready to restore`;
  const detail =
    estimate.unavailable === 1
      ? "One app can't be started on this computer and will be skipped."
      : `${estimate.unavailable} apps can't be started on this computer and will be skipped.`;
  return { title, detail, tone: "warning" };
}

export interface MonitorSummary {
  text: string;
  tone: "neutral" | "warning";
}

function displays(count: number): string {
  return pluralize(count, "display");
}

/**
 * Where saved windows will go, in words: "Saved on 2 displays; 1 is connected — windows from the
 * missing one go to your main display." Null when the workspace has no saved display layout.
 */
export function describeMonitors(mappings: readonly MonitorMapping[]): MonitorSummary | null {
  const saved = mappings.length;
  if (saved === 0) return null;
  const connected = mappings.filter((m) => m.mappedId !== null).length;
  const exact = mappings.filter((m) => m.mappedId !== null && m.exact).length;
  const missing = saved - connected;

  if (missing === 0) {
    const status = saved === 1 ? "it's connected" : saved === 2 ? "both are connected" : "all are connected";
    if (exact === saved) {
      return { text: `Saved on ${displays(saved)}; ${status} — windows go back exactly where they were.`, tone: "neutral" };
    }
    const changed = saved === 1 ? "but its size or position has changed" : "but not all at the same size or position";
    return { text: `Saved on ${displays(saved)}; ${status}, ${changed} — windows are moved and scaled to fit.`, tone: "neutral" };
  }
  if (connected === 0) {
    return saved === 1
      ? { text: "Saved on a display that isn't connected — its windows go to your main display.", tone: "warning" }
      : { text: `Saved on ${displays(saved)}; none are connected — windows go to your main display.`, tone: "warning" };
  }
  const missingWhich = missing === 1 ? "the missing one" : "the missing ones";
  return {
    text: `Saved on ${displays(saved)}; ${connected} ${connected === 1 ? "is" : "are"} connected — windows from ${missingWhich} go to your main display.`,
    tone: "warning",
  };
}

/** One display mapping as a short phrase: "DELL U2720Q → HDMI-1". */
export function describeMapping(mapping: MonitorMapping): { from: string; to: string; note: string } {
  if (mapping.mappedId === null) {
    return { from: mapping.savedName || mapping.savedId, to: "Main display", note: "Not connected" };
  }
  return {
    from: mapping.savedName || mapping.savedId,
    to: mapping.mappedName || mapping.mappedId,
    note: mapping.exact ? "Same size and position" : "Resized to fit",
  };
}

/** Short summary line for an application from its health details ("Project: reframe · Tabs: 12"). */
export function healthSummary(health: Pick<EntryHealth, "details"> | undefined, max = 3): string {
  if (!health || health.details.length === 0) return "";
  return health.details
    .slice(0, max)
    .map((detail) => (detail.value ? `${detail.label}: ${detail.value}` : detail.label))
    .join(" · ");
}
