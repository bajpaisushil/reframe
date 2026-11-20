// Theme resolution and persistence helpers shared by main.tsx (first paint) and ThemeProvider.
//
// The source of truth is `AppSettings.theme` in the database. The last choice is mirrored in
// localStorage only so the very first paint after launch already uses it (no light flash for
// someone who picked dark); storage failures are ignored.

import type { ThemePreference } from "@/types";

export type ResolvedTheme = "light" | "dark";

const STORAGE_KEY = "reframe.theme";
export const DARK_SCHEME_QUERY = "(prefers-color-scheme: dark)";

export function isThemePreference(value: unknown): value is ThemePreference {
  return value === "system" || value === "light" || value === "dark";
}

export function resolveTheme(preference: ThemePreference, systemPrefersDark: boolean): ResolvedTheme {
  if (preference === "system") return systemPrefersDark ? "dark" : "light";
  return preference;
}

export function systemPrefersDark(): boolean {
  return (
    typeof window !== "undefined" &&
    typeof window.matchMedia === "function" &&
    window.matchMedia(DARK_SCHEME_QUERY).matches
  );
}

export function readStoredPreference(): ThemePreference | null {
  try {
    const value = window.localStorage.getItem(STORAGE_KEY);
    return isThemePreference(value) ? value : null;
  } catch {
    return null;
  }
}

export function storePreference(preference: ThemePreference): void {
  try {
    window.localStorage.setItem(STORAGE_KEY, preference);
  } catch {
    // Storage can be unavailable (private mode, blocked site data); the database still has it.
  }
}

/** Stamp the resolved theme on <html> (tokens.css keys off `data-theme`). */
export function applyResolvedTheme(theme: ResolvedTheme): void {
  const root = document.documentElement;
  if (root.dataset.theme !== theme) root.dataset.theme = theme;
  root.style.colorScheme = theme;
}

/** Apply the remembered (or system) theme before React renders. */
export function bootstrapTheme(): void {
  if (typeof document === "undefined") return;
  applyResolvedTheme(resolveTheme(readStoredPreference() ?? "system", systemPrefersDark()));
}

let transitionTimer: ReturnType<typeof setTimeout> | undefined;

/** Briefly animate colors while switching themes (skipped for reduced motion). */
export function withThemeTransition(): void {
  if (typeof window === "undefined") return;
  if (window.matchMedia?.("(prefers-reduced-motion: reduce)").matches) return;
  const root = document.documentElement;
  root.classList.add("theme-transition");
  clearTimeout(transitionTimer);
  transitionTimer = setTimeout(() => root.classList.remove("theme-transition"), 260);
}
