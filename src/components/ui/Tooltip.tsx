import {
  cloneElement,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type FocusEvent,
  type ReactElement,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import { useFloating } from "@/hooks/useFloating";
import type { Placement } from "@/lib/position";
import styles from "./Tooltip.module.css";

export interface TooltipProps {
  content: ReactNode;
  /** A single focusable element (button, link, badge with tabIndex). */
  children: ReactElement<{ "aria-describedby"?: string }>;
  placement?: Placement;
  /** Hover delay in ms (keyboard focus shows immediately). */
  delay?: number;
  disabled?: boolean;
  /**
   * Also expose the text as the trigger's accessible description. Leave off when the tooltip only
   * repeats the trigger's label (icon buttons), to avoid announcing it twice.
   */
  describe?: boolean;
}

/** Moving from one tooltip to the next shortly after skips the delay, like native toolbars. */
let lastHiddenAt = 0;
const SKIP_DELAY_WINDOW = 400;

function isKeyboardFocus(target: EventTarget): boolean {
  try {
    return target instanceof Element && target.matches(":focus-visible");
  } catch {
    return false;
  }
}

export function Tooltip({ content, children, placement = "top", delay = 450, disabled = false, describe = false }: TooltipProps) {
  const id = useId();
  const wrapperRef = useRef<HTMLSpanElement>(null);
  const tooltipRef = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  // The wrapper uses `display: contents`; position against the actual child element.
  const anchorRef = useMemo(
    () => ({
      get current(): HTMLElement | null {
        const el = wrapperRef.current?.firstElementChild;
        return el instanceof HTMLElement ? el : null;
      },
    }),
    [],
  );
  const visible = open && !disabled && content !== null && content !== undefined && content !== "";
  const position = useFloating(anchorRef, tooltipRef, visible, placement, { offset: 6 });

  const show = (immediate: boolean) => {
    clearTimeout(timer.current);
    const wait = immediate || Date.now() - lastHiddenAt < SKIP_DELAY_WINDOW ? 0 : delay;
    timer.current = setTimeout(() => setOpen(true), wait);
  };

  const hide = () => {
    clearTimeout(timer.current);
    setOpen((wasOpen) => {
      if (wasOpen) lastHiddenAt = Date.now();
      return false;
    });
  };

  useEffect(() => () => clearTimeout(timer.current), []);

  useEffect(() => {
    if (!visible) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [visible]);

  const trigger = describe && visible ? cloneElement(children, { "aria-describedby": id }) : children;

  return (
    <span
      ref={wrapperRef}
      className={styles.anchor}
      onPointerEnter={(event) => {
        if (event.pointerType !== "touch") show(false);
      }}
      onPointerLeave={hide}
      onPointerDown={hide}
      onFocus={(event: FocusEvent) => {
        if (isKeyboardFocus(event.target)) show(true);
      }}
      onBlur={hide}
    >
      {trigger}
      {visible &&
        createPortal(
          <div
            ref={tooltipRef}
            id={id}
            role="tooltip"
            className={styles.tooltip}
            data-side={position.side}
            data-ready={position.ready || undefined}
            style={{ left: position.x, top: position.y }}
          >
            {content}
          </div>,
          document.body,
        )}
    </span>
  );
}
