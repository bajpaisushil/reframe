import { useEffect, useLayoutEffect, useRef, useState, type RefObject } from "react";
import { createLayerId, isInsideLayerOrAbove, pushLayer, removeLayer } from "@/components/ui/layers";

export interface DismissableLayerOptions {
  open: boolean;
  /** Close request (Escape, pointer down outside). */
  onDismiss: (reason: "escape" | "outside") => void;
  contentRef: RefObject<HTMLElement | null>;
  /** The trigger counts as "inside" (clicking it toggles instead of dismiss-and-reopen). */
  triggerRef?: RefObject<HTMLElement | null>;
}

/**
 * Register a non-modal overlay (menu, popover) in the layer stack: Escape and outside presses
 * dismiss it, layers opened on top of it count as inside, and everything is cleaned up on close.
 */
export function useDismissableLayer({ open, onDismiss, contentRef, triggerRef }: DismissableLayerOptions): void {
  const [layerId] = useState(createLayerId);
  const latest = useRef(onDismiss);
  useLayoutEffect(() => {
    latest.current = onDismiss;
  });

  useLayoutEffect(() => {
    if (!open) return;
    pushLayer({
      id: layerId,
      modal: false,
      getElement: () => contentRef.current,
      getRelatedElements: () => [triggerRef?.current ?? null],
      onEscape: () => latest.current("escape"),
    });
    return () => removeLayer(layerId);
  }, [open, layerId, contentRef, triggerRef]);

  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => {
      if (!(event.target instanceof Node)) return;
      if (isInsideLayerOrAbove(layerId, event.target)) return;
      latest.current("outside");
    };
    document.addEventListener("pointerdown", onPointerDown, true);
    return () => document.removeEventListener("pointerdown", onPointerDown, true);
  }, [open, layerId]);
}
