import {
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type MouseEvent,
  type ReactNode,
  type RefObject,
} from "react";
import { createPortal } from "react-dom";
import { X } from "lucide-react";
import { usePresence } from "@/hooks/usePresence";
import { cx } from "@/lib/cx";
import { IconButton } from "./IconButton";
import { createLayerId, focusableElements, isTopLayer, pushLayer, removeLayer } from "./layers";
import styles from "./Dialog.module.css";

export interface DialogProps {
  open: boolean;
  /** Called on Escape, click outside, the × button — and by your own Cancel button. */
  onClose: () => void;
  title: ReactNode;
  description?: ReactNode;
  children?: ReactNode;
  /** Buttons, right-aligned in a footer bar. */
  footer?: ReactNode;
  size?: "sm" | "md" | "lg";
  /** False while busy (saving): Escape, outside clicks and × are ignored. */
  dismissible?: boolean;
  /** Element to focus on open; defaults to `[data-autofocus]`, then the first field or button. */
  initialFocusRef?: RefObject<HTMLElement | null>;
  hideCloseButton?: boolean;
  role?: "dialog" | "alertdialog";
  className?: string;
}

const EXIT_MS = 140;

/**
 * Modal dialog: rendered in a portal, labelled by its title, traps focus, restores focus to the
 * element that opened it, closes on Escape / outside click (unless not dismissible).
 */
export function Dialog(props: DialogProps) {
  const { mounted, closing } = usePresence(props.open, EXIT_MS);
  if (!mounted || typeof document === "undefined") return null;
  return createPortal(<DialogSurface {...props} closing={closing} />, document.body);
}

function DialogSurface({
  onClose,
  title,
  description,
  children,
  footer,
  size = "md",
  dismissible = true,
  initialFocusRef,
  hideCloseButton = false,
  role = "dialog",
  className,
  closing,
}: DialogProps & { closing: boolean }) {
  const titleId = useId();
  const descriptionId = useId();
  const dialogRef = useRef<HTMLDivElement>(null);
  const bodyRef = useRef<HTMLDivElement>(null);
  const [layerId] = useState(createLayerId);
  const latest = useRef({ onClose, dismissible });
  useLayoutEffect(() => {
    latest.current = { onClose, dismissible };
  });
  const pointerDownOutside = useRef(false);

  // Register as the top modal layer and move focus in; undo both as soon as closing starts.
  useLayoutEffect(() => {
    if (closing) return;
    const previouslyFocused = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    pushLayer({
      id: layerId,
      modal: true,
      getElement: () => dialogRef.current,
      onEscape: () => {
        if (latest.current.dismissible) latest.current.onClose();
      },
    });
    const dialog = dialogRef.current;
    if (dialog) {
      const preferred =
        initialFocusRef?.current ??
        dialog.querySelector<HTMLElement>("[data-autofocus]") ??
        (bodyRef.current ? focusableElements(bodyRef.current)[0] : undefined) ??
        focusableElements(dialog).find((el) => !el.dataset.dialogClose) ??
        dialog;
      preferred.focus({ preventScroll: true });
    }
    return () => {
      removeLayer(layerId);
      if (previouslyFocused?.isConnected) previouslyFocused.focus({ preventScroll: true });
    };
    // Mount-time setup only; `initialFocusRef` is read once on open by design.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [closing, layerId]);

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== "Tab" || !dialogRef.current) return;
    // Keys from portaled children (a menu or popover opened from the dialog) are theirs to handle.
    if (!(event.target instanceof Node) || !dialogRef.current.contains(event.target)) return;
    const items = focusableElements(dialogRef.current);
    if (items.length === 0) {
      event.preventDefault();
      return;
    }
    const first = items[0];
    const last = items[items.length - 1];
    const active = document.activeElement;
    if (event.shiftKey && (active === first || active === dialogRef.current)) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && active === last) {
      event.preventDefault();
      first.focus();
    }
  };

  // Close only when both press and release happen outside (dragging a text selection out of a
  // field must not dismiss the dialog).
  const onPositionerMouseDown = (event: MouseEvent<HTMLDivElement>) => {
    pointerDownOutside.current = event.target === event.currentTarget;
  };
  const onPositionerMouseUp = (event: MouseEvent<HTMLDivElement>) => {
    const outside = pointerDownOutside.current && event.target === event.currentTarget;
    pointerDownOutside.current = false;
    if (outside && latest.current.dismissible && isTopLayer(layerId)) latest.current.onClose();
  };

  return (
    <div className={styles.root} data-state={closing ? "closed" : "open"}>
      <div className={styles.scrim} aria-hidden="true" />
      <div className={styles.positioner} onMouseDown={onPositionerMouseDown} onMouseUp={onPositionerMouseUp}>
        <div
          ref={dialogRef}
          role={role}
          aria-modal="true"
          aria-labelledby={titleId}
          aria-describedby={description ? descriptionId : undefined}
          tabIndex={-1}
          className={cx(styles.dialog, styles[size], className)}
          onKeyDown={onKeyDown}
        >
          <div className={styles.header}>
            <div className={styles.heading}>
              <h2 id={titleId} className={styles.title}>
                {title}
              </h2>
              {description && (
                <p id={descriptionId} className={styles.description}>
                  {description}
                </p>
              )}
            </div>
            {!hideCloseButton && (
              <IconButton
                icon={X}
                aria-label="Close"
                size="sm"
                className={styles.close}
                data-dialog-close="true"
                disabled={!dismissible}
                onClick={() => latest.current.dismissible && latest.current.onClose()}
              />
            )}
          </div>
          {children !== undefined && children !== null && (
            <div ref={bodyRef} className={styles.body}>
              {children}
            </div>
          )}
          {footer && <div className={styles.footer}>{footer}</div>}
        </div>
      </div>
    </div>
  );
}
