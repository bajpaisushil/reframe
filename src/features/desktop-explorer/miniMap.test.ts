import { describe, expect, it } from "vitest";
import type { Rect, WindowState } from "@/types";
import { buildMiniMap, fitSize, toMapBox, virtualBounds } from "./miniMap";

const rect = (x: number, y: number, width: number, height: number): Rect => ({ x, y, width, height });
const monitor = (id: string, bounds: Rect, isPrimary = false) => ({ id, name: id.toUpperCase(), bounds, isPrimary });
const win = (id: string, bounds: Rect, zOrder: number, state: WindowState = "normal") => ({
  id,
  title: `Window ${id}`,
  bounds,
  state,
  zOrder,
});

describe("virtualBounds", () => {
  it("is the bounding box of all monitors, including negative coordinates", () => {
    expect(virtualBounds([{ bounds: rect(0, 0, 1920, 1080) }, { bounds: rect(-1280, 200, 1280, 1024) }])).toEqual(
      rect(-1280, 0, 3200, 1224),
    );
  });

  it("ignores monitors without a usable size and returns null when none is left", () => {
    expect(virtualBounds([{ bounds: rect(0, 0, 0, 1080) }])).toBeNull();
    expect(virtualBounds([])).toBeNull();
    expect(virtualBounds([{ bounds: rect(0, 0, 0, 0) }, { bounds: rect(10, 10, 100, 50) }])).toEqual(rect(10, 10, 100, 50));
  });
});

describe("toMapBox", () => {
  const bounds = rect(0, 0, 2000, 1000);

  it("converts to percentages of the virtual desktop", () => {
    expect(toMapBox(rect(500, 250, 1000, 500), bounds)).toEqual({ left: 25, top: 25, width: 50, height: 50 });
  });

  it("clips rectangles that stick out", () => {
    expect(toMapBox(rect(-100, -100, 600, 350), bounds)).toEqual({ left: 0, top: 0, width: 25, height: 25 });
    expect(toMapBox(rect(1800, 900, 400, 400), bounds)).toEqual({ left: 90, top: 90, width: 10, height: 10 });
  });

  it("returns null for rectangles outside or without area", () => {
    expect(toMapBox(rect(2100, 0, 100, 100), bounds)).toBeNull();
    expect(toMapBox(rect(10, 10, 0, 100), bounds)).toBeNull();
    expect(toMapBox(rect(Number.NaN, 10, 10, 100), bounds)).toBeNull();
  });
});

describe("buildMiniMap", () => {
  const monitors = [monitor("hdmi-1", rect(0, 0, 1920, 1080), true), monitor("dp-1", rect(1920, 0, 1920, 1080))];

  it("lays monitors side by side and reports the aspect ratio", () => {
    const map = buildMiniMap(monitors, []);
    expect(map?.aspect).toBeCloseTo(3840 / 1080);
    expect(map?.monitors.map((m) => [m.id, m.box.left, m.box.width, m.isPrimary])).toEqual([
      ["hdmi-1", 0, 50, true],
      ["dp-1", 50, 50, false],
    ]);
  });

  it("draws windows back to front and leaves minimized ones out", () => {
    const map = buildMiniMap(monitors, [
      { key: "code", windows: [win("front", rect(0, 0, 960, 540), 0), win("hidden", rect(0, 0, 100, 100), 1, "minimized")] },
      { key: "term", windows: [win("back", rect(1920, 0, 960, 540), 2), win("middle", rect(100, 100, 800, 600), 1)] },
    ]);
    expect(map?.windows.map((w) => [w.id, w.appKey])).toEqual([
      ["back", "term"],
      ["middle", "term"],
      ["front", "code"],
    ]);
    expect(map?.windows[2].box).toEqual({ left: 0, top: 0, width: 25, height: 50 });
  });

  it("keeps the snapshot order for windows with the same stacking value", () => {
    const map = buildMiniMap(monitors, [
      { key: "a", windows: [win("1", rect(0, 0, 10, 10), 0)] },
      { key: "b", windows: [win("2", rect(0, 0, 10, 10), 0)] },
    ]);
    expect(map?.windows.map((w) => w.id)).toEqual(["1", "2"]);
  });

  it("skips windows entirely off screen", () => {
    const map = buildMiniMap(monitors, [{ key: "a", windows: [win("off", rect(-5000, 0, 100, 100), 0)] }]);
    expect(map?.windows).toEqual([]);
  });

  it("is null without usable monitors", () => {
    expect(buildMiniMap([], [{ key: "a", windows: [win("1", rect(0, 0, 10, 10), 0)] }])).toBeNull();
  });
});

describe("fitSize", () => {
  it("is limited by height for wide layouts and by width for tall ones", () => {
    expect(fitSize(32 / 9, 1000, 180)).toEqual({ width: 640, height: 180 });
    expect(fitSize(16 / 9, 320, 400)).toEqual({ width: 320, height: 180 });
  });

  it("returns zero for invalid input", () => {
    expect(fitSize(0, 100, 100)).toEqual({ width: 0, height: 0 });
    expect(fitSize(Number.NaN, 100, 100)).toEqual({ width: 0, height: 0 });
    expect(fitSize(1, 0, 100)).toEqual({ width: 0, height: 0 });
  });
});
