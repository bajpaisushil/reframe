import { useEffect, useRef, type ReactNode } from "react";
import { useNavigateRequests } from "@/hooks/useNavigateRequests";
import { hasPrimaryModifier, isMacLike } from "@/lib/platform";
import { back, getRouter, navigate, useRoute } from "@/lib/router";
import { NAV_ITEMS, Sidebar } from "./Sidebar";
import styles from "./AppShell.module.css";

/** Keyboard shortcuts handled by the shell (for display in Settings or help). */
export const SHELL_SHORTCUTS = [
  { keys: "1 … 5", label: "Go to Home, Explore, Workspaces, Activity, Settings", primary: true },
  { keys: ",", label: "Open Settings", primary: true },
  { keys: isMacLike() ? "[" : "Alt ←", label: "Go back", primary: isMacLike() },
] as const;

function isEditable(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  return target.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName);
}

/**
 * Sidebar + scrolling content sheet. Resets scroll and moves focus to the title on navigation,
 * and follows navigation requests from the system tray.
 */
export function AppShell({ children }: { children: ReactNode }) {
  const { path } = useRoute();
  const mainRef = useRef<HTMLElement>(null);
  const firstRender = useRef(true);
  useNavigateRequests();

  useEffect(() => {
    const main = mainRef.current;
    if (!main) return;
    main.scrollTop = 0;
    if (firstRender.current) {
      firstRender.current = false;
      return;
    }
    // Let the new page render, then announce it by focusing its title (unless the page already
    // put the caret in a field).
    const frame = requestAnimationFrame(() => {
      if (isEditable(document.activeElement)) return;
      main.querySelector<HTMLElement>("[data-page-title]")?.focus({ preventScroll: true });
    });
    return () => cancelAnimationFrame(frame);
  }, [path]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented || event.repeat) return;
      // Nothing behind a modal dialog should react.
      if (document.getElementById("root")?.inert) return;
      const mac = isMacLike();
      if (hasPrimaryModifier(event) && !event.shiftKey && !event.altKey) {
        const index = Number.parseInt(event.key, 10);
        if (index >= 1 && index <= NAV_ITEMS.length) {
          event.preventDefault();
          navigate(NAV_ITEMS[index - 1].route);
          return;
        }
        if (event.key === ",") {
          event.preventDefault();
          navigate({ name: "settings" });
          return;
        }
        if (mac && event.key === "[" && getRouter().canGoBack()) {
          event.preventDefault();
          back();
          return;
        }
      }
      if (!mac && event.altKey && event.key === "ArrowLeft" && !isEditable(event.target) && getRouter().canGoBack()) {
        event.preventDefault();
        back();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  return (
    <div className={styles.shell}>
      <a
        href="#main"
        className={styles.skipLink}
        onClick={(event) => {
          event.preventDefault();
          mainRef.current?.querySelector<HTMLElement>("[data-page-title]")?.focus();
        }}
      >
        Skip to content
      </a>
      <Sidebar />
      <main id="main" ref={mainRef} className={styles.main}>
        <div key={path} className={styles.view}>
          {children}
        </div>
      </main>
    </div>
  );
}
