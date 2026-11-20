// Small environment checks for UI conventions (shortcut labels, modifier keys).

/** True on macOS (and iPadOS), where shortcuts use ⌘ instead of Ctrl. */
export function isMacLike(): boolean {
  if (typeof navigator === "undefined") return false;
  const platform =
    (navigator as Navigator & { userAgentData?: { platform?: string } }).userAgentData?.platform ??
    navigator.platform ??
    "";
  return /mac|iphone|ipad|ipod/i.test(platform);
}

/** The primary shortcut modifier is held (⌘ on macOS, Ctrl elsewhere). */
export function hasPrimaryModifier(event: { metaKey: boolean; ctrlKey: boolean }): boolean {
  return isMacLike() ? event.metaKey : event.ctrlKey;
}

/** Label for the primary modifier in shortcut hints. */
export function primaryModifierLabel(): string {
  return isMacLike() ? "⌘" : "Ctrl";
}
