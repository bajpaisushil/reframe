// Adding freshly captured applications to an existing workspace (tested in capture.test.ts).

import type { CaptureResult, Workspace } from "@/types";

/**
 * The workspace with the captured entries appended (they launch after the existing ones).
 *
 * Saved window positions are only meaningful together with the monitor layout they were captured
 * on, so the layout is extended as well: a workspace without one takes the current monitors (as a
 * new capture would), otherwise monitors the new windows are on are added when it doesn't know
 * them yet. A monitor it already knows keeps its saved geometry.
 */
export function appendCapture(workspace: Workspace, result: Pick<CaptureResult, "entries" | "monitors">): Workspace {
  const applications = [...workspace.applications, ...result.entries];
  if (workspace.monitors.length === 0) {
    return { ...workspace, applications, monitors: [...result.monitors] };
  }
  const known = new Set(workspace.monitors.map((monitor) => monitor.id));
  const used = new Set(result.entries.flatMap((entry) => entry.windows.map((w) => w.monitorId)));
  const added = result.monitors.filter((monitor) => !known.has(monitor.id) && used.has(monitor.id));
  return { ...workspace, applications, monitors: added.length > 0 ? [...workspace.monitors, ...added] : workspace.monitors };
}
