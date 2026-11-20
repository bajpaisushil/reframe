import type { CSSProperties } from "react";
import { cx } from "@/lib/cx";
import styles from "./Skeleton.module.css";

export interface SkeletonProps {
  width?: number | string;
  height?: number | string;
  radius?: number | string;
  /** Text-line skeleton: rounded and slightly shorter than the line height. */
  text?: boolean;
  className?: string;
  style?: CSSProperties;
}

/** Placeholder shape while content loads (gentle pulse; static with reduced motion). */
export function Skeleton({ width, height = 14, radius, text = false, className, style }: SkeletonProps) {
  return (
    <span
      aria-hidden="true"
      className={cx(styles.skeleton, text && styles.text, className)}
      style={{ width, height, borderRadius: radius, ...style }}
    />
  );
}
