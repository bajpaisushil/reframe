import { useCallback, useLayoutEffect, useState, type RefObject } from "react";
import { computePosition, type Placement, type Position, type Side } from "@/lib/position";

export interface FloatingPosition {
  x: number;
  y: number;
  side: Side;
  /** False until the first measurement (render hidden to avoid a jump). */
  ready: boolean;
}

const HIDDEN: FloatingPosition = { x: 0, y: 0, side: "bottom", ready: false };

/**
 * Position a fixed, portaled element next to an anchor and keep it there while the page scrolls
 * or the window resizes. Listeners exist only while `open`.
 */
export function useFloating(
  anchorRef: RefObject<HTMLElement | null>,
  floatingRef: RefObject<HTMLElement | null>,
  open: boolean,
  placement: Placement,
  options: { offset?: number; padding?: number } = {},
): FloatingPosition {
  const [position, setPosition] = useState<FloatingPosition>(HIDDEN);
  const { offset, padding } = options;

  const update = useCallback(() => {
    const anchor = anchorRef.current;
    const floating = floatingRef.current;
    if (!anchor || !floating) return;
    const a = anchor.getBoundingClientRect();
    const next: Position = computePosition(
      { x: a.left, y: a.top, width: a.width, height: a.height },
      { width: floating.offsetWidth, height: floating.offsetHeight },
      placement,
      { width: window.innerWidth, height: window.innerHeight },
      { offset, padding },
    );
    setPosition((prev) =>
      prev.ready && prev.x === next.x && prev.y === next.y && prev.side === next.side ? prev : { ...next, ready: true },
    );
  }, [anchorRef, floatingRef, placement, offset, padding]);

  useLayoutEffect(() => {
    if (!open) {
      setPosition(HIDDEN);
      return;
    }
    update();
    let frame = 0;
    const schedule = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(update);
    };
    window.addEventListener("resize", schedule);
    window.addEventListener("scroll", schedule, true);
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(schedule);
    if (observer && floatingRef.current) observer.observe(floatingRef.current);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener("resize", schedule);
      window.removeEventListener("scroll", schedule, true);
      observer?.disconnect();
    };
  }, [open, update, floatingRef]);

  return position;
}
