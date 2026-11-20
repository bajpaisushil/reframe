import {
  useCallback,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type ReactElement,
  type Ref,
} from "react";
import { createPortal } from "react-dom";
import { Check } from "lucide-react";
import { useDismissableLayer } from "@/hooks/useDismissableLayer";
import { useFloating } from "@/hooks/useFloating";
import { cx } from "@/lib/cx";
import type { Placement } from "@/lib/position";
import { Icon, type LucideIcon } from "./Icon";
import styles from "./Menu.module.css";

export interface MenuItem {
  label: string;
  onSelect: () => void;
  icon?: LucideIcon;
  /** Destructive action (shown in red). */
  danger?: boolean;
  disabled?: boolean;
  /** Makes the item a checkable option (`menuitemradio`), e.g. theme choices. */
  checked?: boolean;
  /** Shortcut hint on the right ("⌘D"). */
  shortcut?: string;
  /** One-line explanation under the label. */
  description?: string;
}

export type MenuEntry = MenuItem | { type: "separator" } | { type: "label"; label: string };

export interface MenuTriggerProps {
  ref: Ref<HTMLButtonElement>;
  onClick: () => void;
  onKeyDown: (event: KeyboardEvent<HTMLButtonElement>) => void;
  "aria-haspopup": "menu";
  "aria-expanded": boolean;
  "aria-controls": string | undefined;
}

export interface MenuProps {
  /** Render the trigger with the given props (usually an IconButton or Button). */
  trigger: (props: MenuTriggerProps) => ReactElement;
  items: readonly MenuEntry[];
  placement?: Placement;
  /** Accessible name for the menu (defaults to the trigger's label via aria-labelledby). */
  "aria-label"?: string;
  minWidth?: number;
}

function isItem(entry: MenuEntry): entry is MenuItem {
  return !("type" in entry);
}

/** Overflow / actions menu: arrow keys, Home/End, typeahead, Enter/Space, Escape, Tab closes. */
export function Menu({ trigger, items, placement = "bottom-end", minWidth = 180, ...rest }: MenuProps) {
  const menuId = useId();
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const itemRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const [open, setOpen] = useState(false);
  const focusOnOpen = useRef<"first" | "last">("first");
  const typeahead = useRef({ text: "", timer: undefined as ReturnType<typeof setTimeout> | undefined });

  const position = useFloating(triggerRef, menuRef, open, placement, { offset: 4 });

  const enabledIndexes = items.map((entry, i) => (isItem(entry) && !entry.disabled ? i : -1)).filter((i) => i !== -1);

  const close = useCallback((restoreFocus: boolean) => {
    setOpen(false);
    if (restoreFocus) triggerRef.current?.focus({ preventScroll: true });
  }, []);

  useDismissableLayer({
    open,
    onDismiss: (reason) => close(reason === "escape"),
    contentRef: menuRef,
    triggerRef,
  });

  // Focus the first/last item once the menu is positioned.
  useLayoutEffect(() => {
    if (!open || !position.ready) return;
    const target = focusOnOpen.current === "last" ? enabledIndexes[enabledIndexes.length - 1] : enabledIndexes[0];
    if (target !== undefined) itemRefs.current[target]?.focus({ preventScroll: true });
    else menuRef.current?.focus({ preventScroll: true });
    // Only when opening.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, position.ready]);

  const focusIndex = (index: number | undefined) => {
    if (index !== undefined) itemRefs.current[index]?.focus({ preventScroll: true });
  };

  const currentEnabledPosition = () => {
    const activeIndex = itemRefs.current.findIndex((el) => el === document.activeElement);
    return enabledIndexes.indexOf(activeIndex);
  };

  const select = (item: MenuItem) => {
    if (item.disabled) return;
    close(true);
    item.onSelect();
  };

  const onMenuKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const pos = currentEnabledPosition();
    switch (event.key) {
      case "ArrowDown":
        event.preventDefault();
        focusIndex(enabledIndexes[(pos + 1 + enabledIndexes.length) % enabledIndexes.length]);
        return;
      case "ArrowUp":
        event.preventDefault();
        focusIndex(enabledIndexes[(pos - 1 + enabledIndexes.length) % enabledIndexes.length]);
        return;
      case "Home":
        event.preventDefault();
        focusIndex(enabledIndexes[0]);
        return;
      case "End":
        event.preventDefault();
        focusIndex(enabledIndexes[enabledIndexes.length - 1]);
        return;
      case "Tab":
        event.preventDefault();
        close(true);
        return;
      default:
        break;
    }
    if (event.key.length === 1 && !event.ctrlKey && !event.metaKey && !event.altKey && event.key !== " ") {
      const state = typeahead.current;
      clearTimeout(state.timer);
      state.text += event.key.toLowerCase();
      state.timer = setTimeout(() => (state.text = ""), 600);
      const ordered = [...enabledIndexes.slice(pos + 1), ...enabledIndexes.slice(0, pos + 1)];
      const match = ordered.find((i) => {
        const entry = items[i];
        return isItem(entry) && entry.label.toLowerCase().startsWith(state.text);
      });
      focusIndex(match);
    }
  };

  const openMenu = (from: "first" | "last") => {
    focusOnOpen.current = from;
    setOpen(true);
  };

  const triggerProps: MenuTriggerProps = {
    ref: triggerRef,
    onClick: () => (open ? close(false) : openMenu("first")),
    onKeyDown: (event) => {
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        openMenu(event.key === "ArrowUp" ? "last" : "first");
      }
    },
    "aria-haspopup": "menu",
    "aria-expanded": open,
    "aria-controls": open ? menuId : undefined,
  };

  return (
    <>
      {trigger(triggerProps)}
      {open &&
        createPortal(
          <div
            ref={menuRef}
            id={menuId}
            role="menu"
            aria-orientation="vertical"
            aria-label={rest["aria-label"]}
            tabIndex={-1}
            className={styles.menu}
            data-ready={position.ready || undefined}
            data-side={position.side}
            style={{ left: position.x, top: position.y, minWidth }}
            onKeyDown={onMenuKeyDown}
          >
            {items.map((entry, index) => {
              if (!isItem(entry)) {
                if (entry.type === "separator") return <div key={`sep-${index}`} role="separator" className={styles.separator} />;
                return (
                  <div key={`label-${index}`} role="presentation" className={styles.groupLabel}>
                    {entry.label}
                  </div>
                );
              }
              const checkable = entry.checked !== undefined;
              return (
                <button
                  key={`${entry.label}-${index}`}
                  ref={(el) => {
                    itemRefs.current[index] = el;
                  }}
                  type="button"
                  role={checkable ? "menuitemradio" : "menuitem"}
                  aria-checked={checkable ? entry.checked : undefined}
                  aria-disabled={entry.disabled || undefined}
                  tabIndex={-1}
                  className={cx(styles.item, entry.danger && styles.danger, entry.disabled && styles.disabled)}
                  onClick={() => select(entry)}
                  onPointerMove={(event) => {
                    if (!entry.disabled && document.activeElement !== event.currentTarget) {
                      event.currentTarget.focus({ preventScroll: true });
                    }
                  }}
                  onKeyDown={(event) => {
                    if (event.key === "Enter" || event.key === " ") {
                      event.preventDefault();
                      select(entry);
                    }
                  }}
                >
                  {checkable ? (
                    <span className={styles.check} aria-hidden="true">
                      {entry.checked && <Icon icon={Check} size={14} />}
                    </span>
                  ) : (
                    entry.icon && <Icon icon={entry.icon} size={16} className={styles.itemIcon} />
                  )}
                  <span className={styles.itemText}>
                    <span className={styles.itemLabel}>{entry.label}</span>
                    {entry.description && <span className={styles.itemDescription}>{entry.description}</span>}
                  </span>
                  {entry.shortcut && <span className={styles.shortcut}>{entry.shortcut}</span>}
                </button>
              );
            })}
          </div>,
          document.body,
        )}
    </>
  );
}
