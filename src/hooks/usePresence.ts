import { useEffect, useState } from "react";

export interface Presence {
  /** Render the element (true while open and during the exit animation). */
  mounted: boolean;
  /** Closing: play the exit animation (`data-state="closed"`). */
  closing: boolean;
}

/**
 * Keep an overlay mounted for `durationMs` after it closes so it can animate out.
 * Pair with CSS keyed on `data-state`; with reduced motion the duration can be 0.
 */
export function usePresence(open: boolean, durationMs = 160): Presence {
  const [mounted, setMounted] = useState(open);
  // Derived state: opening mounts immediately during render (no extra frame).
  if (open && !mounted) setMounted(true);

  useEffect(() => {
    if (open || !mounted) return;
    const timer = setTimeout(() => setMounted(false), durationMs);
    return () => clearTimeout(timer);
  }, [open, mounted, durationMs]);

  return { mounted: open || mounted, closing: mounted && !open };
}
