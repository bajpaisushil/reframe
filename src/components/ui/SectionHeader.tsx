import type { ReactNode } from "react";
import { ArrowRight } from "lucide-react";
import { cx } from "@/lib/cx";
import { formatCount } from "@/lib/format";
import { Link, type Route } from "@/lib/router";
import { Icon } from "./Icon";
import styles from "./SectionHeader.module.css";

export interface SectionHeaderProps {
  title: ReactNode;
  /** Optional count shown after the title in a quieter weight. */
  count?: number;
  description?: ReactNode;
  /** Controls aligned right (a "View all" link, a refresh button). */
  action?: ReactNode;
  as?: "h2" | "h3";
  id?: string;
  className?: string;
}

/** Heading for a page section: 16/600 title, optional count, description and action. */
export function SectionHeader({ title, count, description, action, as: Heading = "h2", id, className }: SectionHeaderProps) {
  return (
    <div className={cx(styles.header, className)}>
      <div className={styles.text}>
        <Heading id={id} className={styles.title}>
          {title}
          {count !== undefined && <span className={styles.count}>{formatCount(count)}</span>}
        </Heading>
        {description !== undefined && <p className={styles.description}>{description}</p>}
      </div>
      {action !== undefined && <div className={styles.action}>{action}</div>}
    </div>
  );
}

export interface SectionLinkProps {
  to: Route;
  children: ReactNode;
}

/** Quiet "View all →" link for a SectionHeader's action slot. */
export function SectionLink({ to, children }: SectionLinkProps) {
  return (
    <Link to={to} className={styles.link}>
      {children}
      <Icon icon={ArrowRight} size={14} />
    </Link>
  );
}
