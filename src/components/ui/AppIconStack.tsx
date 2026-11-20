import { cx } from "@/lib/cx";
import { formatList } from "@/lib/format";
import type { IconLookup } from "@/types";
import { AppIcon, type AppIconSize } from "./AppIcon";
import { Tooltip } from "./Tooltip";
import styles from "./AppIconStack.module.css";

export interface AppIconStackApp {
  lookup: IconLookup;
  name: string;
}

export interface AppIconStackProps {
  apps: readonly AppIconStackApp[];
  /** Total number of apps (may exceed `apps.length` when only the first few are known). */
  total?: number;
  max?: number;
  size?: AppIconSize;
  /** Show each app's name in a tooltip on hover. */
  showNames?: boolean;
  className?: string;
}

/** A row of app icons with a "+N" overflow chip; screen readers get the names as text. */
export function AppIconStack({ apps, total = apps.length, max = 6, size = 20, showNames = false, className }: AppIconStackProps) {
  const shown = apps.slice(0, max);
  const extra = Math.max(0, total - shown.length);
  const names = shown.map((app) => app.name);
  const summary = extra > 0 ? `${names.join(", ")} and ${extra} more` : formatList(names);

  return (
    <span className={cx(styles.stack, className)}>
      <span className="sr-only">{summary}</span>
      {shown.map((app, index) =>
        showNames ? (
          <Tooltip key={`${app.lookup.appId}-${index}`} content={app.name} delay={350}>
            <AppIcon lookup={app.lookup} name={app.name} size={size} />
          </Tooltip>
        ) : (
          <AppIcon key={`${app.lookup.appId}-${index}`} lookup={app.lookup} name={app.name} size={size} />
        ),
      )}
      {extra > 0 && (
        <span className={cx(styles.more, styles[`s${size}`])} aria-hidden="true">
          +{extra}
        </span>
      )}
    </span>
  );
}
