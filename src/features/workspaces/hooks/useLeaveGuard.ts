import { useCallback, useEffect, useRef, useState } from "react";
import { isMacLike } from "@/lib/platform";
import { back, getRouter, navigate } from "@/lib/router";

export interface LeaveGuard {
  /** A navigation away from the page is on hold: ask the user. */
  blocked: boolean;
  /** Stay on the page. */
  stay: () => void;
  /** Carry out the navigation that was put on hold. */
  proceed: () => void;
  /** Let the page's own next navigation through (e.g. after deleting the workspace). */
  allowNext: () => void;
}

function isEditable(target: EventTarget | null): boolean {
  return (
    target instanceof HTMLElement &&
    (target.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName))
  );
}

/**
 * Ask before leaving a page with unsaved changes. While `active`, any route change away from the
 * current page — sidebar links, shortcuts, tray requests, back — is undone before it renders and
 * held until the user decides (`proceed` or `stay`).
 *
 * The router has no blocking API, so a navigation is reverted synchronously from a router
 * subscription (React never renders the destination). That leaves one extra entry for the page in
 * the in-app history after a cancelled leave, which only makes a later "back" land on the same page.
 */
export function useLeaveGuard(active: boolean): LeaveGuard {
  // The held navigation lives in a ref so `proceed` works from any closure, even after a save
  // has already made the page clean again.
  const pending = useRef<(() => void) | null>(null);
  const [blocked, setBlocked] = useState(false);
  const bypass = useRef(false);

  const hold = useCallback((action: () => void) => {
    pending.current = action;
    setBlocked(true);
  }, []);

  useEffect(() => {
    if (!active) return;
    const router = getRouter();
    const home = router.getState().path;

    const unsubscribe = router.subscribe(() => {
      const { path } = router.getState();
      if (bypass.current || path === home) return;
      // Put the page back synchronously, so React never renders the destination.
      navigate(home, { replace: true });
      hold(() => navigate(path));
    });

    // "Back" shortcuts go through history, which settles asynchronously: intercept them up front
    // (the shell ignores keys whose default was prevented).
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented || document.getElementById("root")?.inert) return;
      const mac = isMacLike();
      const isBack = mac
        ? event.metaKey && !event.shiftKey && !event.altKey && event.key === "["
        : event.altKey && !event.ctrlKey && !event.metaKey && event.key === "ArrowLeft" && !isEditable(event.target);
      if (!isBack || !router.canGoBack()) return;
      event.preventDefault();
      hold(() => back());
    };
    window.addEventListener("keydown", onKeyDown, true);

    return () => {
      unsubscribe();
      window.removeEventListener("keydown", onKeyDown, true);
    };
  }, [active, hold]);

  const letThrough = useCallback(() => {
    bypass.current = true;
    // The navigation runs synchronously right after; re-arm the guard if the page is still here.
    setTimeout(() => {
      bypass.current = false;
    }, 0);
  }, []);

  const proceed = useCallback(() => {
    const action = pending.current;
    pending.current = null;
    setBlocked(false);
    if (!action) return;
    letThrough();
    action();
  }, [letThrough]);

  const stay = useCallback(() => {
    pending.current = null;
    setBlocked(false);
  }, []);

  return { blocked, stay, proceed, allowNext: letThrough };
}
