import { useCallback, useId, useLayoutEffect, useRef, type ReactElement, type ReactNode, type Ref } from "react";
import { createPortal } from "react-dom";
import { useDismissableLayer } from "@/hooks/useDismissableLayer";
import { useFloating } from "@/hooks/useFloating";
import { cx } from "@/lib/cx";
import type { Placement } from "@/lib/position";
import { focusableElements } from "./layers";
import styles from "./Popover.module.css";

export interface PopoverTriggerProps {
  ref: Ref<HTMLButtonElement>;
  onClick: () => void;
  "aria-haspopup": "dialog";
  "aria-expanded": boolean;
  "aria-controls": string | undefined;
}

export interface PopoverProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  trigger: (props: PopoverTriggerProps) => ReactElement;
  /** Content; receives `close()` to dismiss and return focus to the trigger. */
  children: ReactNode | ((close: () => void) => ReactNode);
  placement?: Placement;
  /** Accessible name of the popover panel. */
  "aria-label": string;
  className?: string;
}

/**
 * Non-modal floating panel anchored to a trigger (works inside dialogs). Focus moves into the
 * panel on open and back to the trigger on Escape or selection; outside clicks dismiss it.
 */
export function Popover({ open, onOpenChange, trigger, children, placement = "bottom-start", className, ...rest }: PopoverProps) {
  const id = useId();
  const triggerRef = useRef<HTMLButtonElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const position = useFloating(triggerRef, panelRef, open, placement, { offset: 6 });

  const close = useCallback(() => {
    onOpenChange(false);
    triggerRef.current?.focus({ preventScroll: true });
  }, [onOpenChange]);

  useDismissableLayer({
    open,
    onDismiss: (reason) => (reason === "escape" ? close() : onOpenChange(false)),
    contentRef: panelRef,
    triggerRef,
  });

  useLayoutEffect(() => {
    if (!open || !position.ready || !panelRef.current) return;
    if (panelRef.current.contains(document.activeElement)) return;
    const preferred =
      panelRef.current.querySelector<HTMLElement>("[data-autofocus]") ?? focusableElements(panelRef.current)[0] ?? panelRef.current;
    preferred.focus({ preventScroll: true });
  }, [open, position.ready]);

  return (
    <>
      {trigger({
        ref: triggerRef,
        onClick: () => onOpenChange(!open),
        "aria-haspopup": "dialog",
        "aria-expanded": open,
        "aria-controls": open ? id : undefined,
      })}
      {open &&
        createPortal(
          <div
            ref={panelRef}
            id={id}
            role="dialog"
            aria-label={rest["aria-label"]}
            tabIndex={-1}
            className={cx(styles.popover, className)}
            data-ready={position.ready || undefined}
            style={{ left: position.x, top: position.y }}
            onKeyDown={(event) => {
              // Keep Tab inside the panel; Escape (handled by the layer stack) returns to the trigger.
              if (event.key !== "Tab" || !panelRef.current) return;
              const items = focusableElements(panelRef.current);
              if (items.length === 0) return;
              const first = items[0];
              const last = items[items.length - 1];
              if (event.shiftKey && document.activeElement === first) {
                event.preventDefault();
                last.focus();
              } else if (!event.shiftKey && document.activeElement === last) {
                event.preventDefault();
                first.focus();
              }
            }}
          >
            {typeof children === "function" ? children(close) : children}
          </div>,
          document.body,
        )}
    </>
  );
}
