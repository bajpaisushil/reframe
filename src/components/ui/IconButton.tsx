import type { ButtonHTMLAttributes, ReactNode, Ref } from "react";
import { cx } from "@/lib/cx";
import type { Placement } from "@/lib/position";
import { Icon, type LucideIcon } from "./Icon";
import { Kbd } from "./Kbd";
import { Spinner } from "./Spinner";
import { Tooltip } from "./Tooltip";
import styles from "./IconButton.module.css";

export interface IconButtonProps extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, "children" | "aria-label"> {
  icon: LucideIcon;
  /** Required: icon-only buttons need an accessible name. */
  "aria-label": string;
  variant?: "ghost" | "secondary";
  size?: "sm" | "md";
  /** `true` shows the aria-label as a tooltip; a node shows custom content. */
  tooltip?: boolean | ReactNode;
  tooltipPlacement?: Placement;
  /** Keyboard shortcut shown in the tooltip (e.g. "⌘R"). */
  shortcut?: string;
  loading?: boolean;
  /** Pressed/selected look for toggle buttons (pair with aria-pressed). */
  active?: boolean;
  ref?: Ref<HTMLButtonElement>;
}

export function IconButton({
  icon,
  variant = "ghost",
  size = "md",
  tooltip,
  tooltipPlacement = "top",
  shortcut,
  loading = false,
  active = false,
  className,
  type = "button",
  disabled,
  onClick,
  ref,
  ...rest
}: IconButtonProps) {
  const label = rest["aria-label"];
  const button = (
    <button
      ref={ref}
      type={type}
      className={cx(styles.button, styles[variant], styles[size], active && styles.active, className)}
      disabled={disabled}
      aria-busy={loading || undefined}
      onClick={loading ? (event) => event.preventDefault() : onClick}
      {...rest}
    >
      {loading ? <Spinner size={size === "sm" ? 14 : 16} /> : <Icon icon={icon} size={size === "sm" ? 15 : 16} />}
    </button>
  );
  if (!tooltip) return button;
  const content =
    tooltip === true ? (
      <span className={styles.tooltipContent}>
        {label}
        {shortcut && <Kbd tone="inverse">{shortcut}</Kbd>}
      </span>
    ) : (
      tooltip
    );
  return (
    <Tooltip content={content} placement={tooltipPlacement}>
      {button}
    </Tooltip>
  );
}
