import { useEffect, useState } from "react";
import { api } from "@/lib/api";
import { IconCache } from "@/lib/appIcons";
import { avatarColorIndex, initials } from "@/lib/color";
import { cx } from "@/lib/cx";
import type { IconLookup } from "@/types";
import styles from "./AppIcon.module.css";

export type AppIconSize = 20 | 28 | 36;

let cache: IconCache | null = null;

/** Session-wide icon cache backed by `api.appIcon` (created on first use). */
export function appIconCache(): IconCache {
  if (!cache) cache = new IconCache((lookup) => api.appIcon(lookup));
  return cache;
}

export interface AppIconProps {
  lookup: IconLookup;
  /** Display name: used for the fallback initials and as the accessible label. */
  name: string;
  size?: AppIconSize;
  /** Expose the app name to assistive tech (when no visible name sits next to the icon). */
  labelled?: boolean;
  className?: string;
}

/**
 * Application icon from the platform, cached for the session. Until it arrives — or when the app
 * has none — shows a rounded square with initials in a muted color derived from the app id.
 */
export function AppIcon({ lookup, name, size = 28, labelled = false, className }: AppIconProps) {
  const [icon, setIcon] = useState<string | null | undefined>(() => appIconCache().peek(lookup));
  const [broken, setBroken] = useState(false);
  const { appId, executablePath } = lookup;
  const packageId = lookup.package?.id;

  useEffect(() => {
    let cancelled = false;
    const iconCache = appIconCache();
    const known = iconCache.peek(lookup);
    setIcon(known);
    setBroken(false);
    if (known === undefined) {
      void iconCache.load(lookup).then((value) => {
        if (!cancelled) setIcon(value);
      });
    }
    return () => {
      cancelled = true;
    };
    // Re-run only when the identity of the icon changes, not on every new lookup object.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [appId, executablePath, packageId]);

  const a11y = labelled ? { role: "img", "aria-label": name } : { "aria-hidden": true };
  const radius = size === 20 ? 5 : size === 28 ? 7 : 9;

  if (icon && !broken) {
    return (
      <span className={cx(styles.icon, className)} style={{ width: size, height: size }} {...a11y}>
        <img src={icon} alt="" width={size} height={size} draggable={false} onError={() => setBroken(true)} />
      </span>
    );
  }

  const palette = avatarColorIndex(appId || name);
  return (
    <span
      className={cx(styles.fallback, styles[`s${size}`], className)}
      style={{
        width: size,
        height: size,
        borderRadius: radius,
        background: `var(--avatar-bg-${palette})`,
        color: `var(--avatar-fg-${palette})`,
      }}
      {...a11y}
    >
      {initials(name)}
    </span>
  );
}
