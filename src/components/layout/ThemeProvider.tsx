import { createContext, useCallback, useContext, useLayoutEffect, useMemo, type ReactNode } from "react";
import { useMediaQuery } from "@/hooks/useMediaQuery";
import { useSettings } from "@/hooks/useSettings";
import {
  DARK_SCHEME_QUERY,
  applyResolvedTheme,
  readStoredPreference,
  resolveTheme,
  storePreference,
  withThemeTransition,
  type ResolvedTheme,
} from "@/lib/theme";
import type { ThemePreference } from "@/types";

export interface ThemeContextValue {
  /** What the user chose: follow the system, or always light/dark. */
  preference: ThemePreference;
  /** What is on screen right now. */
  resolved: ResolvedTheme;
  /** Switch theme and persist it in the app settings. Rejects if saving failed. */
  setTheme: (preference: ThemePreference) => Promise<void>;
}

const ThemeContext = createContext<ThemeContextValue | null>(null);

/**
 * Applies the theme from `AppSettings.theme` (system / light / dark) to <html data-theme>, follows
 * the OS appearance while set to "system", and persists changes through `api.updateSettings`.
 * Until settings load (or if they cannot be loaded) the last known choice is used.
 */
export function ThemeProvider({ children }: { children: ReactNode }) {
  const { settings, update } = useSettings();
  const systemDark = useMediaQuery(DARK_SCHEME_QUERY);
  const preference: ThemePreference = settings?.theme ?? readStoredPreference() ?? "system";
  const resolved = resolveTheme(preference, systemDark);

  useLayoutEffect(() => {
    applyResolvedTheme(resolved);
  }, [resolved]);

  useLayoutEffect(() => {
    if (settings) storePreference(settings.theme);
  }, [settings]);

  // The settings cache updates optimistically, so the theme switches at once; if saving fails the
  // cache is reloaded from the backend and the effect above re-syncs the remembered choice.
  const setTheme = useCallback(
    async (next: ThemePreference) => {
      withThemeTransition();
      storePreference(next);
      await update({ theme: next });
    },
    [update],
  );

  const value = useMemo(() => ({ preference, resolved, setTheme }), [preference, resolved, setTheme]);
  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>;
}

export function useTheme(): ThemeContextValue {
  const context = useContext(ThemeContext);
  if (!context) throw new Error("useTheme must be used inside <ThemeProvider>");
  return context;
}
