import type { ButtonHTMLAttributes, Ref } from "react";
import { cx } from "@/lib/cx";
import { Icon, type LucideIcon } from "./Icon";
import { Spinner } from "./Spinner";
import styles from "./Button.module.css";

export type ButtonVariant = "primary" | "secondary" | "ghost" | "danger";
export type ButtonSize = "sm" | "md";

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
  size?: ButtonSize;
  /** Shows a spinner in place of the leading icon and blocks clicks. */
  loading?: boolean;
  /** Leading icon. */
  icon?: LucideIcon;
  /** Trailing icon (e.g. a chevron or arrow). */
  iconEnd?: LucideIcon;
  fullWidth?: boolean;
  ref?: Ref<HTMLButtonElement>;
}

export function Button({
  variant = "secondary",
  size = "md",
  loading = false,
  icon,
  iconEnd,
  fullWidth = false,
  disabled,
  className,
  children,
  type = "button",
  onClick,
  ref,
  ...rest
}: ButtonProps) {
  const iconSize = size === "sm" ? 14 : 16;
  return (
    <button
      ref={ref}
      type={type}
      className={cx(
        styles.button,
        styles[variant],
        styles[size],
        fullWidth && styles.fullWidth,
        loading && styles.loading,
        className,
      )}
      disabled={disabled}
      aria-busy={loading || undefined}
      aria-disabled={loading || undefined}
      onClick={loading ? (event) => event.preventDefault() : onClick}
      {...rest}
    >
      {loading ? (
        <Spinner size={iconSize === 14 ? 14 : 16} className={styles.icon} />
      ) : (
        icon && <Icon icon={icon} size={iconSize} className={styles.icon} />
      )}
      {children !== undefined && children !== null && <span className={styles.label}>{children}</span>}
      {iconEnd && !loading && <Icon icon={iconEnd} size={iconSize} className={cx(styles.icon, styles.iconEnd)} />}
    </button>
  );
}
