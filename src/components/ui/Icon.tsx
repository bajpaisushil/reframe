import type { LucideIcon, LucideProps } from "lucide-react";

export type { LucideIcon };

export interface IconProps extends Omit<LucideProps, "ref"> {
  icon: LucideIcon;
  /** 16 in controls (default), 18 in navigation. */
  size?: number;
}

/** A lucide icon with Reframe's stroke weight; decorative (hidden from assistive tech). */
export function Icon({ icon: Glyph, size = 16, strokeWidth = 1.75, ...rest }: IconProps) {
  return <Glyph size={size} strokeWidth={strokeWidth} aria-hidden="true" focusable="false" {...rest} />;
}
