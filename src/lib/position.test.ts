import { describe, expect, it } from "vitest";
import { computePosition, parsePlacement } from "./position";

const viewport = { width: 1000, height: 800 };
const tooltip = { width: 100, height: 30 };

describe("parsePlacement", () => {
  it("splits side and alignment", () => {
    expect(parsePlacement("top")).toEqual({ side: "top", align: "center" });
    expect(parsePlacement("bottom-end")).toEqual({ side: "bottom", align: "end" });
  });
});

describe("computePosition", () => {
  it("centers above the anchor", () => {
    const anchor = { x: 450, y: 400, width: 100, height: 20 };
    expect(computePosition(anchor, tooltip, "top", viewport)).toEqual({ x: 450, y: 364, side: "top" });
  });

  it("aligns start/end on the cross axis", () => {
    const anchor = { x: 400, y: 100, width: 200, height: 32 };
    const menu = { width: 160, height: 120 };
    expect(computePosition(anchor, menu, "bottom-start", viewport, { offset: 4 })).toMatchObject({ x: 400, y: 136 });
    expect(computePosition(anchor, menu, "bottom-end", viewport, { offset: 4 })).toMatchObject({ x: 440, y: 136 });
  });

  it("flips to the opposite side when there is not enough room", () => {
    const nearTop = { x: 450, y: 10, width: 100, height: 20 };
    expect(computePosition(nearTop, tooltip, "top", viewport).side).toBe("bottom");
    const nearBottom = { x: 450, y: 780, width: 100, height: 16 };
    const menu = { width: 160, height: 200 };
    const placed = computePosition(nearBottom, menu, "bottom-start", viewport);
    expect(placed.side).toBe("top");
    expect(placed.y).toBe(780 - 6 - 200);
  });

  it("keeps the element inside the viewport padding", () => {
    const nearRight = { x: 960, y: 400, width: 30, height: 20 };
    expect(computePosition(nearRight, tooltip, "top", viewport).x).toBe(1000 - 100 - 8);
    const nearLeft = { x: 0, y: 400, width: 10, height: 20 };
    expect(computePosition(nearLeft, tooltip, "top", viewport).x).toBe(8);
  });

  it("positions to the side", () => {
    const anchor = { x: 100, y: 100, width: 40, height: 40 };
    expect(computePosition(anchor, tooltip, "right", viewport)).toEqual({ x: 146, y: 105, side: "right" });
    expect(computePosition(anchor, tooltip, "left", viewport).side).toBe("right");
  });

  it("copes with floating elements larger than the viewport", () => {
    const anchor = { x: 10, y: 10, width: 10, height: 10 };
    const huge = { width: 2000, height: 2000 };
    const placed = computePosition(anchor, huge, "bottom", viewport);
    expect(placed.x).toBe(8);
    expect(placed.y).toBe(8);
  });
});
