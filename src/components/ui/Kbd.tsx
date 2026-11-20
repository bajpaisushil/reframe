import type { ReactNode } from "react";
import { cx } from "@/lib/cx";
import styles from "./Kbd.module.css";

export interface KbdProps {
  children: ReactNode;
  /** `inverse` for use inside dark tooltips. */
  tone?: "default" | "inverse";
  className?: string;
}

/** A keyboard key or shortcut ("⌘K", "Esc"). */
export function Kbd({ children, tone = "default", className }: KbdProps) {
  return <kbd className={cx(styles.kbd, tone === "inverse" && styles.inverse, className)}>{children}</kbd>;
}
