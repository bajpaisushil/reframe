import type { ReactNode } from "react";
import { CircleCheck, CircleExclamationPoint, Info, TriangleExclamationPoint } from "lucide-react";
import { Icon, type LucideIcon } from "@/components/ui";
import { cx } from "@/lib/cx";
import styles from "./Notice.module.css";

export type NoticeTone = "info" | "warning" | "danger" | "success";

const ICONS: Record<NoticeTone, LucideIcon> = {
  info: Info,
  warning: TriangleExclamationPoint,
  danger: CircleExclamationPoint,
  success: CircleCheck,
};

export interface NoticeProps {
  tone?: NoticeTone;
  title?: ReactNode;
  /** Body text, or a list of lines (shown as a list when there is more than one). */
  children?: ReactNode;
  lines?: readonly string[];
  icon?: LucideIcon;
  /** Controls on the right (a small button). */
  action?: ReactNode;
  size?: "md" | "sm";
  className?: string;
}

/**
 * An inline message inside a page or dialog: platform limitations, warnings, honest explanations.
 * Not a toast — it stays as long as the situation does.
 */
export function Notice({ tone = "info", title, children, lines, icon, action, size = "md", className }: NoticeProps) {
  const list = lines?.filter((line) => line.trim() !== "") ?? [];
  return (
    <div className={cx(styles.notice, styles[tone], styles[size], className)} role={tone === "danger" ? "alert" : undefined}>
      <Icon icon={icon ?? ICONS[tone]} size={16} className={styles.icon} />
      <div className={styles.body}>
        {title && <p className={styles.title}>{title}</p>}
        {children && <div className={styles.text}>{children}</div>}
        {list.length === 1 && <p className={styles.text}>{list[0]}</p>}
        {list.length > 1 && (
          <ul className={styles.list}>
            {list.map((line, index) => (
              <li key={`${index}-${line}`}>{line}</li>
            ))}
          </ul>
        )}
      </div>
      {action && <div className={styles.action}>{action}</div>}
    </div>
  );
}
