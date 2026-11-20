// A shared, coarse clock for time-relative UI ("2 min ago", greetings). One interval serves every
// subscriber and stops when nobody is listening; returning to the window ticks immediately.

import { useSyncExternalStore } from "react";

const TICK_MS = 15_000;

let now = Date.now();
let timer: ReturnType<typeof setInterval> | undefined;
const listeners = new Set<() => void>();

function tick(): void {
  now = Date.now();
  for (const listener of [...listeners]) listener();
}

function onVisibility(): void {
  if (document.visibilityState === "visible") tick();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  if (listeners.size === 1) {
    now = Date.now();
    timer = setInterval(tick, TICK_MS);
    document.addEventListener("visibilitychange", onVisibility);
  }
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0) {
      clearInterval(timer);
      timer = undefined;
      document.removeEventListener("visibilitychange", onVisibility);
    }
  };
}

function getSnapshot(): number {
  return now;
}

/** Current time in epoch ms, refreshed every 15 seconds while mounted. */
export function useNow(): number {
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}
