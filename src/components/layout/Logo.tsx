import { cx } from "@/lib/cx";
import styles from "./Logo.module.css";

export interface LogoProps {
  size?: number;
  className?: string;
  /** Accessible name; omit when a visible wordmark sits next to the mark. */
  title?: string;
}

/** The Reframe mark (same artwork as public/reframe.svg), inline so it renders instantly. */
export function Logo({ size = 24, className, title }: LogoProps) {
  return (
    <svg
      viewBox="0 0 1024 1024"
      width={size}
      height={size}
      className={cx(styles.logo, className)}
      role={title ? "img" : undefined}
      aria-label={title}
      aria-hidden={title ? undefined : true}
      focusable="false"
    >
      <rect width="1024" height="1024" rx="232" fill="#17171C" />
      <rect className={styles.ring} x="8" y="8" width="1008" height="1008" rx="224" fill="none" strokeWidth="16" />
      <g fill="none" stroke="#F3F2EE" strokeWidth="68" strokeLinecap="round" strokeLinejoin="round">
        <path d="M268 404V308a40 40 0 0 1 40-40h96" />
        <path d="M620 268h96a40 40 0 0 1 40 40v96" />
        <path d="M756 620v96a40 40 0 0 1-40 40h-96" />
        <path d="M404 756h-96a40 40 0 0 1-40-40v-96" />
      </g>
      <rect x="416" y="416" width="192" height="192" rx="44" fill="#6D5DFC" />
    </svg>
  );
}
