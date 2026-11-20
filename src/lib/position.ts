// Positioning for floating UI (tooltips, menus, popovers): place next to an anchor, flip to the
// opposite side when there is not enough room, and keep inside the viewport. Pure (tested).

export type Side = "top" | "bottom" | "left" | "right";
export type Align = "start" | "center" | "end";
export type Placement = Side | `${Side}-${Exclude<Align, "center">}`;

export interface Box {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface Size {
  width: number;
  height: number;
}

export interface Position {
  x: number;
  y: number;
  side: Side;
}

export interface PositionOptions {
  /** Gap between anchor and floating element. */
  offset?: number;
  /** Minimum distance from the viewport edges. */
  padding?: number;
}

const OPPOSITE: Record<Side, Side> = { top: "bottom", bottom: "top", left: "right", right: "left" };

export function parsePlacement(placement: Placement): { side: Side; align: Align } {
  const [side, align] = placement.split("-") as [Side, Exclude<Align, "center"> | undefined];
  return { side, align: align ?? "center" };
}

function clamp(value: number, min: number, max: number): number {
  if (max < min) return min;
  return Math.min(Math.max(value, min), max);
}

function spaceOn(side: Side, anchor: Box, viewport: Size): number {
  switch (side) {
    case "top":
      return anchor.y;
    case "bottom":
      return viewport.height - (anchor.y + anchor.height);
    case "left":
      return anchor.x;
    case "right":
      return viewport.width - (anchor.x + anchor.width);
  }
}

function alignedStart(align: Align, anchorStart: number, anchorSize: number, floatingSize: number): number {
  switch (align) {
    case "start":
      return anchorStart;
    case "end":
      return anchorStart + anchorSize - floatingSize;
    case "center":
      return anchorStart + (anchorSize - floatingSize) / 2;
  }
}

/** Where to put a floating element of `floating` size next to `anchor` (viewport coordinates). */
export function computePosition(
  anchor: Box,
  floating: Size,
  placement: Placement,
  viewport: Size,
  options: PositionOptions = {},
): Position {
  const offset = options.offset ?? 6;
  const padding = options.padding ?? 8;
  const { side: preferred, align } = parsePlacement(placement);
  const vertical = preferred === "top" || preferred === "bottom";
  const needed = (vertical ? floating.height : floating.width) + offset + padding;

  let side = preferred;
  if (spaceOn(preferred, anchor, viewport) < needed) {
    const opposite = OPPOSITE[preferred];
    if (spaceOn(opposite, anchor, viewport) > spaceOn(preferred, anchor, viewport)) side = opposite;
  }

  let x: number;
  let y: number;
  if (side === "top" || side === "bottom") {
    y = side === "top" ? anchor.y - offset - floating.height : anchor.y + anchor.height + offset;
    x = alignedStart(align, anchor.x, anchor.width, floating.width);
  } else {
    x = side === "left" ? anchor.x - offset - floating.width : anchor.x + anchor.width + offset;
    y = alignedStart(align, anchor.y, anchor.height, floating.height);
  }

  return {
    x: Math.round(clamp(x, padding, viewport.width - floating.width - padding)),
    y: Math.round(clamp(y, padding, viewport.height - floating.height - padding)),
    side,
  };
}
