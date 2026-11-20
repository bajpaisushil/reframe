import { createContext, useContext, useId, useRef, type KeyboardEvent, type ReactNode } from "react";
import { cx } from "@/lib/cx";
import { formatCount } from "@/lib/format";
import styles from "./Tabs.module.css";

export interface TabItem<T extends string> {
  value: T;
  label: string;
  /** Optional count badge ("Applications 12"). */
  count?: number;
  disabled?: boolean;
}

const TabsContext = createContext<{ baseId: string; value: string } | null>(null);

export interface TabsProps<T extends string> {
  value: T;
  onChange: (value: T) => void;
  items: readonly TabItem<T>[];
  "aria-label": string;
  /** `TabPanel`s for the items. */
  children?: ReactNode;
  className?: string;
}

const tabId = (base: string, value: string) => `${base}-tab-${value}`;
const panelId = (base: string, value: string) => `${base}-panel-${value}`;

/** Tabs with automatic activation (arrow keys switch tabs), underline indicator. */
export function Tabs<T extends string>({ value, onChange, items, children, className, ...rest }: TabsProps<T>) {
  const baseId = useId();
  const refs = useRef<Array<HTMLButtonElement | null>>([]);
  const enabled = items.map((item, i) => (item.disabled ? -1 : i)).filter((i) => i !== -1);

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (!["ArrowRight", "ArrowLeft", "Home", "End"].includes(event.key) || enabled.length === 0) return;
    event.preventDefault();
    const current = Math.max(0, enabled.indexOf(items.findIndex((item) => item.value === value)));
    let next = current;
    if (event.key === "Home") next = 0;
    else if (event.key === "End") next = enabled.length - 1;
    else next = (current + (event.key === "ArrowRight" ? 1 : -1) + enabled.length) % enabled.length;
    const index = enabled[next];
    onChange(items[index].value);
    refs.current[index]?.focus();
  };

  return (
    <TabsContext.Provider value={{ baseId, value }}>
      <div role="tablist" aria-label={rest["aria-label"]} className={cx(styles.list, className)} onKeyDown={onKeyDown}>
        {items.map((item, index) => {
          const selected = item.value === value;
          return (
            <button
              key={item.value}
              ref={(el) => {
                refs.current[index] = el;
              }}
              id={tabId(baseId, item.value)}
              type="button"
              role="tab"
              aria-selected={selected}
              aria-controls={panelId(baseId, item.value)}
              tabIndex={selected ? 0 : -1}
              disabled={item.disabled}
              className={cx(styles.tab, selected && styles.selected)}
              onClick={() => onChange(item.value)}
            >
              {item.label}
              {item.count !== undefined && <span className={styles.count}>{formatCount(item.count)}</span>}
            </button>
          );
        })}
      </div>
      {children}
    </TabsContext.Provider>
  );
}

export interface TabPanelProps {
  value: string;
  children: ReactNode;
  className?: string;
}

/** Content for one tab; only the selected panel is rendered. */
export function TabPanel({ value, children, className }: TabPanelProps) {
  const context = useContext(TabsContext);
  if (!context) throw new Error("TabPanel must be used inside Tabs");
  if (context.value !== value) return null;
  return (
    <div
      role="tabpanel"
      id={panelId(context.baseId, value)}
      aria-labelledby={tabId(context.baseId, value)}
      tabIndex={0}
      className={cx(styles.panel, className)}
    >
      {children}
    </div>
  );
}
