import type { ReactNode } from "react";
import { ChevronLeft } from "lucide-react";
import { Icon } from "@/components/ui/Icon";
import { cx } from "@/lib/cx";
import { Link, type Route } from "@/lib/router";
import styles from "./Page.module.css";

export interface PageProps {
  children: ReactNode;
  /** `wide` drops the max width (rare: dense tables). */
  width?: "default" | "wide";
  className?: string;
}

/** Content column of a page: centered, max ~1180px, responsive side padding. */
export function Page({ children, width = "default", className }: PageProps) {
  return <div className={cx(styles.page, width === "wide" && styles.wide, className)}>{children}</div>;
}

export interface PageHeaderProps {
  title: ReactNode;
  /** One line under the title. */
  subtitle?: ReactNode;
  /** Primary action on the right (secondary actions before it). */
  actions?: ReactNode;
  /** Link back to the parent page ("Workspaces"). */
  back?: { label: string; to: Route };
  /** Visual before the title (e.g. a workspace icon). */
  leading?: ReactNode;
  className?: string;
}

/**
 * Page title row. The h1 is focusable so navigation can move focus to it (screen readers then
 * announce the new page).
 */
export function PageHeader({ title, subtitle, actions, back, leading, className }: PageHeaderProps) {
  return (
    <header className={cx(styles.header, className)}>
      {back && (
        <Link to={back.to} className={styles.back}>
          <Icon icon={ChevronLeft} size={16} />
          {back.label}
        </Link>
      )}
      <div className={styles.row}>
        {leading && <div className={styles.leading}>{leading}</div>}
        <div className={styles.titles}>
          <h1 className={styles.title} tabIndex={-1} data-page-title="true">
            {title}
          </h1>
          {subtitle && <p className={styles.subtitle}>{subtitle}</p>}
        </div>
        {actions && <div className={styles.actions}>{actions}</div>}
      </div>
    </header>
  );
}
