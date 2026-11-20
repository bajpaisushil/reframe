// Geometry of the monitor mini-map (tested in miniMap.test.ts).
//
// Everything is expressed in percentages of the virtual desktop (the bounding box of all
// monitors), so the diagram scales with its container without re-measuring. Windows are clipped
// to the virtual desktop and listed back to front.

import type { MonitorInfo, Rect, WindowInfo, WindowState } from "@/types";

/** A rectangle in percent (0–100) of the virtual desktop. */
export interface MapBox {
  left: number;
  top: number;
  width: number;
  height: number;
}

export interface MiniMapMonitor {
  id: string;
  name: string;
  isPrimary: boolean;
  box: MapBox;
  /** Physical size, for labels ("2560 × 1440"). */
  width: number;
  height: number;
}

export interface MiniMapWindow {
  id: string;
  appKey: string;
  title: string;
  state: WindowState;
  box: MapBox;
}

export interface MiniMapLayout {
  /** The virtual desktop in desktop coordinates. */
  bounds: Rect;
  /** Width / height of the virtual desktop. */
  aspect: number;
  monitors: MiniMapMonitor[];
  /** Back to front: later entries are drawn on top. */
  windows: MiniMapWindow[];
}

export interface MapApp {
  key: string;
  windows: ReadonlyArray<Pick<WindowInfo, "id" | "title" | "bounds" | "state" | "zOrder">>;
}

function isUsable(rect: Rect): boolean {
  return [rect.x, rect.y, rect.width, rect.height].every(Number.isFinite) && rect.width > 0 && rect.height > 0;
}

/** Bounding box of all monitors with a usable size; `null` when there are none. */
export function virtualBounds(monitors: ReadonlyArray<Pick<MonitorInfo, "bounds">>): Rect | null {
  let left = Number.POSITIVE_INFINITY;
  let top = Number.POSITIVE_INFINITY;
  let right = Number.NEGATIVE_INFINITY;
  let bottom = Number.NEGATIVE_INFINITY;
  for (const { bounds } of monitors) {
    if (!isUsable(bounds)) continue;
    left = Math.min(left, bounds.x);
    top = Math.min(top, bounds.y);
    right = Math.max(right, bounds.x + bounds.width);
    bottom = Math.max(bottom, bounds.y + bounds.height);
  }
  if (!Number.isFinite(left)) return null;
  return { x: left, y: top, width: right - left, height: bottom - top };
}

/** `rect` clipped to `bounds`, in percent of `bounds`; `null` when nothing of it is inside. */
export function toMapBox(rect: Rect, bounds: Rect): MapBox | null {
  if (!isUsable(rect) || !isUsable(bounds)) return null;
  const left = Math.max(rect.x, bounds.x);
  const top = Math.max(rect.y, bounds.y);
  const right = Math.min(rect.x + rect.width, bounds.x + bounds.width);
  const bottom = Math.min(rect.y + rect.height, bounds.y + bounds.height);
  if (right <= left || bottom <= top) return null;
  return {
    left: ((left - bounds.x) / bounds.width) * 100,
    top: ((top - bounds.y) / bounds.height) * 100,
    width: ((right - left) / bounds.width) * 100,
    height: ((bottom - top) / bounds.height) * 100,
  };
}

/**
 * Lay out monitors and the visible windows of `apps`. Minimized windows are left out (they are
 * not on screen). Returns `null` when no monitor has a usable size.
 */
export function buildMiniMap(
  monitors: ReadonlyArray<Pick<MonitorInfo, "id" | "name" | "bounds" | "isPrimary">>,
  apps: readonly MapApp[],
): MiniMapLayout | null {
  const bounds = virtualBounds(monitors);
  if (!bounds) return null;

  const mapMonitors: MiniMapMonitor[] = [];
  for (const monitor of monitors) {
    const box = toMapBox(monitor.bounds, bounds);
    if (!box) continue;
    mapMonitors.push({
      id: monitor.id,
      name: monitor.name,
      isPrimary: monitor.isPrimary,
      box,
      width: monitor.bounds.width,
      height: monitor.bounds.height,
    });
  }

  const entries: Array<{ window: MiniMapWindow; z: number; order: number }> = [];
  for (const app of apps) {
    for (const window of app.windows) {
      if (window.state === "minimized") continue;
      const box = toMapBox(window.bounds, bounds);
      if (!box) continue;
      entries.push({
        window: { id: window.id, appKey: app.key, title: window.title, state: window.state, box },
        z: window.zOrder,
        order: entries.length,
      });
    }
  }
  // zOrder 0 is frontmost, so draw higher values first.
  entries.sort((a, b) => b.z - a.z || a.order - b.order);

  return {
    bounds,
    aspect: bounds.width / bounds.height,
    monitors: mapMonitors,
    windows: entries.map((entry) => entry.window),
  };
}

/** The largest size with the given aspect ratio that fits in `maxWidth` × `maxHeight`. */
export function fitSize(aspect: number, maxWidth: number, maxHeight: number): { width: number; height: number } {
  if (!(aspect > 0) || !(maxWidth > 0) || !(maxHeight > 0)) return { width: 0, height: 0 };
  const width = Math.min(maxWidth, maxHeight * aspect);
  return { width, height: width / aspect };
}
