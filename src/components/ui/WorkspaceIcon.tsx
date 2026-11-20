import { normalizeHexColor } from "@/lib/color";
import { cx } from "@/lib/cx";
import { DEFAULT_WORKSPACE_EMOJI } from "@/lib/emoji";
import styles from "./WorkspaceIcon.module.css";

export type WorkspaceIconSize = 20 | 28 | 36 | 44 | 56;

export interface WorkspaceIconProps {
  emoji: string;
  /** Workspace accent color (`#RRGGBB`); null gives a neutral tile. */
  color: string | null;
  size?: WorkspaceIconSize;
  className?: string;
}

/** The workspace emoji on a square tinted with the workspace color (decorative). */
export function WorkspaceIcon({ emoji, color, size = 36, className }: WorkspaceIconProps) {
  const hex = normalizeHexColor(color);
  return (
    <span
      aria-hidden="true"
      className={cx(styles.tile, hex ? styles.tinted : styles.neutral, className)}
      style={{
        width: size,
        height: size,
        borderRadius: Math.round(size * 0.26),
        fontSize: Math.round(size * 0.5),
        ...(hex ? { ["--ws-color" as string]: hex } : {}),
      }}
    >
      <span className={cx("emoji", styles.emoji)}>{emoji.trim() || DEFAULT_WORKSPACE_EMOJI}</span>
    </span>
  );
}
