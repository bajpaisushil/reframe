import { useLayoutEffect, useMemo, useRef, useState } from "react";
import { AppIcon, RelativeTime } from "@/components/ui";
import { cx } from "@/lib/cx";
import { pluralize } from "@/lib/format";
import type { DiscoveredApplication, MonitorInfo } from "@/types";
import { iconLookup, windowSize, windowTitle } from "./describe";
import { buildMiniMap, type MapBox } from "./miniMap";
import { resolveSelection, type Choices } from "./selection";
import styles from "./MonitorMap.module.css";

/** The app (and optionally one of its windows) under the pointer in the map or the list. */
export interface HoverTarget {
  appKey: string;
  windowId?: string;
}

export interface MonitorMapProps {
  monitors: readonly MonitorInfo[];
  apps: readonly DiscoveredApplication[];
  choices: Choices;
  hovered: HoverTarget | null;
  onHover: (target: HoverTarget | null) => void;
  /** Clicking a window toggles its whole application. */
  onToggleApp: (app: DiscoveredApplication) => void;
  /** When the snapshot was taken (shown in the caption). */
  capturedAt: string;
  scanning?: boolean;
  /** Tallest the diagram may get, in px. */
  maxHeight?: number;
}

/** Gap between adjacent monitors so they read as separate screens. */
const MONITOR_GAP = 2;
/** Smallest window (px) that shows its app icon. */
const ICON_MIN = { width: 40, height: 30 };
/** Narrowest monitor (px) that shows its name (longer names are truncated to the screen). */
const LABEL_MIN_WIDTH = 48;

function position(box: MapBox, inset = 0) {
  return {
    left: `calc(${box.left}% + ${inset}px)`,
    top: `calc(${box.top}% + ${inset}px)`,
    width: `calc(${box.width}% - ${inset * 2}px)`,
    height: `calc(${box.height}% - ${inset * 2}px)`,
  };
}

/**
 * A quiet, proportional miniature of the connected monitors with the windows drawn on them.
 * Selected apps' windows are tinted; hovering an app (here or in the list) brings its windows
 * forward. It is a visual aid only: everything it does is also possible from the list.
 */
export function MonitorMap({
  monitors,
  apps,
  choices,
  hovered,
  onHover,
  onToggleApp,
  capturedAt,
  scanning = false,
  maxHeight = 196,
}: MonitorMapProps) {
  const layout = useMemo(() => buildMiniMap(monitors, apps), [monitors, apps]);
  const stageRef = useRef<HTMLDivElement>(null);
  const [stageWidth, setStageWidth] = useState(0);

  useLayoutEffect(() => {
    const stage = stageRef.current;
    if (!stage) return;
    setStageWidth(stage.getBoundingClientRect().width);
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver((entries) => {
      const width = entries[0]?.contentRect.width;
      if (width !== undefined) setStageWidth(width);
    });
    observer.observe(stage);
    return () => observer.disconnect();
  }, [layout !== null]);

  const appsByKey = useMemo(() => new Map(apps.map((app) => [app.key, app])), [apps]);
  const selectedWindows = useMemo(() => {
    const ids = new Set<string>();
    for (const app of apps) for (const id of resolveSelection(app, choices)?.windowIds ?? []) ids.add(id);
    return ids;
  }, [apps, choices]);

  if (!layout) return null;
  const stageHeight = stageWidth / layout.aspect;
  const visibleWindows = layout.windows.length;
  const minimized = apps.reduce((n, app) => n + app.windows.filter((w) => w.state === "minimized").length, 0);

  const hoveredApp = hovered ? appsByKey.get(hovered.appKey) : undefined;
  const hoveredWindow = hovered?.windowId ? hoveredApp?.windows.find((w) => w.id === hovered.windowId) : undefined;
  let caption: string;
  if (hoveredApp && hoveredWindow) {
    caption = `${hoveredApp.displayName} — ${windowTitle(hoveredWindow.title)} · ${windowSize(hoveredWindow.bounds)}`;
  } else if (hoveredApp) {
    caption = `${hoveredApp.displayName} · ${pluralize(hoveredApp.windows.length, "window")}`;
  } else {
    caption = [
      pluralize(layout.monitors.length, "display"),
      pluralize(visibleWindows, "window"),
      minimized > 0 ? `${minimized} minimized` : null,
    ]
      .filter(Boolean)
      .join(" · ");
  }

  return (
    <figure className={styles.figure}>
      <div className={styles.desk} onMouseLeave={() => onHover(null)}>
        <div
          ref={stageRef}
          className={styles.stage}
          data-hovering={hoveredApp ? true : undefined}
          style={{ aspectRatio: `${layout.bounds.width} / ${layout.bounds.height}`, maxWidth: maxHeight * layout.aspect }}
          aria-hidden="true"
        >
          {layout.monitors.map((monitor) => (
            <div key={monitor.id} className={styles.monitor} style={position(monitor.box, MONITOR_GAP)} />
          ))}

          {layout.windows.map((window) => {
            const app = appsByKey.get(window.appKey);
            if (!app) return null;
            const width = (window.box.width / 100) * stageWidth;
            const height = (window.box.height / 100) * stageHeight;
            const showIcon = width >= ICON_MIN.width && height >= ICON_MIN.height;
            return (
              <div
                key={window.id}
                className={styles.window}
                style={position(window.box)}
                data-selected={selectedWindows.has(window.id) || undefined}
                data-highlighted={hovered?.appKey === window.appKey || undefined}
                data-focused={hovered?.windowId === window.id || undefined}
                onMouseEnter={() => onHover({ appKey: window.appKey, windowId: window.id })}
                onClick={() => onToggleApp(app)}
              >
                {showIcon && <AppIcon lookup={iconLookup(app)} name={app.displayName} size={20} className={styles.windowIcon} />}
              </div>
            );
          })}

          {layout.monitors.map((monitor) => {
            const width = (monitor.box.width / 100) * stageWidth;
            if (width < LABEL_MIN_WIDTH) return null;
            // Screens along the bottom edge are labelled underneath, on the desk; others (stacked
            // above another screen) carry their label in their bottom-left corner.
            const below = monitor.box.top + monitor.box.height >= 99.5;
            const bottom = monitor.box.top + monitor.box.height;
            return (
              <span
                key={`label-${monitor.id}`}
                className={cx(styles.monitorLabel, below && styles.monitorLabelBelow)}
                style={
                  below
                    ? { left: `calc(${monitor.box.left}% + ${MONITOR_GAP}px)`, top: "calc(100% + 6px)", maxWidth: width }
                    : { left: `calc(${monitor.box.left}% + 8px)`, top: `calc(${bottom}% - 8px)`, maxWidth: width - 16 }
                }
              >
                {monitor.name}
                {monitor.isPrimary && <span className={styles.primary}>Primary</span>}
              </span>
            );
          })}
        </div>
      </div>
      <figcaption className={styles.caption}>
        <span className={cx(styles.captionText, hoveredApp && styles.captionActive)}>{caption}</span>
        <span className={styles.captionSide}>
          {visibleWindows > 0 && (
            <span className={styles.legend} aria-hidden="true">
              <span className={cx(styles.swatch, styles.swatchSelected)} />
              Selected
              <span className={styles.swatch} />
              Not selected
            </span>
          )}
          <span className={styles.checked}>
            {scanning ? "Scanning…" : <RelativeTime value={capturedAt} prefix="Checked " />}
          </span>
        </span>
      </figcaption>
    </figure>
  );
}
