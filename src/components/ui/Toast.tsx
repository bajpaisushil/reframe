import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import { CircleCheck, CircleX, Info, X } from "lucide-react";
import { cx } from "@/lib/cx";
import { Icon, type LucideIcon } from "./Icon";
import styles from "./Toast.module.css";

export type ToastTone = "success" | "error" | "info";

export interface ToastOptions {
  tone?: ToastTone;
  title: string;
  description?: string;
  /** Auto-dismiss delay in ms (default 4 s; errors 7 s). `0` keeps it until closed. */
  duration?: number;
}

interface ToastRecord extends Required<Omit<ToastOptions, "description">> {
  id: number;
  description?: string;
  leaving: boolean;
}

export interface ToastApi {
  show: (options: ToastOptions) => number;
  success: (title: string, description?: string) => number;
  error: (title: string, description?: string) => number;
  info: (title: string, description?: string) => number;
  dismiss: (id: number) => void;
}

const ToastContext = createContext<ToastApi | null>(null);

const ICONS: Record<ToastTone, LucideIcon> = { success: CircleCheck, error: CircleX, info: Info };
const MAX_VISIBLE = 4;
const EXIT_MS = 160;

/** Provides `useToast()` and renders the toast stack plus screen-reader live regions. */
export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<ToastRecord[]>([]);
  const [announcement, setAnnouncement] = useState({ polite: "", assertive: "" });
  const nextId = useRef(1);
  const exitTimers = useRef(new Map<number, ReturnType<typeof setTimeout>>());
  const announceTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  const remove = useCallback((id: number) => {
    setToasts((list) => list.filter((t) => t.id !== id));
    exitTimers.current.delete(id);
  }, []);

  const dismiss = useCallback(
    (id: number) => {
      if (exitTimers.current.has(id)) return;
      setToasts((list) => list.map((t) => (t.id === id ? { ...t, leaving: true } : t)));
      exitTimers.current.set(
        id,
        setTimeout(() => remove(id), EXIT_MS),
      );
    },
    [remove],
  );

  const show = useCallback((options: ToastOptions) => {
    const id = nextId.current++;
    const tone = options.tone ?? "info";
    const record: ToastRecord = {
      id,
      tone,
      title: options.title,
      description: options.description,
      duration: options.duration ?? (tone === "error" ? 7000 : 4000),
      leaving: false,
    };
    setToasts((list) => [...list, record].slice(-MAX_VISIBLE));
    const text = options.description ? `${options.title}. ${options.description}` : options.title;
    // Clear first so repeating the same message is announced again.
    setAnnouncement({ polite: "", assertive: "" });
    clearTimeout(announceTimer.current);
    announceTimer.current = setTimeout(() => {
      setAnnouncement(tone === "error" ? { polite: "", assertive: text } : { polite: text, assertive: "" });
    }, 50);
    return id;
  }, []);

  useEffect(() => {
    const timers = exitTimers.current;
    return () => {
      for (const timer of timers.values()) clearTimeout(timer);
      timers.clear();
      clearTimeout(announceTimer.current);
    };
  }, []);

  const api = useMemo<ToastApi>(
    () => ({
      show,
      dismiss,
      success: (title, description) => show({ tone: "success", title, description }),
      error: (title, description) => show({ tone: "error", title, description }),
      info: (title, description) => show({ tone: "info", title, description }),
    }),
    [show, dismiss],
  );

  return (
    <ToastContext.Provider value={api}>
      {children}
      {typeof document !== "undefined" &&
        createPortal(
          <>
            <div className="sr-only" role="status" aria-live="polite" aria-atomic="true">
              {announcement.polite}
            </div>
            <div className="sr-only" role="alert" aria-live="assertive" aria-atomic="true">
              {announcement.assertive}
            </div>
            <section className={styles.viewport} aria-label="Notifications">
              <ol className={styles.list}>
                {toasts.map((toast) => (
                  <ToastItem key={toast.id} toast={toast} onDismiss={dismiss} />
                ))}
              </ol>
            </section>
          </>,
          document.body,
        )}
    </ToastContext.Provider>
  );
}

function ToastItem({ toast, onDismiss }: { toast: ToastRecord; onDismiss: (id: number) => void }) {
  const [paused, setPaused] = useState(false);
  const remaining = useRef(toast.duration);
  const startedAt = useRef(0);

  useEffect(() => {
    if (toast.duration === 0 || paused || toast.leaving) return;
    startedAt.current = Date.now();
    const timer = setTimeout(() => onDismiss(toast.id), remaining.current);
    return () => {
      clearTimeout(timer);
      remaining.current = Math.max(800, remaining.current - (Date.now() - startedAt.current));
    };
  }, [paused, toast.duration, toast.id, toast.leaving, onDismiss]);

  return (
    <li
      className={cx(styles.toast, styles[toast.tone])}
      data-state={toast.leaving ? "closed" : "open"}
      onPointerEnter={() => setPaused(true)}
      onPointerLeave={() => setPaused(false)}
      onFocus={() => setPaused(true)}
      onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setPaused(false);
      }}
    >
      <Icon icon={ICONS[toast.tone]} size={18} className={styles.icon} />
      <div className={styles.text}>
        <p className={styles.title}>{toast.title}</p>
        {toast.description && <p className={styles.description}>{toast.description}</p>}
      </div>
      <button type="button" className={styles.close} aria-label="Dismiss notification" onClick={() => onDismiss(toast.id)}>
        <Icon icon={X} size={14} />
      </button>
    </li>
  );
}

/** Show a toast: `toast.success("Workspace created")`, `toast.error("Couldn't restore", message)`. */
export function useToast(): ToastApi {
  const context = useContext(ToastContext);
  if (!context) throw new Error("useToast must be used inside <ToastProvider>");
  return context;
}
